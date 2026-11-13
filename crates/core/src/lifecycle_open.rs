//! Explicit construction entry points for the lifecycle handle.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use serde::Serialize;

use crate::inventory::{ActiveRecords, FileVersions};
use crate::lifecycle::{Index, IndexError, IndexRevision, PublishedIndex, PublishedSegment};
use crate::lifecycle_catalog::FileCatalog;
use crate::model::CollectionId;
use crate::repository::{
    CollectionDefinition, DirectBuild, Repository, RepositoryOptions, SnapshotMode,
    SnapshotOpenPolicy,
};
use crate::source::SourceRegistry;


/// Opt-in restore attribution. All intervals are local to the calling thread;
/// background reconciliation is measured separately by its task report.
#[derive(Clone, Debug, Default, Serialize)]
pub struct LoadProfileStats {
    pub total_ns: u64,
    pub storage: crate::storage::SnapshotLoadProfile,
    pub extra_metadata_ns: u64,
    pub catalogue_rebuild_ns: u64,
    pub file_versions_ns: u64,
    pub base_active_validation_ns: u64,
    pub segment_decode_ns: u64,
    pub segments: Vec<SegmentLoadProfile>,
    pub cross_segment_uniqueness_ns: u64,
    pub publication_setup_ns: u64,
    pub reconciliation_schedule_ns: u64,
}


