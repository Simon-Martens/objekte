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
use crate::index_storage::{
    IndexReadiness, IndexResidency, IndexStorageFailureKind, IndexStoragePolicy,
    IndexStorageStatus, IndexSynchronization, ResolvedIndexStorage,
};
use crate::inventory::FileVersions;
use crate::lifecycle::{
    Index, IndexError, IndexRevision, IndexStatus, InitialBuildTask, PublishedIndex,
    PublishedSegment, ReconciliationState,
};
use crate::lifecycle_catalog::FileCatalog;
use crate::managed_store::{ManagedStore, OpenedGeneration, SegmentInput, SegmentOrigin};
use crate::model::{CollectionId, RecordId};
use crate::repository::{CollectionDefinition, Repository, RepositoryOptions};
use crate::segment_format::{SegmentTrust, VALIDATOR_VERSION};
use crate::source::SourceRegistry;

struct DurableSegment {
    name: String,
    index: Arc<IndexedCollectionSegment>,
    catalog: Option<Arc<RestoredGenerationCatalog>>,
    /// Whether each encoded file ref points at the mutable current-catalog
    /// slot. Historical-tagged refs remain stable when the catalog changes.
    current_refs: Vec<bool>,
}

pub(crate) struct ManagedPersistence {
    pub(crate) store: ManagedStore,
    residency: IndexResidency,
    durable_generation: Option<u64>,
    durable_segments: HashMap<(CollectionId, u64), DurableSegment>,
    historical_base: Option<Arc<RestoredGenerationCatalog>>,
    pub(crate) last_error: Option<String>,
    pub(crate) last_error_kind: Option<IndexStorageFailureKind>,
    validation: ValidationOpenStats,
}

#[derive(Clone, Default)]
pub(crate) struct ValidationOpenStats {
    pub full: usize,
    pub trusted: usize,
    pub written: usize,
    pub error: Option<String>,
    pub error_kind: Option<IndexStorageFailureKind>,
}

#[derive(Clone)]
pub(crate) struct StorageStatusSnapshot {
    residency: IndexResidency,
    durable_generation: Option<u64>,
    last_error: Option<String>,
    last_error_kind: Option<IndexStorageFailureKind>,
    managed_directory: PathBuf,
    validation: ValidationOpenStats,
}

impl ManagedPersistence {
    pub(crate) fn status_snapshot(&self) -> StorageStatusSnapshot {
        StorageStatusSnapshot {
            residency: self.residency,
            durable_generation: self.durable_generation,
            last_error: self.last_error.clone(),
            last_error_kind: self.last_error_kind,
            managed_directory: self.store.path().to_path_buf(),
            validation: self.validation.clone(),
        }
    }

    pub(crate) fn open(resolved: &ResolvedIndexStorage) -> Result<Self, String> {
        Ok(Self {
            store: ManagedStore::open(resolved)?,
            residency: resolved.residency,
            durable_generation: None,
            durable_segments: HashMap::new(),
            historical_base: None,
            last_error: None,
            last_error_kind: None,
            validation: ValidationOpenStats::default(),
        })
    }

