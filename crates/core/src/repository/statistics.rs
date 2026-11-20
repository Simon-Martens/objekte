//! Build and incremental-operation statistics.

use std::collections::BTreeMap;

use crate::index::bigram::BigramConstructionStats;
use crate::inference::InferenceBuildStats;
use serde::Serialize;

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
    pub zero_base_posting_transfers: u64,
    pub transferred_posting_keys: u64,
    pub transferred_posting_members: u64,
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
