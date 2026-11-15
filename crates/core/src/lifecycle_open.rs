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
        let root = root.into();
        let definitions =
            crate::repository_configuration::prepare_source_definitions(&root, &definitions)
                .map_err(IndexError::IncompatibleConfiguration)?;
        let started = Instant::now();
        let index = Self::build_with_sources_inner(root, definitions, options, sources)?;
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

    fn build_with_sources_inner(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        sources: SourceRegistry,
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
            return Self::from_direct_build(direct);
        }
        let repository = Repository::open_with_sources(root, definitions, options, sources)
            .map_err(IndexError::Build)?;
        Self::from_repository(repository)
    }

    fn from_direct_build(direct: DirectBuild) -> Result<Self, IndexError> {
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
                published = published.with_segment(segment)?;
            }
        }
        crate::lifecycle_compaction::seal_native_segments(&mut published)?;
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
