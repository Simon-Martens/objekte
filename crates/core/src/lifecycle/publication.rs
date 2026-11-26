use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use roaring::RoaringBitmap;

use crate::index::segment::IndexedCollectionSegment;
use crate::index::segment::{PublishedSegment, SegmentId};
use crate::index::storage::file_catalog::{
    FileCatalogBase, FileCatalogDelta, FileCatalogHandle, FileCatalogReader, FileCatalogSource,
    edit_file_catalog,
};
use crate::index::storage::liveness::InitialLiveness;
use crate::index::storage::segment::{SegmentReader, visit_record_headers, visit_segment_files};
use crate::index::storage::segment_catalog::{
    SegmentCatalogHandle, SegmentDirectoryDelta, SegmentRouteSource, build_catalog, edited_catalog,
    install_segment,
};
use crate::inventory::{ActiveRecords, FileEntry, FileStatus};
use crate::model::{CollectionId, RecordId};
use crate::repository::Repository;

use super::{
    FileVersion, Index, IndexError, IndexRevision, IndexStatus, IndexView,
    InitialPublicationPhases, MAX_SEGMENTS_PER_COLLECTION, MAX_SPARSE_SEGMENT_BYTES,
    MIN_RETIREMENT_SLOTS, ReconciliationState, SIZE_MAINTENANCE_REVISIONS,
};

/// One immutable root: shared base, bounded indexed segments, liveness, file
/// catalog and configuration all belong to the same publication revision.
#[derive(Clone)]
pub(crate) struct PublishedIndex {
    pub(crate) instance_id: u64,
    pub(crate) revision: IndexRevision,
    pub(crate) repository: Arc<Repository>,
    pub(crate) configuration_identity: [u8; 32],
    pub(crate) active: Vec<ActiveRecords>,
    pub(crate) segments: Vec<Arc<Vec<Arc<PublishedSegment>>>>,
    pub(crate) record_routes: Vec<SegmentCatalogHandle>,
    pub(crate) next_segment_ordinal: u64,
    /// Hysteresis for byte-triggered coalescing; count and retirement remain
    /// independent triggers and may run sooner.
    pub(crate) last_size_maintenance_revision: u64,
    pub(crate) catalog: FileCatalogHandle,
    pub(crate) reconciliation: ReconciliationState,
    pub(crate) processed_files: usize,
    /// Only populated during an initial build. One bitmap per collection
    /// prevents a shared physical file from being counted too early.
    pub(crate) processed_memberships: Vec<Arc<RoaringBitmap>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RecordMutationKind {
    Insert,
    Replace,
    Delete,
}

pub(crate) struct RecordMutation {
    pub(crate) id: RecordId,
    pub(crate) kind: RecordMutationKind,
}

fn cheapest_consolidation_pair(
    group: &[Arc<PublishedSegment>],
    target_records: usize,
) -> Option<(usize, usize)> {
    let mut sized = group
        .iter()
        .enumerate()
        .map(|(index, segment)| (segment.index.record_count(), index))
        .collect::<Vec<_>>();
    sized.sort_unstable();
    for (position, &(left_size, left)) in sized.iter().enumerate() {
        for &(right_size, right) in &sized[position + 1..] {
            if left_size + right_size > target_records {
                break;
            }
            let left_index = &group[left].index;
            let right_index = &group[right].index;
            if left_index.source_id != right_index.source_id
                || left_index.source_version != right_index.source_version
                || !left_index.backing.compatible_with(&right_index.backing)
            {
                continue;
            }
            let mut overlaps = false;
            for file_number in 0..left_index.file_count() {
                let file_id = left_index.files.get(file_number)?.entry.id;
                if right_index.file_index.get(&file_id).copied().is_some() {
                    overlaps = true;
                    break;
                }
            }
            if !overlaps {
                for &slot in &left_index.identity_order {
                    let id = left_index.record(slot)?.id.0.as_ref();
                    if right_index.id_lookup.get(id).copied().is_some() {
                        overlaps = true;
                        break;
                    }
                }
            }
            if overlaps {
                continue;
            }
            return Some((left.min(right), left.max(right)));
        }
    }
    None
}
impl PublishedIndex {
    fn live_route(
        &self,
        number: usize,
        id: &str,
    ) -> Result<Option<(usize, crate::model::RecordSlot)>, IndexError> {
        let Some(found) = self.record_routes[number].locate(id)? else {
            return Ok(None);
        };
        let segment = self.segments[number]
            .get(found.position)
            .filter(|segment| segment.id == found.route.segment)
            .ok_or_else(|| {
                IndexError::Storage("record route points to a different segment".into())
            })?;
        Ok(segment
            .active
            .contains(found.route.slot)
            .then_some((found.position, found.route.slot)))
    }

    pub(crate) fn rebuild_record_routes(&mut self, number: usize) -> Result<(), IndexError> {
        self.record_routes[number] = build_catalog(
            self.repository.storage_factory.as_ref(),
            self.segments[number].iter().map(Arc::as_ref),
        )?;
        Ok(())
    }

    pub(crate) fn rebuild_all_record_routes(&mut self) -> Result<(), IndexError> {
        for number in 0..self.segments.len() {
            self.rebuild_record_routes(number)?;
        }
        Ok(())
    }

    pub(crate) fn from_repository_owned(
        instance_id: u64,
        revision: IndexRevision,
        mut repository: Repository,
    ) -> Result<Self, IndexError> {
        for collection in &mut repository.collections {
            collection.repository_instance = instance_id;
        }
        let active = std::mem::take(&mut repository.active);
        let segments = std::mem::take(&mut repository.segments);
        Self::from_repository_parts(
            instance_id,
            revision,
            Arc::new(repository),
            active,
            segments,
        )
    }

    pub(crate) fn from_repository(
        instance_id: u64,
        revision: IndexRevision,
        repository: Arc<Repository>,
    ) -> Result<Self, IndexError> {
        Self::from_repository_parts(
            instance_id,
            revision,
            Arc::clone(&repository),
            repository.active.clone(),
            repository.segments.clone(),
        )
    }