#[derive(Clone, Debug, Default, Serialize)]
pub struct SegmentLoadProfile {
    pub collection: String,
    pub ordinal: u64,
    pub encoded_bytes: usize,
    pub records: u32,
    pub decode_validation_ns: u64,
    pub decode: crate::lifecycle_segment_codec::SegmentDecodeProfile,
}


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

    /// Restore through the normal checked path, returning non-persisted timing
    /// evidence for benchmark callers. Reconciliation is still scheduled.
    #[cfg(feature = "detailed-profiling")]
    pub fn load_cache_profiled(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        path: &Path,
    ) -> Result<(Self, LoadProfileStats), IndexError> {
        let mut profile = LoadProfileStats::default();
        let index = Self::load_cache_with_sources_inner(
            root,
            definitions,
            options,
            SourceRegistry::builtin(),
            path,
            Some(&mut profile),
        )?;
        Ok((index, profile))
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
        if !cfg!(test) {
            return Err(IndexError::InvalidCache(
                "deprecated snapshot loading is unavailable; use IndexStoragePolicy".into(),
            ));
        }
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
        options: RepositoryOptions,
        sources: SourceRegistry,
        path: &Path,
    ) -> Result<Self, IndexError> {
        Self::load_cache_with_sources_inner(root, definitions, options, sources, path, None)
    }

    fn load_cache_with_sources_inner(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        mut options: RepositoryOptions,
        sources: SourceRegistry,
        path: &Path,
        mut profile: Option<&mut LoadProfileStats>,
    ) -> Result<Self, IndexError> {
        if !cfg!(test) {
            return Err(IndexError::InvalidCache(
                "deprecated snapshot loading is unavailable; use IndexStoragePolicy".into(),
            ));
        }
        let standard_started = Instant::now();
        let total_started = crate::detailed_timer::start_if_enabled(profile.is_some());
        let root = root
            .into()
            .canonicalize()
            .map_err(|error| IndexError::Io(error.to_string()))?;
        options.snapshot_mode = SnapshotMode::Disabled;
        let inference_enabled = options.inference.mode != crate::inference::InferenceMode::Disabled;
        let (repository, state) = crate::storage::load_lifecycle_snapshot_checked_profiled(
            root,
            definitions,
            options,
            &sources,
            path,
            |_| Ok(()),
            profile.as_mut().map(|profile| &mut profile.storage),
        )
        .map_err(|error| match error {
            crate::storage::SnapshotLoadError::Access(message) => IndexError::Io(message),
            crate::storage::SnapshotLoadError::Invalid(message) => {
                IndexError::InvalidCache(message)
            }
        })?;
        let repository = Arc::new(repository);
        let phase = crate::detailed_timer::start_if_enabled(profile.is_some());
        let extra_files = state
            .decoded_extra_files()
            .map_err(IndexError::InvalidCache)?;
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.extra_metadata_ns = started.elapsed().as_nanos() as u64;
        }
        let phase = crate::detailed_timer::start_if_enabled(profile.is_some());
        let slots = state
            .catalog
            .iter()
            .map(|entry| {
                entry
                    .as_ref()
                    .map(|entry| {
                        Ok((
                            entry.decode_entry(&repository, &extra_files)?,
                            entry.fingerprint,
                            entry.configuration(state.configuration_identity),
                        ))
                    })
                    .transpose()
            })
            .collect::<Result<Vec<_>, String>>()
            .map_err(IndexError::InvalidCache)?;
        let catalog = FileCatalog::restored(Arc::clone(&repository), slots)
            .map_err(IndexError::InvalidCache)?;
        let metadata_ms = standard_started.elapsed().as_millis() as u64;
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.catalogue_rebuild_ns = started.elapsed().as_nanos() as u64;
        }
        let phase = crate::detailed_timer::start_if_enabled(profile.is_some());
        let file_versions =
            FileVersions::from_versions(state.file_versions).map_err(IndexError::InvalidCache)?;
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.file_versions_ns = started.elapsed().as_nanos() as u64;
        }
        let phase = crate::detailed_timer::start_if_enabled(profile.is_some());
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
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.base_active_validation_ns = started.elapsed().as_nanos() as u64;
        }
        let mut segments = Vec::with_capacity(state.segments.len());
        for (number, group) in state.segments.into_iter().enumerate() {
            let mut restored = Vec::with_capacity(group.len());
            for segment in group {
                let phase = crate::detailed_timer::start_if_enabled(profile.is_some());
                let encoded_bytes = segment.index.len();
                let ordinal = segment.ordinal;
                let records = segment.record_count;
                let mut decode_profile = profile
                    .as_ref()
                    .map(|_| crate::lifecycle_segment_codec::SegmentDecodeProfile::default());
                let index = crate::lifecycle_segment_codec::decode_cache_segment_profiled(
                    &segment,
                    &repository,
                    &catalog,
                    &extra_files,
                    decode_profile.as_mut(),
                )
                .map_err(IndexError::InvalidCache)?;
                let active = ActiveRecords::from_bitmap(index.records.len(), segment.active)
                    .map_err(IndexError::InvalidCache)?;
                if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
                    let ns = started.elapsed().as_nanos() as u64;
                    profile.segment_decode_ns = profile.segment_decode_ns.saturating_add(ns);
                    profile.segments.push(SegmentLoadProfile {
                        collection: repository.collections()[number].definition.name.clone(),
                        ordinal,
                        encoded_bytes,
                        records,
                        decode_validation_ns: ns,
                        decode: decode_profile.unwrap(),
                    });
                }
                restored.push(Arc::new(PublishedSegment {
                    ordinal: segment.ordinal,
                    index: Arc::new(index),
                    active,
                }));
            }
            segments.push(Arc::new(restored));
        }
        let phase = crate::detailed_timer::start_if_enabled(profile.is_some());
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
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.cross_segment_uniqueness_ns = started.elapsed().as_nanos() as u64;
        }
        let phase = crate::detailed_timer::start_if_enabled(profile.is_some());
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
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.publication_setup_ns = started.elapsed().as_nanos() as u64;
        }
        let phase = crate::detailed_timer::start_if_enabled(profile.is_some());
        index.start_reconcile()?;
        let available_ms = standard_started.elapsed().as_millis() as u64;
        index.update_timings_ms(|timings| {
            timings.metadata = Some(metadata_ms);
            timings.search_available = Some(available_ms);
            timings.full_available = Some(available_ms);
            if inference_enabled {
                timings.inference_full_available = Some(available_ms);
            }
        })?;
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.reconciliation_schedule_ns = started.elapsed().as_nanos() as u64;
        }
        if let (Some(started), Some(profile)) = (total_started, profile.as_mut()) {
            profile.total_ns = started.elapsed().as_nanos() as u64;
        }
        Ok(index)
    }
}


#[cfg(test)]
#[path = "lifecycle_open.test.rs"]
mod tests;
