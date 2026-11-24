use std::sync::Arc;
use std::sync::atomic::Ordering;

use roaring::RoaringBitmap;

use crate::index::segment::IndexedCollectionSegment;
use crate::index::segment::{PublishedSegment, SegmentId};
use crate::inventory::{ActiveRecords, FileEntry, FileStatus, FileVersions};
use crate::lifecycle::catalog::FileCatalog;
use crate::model::{CollectionId, RecordId};
use crate::repository::Repository;

use super::{
    FileVersion, Index, IndexError, IndexRevision, IndexStatus, InitialPublicationPhases,
    MAX_SEGMENTS_PER_COLLECTION, MAX_SPARSE_SEGMENT_BYTES, MIN_RETIREMENT_SLOTS,
    ReconciliationState, SIZE_MAINTENANCE_REVISIONS,
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
    pub(crate) next_segment_ordinal: u64,
    /// Hysteresis for byte-triggered coalescing; count and retirement remain
    /// independent triggers and may run sooner.
    pub(crate) last_size_maintenance_revision: u64,
    pub(crate) catalog: FileCatalog,
    pub(crate) file_versions: FileVersions,
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
        let file_versions =
            FileVersions::filled(repository.files().files.len(), FileVersion(revision.0));
        let catalog = FileCatalog::from_parts(
            Arc::clone(&repository.files),
            repository.base_fingerprints.clone(),
            configuration_identity,
        );
        let processed_files = catalog.active_files() as usize;
        let next_segment_ordinal = segments
            .iter()
            .flat_map(|group| group.iter())
            .map(|segment| segment.id.get())
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| IndexError::ResourceLimit("segment ordinal overflow".into()))?;
        Ok(Self {
            instance_id,
            revision,
            repository,
            configuration_identity,
            active,
            segments,
            next_segment_ordinal,
            last_size_maintenance_revision: 0,
            catalog,
            file_versions,
            reconciliation: ReconciliationState::NotStarted,
            processed_files,
            processed_memberships: Vec::new(),
        })
    }

    pub(crate) fn begin_initial_construction(&mut self) {
        self.catalog.set_construction_complete(false);
        self.processed_files = 0;
        self.processed_memberships = (0..self.repository.collections().len())
            .map(|_| Arc::new(RoaringBitmap::new()))
            .collect();
    }

    pub(super) fn status(&self) -> IndexStatus {
        debug_assert_eq!(self.catalog.allocated_files(), self.file_versions.len());
        IndexStatus {
            instance_id: self.instance_id,
            revision: self.revision,
            reconciliation: self.reconciliation,
            indexed_files: self.catalog.active_files() as usize,
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
    pub(crate) fn with_segment(self, index: IndexedCollectionSegment) -> Result<Self, IndexError> {
        self.with_segment_observation(index, None)
    }

    fn with_segment_observation(
        mut self,
        mut index: IndexedCollectionSegment,
        observation: Option<Arc<crate::construction::inference::InferenceObservation>>,
    ) -> Result<Self, IndexError> {
        let number = index.collection.0 as usize;
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
        let source_matches = match &index.backing {
            crate::index::segment::SegmentBacking::FileRange { .. } => {
                collection.source.as_ref().is_some_and(|source| {
                    index.source_id == source.id() && index.source_version == source.version()
                })
            }
            crate::index::segment::SegmentBacking::Retained { registration, .. }
            | crate::index::segment::SegmentBacking::Provider { registration, .. } => self
                .repository
                .record_registrations
                .get(*registration as usize)
                .is_some_and(|source| {
                    source.handle.collection == index.collection
                        && index.source_id == source.adapter_id
                        && index.source_version == source.version
                }),
        };
        if !source_matches {
            return Err(IndexError::IncompatibleConfiguration(
                "segment source registration changed".into(),
            ));
        }
        for file in &index.files {
            if self.catalog.entry(file.entry.id).is_none_or(|current| {
                !current.collections.contains(&index.collection)
                    || current.relative_path != file.entry.relative_path
                    || current.revision != file.entry.revision
                    || current.created_ns != file.entry.created_ns
            }) {
                return Err(IndexError::InvalidPublication(
                    "segment file is not active in its collection".into(),
                ));
            }
        }
        for (id, _) in &index.id_lookup {
            let existing_duplicate =
                self.segments[number]
                    .iter()
                    .try_fold(false, |found, segment| {
                        let slot = segment.index.id_lookup.get(id.0.as_ref()).copied();
                        Ok::<_, IndexError>(
                            found || slot.is_some_and(|slot| segment.active.contains(slot)),
                        )
                    })?;
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
        if !self.catalog.construction_complete() {
            for file in &index.files {
                let members = self.processed_memberships.get_mut(number).ok_or_else(|| {
                    IndexError::InvalidPublication("processed membership missing".into())
                })?;
                if Arc::make_mut(members).insert(file.entry.id.0) {
                    let catalogue_entry = self.catalog.entry(file.entry.id).ok_or_else(|| {
                        IndexError::InvalidPublication("processed file missing".into())
                    })?;
                    if catalogue_entry.collections.iter().all(|collection| {
                        self.processed_memberships
                            .get(collection.0 as usize)
                            .is_some_and(|covered| covered.contains(file.entry.id.0))
                    }) {
                        self.processed_files += 1;
                    }
                }
            }
        }
        let active = ActiveRecords::all(index.records.len()).map_err(IndexError::ResourceLimit)?;
        let mut segment_id = index.id;
        if segment_id.get() == 1
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
        index.id = segment_id;
        Arc::make_mut(&mut self.segments[number]).push(Arc::new(PublishedSegment {
            id: segment_id,
            index: Arc::new(index),
            active,
            observation,
        }));
        if self.next_segment_ordinal == 0 {
            self.next_segment_ordinal = 1;
        }
        Ok(self)
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
                    if self.catalog.entry(file.id).is_some_and(|current| {
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
            *group = kept;
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
                    let active = ActiveRecords::all(part.records.len())
                        .map_err(IndexError::ResourceLimit)?;
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
                return Ok(());
            }
        }
        // Native retained-memory estimate, not an encoded length or a strict
        // allocator count; it only orders when size-triggered maintenance runs.
        let sparse_bytes = group
            .iter()
            .filter(|segment| segment.index.record_count() < target_records)
            .map(|segment| segment.index.accounting.retained_bytes_estimated)
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
            .translate_active(&[&group[left].active, &group[right].active])
            .map_err(IndexError::Build)?;
        let ordinal = self.next_segment_ordinal;
        self.next_segment_ordinal = self
            .next_segment_ordinal
            .checked_add(1)
            .ok_or_else(|| IndexError::ResourceLimit("segment ordinal overflow".into()))?;
        let merged_index = merged.index;
        group.remove(right);
        group.remove(left);
        group.push(Arc::new(PublishedSegment {
            id: SegmentId::new(ordinal),
            index: Arc::new(merged_index),
            active,
            observation: None,
        }));
        if size_triggered {
            self.last_size_maintenance_revision = self.revision.0;
        }
        Ok(())
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
                *active = active
                    .retired(slots)
                    .map_err(IndexError::InvalidPublication)?;
            }
            let segments = self
                .segments
                .get_mut(collection_id.0 as usize)
                .ok_or_else(|| {
                    IndexError::InvalidPublication("segment collection missing".into())
                })?;
            for segment in Arc::make_mut(segments).iter_mut() {
                if let Some(slots) = segment.index.file_records(file.id) {
                    let slots = slots.to_vec();
                    let segment = Arc::make_mut(segment);
                    retired_records += slots
                        .iter()
                        .filter(|slot| segment.active.contains(**slot))
                        .count() as u64;
                    segment.active = segment
                        .active
                        .retired(&slots)
                        .map_err(IndexError::InvalidPublication)?;
                }
            }
        }
        Ok(retired_records)
    }
}

