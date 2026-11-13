//! Managed immutable generations for the lifecycle index.
//!
//! The index publisher owns current query state. This module pins one complete
//! publication for a save, reuses unchanged segment files, and atomically
//! binds an independently encoded file catalog to the new manifest.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use crate::build::IndexedCollectionSegment;
use crate::generation_catalog::{GenerationCatalogBuilder, RestoredGenerationCatalog};
use crate::index_storage::{
    IndexReadiness, IndexResidency, IndexStoragePolicy, IndexStorageStatus, IndexSynchronization,
    ResolvedIndexStorage,
};
use crate::inventory::{FileEntry, FileVersions};
use crate::lifecycle::{
    Index, IndexError, IndexRevision, IndexStatus, InitialBuildTask, PublishedIndex,
    PublishedSegment, ReconciliationState,
};
use crate::lifecycle_catalog::FileCatalog;
use crate::managed_store::{ManagedStore, OpenedGeneration, SegmentInput};
use crate::model::{CollectionId, RecordId};
use crate::repository::{CollectionDefinition, Repository, RepositoryOptions};
use crate::source::SourceRegistry;

struct DurableSegment {
    name: String,
    index: Arc<IndexedCollectionSegment>,
    /// Whether each encoded file ref points at the mutable current-catalog
    /// slot. Historical-tagged refs remain stable when the catalog changes.
    current_refs: Vec<bool>,
}

pub(crate) struct ManagedPersistence {
    pub(crate) store: ManagedStore,
    residency: IndexResidency,
    durable_generation: Option<u64>,
    durable_segments: HashMap<(CollectionId, u64), DurableSegment>,
    historical: Vec<FileEntry>,
    pub(crate) last_error: Option<String>,
}

#[derive(Clone)]
pub(crate) struct StorageStatusSnapshot {
    residency: IndexResidency,
    durable_generation: Option<u64>,
    last_error: Option<String>,
    managed_directory: PathBuf,
}

impl ManagedPersistence {
    pub(crate) fn status_snapshot(&self) -> StorageStatusSnapshot {
        StorageStatusSnapshot {
            residency: self.residency,
            durable_generation: self.durable_generation,
            last_error: self.last_error.clone(),
            managed_directory: self.store.path().to_path_buf(),
        }
    }

    pub(crate) fn open(resolved: &ResolvedIndexStorage) -> Result<Self, String> {
        Ok(Self {
            store: ManagedStore::open(resolved)?,
            residency: resolved.residency,
            durable_generation: None,
            durable_segments: HashMap::new(),
            historical: Vec::new(),
            last_error: None,
        })
    }

    pub(crate) fn mark_opened(
        &mut self,
        publication: &PublishedIndex,
        generation: u64,
        names: &[String],
        identities: &[(CollectionId, u64)],
        indexes: &[Arc<IndexedCollectionSegment>],
        historical: &[FileEntry],
    ) -> Result<(), String> {
        if names.len() != identities.len() || names.len() != indexes.len() {
            return Err("opened segment identity table length mismatch".into());
        }
        let mut durable_segments = HashMap::with_capacity(names.len());
        for ((name, identity), index) in names.iter().zip(identities).zip(indexes) {
            if index.collection != identity.0
                || durable_segments
                    .insert(
                        *identity,
                        DurableSegment {
                            name: name.clone(),
                            index: Arc::clone(index),
                            current_refs: index
                                .files
                                .iter()
                                .map(|file| {
                                    publication.catalog.entry(file.entry.id) == Some(&file.entry)
                                })
                                .collect(),
                        },
                    )
                    .is_some()
            {
                return Err("opened segment identity mismatch".into());
            }
        }
        self.durable_generation = Some(generation);
        self.durable_segments = durable_segments;
        self.historical = historical.to_vec();
        Ok(())
    }

