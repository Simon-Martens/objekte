//! Resident repository ownership and the independent scan execution path.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

use crate::bigram::BigramIndex;
use crate::inventory::{
    ByteRange, CollectionBuilder, CollectionGeneration, FileInventory, SourceRevision,
};
use crate::model::{CollectionId, FieldPath, FileId, RecordId, RecordSlot};
use crate::posting::PostingCollection;
use crate::query::Query;
use crate::reference::{
    RecordContext, Schema, Transform, bind_filter, data_values, eval_filter, eval_fts_query,
};
use crate::sorting::{SortBuilder, SortCollection};
use crate::source::{
    ExtractedRecord, SourceFormat, TextSegment, extract_file, read_record_range, reader,
    text_segments,
};
use crate::workload::{BigramAlphabet, ReturnMode, WorkCounters};
use unicode_casefold::UnicodeCaseFold;

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
    pub sort_fields: Vec<FieldPath>,
    pub sort_locale: String,
}

pub struct CollectionState {
    pub definition: CollectionDefinition,
    pub generation: CollectionGeneration,
    pub postings: PostingCollection,
    pub raw: BigramIndex,
    pub custom_text: HashMap<(Vec<String>, String), BigramIndex>,
    pub sorting: SortCollection,
    pub id_lookup: HashMap<RecordId, RecordSlot>,
    pub stats: CollectionStats,
}
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct CollectionStats {
    pub file_count: usize,
    pub record_count: usize,
    pub total_file_bytes: u64,
}
impl CollectionStats {
    pub fn mean_record_bytes(self) -> f64 {
        if self.record_count == 0 {
            0.0
        } else {
            self.total_file_bytes as f64 / self.record_count as f64
        }
    }
    pub fn mean_records_per_file(self) -> f64 {
        if self.file_count == 0 {
            0.0
        } else {
            self.record_count as f64 / self.file_count as f64
        }
    }
}
pub struct Repository {
    pub files: FileInventory,
    pub collections: Vec<CollectionState>,
    pub build_parses: u64,
    pub options: RepositoryOptions,
    pub startup: StartupStats,
    pub(crate) prepared: Mutex<crate::prepared::PreparedCache>,
    pub(crate) verified: Mutex<crate::verified::VerifiedCache>,
}

