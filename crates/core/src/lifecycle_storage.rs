//! Managed immutable generations for the lifecycle index.
//!
//! The index publisher owns current query state. This module pins one complete
//! publication for a save, reuses unchanged segment files, and atomically
//! binds an independently encoded file catalog to the new manifest.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use crate::build::IndexedCollectionSegment;
use crate::generation_catalog::{GenerationCatalogBuilder, RestoredGenerationCatalog};
#[cfg(test)]
use crate::index_storage::IndexLocationResolver;
#[cfg(test)]
use crate::index_storage::IndexSynchronization;
use crate::index_storage::{
    IndexReadiness, IndexStorageFailureKind, IndexStorageStatus, IndexStorageTarget,
    ResolvedIndexStorage,
};
use crate::inventory::FileVersions;
#[cfg(test)]
use crate::lifecycle::InitialBuildTask;
use crate::lifecycle::{
    Index, IndexError, IndexRevision, IndexStatus, PublishedIndex, PublishedSegment,
    ReconciliationState,
};
use crate::lifecycle_catalog::FileCatalog;
use crate::managed_store::{ManagedStore, OpenedGeneration, SegmentInput, SegmentOrigin, StoreReadError};
use crate::model::{CollectionId, RecordId};
use crate::repository::{CollectionDefinition, Repository, RepositoryOptions};
use crate::segment_format::SegmentTrust;
use crate::source::SourceRegistry;

pub(crate) struct ManagedPersistence {
    pub(crate) store: ManagedStore,
    durable_generation: Option<u64>,
    last_saved_publication: Option<(u64, IndexRevision)>,
    historical_base: Option<Arc<RestoredGenerationCatalog>>,
    pub(crate) last_error: Option<String>,
    pub(crate) last_error_kind: Option<IndexStorageFailureKind>,
    validation: ValidationOpenStats,
}

#[derive(Clone, Default)]
pub(crate) struct ValidationOpenStats {
    pub full: usize,
    pub trusted: usize,
}

#[derive(Clone)]
pub(crate) struct StorageStatusSnapshot {
    durable_generation: Option<u64>,
    last_error: Option<String>,
    last_error_kind: Option<IndexStorageFailureKind>,
    managed_directory: PathBuf,
    validation: ValidationOpenStats,
}

impl ManagedPersistence {
    pub(crate) fn status_snapshot(&self) -> StorageStatusSnapshot {
        StorageStatusSnapshot {
            durable_generation: self.durable_generation,
            last_error: self.last_error.clone(),
            last_error_kind: self.last_error_kind,
            managed_directory: self.store.path().to_path_buf(),
            validation: self.validation.clone(),
        }
    }

    pub(crate) fn open(resolved: &ResolvedIndexStorage) -> Result<Self, String> {
        Ok(Self::from_store(ManagedStore::open(resolved)?))
    }

    pub(crate) fn open_existing(
        resolved: &ResolvedIndexStorage,
    ) -> Result<Option<Self>, IndexError> {
        Ok(ManagedStore::open_existing(resolved)
            .map_err(IndexError::from)?
            .map(Self::from_store))
    }

    fn from_store(store: ManagedStore) -> Self {
        Self {
            store,
            durable_generation: None,
            last_saved_publication: None,
            historical_base: None,
            last_error: None,
            last_error_kind: None,
            validation: ValidationOpenStats::default(),
        }
    }

