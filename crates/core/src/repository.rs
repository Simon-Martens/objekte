//! Resident repository ownership and the independent scan execution path.

use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::discovery::Discovery;
use crate::execution_options::{BigramAlphabet, ReturnMode};
use crate::index::bigram::{BigramConstructionStats, BigramIndex};
use crate::index::posting::{CustomPostingIndex, PostingCollection};
use crate::index::relation::RelationIndex;
use crate::index::segment::PublishedSegment;
use crate::index::sorting::{SortCollection, SortConfig, SortOptions};
use crate::inference::{
    InferenceBuildStats, InferenceConfig, InferencePathDiagnostic, InferenceReport, InferenceState,
};
use crate::inventory::{
    ActiveRecords, CollectionGeneration, DiagnosticRef, FileInventory, FileStatus, RecordClass,
};
use crate::lifecycle::IndexRevision;
use crate::lifecycle::catalog::FileCatalog;
use crate::measurement::WorkCounters;
use crate::model::{ArrayValue, CollectionId, FieldPath, RecordId, RecordSlot, Value};
use crate::query::Query;
use crate::schema::{Schema, operation::BindingOperation};
use crate::source::access::SourceAccessSession;
use crate::source::{SourceFormat, registry::PreparedSource, registry::SourceRegistry};

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
    /// Repository instance owning this pinned collection snapshot. Segment
    /// adapters use it to construct a full local-domain identity.
    pub(crate) repository_instance: u64,
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
    pub(crate) text_bindings: crate::query_access::catalog::TextBindingCatalog,
    pub sorting: SortCollection,
    /// Cached narrow gram/posting/sort estimate for status polling.
    pub(crate) index_family_bytes: usize,
    pub id_lookup: HashMap<RecordId, RecordSlot>,
    pub identity_order: Vec<RecordSlot>,
    pub stats: CollectionStats,
    pub relations: Vec<RelationIndex>,
}

#[cfg(test)]
pub(crate) use crate::construction::discovery::{
    prepare_discovered_file, prepare_discovered_window,
};
#[cfg(test)]
pub(crate) use crate::construction::segment_jobs::SegmentJobGate;

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
    pub(crate) files: Arc<FileInventory>,
    /// Whole-file SHA-256 captured during the fresh builder's indexed reads.
    /// A missing digest means at least one eligible binding could not read
    /// complete bytes, so verified maintenance must fall back.
    pub(crate) base_fingerprints: Vec<Option<[u8; 32]>>,
    pub(crate) collections: Vec<CollectionState>,
    pub(crate) active: Vec<ActiveRecords>,
    pub(crate) segments: Vec<Arc<Vec<Arc<PublishedSegment>>>>,
    pub(crate) catalog: FileCatalog,
    pub(crate) revision: IndexRevision,
    pub(crate) sources: SourceRegistry,
    pub(crate) build_parses: u64,
    pub(crate) build_failed_parses: u64,
    pub(crate) instance_id: u64,
    pub(crate) options: RepositoryOptions,
    pub(crate) startup: StartupStats,
    pub(crate) workers: rayon::ThreadPool,
    pub(crate) verified: Mutex<crate::query_results::verified::VerifiedCache>,
    pub(crate) inferred_orders: Mutex<crate::index::sorting::InferredOrderCache>,
}

fn hash_field(hash: &mut Sha256, value: &[u8]) {
    hash.update((value.len() as u64).to_le_bytes());
    hash.update(value);
}

