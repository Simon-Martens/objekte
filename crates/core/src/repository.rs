//! Resident repository ownership and the independent scan execution path.

use rayon::prelude::*;
use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use crate::bigram::{BigramContribution, BigramIndex};
use crate::discovery::{Discovery, discover};
use crate::inference::{
    InferenceBuildStats, InferenceBuilder, InferenceConfig, InferencePathDiagnostic,
    InferenceReport, InferenceState,
};
use crate::inventory::{
    ActiveRecords, CollectionBuilder, CollectionGeneration, DiagnosticRef, FileInventory,
    FileStatus, RecordClass, SourceRevision,
};
use crate::model::{ArrayValue, CollectionId, FieldPath, RecordId, RecordSlot, Value};
use crate::posting::{CustomPostingIndex, PostingCollection, PostingIndex};
use crate::query::Query;
use crate::record_preparation::{materialize, prepare_query_record};
use crate::reference::{BindingOperation, RecordContext, Schema, eval_unavailable_filter};
use crate::relation::{RelationContribution, RelationIndex};
use crate::sorting::{SortBuilder, SortCollection, SortConfig, SortOptions};
use crate::source::{
    PreparedSource, ScanProjection, SourceFormat, SourceRegistry, read_record_range_for_search,
};
use crate::source_access::SourceAccessSession;
use crate::workload::{BigramAlphabet, ReturnMode, WorkCounters};

static NEXT_REPOSITORY_INSTANCE: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_repository_instance() -> u64 {
    NEXT_REPOSITORY_INSTANCE.fetch_add(1, Ordering::Relaxed)
}


#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdentityPolicy {
    Filename,
    /// Repository-native path relative to the collection directory, including its extension.
    RelativePath,
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
        return Some(Value::Array(ArrayValue::from_unrestricted(values)));
    }
    operation.map_value(value)
}

pub(crate) fn string_values(value: &Value) -> impl Iterator<Item = &str> {
    let single = match value {
        Value::String(text) | Value::Id(text) => Some(text.as_str()),
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
                Value::String(text) | Value::Id(text) => Some(text.as_str()),
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


impl CollectionDefinition {
    /// Define a recursive folder collection with no declared fields or typed indexes.
    /// JSONL and registered readers use per-record generated identities because a file
    /// can contain several records. Callers may override any of these defaults.
    pub fn folder(
        name: impl Into<String>,
        source_path: impl Into<PathBuf>,
        format: SourceFormat,
    ) -> Self {
        let identity = if matches!(
            &format,
            SourceFormat::Json | SourceFormat::JsonDecodedScalars
        ) {
            IdentityPolicy::RelativePath
        } else {
            IdentityPolicy::Generated
        };
        Self {
            name: name.into(),
            source_path: source_path.into(),
            format,
            identity,
            discovery: Discovery {
                recursive: true,
                ..Discovery::default()
            },
            schema: Schema::default(),
            sort_fields: Vec::new(),
            sort_locale: "und".into(),
            sort_options: SortOptions::default(),
            sort_overrides: HashMap::new(),
        }
    }
}


/// Published generation and indexes for one collection.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct GlobalInferenceSummary {
    pub(crate) report: InferenceReport,
    pub(crate) diagnostics: Vec<InferencePathDiagnostic>,
}


pub struct CollectionState {
    pub definition: CollectionDefinition,
    pub(crate) source: PreparedSource,
    pub generation: CollectionGeneration,
    pub postings: PostingCollection,
    pub inference: Option<InferenceState>,
    pub(crate) global_inference: Option<GlobalInferenceSummary>,
    pub custom_postings: HashMap<(Vec<String>, String), CustomPostingIndex>,
    pub raw: BigramIndex,
    pub fallback_text: BigramIndex,
    pub logical_records: RoaringBitmap,
    pub empty_array_records: RoaringBitmap,
    pub custom_text: HashMap<(Vec<String>, String), BigramIndex>,
    pub(crate) text_bindings: crate::index_catalog::TextBindingCatalog,
    pub sorting: SortCollection,
    pub id_lookup: HashMap<RecordId, RecordSlot>,
    pub identity_order: Vec<RecordSlot>,
    pub stats: CollectionStats,
    pub relations: Vec<RelationIndex>,
}


/// Coordinator-owned mutable output for one collection. Construction jobs
/// only receive immutable definitions and discovered source metadata.
struct PendingCollection {
    definition: CollectionDefinition,
    definition_shared: Arc<CollectionDefinition>,
    source: PreparedSource,
    id: CollectionId,
    member_files: Vec<crate::model::FileId>,
    builder: CollectionBuilder,
    postings: PostingCollection,
    inference: Option<InferenceBuilder>,
    custom_postings: HashMap<(Vec<String>, String), CustomPostingIndex>,
    raw: BigramContribution,
    fallback_text: BigramContribution,
    logical_records: RoaringBitmap,
    empty_array_records: RoaringBitmap,
    sort_builder: SortBuilder,
    custom_text: HashMap<(Vec<String>, String), BigramContribution>,
    relations: Vec<RelationContribution>,
}


enum ScheduledBuildJob {
    Files(crate::build::RepositoryFileJob),
    Large {
        collection_index: usize,
        collection: CollectionId,
        definition: Arc<CollectionDefinition>,
        source: PreparedSource,
        file: crate::model::FileId,
    },
}


impl PendingCollection {
    fn admit(
        &mut self,
        inventory: &mut FileInventory,
        batch: crate::build::BatchContribution,
    ) -> Result<(), String> {
        crate::build::CollectionAdmission {
            inventory,
            builder: &mut self.builder,
            postings: &mut self.postings,
            inference: &mut self.inference,
            custom_postings: &mut self.custom_postings,
            raw: &mut self.raw,
            fallback_text: &mut self.fallback_text,
            custom_text: &mut self.custom_text,
            logical_records: &mut self.logical_records,
            empty_array_records: &mut self.empty_array_records,
            sorting: &mut self.sort_builder,
            relations: &mut self.relations,
        }
        .admit(batch)
    }
}


#[path = "repository_direct.rs"]
mod direct;
pub(crate) use direct::DirectBuild;

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
    /// Whole-file SHA-256 captured during the fresh builder's indexed reads.
    /// A missing digest means at least one eligible binding could not read
    /// complete bytes, so verified maintenance must fall back.
    pub(crate) base_fingerprints: Vec<Option<[u8; 32]>>,
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
    pub(crate) inferred_orders: Mutex<crate::sorting::InferredOrderCache>,
}


fn finish_fresh_fingerprints(
    files: &FileInventory,
    mut bindings: Vec<Vec<(CollectionId, [u8; 32])>>,
) -> Result<Vec<Option<[u8; 32]>>, String> {
    if bindings.len() != files.files.len() {
        return Err("fresh digest inventory length mismatch".into());
    }
    files
        .files
        .iter()
        .zip(&mut bindings)
        .map(|(file, reads)| {
            reads.sort_unstable_by_key(|(collection, _)| collection.0);
            let mut previous = None;
            let mut observed = 0usize;
            let mut consensus = None;
            for &(collection, digest) in reads.iter() {
                if !file.collections.contains(&collection) {
                    return Err("fresh digest belongs to an ineligible collection".into());
                }
                if consensus.is_some_and(|expected| expected != digest) {
                    return Err("source content changed between collection build reads".into());
                }
                consensus = Some(digest);
                if previous != Some(collection) {
                    previous = Some(collection);
                    observed += 1;
                }
            }
            if observed == file.collections.len() {
                Ok(consensus)
            } else {
                Ok(None)
            }
        })
        .collect()
}


