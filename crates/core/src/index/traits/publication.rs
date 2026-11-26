//! Immutable segment placements in one published revision.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::model::{FileId, RecordSlot};

use super::liveness::ActiveRecords;
use super::segment::{SegmentHandle, SegmentId, SegmentReader};
use super::transfer::StorageResult;

/// A publication stamp copied by readers that decode a pinned placement.
/// Every payload or liveness change creates a new stamp.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct PlacementVersion(u64);

impl PlacementVersion {
    pub(crate) fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// One published payload and its independently versioned local liveness.
#[derive(Clone)]
pub(crate) struct PublishedSegment {
    pub(crate) version: PlacementVersion,
    pub id: SegmentId,
    pub index: SegmentHandle,
    pub active: ActiveRecords,
    pub(crate) observation: Option<crate::index::traits::inference::InferenceObservationHandle>,
}

impl crate::index::traits::segment::SegmentReaderDelegate for PublishedSegment {
    type Payload = dyn SegmentReader;

    fn payload(&self) -> &Self::Payload {
        self.index.as_ref()
    }

    fn placement_id(&self) -> Option<SegmentId> {
        Some(self.id)
    }
}

impl PublishedSegment {
    pub(crate) fn same_snapshot(&self, other: &Self) -> bool {
        self.id == other.id && self.version == other.version
    }

    pub(crate) fn retire(&mut self, slots: &[RecordSlot]) -> StorageResult<()> {
        self.active = self.active.retired(slots)?;
        self.version = PlacementVersion::new();
        Ok(())
    }

    pub(crate) fn replace_payload(&mut self, index: SegmentHandle) {
        self.index = index;
        self.version = PlacementVersion::new();
    }

    pub(crate) fn visit_live_records(
        &self,
        visit: &mut dyn FnMut(&[(&crate::model::RecordId, RecordSlot)]) -> StorageResult<()>,
    ) -> StorageResult<()> {
        crate::index::traits::segment_catalog::visit_live_routes(self, &self.active, visit)
    }

    pub(crate) fn visit_files(
        &self,
        visit: &mut dyn FnMut(&[FileId]) -> StorageResult<()>,
    ) -> StorageResult<()> {
        crate::index::traits::segment_catalog::visit_file_routes(self, visit)
    }

    pub(crate) fn construction_observation_bytes(&self) -> usize {
        self.observation
            .as_ref()
            .map_or(0, |observation| observation.estimated_bytes())
    }
}

/// One request's ordered segment placements. Native readers can share their
/// sealed group. Other readers may collect the bounded pages needed by a request.
pub(crate) type SegmentGroup = Arc<Vec<Arc<PublishedSegment>>>;
