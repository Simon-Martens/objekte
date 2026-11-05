//! Resident repository ownership and the independent scan execution path.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::bigram::BigramIndex;
use crate::inventory::{CollectionBuilder, CollectionGeneration, FileInventory, SourceRevision};
use crate::model::{CollectionId, RecordId};
use crate::posting::PostingCollection;
use crate::query::Query;
use crate::reference::{RecordContext, Schema, bind_filter, eval_filter, eval_fts};
use crate::source::{SourceFormat, extract_file, read_record_range, reader};
use crate::workload::{BigramAlphabet, ReturnMode, WorkCounters};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdentityPolicy {
    Filename,
    Generated,
}

#[derive(Clone, Debug)]
pub struct CollectionDefinition {
    pub name: String,
    pub source_path: PathBuf,
    pub format: SourceFormat,
    pub identity: IdentityPolicy,
    pub schema: Schema,
}

pub struct CollectionState {
    pub definition: CollectionDefinition,
    pub generation: CollectionGeneration,
    pub postings: PostingCollection,
    pub raw: BigramIndex,
}
pub struct Repository {
    pub files: FileInventory,
    pub collections: Vec<CollectionState>,
    pub build_parses: u64,
    pub options: RepositoryOptions,
}

#[derive(Clone, Copy, Debug)]
pub struct RepositoryOptions {
    pub bigram_alphabet: BigramAlphabet,
}
impl Default for RepositoryOptions {
    fn default() -> Self {
        Self {
            bigram_alphabet: BigramAlphabet::Ascii,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SearchHit {
    pub collection: String,
    pub record_id: String,
    pub file_path: PathBuf,
    pub byte_start: u64,
    pub byte_end: u64,
    pub modified_ns: i128,
    pub matched_fields: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct SearchResult {
    pub count: usize,
    pub hits: Vec<SearchHit>,
    pub counters: WorkCounters,
}

impl Repository {
    pub fn search(&self, query: &Query) -> Result<SearchResult, String> {
        crate::executor::search(self, query)
    }
    pub fn open(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
    ) -> Result<Self, String> {
        Self::open_with_options(root, definitions, RepositoryOptions::default())
    }
    pub fn open_with_options(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
    ) -> Result<Self, String> {
        let root = root.into().canonicalize().map_err(|e| e.to_string())?;
        let mut files = FileInventory::new(root.clone());
        let mut selected = Vec::new();
        for (collection_index, definition) in definitions.into_iter().enumerate() {
            let id =
                CollectionId(u32::try_from(collection_index).map_err(|_| "too many collections")?);
            if selected.iter().any(
                |(previous, _, _): &(CollectionDefinition, CollectionId, Vec<_>)| {
                    previous.name == definition.name
                },
            ) {
                return Err(format!("duplicate collection: {}", definition.name));
            }
            let directory = root.join(&definition.source_path);
            let mut paths = Vec::new();
            for entry in std::fs::read_dir(&directory)
                .map_err(|e| format!("{}: {e}", directory.display()))?
            {
                let entry = entry.map_err(|e| e.to_string())?;
                let kind = entry.file_type().map_err(|e| e.to_string())?;
                if !kind.is_file() || kind.is_symlink() {
                    continue;
                }
                let path = entry.path();
                let extension = match definition.format {
                    SourceFormat::Json => "json",
                    SourceFormat::JsonLines => "jsonl",
                };
                if path.extension().is_none_or(|e| e != extension) {
                    continue;
                }
                paths.push(path);
            }
            paths.sort();
            let mut member_files = Vec::new();
            for path in paths {
                let relative = path
                    .strip_prefix(&root)
                    .map_err(|e| e.to_string())?
                    .to_owned();
                let metadata = std::fs::metadata(&path).map_err(|e| e.to_string())?;
                let revision = SourceRevision::from_metadata(&metadata)?;
                let created_ns = metadata
                    .created()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos() as i128);
                let file = files.add(relative, revision, created_ns, id)?;
                member_files.push(file);
            }
            selected.push((definition, id, member_files));
        }
        let mut collections = Vec::new();
        let mut build_parses = 0;
        for (definition, id, member_files) in selected {
            let mut builder =
                CollectionBuilder::new(id, definition.name.clone(), 1, files.files.len());
            let mut postings = PostingCollection::new(&definition.schema);
            let mut raw = BigramIndex::new(options.bigram_alphabet);
            for file_id in member_files {
                builder.include_file(file_id)?;
                let file = &files.files[file_id.0 as usize];
                let path = files.native_path(file_id).ok_or("missing file path")?;
                let stats = extract_file(&path, definition.format, |range, extracted| {
                    let record_id = match definition.identity {
                        IdentityPolicy::Filename => {
                            let stem = path
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .ok_or("non-UTF-8 filename identity")?;
                            RecordId::intrinsic(stem)?
                        }
                        IdentityPolicy::Generated => {
                            RecordId::generated(file_id, file.revision.modified_ns, range.start)
                        }
                    };
                    let slot = builder.add_record(file, record_id, range.start, range.end)?;
                    postings.add(slot, &extracted.value)?;
                    raw.add_record(slot, &extracted.text)?;
                    Ok(())
                })?;
                build_parses += stats.record_parses;
            }
            let generation = builder.finish(&files)?;
            postings.validate(generation.records.len())?;
            raw.validate(generation.records.len())?;
            collections.push(CollectionState {
                definition,
                generation,
                postings,
                raw,
            });
        }
        Ok(Self {
            files,
            collections,
            build_parses,
            options,
        })
    }

    pub fn search_reference(&self, query: &Query) -> Result<SearchResult, String> {
        if !query.options.sort.is_empty() {
            return Err("sorting is not implemented in the reference CLI stage".into());
        }
        if query.options.return_mode != ReturnMode::Locator {
            return Err("payload modes are not implemented in the reference CLI stage".into());
        }
        let mut bound = Vec::new();
        for collection in &self.collections {
            if !query.collections.is_empty()
                && !query.collections.contains(&collection.definition.name)
            {
                continue;
            }
            let filter = query
                .filter
                .as_ref()
                .map(|f| bind_filter(f, &collection.definition.schema))
                .transpose()?;
            let mut fields = Vec::new();
            if let Some(fts) = &query.fts {
                for field in &fts.fields {
                    if field.binding.is_some() {
                        return Err("selected custom FTS binding is not implemented in the reference CLI stage".into());
                    }
                    collection.definition.schema.field(&field.path)?;
                    fields.push(field.path.clone());
                }
            }
            bound.push((collection, filter, fields));
        }
        if bound.is_empty() {
            return Err("unknown or unloaded collection scope".into());
        }
        let mut hits = Vec::new();
        let mut counters = WorkCounters {
            exact_posting_matches: 0,
            candidates: 0,
            scanned_records: 0,
            total_matches: 0,
            returned_records: 0,
            files_opened: 0,
            bytes_read: 0,
            record_parses: 0,
            residual_evaluations: 0,
            prepared_plan_hit: false,
            verified_result_hit: false,
        };
        for (collection, filter, fields) in bound {
            for record in &collection.generation.records {
                let file = &self.files.files[record.file.0 as usize];
                let path = self
                    .files
                    .native_path(record.file)
                    .ok_or("missing source path")?;
                let bytes = read_record_range(&path, record.revision, record.range)?;
                counters.files_opened += 1;
                counters.bytes_read += bytes.len() as u64;
                counters.record_parses += 1;
                counters.scanned_records += 1;
                let extracted = reader(collection.definition.format).parse(&bytes)?;
                let ctx = RecordContext {
                    value: &extracted.value,
                    record,
                    file,
                    collection_name: &collection.definition.name,
                    repository_root: &self.files.root,
                };
                let mut evidence = HashSet::new();
                let fts_match = query.fts.as_ref().is_none_or(|fts| {
                    eval_fts(&fts.literal, &fields, &extracted.text, &mut evidence)
                });
                let filter_match = filter
                    .as_ref()
                    .map(|f| eval_filter(f, &ctx, &mut evidence))
                    .transpose()?
                    .unwrap_or(true);
                counters.residual_evaluations += u64::from(filter.is_some());
                if fts_match && filter_match {
                    let mut matched_fields: Vec<_> = if query.options.evidence {
                        evidence.into_iter().collect()
                    } else {
                        Vec::new()
                    };
                    matched_fields.sort();
                    hits.push(SearchHit {
                        collection: collection.definition.name.clone(),
                        record_id: record.id.0.clone(),
                        file_path: file.relative_path.clone(),
                        byte_start: record.range.start,
                        byte_end: record.range.end,
                        modified_ns: record.revision.modified_ns,
                        matched_fields,
                    });
                }
            }
        }
        hits.sort_by(|a, b| (&a.collection, &a.record_id).cmp(&(&b.collection, &b.record_id)));
        let count = hits.len();
        counters.total_matches = count as u64;
        let hits: Vec<_> = hits
            .into_iter()
            .skip(query.options.offset)
            .take(query.options.limit)
            .collect();
        counters.returned_records = hits.len() as u64;
        Ok(SearchResult {
            count,
            hits,
            counters,
        })
    }
}
