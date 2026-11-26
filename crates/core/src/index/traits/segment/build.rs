//! Ordered admission and finalization of worker contributions.

use std::sync::Arc;

use crate::construction::batch::BatchContribution;
use crate::model::CollectionId;
use crate::records::{RecordHeader, RecordVersion};
use crate::repository::configuration::{CollectionPolicy, RepositoryOptions};
use crate::repository::statistics::{AdmissionBreakdown, IncrementalSegmentProfile};
use crate::source::registry::PreparedSource;

use super::{RetainedBlock, SegmentHandle, SegmentId, SegmentPolicy};
use crate::index::traits::order::DeclaredOrderStore;
use crate::index::traits::transfer::StorageResult;

pub(crate) struct FileSegmentBuild<'a> {
    pub collection: CollectionId,
    pub source: &'a PreparedSource,
    pub policy: SegmentPolicy,
}

pub(crate) struct DirectSegmentBuild<'a> {
    pub collection: CollectionId,
    pub source_id: &'a str,
    pub source_version: u32,
    pub registration: u32,
    pub raw_covers_logical_fields: bool,
    pub bodies: DirectSegmentBodies<'a>,
    pub headers: Vec<RecordHeader>,
    pub contribution: BatchContribution,
}

pub(crate) enum DirectSegmentBodies<'a> {
    Retained(RetainedBlock),
    Versioned(&'a [RecordVersion]),
}

pub(crate) struct SegmentSeal<'a> {
    pub id: SegmentId,
    pub definition: &'a dyn CollectionPolicy,
    pub options: RepositoryOptions,
    pub collection_count: usize,
    pub pool: &'a rayon::ThreadPool,
    pub profile: Option<&'a mut IncrementalSegmentProfile>,
    pub observe_inference: bool,
    pub declared_orders: Option<Arc<dyn DeclaredOrderStore>>,
}

/// A sealed payload and its optional provisional inference observation.
pub(crate) struct BuiltSegment {
    pub index: SegmentHandle,
    pub observation: Option<crate::index::traits::inference::InferenceObservationHandle>,
}

pub(crate) trait SegmentBuilder: Send {
    fn file_count(&self) -> usize;
    fn record_count(&self) -> usize;
    fn source_bytes(&self) -> u64;
    fn at_capacity(&self) -> bool;
    fn can_admit(&self, batch: &BatchContribution) -> StorageResult<bool>;
    fn admit(
        &mut self,
        batch: BatchContribution,
        profile: Option<&mut AdmissionBreakdown>,
    ) -> StorageResult<()>;
    fn seal(self: Box<Self>, input: SegmentSeal<'_>) -> StorageResult<BuiltSegment>;
}
