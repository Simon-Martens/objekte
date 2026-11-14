//! Explicit construction entry points for the lifecycle handle.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use crate::inventory::FileVersions;
use crate::lifecycle::{Index, IndexError, IndexRevision, PublishedIndex};
use crate::lifecycle_catalog::FileCatalog;
use crate::repository::{CollectionDefinition, DirectBuild, Repository, RepositoryOptions};
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
    /// policy builds directly from the supplied sources.
    pub fn build_with_sources(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        sources: SourceRegistry,
    ) -> Result<Self, IndexError> {
        let started = Instant::now();
        let index = Self::build_with_sources_inner(root, definitions, options, sources, true)?;
        index.enable_owned_memory_sealing();
        let full_ms = started.elapsed().as_millis() as u64;
        index.update_timings_ms(|timings| {
            timings.search_available = Some(full_ms);
            timings.full_available = Some(full_ms);
            if options.inference.mode != crate::inference::InferenceMode::Disabled {
                timings.inference_full_available = Some(full_ms);
            }
        })?;
        Ok(index)
    }

    /// Build mutable shards for a first managed generation. The store writer
    /// encodes them directly to immutable files and the next open installs
    /// sealed readers; no temporary owned copy is made during this build.
    pub(crate) fn build_with_sources_unsealed(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        sources: SourceRegistry,
    ) -> Result<Self, IndexError> {
        Self::build_with_sources_inner(root, definitions, options, sources, false)
    }

    fn build_with_sources_inner(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        sources: SourceRegistry,
        seal_owned: bool,
    ) -> Result<Self, IndexError> {
        let root = root.into();
        if let Some(direct) = Repository::build_direct_json(
            root.clone(),
            definitions.clone(),
            options,
            sources.clone(),
        )
        .map_err(IndexError::Build)?
        {
            return Self::from_direct_build(direct, seal_owned);
        }
        let repository = Repository::open_with_sources(root, definitions, options, sources)
        .map_err(IndexError::Build)?;
        Self::from_repository_with_sealing(repository, seal_owned)
    }

    fn from_direct_build(direct: DirectBuild, seal_owned: bool) -> Result<Self, IndexError> {
        let DirectBuild {
            repository,
            catalog_slots,
            segments,
        } = direct;
        let build_ns = repository.startup().build_ns;
        let metadata_ns = repository.startup().build_discovery_ns;
        let inference_enabled =
            repository.options().inference.mode != crate::inference::InferenceMode::Disabled;
        let instance_id = crate::repository::next_repository_instance();
        let repository = Arc::new(repository);
        let mut published = PublishedIndex::from_repository(
            instance_id,
            IndexRevision::INITIAL,
            Arc::clone(&repository),
        )?;
        published.catalog = FileCatalog::restored(repository, catalog_slots)
            .map_err(IndexError::InvalidPublication)?;
        published.file_versions = FileVersions::initial(published.catalog.allocated_files());
        for group in segments {
            for segment in group {
                published = published.with_segment_sealing(segment, seal_owned, false)?.0;
            }
        }
        let index = Self::from_published(published);
        let full_ms = build_ns / 1_000_000;
        index.update_timings_ms(|timings| {
            timings.metadata = Some(metadata_ns / 1_000_000);
            timings.search_available = Some(full_ms);
            timings.full_available = Some(full_ms);
            if inference_enabled {
                timings.inference_full_available = Some(full_ms);
            }
        })?;
        Ok(index)
    }

}

#[cfg(test)]
#[path = "lifecycle_open.test.rs"]
mod tests;
