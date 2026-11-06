//! Resident repository ownership and the independent scan execution path.

use roaring::RoaringBitmap;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crate::bigram::BigramIndex;
use crate::discovery::{Discovery, discover};
use crate::inventory::{
    CollectionBuilder, CollectionGeneration, DiagnosticRef, FileInventory, FileStatus,
    RecordClass, SourceRevision,
};
use crate::model::{ArrayValue, CollectionId, FieldPath, RecordId, RecordSlot, Value};
use crate::posting::{CustomPostingIndex, PostingCollection, PostingIndex};
use crate::query::Query;
use crate::record_preparation::{materialize, prepare_query_record};
use crate::reference::{
    BindingOperation, RecordContext, Schema, bind_filter, bind_fts, eval_bound_fts,
    eval_filter, eval_unavailable_filter,
};
use crate::sorting::{SortBuilder, SortCollection, SortConfig, SortOptions};
use crate::source::{PreparedSource, ScanProjection, SourceFormat, SourceRegistry, read_record_range};
use crate::workload::{BigramAlphabet, WorkCounters};

static NEXT_REPOSITORY_INSTANCE: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_repository_instance() -> u64 {
    NEXT_REPOSITORY_INSTANCE.fetch_add(1, Ordering::Relaxed)
}


#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdentityPolicy {
    Filename,
    Generated,
    Intrinsic(Vec<String>),
}


pub(crate) fn map_binding_value(operation: &dyn BindingOperation, value: &Value) -> Option<Value> {
    if let Value::Array(array) = value {
        let values = array
            .values
            .iter()
            .map(|item| map_binding_value(operation, item))
            .collect::<Option<Vec<_>>>()?;
        return Some(Value::Array(
            ArrayValue::new(values, None, array.nullable_elements).ok()?,
        ));
    }
    operation.map_value(value)
}

pub(crate) fn string_values(value: &Value) -> impl Iterator<Item = &str> {
    let single = match value {
        Value::String(text) => Some(text.as_str()),
        _ => None,
    };
    let array = match value {
        Value::Array(array) => Some(array),
        _ => None,
    };
    single.into_iter().chain(
        array
            .into_iter()
            .flat_map(|array| array.values.iter())
            .filter_map(|value| match value {
                Value::String(text) => Some(text.as_str()),
                _ => None,
            }),
    )
}


/// Caller-supplied source, schema, identity, and sort configuration.
#[derive(Clone, Debug)]
pub struct CollectionDefinition {
    pub name: String,
    pub source_path: PathBuf,
    pub format: SourceFormat,
    pub identity: IdentityPolicy,
    pub discovery: Discovery,
    pub schema: Schema,
    pub sort_fields: Vec<FieldPath>,
    pub sort_locale: String,
    pub sort_options: SortOptions,
    pub sort_overrides: HashMap<FieldPath, SortConfig>,
}


/// Published generation and indexes for one collection.
pub struct CollectionState {
    pub definition: CollectionDefinition,
    pub(crate) source: PreparedSource,
    pub generation: CollectionGeneration,
    pub postings: PostingCollection,
    pub custom_postings: HashMap<(Vec<String>, String), CustomPostingIndex>,
    pub raw: BigramIndex,
    pub fallback_text: BigramIndex,
    pub logical_records: RoaringBitmap,
    pub custom_text: HashMap<(Vec<String>, String), BigramIndex>,
    pub sorting: SortCollection,
    pub id_lookup: HashMap<RecordId, RecordSlot>,
    pub identity_order: Vec<RecordSlot>,
    pub stats: CollectionStats,
}


/// Built-in identity and case-fold registrations for the same logical field
/// have identical candidate grams after mandatory index folding.
pub(crate) fn canonical_custom_text_key(
    schema: &Schema,
    path: &[String],
    name: &str,
) -> (Vec<String>, String) {
    let canonical = if schema
        .custom
        .contains_key(&(path.to_vec(), name.to_owned()))
    {
        schema
            .custom
            .keys()
            .filter(|(candidate_path, _)| candidate_path == path)
            .map(|(_, candidate_name)| candidate_name.as_str())
            .min()
            .unwrap_or(name)
    } else {
        name
    };
    (path.to_vec(), canonical.to_owned())
}

