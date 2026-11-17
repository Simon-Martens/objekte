//! Shared in-memory storage accounting and test-only work counters.

use serde::Serialize;

/// Unique checked-archive storage observed through one publication root.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ArchiveUsage {
    pub owners: usize,
    pub serialized_bytes: u64,
    pub allocated_bytes: u64,
    pub capacity_slack_bytes: u64,
}

impl ArchiveUsage {
    pub(crate) fn record(&mut self, serialized: usize, capacity: usize) {
        self.owners = self.owners.saturating_add(1);
        self.serialized_bytes = self.serialized_bytes.saturating_add(serialized as u64);
        self.allocated_bytes = self.allocated_bytes.saturating_add(capacity as u64);
        self.capacity_slack_bytes = self
            .capacity_slack_bytes
            .saturating_add(capacity.saturating_sub(serialized) as u64);
    }
}

/// Query-representation storage observed without materializing compatibility
/// rows or cloning bitmap/key payloads.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct StorageUsage {
    #[serde(flatten)]
    pub archive: ArchiveUsage,
    pub posting_bitmap_payload_bytes: u64,
    pub bigram_payload_bytes: u64,
    pub sorting_payload_bytes: u64,
    pub relation_payload_bytes: u64,
    pub active_and_control_metadata_bytes: u64,
    pub native_inventory_bytes: u64,
    pub native_diagnostics_bytes: u64,
    pub file_row_cache_bytes: u64,
    pub record_row_cache_bytes: u64,
    pub metadata_cache_initializations: u64,
}

impl StorageUsage {
    pub(crate) fn record_archive(
        &mut self,
        seen: &mut std::collections::HashSet<usize>,
        owner: usize,
        serialized: usize,
        capacity: usize,
    ) {
        if seen.insert(owner) {
            self.archive.record(serialized, capacity);
        }
    }
}

/// Narrow work counters used by regressions and optional detailed profiling.
///
/// Producing these counters never takes a process-global lock. In ordinary
/// release builds the hot-path methods compile to no-ops.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct StorageWorkCounters {
    pub metadata_rows_examined: u64,
    pub path_entries_examined: u64,
    pub ownership_routes_examined: u64,
    pub segment_descriptors_examined: u64,
    pub metadata_cache_initializations: u64,
    pub catalog_row_cache_initializations: u64,
    pub file_row_cache_initializations: u64,
    pub record_row_cache_initializations: u64,
    pub metadata_shards_rebuilt: u64,
    pub path_shards_rebuilt: u64,
    pub ownership_shards_rebuilt: u64,
    pub merge_candidate_probes: u64,
    pub segment_reconstructions: u64,
    pub archive_bytes_generated: u64,
    pub posting_logical_validation_passes: u64,
    pub posting_logical_visits: u64,
    pub validation_owned_key_bytes: u64,
    pub checked_archive_validations: u64,
    pub semantic_archive_visits: u64,
    pub row_cross_checks: u64,
    pub accounting_visits: u64,
    pub temporary_ordinal_trees: u64,
    pub first_batch_transfers: u64,
    pub field_container_rebuilds: u64,
    pub sort_payload_clones: u64,
    pub native_compatibility_copies: u64,
    pub inventory_copies: u64,
    pub empty_shell_finalizations: u64,
    pub build_pool_creations: u64,
    pub build_pool_destructions: u64,
    pub duplicate_id_probes: u64,
    pub duplicate_registry_bytes: u64,
}

#[cfg(any(test, feature = "detailed-profiling"))]
thread_local! {
    static STORAGE_WORK: std::cell::Cell<StorageWorkCounters> =
        const { std::cell::Cell::new(StorageWorkCounters::ZERO) };
}

impl StorageWorkCounters {
    #[cfg(any(test, feature = "detailed-profiling"))]
    pub(crate) const ZERO: Self = Self {
        metadata_rows_examined: 0,
        path_entries_examined: 0,
        ownership_routes_examined: 0,
        segment_descriptors_examined: 0,
        metadata_cache_initializations: 0,
        catalog_row_cache_initializations: 0,
        file_row_cache_initializations: 0,
        record_row_cache_initializations: 0,
        metadata_shards_rebuilt: 0,
        path_shards_rebuilt: 0,
        ownership_shards_rebuilt: 0,
        merge_candidate_probes: 0,
        segment_reconstructions: 0,
        archive_bytes_generated: 0,
        posting_logical_validation_passes: 0,
        posting_logical_visits: 0,
        validation_owned_key_bytes: 0,
        checked_archive_validations: 0,
        semantic_archive_visits: 0,
        row_cross_checks: 0,
        accounting_visits: 0,
        temporary_ordinal_trees: 0,
        first_batch_transfers: 0,
        field_container_rebuilds: 0,
        sort_payload_clones: 0,
        native_compatibility_copies: 0,
        inventory_copies: 0,
        empty_shell_finalizations: 0,
        build_pool_creations: 0,
        build_pool_destructions: 0,
        duplicate_id_probes: 0,
        duplicate_registry_bytes: 0,
    };

