//! Immutable segment placements in one published revision.

use std::sync::Arc;

use crate::model::{FileId, RecordSlot};

use super::liveness::ActiveRecords;
use super::segment::{SegmentHandle, SegmentId, SegmentReader};
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

impl PublishedSegment {
    pub(crate) fn visit_live_records(
        &self,
        visit: &mut dyn FnMut(&[(&crate::model::RecordId, RecordSlot)]) -> StorageResult<()>,
    ) -> StorageResult<()> {
        crate::index::storage::segment_catalog::visit_live_routes(self, &self.active, visit)
    }

    pub(crate) fn visit_files(
        &self,
        visit: &mut dyn FnMut(&[FileId]) -> StorageResult<()>,
    ) -> StorageResult<()> {
        crate::index::storage::segment_catalog::visit_file_routes(self, visit)
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
