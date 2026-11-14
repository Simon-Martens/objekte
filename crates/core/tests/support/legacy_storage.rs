//! Test-only bridge for historical persisted-index oracle cases.
//! Production callers use separate `try_load` and `flush_to` operations.

use std::path::PathBuf;

use objekte_search_core::index_storage::IndexStorageTarget;
use objekte_search_core::lifecycle::{Index, IndexError};
use objekte_search_core::repository::{CollectionDefinition, RepositoryOptions};
use objekte_search_core::source::SourceRegistry;

#[allow(dead_code)]
#[derive(Clone)]
pub enum IndexStorageLocation {
    MemoryOnly,
    SystemTemporary,
    RepositoryDirectory,
    CustomDirectory(PathBuf),
}

#[derive(Clone)]
pub struct IndexStoragePolicy {
    pub location: IndexStorageLocation,
}

#[allow(dead_code)]
pub trait HistoricalSavedIndex: Sized {
    fn open_with_storage(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        storage: IndexStoragePolicy,
        sources: SourceRegistry,
    ) -> Result<Self, IndexError>;

    fn stop_sync_storage(&self) -> Result<(), IndexError>;
}

impl HistoricalSavedIndex for Index {
    fn open_with_storage(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        storage: IndexStoragePolicy,
        sources: SourceRegistry,
    ) -> Result<Self, IndexError> {
        let root = root.into();
        let target = match storage.location {
            IndexStorageLocation::MemoryOnly => {
                return Index::build_with_sources(root, definitions, options, sources);
            }
            IndexStorageLocation::SystemTemporary => IndexStorageTarget::SystemTemporary,
            IndexStorageLocation::RepositoryDirectory => IndexStorageTarget::RepositoryDirectory,
            IndexStorageLocation::CustomDirectory(path) => {
                IndexStorageTarget::CustomDirectory(path)
            }
        };
        if let Some(index) = Index::try_load_with_sources(
            &root,
            definitions.clone(),
            options,
            &target,
            sources.clone(),
        )? {
            return Ok(index);
        }
        let index = Index::build_with_sources(root, definitions, options, sources)?;
        index.flush_to(&target)?;
        Ok(index)
    }

    fn stop_sync_storage(&self) -> Result<(), IndexError> {
        Ok(())
    }
}