/// Time spent restoring, reconciling, building, and writing state.
#[derive(Clone, Debug, Default)]
pub struct StartupStats {
    pub restored: bool,
    /// What the open operation actually did with the snapshot.
    pub snapshot_action: SnapshotAction,
    /// A recoverable restore failure that caused a read/write rebuild.
    pub snapshot_restore_error: Option<String>,
    pub snapshot_decode_ns: u64,
    pub reconciliation_ns: u64,
    pub build_ns: u64,
    pub build_discovery_ns: u64,
    pub build_extraction_index_ns: u64,
    pub build_finalization_ns: u64,
    pub snapshot_write_ns: u64,
    pub build_workers: usize,
    pub peak_build_admitted_bytes: u64,
    pub build_weight_limit_bytes: u64,
    pub build_peak_active_jobs: usize,
    pub build_peak_ready_batches: usize,
    pub build_job_execution_ns: u64,
    pub build_receive_wait_ns: u64,
    pub build_admission_ns: u64,
    pub build_out_of_order_completed: u64,
    pub build_ready_wait_sum_ns: u64,
    pub build_ready_wait_max_ns: u64,
    pub build_framing_ns: u64,
    pub build_framing_chunks: u64,
    pub build_framing_bytes: u64,
    /// Fresh-build inference counters and retained-state estimates.
    pub inference_build: InferenceBuildStats,
    /// Opt-in fresh-build timeline and sampled worker attribution.
    pub build_profile: Option<BuildProfileStats>,
}


#[derive(Clone, Debug, Default, Serialize)]
pub struct BuildProfileStats {
    pub discovery_enumeration_ns: u64,
    pub discovery_catalogue_ns: u64,
    pub discovery_metadata_prepare_ns: u64,
    pub discovery_ordered_insert_ns: u64,
    pub discovery_metadata_peak_active: usize,
    pub pool_init_ns: u64,
    pub builder_setup_ns: u64,
    pub source_work_ns: u64,
    pub fingerprint_finish_ns: u64,
    pub finalize_generation_ns: u64,
    pub finalize_bigrams_ns: u64,
    pub finalize_validation_ns: u64,
    pub finalize_sort_ns: u64,
    pub finalize_inference_ns: u64,
    pub finalize_identity_ns: u64,
    pub relations_ns: u64,
    pub final_checks_ns: u64,
    pub pool_teardown_ns: u64,
    pub query_pool_init_ns: u64,
    pub large_file_total_ns: u64,
    pub large_files: u64,
    pub discovery_rss_bytes: u64,
    pub extraction_rss_bytes: u64,
    pub finalization_rss_bytes: u64,
    pub work: crate::build::BuildWorkProfile,
}


struct PreparedDiscoveredFile {
    relative: PathBuf,
    revision: SourceRevision,
    created_ns: Option<i128>,
}


fn prepare_discovered_file(root: &Path, path: PathBuf) -> Result<PreparedDiscoveredFile, String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|error| error.to_string())?
        .to_owned();
    let metadata = std::fs::metadata(&path).map_err(|error| error.to_string())?;
    let revision = SourceRevision::from_metadata(&metadata)?;
    let created_ns = metadata
        .created()
        .ok()
        .and_then(|time| time.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos() as i128);
    Ok(PreparedDiscoveredFile {
        relative,
        revision,
        created_ns,
    })
}

/// Indexed Rayon collection keeps every result at its source ordinal. Errors
/// are examined by the caller in that order, independent of completion order.
fn prepare_discovered_window(
    pool: &rayon::ThreadPool,
    root: &Path,
    window: Vec<PathBuf>,
    profile: bool,
    peak_active: &AtomicUsize,
) -> Vec<Result<PreparedDiscoveredFile, String>> {
    let active = AtomicUsize::new(0);
    pool.install(|| {
        window
            .into_par_iter()
            .map(|path| {
                if profile {
                    let count = active.fetch_add(1, Ordering::Relaxed) + 1;
                    peak_active.fetch_max(count, Ordering::Relaxed);
                }
                let result = prepare_discovered_file(root, path);
                if profile {
                    active.fetch_sub(1, Ordering::Relaxed);
                }
                result
            })
            .collect()
    })
}

fn build_profile_rss_bytes() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.starts_with("VmRSS:"))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|number| number.parse::<u64>().ok())
        })
        .unwrap_or(0)
        .saturating_mul(1024)
}


#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotMode {
    /// Build from source without inspecting a snapshot.
    Disabled,
    /// Restore a compatible current snapshot, or build from source.
    IfValid,
    /// Require a compatible current snapshot.
    Required,
}


/// Actual result of the requested snapshot policy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SnapshotAction {
    #[default]
    BuiltInMemory,
    Restored,
}


/// Snapshot load policy and directory for one repository opening.
#[derive(Clone, Debug)]
pub struct SnapshotOpenPolicy {
    pub mode: SnapshotMode,
    /// `None` selects `<root>/.objekte`; a relative path uses the process cwd.
    pub directory: Option<PathBuf>,
}


impl SnapshotOpenPolicy {
    pub fn new(mode: SnapshotMode, directory: Option<PathBuf>) -> Self {
        Self { mode, directory }
    }
}


/// Index alphabet, snapshot, cache, and worker settings.
#[derive(Clone, Copy, Debug)]
pub struct RepositoryOptions {
    pub bigram_alphabet: BigramAlphabet,
    /// Estimated retained prepared-plan state; no memory is allocated up front.
    /// This is not a process RSS limit.
    pub prepared_cache_bytes: usize,
    pub verified_cache_bytes: usize,
    pub snapshot_mode: SnapshotMode,
    pub query_workers: usize,
    /// `None` uses the parallelism available to this process at build time.
    pub build_workers: Option<NonZeroUsize>,
    pub inference: InferenceConfig,
    /// Collect sampled build work and detailed timeline diagnostics.
    pub profile_build: bool,
}


