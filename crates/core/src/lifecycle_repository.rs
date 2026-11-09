//! Repository-layout convenience opening. Schema and cache discovery are
//! confined to this entry point; all index work uses explicit primitives.

use std::fs;
use std::path::{Path, PathBuf};

use crate::lifecycle::{Index, IndexError};
use crate::repository::RepositoryOptions;
use crate::repository_configuration::{RepositoryConfiguration, load_repository_configuration};
use crate::storage::snapshot_path_in;


/// Choices for opening the established `.objekte/collections` layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheFailurePolicy {
    /// Report a corrupt or incompatible cache and preserve it for inspection.
    Report,
    /// Delete an invalid cache after a safe source build, then write its replacement.
    Rebuild,
}


/// Whether convenience opening may inspect the selected cache.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheOpenPolicy {
    /// Restore a cache when present, otherwise build from sources.
    IfPresent,
    /// Require an existing, compatible cache.
    Required,
    /// Build from sources without reading the cache.
    Ignore,
}


/// A custom cache directory is resolved relative to the repository root.
/// With `sync_cache` disabled, opening never writes a cache.
#[derive(Clone, Debug)]
pub struct RepositoryOpenOptions {
    pub index: RepositoryOptions,
    pub cache_directory: Option<PathBuf>,
    pub cache_open: CacheOpenPolicy,
    pub cache_failure: CacheFailurePolicy,
    pub sync_cache: bool,
}


impl Default for RepositoryOpenOptions {
    fn default() -> Self {
        Self {
            index: RepositoryOptions::default(),
            cache_directory: None,
            cache_open: CacheOpenPolicy::IfPresent,
            cache_failure: CacheFailurePolicy::Rebuild,
            sync_cache: false,
        }
    }
}


impl Index {
    /// Open the configured repository at `root`. This is the library entry
    /// point that discovers schemas and an alphabet-specific cache. It returns
    /// a searchable restored view before background source reconciliation;
    /// when no cache exists, initial source indexing completes before return.
    pub fn open_repository(
        root: impl AsRef<Path>,
        options: RepositoryOpenOptions,
    ) -> Result<Self, IndexError> {
        let configuration = load_repository_configuration(root.as_ref())
            .map_err(IndexError::IncompatibleConfiguration)?;
        Self::open_configured_repository(configuration, options)
    }

    /// Open an already loaded repository configuration. This explicit entry
    /// point performs no schema discovery or parsing; callers that need
    /// opening metadata can retain their original configuration.
    pub fn open_configured_repository(
        configuration: RepositoryConfiguration,
        options: RepositoryOpenOptions,
    ) -> Result<Self, IndexError> {
        let root = configuration
            .root
            .canonicalize()
            .map_err(|error| IndexError::Io(error.to_string()))?;
        let cache_directory = match options.cache_directory {
            Some(directory) if directory.is_absolute() => directory,
            Some(directory) => root.join(directory),
            None => root.join(".objekte"),
        };
        let cache = snapshot_path_in(&cache_directory, options.index.bigram_alphabet);
        let mut definitions = configuration.definitions;
        crate::lifecycle_cache_guard::exclude_cache_artifacts(&root, &mut definitions, &cache)
            .map_err(IndexError::IncompatibleConfiguration)?;
        let mut protected = configuration.schema_files;
        protected.extend(configuration.protected_files);
        protected.sort();
        protected.dedup();

        // `symlink_metadata` keeps a broken cache symlink from being mistaken
        // for an absent cache. Access errors are never treated as a miss.
        let exists = match fs::symlink_metadata(&cache) {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(IndexError::Io(format!("{}: {error}", cache.display()))),
        };
        if !exists && options.cache_open == CacheOpenPolicy::Required {
            return Err(IndexError::InvalidCache(format!(
                "required cache is missing: {}",
                cache.display()
            )));
        }
        let index = if exists && options.cache_open != CacheOpenPolicy::Ignore {
            if options.cache_failure == CacheFailurePolicy::Rebuild {
                Self::load_cache_or_rebuild_invalid(
                    &root,
                    definitions,
                    options.index,
                    &cache,
                    &protected,
                )?
            } else {
                Self::load_cache(&root, definitions, options.index, &cache)?
            }
        } else {
            Self::build(&root, definitions, options.index)?
        };
        let saved = index.status()?.saved_revision;
        index.select_cache_destination(&cache, saved, protected)?;
        if options.sync_cache {
            index.enable_sync_cache()?;
        }
        Ok(index)
    }
}


#[cfg(test)]
#[path = "lifecycle_repository.test.rs"]
mod tests;
