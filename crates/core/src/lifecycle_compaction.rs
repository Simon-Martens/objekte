//! Active-only, source-free maintenance of the published index.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::build::{IndexedCollectionSegment, SegmentFile};
use crate::inventory::ActiveRecords;
use crate::lifecycle::{
    Index, IndexError, IndexRevision, MaintenancePhase, PublishedIndex, PublishedSegment,
};
use crate::lifecycle_catalog::{FileCatalog, OwnershipRoute};
use crate::lifecycle_partition::compact_source_parts_with_policy;
use crate::repository::CollectionState;

/// Dense maintenance result. Old pinned views may keep retired memory until
/// their readers finish; the new root contains only active contributions.
#[derive(Clone, Debug)]
pub struct CompactionReport {
    pub revision: IndexRevision,
    pub attempts: usize,
    pub slots_before: u64,
    pub slots_after: u64,
    pub retired_slots_removed: u64,
    /// Retired slots left for a later bounded compaction call.
    pub remaining_retired_slots: u64,
    /// Source slot metadata visited by this call.
    pub input_records_examined: u64,
    /// Greater of the native retained-memory estimate and owning file sizes per shard.
    pub input_bytes_budgeted: u64,
    /// Planning estimate, not an allocator count or a strict peak bound.
    pub temporary_bytes_estimated: u64,
    pub pinned_prior_views: usize,
    pub duration: Duration,
    pub segments_before: usize,
    pub segments_after: usize,
    pub segments_rewritten: usize,
    pub segments_removed: usize,
    pub segments_merged: usize,
    pub segments_retained_dirty: usize,
    pub segments_merge_candidates: usize,
    pub segments_produced: usize,
    pub archive_bytes_before: usize,
    pub archive_bytes_after: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CompactionMode {
    #[default]
    Bounded,
    Full,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CompactionOptions {
    pub mode: CompactionMode,
    pub merge_small_segments: bool,
}

#[derive(Clone, Copy)]
struct CompactionBudget {
    records: u64,
    input_bytes: u64,
    temporary_bytes: u64,
    elapsed: Duration,
}

impl CompactionBudget {
    // One publication batch may include several shards. A single shard can
    // exceed a limit so that repeated calls always make forward progress.
    const DEFAULT: Self = Self {
        records: 32_768,
        input_bytes: 64 * 1024 * 1024,
        temporary_bytes: 128 * 1024 * 1024,
        elapsed: Duration::from_secs(1),
    };
}

#[derive(Default)]
struct CompactionWork {
    records: u64,
    input_bytes: u64,
    temporary_bytes: u64,
    segments_reconstructed: usize,
    segments_produced: usize,
    merge_candidates: usize,
}

fn source_budget_bytes(source: &IndexedCollectionSegment) -> u64 {
    u64::try_from(source.accounting.retained_bytes_estimated)
        .unwrap_or(u64::MAX)
        .max(source.accounting.source_bytes_estimated)
}

fn budget_allows(
    work: &CompactionWork,
    budget: CompactionBudget,
    records: u64,
    input_bytes: u64,
    temporary_bytes: u64,
    started: Instant,
) -> bool {
    work.records.saturating_add(records) <= budget.records
        && work.input_bytes.saturating_add(input_bytes) <= budget.input_bytes
        && work.temporary_bytes.saturating_add(temporary_bytes) <= budget.temporary_bytes
        && started.elapsed() < budget.elapsed
}

fn charge_work(
    work: &mut CompactionWork,
    records: u64,
    input_bytes: u64,
    temporary_bytes: u64,
) {
    work.records = work.records.saturating_add(records);
    work.input_bytes = work.input_bytes.saturating_add(input_bytes);
    work.temporary_bytes = work.temporary_bytes.saturating_add(temporary_bytes);
}

fn removed_ownership_routes(
    index: &IndexedCollectionSegment,
    segment: crate::native_segment::SegmentId,
    versions: &crate::inventory::FileVersions,
) -> Vec<(crate::model::FileId, crate::inventory::FileVersion, OwnershipRoute)> {
    (0..index.file_count())
        .filter_map(|ordinal| {
            let file = index.files.file_id(ordinal)?;
            let version = versions.get(file)?;
            Some((
                file,
                version,
                OwnershipRoute {
                    collection: index.collection,
                    segment,
                    file_row_ordinal: ordinal as u32,
                },
            ))
        })
        .collect()
}

fn added_ownership_routes(
    segment: &PublishedSegment,
) -> Vec<(crate::model::FileId, OwnershipRoute)> {
    (0..segment.index.file_count())
        .filter_map(|ordinal| {
            let file = segment.index.files.file_id(ordinal)?;
            Some((
                file,
                OwnershipRoute {
                    collection: segment.index.collection,
                    segment: segment.id,
                    file_row_ordinal: ordinal as u32,
                },
            ))
        })
        .collect()
}

fn commit_compaction_ownership(
    root: &mut PublishedIndex,
    removed: &[(crate::model::FileId, crate::inventory::FileVersion, OwnershipRoute)],
    added: &[(crate::model::FileId, OwnershipRoute)],
) -> Result<(), IndexError> {
    root.catalog
        .update_ownership(&root.file_versions, removed, added)
        .map_err(IndexError::InvalidPublication)
}

pub(crate) fn base_as_segment(
    collection: &CollectionState,
    files: &crate::inventory::FileInventory,
) -> Result<IndexedCollectionSegment, IndexError> {
    let mut segment_files = Vec::new();
    let mut file_index = HashMap::new();
    for id in &collection.generation.files {
        let entry = files
            .files
            .get(id.0 as usize)
            .ok_or_else(|| IndexError::InvalidPublication("base file missing".into()))?
            .clone();
        let records = collection
            .generation
            .file_records
            .records_for(*id)
            .ok_or_else(|| IndexError::InvalidPublication("base file ownership missing".into()))?
            .to_vec();
        file_index.insert(*id, segment_files.len());
        segment_files.push(SegmentFile { entry, records });
    }
    let relations = collection
        .relations
        .iter()
        .map(|relation| {
            crate::relation::RelationContribution::from_resolved_index(
                relation,
                collection.generation.records.len(),
            )
            .map_err(IndexError::InvalidPublication)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let (segment_files, records) =
        crate::segment_archive::SegmentRows::freeze(segment_files, collection.generation.records.clone())
            .map_err(IndexError::InvalidPublication)?
            .views();
    let file_index =
        crate::segment_archive::FileLookup::freeze(file_index).map_err(IndexError::InvalidPublication)?;
    let id_lookup = crate::segment_archive::RecordLookup::freeze(collection.id_lookup.clone())
        .map_err(IndexError::InvalidPublication)?;
    let mut segment = IndexedCollectionSegment {
        id: crate::native_segment::SegmentId::new(1),
        collection: collection.generation.collection,
        source_id: collection.source.id().into(),
        source_version: collection.source.version(),
        files: segment_files,
        file_index,
        records,
        id_lookup,
        identity_order: collection.identity_order.clone(),
        postings: collection.postings.clone(),
        inference_observation: None,
        inference: collection.inference.clone(),
        custom_postings: collection.custom_postings.clone(),
        raw: collection.raw.clone(),
        fallback_text: collection.fallback_text.clone(),
        custom_text: collection.custom_text.clone(),
        logical_records: collection.logical_records.clone(),
        empty_array_records: collection.empty_array_records.clone(),
        sorting: collection.sorting.clone(),
        relations,
        record_parses: 0,
        failed_parses: 0,
        accounting: Default::default(),
    };
    segment.recompute_accounting();
    Ok(segment)
}

fn slot_count(root: &PublishedIndex) -> u64 {
    root.repository
        .collections()
        .iter()
        .enumerate()
        .map(|(number, _collection)| {
            root.active[number].slot_count() as u64
                + root.segments[number]
                    .iter()
                    .map(|segment| segment.index.record_count() as u64)
                    .sum::<u64>()
        })
        .sum()
}

fn segment_count(root: &PublishedIndex) -> usize {
    root.segments.iter().map(|group| group.len()).sum()
}

fn publication_archive_bytes(root: &PublishedIndex) -> usize {
    root.archive_bytes()
}

fn clean_merge_compatible(
    left: &IndexedCollectionSegment,
    right: &IndexedCollectionSegment,
    target_records: usize,
    max_files: usize,
) -> bool {
    crate::storage_usage::StorageWorkCounters::update(|counters| {
        counters.merge_candidate_probes = counters.merge_candidate_probes.saturating_add(1);
    });
    if left.collection != right.collection
        || left.source_id != right.source_id
        || left.source_version != right.source_version
        || left.record_count().saturating_add(right.record_count()) > target_records
        || left.file_count().saturating_add(right.file_count()) > max_files
    {
        return false;
    }
    for index in 0..left.file_count() {
        let Some(file) = left.files.file_id(index) else {
            return false;
        };
        if right.file_index.get(file).is_some() {
            return false;
        }
    }
    let left_ids = left
        .identity_order
        .iter()
        .filter_map(|slot| left.records.record_id(*slot))
        .collect::<std::collections::HashSet<_>>();
    for slot in 0..right.record_count() {
        let Some(id) = right.records.record_id(crate::model::RecordSlot(slot as u32)) else {
            return false;
        };
        if left_ids.contains(id) {
            return false;
        }
    }
    true
}

fn merge_small_segments(
    root: &mut PublishedIndex,
    work: &mut CompactionWork,
    budget: CompactionBudget,
    started: Instant,
    cancelled: impl Fn() -> bool,
) -> Result<usize, IndexError> {
    let mut merged_count = 0usize;
    let target = root.repository.options().segment_policy.target_records.get();
    let max_files = root.repository.options().segment_policy.max_files.get();
    for number in 0..root.segments.len() {
        loop {
            if cancelled() {
                return Err(IndexError::Cancelled);
            }
            let group = &root.segments[number];
            let mut positions = group
                .iter()
                .enumerate()
                .filter_map(|(position, segment)| {
                    (segment.active.len() == segment.index.record_count() as u64)
                        .then_some(position)
                })
                .collect::<Vec<_>>();
            positions.sort_by(|left, right| {
                group[*left]
                    .active
                    .len()
                    .cmp(&group[*right].active.len())
                    .then_with(|| group[*left].id.cmp(&group[*right].id))
            });
            if positions.len() < 2
                || group[positions[0]]
                    .active
                    .len()
                    .saturating_add(group[positions[1]].active.len())
                    > target as u64
            {
                break;
            }
            let mut selected = None;
            'pairs: for (left_at, left) in positions.iter().copied().enumerate() {
                for right in positions.iter().copied().skip(left_at + 1) {
                    let combined = group[left]
                        .active
                        .len()
                        .saturating_add(group[right].active.len());
                    if combined > target as u64 {
                        break;
                    }
                    if clean_merge_compatible(
                        &group[left].index,
                        &group[right].index,
                        target,
                        max_files,
                    ) {
                        selected = Some((left, right));
                        break 'pairs;
                    }
                }
            }
            let Some((left, right)) = selected else {
                break;
            };
            let records = group[left]
                .index
                .record_count()
                .saturating_add(group[right].index.record_count()) as u64;
            let input_bytes = source_budget_bytes(&group[left].index)
                .saturating_add(source_budget_bytes(&group[right].index));
            let temporary_bytes = input_bytes
                .saturating_mul(3)
                .saturating_add(records.saturating_mul(512));
            if !budget_allows(
                work,
                budget,
                records,
                input_bytes,
                temporary_bytes,
                started,
            ) {
                break;
            }
            charge_work(work, records, input_bytes, temporary_bytes);
            work.merge_candidates = work.merge_candidates.saturating_add(1);
            if cancelled() {
                return Err(IndexError::Cancelled);
            }
            let consolidated = IndexedCollectionSegment::consolidate(
                &[&group[left].index, &group[right].index],
                target,
            )
            .map_err(IndexError::Build)?;
            work.segments_reconstructed = work.segments_reconstructed.saturating_add(1);
            work.segments_produced = work.segments_produced.saturating_add(1);
            let mut index = consolidated.index;
            index.id = crate::native_segment::SegmentId::new(root.next_segment_ordinal);
            root.next_segment_ordinal = root
                .next_segment_ordinal
                .checked_add(1)
                .ok_or_else(|| IndexError::ResourceLimit("segment ordinal overflow".into()))?;
            let active =
                ActiveRecords::all(index.record_count()).map_err(IndexError::ResourceLimit)?;
            let replacement = Arc::new(
                PublishedSegment::checked(index.id, Arc::new(index), active)
                    .map_err(IndexError::InvalidPublication)?,
            );
            let mut removed = removed_ownership_routes(
                &group[left].index,
                group[left].id,
                &root.file_versions,
            );
            removed.extend(removed_ownership_routes(
                &group[right].index,
                group[right].id,
                &root.file_versions,
            ));
            let added = added_ownership_routes(&replacement);
            root.catalog
                .update_ownership(&root.file_versions, &removed, &added)
                .map_err(IndexError::InvalidPublication)?;
            let group = Arc::make_mut(&mut root.segments[number]);
            let (first, second) = if left < right {
                (left, right)
            } else {
                (right, left)
            };
            group.remove(second);
            group.remove(first);
            group.insert(first, replacement);
            merged_count += 1;
        }
    }
    Ok(merged_count)
}

/// Rebuild one staged publication from its active contributions. The caller
/// chooses the already reserved revision and performs the single CAS publish.
pub(crate) fn compact_published(
    old: &PublishedIndex,
    next_revision: IndexRevision,
) -> Result<PublishedIndex, IndexError> {
    let (next, _) =
        compact_published_with_budget(old, next_revision, || false, CompactionBudget::DEFAULT)?;
    let live = old.active.iter().map(ActiveRecords::len).sum::<u64>()
        + old
            .segments
            .iter()
            .flat_map(|group| group.iter())
            .map(|segment| segment.active.len())
            .sum::<u64>();
    if slot_count(&next) > live {
        return Err(IndexError::ResourceLimit(
            "verified compaction exceeded its work budget".into(),
        ));
    }
    Ok(next)
}

fn compact_published_with_budget(
    old: &PublishedIndex,
    next_revision: IndexRevision,
    cancelled: impl Fn() -> bool,
    budget: CompactionBudget,
) -> Result<(PublishedIndex, CompactionWork), IndexError> {
    if cancelled() {
        return Err(IndexError::Cancelled);
    }
    let mut ownership_removed = Vec::new();
    let mut ownership_added = Vec::new();
    // After the initial base has been partitioned, every contribution lives
    // in an immutable shard. Keep the existing catalog and unaffected shard
    // groups instead of rebuilding an empty repository for every collection.
    if old.active.iter().all(|active| active.slot_count() == 0) {
        let started = Instant::now();
        let mut work = CompactionWork::default();
        let mut next = old.clone();
        next.revision = next_revision;
        for (number, group) in old.segments.iter().enumerate() {
            if cancelled() {
                return Err(IndexError::Cancelled);
            }
            if group
                .iter()
                .all(|segment| segment.active.len() == segment.index.record_count() as u64)
            {
                continue;
            }
            let mut compacted = Vec::with_capacity(group.len());
            for segment in group.iter() {
                if cancelled() {
                    return Err(IndexError::Cancelled);
                }
                if segment.active.len() == segment.index.record_count() as u64 {
                    compacted.push(Arc::clone(segment));
                    continue;
                }
                let records = segment.index.record_count() as u64;
                let input_bytes = source_budget_bytes(&segment.index);
                let temporary_bytes = input_bytes
                    .saturating_mul(3)
                    .saturating_add(records.saturating_mul(512));
                if work.records > 0
                    && (work.records.saturating_add(records) > budget.records
                        || work.input_bytes.saturating_add(input_bytes) > budget.input_bytes
                        || work.temporary_bytes.saturating_add(temporary_bytes)
                            > budget.temporary_bytes
                        || started.elapsed() >= budget.elapsed)
                {
                    compacted.push(Arc::clone(segment));
                    continue;
                }
                work.records = work.records.saturating_add(records);
                work.input_bytes = work.input_bytes.saturating_add(input_bytes);
                work.temporary_bytes = work.temporary_bytes.saturating_add(temporary_bytes);
                work.segments_reconstructed = work.segments_reconstructed.saturating_add(1);
                ownership_removed.extend(removed_ownership_routes(
                    &segment.index,
                    segment.id,
                    &next.file_versions,
                ));
                let parts = compact_source_parts_with_policy(
                    &segment.index,
                    &segment.active,
                    &next.catalog,
                    next.repository.options().segment_policy,
                    &cancelled,
                )?;
                work.segments_produced = work
                    .segments_produced
                    .saturating_add(parts.len());
                for part in parts {
                    if cancelled() {
                        return Err(IndexError::Cancelled);
                    }
                    let ordinal = next.next_segment_ordinal;
                    next.next_segment_ordinal = ordinal.checked_add(1).ok_or_else(|| {
                        IndexError::ResourceLimit("segment ordinal overflow".into())
                    })?;
                    let active = ActiveRecords::all(part.records.len())
                        .map_err(IndexError::ResourceLimit)?;
                    let id = crate::native_segment::SegmentId::new(ordinal);
                    let mut part = part;
                    part.id = id;
                    let published = Arc::new(
                        PublishedSegment::checked(id, Arc::new(part), active)
                            .map_err(IndexError::InvalidPublication)?,
                    );
                    ownership_added.extend(added_ownership_routes(&published));
                    compacted.push(published);
                }
            }
            next.segments[number] = Arc::new(compacted);
        }
        commit_compaction_ownership(
            &mut next,
            &ownership_removed,
            &ownership_added,
        )?;
        next.rebuild_all_segment_positions();
        return Ok((next, work));
    }
    let empty = Arc::new(old.repository.empty_like().map_err(IndexError::Build)?);
    let mut next =
        PublishedIndex::from_repository(old.instance_id, next_revision, Arc::clone(&empty))?;
    if next.configuration_identity != old.configuration_identity {
        return Err(IndexError::IncompatibleConfiguration(
            "compaction configuration changed".into(),
        ));
    }
    next.catalog = FileCatalog::restored(Arc::clone(&empty), old.catalog.snapshot_slots())
        .map_err(IndexError::InvalidPublication)?;
    next.file_versions = old.file_versions.clone();
    next.next_segment_ordinal = old.next_segment_ordinal;
    next.last_size_maintenance_revision = old.last_size_maintenance_revision;
    next.reconciliation = old.reconciliation;
    for (number, collection) in old.repository.collections().iter().enumerate() {
        if cancelled() {
            return Err(IndexError::Cancelled);
        }
        let mut compacted = Vec::new();
        let mut compact_source = |source: &IndexedCollectionSegment,
                                  active: &ActiveRecords|
         -> Result<Vec<Arc<PublishedSegment>>, IndexError> {
            if cancelled() {
                return Err(IndexError::Cancelled);
            }
            let mut replacements = Vec::new();
            for part in compact_source_parts_with_policy(
                source,
                active,
                &next.catalog,
                next.repository.options().segment_policy,
                &cancelled,
            )? {
                if cancelled() {
                    return Err(IndexError::Cancelled);
                }
                let ordinal = next.next_segment_ordinal;
                next.next_segment_ordinal = ordinal
                    .checked_add(1)
                    .ok_or_else(|| IndexError::ResourceLimit("segment ordinal overflow".into()))?;
                let active =
                    ActiveRecords::all(part.records.len()).map_err(IndexError::ResourceLimit)?;
                let id = crate::native_segment::SegmentId::new(ordinal);
                let mut part = part;
                part.id = id;
                replacements.push(Arc::new(
                    PublishedSegment::checked(id, Arc::new(part), active)
                        .map_err(IndexError::InvalidPublication)?,
                ));
            }
            Ok(replacements)
        };
        if old.active[number].slot_count() != 0 {
            let mut base = base_as_segment(collection, old.repository.files())?;
            base.id = crate::native_segment::SegmentId::new(0);
            ownership_removed.extend(removed_ownership_routes(
                &base,
                base.id,
                &next.file_versions,
            ));
            let replacements = compact_source(&base, &old.active[number])?;
            ownership_added.extend(
                replacements
                    .iter()
                    .flat_map(|segment| added_ownership_routes(segment)),
            );
            compacted.extend(replacements);
        }
        for segment in old.segments[number].iter() {
            if segment.active.len() == segment.index.record_count() as u64 {
                compacted.push(Arc::clone(segment));
            } else {
                ownership_removed.extend(removed_ownership_routes(
                    &segment.index,
                    segment.id,
                    &next.file_versions,
                ));
                let replacements =
                    compact_source(&segment.index, &segment.active)?;
                ownership_added.extend(
                    replacements
                        .iter()
                        .flat_map(|segment| added_ownership_routes(segment)),
                );
                compacted.extend(replacements);
            }
        }
        drop(compact_source);
        next.segments[number] = Arc::new(compacted);
    }
    let all_routes = crate::lifecycle::initial_ownership_routes(
        &next.repository,
        &next.active,
        &next.segments,
        next.file_versions.len(),
    );
    next.catalog
        .replace_all_ownership(&next.file_versions, &all_routes)
        .map_err(IndexError::InvalidPublication)?;
    next.rebuild_all_segment_positions();
    Ok((next, CompactionWork::default()))
}

impl Index {
    /// Reclaim retired records without reading source files. One call handles
    /// a bounded batch; `remaining_retired_slots` tells callers when to call
    /// again. Preparation uses a pinned root and retries publication conflicts
    /// without replacing a newer revision.
    pub fn compact(&self) -> Result<CompactionReport, IndexError> {
        self.compact_with_options(CompactionOptions::default())
    }

    pub fn compact_with_options(
        &self,
        options: CompactionOptions,
    ) -> Result<CompactionReport, IndexError> {
        self.compact_with_control_options(|| false, |_, _| {}, options)
    }

    pub(crate) fn compact_with_control(
        &self,
        cancelled: impl Fn() -> bool,
        progress: impl FnMut(MaintenancePhase, usize),
    ) -> Result<CompactionReport, IndexError> {
        self.compact_with_control_options(cancelled, progress, CompactionOptions::default())
    }

    fn compact_with_control_options(
        &self,
        cancelled: impl Fn() -> bool,
        mut progress: impl FnMut(MaintenancePhase, usize),
        options: CompactionOptions,
    ) -> Result<CompactionReport, IndexError> {
        let started = Instant::now();
        let mut conflict = None;
        for attempt in 1..=3 {
            if cancelled() {
                return Err(IndexError::Cancelled);
            }
            progress(MaintenancePhase::Preparing, attempt);
            let view = self.view()?;
            let before = slot_count(&view.0);
            let segments_before = segment_count(&view.0);
            let archive_bytes_before = publication_archive_bytes(&view.0);
            let old = &view.0;
            let live = old.active.iter().map(ActiveRecords::len).sum::<u64>()
                + old
                    .segments
                    .iter()
                    .flat_map(|group| group.iter())
                    .map(|segment| segment.active.len())
                    .sum::<u64>();
            if before == live && !(options.mode == CompactionMode::Full && options.merge_small_segments)
            {
                return Ok(CompactionReport {
                    revision: view.revision(),
                    attempts: attempt,
                    slots_before: before,
                    slots_after: before,
                    retired_slots_removed: 0,
                    remaining_retired_slots: 0,
                    input_records_examined: 0,
                    input_bytes_budgeted: 0,
                    temporary_bytes_estimated: 0,
                    pinned_prior_views: 0,
                    duration: started.elapsed(),
                    segments_before,
                    segments_after: segments_before,
                    segments_rewritten: 0,
                    segments_removed: 0,
                    segments_merged: 0,
                    segments_retained_dirty: 0,
                    segments_merge_candidates: 0,
                    segments_produced: 0,
                    archive_bytes_before,
                    archive_bytes_after: archive_bytes_before,
                });
            }
            let budget = match options.mode {
                CompactionMode::Bounded => CompactionBudget::DEFAULT,
                CompactionMode::Full => CompactionBudget {
                    records: u64::MAX,
                    input_bytes: u64::MAX,
                    temporary_bytes: u64::MAX,
                    elapsed: Duration::MAX,
                },
            };
            let (mut next, mut work) = compact_published_with_budget(
                old,
                view.revision().next()?,
                &cancelled,
                budget,
            )?;
            let segments_merged = if options.merge_small_segments {
                merge_small_segments(&mut next, &mut work, budget, started, &cancelled)?
            } else {
                0
            };
            next.rebuild_all_segment_positions();
            if cancelled() {
                return Err(IndexError::Cancelled);
            }
            progress(MaintenancePhase::Publishing, attempt);
            let after = slot_count(&next);
            let segments_after = segment_count(&next);
            let segments_retained_dirty = next
                .segments
                .iter()
                .flat_map(|group| group.iter())
                .filter(|segment| segment.active.len() != segment.index.record_count() as u64)
                .count();
            let archive_bytes_after = publication_archive_bytes(&next);
            if after == before && segments_after == segments_before && segments_merged == 0 {
                return Ok(CompactionReport {
                    revision: view.revision(),
                    attempts: attempt,
                    slots_before: before,
                    slots_after: after,
                    retired_slots_removed: 0,
                    remaining_retired_slots: after.saturating_sub(live),
                    input_records_examined: 0,
                    input_bytes_budgeted: 0,
                    temporary_bytes_estimated: 0,
                    pinned_prior_views: Arc::strong_count(&view.0).saturating_sub(2),
                    duration: started.elapsed(),
                    segments_before,
                    segments_after,
                    segments_rewritten: 0,
                    segments_removed: 0,
                    segments_merged: 0,
                    segments_retained_dirty,
                    segments_merge_candidates: 0,
                    segments_produced: 0,
                    archive_bytes_before,
                    archive_bytes_after,
                });
            }
            let pinned_prior_views = Arc::strong_count(&view.0).saturating_sub(2);
            match self.publish_prepared(view.revision(), next) {
                Ok(revision) => {
                    return Ok(CompactionReport {
                        revision,
                        attempts: attempt,
                        slots_before: before,
                        slots_after: after,
                        retired_slots_removed: before.saturating_sub(after),
                        remaining_retired_slots: after.saturating_sub(live),
                        input_records_examined: work.records,
                        input_bytes_budgeted: work.input_bytes,
                        temporary_bytes_estimated: work.temporary_bytes,
                        pinned_prior_views,
                        duration: started.elapsed(),
                        segments_before,
                        segments_after,
                        segments_rewritten: work.segments_reconstructed,
                        segments_removed: segments_before
                            .saturating_sub(segments_after)
                            .saturating_sub(segments_merged),
                        segments_merged,
                        segments_retained_dirty,
                        segments_merge_candidates: work.merge_candidates,
                        segments_produced: work.segments_produced,
                        archive_bytes_before,
                        archive_bytes_after,
                    });
                }
                Err(error @ IndexError::RevisionConflict { .. }) => conflict = Some(error),
                Err(error) => return Err(error),
            }
        }
        Err(conflict.expect("compaction retry always captures a revision conflict"))
    }
}

#[cfg(test)]
#[path = "lifecycle_compaction.test.rs"]
mod tests;