    #[inline]
    pub(crate) fn update(change: impl FnOnce(&mut Self)) {
        #[cfg(any(test, feature = "detailed-profiling"))]
        STORAGE_WORK.with(|counters| {
            let mut next = counters.get();
            change(&mut next);
            counters.set(next);
        });
        #[cfg(not(any(test, feature = "detailed-profiling")))]
        let _ = change;
    }

    #[cfg(any(test, feature = "detailed-profiling"))]
    #[allow(dead_code)]
    pub(crate) fn snapshot() -> Self {
        STORAGE_WORK.with(std::cell::Cell::get)
    }

    #[cfg(any(test, feature = "detailed-profiling"))]
    #[allow(dead_code)]
    pub(crate) fn reset() {
        STORAGE_WORK.with(|counters| counters.set(Self::ZERO));
    }

    pub(crate) fn take() -> Self {
        #[cfg(any(test, feature = "detailed-profiling"))]
        {
            return STORAGE_WORK.with(|counters| counters.replace(Self::ZERO));
        }
        #[cfg(not(any(test, feature = "detailed-profiling")))]
        Self::default()
    }

    pub(crate) fn merge(&mut self, other: Self) {
        #[cfg(any(test, feature = "detailed-profiling"))]
        {
            self.metadata_rows_examined = self
                .metadata_rows_examined
                .saturating_add(other.metadata_rows_examined);
            self.path_entries_examined = self
                .path_entries_examined
                .saturating_add(other.path_entries_examined);
            self.ownership_routes_examined = self
                .ownership_routes_examined
                .saturating_add(other.ownership_routes_examined);
            self.segment_descriptors_examined = self
                .segment_descriptors_examined
                .saturating_add(other.segment_descriptors_examined);
            self.metadata_cache_initializations = self
                .metadata_cache_initializations
                .saturating_add(other.metadata_cache_initializations);
            self.catalog_row_cache_initializations = self
                .catalog_row_cache_initializations
                .saturating_add(other.catalog_row_cache_initializations);
            self.file_row_cache_initializations = self
                .file_row_cache_initializations
                .saturating_add(other.file_row_cache_initializations);
            self.record_row_cache_initializations = self
                .record_row_cache_initializations
                .saturating_add(other.record_row_cache_initializations);
            self.metadata_shards_rebuilt = self
                .metadata_shards_rebuilt
                .saturating_add(other.metadata_shards_rebuilt);
            self.path_shards_rebuilt = self
                .path_shards_rebuilt
                .saturating_add(other.path_shards_rebuilt);
            self.ownership_shards_rebuilt = self
                .ownership_shards_rebuilt
                .saturating_add(other.ownership_shards_rebuilt);
            self.merge_candidate_probes = self
                .merge_candidate_probes
                .saturating_add(other.merge_candidate_probes);
            self.segment_reconstructions = self
                .segment_reconstructions
                .saturating_add(other.segment_reconstructions);
            self.archive_bytes_generated = self
                .archive_bytes_generated
                .saturating_add(other.archive_bytes_generated);
            self.posting_logical_validation_passes = self
                .posting_logical_validation_passes
                .saturating_add(other.posting_logical_validation_passes);
            self.posting_logical_visits = self
                .posting_logical_visits
                .saturating_add(other.posting_logical_visits);
            self.validation_owned_key_bytes = self
                .validation_owned_key_bytes
                .saturating_add(other.validation_owned_key_bytes);
            self.checked_archive_validations = self
                .checked_archive_validations
                .saturating_add(other.checked_archive_validations);
            self.semantic_archive_visits = self
                .semantic_archive_visits
                .saturating_add(other.semantic_archive_visits);
            self.row_cross_checks = self.row_cross_checks.saturating_add(other.row_cross_checks);
            self.accounting_visits = self
                .accounting_visits
                .saturating_add(other.accounting_visits);
            self.temporary_ordinal_trees = self
                .temporary_ordinal_trees
                .saturating_add(other.temporary_ordinal_trees);
            self.first_batch_transfers = self
                .first_batch_transfers
                .saturating_add(other.first_batch_transfers);
            self.field_container_rebuilds = self
                .field_container_rebuilds
                .saturating_add(other.field_container_rebuilds);
            self.sort_payload_clones = self
                .sort_payload_clones
                .saturating_add(other.sort_payload_clones);
            self.native_compatibility_copies = self
                .native_compatibility_copies
                .saturating_add(other.native_compatibility_copies);
            self.inventory_copies = self.inventory_copies.saturating_add(other.inventory_copies);
            self.empty_shell_finalizations = self
                .empty_shell_finalizations
                .saturating_add(other.empty_shell_finalizations);
            self.build_pool_creations = self
                .build_pool_creations
                .saturating_add(other.build_pool_creations);
            self.build_pool_destructions = self
                .build_pool_destructions
                .saturating_add(other.build_pool_destructions);
            self.duplicate_id_probes = self
                .duplicate_id_probes
                .saturating_add(other.duplicate_id_probes);
            self.duplicate_registry_bytes = self
                .duplicate_registry_bytes
                .saturating_add(other.duplicate_registry_bytes);
        }
        #[cfg(not(any(test, feature = "detailed-profiling")))]
        let _ = other;
    }

    pub(crate) fn commit_merge(other: Self) {
        Self::update(|current| current.merge(other));
    }
}