    pub(crate) fn flush(&mut self, publication: &PublishedIndex) -> Result<IndexRevision, String> {
        if !publication.catalog.construction_complete() {
            return Err("cannot persist incomplete initial construction".into());
        }
        let generation = publication.revision.0;
        if self.durable_generation == Some(generation) {
            self.last_error = None;
            return Ok(publication.revision);
        }
        if self
            .durable_generation
            .is_some_and(|previous| generation <= previous)
        {
            return Err("published revision did not advance beyond durable generation".into());
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
            .map(|segment| segment.index.files.len())
            .sum::<usize>();
        let rebase_history = self.historical.len() > referenced_files.saturating_mul(2).max(64);
        let history = if rebase_history {
            &[][..]
        } else {
            &self.historical
        };
        let mut catalog =
            GenerationCatalogBuilder::from_publication_with_history(publication, history)?;
        let mut inputs = Vec::new();
        let mut next_indexes = Vec::new();
        for (number, group) in publication.segments.iter().enumerate() {
            let collection = CollectionId(number as u32);
            let definition = &publication.repository.collections()[number].definition;
            for segment in group.iter() {
                if segment.index.collection != collection || segment.ordinal == 0 {
                    return Err("published segment identity invalid".into());
                }
                let identity = (collection, segment.ordinal);
                let previous = self.durable_segments.get(&identity);
                let reusable = !rebase_history && previous.is_some_and(|previous| {
                    Arc::ptr_eq(&previous.index, &segment.index)
                        && previous.current_refs.len() == segment.index.files.len()
                        && previous.current_refs.iter().zip(&segment.index.files).all(
                            |(current_ref, file)| {
                                !current_ref
                                    || publication.catalog.entry(file.entry.id) == Some(&file.entry)
                            },
                        )
                });
                if reusable {
                    let previous = previous.expect("reusable segment exists");
                    inputs.push(SegmentInput::Existing {
                        name: &previous.name,
                        active: &segment.active,
                    });
                } else {
                    let references = segment
                        .index
                        .files
                        .iter()
                        .map(|file| catalog.intern(&file.entry))
                        .collect::<Result<Vec<_>, _>>()?;
                    let index = Arc::clone(&segment.index);
                    let ordinal = segment.ordinal;
                    let options = publication.repository.options();
                    inputs.push(SegmentInput::NewStream {
                        collection,
                        ordinal,
                        active: &segment.active,
                        write: Box::new(move |file| {
                            let mut position = 0usize;
                            let mut intern = |entry: &crate::inventory::FileEntry| {
                                    let expected = index
                                        .files
                                        .get(position)
                                        .ok_or("segment metadata reference overflow")?;
                                    if entry != &expected.entry {
                                        return Err(
                                            "segment metadata reference order changed".into()
                                        );
                                    }
                                    let reference = *references
                                        .get(position)
                                        .ok_or("segment metadata reference missing")?;
                                    position += 1;
                                    Ok(reference)
                            };
                            if index.raw.is_sealed() {
                                crate::hybrid_segment_codec::write_sealed_segment(
                                    &index, ordinal, options, file, &mut intern,
                                )?;
                            } else {
                                crate::hybrid_segment_codec::write_segment(
                                    &index, ordinal, definition, options, file, &mut intern,
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
                    segment
                        .index
                        .files
                        .iter()
                        .map(|file| publication.catalog.entry(file.entry.id) == Some(&file.entry))
                        .collect()
                };
                next_indexes.push((identity, Arc::clone(&segment.index), current_refs));
            }
        }
        let next_historical = catalog.historical_entries().to_vec();
        let catalog_bytes: Arc<[u8]> = Arc::from(catalog.encode()?);
        let committed = self.store.commit_generation_with_catalog(
            self.durable_generation,
            generation,
            &inputs,
            catalog_bytes,
        )?;
        drop(inputs);
        if committed.segment_names.len() != next_indexes.len() {
            return Err("committed segment name count mismatch".into());
        }
        self.durable_segments = committed
            .segment_names
            .into_iter()
            .zip(next_indexes)
            .map(|(name, (identity, index, current_refs))| {
                (
                    identity,
                    DurableSegment {
                        name,
                        index,
                        current_refs,
                    },
                )
            })
            .collect();
        self.durable_generation = Some(generation);
        self.historical = next_historical;
        self.last_error = None;
        Ok(publication.revision)
    }
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
            managed_directory: None,
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
        managed_directory: Some(managed.managed_directory.clone()),
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
        .as_deref()
        .ok_or_else(|| IndexError::InvalidCache("generation catalog missing".into()))?;
    let restored = RestoredGenerationCatalog::decode(
        bytes,
        resolved.configuration_identity,
        definitions.len(),
    )
    .map_err(IndexError::InvalidCache)?;
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
    {
        return Err(IndexError::InvalidCache(
            "manifest segment tables differ in length".into(),
        ));
    }
    let mut groups = vec![Vec::new(); count];
    let mut indexes = Vec::with_capacity(opened.segments.len());
    let mut ordinals = HashSet::with_capacity(opened.segments.len());
    for ((reader, &(collection, ordinal)), active) in opened
        .segments
        .iter()
        .zip(&opened.identities)
        .zip(&opened.active)
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
        let segment = crate::hybrid_segment_codec::decode_segment_sealed(
            Arc::clone(reader),
            ordinal,
            &configured.definition,
            options,
            collection,
            (configured.source.id(), configured.source.version()),
            active.slot_count(),
            &|reference| restored.resolve_metadata(reference),
        )
        .map_err(IndexError::InvalidCache)?;
        if segment.records.len() != active.slot_count() as usize {
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
    let historical = restored.historical_entries().to_vec();
    let catalog = FileCatalog::restored(Arc::clone(&owner), restored.slots)
        .map_err(IndexError::InvalidCache)?;
    let file_versions =
        FileVersions::from_versions(restored.file_versions).map_err(IndexError::InvalidCache)?;
    for group in &groups {
        let mut active_ids = HashSet::<RecordId>::new();
        for segment in group {
            for record in &segment.index.records {
                if !segment.active.contains(record.slot) {
                    continue;
                }
                let current = catalog
                    .entry(record.file)
                    .ok_or_else(|| IndexError::InvalidCache("active record file retired".into()))?;
                if current.revision != record.revision
                    || !current.collections.contains(&segment.index.collection)
                    || !active_ids.insert(record.id.clone())
                {
                    return Err(IndexError::InvalidCache(
                        "active record source or ID mismatch".into(),
                    ));
                }
            }
        }
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
    index.mark_storage_opened(
        opened.generation,
        &opened.segment_names,
        &opened.identities,
        &indexes,
        &historical,
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

#[cfg(test)]
#[path = "lifecycle_storage.test.rs"]
mod tests;
