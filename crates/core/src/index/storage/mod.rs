//! Backend-independent index semantics and storage contracts.

pub(crate) mod component;
pub(crate) mod file_catalog;
pub(crate) mod order;
pub(crate) mod ordered_slots;
pub(crate) mod posting;
pub(crate) mod relation;
pub(crate) mod segment;
pub(crate) mod text;
pub(crate) mod transfer;

pub(crate) mod segment_catalog;

use std::sync::Arc;

use segment_catalog::SegmentCatalogEditor;
use transfer::StorageResult;

pub(crate) trait StorageFactory: Send + Sync {
    fn segment_catalog(&self) -> StorageResult<Box<dyn SegmentCatalogEditor>>;
}

pub(crate) fn default_factory() -> Arc<dyn StorageFactory> {
    Arc::new(super::memory::MemoryStorageFactory)
}
