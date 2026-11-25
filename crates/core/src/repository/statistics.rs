//! Build and incremental-operation statistics.

use std::collections::BTreeMap;

use crate::inference::InferenceBuildStats;
use serde::Serialize;

/// Simultaneous use of the shared streaming budget. Byte values describe
/// owned Rust buffer capacity, charged work or an explicit index estimate.
#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct BigramConstructionStats {
    pub chunk_element_size: usize,
    pub span_element_size: usize,
    pub block_element_size: usize,
    pub pending_element_size: usize,
    pub chunk_len: usize,
    pub chunk_capacity: usize,
    pub normal_span_len: usize,
    pub normal_span_capacity: usize,
    pub normal_block_len: usize,
    pub normal_block_capacity: usize,
    pub skip_span_len: usize,
    pub skip_span_capacity: usize,
    pub skip_block_len: usize,
    pub skip_block_capacity: usize,
    pub pending_normal_capacity: usize,
    pub pending_skip_capacity: usize,
    pub retained_capacity_bytes: usize,
    pub live_payload_bytes: usize,
    pub diagnostic_counter_bytes: usize,
    pub copied_block_bytes: u64,
    pub admitted_chunks: u64,
    /// Lower bound for HashMap-backed scratch: excludes control bytes and
    /// allocator rounding. Maxima are per contribution, not summed peaks.
    pub worker_scratch_peak_capacity_bytes: usize,
    pub worker_scratch_after_flush_capacity_bytes: usize,
    pub pending_peak_capacity_bytes: usize,
    pub gram_set_lookups: u64,
    pub duplicate_bit_ors: u64,
    pub sorted_entries: u64,
    pub add_segment_ns: u64,
    pub scratch_drain_ns: u64,
    pub sort_ns: u64,
    pub pack_ns: u64,
    pub validation_ns: u64,
    pub admission_merge_ns: u64,
}

impl BigramConstructionStats {
    pub fn absorb(&mut self, other: Self) {
        self.chunk_element_size = self.chunk_element_size.max(other.chunk_element_size);
        self.span_element_size = self.span_element_size.max(other.span_element_size);
        self.block_element_size = self.block_element_size.max(other.block_element_size);
        self.pending_element_size = self.pending_element_size.max(other.pending_element_size);
        self.chunk_len = self.chunk_len.saturating_add(other.chunk_len);
        self.chunk_capacity = self.chunk_capacity.saturating_add(other.chunk_capacity);
        self.normal_span_len = self.normal_span_len.saturating_add(other.normal_span_len);
        self.normal_span_capacity = self
            .normal_span_capacity
            .saturating_add(other.normal_span_capacity);
        self.normal_block_len = self.normal_block_len.saturating_add(other.normal_block_len);
        self.normal_block_capacity = self
            .normal_block_capacity
            .saturating_add(other.normal_block_capacity);
        self.skip_span_len = self.skip_span_len.saturating_add(other.skip_span_len);
        self.skip_span_capacity = self
            .skip_span_capacity
            .saturating_add(other.skip_span_capacity);
        self.skip_block_len = self.skip_block_len.saturating_add(other.skip_block_len);
        self.skip_block_capacity = self
            .skip_block_capacity
            .saturating_add(other.skip_block_capacity);
        self.pending_normal_capacity = self
            .pending_normal_capacity
            .saturating_add(other.pending_normal_capacity);
        self.pending_skip_capacity = self
            .pending_skip_capacity
            .saturating_add(other.pending_skip_capacity);
        self.retained_capacity_bytes = self
            .retained_capacity_bytes
            .saturating_add(other.retained_capacity_bytes);
        self.live_payload_bytes = self
            .live_payload_bytes
            .saturating_add(other.live_payload_bytes);
        self.diagnostic_counter_bytes = self
            .diagnostic_counter_bytes
            .saturating_add(other.diagnostic_counter_bytes);
        self.copied_block_bytes = self
            .copied_block_bytes
            .saturating_add(other.copied_block_bytes);
        self.admitted_chunks = self.admitted_chunks.saturating_add(other.admitted_chunks);
        self.worker_scratch_peak_capacity_bytes = self
            .worker_scratch_peak_capacity_bytes
            .max(other.worker_scratch_peak_capacity_bytes);
        self.worker_scratch_after_flush_capacity_bytes = self
            .worker_scratch_after_flush_capacity_bytes
            .max(other.worker_scratch_after_flush_capacity_bytes);
        self.pending_peak_capacity_bytes = self
            .pending_peak_capacity_bytes
            .max(other.pending_peak_capacity_bytes);
        self.gram_set_lookups = self.gram_set_lookups.saturating_add(other.gram_set_lookups);
        self.duplicate_bit_ors = self
            .duplicate_bit_ors
            .saturating_add(other.duplicate_bit_ors);
        self.sorted_entries = self.sorted_entries.saturating_add(other.sorted_entries);
        self.add_segment_ns = self.add_segment_ns.saturating_add(other.add_segment_ns);
        self.scratch_drain_ns = self.scratch_drain_ns.saturating_add(other.scratch_drain_ns);
        self.sort_ns = self.sort_ns.saturating_add(other.sort_ns);
        self.pack_ns = self.pack_ns.saturating_add(other.pack_ns);
        self.validation_ns = self.validation_ns.saturating_add(other.validation_ns);
        self.admission_merge_ns = self
            .admission_merge_ns
            .saturating_add(other.admission_merge_ns);
    }
}