    pub(crate) fn mark_opened(
        &mut self,
        publication: &PublishedIndex,
        generation: u64,
        historical: Arc<RestoredGenerationCatalog>,
        validation: ValidationOpenStats,
    ) -> Result<(), String> {
        self.durable_generation = Some(generation);
        self.last_saved_publication = Some((publication.instance_id, publication.revision));
        self.historical_base = Some(historical);
        self.validation = validation;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn flush(
        &mut self,
        publication: &PublishedIndex,
    ) -> Result<IndexRevision, IndexError> {
        if !publication.catalog.construction_complete() {
            return Err(IndexError::IncompleteBuild);
        }
        let generation = publication.revision.0;
        if self.durable_generation == Some(generation) {
            self.last_error = None;
            self.last_error_kind = None;
            return Ok(publication.revision);
        }
        if self
            .durable_generation
            .is_some_and(|previous| generation <= previous)
        {
            return Err(IndexError::StoreConflict(
                "published revision did not advance beyond durable generation".into(),
            ));
        }
        self.flush_generation(publication, generation)?;
        Ok(publication.revision)
    }

    pub(crate) fn flush_explicit(
        &mut self,
        publication: &PublishedIndex,
    ) -> Result<(u64, bool, u64), IndexError> {
        if !publication.catalog.construction_complete() {
            return Err(IndexError::IncompleteBuild);
        }
        if self.last_saved_publication == Some((publication.instance_id, publication.revision))
            && let Some(expected) = self.durable_generation
        {
            let current = self.store.current_generation().map_err(IndexError::from)?;
            if current != Some(expected) {
                return Err(IndexError::StoreConflict(
                    "destination generation changed since this index saved it".into(),
                ));
            }
            self.last_error = None;
            self.last_error_kind = None;
            return Ok((expected, true, 0));
        }
        let actual = match self.store.read_generation_readonly() {
            Ok(opened) => opened.map(|opened| opened.generation),
            Err(StoreReadError::Incompatible(_)) => None,
            Err(error) => return Err(IndexError::from(error)),
        };
        if self.durable_generation.is_some() && self.durable_generation != actual {
            return Err(IndexError::StoreConflict(
                "destination generation changed since this index observed it".into(),
            ));
        }
        self.durable_generation = actual;
        let generation = actual.unwrap_or(0).checked_add(1).ok_or_else(|| {
            IndexError::StoreConflict("destination generation counter overflow".into())
        })?;
        let stats = self.flush_generation(publication, generation)?;
        Ok((generation, false, stats.segment_bytes_written))
    }

    fn flush_generation(
        &mut self,
        publication: &PublishedIndex,
        generation: u64,
    ) -> Result<crate::managed_store::CommitStats, IndexError> {
        // Rebuild a compact historical table when it has grown far beyond the
        // metadata this publication still references.
        let referenced_files = publication
            .segments
            .iter()
            .flat_map(|group| group.iter())
            .map(|segment| segment.index.file_count())
            .sum::<usize>();
        let previous_history = self
            .historical_base
            .as_ref()
            .map_or(0, |base| base.historical_count());
        let rebase_history = previous_history > referenced_files.saturating_mul(2).max(64);
        let catalog = if rebase_history {
            GenerationCatalogBuilder::from_publication_with_history(publication, &[])
        } else if let Some(base) = &self.historical_base {
            GenerationCatalogBuilder::from_publication_with_persisted_history(
                publication,
                Arc::clone(base),
            )
        } else {
            GenerationCatalogBuilder::from_publication_with_history(publication, &[])
        };
        let mut catalog = catalog.map_err(IndexError::InvalidPublication)?;
        catalog.set_store_generation(generation);
        let mut inputs = Vec::new();
        for (number, group) in publication.segments.iter().enumerate() {
            let collection = CollectionId(number as u32);
            let definition = &publication.repository.collections()[number].definition;
            let options = publication.repository.options();
            for segment in group.iter() {
                if segment.index.collection != collection || segment.ordinal == 0 {
                    return Err(IndexError::InvalidPublication(
                        "published segment identity invalid".into(),
                    ));
                }
                let ordinal = segment.ordinal;
                if let Some(persisted) = &segment.index.persisted {
                    // Translate this segment's metadata references before the
                    // catalog is committed. The segment itself is decoded only
                    // when its file is written, so memory stays bounded to one
                    // output segment at a time.
                    let base = publication.catalog.persisted_base().ok_or_else(|| {
                        IndexError::InvalidPublication("persisted segment catalog missing".into())
                    })?;
                    let references = (0..persisted.file_count())
                        .map(|position| {
                            let reference = persisted
                                .metadata_ref_at(position)
                                .map_err(IndexError::InvalidCache)?
                                .ok_or_else(|| {
                                    IndexError::InvalidCache(
                                        "persisted segment file reference missing".into(),
                                    )
                                })?;
                            let entry = base
                                .entry_for_ref(reference)
                                .map_err(IndexError::InvalidCache)?
                                .ok_or_else(|| {
                                    IndexError::InvalidCache(
                                        "persisted segment metadata missing".into(),
                                    )
                                })?;
                            catalog
                                .intern(&entry)
                                .map_err(IndexError::InvalidPublication)
                        })
                        .collect::<Result<Vec<_>, IndexError>>()?;
                    let source_index = Arc::clone(&segment.index);
                    let base = Arc::clone(base);
                    inputs.push(SegmentInput::NewStream {
                        origin: SegmentOrigin::Validated,
                        collection,
                        ordinal,
                        active: &segment.active,
                        write: Box::new(move |file, metadata| {
                            let index = materialize_for_rewrite(
                                &source_index,
                                ordinal,
                                definition,
                                options,
                                collection,
                                &base,
                            )
                            .map_err(|error| format!("{error}"))?;
                            if index.file_count() != references.len() {
                                return Err("segment metadata reference count changed".into());
                            }
                            let mut position = 0usize;
                            let mut intern = |entry: &crate::inventory::FileEntry| {
                                let expected = index
                                    .files
                                    .get(position)
                                    .ok_or("segment metadata reference overflow")?;
                                if entry != &expected.entry {
                                    return Err("segment metadata reference order changed".into());
                                }
                                let reference = *references
                                    .get(position)
                                    .ok_or("segment metadata reference missing")?;
                                position += 1;
                                Ok(reference)
                            };
                            crate::hybrid_segment_codec::write_sealed_segment(
                                &index,
                                ordinal,
                                options,
                                file,
                                &mut intern,
                                metadata,
                            )?;
                            if position != references.len() {
                                return Err("segment metadata reference count changed".into());
                            }
                            Ok(())
                        }),
                    });
                } else {
                    let index = Arc::clone(&segment.index);
                    let references = index
                        .files
                        .iter()
                        .map(|file| catalog.intern(&file.entry))
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(IndexError::InvalidPublication)?;
                    let write_index = Arc::clone(&index);
                    inputs.push(SegmentInput::NewStream {
                        origin: SegmentOrigin::Validated,
                        collection,
                        ordinal,
                        active: &segment.active,
                        write: Box::new(move |file, metadata| {
                            let mut position = 0usize;
                            let mut intern = |entry: &crate::inventory::FileEntry| {
                                let expected = write_index
                                    .files
                                    .get(position)
                                    .ok_or("segment metadata reference overflow")?;
                                if entry != &expected.entry {
                                    return Err("segment metadata reference order changed".into());
                                }
                                let reference = *references
                                    .get(position)
                                    .ok_or("segment metadata reference missing")?;
                                position += 1;
                                Ok(reference)
                            };
                            if write_index.raw.is_sealed() {
                                crate::hybrid_segment_codec::write_sealed_segment(
                                    &write_index,
                                    ordinal,
                                    options,
                                    file,
                                    &mut intern,
                                    metadata,
                                )?;
                            } else {
                                crate::hybrid_segment_codec::write_segment(
                                    &write_index,
                                    ordinal,
                                    definition,
                                    options,
                                    file,
                                    &mut intern,
                                    metadata,
                                )?;
                            }
                            if position != references.len() {
                                return Err("segment metadata reference count changed".into());
                            }
                            Ok(())
                        }),
                    });
                }
            }
        }
        let catalog_bytes: Arc<[u8]> =
            Arc::from(catalog.encode().map_err(IndexError::InvalidPublication)?);
        // Validate before the manifest commit so no fallible catalog decode can
        // turn a committed save into a reported failure.
        let validated_history = Arc::new(
            RestoredGenerationCatalog::decode_arc(
                Arc::clone(&catalog_bytes),
                publication.configuration_identity,
                publication.repository.collections().len(),
            )
            .map_err(IndexError::InvalidPublication)?,
        );
        if validated_history.expected_segment_count as usize
            != publication
                .segments
                .iter()
                .map(|group| group.len())
                .sum::<usize>()
        {
            return Err(IndexError::InvalidPublication(
                "catalog expected segment count differs from publication".into(),
            ));
        }
        let committed = self
            .store
            .commit_generation_with_catalog_typed(
                self.durable_generation,
                generation,
                &inputs,
                Arc::clone(&catalog_bytes),
            )
            .map_err(IndexError::from)?;
        drop(inputs);
        self.durable_generation = Some(generation);
        self.last_saved_publication = Some((publication.instance_id, publication.revision));
        self.historical_base = Some(validated_history);
        self.last_error = None;
        self.last_error_kind = None;
        Ok(committed)
    }
}

/// A changed current metadata ref cannot be reused in the next catalog.
/// Materialize only that one affected segment for its maintenance rewrite.
fn materialize_for_rewrite(
    index: &IndexedCollectionSegment,
    ordinal: u64,
    definition: &CollectionDefinition,
    options: RepositoryOptions,
    collection: CollectionId,
    catalog: &RestoredGenerationCatalog,
) -> Result<IndexedCollectionSegment, String> {
    let persisted = index.persisted.as_ref().ok_or("segment is not persisted")?;
    let mut entries = HashMap::with_capacity(persisted.file_count());
    for position in 0..persisted.file_count() {
        let reference = persisted
            .metadata_ref_at(position)?
            .ok_or("persisted segment file reference missing")?;
        let entry = catalog
            .entry_for_ref(reference)?
            .ok_or("persisted segment metadata missing")?;
        entries.insert(reference, entry);
    }
    crate::hybrid_segment_codec::decode_segment_sealed_with_trust(
        persisted.reader(),
        ordinal,
        definition,
        options,
        collection,
        (&index.source_id, index.source_version),
        index.record_count() as u32,
        &|reference| entries.get(&reference),
        SegmentTrust::Untrusted,
    )
}

pub(crate) fn storage_status(
    status: &IndexStatus,
    managed: Option<&StorageStatusSnapshot>,
    _synchronizing: bool,
) -> IndexStorageStatus {
    let source_reconciliation_complete = matches!(
        status.reconciliation,
        ReconciliationState::NotStarted | ReconciliationState::Complete
    );
    let Some(managed) = managed else {
        return IndexStorageStatus {
            readiness: if status.initial_construction_complete {
                IndexReadiness::Searchable
            } else {
                IndexReadiness::Building
            },
            source_reconciliation_complete,
            current_revision: status.revision,
            last_saved_revision: status.saved_revision,
            durable_generation: None,
            #[cfg(test)]
            synchronization: IndexSynchronization::MemoryOnly,
            last_error: None,
            last_error_kind: None,
            managed_directory: None,
            segments_full_validated: 0,
            segments_write_validated: 0,
        };
    };
    IndexStorageStatus {
        readiness: if !status.initial_construction_complete {
            IndexReadiness::Building
        } else {
            IndexReadiness::Searchable
        },
        source_reconciliation_complete,
        current_revision: status.revision,
        last_saved_revision: status.saved_revision,
        durable_generation: managed.durable_generation,
        #[cfg(test)]
        synchronization: if _synchronizing {
            IndexSynchronization::Synchronizing
        } else if managed.last_error.is_some() {
            IndexSynchronization::Failed
        } else if managed.durable_generation == Some(status.revision.0) {
            IndexSynchronization::Synchronized
        } else {
            IndexSynchronization::Pending
        },
        last_error: managed.last_error.clone(),
        last_error_kind: managed.last_error_kind,
        managed_directory: Some(managed.managed_directory.clone()),
        segments_full_validated: managed.validation.full,
        segments_write_validated: managed.validation.trusted,
    }
}

#[cfg(test)]
fn read_existing_generation(store: &ManagedStore) -> Result<Option<OpenedGeneration>, IndexError> {
    store.read_generation().map_err(IndexError::from)
}

impl Index {
    /// Load only the selected committed generation. Absence is distinct from
    /// corruption and this operation never creates or updates a store.
    pub fn try_load(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        target: &IndexStorageTarget,
    ) -> Result<Option<Self>, IndexError> {
        Self::try_load_with_sources(
            root,
            definitions,
            options,
            target,
            SourceRegistry::builtin(),
        )
    }