pub(crate) fn finish_fresh_fingerprints(
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
    pub build_ns: u64,
    pub build_discovery_ns: u64,
    pub build_extraction_index_ns: u64,
    /// Time spent sealing, indexing, and publishing initial segments.
    pub build_publication_ns: u64,
    pub build_finalization_ns: u64,
    pub build_workers: usize,
    /// Actual construction source opens that reached a read attempt.
    pub build_source_reads: u64,
    /// Bytes consumed by those construction reads, excluding query checks.
    pub build_source_bytes_read: u64,
    pub peak_build_admitted_bytes: u64,
    pub build_weight_limit_bytes: u64,
    pub build_peak_active_jobs: usize,
    pub build_peak_ready_batches: usize,
    pub build_peak_segment_jobs: usize,
    pub build_peak_segment_window: usize,
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
    /// Exclusive incremental admission work, excluding sealing/publication.
    pub admission: crate::construction::segment_admission::AdmissionBreakdown,
    pub admission_by_collection:
        BTreeMap<String, crate::construction::segment_admission::AdmissionBreakdown>,
    /// Sampled worker work by collection; these totals overlap in wall time.
    pub work_by_collection: BTreeMap<String, crate::construction::batch::BuildWorkProfile>,
    pub admission_schedule: crate::construction::scheduling::AdmissionScheduleProfile,
    pub peak_segment_jobs: usize,
    pub peak_segment_window: usize,
    /// Initial lifecycle segments, in publication order.
    pub segments: Vec<IncrementalSegmentProfile>,
    pub contribution_storage: BuildContributionStorageProfile,
    pub discovery_enumeration_ns: u64,
    pub discovery_catalogue_ns: u64,
    pub discovery_metadata_prepare_ns: u64,
    pub discovery_ordered_insert_ns: u64,
    pub discovery_metadata_peak_active: usize,
    pub pool_init_ns: u64,
    pub builder_setup_ns: u64,
    pub job_scheduling_ns: u64,
    pub source_work_ns: u64,
    pub final_segment_flush_ns: u64,
    pub publication_index_ns: u64,
    pub publication_prepare_ns: u64,
    pub publication_install_ns: u64,
    pub publication_callback_ns: u64,
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
    pub work: crate::construction::batch::BuildWorkProfile,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct IncrementalSegmentProfile {
    pub collection: String,
    pub files: usize,
    pub records: usize,
    pub source_bytes: u64,
    pub index_ns: u64,
    pub prepare_ns: u64,
    pub install_ns: u64,
    pub callback_ns: u64,
    pub aggregate_merge_ns: u64,
    pub postings_finish_validation_ns: u64,
    pub bigram_finish_ns: u64,
    pub bigram_validation_ns: u64,
    pub sorting_finish_ns: u64,
    pub inference_finish_ns: u64,
    pub identity_order_ns: u64,
}

/// Opt-in construction capacity samples. Capacity bytes are lower bounds:
/// hash control bytes, allocator rounding, active workers and channel-held
/// results are excluded. The ready and admitted peaks need not coincide.
#[derive(Clone, Debug, Default, Serialize)]
pub struct BuildContributionStorageProfile {
    pub peak_global_capacity_bytes: usize,
    pub peak_admitted_batch_capacity_bytes: usize,
    pub peak_global_plus_admitted_batch_capacity_bytes: usize,
    pub peak_ready_capacity_bytes: usize,
    pub final_global_capacity_bytes: usize,
    pub final_global_live_payload_bytes: usize,
    pub final_diagnostic_counter_bytes: usize,
    pub final_global_chunks: usize,
    pub final_global_chunk_capacity: usize,
    pub final_global_spans: usize,
    pub final_global_span_capacity: usize,
    pub final_global_blocks: usize,
    pub final_global_block_capacity: usize,
    pub final_global_chunk_capacity_bytes: usize,
    pub final_global_span_capacity_bytes: usize,
    pub final_global_block_capacity_bytes: usize,
    pub chunk_element_size: usize,
    pub span_element_size: usize,
    pub block_element_size: usize,
    pub pending_element_size: usize,
    pub final_copied_block_bytes: u64,
    pub final_admitted_chunks: u64,
    pub final_gram_set_lookups: u64,
    pub final_duplicate_bit_ors: u64,
    pub final_sorted_entries: u64,
    /// Sums of opt-in intervals across contributions. Worker and merge
    /// intervals may overlap; these are not exclusive wall time.
    pub final_add_segment_ns: u64,
    pub final_scratch_drain_ns: u64,
    pub final_sort_ns: u64,
    pub final_pack_ns: u64,
    pub final_validation_ns: u64,
    pub final_admission_merge_ns: u64,
    pub max_worker_scratch_peak_capacity_bytes: usize,
    pub max_worker_scratch_after_flush_capacity_bytes: usize,
    pub max_pending_peak_capacity_bytes: usize,
}

impl BuildContributionStorageProfile {
    #[allow(
        dead_code,
        reason = "retained for complete construction storage profile accounting"
    )]
    fn observe(&mut self, global: BigramConstructionStats, batch: BigramConstructionStats) {
        self.chunk_element_size = self
            .chunk_element_size
            .max(global.chunk_element_size)
            .max(batch.chunk_element_size);
        self.span_element_size = self
            .span_element_size
            .max(global.span_element_size)
            .max(batch.span_element_size);
        self.block_element_size = self
            .block_element_size
            .max(global.block_element_size)
            .max(batch.block_element_size);
        self.pending_element_size = self
            .pending_element_size
            .max(global.pending_element_size)
            .max(batch.pending_element_size);
        self.peak_global_capacity_bytes = self
            .peak_global_capacity_bytes
            .max(global.retained_capacity_bytes);
        self.peak_admitted_batch_capacity_bytes = self
            .peak_admitted_batch_capacity_bytes
            .max(batch.retained_capacity_bytes);
        self.peak_global_plus_admitted_batch_capacity_bytes =
            self.peak_global_plus_admitted_batch_capacity_bytes.max(
                global
                    .retained_capacity_bytes
                    .saturating_add(batch.retained_capacity_bytes),
            );
        self.final_global_capacity_bytes = global.retained_capacity_bytes;
        self.final_global_live_payload_bytes = global.live_payload_bytes;
        self.final_diagnostic_counter_bytes = global.diagnostic_counter_bytes;
        self.final_global_chunks = global.chunk_len;
        self.final_global_chunk_capacity = global.chunk_capacity;
        self.final_global_spans = global.normal_span_len + global.skip_span_len;
        self.final_global_span_capacity = global.normal_span_capacity + global.skip_span_capacity;
        self.final_global_blocks = global.normal_block_len + global.skip_block_len;
        self.final_global_block_capacity =
            global.normal_block_capacity + global.skip_block_capacity;
        self.final_global_chunk_capacity_bytes = global.chunk_capacity * global.chunk_element_size;
        self.final_global_span_capacity_bytes =
            self.final_global_span_capacity * global.span_element_size;
        self.final_global_block_capacity_bytes =
            self.final_global_block_capacity * global.block_element_size;
        self.final_copied_block_bytes = global.copied_block_bytes;
        self.final_admitted_chunks = global.admitted_chunks;
        self.final_gram_set_lookups = global.gram_set_lookups;
        self.final_duplicate_bit_ors = global.duplicate_bit_ors;
        self.final_sorted_entries = global.sorted_entries;
        self.final_add_segment_ns = global.add_segment_ns;
        self.final_scratch_drain_ns = global.scratch_drain_ns;
        self.final_sort_ns = global.sort_ns;
        self.final_pack_ns = global.pack_ns;
        self.final_validation_ns = global.validation_ns;
        self.final_admission_merge_ns = global.admission_merge_ns;
        self.max_worker_scratch_peak_capacity_bytes = self
            .max_worker_scratch_peak_capacity_bytes
            .max(batch.worker_scratch_peak_capacity_bytes);
        self.max_worker_scratch_after_flush_capacity_bytes = self
            .max_worker_scratch_after_flush_capacity_bytes
            .max(batch.worker_scratch_after_flush_capacity_bytes);
        self.max_pending_peak_capacity_bytes = self
            .max_pending_peak_capacity_bytes
            .max(batch.pending_peak_capacity_bytes);
    }
}