impl Default for RepositoryOptions {
    fn default() -> Self {
        Self {
            bigram_alphabet: BigramAlphabet::Ascii,
            prepared_cache_bytes: 16 * 1024 * 1024,
            verified_cache_bytes: 0,
            snapshot_mode: SnapshotMode::IfValid,
            query_workers: std::thread::available_parallelism()
                .map_or(1, |workers| workers.get().min(32)),
            build_workers: None,
            inference: InferenceConfig::default(),
            profile_build: false,
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
    /// Retrieve one record by its collection-local ID. Locator access uses the
    /// published generation only; payload access verifies the source revision
    /// with the same reader and decoding path used by search results.
    pub fn get_record(
        &self,
        collection_name: &str,
        record_id: &str,
        return_mode: ReturnMode,
    ) -> Result<Option<SearchHit>, String> {
        self.get_record_with_active(collection_name, record_id, return_mode, None)
    }

    pub(crate) fn get_record_with_active(
        &self,
        collection_name: &str,
        record_id: &str,
        return_mode: ReturnMode,
        active: Option<&[ActiveRecords]>,
    ) -> Result<Option<SearchHit>, String> {
        let collection_index = self
            .collections
            .iter()
            .position(|collection| collection.definition.name == collection_name)
            .ok_or("unknown collection")?;
        let collection = &self.collections[collection_index];
        let Some(slot) = collection.id_lookup.get(&RecordId(record_id.into())) else {
            return Ok(None);
        };
        if active.is_some_and(|domains| !domains[collection_index].contains(*slot)) {
            return Ok(None);
        }
        let record = collection
            .generation
            .records
            .get(slot.0 as usize)
            .ok_or("record ID points outside generation")?;
        let payload = match return_mode {
            ReturnMode::Locator => None,
            ReturnMode::Logical if record.class == RecordClass::RawFallback => {
                Some(Payload::LogicalUnavailable {
                    diagnostic_ref: record.diagnostic_ref,
                })
            }
            ReturnMode::Bytes | ReturnMode::Logical => {
                let mut access = SourceAccessSession::new(&self.files);
                let mut read = access.read(record.file, &[(slot.0, record.range)])?;
                let bytes = read.records.pop().ok_or("source record missing")?.1;
                crate::materialization::payload_from_bytes(
                    record.class,
                    Some(collection),
                    &return_mode,
                    bytes,
                )?
                .0
            }
        };
        Ok(Some(crate::run::make_hit(
            self,
            collection_index,
            record,
            Vec::new(),
            payload,
        )))
    }

    pub fn files(&self) -> &FileInventory {
        &self.files
    }

    pub fn collections(&self) -> &[CollectionState] {
        &self.collections
    }

    /// Read one declared source relation without decoding source records.
    pub fn relation_index(
        &self,
        source_collection: &str,
        path: &[String],
    ) -> Option<&RelationIndex> {
        let collection = self
            .collections
            .iter()
            .find(|item| item.definition.name == source_collection)?;
        collection
            .relations
            .iter()
            .find(|relation| relation.declaration.path == path)
    }

    /// Resolve a reverse alias in its target collection's namespace.
    pub fn backlink_relation(
        &self,
        target_collection: &str,
        alias: &str,
    ) -> Option<&RelationIndex> {
        let target = self
            .collections
            .iter()
            .find(|item| item.definition.name == target_collection)?;
        self.collections
            .iter()
            .flat_map(|source| &source.relations)
            .find(|relation| {
                relation.target_collection == target.generation.collection
                    && relation.declaration.reverse_alias.as_deref() == Some(alias)
            })
    }

    /// Borrow the existing typed posting for sources referencing this ID.
    pub fn relation_reverse_sources(
        &self,
        relation: &RelationIndex,
        target_id: &str,
    ) -> Option<&RoaringBitmap> {
        let source = self.collections.get(relation.id.source.0 as usize)?;
        if !source
            .relations
            .iter()
            .any(|registered| std::ptr::eq(registered, relation))
        {
            return None;
        }
        source
            .postings
            .fields
            .get(&relation.declaration.path)?
            .values
            .get(&crate::posting::ScalarKey::Id(target_id.to_owned()))
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

    /// Completed observations for a collection; absent when inference was disabled.
    pub fn inference_report(&self, collection: &str) -> Option<&InferenceReport> {
        self.collections
            .iter()
            .find(|state| state.definition.name == collection)
            .and_then(|state| {
                state
                    .global_inference
                    .as_ref()
                    .map(|summary| &summary.report)
                    .or_else(|| state.inference.as_ref().map(|inference| &inference.report))
            })
    }

    /// Ordered-posting outcomes for every observed path, sorted by typed path.
    /// Returns `None` when inference was disabled for the collection.
    pub fn inference_index_diagnostics(
        &self,
        collection: &str,
    ) -> Option<Vec<InferencePathDiagnostic>> {
        self.collections
            .iter()
            .find(|state| state.definition.name == collection)
            .and_then(|state| {
                state
                    .global_inference
                    .as_ref()
                    .map(|summary| summary.diagnostics.clone())
                    .or_else(|| {
                        state
                            .inference
                            .as_ref()
                            .map(InferenceState::path_diagnostics)
                    })
            })
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
        let resolved = query.resolve_current_clock()?;
        crate::run::search(self, resolved.as_ref())
    }

    /// Count exact matches without constructing a result page or sort keys.
    /// Query parameters must already be bound; unresolved clock macros use
    /// the current clock, as in `search`.
    pub fn count_exact(&self, query: &Query) -> Result<usize, String> {
        let mut request = query.clone();
        request.options.count = true;
        request.options.limit = 0;
        request.options.offset = 0;
        self.search(&request)?
            .count
            .ok_or("count result missing".into())
    }

    /// Count with the same frozen clock, cancellation, and limits as
    /// `search_with_context`. The context belongs to this one invocation.
    pub fn count_exact_with_context(
        &self,
        query: &Query,
        context: &crate::streaming::ExecutionContext,
    ) -> Result<usize, crate::streaming::ExecutionError> {
        let mut request = query.clone();
        request.options.count = true;
        request.options.limit = 0;
        request.options.offset = 0;
        self.search_with_context(&request, context)?
            .count
            .ok_or_else(|| crate::streaming::ExecutionError {
                kind: crate::streaming::ExecutionErrorKind::Execution,
                message: "count result missing".into(),
            })
    }

    /// Search using one frozen clock and shared cancellation/resource limits.
    pub fn search_with_context(
        &self,
        query: &Query,
        context: &crate::streaming::ExecutionContext,
    ) -> Result<SearchResult, crate::streaming::ExecutionError> {
        context.enter()?;
        let resolved = query.with_clock(context.clock.now).map_err(|message| {
            context.error(crate::streaming::ExecutionErrorKind::InvalidQuery, message)
        })?;
        crate::run::search_with_context(self, &resolved, context)
    }

    /// Deliver verified requested-page updates while the query is executing.
    pub fn search_stream<F>(
        &self,
        query: &Query,
        options: crate::streaming::StreamOptions,
        cancellation: &crate::streaming::CancellationToken,
        sink: &mut F,
    ) -> Result<crate::streaming::StreamOutcome, crate::streaming::StreamError>
    where
        F: for<'e> FnMut(
            crate::streaming::StreamEvent<'e>,
        ) -> Result<crate::streaming::StreamControl, String>,
    {
        self.search_stream_scoped(
            query,
            crate::streaming::ResultScope::Page {
                offset: query.options.offset,
                limit: query.options.limit,
            },
            options,
            cancellation,
            sink,
        )
    }

    /// Stream a finite page or every match from one frozen execution.
    pub fn search_stream_scoped<F>(
        &self,
        query: &Query,
        scope: crate::streaming::ResultScope,
        options: crate::streaming::StreamOptions,
        cancellation: &crate::streaming::CancellationToken,
        sink: &mut F,
    ) -> Result<crate::streaming::StreamOutcome, crate::streaming::StreamError>
    where
        F: for<'e> FnMut(
            crate::streaming::StreamEvent<'e>,
        ) -> Result<crate::streaming::StreamControl, String>,
    {
        let context = crate::streaming::ExecutionContext::new(
            crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
            crate::streaming::ExecutionLimits::default(),
            Some(cancellation.clone()),
        );
        self.search_stream_scoped_with_context(query, scope, options, &context, sink)
    }

    /// Stream with a caller-supplied frozen clock and shared request limits.
    pub fn search_stream_scoped_with_context<F>(
        &self,
        query: &Query,
        scope: crate::streaming::ResultScope,
        options: crate::streaming::StreamOptions,
        context: &crate::streaming::ExecutionContext,
        sink: &mut F,
    ) -> Result<crate::streaming::StreamOutcome, crate::streaming::StreamError>
    where
        F: for<'e> FnMut(
            crate::streaming::StreamEvent<'e>,
        ) -> Result<crate::streaming::StreamControl, String>,
    {
        self.search_stream_scoped_with_context_active(query, scope, options, context, sink, None)
    }