    fn from_repository_parts(
        instance_id: u64,
        revision: IndexRevision,
        repository: Arc<Repository>,
        active: Vec<ActiveRecords>,
        segments: Vec<Arc<Vec<Arc<PublishedSegment>>>>,
    ) -> Result<Self, IndexError> {
        let configuration_identity = repository.configuration_identity;
        let catalog = if revision == repository.revision {
            Arc::clone(&repository.catalog)
        } else {
            edit_file_catalog(
                repository.catalog.as_ref(),
                [FileCatalogDelta::ResetVersions(FileVersion(revision.0))],
            )?
        };
        let processed_files = catalog.live_count() as usize;
        let next_segment_ordinal = segments
            .iter()
            .flat_map(|group| group.iter())
            .map(|segment| segment.id.get())
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| IndexError::ResourceLimit("segment ordinal overflow".into()))?;
        let record_routes = segments
            .iter()
            .map(|group| {
                build_catalog(
                    repository.storage_factory.as_ref(),
                    group.iter().map(Arc::as_ref),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            instance_id,
            revision,
            repository,
            configuration_identity,
            active,
            segments,
            record_routes,
            next_segment_ordinal,
            last_size_maintenance_revision: 0,
            catalog,
            reconciliation: ReconciliationState::NotStarted,
            processed_files,
            processed_memberships: Vec::new(),
        })
    }

    pub(crate) fn begin_initial_construction(&mut self) -> Result<(), IndexError> {
        self.catalog = edit_file_catalog(
            self.catalog.as_ref(),
            [FileCatalogDelta::ConstructionComplete(false)],
        )?;
        self.processed_files = 0;
        self.processed_memberships = (0..self.repository.collections().len())
            .map(|_| Arc::new(RoaringBitmap::new()))
            .collect();
        Ok(())
    }

    pub(super) fn status(&self) -> IndexStatus {
        IndexStatus {
            instance_id: self.instance_id,
            revision: self.revision,
            reconciliation: self.reconciliation,
            indexed_files: self.catalog.live_count() as usize,
            processed_files: self.processed_files,
            initial_construction_complete: self.catalog.construction_complete(),
            build_parses: self.repository.build_parses(),
            build_source_reads: self.repository.startup().build_source_reads,
            build_source_bytes_read: self.repository.startup().build_source_bytes_read,
            fallback_records: self
                .repository
                .collections()
                .iter()
                .enumerate()
                .map(|(number, collection)| {
                    let base = collection
                        .generation
                        .records
                        .iter()
                        .filter(|record| {
                            record.class == crate::inventory::RecordClass::RawFallback
                                && self.active[number].contains(record.slot)
                        })
                        .count() as u64;
                    let segments = self.segments[number]
                        .iter()
                        .map(|segment| {
                            (&segment.active.bitmap() - &segment.index.logical_records).len()
                        })
                        .sum::<u64>();
                    base + segments
                })
                .sum(),
            live_records: self.active.iter().map(ActiveRecords::len).sum::<u64>()
                + self
                    .segments
                    .iter()
                    .flat_map(|segments| segments.iter())
                    .map(|segment| segment.active.len())
                    .sum::<u64>(),
        }
    }

    /// Stage one fully indexed local segment in a private next root. The
    /// caller may stage several affected collections before one publication.
    pub(crate) fn stage_owned_segment(
        &mut self,
        index: IndexedCollectionSegment,
    ) -> Result<(), IndexError> {
        self.stage_segment(&mut Arc::new(index), None)
    }

    fn stage_segment(
        &mut self,
        payload: &mut Arc<IndexedCollectionSegment>,
        observation: Option<crate::index::storage::inference::InferenceObservationHandle>,
    ) -> Result<(), IndexError> {
        self.stage_segment_payload(payload, observation, false)
    }

    pub(crate) fn stage_reusable_file_segment(
        &mut self,
        payload: &Arc<IndexedCollectionSegment>,
    ) -> Result<(), IndexError> {
        self.stage_segment_payload(&mut Arc::clone(payload), None, true)
    }

    fn stage_segment_payload(
        &mut self,
        payload: &mut Arc<IndexedCollectionSegment>,
        observation: Option<crate::index::storage::inference::InferenceObservationHandle>,
        allocate_id: bool,
    ) -> Result<(), IndexError> {
        let index = &**payload;
        let descriptor = index.descriptor();
        let number = descriptor.collection.0 as usize;
        if number >= self.segments.len() {
            return Err(IndexError::InvalidPublication(
                "segment collection missing".into(),
            ));
        }
        self.make_room_for_segment(number)?;
        let collection =
            self.repository.collections().get(number).ok_or_else(|| {
                IndexError::InvalidPublication("segment collection missing".into())
            })?;
        let binding = index.source();
        let source_matches = match binding.registration {
            None => collection.source.as_ref().is_some_and(|source| {
                binding.adapter == source.id() && binding.version == source.version()
            }),
            Some(registration) => self
                .repository
                .record_registrations
                .get(registration as usize)
                .is_some_and(|source| {
                    source.handle.collection == descriptor.collection
                        && binding.adapter == source.adapter_id
                        && binding.version == source.version
                }),
        };
        if !source_matches {
            return Err(IndexError::IncompatibleConfiguration(
                "segment source registration changed".into(),
            ));
        }
        let constructing = !self.catalog.construction_complete();
        visit_segment_files(index, |file| {
            let current = self.catalog.file(file.id)?.ok_or_else(|| {
                IndexError::InvalidPublication(
                    "segment file is not active in its collection".into(),
                )
            })?;
            if !current.collections.contains(&descriptor.collection)
                || current.relative_path != file.relative_path
                || current.revision != file.revision
                || current.created_ns != file.created_ns
            {
                return Err(IndexError::InvalidPublication(
                    "segment file is not active in its collection".into(),
                ));
            }
            if constructing {
                let members = self.processed_memberships.get_mut(number).ok_or_else(|| {
                    IndexError::InvalidPublication("processed membership missing".into())
                })?;
                if Arc::make_mut(members).insert(file.id.0)
                    && current.collections.iter().all(|collection| {
                        self.processed_memberships
                            .get(collection.0 as usize)
                            .is_some_and(|covered| covered.contains(file.id.0))
                    })
                {
                    self.processed_files += 1;
                }
            }
            Ok(())
        })?;
        visit_record_headers(index, |headers| {
            for header in headers {
                let id = &header.id;
                let existing_duplicate = self.live_route(number, id.0.as_ref())?.is_some();
                if collection
                    .id_lookup
                    .get(id)
                    .is_some_and(|slot| self.active[number].contains(*slot))
                    || existing_duplicate
                {
                    return Err(IndexError::DuplicateRecordId {
                        collection: collection.definition.name.clone(),
                        record_id: id.0.to_string(),
                    });
                }
            }
            Ok(())
        })?;
        let active = self
            .repository
            .storage_factory
            .live_records(descriptor.slots, InitialLiveness::All)?;
        let mut segment_id = descriptor.id;
        if allocate_id
            || segment_id.get() == 1
            || self.segments[number]
                .iter()
                .any(|segment| segment.id == segment_id)
        {
            segment_id = SegmentId::new(self.next_segment_ordinal);
        }
        self.next_segment_ordinal = self.next_segment_ordinal.max(
            segment_id
                .get()
                .checked_add(1)
                .ok_or_else(|| IndexError::ResourceLimit("segment identity overflow".into()))?,
        );
        if !allocate_id && descriptor.id != segment_id {
            Arc::get_mut(payload)
                .expect("unpublished payload is uniquely owned before placement")
                .id = segment_id;
        }
        Arc::make_mut(&mut self.segments[number]).push(Arc::new(PublishedSegment {
            id: segment_id,
            index: Arc::clone(payload),
            active,
            observation,
        }));
        let group = &self.segments[number];
        install_segment(
            &mut self.record_routes[number],
            group.last().expect("new segment").as_ref(),
            group.len() - 1,
        )?;
        if self.next_segment_ordinal == 0 {
            self.next_segment_ordinal = 1;
        }
        Ok(())
    }

