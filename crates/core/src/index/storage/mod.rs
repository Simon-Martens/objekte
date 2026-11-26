//! Backend-independent index semantics and storage contracts.

pub(crate) mod component;
pub(crate) mod file_catalog;
pub(crate) mod inference;
pub(crate) mod liveness;
pub(crate) mod order;
pub(crate) mod ordered_slots;
pub(crate) mod posting;
pub(crate) mod publication;
pub(crate) mod relation;
pub(crate) mod segment;
pub(crate) mod text;
pub(crate) mod transfer;
pub(crate) mod worker;

pub(crate) mod segment_catalog;

use std::sync::Arc;

use file_catalog::{FileCatalogBase, FileCatalogHandle, FileCatalogRevision};
use segment_catalog::SegmentCatalogEditor;
use transfer::StorageResult;

pub(crate) trait StorageFactory: Send + Sync {
    /// Import a sealed file-backed segment via logical blocks. A factory can
    /// decline a family it cannot preserve before beginning the import.
    fn import_file_segment(
        &self,
        _source: &dyn segment::SegmentReader,
        _layout: segment::transfer::FileSegmentLayout<'_>,
        _cancelled: &mut dyn FnMut() -> bool,
    ) -> StorageResult<Option<segment::SegmentHandle>> {
        Ok(None)
    }

    fn project_segment_partitions(
        &self,
        source: &dyn segment::SegmentReader,
        partitions: &[segment::maintenance::SegmentPartition],
        files: &dyn file_catalog::FileCatalogReader,
        cancelled: &mut dyn FnMut() -> bool,
    ) -> StorageResult<Vec<segment::SegmentHandle>> {
        segment::maintenance::require_maintenance(source)?
            .project_partitions(partitions, files, cancelled)
    }

    fn consolidate_segments(
        &self,
        _segments: &[&dyn segment::SegmentReader],
        _max_records: usize,
    ) -> StorageResult<Option<segment::maintenance::ConsolidatedSegment>> {
        Ok(None)
    }

    #[cfg(test)]
    fn base_segment(
        &self,
        _collection: &crate::repository::state::CollectionState,
        _files: &dyn file_catalog::FileCatalogReader,
    ) -> StorageResult<segment::SegmentHandle> {
        Err(transfer::StorageError::invalid(
            "project collection base",
            "base projection unavailable",
        ))
    }

    fn declared_order_store(&self) -> Option<Arc<dyn order::DeclaredOrderStore>> {
        None
    }

    fn start_file_segment(
        &self,
        input: segment::build::FileSegmentBuild<'_>,
    ) -> StorageResult<Box<dyn segment::build::SegmentBuilder>>;

    fn seal_direct_segment(
        &self,
        input: segment::build::DirectSegmentBuild<'_>,
        seal: segment::build::SegmentSeal<'_>,
    ) -> StorageResult<segment::build::BuiltSegment>;

    fn retain_record_bodies(
        &self,
        bodies: Vec<crate::records::RetainedBody>,
    ) -> StorageResult<segment::RetainedBlock>;

    fn inferred_order(
        &self,
        rows: order::InferredOrderRows,
    ) -> StorageResult<Arc<dyn order::OrderColumnReader>>;

    fn live_records(
        &self,
        slots: usize,
        initial: liveness::InitialLiveness<'_>,
    ) -> StorageResult<liveness::ActiveRecords>;

    fn prepare_file_records(
        &self,
        batch: worker::FileWorkerBatch<'_>,
    ) -> StorageResult<crate::construction::batch::BatchContribution>;

    fn prepare_direct_records(
        &self,
        batch: worker::DirectRecordBatch<'_>,
    ) -> StorageResult<worker::PreparedDirectBatch>;

    fn start_file_discovery(
        &self,
        root: std::path::PathBuf,
    ) -> StorageResult<Box<dyn file_catalog::FileDiscoveryBuilder>>;

    fn file_catalog(
        &self,
        base: &dyn FileCatalogBase,
        initial_version: crate::inventory::FileVersion,
    ) -> StorageResult<FileCatalogHandle>;

    fn restore_file_catalog(
        &self,
        base: &dyn FileCatalogBase,
        source: &dyn FileCatalogRevision,
        cancelled: &dyn Fn() -> bool,
    ) -> StorageResult<FileCatalogHandle>;

    fn segment_catalog(&self) -> StorageResult<Box<dyn SegmentCatalogEditor>>;
}

pub(crate) fn default_factory() -> Arc<dyn StorageFactory> {
    Arc::new(super::memory::MemoryStorageFactory)
}

#[cfg(test)]
pub(crate) fn test_factory() -> &'static Arc<dyn StorageFactory> {
    static FACTORY: std::sync::OnceLock<Arc<dyn StorageFactory>> = std::sync::OnceLock::new();
    FACTORY.get_or_init(default_factory)
}
