//! Explicit construction entry points for the lifecycle handle.

use std::path::PathBuf;

use crate::lifecycle::{Index, IndexError};
use crate::repository::{
    CollectionDefinition, Repository, RepositoryOptions, SnapshotMode, SnapshotOpenPolicy,
};
use crate::source::SourceRegistry;

impl Index {
    /// Build from exactly the supplied definitions and source root. A relative
    /// root resolves against the process working directory once. This call
    /// does not discover schemas or caches, and it never writes a cache.
    pub fn build(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
    ) -> Result<Self, IndexError> {
        Self::build_with_sources(root, definitions, options, SourceRegistry::builtin())
    }

    /// Registered-reader equivalent of [`Self::build`]. The explicit build
    /// policy takes precedence over the legacy repository snapshot mode.
    pub fn build_with_sources(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        mut options: RepositoryOptions,
        sources: SourceRegistry,
    ) -> Result<Self, IndexError> {
        options.snapshot_mode = SnapshotMode::Disabled;
        let repository = Repository::open_with_sources_and_snapshot_policy(
            root,
            definitions,
            options,
            sources,
            SnapshotOpenPolicy::new(SnapshotMode::Disabled, None),
        )
        .map_err(IndexError::Build)?;
        Self::from_repository(repository)
    }
}

#[cfg(test)]
#[path = "lifecycle_open.test.rs"]
mod tests;