    /// Coalesce small delta shards without enlarging a partition beyond the
    /// local verified-maintenance budget. Full base partitions do not consume
    /// the sparse-delta allowance, even when a collection has more than 64.
    pub(crate) fn make_room_for_segment(&mut self, number: usize) -> Result<(), IndexError> {
        self.make_room_for_segment_with_limits(
            number,
            MAX_SPARSE_SEGMENT_BYTES,
            SIZE_MAINTENANCE_REVISIONS,
        )
    }

    pub(super) fn make_room_for_segment_with_limits(
        &mut self,
        number: usize,
        sparse_byte_limit: usize,
        size_cooldown: u64,
    ) -> Result<(), IndexError> {
        let policy = self.repository.options().segment_policy;
        let target_records = policy.target_records.get();
        let full_partitions = self.segments[number]
            .iter()
            .filter(|segment| segment.index.record_count() >= target_records)
            .count();
        let admission_limit = MAX_SEGMENTS_PER_COLLECTION.saturating_add(full_partitions);
        let count_triggered = self.segments[number].len() >= admission_limit;
        let group = Arc::make_mut(&mut self.segments[number]);
        if count_triggered {
            // A current zero-record file still owns its catalogue contribution.
            let old_count = group.len();
            let mut kept = Vec::with_capacity(group.len());
            for segment in group.drain(..) {
                let mut current_file = false;
                for file_number in 0..segment.index.file_count() {
                    let file = segment
                        .index
                        .files
                        .get(file_number)
                        .map(|file| &file.entry)
                        .ok_or_else(|| {
                            IndexError::InvalidPublication("segment file row missing".into())
                        })?;
                    if self.catalog.file(file.id)?.is_some_and(|current| {
                        current.revision == file.revision
                            && current.created_ns == file.created_ns
                            && current.collections.contains(&segment.index.collection)
                    }) {
                        current_file = true;
                        break;
                    }
                }
                if segment.active.len() > 0 || current_file {
                    kept.push(segment);
                }
            }
            if kept.len() != old_count {
                *group = kept;
                self.record_routes[number] = build_catalog(
                    self.repository.storage_factory.as_ref(),
                    group.iter().map(Arc::as_ref),
                )?;
            } else {
                *group = kept;
            }
        }
        let count_triggered = group.len() >= admission_limit;
        if !count_triggered {
            let retired = group
                .iter()
                .enumerate()
                .filter_map(|(index, segment)| {
                    let total = segment.index.record_count();
                    let removed = total.saturating_sub(segment.active.len() as usize);
                    (total >= MIN_RETIREMENT_SLOTS
                        && total <= target_records
                        && removed.saturating_mul(4) >= total)
                        .then_some((removed, index))
                })
                .max();
            if let Some((_, index)) = retired {
                let old = Arc::clone(&group[index]);
                let replacements = crate::lifecycle::maintenance::compaction::partition::compact_source_parts_with_policy(
                    &old.index,
                    &old.active,
                    &self.catalog,
                    policy,
                    || false,
                )?;
                group.remove(index);
                for (offset, part) in replacements.into_iter().enumerate() {
                    let ordinal = self.next_segment_ordinal;
                    self.next_segment_ordinal = ordinal.checked_add(1).ok_or_else(|| {
                        IndexError::ResourceLimit("segment ordinal overflow".into())
                    })?;
                    let active = self
                        .repository
                        .storage_factory
                        .live_records(part.records.len(), InitialLiveness::All)?;
                    group.insert(
                        index + offset,
                        Arc::new(PublishedSegment {
                            id: SegmentId::new(ordinal),
                            index: Arc::new(part),
                            active,
                            observation: None,
                        }),
                    );
                }
                self.record_routes[number] = build_catalog(
                    self.repository.storage_factory.as_ref(),
                    group.iter().map(Arc::as_ref),
                )?;
                return Ok(());
            }
        }
        // Native retained-memory estimate, not an encoded length or a strict
        // allocator count; it only orders when size-triggered maintenance runs.
        let sparse_bytes = group
            .iter()
            .filter(|segment| segment.index.record_count() < target_records)
            .map(|segment| segment.estimates().retained_bytes_estimated)
            .fold(0usize, usize::saturating_add);
        let size_triggered = sparse_bytes > sparse_byte_limit
            && (self.last_size_maintenance_revision == 0
                || self
                    .revision
                    .0
                    .saturating_sub(self.last_size_maintenance_revision)
                    >= size_cooldown);
        if !count_triggered && !size_triggered {
            return Ok(());
        }
        let Some((left, right)) = cheapest_consolidation_pair(group, target_records) else {
            return Ok(());
        };
        let merged = IndexedCollectionSegment::consolidate(
            &[&group[left].index, &group[right].index],
            target_records,
        )
        .map_err(IndexError::Build)?;
        let active = merged
            .translate_active(
                self.repository.storage_factory.as_ref(),
                &[&group[left].active, &group[right].active],
            )
            .map_err(IndexError::Build)?;
        let ordinal = self.next_segment_ordinal;
        self.next_segment_ordinal = self
            .next_segment_ordinal
            .checked_add(1)
            .ok_or_else(|| IndexError::ResourceLimit("segment ordinal overflow".into()))?;
        let merged_index = merged.index;
        let removed_right = group.remove(right);
        let removed_left = group.remove(left);
        group.push(Arc::new(PublishedSegment {
            id: SegmentId::new(ordinal),
            index: Arc::new(merged_index),
            active,
            observation: None,
        }));
        let added = Arc::clone(group.last().expect("consolidated segment"));
        let positions = group.iter().map(|segment| segment.id).collect::<Vec<_>>();
        let removed: [&dyn SegmentRouteSource; 2] = [removed_left.as_ref(), removed_right.as_ref()];
        self.record_routes[number] = edited_catalog(
            &self.record_routes[number],
            SegmentDirectoryDelta::Replace {
                removed: &removed,
                replacement: added.as_ref(),
                positions: &positions,
            },
        )?;
        if size_triggered {
            self.last_size_maintenance_revision = self.revision.0;
        }
        Ok(())
    }

