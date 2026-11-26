//! Source-free segment projection requests.

use std::collections::HashSet;

use crate::inventory::ActiveRecords;
use crate::model::FileId;

/// Local live slots and file rows selected for one output segment.
pub(crate) struct SegmentPartition {
    pub files: HashSet<FileId>,
    pub slots: ActiveRecords,
}

use super::{SegmentHandle, SegmentReader};
use crate::index::storage::file_catalog::FileCatalogReader;
use crate::index::storage::transfer::{StorageError, StorageResult};
use crate::inventory::FileEntry;
use crate::model::RecordSlot;
use crate::repository::state::GlobalInferenceSummary;

pub(crate) struct ProjectedSegment {
    pub payload: SegmentHandle,
    pub old_to_new: Vec<Option<RecordSlot>>,
}

pub(crate) struct ConsolidatedSegment {
    pub payload: SegmentHandle,
    pub offsets: Vec<u32>,
}

pub(crate) struct RelinkedFile {
    pub linked: bool,
    pub updated: Option<SegmentHandle>,
}

/// Source-free edits of one sealed segment. Results remain private until the
/// publication coordinator installs a successor revision.
pub(crate) trait SegmentMaintenance: Send + Sync {
    fn project_active(
        &self,
        active: &ActiveRecords,
        max_slots: usize,
        retain_file: &mut dyn FnMut(&FileEntry) -> bool,
    ) -> StorageResult<ProjectedSegment>;

    fn project_partitions(
        &self,
        partitions: &[SegmentPartition],
        catalog: &dyn FileCatalogReader,
        cancelled: &mut dyn FnMut() -> bool,
    ) -> StorageResult<Vec<SegmentHandle>>;

    fn remap_dense(&self, new_slots: &[RecordSlot]) -> StorageResult<SegmentHandle>;

    fn reconcile_final_metadata(
        &self,
        catalog: &dyn FileCatalogReader,
        decision: Option<&GlobalInferenceSummary>,
    ) -> StorageResult<Option<SegmentHandle>>;

    fn relink_file(
        &self,
        file: FileId,
        replacement: Option<&FileEntry>,
        root: Option<&std::path::Path>,
        collection_name: &str,
    ) -> StorageResult<Option<RelinkedFile>>;
}

pub(crate) fn require_maintenance(
    segment: &dyn SegmentReader,
) -> StorageResult<&dyn SegmentMaintenance> {
    segment
        .maintenance()
        .ok_or_else(|| StorageError::invalid("edit segment", "segment maintenance unavailable"))
}

impl ConsolidatedSegment {
    pub(crate) fn translate_active(
        &self,
        factory: &dyn crate::index::storage::StorageFactory,
        masks: &[&ActiveRecords],
    ) -> StorageResult<ActiveRecords> {
        translate_active(
            self.payload.descriptor().slots,
            &self.offsets,
            factory,
            masks,
        )
    }
}

pub(crate) fn translate_active(
    count: usize,
    offsets: &[u32],
    factory: &dyn crate::index::storage::StorageFactory,
    masks: &[&ActiveRecords],
) -> StorageResult<ActiveRecords> {
    use roaring::RoaringBitmap;
    let invalid = |detail| StorageError::invalid("translate consolidated membership", detail);
    if masks.len() != offsets.len() {
        return Err(invalid("consolidation active-mask count mismatch"));
    }
    let mut retired = Vec::new();
    for (index, mask) in masks.iter().enumerate() {
        let start = offsets[index];
        let end = offsets.get(index + 1).copied().unwrap_or(count as u32);
        if end.checked_sub(start) != Some(mask.slot_count()) {
            return Err(invalid("consolidation active-mask coverage mismatch"));
        }
        for slot in 0..mask.slot_count() {
            if !mask.contains(RecordSlot(slot)) {
                retired.push(RecordSlot(
                    start
                        .checked_add(slot)
                        .ok_or_else(|| invalid("consolidation active slot overflow"))?,
                ));
            }
        }
    }
    use crate::index::storage::liveness::InitialLiveness;
    use crate::query_access::membership::MembershipView;

    if retired.is_empty() {
        return Ok(factory.live_records(count, InitialLiveness::All)?);
    }
    let mut live = RoaringBitmap::new();
    live.insert_range(0..u32::try_from(count).map_err(|_| invalid("too many consolidated slots"))?);
    for slot in retired {
        live.remove(slot.0);
    }
    Ok(factory.live_records(
        count,
        InitialLiveness::Selected(MembershipView::Borrowed(&live)),
    )?)
}