pub(crate) fn build_profile_rss_bytes() -> u64 {
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

/// Index alphabet, verified cache, and worker settings.
#[derive(Clone, Copy, Debug)]
pub struct RepositoryOptions {
    pub bigram_alphabet: BigramAlphabet,
    pub verified_cache_bytes: usize,
    pub query_workers: usize,
    /// `None` uses the parallelism available to this process at build time.
    pub build_workers: Option<NonZeroUsize>,
    /// Shared soft record/file boundary for all native segment producers.
    pub segment_policy: crate::index::segment::SegmentPolicy,
    pub inference: InferenceConfig,
    /// Request detailed instrumentation in a build compiled with the
    /// `detailed-profiling` feature. Ignored by ordinary builds.
    pub profile_build: bool,
}

impl Default for RepositoryOptions {
    fn default() -> Self {
        Self {
            bigram_alphabet: BigramAlphabet::Ascii,
            verified_cache_bytes: 0,
            query_workers: std::thread::available_parallelism()
                .map_or(1, |workers| workers.get().min(32)),
            build_workers: None,
            segment_policy: crate::index::segment::SegmentPolicy::default(),
            inference: InferenceConfig::default(),
            profile_build: false,
        }
    }
}

impl RepositoryOptions {
    /// A compile-time false result removes detailed profiling from normal
    /// builds, including callers that accidentally set `profile_build`.
    #[inline(always)]
    pub(crate) const fn detailed_profiling_enabled(&self) -> bool {
        #[cfg(feature = "detailed-profiling")]
        {
            self.profile_build
        }

        #[cfg(not(feature = "detailed-profiling"))]
        {
            false
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
        if self.has_native_segments() {
            let domains = active.unwrap_or(&self.active);
            return match crate::query_execution::runner::direct_lookup_segmented(
                self,
                collection_name,
                record_id,
                return_mode,
                domains,
                &self.segments,
                &self.catalog,
                self.revision,
            )? {
                crate::query_execution::runner::DirectLookupResult::Found(mut hit) => {
                    self.stamp_native_hit(&mut hit);
                    Ok(Some(hit))
                }
                crate::query_execution::runner::DirectLookupResult::NotFound => Ok(None),
                crate::query_execution::runner::DirectLookupResult::SourceUnavailable(
                    diagnostic,
                ) => Err(diagnostic.message),
            };
        }
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
                crate::query_results::payload::payload_from_bytes(
                    record.class,
                    Some(collection),
                    &return_mode,
                    bytes,
                )?
                .0
            }
        };
        Ok(Some(crate::query_execution::runner::make_hit(
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
            .get(&crate::index::posting::ScalarKey::Id(target_id.to_owned()))
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

    /// Completed construction-generation observations for a collection.
    ///
    /// File mutations keep this report tied to the construction generation
    /// whose segments produced it; compaction or rebuild paths mark it
    /// incomplete until a new collection decision is available.
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

    pub(crate) fn native_snapshot(&self) -> crate::index::segment::NativeSnapshotRef<'_> {
        crate::index::segment::NativeSnapshotRef {
            active: &self.active,
            segments: &self.segments,
            catalog: &self.catalog,
            revision: self.revision,
        }
    }

    #[allow(
        dead_code,
        reason = "retained as the collection-scoped segment read-view adapter"
    )]
    pub(crate) fn segment_read_views(
        &self,
        collection: CollectionId,
    ) -> impl Iterator<Item = crate::index::segment::SegmentReadView<'_>> {
        self.segments
            .get(collection.0 as usize)
            .into_iter()
            .flat_map(|segments| segments.iter())
            .map(|segment| crate::index::segment::IndexedCollectionSegment::read_view(segment))
    }

    /// Compact native segment inventory for benchmarks and diagnostics.
    pub fn segment_summaries(&self) -> Vec<crate::index::segment::NativeSegmentSummary> {
        self.segment_summaries_for(&self.segments).collect()
    }

    pub(crate) fn segment_summaries_for<'a>(
        &self,
        segments: &'a [Arc<Vec<Arc<crate::index::segment::PublishedSegment>>>],
    ) -> impl Iterator<Item = crate::index::segment::NativeSegmentSummary> + 'a {
        segments
            .iter()
            .flat_map(|segments| {
                segments.iter().map(|segment| {
                    crate::index::segment::IndexedCollectionSegment::read_view(segment)
                })
            })
            .map(|view| crate::index::segment::NativeSegmentSummary {
                id: view.id,
                collection: view.collection,
                records: view.index.record_count(),
                active_records: view.active.len() as usize,
                files: view.index.file_count(),
                source_bytes: view.index.accounting.source_bytes_estimated,
                posting_fields: view.index.postings.fields.len(),
                sort_columns: view.index.sorting.columns.len(),
                relation_count: view.index.relations.len(),
                relation_edges: view
                    .index
                    .relations
                    .iter()
                    .map(|relation| relation.edge_count())
                    .sum(),
                relation_heap_bytes: view
                    .index
                    .relations
                    .iter()
                    .map(|relation| relation.estimated_bytes())
                    .sum(),
                bigram_stores: 2 + view.index.custom_text.len(),
                distinct_gram_keys: std::iter::once(&view.index.raw)
                    .chain(std::iter::once(&view.index.fallback_text))
                    .chain(view.index.custom_text.values())
                    .map(|index| index.normal.len() + index.skip.len())
                    .sum(),
                bigram_heap_bytes: std::iter::once(&view.index.raw)
                    .chain(std::iter::once(&view.index.fallback_text))
                    .chain(view.index.custom_text.values())
                    .map(crate::index::bigram::BigramIndex::estimated_heap_bytes)
                    .sum(),
                index_family_bytes: view.index.accounting.index_family_bytes,
                active: view.active.len() != 0,
            })
    }

    /// Borrowed descriptors for one segment's independently owned native
    /// components.
    pub fn segment_components(
        &self,
        collection: CollectionId,
        segment: crate::index::segment::SegmentId,
    ) -> Option<Vec<crate::index::segment::NativeComponentDescriptor<'_>>> {
        self.segments
            .get(collection.0 as usize)?
            .iter()
            .find(|candidate| candidate.id == segment)
            .map(|candidate| candidate.index.component_descriptors())
    }

    /// Canonical logical-content digest for the adopted native segments.
    /// Segment grouping and local slots are normalized to logical record IDs,
    /// so a policy change does not masquerade as an index-content change.
    pub fn native_index_signature(&self) -> String {
        self.native_index_signature_for(&self.segments)
    }

    pub(crate) fn native_index_signature_for(
        &self,
        segments: &[Arc<Vec<Arc<crate::index::segment::PublishedSegment>>>],
    ) -> String {
        let mut hash = Sha256::new();
        for (collection_index, collection) in self.collections.iter().enumerate() {
            hash_field(&mut hash, collection.definition.name.as_bytes());
            let records = &collection.generation.records;
            let mut components = BTreeMap::<String, Vec<String>>::new();
            for segment in segments[collection_index].iter() {
                let record_id = |slot: RecordSlot| {
                    segment
                        .index
                        .record(slot)
                        .map(|record| record.id.0.to_string())
                        .unwrap_or_else(|| format!("missing:{}", slot.0))
                };
                let add_bitmap = |components: &mut BTreeMap<String, Vec<String>>,
                                  label: String,
                                  bitmap: &RoaringBitmap| {
                    components
                        .entry(label)
                        .or_default()
                        .extend(bitmap.iter().map(|slot| record_id(RecordSlot(slot))));
                };
                for (path, posting) in &segment.index.postings.fields {
                    let path = path.join(".");
                    add_bitmap(
                        &mut components,
                        format!("declared:{path}:presence"),
                        &posting.presence,
                    );
                    add_bitmap(
                        &mut components,
                        format!("declared:{path}:nulls"),
                        &posting.nulls,
                    );
                    add_bitmap(
                        &mut components,
                        format!("declared:{path}:empty"),
                        &posting.empty_selections,
                    );
                    for (key, bitmap) in &posting.values {
                        add_bitmap(
                            &mut components,
                            format!("declared:{path}:value:{key:?}"),
                            bitmap,
                        );
                    }
                    for (key, bitmap) in &posting.lengths {
                        add_bitmap(
                            &mut components,
                            format!("declared:{path}:length:{key:?}"),
                            bitmap,
                        );
                    }
                    if let Some(facts) = &posting.structural {
                        for (key, bitmap) in &facts.postings {
                            add_bitmap(
                                &mut components,
                                format!("declared:{path}:structural:{key:?}"),
                                bitmap,
                            );
                        }
                    }
                }
                if let Some(inference) = &segment.index.inference {
                    for (path, posting) in &inference.indexes {
                        let path = path.join(".");
                        add_bitmap(
                            &mut components,
                            format!("inferred:{path}:presence"),
                            &posting.presence,
                        );
                        for (key, bitmap) in &posting.values {
                            add_bitmap(
                                &mut components,
                                format!("inferred:{path}:value:{key:?}"),
                                bitmap,
                            );
                        }
                    }
                }
                for ((path, binding), posting) in &segment.index.custom_postings {
                    for (key, bitmap) in &posting.index.values {
                        add_bitmap(
                            &mut components,
                            format!("custom:{}:{binding}:{key:?}", path.join(".")),
                            bitmap,
                        );
                    }
                }
                for (label, index) in [
                    ("raw".to_owned(), &segment.index.raw),
                    ("fallback".to_owned(), &segment.index.fallback_text),
                ] {
                    for (kind, map) in [("normal", &index.normal), ("skip", &index.skip)] {
                        for (key, bitmap) in map {
                            add_bitmap(&mut components, format!("{label}:{kind}:{key}"), bitmap);
                        }
                    }
                }
                for ((path, binding), index) in &segment.index.custom_text {
                    for (kind, map) in [("normal", &index.normal), ("skip", &index.skip)] {
                        for (key, bitmap) in map {
                            add_bitmap(
                                &mut components,
                                format!("text:{}:{binding}:{kind}:{key}", path.join(".")),
                                bitmap,
                            );
                        }
                    }
                }
                for (path, column) in &segment.index.sorting.columns {
                    for (slot, key) in column.keys.iter().enumerate() {
                        let slot = RecordSlot(u32::try_from(slot).unwrap_or(u32::MAX));
                        components
                            .entry(format!("sort:{}", path.display()))
                            .or_default()
                            .push(format!("{}:{key:?}", record_id(slot)));
                    }
                }
            }
            for (label, ids) in &mut components {
                ids.sort();
                ids.dedup();
                hash_field(&mut hash, label.as_bytes());
                for id in ids {
                    hash_field(&mut hash, id.as_bytes());
                }
            }
            for relation in &collection.relations {
                hash_field(&mut hash, relation.declaration.path.join(".").as_bytes());
                for record in records {
                    if let Some(edges) = relation.edges(record.slot) {
                        for edge in edges {
                            hash_field(&mut hash, record.id.0.as_bytes());
                            hash_field(&mut hash, edge.target_id().as_bytes());
                            for position in &edge.positions {
                                hash_field(&mut hash, &position.to_le_bytes());
                            }
                        }
                    }
                }
            }
        }
        format!("{:x}", hash.finalize())
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

    fn has_native_segments(&self) -> bool {
        self.segments.iter().any(|segments| !segments.is_empty())
    }

    fn stamp_native_result(&self, mut result: SearchResult) -> SearchResult {
        for hit in &mut result.hits {
            self.stamp_native_hit(hit);
        }
        result
    }

    fn stamp_native_query_result(
        &self,
        query: &crate::query::Query,
        mut result: SearchResult,
    ) -> SearchResult {
        if query.options.return_mode != crate::execution_options::ReturnMode::Locator
            && result.counters.source_unavailable_events > 0
        {
            result.count = None;
            result.counters.match_count_complete = false;
        }
        self.stamp_native_result(result)
    }

    pub(crate) fn stamp_native_hit(&self, hit: &mut SearchHit) {
        if let Some(collection) = self
            .collections
            .iter()
            .find(|collection| collection.definition.name == hit.collection)
        {
            hit.generation = collection.generation.generation;
        }
        self.remap_native_diagnostic(hit);
    }

    pub(crate) fn stamp_native_hit_at(&self, hit: &mut SearchHit, revision: IndexRevision) {
        hit.generation = revision.0;
        self.remap_native_diagnostic(hit);
    }

    fn remap_native_diagnostic(&self, hit: &mut SearchHit) {
        let Some(local) = hit.diagnostic_ref.stored_index() else {
            return;
        };
        let Some(collection_index) = self
            .collections
            .iter()
            .position(|collection| collection.definition.name == hit.collection)
        else {
            return;
        };
        let file = crate::model::FileId(hit.file_id);
        for segment in self.segments[collection_index].iter() {
            let Some(slot) = segment.index.id_lookup.get(hit.record_id.as_str()).copied() else {
                continue;
            };
            if !segment.active.contains(slot)
                || segment
                    .index
                    .record(slot)
                    .is_none_or(|record| record.file != file)
            {
                continue;
            }
            let Some(source) = segment.index.file(file) else {
                return;
            };
            let Some(diagnostic) = source.entry.diagnostics.get(local) else {
                return;
            };
            let Some(position) = self.catalog.entry(file).and_then(|entry| {
                entry
                    .diagnostics
                    .iter()
                    .position(|candidate| candidate == diagnostic)
            }) else {
                return;
            };
            let reference = crate::inventory::DiagnosticRef((position + 1) as u32);
            hit.diagnostic_ref = reference;
            if let Some(Payload::LogicalUnavailable { diagnostic_ref }) = &mut hit.payload {
                *diagnostic_ref = reference;
            }
            return;
        }
    }

    pub fn search(&self, query: &Query) -> Result<SearchResult, String> {
        let resolved = query.resolve_current_clock()?;
        let result = if self.has_native_segments() {
            let snapshot = self.native_snapshot();
            let cache = crate::query_execution::runner::LifecycleCacheScope {
                store: &self.verified,
                instance_id: self.instance_id,
            };
            crate::query_execution::runner::search_segmented_lifecycle(
                self,
                resolved.as_ref(),
                snapshot.active,
                snapshot.segments,
                snapshot.catalog,
                snapshot.revision,
                Some(cache),
            )?
        } else {
            crate::query_execution::runner::search(self, resolved.as_ref())?
        };
        Ok(self.stamp_native_query_result(query, result))
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
        let result = if self.has_native_segments() {
            let cache = crate::query_execution::runner::LifecycleCacheScope {
                store: &self.verified,
                instance_id: self.instance_id,
            };
            crate::query_execution::runner::search_segmented_with_context_lifecycle(
                self,
                &resolved,
                &self.active,
                &self.segments,
                &self.catalog,
                self.revision,
                context,
                Some(cache),
            )?
        } else {
            crate::query_execution::runner::search_with_context(self, &resolved, context)?
        };
        Ok(self.stamp_native_result(result))
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
            crate::query::clock::ClockContext::new(time::OffsetDateTime::now_utc()),
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
        if active.is_none() && self.has_native_segments() {
            let cache = crate::query_execution::runner::LifecycleCacheScope {
                store: &self.verified,
                instance_id: self.instance_id,
            };
            let needs_remap = |hit: &SearchHit| {
                hit.diagnostic_ref.stored_index().is_some()
                    || self
                        .collections
                        .iter()
                        .find(|collection| collection.definition.name == hit.collection)
                        .is_none_or(|collection| collection.generation.generation != hit.generation)
            };
            let mut remap_rows = |event: crate::streaming::StreamEvent<'_>| -> Result<
                crate::streaming::StreamControl,
                String,
            > {
                match event {
                    crate::streaming::StreamEvent::OrderedRows {
                        sequence,
                        start,
                        rows,
                        progress,
                    } if rows.iter().any(needs_remap) => {
                        let mut remapped = rows.to_vec();
                        for hit in &mut remapped {
                            self.stamp_native_hit(hit);
                        }
                        sink(crate::streaming::StreamEvent::OrderedRows {
                            sequence,
                            start,
                            rows: &remapped,
                            progress,
                        })
                    }
                    crate::streaming::StreamEvent::PageReplacement {
                        sequence,
                        rows,
                        progress,
                    } if rows.iter().any(needs_remap) => {
                        let mut remapped = rows.to_vec();
                        for hit in &mut remapped {
                            self.stamp_native_hit(hit);
                        }
                        sink(crate::streaming::StreamEvent::PageReplacement {
                            sequence,
                            rows: &remapped,
                            progress,
                        })
                    }
                    event => sink(event),
                }
            };
            return crate::query_execution::runner::search_stream_segmented_with_context_lifecycle(
                self,
                query,
                scope,
                options,
                context,
                &self.active,
                &self.segments,
                &self.catalog,
                self.revision,
                &mut remap_rows,
                Some(cache),
            );
        }
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
        let needs_remap = |hit: &SearchHit| {
            hit.diagnostic_ref.stored_index().is_some()
                || self
                    .collections
                    .iter()
                    .find(|collection| collection.definition.name == hit.collection)
                    .is_none_or(|collection| collection.generation.generation != hit.generation)
        };
        let mut remap_rows = |event: crate::streaming::StreamEvent<'_>| -> Result<
            crate::streaming::StreamControl,
            String,
        > {
            match event {
                crate::streaming::StreamEvent::OrderedRows {
                    sequence,
                    start,
                    rows,
                    progress,
                } if rows.iter().any(needs_remap) => {
                    let mut remapped = rows.to_vec();
                    for hit in &mut remapped {
                        self.stamp_native_hit(hit);
                    }
                    sink(crate::streaming::StreamEvent::OrderedRows {
                        sequence,
                        start,
                        rows: &remapped,
                        progress,
                    })
                }
                crate::streaming::StreamEvent::PageReplacement {
                    sequence,
                    rows,
                    progress,
                } if rows.iter().any(needs_remap) => {
                    let mut remapped = rows.to_vec();
                    for hit in &mut remapped {
                        self.stamp_native_hit(hit);
                    }
                    sink(crate::streaming::StreamEvent::PageReplacement {
                        sequence,
                        rows: &remapped,
                        progress,
                    })
                }
                event => sink(event),
            }
        };
        crate::query_execution::runner::search_stream_scoped_with_context_active(
            self,
            &request,
            scope,
            options,
            &cancellation,
            context,
            &mut remap_rows,
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

    pub fn explain(
        &self,
        query: &Query,
    ) -> Result<Vec<crate::query_diagnostics::trace::QueryPlanTrace>, String> {
        let resolved = query.resolve_current_clock()?;
        if self.has_native_segments() {
            crate::query_execution::runner::explain_segmented(
                self,
                resolved.as_ref(),
                &self.active,
                &self.segments,
                self.revision,
            )
        } else {
            crate::query_execution::runner::explain(self, resolved.as_ref())
        }
    }

    /// Explain one execution-bound parameter choice.
    pub fn explain_with_parameters(
        &self,
        query: &Query,
        parameters: &HashMap<String, Value>,
    ) -> Result<Vec<crate::query_diagnostics::trace::QueryPlanTrace>, String> {
        self.explain(&query.with_parameters(parameters)?)
    }

    pub fn explain_analyze(
        &self,
        query: &Query,
    ) -> Result<
        (
            SearchResult,
            Vec<crate::query_diagnostics::trace::QueryPlanTrace>,
        ),
        String,
    > {
        let resolved = query.resolve_current_clock()?;
        let result = if self.has_native_segments() {
            crate::query_execution::runner::search_analyze_segmented(
                self,
                resolved.as_ref(),
                &self.active,
                &self.segments,
                &self.catalog,
                self.revision,
            )?
        } else {
            crate::query_execution::runner::search_analyze(self, resolved.as_ref())?
        };
        Ok((self.stamp_native_result(result.0), result.1))
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

    /// Construct zero-record indexes with the same configuration, without
    /// enumerating or opening any source file. Maintenance can then publish
    /// active-only compacted contributions as new segments.
    pub(crate) fn empty_like(&self) -> Result<Self, String> {
        let mut empty = crate::construction::repository_state::initialize_empty(
            self.files.root.clone(),
            self.collections
                .iter()
                .map(|state| state.definition.clone())
                .collect(),
            self.options,
            &self.sources,
            None,
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
            if let Some(summary) = target.global_inference.as_mut() {
                summary.report.complete = false;
            }
        }
        empty.build_parses = self.build_parses;
        empty.build_failed_parses = self.build_failed_parses;
        empty.startup = self.startup.clone();
        Ok(empty)
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
        let domains = active.unwrap_or(&self.active);
        let result = crate::reference::relations::search_segmented(
            self,
            query,
            domains,
            &self.segments,
            &self.catalog,
            self.revision,
        )?;
        Ok(self.stamp_native_result(result))
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

#[cfg(test)]
#[path = "repository.test.rs"]
mod tests;
