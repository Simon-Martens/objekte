//! Explicit construction entry points for the lifecycle handle.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::inventory::{ActiveRecords, FileVersions};
use crate::lifecycle::{Index, IndexError, IndexRevision, PublishedIndex, PublishedSegment};
use crate::lifecycle_catalog::FileCatalog;
use crate::model::CollectionId;
use crate::repository::{
    CollectionDefinition, DirectBuild, Repository, RepositoryOptions, SnapshotMode,
    SnapshotOpenPolicy,
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

    fn from_direct_build(direct: DirectBuild) -> Result<Self, IndexError> {
        let DirectBuild {
            repository,
            catalog_slots,
            segments,
        } = direct;
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
        Ok(Self::from_published(published))
    }

    /// Restore a checked cache at an explicit path, then immediately start
    /// metadata reconciliation on a background worker. The returned index is
    /// searchable before any source directory is enumerated.
    pub fn load_cache(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        path: &Path,
    ) -> Result<Self, IndexError> {
        Self::load_cache_with_sources(root, definitions, options, SourceRegistry::builtin(), path)
    }

    /// Restore a selected cache, or replace it after a validated corrupt or
    /// incompatible-cache failure. Access errors still fail, and a failed
    /// source build leaves the rejected cache untouched.
    pub fn load_cache_or_rebuild_invalid(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        path: &Path,
        protected_files: &[PathBuf],
    ) -> Result<Self, IndexError> {
        let root = root.into();
        match Self::load_cache(&root, definitions.clone(), options, path) {
            Ok(index) => {
                index.protect_configuration_files(protected_files)?;
                Ok(index)
            }
            Err(IndexError::InvalidCache(_) | IndexError::IncompatibleConfiguration(_)) => {
                let index = Self::build(&root, definitions, options)?;
                index.protect_configuration_files(protected_files)?;
                index.discard_invalid_cache(path)?;
                index.flush_cache(path)?;
                Ok(index)
            }
            Err(error) => Err(error),
        }
    }

    pub fn load_cache_with_sources(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        mut options: RepositoryOptions,
        sources: SourceRegistry,
        path: &Path,
    ) -> Result<Self, IndexError> {
        let root = root
            .into()
            .canonicalize()
            .map_err(|error| IndexError::Io(error.to_string()))?;
        options.snapshot_mode = SnapshotMode::Disabled;
        let (repository, state) = crate::storage::load_lifecycle_snapshot_checked(
            root,
            definitions,
            options,
            &sources,
            path,
            |_| Ok(()),
        )
        .map_err(|error| match error {
            crate::storage::SnapshotLoadError::Access(message) => IndexError::Io(message),
            crate::storage::SnapshotLoadError::Invalid(message) => {
                IndexError::InvalidCache(message)
            }
        })?;
        let repository = Arc::new(repository);
        let slots = state
            .catalog
            .iter()
            .map(|entry| {
                entry
                    .as_ref()
                    .map(|entry| {
                        Ok((
                            entry.decode_entry()?,
                            entry.fingerprint,
                            entry.configuration,
                        ))
                    })
                    .transpose()
            })
            .collect::<Result<Vec<_>, String>>()
            .map_err(IndexError::InvalidCache)?;
        let catalog = FileCatalog::restored(Arc::clone(&repository), slots)
            .map_err(IndexError::InvalidCache)?;
        let file_versions =
            FileVersions::from_versions(state.file_versions).map_err(IndexError::InvalidCache)?;
        let mut active = Vec::with_capacity(state.base_active.len());
        for (number, bitmap) in state.base_active.into_iter().enumerate() {
            let collection = &repository.collections()[number];
            let domain = ActiveRecords::from_bitmap(collection.generation.records.len(), bitmap)
                .map_err(IndexError::InvalidCache)?;
            for slot in domain.bitmap().iter() {
                let record = &collection.generation.records[slot as usize];
                let baseline = repository
                    .files()
                    .files
                    .get(record.file.0 as usize)
                    .ok_or_else(|| IndexError::InvalidCache("base record file missing".into()))?;
                let current = catalog.entry(record.file).ok_or_else(|| {
                    IndexError::InvalidCache("active base record file retired".into())
                })?;
                if current.relative_path != baseline.relative_path
                    || current.revision != baseline.revision
                    || !current.collections.contains(&CollectionId(number as u32))
                {
                    return Err(IndexError::InvalidCache(
                        "active base record file changed without retirement".into(),
                    ));
                }
            }
            active.push(domain);
        }
        let mut segments = Vec::with_capacity(state.segments.len());
        for group in state.segments {
            let mut restored = Vec::with_capacity(group.len());
            for segment in group {
                let index = crate::lifecycle_segment_codec::decode_cache_segment(
                    &segment,
                    &repository,
                    &catalog,
                )
                .map_err(IndexError::InvalidCache)?;
                let active = ActiveRecords::from_bitmap(index.records.len(), segment.active)
                    .map_err(IndexError::InvalidCache)?;
                restored.push(Arc::new(PublishedSegment {
                    ordinal: segment.ordinal,
                    index: Arc::new(index),
                    active,
                }));
            }
            segments.push(Arc::new(restored));
        }
        for (number, collection) in repository.collections().iter().enumerate() {
            let mut ids = HashSet::new();
            for (id, slot) in &collection.id_lookup {
                if active[number].contains(*slot) {
                    ids.insert(id.clone());
                }
            }
            for segment in segments[number].iter() {
                for (id, slot) in &segment.index.id_lookup {
                    if segment.active.contains(*slot) && !ids.insert(id.clone()) {
                        return Err(IndexError::InvalidCache(
                            "duplicate active record ID across shards".into(),
                        ));
                    }
                }
            }
        }
        let revision = IndexRevision(state.revision);
        let instance_id = crate::repository::next_repository_instance();
        let mut published = PublishedIndex::from_repository(instance_id, revision, repository)?;
        published.active = active;
        published.segments = segments;
        published.catalog = catalog;
        published.file_versions = file_versions;
        published.next_segment_ordinal = state.next_segment_ordinal;
        published.saved_revision = Some(revision);
        let published = crate::lifecycle_partition::partition_base(published)?;
        let index = Index::from_published(published);
        index.mark_restored_from_cache();
        index.select_cache_destination(path, Some(revision), Vec::new())?;
        index.start_reconcile()?;
        Ok(index)
    }
}

#[cfg(test)]
#[path = "lifecycle_open.test.rs"]
mod tests;