    pub fn try_load_with_sources(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        target: &IndexStorageTarget,
        sources: SourceRegistry,
    ) -> Result<Option<Self>, IndexError> {
        let started = Instant::now();
        target
            .validate()
            .map_err(IndexError::IncompatibleConfiguration)?;
        let root = root.into();
        let definitions = IndexStorageTarget::prepare_logical_definitions(&root, &definitions)
            .map_err(IndexError::IncompatibleConfiguration)?;
        let resolved = target
            .resolve_logical(&root, &definitions, options, &sources)
            .map_err(IndexError::IncompatibleConfiguration)?;
        let Some(store) = ManagedStore::open_existing(&resolved).map_err(IndexError::from)? else {
            return Ok(None);
        };
        let Some(opened) = store.read_generation_readonly().map_err(IndexError::from)? else {
            return Ok(None);
        };
        restore_generation(
            &resolved,
            definitions,
            options,
            sources,
            opened,
            started,
            true,
        )
        .map(Some)
    }

    /// Start a managed initial build with catalogue and batch publications.
    /// A complete stored generation is returned as an already-ready task;
    /// an absent generation is flushed only after construction completes.
    #[cfg(test)]
    pub(crate) fn start_incremental_build_with_storage(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        storage: IndexLocationResolver,
        sources: SourceRegistry,
    ) -> Result<InitialBuildTask, IndexError> {
        let started = Instant::now();
        let root = root.into();
        let prepared = storage
            .prepare(&root, &definitions, options, &sources)
            .map_err(IndexError::IncompatibleConfiguration)?;
        if prepared.resolved.managed_directory.is_none() {
            return Self::start_incremental_build_with_sources(
                root,
                prepared.definitions,
                options,
                sources,
            );
        }
        let store = ManagedStore::open(&prepared.resolved).map_err(IndexError::Io)?;
        if let Some(opened) = read_existing_generation(&store)? {
            let index = restore_generation(
                &prepared.resolved,
                prepared.definitions,
                options,
                sources,
                opened,
                started,
                false,
            )?;
            index.set_sync_storage(true)?;
            store
                .cleanup_deprecated_snapshots()
                .map_err(IndexError::Io)?;
            return InitialBuildTask::from_ready(index);
        }
        let resolved = prepared.resolved;
        Self::start_incremental_build_with_sources_and_completion(
            root,
            prepared.definitions,
            options,
            sources,
            true,
            move |index| {
                index.attach_storage(resolved)?;
                index.flush_storage()?;
                index.install_committed_sealed_segments()?;
                index.set_sync_storage(true)?;
                store
                    .cleanup_deprecated_snapshots()
                    .map_err(IndexError::Io)?;
                Ok(())
            },
        )
    }