    pub(crate) fn search_stream_scoped_with_context_active<F>(
        &self,
        query: &Query,
        scope: crate::streaming::ResultScope,
        options: crate::streaming::StreamOptions,
        context: &crate::streaming::ExecutionContext,
        sink: &mut F,
        active: Option<&[ActiveRecords]>,
    ) -> Result<crate::streaming::StreamOutcome, crate::streaming::StreamError>
    where
        F: for<'e> FnMut(
            crate::streaming::StreamEvent<'e>,
        ) -> Result<crate::streaming::StreamControl, String>,
    {
        let cancellation = context.cancellation().cloned().unwrap_or_default();
        if let Err(error) = context.enter() {
            if error.kind == crate::streaming::ExecutionErrorKind::Cancelled {
                return Ok(crate::streaming::StreamOutcome::Cancelled(
                    Default::default(),
                ));
            }
            return Err(crate::streaming::StreamError::Execution {
                kind: error.kind,
                message: error.message,
                progress: Default::default(),
            });
        }
        let resolved = query.with_clock(context.clock.now).map_err(|message| {
            crate::streaming::StreamError::Execution {
                kind: crate::streaming::ExecutionErrorKind::InvalidQuery,
                message,
                progress: Default::default(),
            }
        })?;
        let mut request = resolved;
        request.options.offset = scope.offset();
        match scope {
            crate::streaming::ResultScope::Page { limit, .. } => request.options.limit = limit,
            crate::streaming::ResultScope::All { .. } => {
                // Per-record residual verification needs one retained match.
                // The scope, not this local page size, controls delivery.
                request.options.limit = 1;
                request.options.count = true;
            }
        }
        crate::run::search_stream_scoped_with_context_active(
            self,
            &request,
            scope,
            options,
            &cancellation,
            context,
            sink,
            active,
        )
    }

    /// Bind typed parameters once, then stream the resulting frozen request.
    pub fn search_stream_with_parameters<F>(
        &self,
        query: &Query,
        parameters: &HashMap<String, Value>,
        options: crate::streaming::StreamOptions,
        cancellation: &crate::streaming::CancellationToken,
        sink: &mut F,
    ) -> Result<crate::streaming::StreamOutcome, crate::streaming::StreamError>
    where
        F: for<'e> FnMut(
            crate::streaming::StreamEvent<'e>,
        ) -> Result<crate::streaming::StreamControl, String>,
    {
        if cancellation.is_cancelled() {
            return Ok(crate::streaming::StreamOutcome::Cancelled(
                Default::default(),
            ));
        }
        let bound = query.with_parameters(parameters).map_err(|message| {
            crate::streaming::StreamError::Execution {
                kind: crate::streaming::ExecutionErrorKind::InvalidQuery,
                message,
                progress: Default::default(),
            }
        })?;
        self.search_stream(&bound, options, cancellation, sink)
    }

    /// Execute a parsed query with immutable, typed parameter values.
    pub fn search_with_parameters(
        &self,
        query: &Query,
        parameters: &HashMap<String, Value>,
    ) -> Result<SearchResult, String> {
        self.search(&query.with_parameters(parameters)?)
    }

    pub fn explain(&self, query: &Query) -> Result<Vec<crate::run::QueryPlanTrace>, String> {
        let resolved = query.resolve_current_clock()?;
        crate::run::explain(self, resolved.as_ref())
    }

    /// Explain one execution-bound parameter choice.
    pub fn explain_with_parameters(
        &self,
        query: &Query,
        parameters: &HashMap<String, Value>,
    ) -> Result<Vec<crate::run::QueryPlanTrace>, String> {
        self.explain(&query.with_parameters(parameters)?)
    }

    pub fn explain_analyze(
        &self,
        query: &Query,
    ) -> Result<(SearchResult, Vec<crate::run::QueryPlanTrace>), String> {
        let resolved = query.resolve_current_clock()?;
        crate::run::search_analyze(self, resolved.as_ref())
    }

    pub fn configure_verified_cache(&self, bytes: usize) -> Result<(), String> {
        let displaced = self
            .verified
            .lock()
            .map_err(|e| e.to_string())?
            .set_budget(bytes);
        drop(displaced);
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

    /// Open with an independent load policy and an optional snapshot directory.
    pub fn open_with_snapshot_policy(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        policy: SnapshotOpenPolicy,
    ) -> Result<Self, String> {
        Self::open_with_sources_and_snapshot_policy(
            root,
            definitions,
            options,
            SourceRegistry::builtin(),
            policy,
        )
    }

    pub fn open_with_sources(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        sources: SourceRegistry,
    ) -> Result<Self, String> {
        Self::open_with_sources_and_snapshot_policy(
            root,
            definitions,
            options,
            sources,
            SnapshotOpenPolicy::new(options.snapshot_mode, None),
        )
    }

    /// Registered-reader equivalent of `open_with_snapshot_policy`.
    pub fn open_with_sources_and_snapshot_policy(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        mut options: RepositoryOptions,
        sources: SourceRegistry,
        policy: SnapshotOpenPolicy,
    ) -> Result<Self, String> {
        options.inference.validate()?;
        crate::relation::validate_definitions(&definitions)?;
        options.snapshot_mode = policy.mode;
        let root = root.into().canonicalize().map_err(|e| e.to_string())?;
        let snapshot_directory = policy
            .directory
            .as_ref()
            .map(|directory| {
                if directory.is_absolute() {
                    Ok(directory.clone())
                } else {
                    std::env::current_dir()
                        .map(|cwd| cwd.join(directory))
                        .map_err(|error| error.to_string())
                }
            })
            .transpose()?
            .unwrap_or_else(|| root.join(".objekte"));
        let snapshot =
            crate::storage::snapshot_path_in(&snapshot_directory, options.bigram_alphabet);
        let mut decode_ns = 0;
        let mut reconciliation_ns = 0;
        let mut previous_generations = HashMap::new();
        let mut restore_error = None;
        let mut snapshot_exists = false;
        if matches!(
            options.snapshot_mode,
            SnapshotMode::Required | SnapshotMode::IfValid
        ) {
            match std::fs::metadata(&snapshot) {
                Ok(metadata) if metadata.is_file() => snapshot_exists = true,
                Ok(_) => return Err(format!("snapshot is not a file: {}", snapshot.display())),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    if options.snapshot_mode == SnapshotMode::Required {
                        return Err(format!(
                            "snapshot missing: {}; use --load-index with --write-index to rebuild",
                            snapshot.display()
                        ));
                    }
                }
                Err(error) => {
                    return Err(format!("snapshot access: {}: {error}", snapshot.display()));
                }
            }
        }
        if matches!(
            options.snapshot_mode,
            SnapshotMode::Required | SnapshotMode::IfValid
        ) && snapshot_exists
        {
            let decode_started = Instant::now();
            let loaded = crate::storage::load_snapshot_checked(
                root.clone(),
                definitions.clone(),
                options,
                &sources,
                &snapshot,
            );
            decode_ns = decode_started.elapsed().as_nanos() as u64;
            match loaded {
                Ok(mut repo) => {
                    let reconciliation_started = Instant::now();
                    let current = crate::storage::sources_match(&repo)?;
                    reconciliation_ns = reconciliation_started.elapsed().as_nanos() as u64;
                    if current {
                        repo.startup = StartupStats {
                            restored: true,
                            snapshot_action: SnapshotAction::Restored,
                            snapshot_decode_ns: decode_ns,
                            reconciliation_ns,
                            ..StartupStats::default()
                        };
                        return Ok(repo);
                    }
                    restore_error = Some("snapshot source inventory is stale".into());
                    for collection in repo.collections {
                        previous_generations
                            .insert(collection.definition.name, collection.generation.generation);
                    }
                }
                Err(crate::storage::SnapshotLoadError::Access(message)) => return Err(message),
                Err(crate::storage::SnapshotLoadError::Invalid(message)) => {
                    restore_error = Some(message);
                }
            }
            if options.snapshot_mode == SnapshotMode::Required {
                return Err(format!(
                    "snapshot cannot be loaded: {}; use --load-index with --write-index to rebuild",
                    restore_error
                        .as_deref()
                        .unwrap_or("unknown incompatibility")
                ));
            }
        }
        // Keep source construction separate from the optional restore attempt.
        let build_started = Instant::now();
        let mut repo = Self::build_fresh(root, definitions, options, &sources, true)?;
        for collection in &mut repo.collections {
            if let Some(previous) = previous_generations.get(&collection.definition.name) {
                collection.generation.generation =
                    previous.checked_add(1).ok_or("generation overflow")?;
            }
        }
        repo.startup.snapshot_decode_ns = decode_ns;
        repo.startup.reconciliation_ns = reconciliation_ns;
        repo.startup.snapshot_restore_error = restore_error;
        repo.startup.build_ns = build_started.elapsed().as_nanos() as u64;
        Ok(repo)
    }

    /// Construct zero-record indexes with the same configuration, without
    /// enumerating or opening any source file. Maintenance can then publish
    /// active-only compacted contributions as new segments.
    pub(crate) fn empty_like(&self) -> Result<Self, String> {
        let mut empty = Self::build_fresh(
            self.files.root.clone(),
            self.collections
                .iter()
                .map(|state| state.definition.clone())
                .collect(),
            self.options,
            &self.sources,
            false,
        )?;
        for (source, target) in self.collections.iter().zip(&mut empty.collections) {
            target.global_inference = source.global_inference.clone().or_else(|| {
                source
                    .inference
                    .as_ref()
                    .map(|state| GlobalInferenceSummary {
                        report: state.report.clone(),
                        diagnostics: state.path_diagnostics(),
                    })
            });
        }
        empty.build_parses = self.build_parses;
        empty.build_failed_parses = self.build_failed_parses;
        empty.startup = self.startup.clone();
        Ok(empty)
    }

    fn build_fresh(
        root: PathBuf,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        sources: &SourceRegistry,
        discover_sources: bool,
    ) -> Result<Self, String> {
        let mut profile = options.profile_build.then(BuildProfileStats::default);
        let pool_started = profile.as_ref().map(|_| Instant::now());
        let build_workers = crate::build::resolve_build_workers(
            options.build_workers,
            std::thread::available_parallelism().ok(),
        )?;
        let build_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(build_workers)
            .thread_name(|index| format!("objekte-build-{index}"))
            .build()
            .map_err(|error| error.to_string())?;
        if let (Some(started), Some(profile)) = (pool_started, profile.as_mut()) {
            profile.pool_init_ns = started.elapsed().as_nanos() as u64;
        }
        let discovery_started = Instant::now();
        let mut files = FileInventory::new(root.clone());
        let mut selected = Vec::new();
        // At most 32 paths per worker are prepared before ordered insertion.
        // The discovered collection remains the only corpus-sized path list.
        let metadata_window = build_workers.saturating_mul(32);
        let metadata_peak_active = AtomicUsize::new(0);
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
            let enumeration_started = profile.as_ref().map(|_| Instant::now());
            let paths = if discover_sources {
                discover(
                    &root,
                    &definition.source_path,
                    &definition.discovery,
                    &source,
                )?
            } else {
                Vec::new()
            };
            if let (Some(started), Some(profile)) = (enumeration_started, profile.as_mut()) {
                profile.discovery_enumeration_ns += started.elapsed().as_nanos() as u64;
            }
            let catalogue_started = profile.as_ref().map(|_| Instant::now());
            let mut member_files = Vec::new();
            let mut paths = paths.into_iter();
            loop {
                let window = paths.by_ref().take(metadata_window).collect::<Vec<_>>();
                if window.is_empty() {
                    break;
                }
                let prepare_started = profile.as_ref().map(|_| Instant::now());
                let prepared = if build_workers == 1 {
                    if profile.is_some() {
                        metadata_peak_active.fetch_max(1, Ordering::Relaxed);
                    }
                    window
                        .into_iter()
                        .map(|path| prepare_discovered_file(&root, path))
                        .collect::<Vec<_>>()
                } else {
                    prepare_discovered_window(
                        &build_pool,
                        &root,
                        window,
                        profile.is_some(),
                        &metadata_peak_active,
                    )
                };
                if let (Some(started), Some(profile)) = (prepare_started, profile.as_mut()) {
                    profile.discovery_metadata_prepare_ns += started.elapsed().as_nanos() as u64;
                }
                let insert_started = profile.as_ref().map(|_| Instant::now());
                for entry in prepared {
                    let entry = entry?;
                    let file = files.add(entry.relative, entry.revision, entry.created_ns, id)?;
                    member_files.push(file);
                }
                if let (Some(started), Some(profile)) = (insert_started, profile.as_mut()) {
                    profile.discovery_ordered_insert_ns += started.elapsed().as_nanos() as u64;
                }
            }
            if let (Some(started), Some(profile)) = (catalogue_started, profile.as_mut()) {
                profile.discovery_catalogue_ns += started.elapsed().as_nanos() as u64;
            }
            selected.push((definition, id, source, member_files));
        }
        let discovery_ns = discovery_started.elapsed().as_nanos() as u64;
        if let Some(profile) = profile.as_mut() {
            profile.discovery_rss_bytes = build_profile_rss_bytes();
            profile.discovery_metadata_peak_active = metadata_peak_active.load(Ordering::Relaxed);
        }
        let source_files = Arc::new(
            files
                .files
                .iter()
                .map(crate::build::SourceFileInput::from_entry)
                .collect(),
        );
        let root_shared = Arc::new(root.clone());
        let mut pending = Vec::new();
        let extraction_started = Instant::now();
        let setup_started = profile.as_ref().map(|_| Instant::now());
        let mut build_parses = 0;
        let mut build_failed_parses = 0;
        let mut extraction_index_ns = 0u64;
        let mut finalization_ns = 0u64;
        let mut schedule_stats = crate::build::BuildScheduleStats::default();
        let mut digest_bindings = vec![Vec::new(); files.files.len()];
        let mut large_digests = Vec::new();
        let collection_count = selected.len();
        for (definition, id, source, member_files) in selected {
            let builder = CollectionBuilder::new(id, definition.name.clone(), 1, files.files.len());
            let postings = PostingCollection::new(&definition.schema);
            let inference = InferenceBuilder::for_collection(options.inference, collection_count);
            let custom_postings: HashMap<_, _> = definition
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
            let raw = BigramContribution::new(options.bigram_alphabet);
            let fallback_text = BigramContribution::new(options.bigram_alphabet);
            let logical_records = RoaringBitmap::new();
            let empty_array_records = RoaringBitmap::new();
            let sort_builder = SortBuilder::new(
                &definition.sort_fields,
                &definition.schema,
                &definition.sort_locale,
                definition.sort_options,
                &definition.sort_overrides,
            )?;
            let custom_text: HashMap<_, _> = definition
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
                        BigramContribution::new(options.bigram_alphabet),
                    )
                })
                .collect();
            let relations = RelationContribution::for_schema(&definition.schema);
            pending.push(PendingCollection {
                definition_shared: Arc::new(definition.clone()),
                definition,
                source,
                id,
                member_files,
                builder,
                postings,
                inference,
                custom_postings,
                raw,
                fallback_text,
                logical_records,
                empty_array_records,
                sort_builder,
                custom_text,
                relations,
            });
        }
        let mut scheduled = Vec::new();
        for (collection_index, state) in pending.iter().enumerate() {
            for batch in crate::build::file_batches(&state.member_files, &files) {
                if batch.len() == 1
                    && files.files[batch[0].0 as usize].revision.size > 8 * 1024 * 1024
                {
                    scheduled.push(ScheduledBuildJob::Large {
                        collection_index,
                        collection: state.id,
                        definition: Arc::clone(&state.definition_shared),
                        source: state.source.clone(),
                        file: batch[0],
                    });
                } else {
                    scheduled.push(ScheduledBuildJob::Files(crate::build::RepositoryFileJob {
                        collection_index,
                        collection: state.id,
                        definition: Arc::clone(&state.definition_shared),
                        source: state.source.clone(),
                        files: batch.to_vec(),
                    }));
                }
            }
        }
        if let (Some(started), Some(profile)) = (setup_started, profile.as_mut()) {
            profile.builder_setup_ns = started.elapsed().as_nanos() as u64;
        }
        let mut normal = Vec::new();
        let mut admit = |collection_index: usize, batch: crate::build::BatchContribution| {
            if let Some(profile) = profile.as_mut() {
                profile.work.absorb(&batch.profile);
            }
            for &(file_id, digest) in batch.file_fingerprints() {
                digest_bindings
                    .get_mut(file_id.0 as usize)
                    .ok_or("fresh digest file ID out of bounds")?
                    .push((CollectionId(collection_index as u32), digest));
            }
            build_parses += batch.record_parses;
            build_failed_parses += batch.failed_parses;
            pending[collection_index].admit(&mut files, batch)
        };
        let source_work_started = options.profile_build.then(Instant::now);
        let mut large_file_total_ns = 0u64;
        let mut large_files = 0u64;
        for job in scheduled {
            match job {
                ScheduledBuildJob::Files(job) => normal.push(job),
                ScheduledBuildJob::Large {
                    collection_index,
                    collection,
                    definition,
                    source,
                    file,
                } => {
                    if !normal.is_empty() {
                        let stats = crate::build::process_repository_file_batches_profiled(
                            &build_pool,
                            Arc::clone(&root_shared),
                            &normal,
                            Arc::clone(&source_files),
                            options.bigram_alphabet,
                            options.inference,
                            options.profile_build,
                            &mut admit,
                        )?;
                        schedule_stats.absorb(stats);
                        normal.clear();
                    }
                    let large_started = options.profile_build.then(Instant::now);
                    let stats = crate::build::process_framed_file_profiled(
                        &build_pool,
                        Arc::clone(&root_shared),
                        definition,
                        collection,
                        source,
                        file,
                        Arc::clone(&source_files),
                        options.bigram_alphabet,
                        options.inference,
                        options.profile_build,
                        |batch| admit(collection_index, batch),
                    )?;
                    if let Some(started) = large_started {
                        large_file_total_ns += started.elapsed().as_nanos() as u64;
                        large_files += 1;
                    }
                    if let Some((file_id, digest)) = stats.file_fingerprint {
                        large_digests.push((
                            file_id,
                            CollectionId(collection_index as u32),
                            digest,
                        ));
                    }
                    schedule_stats.absorb(stats);
                }
            }
        }
        if !normal.is_empty() {
            let stats = crate::build::process_repository_file_batches_profiled(
                &build_pool,
                Arc::clone(&root_shared),
                &normal,
                Arc::clone(&source_files),
                options.bigram_alphabet,
                options.inference,
                options.profile_build,
                &mut admit,
            )?;
            schedule_stats.absorb(stats);
        }
        drop(admit);
        if let (Some(started), Some(profile)) = (source_work_started, profile.as_mut()) {
            profile.source_work_ns = started.elapsed().as_nanos() as u64;
            profile.large_file_total_ns = large_file_total_ns;
            profile.large_files = large_files;
        }
        let fingerprints_started = profile.as_ref().map(|_| Instant::now());
        for (file_id, collection, digest) in large_digests {
            digest_bindings
                .get_mut(file_id.0 as usize)
                .ok_or("fresh digest file ID out of bounds")?
                .push((collection, digest));
        }
        let base_fingerprints = finish_fresh_fingerprints(&files, digest_bindings)?;
        if let (Some(started), Some(profile)) = (fingerprints_started, profile.as_mut()) {
            profile.fingerprint_finish_ns = started.elapsed().as_nanos() as u64;
            profile.extraction_rss_bytes = build_profile_rss_bytes();
        }
        extraction_index_ns += extraction_started.elapsed().as_nanos() as u64;
        let mut collections = Vec::new();
        let mut pending_relations = Vec::new();
        let mut inference_build = InferenceBuildStats::default();
        for state in pending {
            let PendingCollection {
                definition,
                definition_shared: _,
                source,
                id: _,
                member_files: _,
                builder,
                mut postings,
                inference,
                mut custom_postings,
                raw,
                fallback_text,
                logical_records,
                empty_array_records,
                sort_builder,
                custom_text,
                relations,
            } = state;
            let finalization_started = Instant::now();
            let stage_started = profile.as_ref().map(|_| Instant::now());
            let generation = builder.finish(&files)?;
            if let (Some(started), Some(profile)) = (stage_started, profile.as_mut()) {
                profile.finalize_generation_ns += started.elapsed().as_nanos() as u64;
            }
            let stage_started = profile.as_ref().map(|_| Instant::now());
            let raw = raw.into_index_in_pool(&build_pool)?;
            let fallback_text = fallback_text.into_index_in_pool(&build_pool)?;
            let custom_text = custom_text
                .into_iter()
                .map(|(key, index)| {
                    index
                        .into_index_in_pool(&build_pool)
                        .map(|index| (key, index))
                })
                .collect::<Result<HashMap<_, _>, _>>()?;
            if let (Some(started), Some(profile)) = (stage_started, profile.as_mut()) {
                profile.finalize_bigrams_ns += started.elapsed().as_nanos() as u64;
            }
            let stage_started = profile.as_ref().map(|_| Instant::now());
            let all_logical = logical_records.len() == generation.records.len() as u64;
            postings.finish_stats();
            postings.validate_eligible(generation.records.len(), logical_records.len() as usize)?;
            if !all_logical {
                postings.validate_domain(&logical_records)?;
            }
            if !(&empty_array_records - &logical_records).is_empty() {
                return Err("empty-array structural bitmap exceeds logical records".into());
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
            if let (Some(started), Some(profile)) = (stage_started, profile.as_mut()) {
                profile.finalize_validation_ns += started.elapsed().as_nanos() as u64;
            }
            let stage_started = profile.as_ref().map(|_| Instant::now());
            let sorting = sort_builder.finish(&generation)?;
            if let (Some(started), Some(profile)) = (stage_started, profile.as_mut()) {
                profile.finalize_sort_ns += started.elapsed().as_nanos() as u64;
            }
            let stage_started = profile.as_ref().map(|_| Instant::now());
            let inference = inference
                .map(|builder| {
                    let mut stats = builder.build_stats();
                    let started = Instant::now();
                    let result = builder.finish(
                        &logical_records,
                        generation.records.len(),
                        &definition.schema,
                    );
                    stats.finish_ns = started.elapsed().as_nanos() as u64;
                    inference_build.absorb(stats);
                    result
                })
                .transpose()?;
            if let (Some(started), Some(profile)) = (stage_started, profile.as_mut()) {
                profile.finalize_inference_ns += started.elapsed().as_nanos() as u64;
            }
            let stage_started = profile.as_ref().map(|_| Instant::now());
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
            let text_bindings = crate::index_catalog::TextBindingCatalog::new(&definition.schema);
            if let (Some(started), Some(profile)) = (stage_started, profile.as_mut()) {
                profile.finalize_identity_ns += started.elapsed().as_nanos() as u64;
            }
            collections.push(CollectionState {
                definition,
                source,
                generation,
                postings,
                inference,
                global_inference: None,
                custom_postings,
                raw,
                fallback_text,
                logical_records,
                empty_array_records,
                custom_text,
                text_bindings,
                sorting,
                id_lookup,
                identity_order,
                stats,
                relations: Vec::new(),
            });
            pending_relations.push(relations);
            finalization_ns += finalization_started.elapsed().as_nanos() as u64;
        }
        if let Some(profile) = profile.as_mut() {
            profile.finalization_rss_bytes = build_profile_rss_bytes();
        }
        let relations_started = profile.as_ref().map(|_| Instant::now());
        let mut resolved_relations = Vec::with_capacity(collections.len());
        for (source_index, contributions) in pending_relations.into_iter().enumerate() {
            let source_id = collections[source_index].generation.collection;
            let indexes = contributions
                .into_iter()
                .enumerate()
                .map(|(ordinal, contribution)| {
                    contribution.resolve(
                        source_id,
                        u32::try_from(ordinal).map_err(|_| "too many relations")?,
                        &collections,
                    )
                })
                .collect::<Result<Vec<_>, String>>()?;
            resolved_relations.push(indexes);
        }
        for (collection, indexes) in collections.iter_mut().zip(resolved_relations) {
            collection.relations = indexes;
        }
        crate::relation::validate_indexes(&collections)?;
        if let (Some(started), Some(profile)) = (relations_started, profile.as_mut()) {
            profile.relations_ns = started.elapsed().as_nanos() as u64;
        }
        let checks_started = profile.as_ref().map(|_| Instant::now());
        for state in &mut collections {
            state.sorting.refresh_file_status(&state.generation, &files);
        }
        files.validate_diagnostics()?;
        if let (Some(started), Some(profile)) = (checks_started, profile.as_mut()) {
            profile.final_checks_ns = started.elapsed().as_nanos() as u64;
        }
        let teardown_started = profile.as_ref().map(|_| Instant::now());
        drop(build_pool);
        if let (Some(started), Some(profile)) = (teardown_started, profile.as_mut()) {
            profile.pool_teardown_ns = started.elapsed().as_nanos() as u64;
        }
        let query_pool_started = profile.as_ref().map(|_| Instant::now());
        let workers = Self::create_workers(options.query_workers)?;
        if let (Some(started), Some(profile)) = (query_pool_started, profile.as_mut()) {
            profile.query_pool_init_ns = started.elapsed().as_nanos() as u64;
        }
        Ok(Self {
            files,
            base_fingerprints,
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
                build_workers,
                peak_build_admitted_bytes: schedule_stats.peak_weight_bytes,
                build_weight_limit_bytes: schedule_stats.weight_limit_bytes,
                build_peak_active_jobs: schedule_stats.peak_active_jobs,
                build_peak_ready_batches: schedule_stats.peak_ready_batches,
                build_job_execution_ns: schedule_stats.job_execution_ns,
                build_receive_wait_ns: schedule_stats.receive_wait_ns,
                build_admission_ns: schedule_stats.admission_ns,
                build_out_of_order_completed: schedule_stats.out_of_order_completed,
                build_ready_wait_sum_ns: schedule_stats.ready_wait_sum_ns,
                build_ready_wait_max_ns: schedule_stats.ready_wait_max_ns,
                build_framing_ns: schedule_stats.framing_ns,
                build_framing_chunks: schedule_stats.framing_chunks,
                build_framing_bytes: schedule_stats.framing_bytes,
                inference_build,
                build_profile: profile,
                ..StartupStats::default()
            },
            workers,
            prepared: Mutex::new(crate::prepared::PreparedCache::new(
                options.prepared_cache_bytes,
            )),
            verified: Mutex::new(crate::verified::VerifiedCache::new(
                options.verified_cache_bytes,
            )),
            inferred_orders: Mutex::new(crate::sorting::InferredOrderCache::new(
                crate::sorting::INFERRED_ORDER_CACHE_BYTES,
            )),
        })
    }

    pub fn search_reference(&self, query: &Query) -> Result<SearchResult, String> {
        self.search_reference_with_active(query, None)
    }

    pub(crate) fn search_reference_with_active(
        &self,
        query: &Query,
        active: Option<&[ActiveRecords]>,
    ) -> Result<SearchResult, String> {
        let resolved = query.resolve_current_clock()?;
        let query = resolved.as_ref();
        let mut bound = Vec::new();
        let mut parameter_types = HashMap::new();
        for collection in &self.collections {
            if !query.collections.is_empty()
                && !query.collections.contains(&collection.definition.name)
            {
                continue;
            }
            let filter = query
                .filter
                .as_ref()
                .map(|f| {
                    crate::relation_query::bind_filter_with_relations(
                        self,
                        collection.generation.collection.0 as usize,
                        f,
                        &mut parameter_types,
                    )
                })
                .transpose()?;
            let fts = query
                .fts
                .as_ref()
                .map(|fts| {
                    crate::relation_query::bind_fts_with_relations(
                        self,
                        collection.generation.collection.0 as usize,
                        fts,
                    )
                })
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
            source_unavailable_events: 0,
            source_diagnostics: Vec::new(),
            returned_records: 0,
            files_opened: 0,
            distinct_files: 0,
            bytes_read: 0,
            peak_retained_payload_bytes: 0,
            sort_key_evaluations: 0,
            peak_retained_sort_key_bytes: 0,
            sort_target_reads: 0,
            sort_target_reuse: 0,
            record_parses: 0,
            raw_text_verifications: 0,
            residual_evaluations: 0,
            relation_source_reads: 0,
            relation_target_reads: 0,
            relation_target_file_opens: 0,
            relation_edge_visits: 0,
            relation_backlink_probe_visits: 0,
            relation_bytes_read: 0,
            relation_target_candidates: 0,
            relation_target_matches: 0,
            relation_reverse_postings_visited: 0,
            relation_verified_targets_reused: 0,
            preparation_ns: 0,
            planning_ns: 0,
            prepared_plan_hit: false,
            verified_result_hit: false,
        };
        let mut files_read = HashSet::new();
        let relation_active = active.map(|domains| Arc::new(domains.to_vec()));
        for (collection, filter, fts) in bound {
            for record in &collection.generation.records {
                if active.is_some_and(|domains| {
                    !domains[collection.generation.collection.0 as usize].contains(record.slot)
                }) {
                    continue;
                }
                files_read.insert(record.file.0);
                let file = &self.files.files[record.file.0 as usize];
                let path = self
                    .files
                    .native_path(record.file)
                    .ok_or("missing source path")?;
                let bytes =
                    read_record_range_for_search(&path, record.revision, record.range, None)?;
                counters.files_opened += 1;
                counters.bytes_read += bytes.len() as u64;
                counters.record_parses += u64::from(record.class == RecordClass::Logical);
                counters.scanned_records += 1;
                let extracted = if record.class == RecordClass::Logical {
                    let projection = if fts.as_ref().is_some_and(|fts| !fts.fields.is_empty()) {
                        ScanProjection::full_resolved_text()
                    } else {
                        ScanProjection::full(true)
                    };
                    prepare_query_record(
                        record.class,
                        &collection.source,
                        &collection.definition.schema,
                        &bytes,
                        &projection,
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
                let mut relation_work = crate::relation_query::RelationWork::default();
                relation_work.attach_active(relation_active.clone());
                let fts_match = if record.class == RecordClass::RawFallback {
                    if let Some(fts) = &fts {
                        if fts.fields.is_empty() {
                            counters.raw_text_verifications += 1;
                            let matched = crate::reference::raw_fallback_contains(
                                fts,
                                &String::from_utf8_lossy(&bytes),
                            );
                            if matched && query.options.evidence {
                                evidence.insert("@record.raw_text".into());
                            }
                            matched
                        } else {
                            if query.options.evidence {
                                crate::relation_query::eval_fts_with_relations(
                                    self,
                                    fts,
                                    &[],
                                    &ctx,
                                    &mut evidence,
                                    crate::relation_query::EvaluationMode::Reference,
                                    &mut relation_work,
                                )?
                            } else {
                                crate::relation_query::eval_fts_with_relations_membership(
                                    self,
                                    fts,
                                    &[],
                                    &ctx,
                                    crate::relation_query::EvaluationMode::Reference,
                                    &mut relation_work,
                                )?
                            }
                        }
                    } else {
                        true
                    }
                } else {
                    fts.as_ref()
                        .map(|fts| {
                            if query.options.evidence {
                                crate::relation_query::eval_fts_with_relations(
                                    self,
                                    fts,
                                    &extracted.text,
                                    &ctx,
                                    &mut evidence,
                                    crate::relation_query::EvaluationMode::Reference,
                                    &mut relation_work,
                                )
                            } else {
                                crate::relation_query::eval_fts_with_relations_membership(
                                    self,
                                    fts,
                                    &extracted.text,
                                    &ctx,
                                    crate::relation_query::EvaluationMode::Reference,
                                    &mut relation_work,
                                )
                            }
                        })
                        .transpose()?
                        .unwrap_or(true)
                };
                let filter_match = if record.class == RecordClass::RawFallback {
                    filter
                        .as_ref()
                        .map(|f| {
                            if query.options.evidence {
                                eval_unavailable_filter(f, &ctx, &mut evidence)
                            } else {
                                crate::reference::eval_unavailable_filter_membership(f, &ctx)
                            }
                        })
                        .transpose()?
                        .flatten()
                        == Some(true)
                        || filter.is_none()
                } else {
                    filter
                        .as_ref()
                        .map(|f| {
                            if query.options.evidence {
                                crate::relation_query::eval_filter_truth(
                                    self,
                                    f,
                                    &ctx,
                                    &mut evidence,
                                    crate::relation_query::EvaluationMode::Reference,
                                    &mut relation_work,
                                )
                            } else {
                                crate::relation_query::eval_filter_truth_membership(
                                    self,
                                    f,
                                    &ctx,
                                    crate::relation_query::EvaluationMode::Reference,
                                    &mut relation_work,
                                )
                            }
                            .map(|answer| answer == Some(true))
                        })
                        .transpose()?
                        .unwrap_or(true)
                };
                counters.residual_evaluations += u64::from(filter.is_some());
                counters.relation_source_reads += relation_work.source_reads;
                counters.relation_target_reads += relation_work.target_reads;
                counters.relation_target_file_opens += relation_work.target_reads;
                counters.relation_edge_visits += relation_work.edge_visits;
                counters.relation_backlink_probe_visits += relation_work.backlink_probe_visits;
                counters.relation_bytes_read += relation_work.bytes_read;
                crate::relation_query::check_query_work_budget(&counters)?;
                counters.files_opened += relation_work.source_reads + relation_work.target_reads;
                counters.bytes_read += relation_work.bytes_read;
                counters.record_parses += relation_work.source_reads + relation_work.target_reads;
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
        sort_rows(self, query, &mut hits, &mut counters, active)?;
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

    /// Evaluate a parameter choice through the independent reference route.
    pub fn search_reference_with_parameters(
        &self,
        query: &Query,
        parameters: &HashMap<String, Value>,
    ) -> Result<SearchResult, String> {
        let bound = query.with_parameters(parameters)?;
        self.search_reference(&bound)
    }
}


pub(crate) fn validate_sort(repo: &Repository, query: &Query) -> Result<(), String> {
    crate::sort_binding::BoundSort::bind(repo, query).map(|_| ())
}

pub(crate) fn sort_rows(
    repo: &Repository,
    query: &Query,
    hits: &mut [(usize, u32, SearchHit)],
    counters: &mut WorkCounters,
    active: Option<&[ActiveRecords]>,
) -> Result<(), String> {
    let bound = crate::sort_binding::BoundSort::bind(repo, query)?;
    if bound.has_computed {
        let mut cache = crate::sort_binding::SortTargetCache::default();
        if let Some(active) = active {
            let context = crate::streaming::ExecutionContext::new(
                crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
                crate::streaming::ExecutionLimits::default(),
                None,
            )
            .with_active(active);
            cache.configure_execution(&context);
        }
        let target_cache = Mutex::new(cache);
        let empty = Value::Object(Default::default());
        let mut keys = HashMap::with_capacity(hits.len());
        for (collection_index, slot, hit) in hits.iter() {
            let collection = &repo.collections[*collection_index];
            let record = &collection.generation.records[*slot as usize];
            let projected;
            let value = if bound.needs_source && record.class == RecordClass::Logical {
                match &hit.payload {
                    Some(Payload::Logical(value)) => value,
                    Some(Payload::Bytes(bytes)) => {
                        projected = prepare_query_record(
                            record.class,
                            &collection.source,
                            &collection.definition.schema,
                            bytes,
                            &ScanProjection::full(false),
                            false,
                        )?;
                        counters.record_parses += 1;
                        &projected.value
                    }
                    _ => {
                        let path = repo
                            .files
                            .native_path(record.file)
                            .ok_or("sort source path missing")?;
                        let bytes = read_record_range_for_search(
                            &path,
                            record.revision,
                            record.range,
                            None,
                        )?;
                        counters.files_opened += 1;
                        counters.bytes_read += bytes.len() as u64;
                        projected = prepare_query_record(
                            record.class,
                            &collection.source,
                            &collection.definition.schema,
                            &bytes,
                            &ScanProjection::full(false),
                            false,
                        )?;
                        counters.record_parses += 1;
                        &projected.value
                    }
                }
            } else {
                &empty
            };
            let ctx = RecordContext {
                value,
                source_bytes: None,
                record,
                file: &repo.files.files[record.file.0 as usize],
                collection_name: &collection.definition.name,
                repository_root: &repo.files.root,
            };
            keys.insert(
                (*collection_index, *slot),
                bound.keys(repo, *collection_index, &ctx, &target_cache, None)?,
            );
            counters.sort_key_evaluations += 1;
        }
        hits.sort_by(|a, b| {
            crate::sort_binding::compare_parts(&keys[&(a.0, a.1)], &keys[&(b.0, b.1)]).then_with(
                || (&a.2.collection, &a.2.record_id).cmp(&(&b.2.collection, &b.2.record_id)),
            )
        });
        let targets = target_cache.lock().map_err(|error| error.to_string())?;
        counters.sort_target_reads += targets.reads;
        counters.sort_target_reuse += targets.reuse;
        counters.relation_edge_visits += targets.edge_visits;
        counters.files_opened += targets.reads;
        counters.record_parses += targets.reads;
        counters.bytes_read += targets.bytes_read;
        return Ok(());
    }
    hits.sort_by(|(a_collection, a_slot, a), (b_collection, b_slot, b)| {
        for (a_term, b_term) in bound.terms[*a_collection]
            .as_ref()
            .expect("selected collection")
            .iter()
            .zip(
                bound.terms[*b_collection]
                    .as_ref()
                    .expect("selected collection"),
            )
        {
            let crate::sort_binding::BoundSortSource::Stored(a_column) = &a_term.source else {
                unreachable!("all terms resident")
            };
            let crate::sort_binding::BoundSortSource::Stored(b_column) = &b_term.source else {
                unreachable!("all terms resident")
            };
            let order = a_column.keys[*a_slot as usize].compare_with(
                &b_column.keys[*b_slot as usize],
                a_term.descending,
                a_term.options,
            );
            if !order.is_eq() {
                return order;
            }
        }
        (&a.collection, &a.record_id).cmp(&(&b.collection, &b.record_id))
    });
    Ok(())
}

#[cfg(test)]
#[path = "repository.test.rs"]
mod tests;