pub(crate) fn built_in_field_text_key(
    schema: &Schema,
    path: &[String],
) -> Option<(Vec<String>, String)> {
    let name = schema
        .custom
        .keys()
        .filter(|(candidate_path, _)| candidate_path == path)
        .map(|(_, name)| name.as_str())
        .min()?;
    Some((path.to_vec(), name.to_owned()))
}


/// Collection size summary used by query planning.
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


/// Resident owner of file inventory, collections, workers, and caches.
///
/// Published structural state is only available through shared references.
/// A future whole-file replacement must build new generations and publish them
/// together with its file-to-record mapping; it cannot edit a published view.
///
/// ```compile_fail
/// use objekte_search_core::repository::Repository;
/// fn mutate_published(repo: &mut Repository) {
///     repo.collections.clear();
/// }
/// ```
pub struct Repository {
    pub(crate) files: FileInventory,
    pub(crate) collections: Vec<CollectionState>,
    pub(crate) sources: SourceRegistry,
    pub(crate) build_parses: u64,
    pub(crate) build_failed_parses: u64,
    pub(crate) instance_id: u64,
    pub(crate) options: RepositoryOptions,
    pub(crate) startup: StartupStats,
    pub(crate) workers: rayon::ThreadPool,
    pub(crate) prepared: Mutex<crate::prepared::PreparedCache>,
    pub(crate) verified: Mutex<crate::verified::VerifiedCache>,
}


/// Time spent restoring, reconciling, building, and writing state.
#[derive(Clone, Debug, Default)]
pub struct StartupStats {
    pub restored: bool,
    pub snapshot_decode_ns: u64,
    pub reconciliation_ns: u64,
    pub build_ns: u64,
    pub build_discovery_ns: u64,
    pub build_extraction_index_ns: u64,
    pub build_finalization_ns: u64,
    pub snapshot_write_ns: u64,
}


#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotMode {
    Disabled,
    ReadWrite,
    Rebuild,
}


/// Index alphabet, snapshot, cache, and worker settings.
#[derive(Clone, Copy, Debug)]
pub struct RepositoryOptions {
    pub bigram_alphabet: BigramAlphabet,
    pub prepared_cache_bytes: usize,
    pub verified_cache_bytes: usize,
    pub snapshot_mode: SnapshotMode,
    pub query_workers: usize,
}


impl Default for RepositoryOptions {
    fn default() -> Self {
        Self {
            bigram_alphabet: BigramAlphabet::Ascii,
            prepared_cache_bytes: 4 * 1024 * 1024,
            verified_cache_bytes: 0,
            snapshot_mode: SnapshotMode::ReadWrite,
            query_workers: std::thread::available_parallelism()
                .map_or(1, |workers| workers.get().min(32)),
        }
    }
}


/// One page result with locator, evidence, and optional payload.
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
    pub file_status: FileStatus,
    pub record_class: RecordClass,
    pub diagnostic_ref: DiagnosticRef,
    pub payload: Option<Payload>,
}


#[derive(Clone, Debug, PartialEq)]
pub enum Payload {
    Bytes(Vec<u8>),
    Logical(crate::model::Value),
    LogicalUnavailable { diagnostic_ref: DiagnosticRef },
}


/// Requested exact match count, returned page, and measured work.
#[derive(Clone, Debug)]
pub struct SearchResult {
    pub count: Option<usize>,
    pub hits: Vec<SearchHit>,
    pub counters: WorkCounters,
}


impl Repository {
    pub fn files(&self) -> &FileInventory {
        &self.files
    }

    pub fn collections(&self) -> &[CollectionState] {
        &self.collections
    }

    pub fn build_parses(&self) -> u64 {
        self.build_parses
    }

    pub fn build_failed_parses(&self) -> u64 {
        self.build_failed_parses
    }

    /// Opening configuration. The verified cache budget may subsequently be
    /// changed through `configure_verified_cache`.
    pub fn options(&self) -> RepositoryOptions {
        self.options
    }

    pub fn verified_cache_budget(&self) -> Result<usize, String> {
        Ok(self.verified.lock().map_err(|e| e.to_string())?.budget())
    }

