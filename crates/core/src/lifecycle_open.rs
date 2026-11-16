//! Explicit construction entry points for the lifecycle handle.

use std::path::PathBuf;
use std::time::Instant;

use crate::lifecycle::{Index, IndexError};
use crate::repository::{CollectionDefinition, Repository, RepositoryOptions};
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
        let definitions = crate::repository_configuration::prepare_source_definitions(
            &root,
            &definitions,
        )
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
        let repository = Repository::open_with_sources(root, definitions, options, sources)
            .map_err(IndexError::Build)?;
        Self::from_repository(repository)
    }
}

#[cfg(test)]
#[path = "lifecycle_open.test.rs"]
mod tests;