    pub(crate) fn mark_opened(
        &mut self,
        publication: &PublishedIndex,
        generation: u64,
        names: &[String],
        identities: &[(CollectionId, u64)],
        indexes: &[Arc<IndexedCollectionSegment>],
        historical: Arc<RestoredGenerationCatalog>,
        validation: ValidationOpenStats,
    ) -> Result<(), String> {
        if names.len() != identities.len() || names.len() != indexes.len() {
            return Err("opened segment identity table length mismatch".into());
        }
        let mut durable_segments = HashMap::with_capacity(names.len());
        for ((name, identity), index) in names.iter().zip(identities).zip(indexes) {
            if index.collection != identity.0 {
                return Err("opened segment identity mismatch".into());
            }
            let current_refs = if let Some(persisted) = &index.persisted {
                (0..persisted.file_count())
                    .map(|position| {
                        persisted
                            .metadata_ref_at(position)
                            .and_then(|reference| {
                                reference.ok_or("opened segment metadata reference missing".into())
                            })
                            .map(|reference| reference & 0x8000_0000 == 0)
                    })
                    .collect::<Result<Vec<_>, String>>()?
            } else {
                index
                    .files
                    .iter()
                    .map(|file| publication.catalog.entry(file.entry.id) == Some(&file.entry))
                    .collect()
            };
            if durable_segments
                .insert(
                    *identity,
                    DurableSegment {
                        name: name.clone(),
                        index: Arc::clone(index),
                        current_refs,
                        catalog: index.persisted.as_ref().map(|_| Arc::clone(&historical)),
                    },
                )
                .is_some()
            {
                return Err("opened segment identity mismatch".into());
            }
        }
        self.durable_generation = Some(generation);
        self.durable_segments = durable_segments;
        self.historical_base = Some(historical);
        self.validation = validation;
        Ok(())
    }

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
        // Historical refs have stable offsets only while their segment files
        // are reused. Once the table greatly exceeds the metadata still
        // referenced by this publication, rewrite those segments and rebuild
        // the table from their actual file entries. Otherwise repeated
        // replacements retain obsolete metadata after old shards are gone.
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
        let mut inputs = Vec::new();
        let mut next_indexes = Vec::new();
        for (number, group) in publication.segments.iter().enumerate() {
            let collection = CollectionId(number as u32);
            let definition = &publication.repository.collections()[number].definition;
            for segment in group.iter() {
                if segment.index.collection != collection || segment.ordinal == 0 {
                    return Err(IndexError::InvalidPublication(
                        "published segment identity invalid".into(),
                    ));
                }
                let identity = (collection, segment.ordinal);
                let previous = self.durable_segments.get(&identity);
                let mut reusable = !rebase_history
                    && previous.is_some_and(|previous| {
                        Arc::ptr_eq(&previous.index, &segment.index)
                            && previous.current_refs.len() == segment.index.file_count()
                    });
                if reusable {
                    let previous = previous.expect("reusable segment exists");
                    for (position, &current_ref) in previous.current_refs.iter().enumerate() {
                        if !current_ref {
                            continue;
                        }
                        let unchanged = if let (Some(persisted), Some(base)) =
                            (&segment.index.persisted, &previous.catalog)
                        {
                            let id = persisted
                                .file_id_at(position)
                                .map_err(IndexError::InvalidCache)?
                                .ok_or_else(|| {
                                    IndexError::InvalidCache("segment file row missing".into())
                                })?;
                            publication.catalog.unchanged_from_persisted(id, base)
                        } else {
                            let file = segment.index.files.get(position).ok_or_else(|| {
                                IndexError::InvalidPublication("segment file row missing".into())
                            })?;
                            publication.catalog.entry(file.entry.id) == Some(&file.entry)
                        };
                        if !unchanged {
                            reusable = false;
                            break;
                        }
                    }
                }
                if reusable {
                    let previous = previous.expect("reusable segment exists");
                    inputs.push(SegmentInput::Existing {
                        name: &previous.name,
                        active: &segment.active,
                    });
                }
                let index = if !reusable && segment.index.persisted.is_some() {
                    Arc::new(
                        materialize_for_rewrite(
                            &segment.index,
                            segment.ordinal,
                            definition,
                            publication.repository.options(),
                            collection,
                            previous
                                .and_then(|segment| segment.catalog.as_ref())
                                .or_else(|| publication.catalog.persisted_base())
                                .ok_or_else(|| {
                                    IndexError::InvalidPublication(
                                        "persisted segment catalog missing".into(),
                                    )
                                })?,
                        )
                        .map_err(IndexError::InvalidCache)?,
                    )
                } else {
                    Arc::clone(&segment.index)
                };
                if !reusable {
                    let references = index
                        .files
                        .iter()
                        .map(|file| catalog.intern(&file.entry))
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(IndexError::InvalidPublication)?;
                    let write_index = Arc::clone(&index);
                    let ordinal = segment.ordinal;
                    let options = publication.repository.options();
                    inputs.push(SegmentInput::NewStream {
                        origin: SegmentOrigin::Validated,
                        collection,
                        ordinal,
                        active: &segment.active,
                        write: Box::new(move |file| {
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
                                )?;
                            } else {
                                crate::hybrid_segment_codec::write_segment(
                                    &write_index,
                                    ordinal,
                                    definition,
                                    options,
                                    file,
                                    &mut intern,
                                )?;
                            }
                            if position != references.len() {
                                return Err("segment metadata reference count changed".into());
                            }
                            Ok(())
                        }),
                    });
                }
                let current_refs = if reusable {
                    previous
                        .expect("reusable segment exists")
                        .current_refs
                        .clone()
                } else {
                    index
                        .files
                        .iter()
                        .map(|file| publication.catalog.entry(file.entry.id) == Some(&file.entry))
                        .collect()
                };
                let segment_catalog = if segment.index.persisted.is_some() {
                    previous
                        .and_then(|segment| segment.catalog.clone())
                        .or_else(|| publication.catalog.persisted_base().cloned())
                } else {
                    None
                };
                next_indexes.push((
                    identity,
                    Arc::clone(&segment.index),
                    current_refs,
                    segment_catalog,
                ));
            }
        }
        let catalog_bytes: Arc<[u8]> =
            Arc::from(catalog.encode().map_err(IndexError::InvalidPublication)?);
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
        if committed.segment_names.len() != next_indexes.len() {
            return Err(IndexError::InvalidPublication(
                "committed segment name count mismatch".into(),
            ));
        }
        self.durable_segments = committed
            .segment_names
            .into_iter()
            .zip(next_indexes)
            .map(|(name, (identity, index, current_refs, catalog))| {
                (
                    identity,
                    DurableSegment {
                        name,
                        index,
                        current_refs,
                        catalog,
                    },
                )
            })
            .collect();
        self.durable_generation = Some(generation);
        self.historical_base = Some(Arc::new(
            RestoredGenerationCatalog::decode_verified_arc(
                catalog_bytes,
                publication.configuration_identity,
                publication.repository.collections().len(),
            )
            .map_err(IndexError::InvalidPublication)?,
        ));
        self.last_error = None;
        self.last_error_kind = None;
        Ok(publication.revision)
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
    synchronizing: bool,
) -> IndexStorageStatus {
    let source_reconciliation_complete = matches!(
        status.reconciliation,
        ReconciliationState::NotStarted | ReconciliationState::Complete
    );
    let Some(managed) = managed else {
        return IndexStorageStatus {
            readiness: if status.initial_construction_complete {
                IndexReadiness::FullyResidentReady
            } else {
                IndexReadiness::Building
            },
            source_reconciliation_complete,
            current_generation: status.revision.0,
            durable_generation: None,
            synchronization: IndexSynchronization::MemoryOnly,
            last_error: None,
            last_error_kind: None,
            managed_directory: None,
            segments_full_validated: 0,
            segments_stamp_trusted: 0,
            validation_stamps_written: 0,
            validation_stamp_error: None,
            validation_stamp_error_kind: None,
        };
    };
    IndexStorageStatus {
        readiness: if !status.initial_construction_complete {
            IndexReadiness::Building
        } else if managed.residency == IndexResidency::FullyResident {
            IndexReadiness::FullyResidentReady
        } else {
            IndexReadiness::Searchable
        },
        source_reconciliation_complete,
        current_generation: status.revision.0,
        durable_generation: managed.durable_generation,
        synchronization: if synchronizing {
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
        segments_stamp_trusted: managed.validation.trusted,
        validation_stamps_written: managed.validation.written,
        validation_stamp_error: managed.validation.error.clone(),
        validation_stamp_error_kind: managed.validation.error_kind,
    }
}

impl Index {
    /// Start a managed initial build with catalogue and batch publications.
    /// A complete stored generation is returned as an already-ready task;
    /// an absent generation is flushed only after construction completes.
    pub fn start_incremental_build_with_storage(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        storage: IndexStoragePolicy,
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
        if let Some(opened) = store.read_generation().map_err(IndexError::from)? {
            let index = restore_generation(
                &prepared.resolved,
                prepared.definitions,
                options,
                sources,
                opened,
                started,
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
    pub fn open_with_storage(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        storage: IndexStoragePolicy,
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
        if let Some(opened) = store.read_generation().map_err(IndexError::from)? {
            let index = restore_generation(
                &prepared.resolved,
                prepared.definitions,
                options,
                sources,
                opened,
                started,
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
) -> Result<Index, IndexError> {
    let bytes = opened
        .catalog
        .as_ref()
        .ok_or_else(|| IndexError::InvalidCache("generation catalog missing".into()))?;
    let restored = Arc::new(
        RestoredGenerationCatalog::decode_verified_arc(
            Arc::clone(bytes),
            resolved.configuration_identity,
            definitions.len(),
        )
        .map_err(IndexError::InvalidCache)?,
    );
    if restored.revision != opened.generation {
        return Err(IndexError::InvalidCache(
            "catalog and manifest revisions differ".into(),
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
    let metadata_ms = started.elapsed().as_millis() as u64;
    let count = owner.collections().len();
    if opened.segments.len() != opened.identities.len()
        || opened.segments.len() != opened.active.len()
        || opened.segments.len() != opened.segment_names.len()
        || opened.segments.len() != opened.segment_lengths.len()
        || opened.segments.len() != opened.validation.len()
    {
        return Err(IndexError::InvalidCache(
            "manifest segment tables differ in length".into(),
        ));
    }
    let mut groups = vec![Vec::new(); count];
    let mut indexes = Vec::with_capacity(opened.segments.len());
    let mut ordinals = HashSet::with_capacity(opened.segments.len());
    for (((reader, &(collection, ordinal)), active), &trust) in opened
        .segments
        .iter()
        .zip(&opened.identities)
        .zip(&opened.active)
        .zip(&opened.validation)
    {
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
        let segment = crate::hybrid_segment_codec::decode_segment_sealed_persisted_with_trust(
            Arc::clone(reader),
            ordinal,
            &configured.definition,
            options,
            collection,
            (configured.source.id(), configured.source.version()),
            active.slot_count(),
            metadata,
            trust,
        )
        .map_err(IndexError::InvalidCache)?;
        if segment.record_count() != active.slot_count() as usize {
            return Err(IndexError::InvalidCache(
                "manifest active mask slot count differs from records".into(),
            ));
        }
        let index = Arc::new(segment);
        groups[number].push(Arc::new(PublishedSegment {
            ordinal,
            index: Arc::clone(&index),
            active: active.clone(),
        }));
        indexes.push(index);
    }
    let catalog = FileCatalog::restored_persisted(Arc::clone(&owner), Arc::clone(&restored))
        .map_err(IndexError::InvalidCache)?;
    let file_versions = FileVersions::from_persisted(Arc::clone(&restored));
    for group in &groups {
        validate_live_records(group, &restored)?;
    }
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
    index.mark_restored_from_cache();
    index.attach_storage(resolved.clone())?;
    let mut validation = ValidationOpenStats {
        full: opened
            .validation
            .iter()
            .filter(|&&trust| trust == SegmentTrust::Untrusted)
            .count(),
        trusted: opened
            .validation
            .iter()
            .filter(|&&trust| trust == SegmentTrust::Stamped)
            .count(),
        ..Default::default()
    };
    if validation.full > 0 {
        let stamps = opened
            .segment_names
            .iter()
            .zip(&opened.segment_lengths)
            .zip(&opened.validation)
            .filter_map(|((name, &length), &trust)| {
                (trust == SegmentTrust::Untrusted).then_some((
                    name.clone(),
                    length,
                    VALIDATOR_VERSION,
                ))
            })
            .collect::<Vec<_>>();
        let stamp_result = match ManagedStore::open(resolved) {
            Ok(store) => store
                .record_validation_stamps(opened.generation, &stamps)
                .map_err(|error| (IndexStorageFailureKind::from(&error), error.to_string())),
            Err(error) => Err((IndexStorageFailureKind::Access, error)),
        };
        match stamp_result {
            Ok(true) => validation.written = stamps.len(),
            Ok(false) => {}
            Err((kind, error)) => {
                validation.error = Some(error);
                validation.error_kind = Some(kind);
            }
        }
    }
    index.mark_storage_opened(
        opened.generation,
        &opened.segment_names,
        &opened.identities,
        &indexes,
        restored,
        validation,
    )?;
    let available_ms = started.elapsed().as_millis() as u64;
    index.update_timings_ms(|timings| {
        timings.metadata = Some(metadata_ms);
        timings.search_available = Some(available_ms);
        timings.full_available = Some(available_ms);
    })?;
    index.start_reconcile()?;
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
