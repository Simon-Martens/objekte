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
        self.serialized_bytes = self
            .serialized_bytes
            .saturating_add(serialized as u64);
        self.allocated_bytes = self
            .allocated_bytes
            .saturating_add(capacity as u64);
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
    pub(crate) fn snapshot() -> Self {
        STORAGE_WORK.with(std::cell::Cell::get)
    }

    #[cfg(any(test, feature = "detailed-profiling"))]
    pub(crate) fn reset() {
        STORAGE_WORK.with(|counters| counters.set(Self::ZERO));
    }
}
