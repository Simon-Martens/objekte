//! Immutable segment placements in one published revision.

use std::sync::Arc;

use crate::lifecycle::IndexRevision;
use crate::model::{FileId, RecordSlot};

use super::file_catalog::FileCatalogHandle;
use super::liveness::ActiveRecords;
use super::segment::{SegmentDescriptor, SegmentHandle, SegmentId, SegmentReader};
use super::transfer::StorageResult;

/// One published payload and its independently versioned local liveness.
#[derive(Clone)]
pub(crate) struct PublishedSegment {
    pub id: SegmentId,
    pub index: SegmentHandle,
    pub active: ActiveRecords,
    pub(crate) observation: Option<crate::index::storage::inference::InferenceObservationHandle>,
}

impl crate::index::storage::segment::SegmentReaderDelegate for PublishedSegment {
    type Payload = dyn SegmentReader;

    fn payload(&self) -> &Self::Payload {
        self.index.as_ref()
    }

    fn placement_id(&self) -> Option<SegmentId> {
        Some(self.id)
    }
}

impl crate::index::storage::segment_catalog::SegmentRouteSource for PublishedSegment {
    fn segment_descriptor(&self) -> SegmentDescriptor {
        self.descriptor()
    }

    fn visit_live_records(
        &self,
        visit: &mut dyn FnMut(&[(&crate::model::RecordId, RecordSlot)]) -> StorageResult<()>,
    ) -> StorageResult<()> {
        crate::index::storage::segment_catalog::visit_live_routes(self, &self.active, visit)
    }

    fn visit_files(
        &self,
        visit: &mut dyn FnMut(&[FileId]) -> StorageResult<()>,
    ) -> StorageResult<()> {
        crate::index::storage::segment_catalog::visit_file_routes(self, visit)
    }
}

impl PublishedSegment {
    pub(crate) fn construction_observation_bytes(&self) -> usize {
        self.observation
            .as_ref()
            .map_or(0, |observation| observation.estimated_bytes())
    }
}

/// Borrowed view of the metadata, liveness, and segment payloads that belong
/// to one publication. Repository and lifecycle reads expose the same shape.
#[derive(Clone, Copy)]
pub(crate) struct NativeSnapshotRef<'a> {
    pub active: &'a [ActiveRecords],
    pub segments: &'a [Arc<Vec<Arc<PublishedSegment>>>],
    pub catalog: &'a FileCatalogHandle,
    pub revision: IndexRevision,
}
