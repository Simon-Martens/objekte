//! Native in-memory index storage.

pub(crate) mod access;
pub mod bigram;
pub(crate) mod file_catalog;
pub mod posting;
pub mod relation;
pub mod segment;
pub(crate) mod segment_catalog;
pub mod sorting;

pub(crate) struct MemoryStorageFactory;

impl super::storage::StorageFactory for MemoryStorageFactory {
    fn segment_catalog(
        &self,
    ) -> super::storage::transfer::StorageResult<
        Box<dyn super::storage::segment_catalog::SegmentCatalogEditor>,
    > {
        Ok(Box::new(segment_catalog::RecordRoutes::empty()))
    }
}