fn prepare_initial_segment(
    next: &mut PublishedIndex,
    segment: (
        IndexedCollectionSegment,
        Option<Arc<crate::construction::inference::InferenceObservation>>,
    ),
) -> Result<(), IndexError> {
    let (segment, observation) = segment;
    for file in &segment.files {
        if file.entry.status == FileStatus::Valid
            && file.entry.diagnostics.is_empty()
            && file.entry.omitted_diagnostics == 0
        {
            continue;
        }
        let mut merged = next
            .catalog
            .entry(file.entry.id)
            .ok_or(IndexError::UnknownFile)?
            .clone();
        merged.status = merged.status.max(file.entry.status);
        for diagnostic in &file.entry.diagnostics {
            merged
                .record_diagnostic(diagnostic.clone())
                .map_err(IndexError::InvalidPublication)?;
        }
        merged.omitted_diagnostics = merged
            .omitted_diagnostics
            .saturating_add(file.entry.omitted_diagnostics);
        next.catalog = next
            .catalog
            .replaced(merged, None, None)
            .map_err(IndexError::InvalidPublication)?;
    }
    *next = next
        .clone()
        .with_segment_observation(segment, observation)?;
    Ok(())
}

impl Index {
    /// Apply one accepted registrar batch atomically. Retirement and any new
    /// native segment are staged in a private root before the revision CAS.
    #[cfg(test)]
    pub(crate) fn publish_record_batch(
        &self,
        expected: IndexRevision,
        collection: CollectionId,
        registration: u32,
        changes: &[RecordMutation],
        segment: Option<IndexedCollectionSegment>,
    ) -> Result<IndexRevision, IndexError> {
        self.publish_record_batch_with_files(
            expected,
            collection,
            registration,
            changes,
            segment,
            &[],
        )
    }