    /// File import membership comes from file-backed contributions, not from
    /// independently declared navigation links in the same collection.
    pub(crate) fn imported_collections(
        &self,
        file: &FileEntry,
    ) -> Result<BTreeSet<CollectionId>, IndexError> {
        let mut imported = BTreeSet::new();
        if let Some(base) = self.repository.catalog.file(file.id)?
            && base.revision == file.revision
            && base.created_ns == file.created_ns
        {
            for collection in &base.collections {
                if self.repository.collections()[collection.0 as usize]
                    .generation
                    .files
                    .contains(&file.id)
                {
                    imported.insert(*collection);
                }
            }
        }
        for collection in &file.collections {
            let number = collection.0 as usize;
            let routes = &self.record_routes[number];
            for id in routes.segments_for_file(file.id)?.iter() {
                let Some(position) = routes.position(*id)? else {
                    continue;
                };
                let segment = &self.segments[number][position].index;
                if matches!(
                    &segment.backing,
                    crate::index::segment::SegmentBacking::FileRange { .. }
                ) && segment.file(file.id).is_some_and(|owned| {
                    owned.entry.revision == file.revision
                        && owned.entry.created_ns == file.created_ns
                }) {
                    imported.insert(*collection);
                    break;
                }
            }
        }
        Ok(imported)
    }

