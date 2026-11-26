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
pub(crate) trait SegmentMaintenance: std::any::Any + Send + Sync {
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

pub(crate) fn check_projection_source(
    source: &dyn SegmentReader,
    target: &dyn SegmentReader,
    slots: usize,
) -> StorageResult<()> {
    let descriptor = target.descriptor();
    let (old, new) = (source.source(), target.source());
    if descriptor.slots != slots
        || descriptor.collection != source.descriptor().collection
        || old.adapter != new.adapter
        || old.version != new.version
        || old.registration != new.registration
        || source.backing_kind() != target.backing_kind()
    {
        return Err(StorageError::invalid(
            "validate segment projection",
            "output has a different logical domain",
        ));
    }
    Ok(())
}

pub(crate) fn project_active_checked(
    source: &dyn SegmentReader,
    editor: &dyn SegmentMaintenance,
    active: &ActiveRecords,
    max_slots: usize,
    retain_file: &mut dyn FnMut(&FileEntry) -> bool,
) -> StorageResult<ProjectedSegment> {
    const OP: &str = "project active records";
    let slots = source.descriptor().slots;
    if active.slot_count() as usize != slots {
        return Err(StorageError::invalid(
            OP,
            "active selection covers a different domain",
        ));
    }
    if slots > max_slots {
        return Err(StorageError::ResourceLimit(
            "active projection exceeds its input budget".into(),
        ));
    }
    let result = editor.project_active(active, max_slots, retain_file)?;
    check_projection_source(source, result.payload.as_ref(), active.len() as usize)?;
    if result.old_to_new.len() != slots {
        return Err(StorageError::invalid(
            OP,
            "slot mapping has incomplete input coverage",
        ));
    }
    let mut used = roaring::RoaringBitmap::new();
    for (old, translated) in result.old_to_new.iter().enumerate() {
        let live = active.contains(RecordSlot(old as u32))?;
        match translated {
            Some(slot) if live && u64::from(slot.0) < active.len() && used.insert(slot.0) => {}
            None if !live => {}
            _ => {
                return Err(StorageError::invalid(
                    OP,
                    "mapping disagrees with the live selection",
                ));
            }
        }
    }
    Ok(result)
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
    let end = u32::try_from(count).map_err(|_| invalid("too many consolidated slots"))?;
    if masks.len() != offsets.len() || offsets.first().copied().unwrap_or(end) != 0 {
        return Err(invalid("consolidation active-mask count or start mismatch"));
    }
    let mut retired = Vec::new();
    for (index, mask) in masks.iter().enumerate() {
        let start = offsets[index];
        let end = offsets.get(index + 1).copied().unwrap_or(end);
        if end.checked_sub(start) != Some(mask.slot_count()) {
            return Err(invalid("consolidation active-mask coverage mismatch"));
        }
        for slot in 0..mask.slot_count() {
            if !mask.contains(RecordSlot(slot))? {
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
    live.insert_range(0..end);
    for slot in retired {
        live.remove(slot.0);
    }
    Ok(factory.live_records(
        count,
        InitialLiveness::Selected(MembershipView::Borrowed(&live)),
    )?)
}