    pub(crate) fn publish_record_batch_with_files(
        &self,
        expected: IndexRevision,
        collection: CollectionId,
        registration: u32,
        changes: &[RecordMutation],
        segment: Option<IndexedCollectionSegment>,
        files: &[crate::inventory::FileEntry],
    ) -> Result<IndexRevision, IndexError> {
        let view = self.view()?;
        if view.revision() != expected {
            return Err(IndexError::RevisionConflict {
                expected,
                actual: view.revision(),
            });
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
        for file in files {
            let root = next.repository.files().root().ok_or_else(|| {
                IndexError::InvalidPublication("file catalog is not registered".into())
            })?;
            let path = root.join(&file.relative_path);
            let canonical = path
                .canonicalize()
                .map_err(|error| IndexError::Build(error.to_string()))?;
            if !canonical.starts_with(root) {
                return Err(IndexError::InvalidPublication(
                    "file path escapes catalog root".into(),
                ));
            }
            let metadata = std::fs::metadata(canonical)
                .map_err(|error| IndexError::Build(error.to_string()))?;
            if !metadata.is_file()
                || crate::inventory::SourceRevision::from_metadata(&metadata)
                    .map_err(IndexError::Build)?
                    != file.revision
            {
                return Err(IndexError::InvalidPublication(
                    "declared file revision changed".into(),
                ));
            }
            if !file.collections.contains(&collection) {
                return Err(IndexError::InvalidPublication(
                    "declared file is not in collection".into(),
                ));
            }
            if let Some(existing) = next.catalog.entry(file.id) {
                if existing.relative_path != file.relative_path
                    || existing.revision != file.revision
                {
                    return Err(IndexError::InvalidPublication(
                        "file catalog identity changed".into(),
                    ));
                }
                let fingerprint = next.catalog.fingerprint(file.id);
                let configuration = next.catalog.configuration(file.id);
                next.catalog = next
                    .catalog
                    .replaced(file.clone(), fingerprint, configuration)
                    .map_err(IndexError::InvalidPublication)?;
            } else {
                next.catalog = next
                    .catalog
                    .appended(file.clone(), None, None)
                    .map_err(IndexError::InvalidPublication)?;
                next.file_versions = next
                    .file_versions
                    .appended()
                    .map_err(IndexError::InvalidPublication)?;
            }
        }
        for change in changes {
            let base = collection_state
                .id_lookup
                .get(&change.id)
                .copied()
                .filter(|slot| next.active[number].contains(*slot));
            let mut found = None;
            for (index, published) in next.segments[number].iter().enumerate() {
                if let Some(slot) = published.index.id_lookup.get(&change.id).copied()
                    && published.active.contains(slot)
                {
                    if found.replace((index, slot)).is_some() || base.is_some() {
                        return Err(IndexError::InvalidPublication(
                            "duplicate live record ID".into(),
                        ));
                    }
                }
            }
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
                    let owner = match &published.index.backing {
                        crate::index::segment::SegmentBacking::Retained {
                            registration, ..
                        }
                        | crate::index::segment::SegmentBacking::Provider {
                            registration, ..
                        } => *registration,
                        crate::index::segment::SegmentBacking::FileRange { .. } => u32::MAX,
                    };
                    if owner != registration {
                        return Err(IndexError::Build(format!(
                            "record {} belongs to another registrar",
                            change.id.0
                        )));
                    }
                    let group = Arc::make_mut(&mut next.segments[number]);
                    let published = Arc::make_mut(&mut group[index]);
                    published.active = published
                        .active
                        .retired(&[slot])
                        .map_err(IndexError::InvalidPublication)?;
                }
            }
        }
        if let Some(segment) = segment {
            if segment.collection != collection {
                return Err(IndexError::ForeignInstance);
            }
            next = next.with_segment_observation(segment, None)?;
        }
        next.revision = expected.next()?;
        self.publish_prepared(expected, next)
    }

    /// Publish one complete initial work unit. A concurrent lifecycle
    /// mutation changes the revision and rejects this stale work atomically.
    pub(crate) fn publish_initial_segment(
        &self,
        expected: IndexRevision,
        segment: IndexedCollectionSegment,
        observation: Option<Arc<crate::construction::inference::InferenceObservation>>,
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
            Option<Arc<crate::construction::inference::InferenceObservation>>,
        )>,
        profile_phases: bool,
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
        let mut next = (*view.0).clone();
        for segment in segments {
            prepare_initial_segment(&mut next, segment)?;
        }
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

    /// Finalize metadata without copying the completed segment payloads.
    pub(crate) fn finish_initial_construction(
        &self,
        expected: IndexRevision,
        repository: Arc<Repository>,
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
        if next.processed_files != next.catalog.active_files() as usize {
            return Err(IndexError::InvalidPublication(
                "initial construction has unprocessed files".into(),
            ));
        }
        for group in &mut next.segments {
            for published in Arc::make_mut(group).iter_mut() {
                let needs_status_refresh = published.index.files.iter().any(|file| {
                    repository
                        .files
                        .files
                        .get(file.entry.id.0 as usize)
                        .is_some_and(|current| current.status != file.entry.status)
                });
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
                    if let Some(current) = repository.files.files.get(file.entry.id.0 as usize) {
                        file.entry.status = current.status;
                        file.entry.diagnostics = current.diagnostics.clone();
                        file.entry.omitted_diagnostics = current.omitted_diagnostics;
                    }
                }
                index
                    .sorting
                    .refresh_file_status_for_records(&index.records, |slot| {
                        let file = index.backing.file_locator(slot)?.file;
                        Some(
                            repository
                                .files
                                .files
                                .get(file.0 as usize)
                                .map_or(crate::inventory::FileStatus::Valid.code(), |entry| {
                                    entry.status.code()
                                }),
                        )
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
        next.catalog
            .rebase(Arc::clone(&repository))
            .map_err(IndexError::InvalidPublication)?;
        next.catalog.set_construction_complete(true);
        next.processed_memberships.clear();
        next.repository = repository;
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
            std::mem::replace(&mut *guard, next)
        };
        drop(retired);
        #[cfg(test)]
        let _ = notify_cache;
        Ok(next_revision)
    }
}