    pub fn startup(&self) -> &StartupStats {
        &self.startup
    }

    pub(crate) fn create_workers(count: usize) -> Result<rayon::ThreadPool, String> {
        if count == 0 || count > 256 {
            return Err("query_workers must be between 1 and 256".into());
        }
        rayon::ThreadPoolBuilder::new()
            .num_threads(count)
            .thread_name(|index| format!("objekte-query-{index}"))
            .build()
            .map_err(|error| error.to_string())
    }

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
        Self::open_with_sources(root, definitions, options, SourceRegistry::builtin())
    }

    pub fn open_with_sources(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        sources: SourceRegistry,
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
                &sources,
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
                        ..StartupStats::default()
                    };
                    return Ok(repo);
                }
                for collection in repo.collections {
                    previous_generations
                        .insert(collection.definition.name, collection.generation.generation);
                }
            }
        }
        let mut repo = Self::build_fresh(root, definitions, options, &sources)?;
        for collection in &mut repo.collections {
            if let Some(previous) = previous_generations.get(&collection.definition.name) {
                collection.generation.generation =
                    previous.checked_add(1).ok_or("generation overflow")?;
            }
        }
        repo.startup.snapshot_decode_ns = decode_ns;
        repo.startup.reconciliation_ns = reconciliation_ns;
        repo.startup.build_ns = started.elapsed().as_nanos() as u64;
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
        sources: &SourceRegistry,
    ) -> Result<Self, String> {
        let discovery_started = Instant::now();
        let mut files = FileInventory::new(root.clone());
        let mut selected = Vec::new();
        for (collection_index, definition) in definitions.into_iter().enumerate() {
            let source = sources.resolve(&definition.format)?;
            let id =
                CollectionId(u32::try_from(collection_index).map_err(|_| "too many collections")?);
            if selected.iter().any(
                |(previous, _, _, _): &(
                    CollectionDefinition,
                    CollectionId,
                    PreparedSource,
                    Vec<_>,
                )| { previous.name == definition.name },
            ) {
                return Err(format!("duplicate collection: {}", definition.name));
            }
            let paths = discover(
                &root,
                &definition.source_path,
                &definition.discovery,
                &source,
            )?;
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
            selected.push((definition, id, source, member_files));
        }
        let discovery_ns = discovery_started.elapsed().as_nanos() as u64;
        let mut collections = Vec::new();
        let mut build_parses = 0;
        let mut build_failed_parses = 0;
        let mut extraction_index_ns = 0u64;
        let mut finalization_ns = 0u64;
        for (definition, id, source, member_files) in selected {
            let extraction_started = Instant::now();
            let mut builder =
                CollectionBuilder::new(id, definition.name.clone(), 1, files.files.len());
            let mut postings = PostingCollection::new(&definition.schema);
            let mut custom_postings: HashMap<_, _> = definition
                .schema
                .operations
                .iter()
                .filter(|(_, binding)| binding.operation.posting())
                .map(|(key, binding)| {
                    (
                        key.clone(),
                        CustomPostingIndex {
                            index: PostingIndex::new(&definition.schema.fields[&key.0]),
                            complete: binding.operation.complete_coverage(),
                        },
                    )
                })
                .collect();
            let mut raw = BigramIndex::new(options.bigram_alphabet);
            let mut fallback_text = BigramIndex::new(options.bigram_alphabet);
            let mut logical_records = RoaringBitmap::new();
            let mut sort_builder = SortBuilder::new(
                &definition.sort_fields,
                &definition.schema,
                &definition.sort_locale,
                definition.sort_options,
                &definition.sort_overrides,
            )?;
            let mut custom_text: HashMap<_, _> = definition
                .schema
                .custom
                .keys()
                .chain(
                    definition
                        .schema
                        .operations
                        .iter()
                        .filter(|(_, binding)| {
                            binding.operation.bigram() && binding.operation.complete_coverage()
                        })
                        .map(|(key, _)| key),
                )
                .map(|key| {
                    (
                        canonical_custom_text_key(&definition.schema, &key.0, &key.1),
                        BigramIndex::new(options.bigram_alphabet),
                    )
                })
                .collect();
            for file_ids in crate::build::file_batches(&member_files, &files) {
                let batch = crate::build::process_batch(
                    &root, &definition, id, &source, file_ids, &files,
                    options.bigram_alphabet,
                )?;
                build_parses += batch.record_parses;
                build_failed_parses += batch.failed_parses;
                crate::build::CollectionAdmission {
                    inventory: &mut files,
                    builder: &mut builder,
                    postings: &mut postings,
                    custom_postings: &mut custom_postings,
                    raw: &mut raw,
                    fallback_text: &mut fallback_text,
                    custom_text: &mut custom_text,
                    logical_records: &mut logical_records,
                    sorting: &mut sort_builder,
                }.admit(batch)?;
            }
            extraction_index_ns += extraction_started.elapsed().as_nanos() as u64;
            let finalization_started = Instant::now();
            let generation = builder.finish(&files)?;
            let all_logical = logical_records.len() == generation.records.len() as u64;
            postings.finish_stats();
            postings.validate_eligible(generation.records.len(), logical_records.len() as usize)?;
            if !all_logical {
                postings.validate_domain(&logical_records)?;
            }
            for custom in custom_postings.values_mut() {
                custom.index.finish_stats();
                custom
                    .index
                    .validate_eligible(generation.records.len(), logical_records.len() as usize)?;
                if !all_logical {
                    custom.index.validate_domain(&logical_records)?;
                }
            }
            raw.validate(generation.records.len())?;
            if !all_logical {
                raw.validate_domain(&logical_records)?;
            }
            fallback_text.validate(generation.records.len())?;
            if all_logical {
                fallback_text.validate_domain(&RoaringBitmap::new())?;
            } else if !logical_records.is_empty() {
                let fallback_slots: RoaringBitmap = generation
                    .records
                    .iter()
                    .filter(|record| record.class == RecordClass::RawFallback)
                    .map(|record| record.slot.0)
                    .collect();
                fallback_text.validate_domain(&fallback_slots)?;
            }
            for index in custom_text.values() {
                index.validate(generation.records.len())?;
                if !all_logical {
                    index.validate_domain(&logical_records)?;
                }
            }
            let sorting = sort_builder.finish(&generation)?;
            let id_lookup = generation
                .records
                .iter()
                .map(|r| (r.id.clone(), r.slot))
                .collect();
            let mut identity_order = generation
                .records
                .iter()
                .map(|r| r.slot)
                .collect::<Vec<_>>();
            identity_order.sort_by(|a, b| {
                generation.records[a.0 as usize]
                    .id
                    .cmp(&generation.records[b.0 as usize].id)
            });
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
                source,
                generation,
                postings,
                custom_postings,
                raw,
                fallback_text,
                logical_records,
                custom_text,
                sorting,
                id_lookup,
                identity_order,
                stats,
            });
            finalization_ns += finalization_started.elapsed().as_nanos() as u64;
        }
        for state in &mut collections {
            state.sorting.refresh_file_status(&state.generation, &files);
        }
        files.validate_diagnostics()?;
        Ok(Self {
            files,
            collections,
            sources: sources.clone(),
            build_parses,
            build_failed_parses,
            instance_id: next_repository_instance(),
            options,
            startup: StartupStats {
                build_discovery_ns: discovery_ns,
                build_extraction_index_ns: extraction_index_ns,
                build_finalization_ns: finalization_ns,
                ..StartupStats::default()
            },
            workers: Self::create_workers(options.query_workers)?,
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
            let fts = query
                .fts
                .as_ref()
                .map(|fts| bind_fts(fts, &collection.definition.schema))
                .transpose()?;
            bound.push((collection, filter, fts));
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
            match_count_complete: false,
            returned_records: 0,
            files_opened: 0,
            distinct_files: 0,
            bytes_read: 0,
            peak_retained_payload_bytes: 0,
            record_parses: 0,
            raw_text_verifications: 0,
            residual_evaluations: 0,
            preparation_ns: 0,
            planning_ns: 0,
            prepared_plan_hit: false,
            verified_result_hit: false,
        };
        let mut files_read = HashSet::new();
        for (collection, filter, fts) in bound {
            for record in &collection.generation.records {
                files_read.insert(record.file.0);
                let file = &self.files.files[record.file.0 as usize];
                let path = self
                    .files
                    .native_path(record.file)
                    .ok_or("missing source path")?;
                let bytes = read_record_range(&path, record.revision, record.range)?;
                counters.files_opened += 1;
                counters.bytes_read += bytes.len() as u64;
                counters.record_parses += u64::from(record.class == RecordClass::Logical);
                counters.scanned_records += 1;
                let extracted = if record.class == RecordClass::Logical {
                    prepare_query_record(
                        &collection.source,
                        &collection.definition.schema,
                        &bytes,
                        &ScanProjection::full(true),
                        true,
                    )?
                } else {
                    crate::source::ExtractedRecord {
                        value: Value::Object(Default::default()),
                        text: Vec::new(),
                    }
                };
                let ctx = RecordContext {
                    value: &extracted.value,
                    source_bytes: Some(&bytes),
                    record,
                    file,
                    collection_name: &collection.definition.name,
                    repository_root: &self.files.root,
                };
                let mut evidence = HashSet::new();
                let fts_match = if record.class == RecordClass::RawFallback {
                    if let Some(fts) = &fts {
                        if fts.fields.is_empty() {
                            counters.raw_text_verifications += 1;
                            let matched = crate::reference::raw_fallback_contains(
                                fts,
                                &String::from_utf8_lossy(&bytes),
                            );
                            if matched {
                                evidence.insert("@record.raw_text".into());
                            }
                            matched
                        } else {
                            eval_bound_fts(fts, &[], &ctx, &mut evidence)?
                        }
                    } else {
                        true
                    }
                } else {
                    fts.as_ref()
                        .map(|fts| eval_bound_fts(fts, &extracted.text, &ctx, &mut evidence))
                        .transpose()?
                        .unwrap_or(true)
                };
                let filter_match = if record.class == RecordClass::RawFallback {
                    filter
                        .as_ref()
                        .map(|f| eval_unavailable_filter(f, &ctx, &mut evidence))
                        .transpose()?
                        .flatten()
                        == Some(true)
                        || filter.is_none()
                } else {
                    filter
                        .as_ref()
                        .map(|f| eval_filter(f, &ctx, &mut evidence))
                        .transpose()?
                        .unwrap_or(true)
                };
                counters.residual_evaluations += u64::from(filter.is_some());
                if fts_match && filter_match {
                    let mut matched_fields: Vec<_> = if query.options.evidence {
                        evidence.into_iter().collect()
                    } else {
                        Vec::new()
                    };
                    matched_fields.sort();
                    let payload = if record.class == RecordClass::RawFallback
                        && query.options.return_mode == crate::workload::ReturnMode::Logical
                    {
                        Some(Payload::LogicalUnavailable {
                            diagnostic_ref: record.diagnostic_ref,
                        })
                    } else {
                        materialize(&query.options.return_mode, bytes, Some(extracted))?
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
                            file_status: file.status,
                            record_class: record.class,
                            diagnostic_ref: record.diagnostic_ref,
                            payload,
                        },
                    ));
                }
            }
        }
        counters.distinct_files = files_read.len() as u64;
        sort_rows(self, query, &mut hits);
        let count = hits.len();
        counters.total_matches = count as u64;
        counters.match_count_complete = true;
        let hits: Vec<_> = hits
            .into_iter()
            .skip(query.options.offset)
            .take(query.options.limit)
            .map(|(_, _, hit)| hit)
            .collect();
        counters.returned_records = hits.len() as u64;
        Ok(SearchResult {
            count: query.options.count.then_some(count),
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
            let definition = (
                &column.kind,
                &column.locale,
                &column.options,
                &column.collation_version,
            );
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
            let options = repo.collections[*a_collection]
                .sorting
                .column(&field.field)
                .expect("sort validated")
                .options;
            let order = a_key.compare_with(b_key, field.descending, options);
            if !order.is_eq() {
                return order;
            }
        }
        (&a.collection, &a.record_id).cmp(&(&b.collection, &b.record_id))
    });
}