    /// Retire slots owned by one incarnation without touching the catalog.
    /// The caller either replaces or removes its catalog entry atomically.
    pub(super) fn retire_file_records(&mut self, file: &FileEntry) -> Result<u64, IndexError> {
        let mut retired_records = 0u64;
        for collection_id in &file.collections {
            let collection = self
                .repository
                .collections()
                .get(collection_id.0 as usize)
                .ok_or_else(|| IndexError::InvalidPublication("file collection missing".into()))?;
            let active = self
                .active
                .get_mut(collection_id.0 as usize)
                .ok_or_else(|| IndexError::InvalidPublication("active domain missing".into()))?;
            if active.slot_count() != 0
                && let Some(slots) = collection.generation.file_records.records_for(file.id)
            {
                retired_records +=
                    slots.iter().filter(|slot| active.contains(**slot)).count() as u64;
                *active = active.retired(slots)?;
            }
            let number = collection_id.0 as usize;
            let routes = self
                .record_routes
                .get_mut(number)
                .ok_or_else(|| IndexError::InvalidPublication("record routes missing".into()))?;
            let positions = routes
                .segments_for_file(file.id)?
                .iter()
                .map(|&id| routes.position(id))
                .collect::<Result<Vec<_>, _>>()?;
            if positions.is_empty() {
                continue;
            }
            let segments = self.segments.get_mut(number).ok_or_else(|| {
                IndexError::InvalidPublication("segment collection missing".into())
            })?;
            let group = Arc::make_mut(segments);
            for position in positions.into_iter().flatten() {
                let segment = group.get_mut(position).ok_or_else(|| {
                    IndexError::InvalidPublication("routed segment missing".into())
                })?;
                if !matches!(
                    &segment.index.backing,
                    crate::index::segment::SegmentBacking::FileRange { .. }
                ) {
                    continue;
                }
                let Some(file_index) = segment.index.file_index.get(&file.id).copied() else {
                    continue;
                };
                let source = &segment.index.files[file_index];
                if source.entry.revision != file.revision
                    || source.entry.created_ns != file.created_ns
                {
                    continue;
                }
                let active_slots = source
                    .records
                    .iter()
                    .copied()
                    .filter(|slot| segment.active.contains(*slot))
                    .collect::<Vec<_>>();
                if active_slots.is_empty() {
                    if !source.records.is_empty() {
                        *routes = edited_catalog(
                            routes,
                            SegmentDirectoryDelta::RetireFile {
                                file: file.id,
                                segment: segment.id,
                                records: &[],
                            },
                        )?;
                    }
                    continue;
                }
                let route_keys = active_slots
                    .iter()
                    .map(|slot| {
                        segment
                            .index
                            .record(*slot)
                            .map(|record| (record.id.clone(), *slot))
                            .ok_or_else(|| {
                                IndexError::InvalidPublication("routed record missing".into())
                            })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let segment = Arc::make_mut(segment);
                segment.active = segment.active.retired(&active_slots)?;
                retired_records += active_slots.len() as u64;
                *routes = edited_catalog(
                    routes,
                    SegmentDirectoryDelta::RetireFile {
                        file: file.id,
                        segment: segment.id,
                        records: &route_keys,
                    },
                )?;
            }
        }
        Ok(retired_records)
    }

    /// Reconcile optional provenance without retiring registrar-owned records.
    /// Range links survive only while their observed content revision remains
    /// valid. Path-only links follow a live file across content revisions.
    pub(super) fn refresh_record_links(
        &mut self,
        file: crate::model::FileId,
        replacement: Option<&FileEntry>,
    ) -> Result<Vec<CollectionId>, IndexError> {
        let mut memberships = Vec::new();
        for number in 0..self.segments.len() {
            let routes = &self.record_routes[number];
            let positions = routes
                .segments_for_file(file)?
                .iter()
                .map(|&id| routes.position(id))
                .collect::<Result<Vec<_>, _>>()?;
            if positions.is_empty() {
                continue;
            }
            for position in positions.into_iter().flatten() {
                let published = &self.segments[number][position];
                let Some(file_index) = published.index.file_index.get(&file).copied() else {
                    continue;
                };
                if matches!(
                    published.index.backing,
                    crate::index::segment::SegmentBacking::FileRange { .. }
                ) {
                    continue;
                }
                let old_entry = &published.index.files[file_index].entry;
                let old_slots = &published.index.files[file_index].records;
                let kept = old_slots
                    .iter()
                    .copied()
                    .filter(|slot| {
                        published
                            .index
                            .backing
                            .file_locator(*slot)
                            .is_some_and(|locator| {
                                replacement.is_some_and(|entry| {
                                    locator.range.is_none() || locator.revision == entry.revision
                                })
                            })
                    })
                    .collect::<Vec<_>>();
                if !kept.is_empty() && !memberships.contains(&published.index.collection) {
                    memberships.push(published.index.collection);
                }
                if replacement == Some(old_entry) && kept.len() == old_slots.len() {
                    continue;
                }
                let group = Arc::make_mut(&mut self.segments[number]);
                let published = group.get_mut(position).ok_or_else(|| {
                    IndexError::InvalidPublication("routed segment missing".into())
                })?;
                let segment_id = published.id;
                let segment = Arc::make_mut(published);
                let index = Arc::make_mut(&mut segment.index);
                let links = match &mut index.backing {
                    crate::index::segment::SegmentBacking::Retained { links, .. }
                    | crate::index::segment::SegmentBacking::Provider { links, .. } => links,
                    crate::index::segment::SegmentBacking::FileRange { .. } => unreachable!(),
                };
                let old_slots = std::mem::take(&mut index.files[file_index].records);
                for &slot in &old_slots {
                    let Some(locator) = links[slot.0 as usize].as_mut() else {
                        continue;
                    };
                    let preserve = replacement.is_some_and(|entry| {
                        locator.range.is_none() || locator.revision == entry.revision
                    });
                    if preserve {
                        locator.revision = replacement.expect("live replacement").revision;
                    } else {
                        links[slot.0 as usize] = None;
                    }
                }
                if kept.is_empty() {
                    index.files.remove(file_index);
                    index.file_index.clear();
                    for (position, file) in index.files.iter().enumerate() {
                        index.file_index.insert(file.entry.id, position);
                    }
                    self.record_routes[number] = edited_catalog(
                        &self.record_routes[number],
                        SegmentDirectoryDelta::RetireFile {
                            file,
                            segment: segment_id,
                            records: &[],
                        },
                    )?;
                } else {
                    index.files[file_index].entry =
                        replacement.expect("kept links have replacement").clone();
                    index.files[file_index].records = kept;
                }
                index
                    .refresh_metadata_sort_slots(
                        old_slots,
                        self.repository.repository_root(),
                        &self.repository.collections()[number].definition.name,
                    )
                    .map_err(IndexError::Build)?;
                index.recompute_accounting();
            }
        }
        Ok(memberships)
    }
}

fn prepare_initial_segment(
    next: &mut PublishedIndex,
    segment: (
        IndexedCollectionSegment,
        Option<crate::index::storage::inference::InferenceObservationHandle>,
    ),
) -> Result<(), IndexError> {
    let (segment, observation) = segment;
    visit_segment_files(&segment, |file| {
        if file.status == FileStatus::Valid
            && file.diagnostics.is_empty()
            && file.omitted_diagnostics == 0
        {
            return Ok(());
        }
        let mut merged = next
            .catalog
            .file(file.id)?
            .ok_or(IndexError::UnknownFile)?
            .into_owned();
        merged.status = merged.status.max(file.status);
        for diagnostic in &file.diagnostics {
            merged
                .record_diagnostic(diagnostic.clone())
                .map_err(IndexError::InvalidPublication)?;
        }
        merged.omitted_diagnostics = merged
            .omitted_diagnostics
            .saturating_add(file.omitted_diagnostics);
        next.catalog = next.catalog.replaced(merged, None, None)?;
        Ok::<_, IndexError>(())
    })?;
    next.stage_segment(&mut Arc::new(segment), observation)
}

fn verify_initial_files(
    view: &IndexView,
    segments: &[&IndexedCollectionSegment],
) -> Result<(), IndexError> {
    use crate::construction::file_preparation::created_ns;
    use crate::inventory::{
        SourceRevision,
        discovery::{PreparedScanSelection, SymlinkPolicy, discover},
    };
    use rayon::prelude::*;

    let repository = view.repository();
    let root = repository.required_file_root();
    let mut selections = Vec::new();
    let mut by_collection = std::collections::HashMap::new();
    for segment in segments {
        if by_collection.contains_key(&segment.collection) {
            continue;
        }
        let collection = repository
            .collections()
            .get(segment.collection.0 as usize)
            .ok_or_else(|| IndexError::InvalidPublication("missing file collection".into()))?;
        let config = collection.required_file_source();
        let reader = collection.required_file_reader();
        let position = match selections
            .iter()
            .position(|(source_path, rules, extension, _, _)| {
                *source_path == &config.source_path
                    && *rules == &config.discovery
                    && *extension == reader.extension()
            }) {
            Some(position) => position,
            None => {
                let position = selections.len();
                let chosen_aliases = if config.discovery.symlinks == SymlinkPolicy::FollowWithinRoot
                {
                    Some(
                        discover(root, &config.source_path, &config.discovery, reader)
                            .map_err(IndexError::SourceUnavailable)?,
                    )
                } else {
                    None
                };
                selections.push((
                    &config.source_path,
                    &config.discovery,
                    reader.extension(),
                    PreparedScanSelection::new(
                        root,
                        &config.source_path,
                        &config.discovery,
                        reader,
                    )
                    .map_err(IndexError::SourceUnavailable)?,
                    chosen_aliases,
                ));
                position
            }
        };
        by_collection.insert(segment.collection, position);
    }
    let verify = |entry: &FileEntry, position: usize| {
        let (_, _, _, selection, chosen_aliases) = &selections[position];
        let ineligible = || {
            IndexError::SourceUnavailable(format!(
                "source is ineligible before initial publication: {}",
                root.join(&entry.relative_path).display()
            ))
        };
        if chosen_aliases.as_ref().is_some_and(|paths| {
            paths
                .binary_search(&root.join(&entry.relative_path))
                .is_err()
        }) {
            return Err(ineligible());
        }
        let metadata = selection
            .selected_metadata(root, &entry.relative_path)
            .map_err(IndexError::SourceUnavailable)?
            .ok_or_else(ineligible)?;
        let revision =
            SourceRevision::from_metadata(&metadata).map_err(IndexError::SourceUnavailable)?;
        if revision != entry.revision || created_ns(&metadata) != entry.created_ns {
            return Err(IndexError::SourceUnavailable(format!(
                "source changed before initial publication: {}",
                root.join(&entry.relative_path).display()
            )));
        }
        if revision.object_id.is_none() {
            return Err(IndexError::SourceUnavailable(format!(
                "source identity cannot be verified: {}",
                root.join(&entry.relative_path).display()
            )));
        }
        Ok(())
    };
    let workers = &repository.workers;
    if let [segment] = segments {
        let selection = by_collection[&segment.collection];
        if segment.files.len() >= 256 && workers.current_num_threads() > 1 {
            return workers.install(|| {
                segment
                    .files
                    .par_iter()
                    .try_for_each(|file| verify(&file.entry, selection))
            });
        }
        return segment
            .files
            .iter()
            .try_for_each(|file| verify(&file.entry, selection));
    }
    let mut checked = std::collections::HashSet::new();
    let entries = segments
        .iter()
        .flat_map(|segment| {
            let selection = by_collection[&segment.collection];
            segment
                .files
                .iter()
                .map(move |file| (&file.entry, selection))
        })
        .filter(|(entry, selection)| checked.insert((entry.id, *selection)))
        .collect::<Vec<_>>();
    let verify_entry = |(entry, selection): (&FileEntry, usize)| verify(entry, selection);
    if entries.len() >= 256 && workers.current_num_threads() > 1 {
        workers.install(|| entries.into_par_iter().try_for_each(verify_entry))
    } else {
        entries.into_iter().try_for_each(verify_entry)
    }
}

impl Index {
    /// Apply one accepted registrar batch atomically. Retirement and any new
    /// native segment are staged in a private root before the revision CAS.
    pub(crate) fn publish_record_batch<G>(
        &self,
        prepared_from: &IndexView,
        registration: crate::lifecycle::builder::RecordRegistration,
        changes: &[RecordMutation],
        segments: impl IntoIterator<Item = IndexedCollectionSegment>,
        publication_guard: impl Fn() -> Result<G, IndexError>,
    ) -> Result<IndexRevision, IndexError> {
        let mut segments: Vec<_> = segments.into_iter().map(Arc::new).collect();
        self.publish_prepared_record_batch(
            prepared_from,
            registration,
            changes,
            &mut segments,
            publication_guard,
        )
    }

    pub(crate) fn publish_prepared_record_batch<G>(
        &self,
        prepared_from: &IndexView,
        registration: crate::lifecycle::builder::RecordRegistration,
        changes: &[RecordMutation],
        segments: &mut [Arc<IndexedCollectionSegment>],
        publication_guard: impl Fn() -> Result<G, IndexError>,
    ) -> Result<IndexRevision, IndexError> {
        // File mutation and direct ingestion share placement coordination.
        // Their file reads, decoding and segment construction happen first.
        let _placement = self.0.writer.lock().map_err(|_| IndexError::LockPoisoned)?;
        loop {
            let view = self.view()?;
            match self.stage_record_batch(prepared_from, &view, registration, changes, segments) {
                Ok(next) => {
                    let _guard = publication_guard()?;
                    match self.publish_prepared(view.revision(), next) {
                        Err(IndexError::RevisionConflict { .. }) => continue,
                        result => return result,
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn stage_record_batch(
        &self,
        prepared_from: &IndexView,
        view: &IndexView,
        registration: crate::lifecycle::builder::RecordRegistration,
        changes: &[RecordMutation],
        segments: &mut [Arc<IndexedCollectionSegment>],
    ) -> Result<PublishedIndex, IndexError> {
        let expected = view.revision();
        let collection = registration.collection;
        if view
            .repository()
            .record_registrations
            .get(registration.ordinal as usize)
            .is_none_or(|source| source.handle != registration)
        {
            return Err(IndexError::ForeignInstance);
        }
        if !view.initial_construction_complete() {
            return Err(IndexError::IncompleteBuild);
        }
        let number = collection.0 as usize;
        let collection_state =
            view.0.repository.collections.get(number).ok_or_else(|| {
                IndexError::InvalidPublication("record collection missing".into())
            })?;
        let mut next = (*view.0).clone();
        let mut route_editor = None;
        for change in changes {
            let base = collection_state
                .id_lookup
                .get(&change.id)
                .copied()
                .filter(|slot| next.active[number].contains(*slot));
            let found = next.live_route(number, change.id.0.as_ref())?;
            match change.kind {
                RecordMutationKind::Insert => {
                    if base.is_some() || found.is_some() {
                        return Err(IndexError::DuplicateRecordId {
                            collection: collection_state.definition.name.clone(),
                            record_id: change.id.0.to_string(),
                        });
                    }
                }
                RecordMutationKind::Replace | RecordMutationKind::Delete => {
                    let (index, slot) = found.ok_or_else(|| {
                        IndexError::Build(format!(
                            "record {} is missing or owned by another registrar",
                            change.id.0
                        ))
                    })?;
                    let published = &next.segments[number][index];
                    if published.index.source().registration != Some(registration.ordinal) {
                        return Err(IndexError::Build(format!(
                            "record {} belongs to another registrar",
                            change.id.0
                        )));
                    }
                    if expected != prepared_from.revision() {
                        let current = published.index.write_version(slot)?.ok_or_else(|| {
                            IndexError::Storage("live direct record write version missing".into())
                        })?;
                        let original = prepared_from.0.live_route(number, change.id.0.as_ref())?;
                        let original = match original {
                            Some((position, slot)) => prepared_from.0.segments[number][position]
                                .index
                                .write_version(slot)?,
                            None => None,
                        };
                        if original != Some(current) {
                            return Err(IndexError::Build(format!(
                                "record {} changed during preparation",
                                change.id.0
                            )));
                        }
                    }
                    let group = Arc::make_mut(&mut next.segments[number]);
                    let published = Arc::make_mut(&mut group[index]);
                    published.active = published.active.retired(&[slot])?;
                    if route_editor.is_none() {
                        route_editor = Some(next.record_routes[number].edit()?);
                    }
                    route_editor
                        .as_mut()
                        .expect("initialized route editor")
                        .apply(SegmentDirectoryDelta::RetireRecord(change.id.0.as_ref()))?;
                }
            }
        }
        if let Some(editor) = route_editor {
            next.record_routes[number] = editor.seal()?;
        }
        for segment in segments {
            if segment.collection != collection {
                return Err(IndexError::ForeignInstance);
            }
            next.stage_segment(segment, None)?;
        }
        next.revision = expected.next()?;
        Ok(next)
    }

    /// Publish one complete initial work unit. A concurrent lifecycle
    /// mutation changes the revision and rejects this stale work atomically.
    pub(crate) fn publish_initial_segment(
        &self,
        expected: IndexRevision,
        segment: IndexedCollectionSegment,
        observation: Option<crate::index::storage::inference::InferenceObservationHandle>,
        profile_phases: bool,
    ) -> Result<(IndexRevision, InitialPublicationPhases), IndexError> {
        let prepare_started = crate::measurement::timer::start_if_enabled(profile_phases);
        let view = self.view()?;
        if view.revision() != expected {
            return Err(IndexError::RevisionConflict {
                expected,
                actual: view.revision(),
            });
        }
        if view.initial_construction_complete() {
            return Err(IndexError::InvalidPublication(
                "initial construction already completed".into(),
            ));
        }
        verify_initial_files(&view, &[&segment])?;
        let mut next = (*view.0).clone();
        prepare_initial_segment(&mut next, (segment, observation))?;
        next.revision = expected.next()?;
        let preparation_ns = prepare_started
            .map(|started| started.elapsed().as_nanos() as u64)
            .unwrap_or(0);
        let install_started = crate::measurement::timer::start_if_enabled(profile_phases);
        let revision = self.publish_prepared_inner(expected, next)?;
        let install_ns = install_started
            .map(|started| started.elapsed().as_nanos() as u64)
            .unwrap_or(0);
        Ok((
            revision,
            InitialPublicationPhases {
                preparation_ns,
                install_ns,
            },
        ))
    }

    /// Install a validated physical-file group as one publication revision.
    pub(crate) fn publish_initial_segments(
        &self,
        expected: IndexRevision,
        segments: Vec<(
            IndexedCollectionSegment,
            Option<crate::index::storage::inference::InferenceObservationHandle>,
        )>,
        profile_phases: bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(IndexRevision, InitialPublicationPhases), IndexError> {
        if segments.is_empty() {
            return Err(IndexError::InvalidPublication(
                "initial publication group is empty".into(),
            ));
        }
        let prepare_started = crate::measurement::timer::start_if_enabled(profile_phases);
        let view = self.view()?;
        if view.revision() != expected {
            return Err(IndexError::RevisionConflict {
                expected,
                actual: view.revision(),
            });
        }
        if view.initial_construction_complete() {
            return Err(IndexError::InvalidPublication(
                "initial construction already completed".into(),
            ));
        }
        verify_initial_files(
            &view,
            &segments
                .iter()
                .map(|(segment, _)| segment)
                .collect::<Vec<_>>(),
        )?;
        let mut next = (*view.0).clone();
        for segment in segments {
            prepare_initial_segment(&mut next, segment)?;
        }
        next.revision = expected.next()?;
        let preparation_ns = prepare_started
            .map(|started| started.elapsed().as_nanos() as u64)
            .unwrap_or(0);
        let install_started = crate::measurement::timer::start_if_enabled(profile_phases);
        let revision = self.publish_prepared_inner_if(expected, next, cancelled)?;
        let install_ns = install_started
            .map(|started| started.elapsed().as_nanos() as u64)
            .unwrap_or(0);
        Ok((
            revision,
            InitialPublicationPhases {
                preparation_ns,
                install_ns,
            },
        ))
    }

    /// Finalize metadata without copying the completed segment payloads.
    pub(crate) fn finish_initial_construction(
        &self,
        expected: IndexRevision,
        mut repository: Repository,
        file_base: Box<dyn FileCatalogBase>,
    ) -> Result<IndexRevision, IndexError> {
        let view = self.view()?;
        if view.revision() != expected {
            return Err(IndexError::RevisionConflict {
                expected,
                actual: view.revision(),
            });
        }
        if view.initial_construction_complete() {
            return Err(IndexError::InvalidPublication(
                "initial construction already completed".into(),
            ));
        }
        let mut next = (*view.0).clone();
        if next.processed_files != next.catalog.live_count() as usize {
            return Err(IndexError::InvalidPublication(
                "initial construction has unprocessed files".into(),
            ));
        }
        next.catalog = edit_file_catalog(
            next.catalog.as_ref(),
            [
                FileCatalogDelta::Rebase(file_base),
                FileCatalogDelta::ConstructionComplete(true),
            ],
        )?;
        for group in &mut next.segments {
            for published in Arc::make_mut(group).iter_mut() {
                let mut needs_status_refresh = false;
                visit_segment_files(published.index.as_ref(), |file| {
                    if !needs_status_refresh {
                        needs_status_refresh = next
                            .catalog
                            .file(file.id)?
                            .is_some_and(|current| current.status != file.status);
                    }
                    Ok::<_, IndexError>(())
                })?;
                let decision = repository
                    .collections
                    .get(published.index.collection.0 as usize)
                    .and_then(|collection| collection.global_inference.as_ref());
                let needs_inference_prune = decision.is_some_and(|decision| {
                    published.index.inference.as_ref().is_some_and(|state| {
                        state.indexes.iter().any(|(path, index)| {
                            let field = decision
                                .report
                                .fields
                                .iter()
                                .find(|field| field.path == *path);
                            if field.is_none_or(|field| {
                                field.index_status != crate::inference::InferenceIndexStatus::Built
                            }) {
                                return true;
                            }
                            let diagnostic = decision
                                .diagnostics
                                .iter()
                                .find(|diagnostic| diagnostic.path == *path);
                            index.value_complete
                                && !diagnostic
                                    .is_some_and(|diagnostic| diagnostic.value_dictionary_complete)
                                || index.length_complete
                                    && !diagnostic
                                        .is_some_and(|diagnostic| diagnostic.array_length_complete)
                                || index.empty_string_complete
                                    && !diagnostic
                                        .is_some_and(|diagnostic| diagnostic.empty_string_exact)
                        })
                    })
                });
                if !needs_status_refresh && !needs_inference_prune {
                    if published.observation.is_some() {
                        *published = Arc::new(PublishedSegment {
                            id: published.id,
                            index: Arc::clone(&published.index),
                            active: published.active.clone(),
                            observation: None,
                        });
                    }
                    continue;
                }
                let mut index = (*published.index).clone();
                for file in &mut index.files {
                    if let Some(current) = next.catalog.file(file.entry.id)? {
                        file.entry.status = current.status;
                        file.entry.diagnostics = current.diagnostics.clone();
                        file.entry.omitted_diagnostics = current.omitted_diagnostics;
                    }
                }
                let files = &index.files;
                let positions = &index.file_index;
                index
                    .sorting
                    .refresh_file_status_for_records(&index.records, |slot| {
                        let file = index.backing.file_locator(slot)?.file;
                        let status = positions
                            .get(&file)
                            .and_then(|&position| files.get(position))
                            .map_or(FileStatus::Valid, |entry| entry.entry.status);
                        Some(status.code())
                    });
                if let Some(decision) = decision {
                    if let Some(inference) = index.inference.as_mut() {
                        for field in &decision.report.fields {
                            if matches!(
                                field.index_status,
                                crate::inference::InferenceIndexStatus::Redundant
                                    | crate::inference::InferenceIndexStatus::ExplicitOptOut
                            ) {
                                inference.indexes.remove(&field.path);
                                inference.short_string_coverage.remove(&field.path);
                                continue;
                            }
                            let diagnostics = decision
                                .diagnostics
                                .iter()
                                .find(|diagnostic| diagnostic.path == field.path);
                            let general_values = diagnostics
                                .is_some_and(|diagnostic| diagnostic.value_dictionary_complete);
                            let short_values = diagnostics
                                .is_some_and(|diagnostic| diagnostic.short_string_candidates);
                            let empty_string_exact =
                                diagnostics.is_some_and(|diagnostic| diagnostic.empty_string_exact);
                            if let Some(posting) = inference.indexes.get_mut(&field.path) {
                                let selected_null = posting
                                    .values
                                    .remove(&crate::index::posting::ScalarKey::Null);
                                if !general_values && !short_values {
                                    let empty = empty_string_exact.then(|| {
                                        posting.values.remove(
                                                &crate::index::posting::ScalarKey::String(
                                                    String::new(),
                                                ),
                                            )
                                    });
                                    posting.values.clear();
                                    if let Some(Some(slots)) = empty {
                                        posting.values.insert(
                                            crate::index::posting::ScalarKey::String(String::new()),
                                            slots,
                                        );
                                    }
                                    posting.values_indexed = empty_string_exact;
                                }
                                if let Some(slots) = selected_null {
                                    posting
                                        .values
                                        .insert(crate::index::posting::ScalarKey::Null, slots);
                                }
                                posting.value_complete = general_values;
                                posting.length_complete = diagnostics
                                    .is_some_and(|diagnostic| diagnostic.array_length_complete);
                                posting.empty_string_complete = empty_string_exact;
                                if !posting.length_complete {
                                    posting.lengths.clear();
                                }
                                posting.finish_stats();
                            }
                            if !short_values {
                                inference.short_string_coverage.remove(&field.path);
                            }
                        }
                    }
                }
                *published = Arc::new(PublishedSegment {
                    id: published.id,
                    index: Arc::new(index),
                    active: published.active.clone(),
                    observation: None,
                });
            }
        }
        next.processed_memberships.clear();
        repository.catalog = Arc::clone(&next.catalog);
        next.repository = Arc::new(repository);
        next.revision = expected.next()?;
        self.publish_prepared(expected, next)
    }

    /// Install a fully prepared root only if its captured predecessor is
    /// current. The retired Arc is dropped after releasing the write lock.
    /// No filesystem work or index construction belongs inside this method.
    pub(crate) fn publish_prepared(
        &self,
        expected: IndexRevision,
        next: PublishedIndex,
    ) -> Result<IndexRevision, IndexError> {
        self.publish_prepared_inner(expected, next)
    }

    fn publish_prepared_inner(
        &self,
        expected: IndexRevision,
        next: PublishedIndex,
    ) -> Result<IndexRevision, IndexError> {
        self.publish_prepared_inner_if(expected, next, &|| false)
    }

    fn publish_prepared_inner_if(
        &self,
        expected: IndexRevision,
        next: PublishedIndex,
        cancelled: &impl Fn() -> bool,
    ) -> Result<IndexRevision, IndexError> {
        if next.instance_id != self.0.instance_id {
            return Err(IndexError::ForeignInstance);
        }
        let next_revision = expected.next()?;
        if next.revision != next_revision {
            return Err(IndexError::InvalidPublication(
                "prepared root must carry the next revision".into(),
            ));
        }
        #[cfg(test)]
        let notify_cache = next.catalog.construction_complete();
        let next = Arc::new(next);
        let retired = {
            let mut guard = self
                .0
                .published
                .write()
                .map_err(|_| IndexError::LockPoisoned)?;
            if self.0.quiescing.load(Ordering::Acquire) {
                return Err(IndexError::Cancelled);
            }
            if guard.revision != expected {
                return Err(IndexError::RevisionConflict {
                    expected,
                    actual: guard.revision,
                });
            }
            // This is the cancellation decision point. A cancellation
            // observed here wins; after this decision the held lock orders
            // the completed swap before other publications.
            if cancelled() {
                return Err(IndexError::Cancelled);
            }
            std::mem::replace(&mut *guard, next)
        };
        drop(retired);
        #[cfg(test)]
        let _ = notify_cache;
        Ok(next_revision)
    }
}