/// Optional aggregate timing of the ordered merge. Values from separate
/// batches are summed; `remainder_ns` includes timer and drop boundaries.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct AdmissionBreakdown {
    pub batches: u64,
    pub files: u64,
    pub records: u64,
    pub callback_ns: u64,
    pub total_ns: u64,
    /// Incremental coordinator accounting and capacity checks.
    pub coordinator_ns: u64,
    /// Collection-wide inference copy and merge outside segment admission.
    pub inference_observation_ns: u64,
    pub record_inventory_ns: u64,
    pub postings_ns: u64,
    pub inference_ns: u64,
    pub relations_ns: u64,
    pub custom_postings_ns: u64,
    pub bigrams_ns: u64,
    /// Incremental bigram merges; `bigrams_ns` is their nonadditive parent.
    pub raw_bigrams_ns: u64,
    pub fallback_bigrams_ns: u64,
    pub custom_bigrams_ns: u64,
    pub structural_facts_ns: u64,
    pub sorting_ns: u64,
    pub remainder_ns: u64,
}

impl AdmissionBreakdown {
    pub(crate) fn absorb(&mut self, other: Self) {
        self.batches += other.batches;
        self.files += other.files;
        self.records += other.records;
        self.callback_ns += other.callback_ns;
        self.total_ns += other.total_ns;
        self.coordinator_ns += other.coordinator_ns;
        self.inference_observation_ns += other.inference_observation_ns;
        self.record_inventory_ns += other.record_inventory_ns;
        self.postings_ns += other.postings_ns;
        self.inference_ns += other.inference_ns;
        self.relations_ns += other.relations_ns;
        self.custom_postings_ns += other.custom_postings_ns;
        self.bigrams_ns += other.bigrams_ns;
        self.raw_bigrams_ns += other.raw_bigrams_ns;
        self.fallback_bigrams_ns += other.fallback_bigrams_ns;
        self.custom_bigrams_ns += other.custom_bigrams_ns;
        self.structural_facts_ns += other.structural_facts_ns;
        self.sorting_ns += other.sorting_ns;
        self.remainder_ns += other.remainder_ns;
    }

    /// Exclusive incremental phases; omit the aggregate `bigrams_ns` parent.
    pub(crate) fn incremental_component_ns(&self) -> u64 {
        self.coordinator_ns
            + self.inference_observation_ns
            + self.record_inventory_ns
            + self.postings_ns
            + self.inference_ns
            + self.relations_ns
            + self.custom_postings_ns
            + self.raw_bigrams_ns
            + self.fallback_bigrams_ns
            + self.custom_bigrams_ns
            + self.structural_facts_ns
            + self.sorting_ns
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct StreamMemoryStats {
    pub active_framers: u64,
    pub active_callbacks: u64,
    pub pending_input_bytes: u64,
    pub admitted_decoder_bytes: u64,
    pub active_decoders: u64,
    pub completed_chunks: u64,
    pub unsealed_contributions: u64,
    pub private_segments: u64,
    pub private_index_family_bytes: u64,
    pub pending_and_admitted_bytes: u64,
}

impl StreamMemoryStats {
    pub(crate) fn absorb(&mut self, other: Self) {
        self.active_framers = self.active_framers.max(other.active_framers);
        self.active_callbacks = self.active_callbacks.max(other.active_callbacks);
        self.pending_input_bytes = self.pending_input_bytes.max(other.pending_input_bytes);
        self.admitted_decoder_bytes = self
            .admitted_decoder_bytes
            .max(other.admitted_decoder_bytes);
        self.active_decoders = self.active_decoders.max(other.active_decoders);
        self.completed_chunks = self.completed_chunks.max(other.completed_chunks);
        self.unsealed_contributions = self
            .unsealed_contributions
            .max(other.unsealed_contributions);
        self.private_segments = self.private_segments.max(other.private_segments);
        self.private_index_family_bytes = self
            .private_index_family_bytes
            .max(other.private_index_family_bytes);
        self.pending_and_admitted_bytes = self
            .pending_and_admitted_bytes
            .max(other.pending_and_admitted_bytes);
    }
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
    pub build_stream_peak: StreamMemoryStats,
    /// Fresh-build inference counters and retained-state estimates.
    pub inference_build: InferenceBuildStats,
    /// Opt-in fresh-build timeline and sampled worker attribution.
    pub build_profile: Option<BuildProfileStats>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct BuildProfileStats {
    /// Exclusive incremental admission work, excluding sealing/publication.
    pub admission: AdmissionBreakdown,
    pub admission_by_collection: BTreeMap<String, AdmissionBreakdown>,
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