#[derive(Clone, Debug, Default)]
pub struct StartupStats {
    pub restored: bool,
    pub snapshot_decode_ns: u64,
    pub reconciliation_ns: u64,
    pub build_ns: u64,
    pub snapshot_write_ns: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotMode {
    Disabled,
    ReadWrite,
    Rebuild,
}

#[derive(Clone, Copy, Debug)]
pub struct RepositoryOptions {
    pub bigram_alphabet: BigramAlphabet,
    pub prepared_cache_bytes: usize,
    pub verified_cache_bytes: usize,
    pub snapshot_mode: SnapshotMode,
}
impl Default for RepositoryOptions {
    fn default() -> Self {
        Self {
            bigram_alphabet: BigramAlphabet::Ascii,
            prepared_cache_bytes: 4 * 1024 * 1024,
            verified_cache_bytes: 0,
            snapshot_mode: SnapshotMode::ReadWrite,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchHit {
    pub collection: String,
    pub record_id: String,
    pub generation: u64,
    pub file_id: u32,
    pub file_path: PathBuf,
    pub file_size: u64,
    pub file_created_ns: Option<i128>,
    pub byte_start: u64,
    pub byte_end: u64,
    pub modified_ns: i128,
    pub matched_fields: Vec<String>,
    pub payload: Option<Payload>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Payload {
    Bytes(Vec<u8>),
    Logical(crate::model::Value),
}
#[derive(Clone, Debug)]
pub struct SearchResult {
    pub count: usize,
    pub hits: Vec<SearchHit>,
    pub counters: WorkCounters,
}

impl Repository {
    pub fn search(&self, query: &Query) -> Result<SearchResult, String> {
        crate::run::search(self, query)
    }
    pub fn explain(&self, query: &Query) -> Result<Vec<crate::run::QueryPlanTrace>, String> {
        crate::run::explain(self, query)
    }
    pub fn explain_analyze(
        &self,
        query: &Query,
    ) -> Result<(SearchResult, Vec<crate::run::QueryPlanTrace>), String> {
        crate::run::search_analyze(self, query)
    }
    pub fn configure_verified_cache(&self, bytes: usize) -> Result<(), String> {
        self.verified
            .lock()
            .map_err(|e| e.to_string())?
            .set_budget(bytes);
        Ok(())
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
        let started = Instant::now();
        let root = root.into().canonicalize().map_err(|e| e.to_string())?;
        let snapshot = crate::storage::snapshot_path(&root, options.bigram_alphabet);
        let mut decode_ns = 0;
        let mut reconciliation_ns = 0;
        let mut previous_generations = HashMap::new();
        if options.snapshot_mode == SnapshotMode::ReadWrite && snapshot.is_file() {
            let decode_started = Instant::now();
            let loaded = crate::storage::load_snapshot(
                root.clone(),
                definitions.clone(),
                options,
                &snapshot,
            );
            decode_ns = decode_started.elapsed().as_nanos() as u64;
            if let Ok(mut repo) = loaded {
                let reconciliation_started = Instant::now();
                let current = crate::storage::sources_match(&repo)?;
                reconciliation_ns = reconciliation_started.elapsed().as_nanos() as u64;
                if current {
                    repo.startup = StartupStats {
                        restored: true,
                        snapshot_decode_ns: decode_ns,
                        reconciliation_ns,
                        build_ns: 0,
                        snapshot_write_ns: 0,
                    };
                    return Ok(repo);
                }
                for collection in repo.collections {
                    previous_generations
                        .insert(collection.definition.name, collection.generation.generation);
                }
            }
        }
        let mut repo = Self::build_fresh(root, definitions, options)?;
        for collection in &mut repo.collections {
            if let Some(previous) = previous_generations.get(&collection.definition.name) {
                collection.generation.generation =
                    previous.checked_add(1).ok_or("generation overflow")?;
            }
        }
        repo.startup = StartupStats {
            restored: false,
            snapshot_decode_ns: decode_ns,
            reconciliation_ns,
            build_ns: started.elapsed().as_nanos() as u64,
            snapshot_write_ns: 0,
        };
        if options.snapshot_mode != SnapshotMode::Disabled {
            let write_started = Instant::now();
            repo.save_snapshot(&snapshot)?;
            repo.startup.snapshot_write_ns = write_started.elapsed().as_nanos() as u64;
        }
        Ok(repo)
    }

    fn build_fresh(
        root: PathBuf,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
    ) -> Result<Self, String> {
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
        let mut shared_extractions: HashMap<FileId, Vec<(ByteRange, ExtractedRecord)>> =
            HashMap::new();
        for (definition, id, member_files) in selected {
            let mut builder =
                CollectionBuilder::new(id, definition.name.clone(), 1, files.files.len());
            let mut postings = PostingCollection::new(&definition.schema);
            let mut raw = BigramIndex::new(options.bigram_alphabet);
            let mut sort_builder = SortBuilder::new(
                &definition.sort_fields,
                &definition.schema,
                &definition.sort_locale,
            )?;
            let mut custom_text: HashMap<_, _> = definition
                .schema
                .custom
                .keys()
                .map(|key| (key.clone(), BigramIndex::new(options.bigram_alphabet)))
                .collect();
            for file_id in member_files {
                builder.include_file(file_id)?;
                let file = &files.files[file_id.0 as usize];
                let path = files.native_path(file_id).ok_or("missing file path")?;
                let mut consume = |range: ByteRange, mut extracted: ExtractedRecord| {
                    definition.schema.coerce_record(&mut extracted.value)?;
                    if definition.schema.has_temporal_fields() {
                        extracted.text = text_segments(&extracted.value);
                    }
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
                    sort_builder.add(
                        slot,
                        &extracted.value,
                        builder.record(slot).unwrap(),
                        file,
                        &root,
                        &definition.name,
                    )?;
                    for (key, index) in &mut custom_text {
                        let binding = &definition.schema.custom[key];
                        let mut segments = Vec::new();
                        if let Some(values) = data_values(&extracted.value, &key.0) {
                            for value in values {
                                let values: Vec<_> =
                                    if let crate::model::Value::Array(array) = value {
                                        array.values.iter().collect()
                                    } else {
                                        vec![value]
                                    };
                                for value in values {
                                    if let crate::model::Value::String(text) = value {
                                        let text = match binding.transform {
                                            Transform::Identity => text.clone(),
                                            Transform::CaseFold => text.case_fold().collect(),
                                        };
                                        segments.push(TextSegment {
                                            path: key.0.clone(),
                                            text,
                                        });
                                    }
                                }
                            }
                        }
                        index.add_record(slot, &segments)?;
                    }
                    Ok(())
                };
                if file.collections.len() > 1 {
                    if !shared_extractions.contains_key(&file_id) {
                        let mut extracted = Vec::new();
                        let stats = extract_file(
                            &path,
                            definition.format,
                            file.revision,
                            |range, record| {
                                extracted.push((range, record));
                                Ok(())
                            },
                        )?;
                        build_parses += stats.record_parses;
                        shared_extractions.insert(file_id, extracted);
                    }
                    for (range, record) in &shared_extractions[&file_id] {
                        consume(*range, record.clone())?;
                    }
                } else {
                    let stats =
                        extract_file(&path, definition.format, file.revision, &mut consume)?;
                    build_parses += stats.record_parses;
                }
            }
            let generation = builder.finish(&files)?;
            postings.finish_stats();
            postings.validate(generation.records.len())?;
            raw.validate(generation.records.len())?;
            for index in custom_text.values() {
                index.validate(generation.records.len())?;
            }
            let sorting = sort_builder.finish(&generation)?;
            let id_lookup = generation
                .records
                .iter()
                .map(|r| (r.id.clone(), r.slot))
                .collect();
            let stats = CollectionStats {
                file_count: generation.files.len(),
                record_count: generation.records.len(),
                total_file_bytes: generation
                    .files
                    .iter()
                    .map(|file| files.files[file.0 as usize].revision.size)
                    .sum(),
            };
            collections.push(CollectionState {
                definition,
                generation,
                postings,
                raw,
                custom_text,
                sorting,
                id_lookup,
                stats,
            });
        }
        Ok(Self {
            files,
            collections,
            build_parses,
            options,
            startup: StartupStats::default(),
            prepared: Mutex::new(crate::prepared::PreparedCache::new(
                options.prepared_cache_bytes,
            )),
            verified: Mutex::new(crate::verified::VerifiedCache::new(
                options.verified_cache_bytes,
            )),
        })
    }

    pub fn search_reference(&self, query: &Query) -> Result<SearchResult, String> {
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
            if let Some(fts) = &query.fts {
                for field in &fts.fields {
                    collection.definition.schema.field(&field.path)?;
                    if let Some(name) = &field.binding {
                        if name != "lower"
                            && !collection
                                .definition
                                .schema
                                .custom
                                .contains_key(&(field.path.segments.clone(), name.clone()))
                        {
                            return Err(format!("unknown FTS binding {name}"));
                        }
                    }
                }
            }
            bound.push((collection, filter));
        }
        if bound.is_empty() {
            return Err("unknown or unloaded collection scope".into());
        }
        validate_sort(self, query)?;
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
            preparation_ns: 0,
            planning_ns: 0,
            prepared_plan_hit: false,
            verified_result_hit: false,
        };
        for (collection, filter) in bound {
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
                let mut extracted = reader(collection.definition.format).parse(&bytes)?;
                collection
                    .definition
                    .schema
                    .coerce_record(&mut extracted.value)?;
                if collection.definition.schema.has_temporal_fields() {
                    extracted.text = text_segments(&extracted.value);
                }
                let ctx = RecordContext {
                    value: &extracted.value,
                    record,
                    file,
                    collection_name: &collection.definition.name,
                    repository_root: &self.files.root,
                };
                let mut evidence = HashSet::new();
                let fts_match = query
                    .fts
                    .as_ref()
                    .map(|fts| {
                        eval_fts_query(
                            fts,
                            &collection.definition.schema,
                            &extracted.text,
                            &ctx,
                            &mut evidence,
                        )
                    })
                    .transpose()?
                    .unwrap_or(true);
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
                    let payload = match query.options.return_mode {
                        ReturnMode::Locator => None,
                        ReturnMode::Bytes => Some(Payload::Bytes(bytes)),
                        ReturnMode::Logical => Some(Payload::Logical(extracted.value)),
                    };
                    hits.push((
                        collection.generation.collection.0 as usize,
                        record.slot.0,
                        SearchHit {
                            collection: collection.definition.name.clone(),
                            record_id: record.id.0.clone(),
                            generation: collection.generation.generation,
                            file_id: file.id.0,
                            file_path: file.relative_path.clone(),
                            file_size: file.revision.size,
                            file_created_ns: file.created_ns,
                            byte_start: record.range.start,
                            byte_end: record.range.end,
                            modified_ns: record.revision.modified_ns,
                            matched_fields,
                            payload,
                        },
                    ));
                }
            }
        }
        sort_rows(self, query, &mut hits);
        let count = hits.len();
        counters.total_matches = count as u64;
        let hits: Vec<_> = hits
            .into_iter()
            .skip(query.options.offset)
            .take(query.options.limit)
            .map(|(_, _, hit)| hit)
            .collect();
        counters.returned_records = hits.len() as u64;
        Ok(SearchResult {
            count,
            hits,
            counters,
        })
    }
}

pub(crate) fn validate_sort(repo: &Repository, query: &Query) -> Result<(), String> {
    for field in &query.options.sort {
        let mut expected = None;
        for collection in &repo.collections {
            if !query.collections.is_empty()
                && !query.collections.contains(&collection.definition.name)
            {
                continue;
            }
            let column = collection.sorting.column(&field.field)?;
            let definition = (&column.kind, &column.locale, &column.collation_version);
            if expected.as_ref().is_some_and(|prior| *prior != definition) {
                return Err(format!(
                    "incompatible cross-collection sort: {}",
                    field.field
                ));
            }
            expected = Some(definition);
        }
    }
    Ok(())
}

pub(crate) fn sort_rows(repo: &Repository, query: &Query, hits: &mut [(usize, u32, SearchHit)]) {
    hits.sort_by(|(a_collection, a_slot, a), (b_collection, b_slot, b)| {
        for field in &query.options.sort {
            let a_key = repo.collections[*a_collection]
                .sorting
                .key(&field.field, RecordSlot(*a_slot))
                .expect("sort validated");
            let b_key = repo.collections[*b_collection]
                .sorting
                .key(&field.field, RecordSlot(*b_slot))
                .expect("sort validated");
            let order = a_key.compare(b_key, field.descending);
            if !order.is_eq() {
                return order;
            }
        }
        (&a.collection, &a.record_id).cmp(&(&b.collection, &b.record_id))
    });
}