    /// Open one memory-only or managed index from the paired storage policy.
    /// A valid durable generation becomes searchable without source discovery;
    /// source reconciliation then runs independently in the background.
    #[cfg(test)]
    pub(crate) fn open_with_storage(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        storage: IndexLocationResolver,
        sources: SourceRegistry,
    ) -> Result<Self, IndexError> {
        let started = Instant::now();
        let root = root.into();
        let prepared = storage
            .prepare(&root, &definitions, options, &sources)
            .map_err(IndexError::IncompatibleConfiguration)?;
        if prepared.resolved.managed_directory.is_none() {
            return Self::build_with_sources(root, prepared.definitions, options, sources);
        }
        let store = ManagedStore::open(&prepared.resolved).map_err(IndexError::Io)?;
        if let Some(opened) = read_existing_generation(&store)? {
            let index = restore_generation(
                &prepared.resolved,
                prepared.definitions,
                options,
                sources,
                opened,
                started,
                false,
            )?;
            index.set_sync_storage(true)?;
            store
                .cleanup_deprecated_snapshots()
                .map_err(IndexError::Io)?;
            return Ok(index);
        }
        let index =
            Self::build_with_sources_unsealed(root, prepared.definitions, options, sources)?;
        index.attach_storage(prepared.resolved)?;
        index.flush_storage()?;
        index.install_committed_sealed_segments()?;
        index.set_sync_storage(true)?;
        store
            .cleanup_deprecated_snapshots()
            .map_err(IndexError::Io)?;
        Ok(index)
    }
}

fn restore_generation(
    resolved: &ResolvedIndexStorage,
    definitions: Vec<CollectionDefinition>,
    options: RepositoryOptions,
    sources: SourceRegistry,
    opened: OpenedGeneration,
    started: Instant,
    readonly: bool,
) -> Result<Index, IndexError> {
    #[cfg(feature = "detailed-profiling")]
    let phase_started = Instant::now();
    let bytes = opened
        .catalog
        .as_ref()
        .ok_or_else(|| IndexError::InvalidCache("generation catalog missing".into()))?;
    let restored = Arc::new(
        RestoredGenerationCatalog::decode_arc(
            Arc::clone(bytes),
            resolved.configuration_identity,
            definitions.len(),
        )
        .map_err(IndexError::InvalidCache)?,
    );
    #[cfg(feature = "detailed-profiling")]
    let catalog_validation_ns = phase_started.elapsed().as_nanos();
    if restored.revision != opened.generation {
        return Err(IndexError::InvalidCache(
            "catalog and manifest revisions differ".into(),
        ));
    }
    if restored.expected_segment_count as usize != opened.segments.len() {
        return Err(IndexError::InvalidCache(
            "catalog expected segment count differs from discovered segments".into(),
        ));
    }
    let mut owner = Repository::empty_for_managed_generation(
        resolved.repository_root.clone(),
        definitions,
        options,
        sources,
    )
    .map_err(IndexError::Build)?;
    for (collection, summary) in owner.collections.iter_mut().zip(&restored.inference) {
        if summary.as_ref().is_some_and(|summary| {
            summary.report.mode != options.inference.mode
                || summary.report.policy != options.inference.policy
                || summary.diagnostics.len() != summary.report.fields.len()
        }) {
            return Err(IndexError::InvalidCache(
                "managed inference summary differs from configuration".into(),
            ));
        }
        collection.global_inference = summary.clone();
    }
    let owner = Arc::new(owner);
    #[cfg(feature = "detailed-profiling")]
    let owner_ns = phase_started.elapsed().as_nanos() - catalog_validation_ns;
    let metadata_ms = started.elapsed().as_millis() as u64;
    let count = owner.collections().len();
    let mut groups = vec![Vec::new(); count];
    let mut ordinals = HashSet::with_capacity(opened.segments.len());
    for segment in &opened.segments {
        let collection = segment.descriptor.collection;
        let ordinal = segment.descriptor.ordinal;
        let active = &segment.descriptor.active;
        let number = collection.0 as usize;
        if number >= count
            || ordinal == 0
            || ordinal >= restored.next_segment_ordinal
            || !ordinals.insert(ordinal)
        {
            return Err(IndexError::InvalidCache(
                "manifest segment collection or ordinal invalid".into(),
            ));
        }
        let configured = &owner.collections()[number];
        let metadata: Arc<dyn crate::segment_metadata::SegmentMetadataRefs> = restored.clone();
        let decoded = crate::hybrid_segment_codec::decode_segment_sealed_persisted_with_trust(
            Arc::clone(&segment.reader),
            ordinal,
            &configured.definition,
            options,
            collection,
            (configured.source.id(), configured.source.version()),
            active.slot_count(),
            metadata,
            segment.trust(),
        )
        .map_err(IndexError::InvalidCache)?;
        if decoded.record_count() != active.slot_count() as usize {
            return Err(IndexError::InvalidCache(
                "segment active mask slot count differs from records".into(),
            ));
        }
        groups[number].push(Arc::new(PublishedSegment {
            ordinal,
            index: Arc::new(decoded),
            active: active.clone(),
        }));
    }
    #[cfg(feature = "detailed-profiling")]
    let segment_install_ns = phase_started.elapsed().as_nanos() - catalog_validation_ns - owner_ns;
    let catalog = FileCatalog::restored_persisted(Arc::clone(&owner), Arc::clone(&restored))
        .map_err(IndexError::InvalidCache)?;
    let file_versions = FileVersions::from_persisted(Arc::clone(&restored));
    #[cfg(feature = "detailed-profiling")]
    let catalog_install_ns =
        phase_started.elapsed().as_nanos() - catalog_validation_ns - owner_ns - segment_install_ns;
    for group in &groups {
        validate_live_records(group, &restored)?;
    }
    #[cfg(feature = "detailed-profiling")]
    let live_validation_ns = phase_started.elapsed().as_nanos()
        - catalog_validation_ns
        - owner_ns
        - segment_install_ns
        - catalog_install_ns;
    let instance_id = crate::repository::next_repository_instance();
    let revision = IndexRevision(opened.generation);
    let mut published = PublishedIndex::from_repository(instance_id, revision, owner)?;
    if published.configuration_identity != resolved.configuration_identity {
        return Err(IndexError::IncompatibleConfiguration(
            "restored repository identity differs from selected store".into(),
        ));
    }
    published.segments = groups.into_iter().map(Arc::new).collect();
    published.catalog = catalog;
    published.file_versions = file_versions;
    published.next_segment_ordinal = restored.next_segment_ordinal;
    published.saved_revision = Some(revision);
    published.processed_files = published.catalog.active_files() as usize;
    let index = Index::from_published(published);
    index.mark_restored_from_storage();
    if let Some(path) = &resolved.managed_directory {
        index.mark_loaded_origin(path.clone())?;
    }
    if readonly {
        index.attach_storage_existing(resolved.clone())?;
    } else {
        index.attach_storage(resolved.clone())?;
    }
    let validation = ValidationOpenStats {
        full: opened
            .segments
            .iter()
            .filter(|segment| !segment.descriptor.write_validated)
            .count(),
        trusted: opened
            .segments
            .iter()
            .filter(|segment| segment.descriptor.write_validated)
            .count(),
    };
    index.mark_storage_opened(opened.generation, restored, validation)?;
    let available_ms = started.elapsed().as_millis() as u64;
    index.update_timings_ms(|timings| {
        timings.metadata = Some(metadata_ms);
        timings.search_available = Some(available_ms);
        timings.full_available = Some(available_ms);
    })?;
    index.start_reconcile()?;
    #[cfg(feature = "detailed-profiling")]
    if std::env::var_os("OBJEKTE_DIRECT_OPEN_PHASES").is_some() {
        eprintln!(
            "direct-restore catalog_validation_ns={} owner_ns={} segment_install_ns={} catalog_install_ns={} live_validation_ns={} publication_ns={} total_ns={}",
            catalog_validation_ns,
            owner_ns,
            segment_install_ns,
            catalog_install_ns,
            live_validation_ns,
            phase_started.elapsed().as_nanos().saturating_sub(
                catalog_validation_ns
                    + owner_ns
                    + segment_install_ns
                    + catalog_install_ns
                    + live_validation_ns
            ),
            phase_started.elapsed().as_nanos(),
        );
    }
    Ok(index)
}

/// Merge the already validated per-segment identity orders using one cursor
/// and one pending identity per segment. This also checks live ownership
/// without decoding every file metadata leaf into the published catalog.
fn validate_live_records(
    group: &[Arc<PublishedSegment>],
    catalog: &RestoredGenerationCatalog,
) -> Result<(), IndexError> {
    let mut positions = vec![0usize; group.len()];
    let mut next = BinaryHeap::<Reverse<(RecordId, usize)>>::new();
    for number in 0..group.len() {
        enqueue_live_record(group, &mut positions, &mut next, number)?;
    }
    let mut previous: Option<RecordId> = None;
    while let Some(Reverse((id, number))) = next.pop() {
        let segment = &group[number];
        let slot = segment.index.identity_order[positions[number] - 1];
        let record = segment
            .index
            .try_record(slot)
            .map_err(IndexError::InvalidCache)?
            .ok_or_else(|| IndexError::InvalidCache("active record slot missing".into()))?;
        if previous.as_ref() == Some(&id)
            || !catalog.is_current(record.file)
            || catalog.revision_for_ref(record.file.0) != Some(record.revision)
            || catalog.contains_collection_for_ref(record.file.0, segment.index.collection)
                != Some(true)
        {
            return Err(IndexError::InvalidCache(
                "active record source or ID mismatch".into(),
            ));
        }
        previous = Some(id);
        enqueue_live_record(group, &mut positions, &mut next, number)?;
    }
    Ok(())
}

fn enqueue_live_record(
    group: &[Arc<PublishedSegment>],
    positions: &mut [usize],
    next: &mut BinaryHeap<Reverse<(RecordId, usize)>>,
    number: usize,
) -> Result<(), IndexError> {
    let segment = &group[number];
    while let Some(&slot) = segment.index.identity_order.get(positions[number]) {
        positions[number] += 1;
        if segment.active.contains(slot) {
            let record = segment
                .index
                .try_record(slot)
                .map_err(IndexError::InvalidCache)?
                .ok_or_else(|| IndexError::InvalidCache("active record slot missing".into()))?;
            next.push(Reverse((record.id.clone(), number)));
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "lifecycle_storage.test.rs"]
mod tests;
