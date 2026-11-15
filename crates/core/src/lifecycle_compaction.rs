//! Active-only, source-free maintenance of the published index.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::build::{IndexedCollectionSegment, SegmentFile};
use crate::inventory::ActiveRecords;
use crate::lifecycle::{
    Index, IndexError, IndexRevision, MaintenancePhase, PublishedIndex, PublishedSegment,
};
use crate::lifecycle_catalog::FileCatalog;
use crate::lifecycle_partition::compact_source_parts;
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
}


pub(crate) fn seal_native_segments(next: &mut PublishedIndex) -> Result<(), IndexError> {
    let definitions = next
        .repository
        .collections()
        .iter()
        .map(|collection| collection.definition.clone())
        .collect::<Vec<_>>();
    for (number, group) in next.segments.iter_mut().enumerate() {
        if group.iter().all(|segment| segment.packed.is_some()) {
            continue;
        }
        let mut sealed = Vec::with_capacity(group.len());
        for segment in group.iter() {
            if segment.packed.is_some() {
                sealed.push(Arc::clone(segment));
                continue;
            }
            let active = segment.active.bitmap();
            let input = crate::index_storage::native_pack::pack_segment(
                &segment.index,
                &active,
                &next.file_versions,
                next.repository.collections(),
                &next.catalog,
            )
            .map_err(|error| IndexError::InvalidPublication(error.to_string()))?;
            let built = crate::index_storage::segment_builder::build_segment(input)
                .map_err(|error| IndexError::InvalidPublication(error.to_string()))?;
            let frozen = Arc::new(
                crate::index_storage::segment::FrozenSegment::from_bytes(built.object.into_bytes())
                    .map_err(|error| IndexError::InvalidPublication(error.to_string()))?,
            );
            let index = crate::index_storage::packed_restore::segment_from_packed(
                &frozen,
                number,
                &next.catalog,
                &definitions,
            )?;
            let active = ActiveRecords::from_bitmap(index.record_count(), active)
                .map_err(IndexError::ResourceLimit)?;
            sealed.push(Arc::new(PublishedSegment {
                ordinal: segment.ordinal,
                level: segment.level,
                index: Arc::new(index),
                active,
                packed: Some(frozen),
            }));
        }
        *group = Arc::new(sealed);
    }
    Ok(())
}

fn source_budget_bytes(source: &IndexedCollectionSegment) -> u64 {
    u64::try_from(source.accounting.retained_bytes_estimated)
        .unwrap_or(u64::MAX)
        .max(source.accounting.source_bytes_estimated)
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
    let mut segment = IndexedCollectionSegment {
        collection: collection.generation.collection,
        source_id: collection.source.id().into(),
        source_version: collection.source.version(),
        files: segment_files,
        file_index,
        records: collection.generation.records.clone(),
        id_lookup: collection.id_lookup.clone(),
        identity_order: collection.identity_order.clone(),
        postings: collection.postings.clone(),
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
        packed: collection.packed.clone(),
    };
    segment.recompute_accounting();
    Ok(segment)
}

fn slot_count(root: &PublishedIndex) -> u64 {
    root.repository
        .collections()
        .iter()
        .enumerate()
        .map(|(number, collection)| {
            collection.generation.records.len() as u64
                + root.segments[number]
                    .iter()
                    .map(|segment| segment.index.record_count() as u64)
                    .sum::<u64>()
        })
        .sum()
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
    // After the initial base has been partitioned, every contribution lives
    // in an immutable shard. Keep the existing catalog and unaffected shard
    // groups instead of rebuilding an empty repository for every collection.
    if old
        .repository
        .collections()
        .iter()
        .all(|collection| collection.generation.files.is_empty())
    {
        let started = Instant::now();
        let mut work = CompactionWork::default();
        let mut next = old.clone();
        next.revision = next_revision;
        let definitions = old
            .repository
            .collections()
            .iter()
            .map(|collection| collection.definition.clone())
            .collect::<Vec<_>>();
        'collections: for (number, group) in old.segments.iter().enumerate() {
            if cancelled() {
                return Err(IndexError::Cancelled);
            }
            let mut compacted = Vec::with_capacity(group.len());
            for (position, segment) in group.iter().enumerate() {
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
                    compacted.extend(group[position..].iter().cloned());
                    next.segments[number] = Arc::new(compacted);
                    break 'collections;
                }
                work.records = work.records.saturating_add(records);
                work.input_bytes = work.input_bytes.saturating_add(input_bytes);
                work.temporary_bytes = work.temporary_bytes.saturating_add(temporary_bytes);
                if let Some(frozen) = segment.index.packed.as_ref() {
                    if let Some(packed) =
                        crate::index_storage::stream_compact::compact_packed_merge(
                            &[frozen],
                            &[segment.active.bitmap()],
                            number as u32,
                        )
                        .map_err(|error| IndexError::InvalidPublication(error.to_string()))?
                    {
                        let packed = Arc::new(packed);
                        let ordinal = next.next_segment_ordinal;
                        next.next_segment_ordinal = ordinal.checked_add(1).ok_or_else(|| {
                            IndexError::ResourceLimit("segment ordinal overflow".into())
                        })?;
                        let index = crate::index_storage::packed_restore::segment_from_packed(
                            &packed,
                            number,
                            &next.catalog,
                            &definitions,
                        )?;
                        let active = ActiveRecords::all(index.record_count())
                            .map_err(IndexError::ResourceLimit)?;
                        compacted.push(Arc::new(PublishedSegment {
                            ordinal,
                            level: segment.level.saturating_add(1),
                            index: Arc::new(index),
                            active,
                            packed: Some(Arc::clone(&packed)),
                        }));
                    }
                    continue;
                }
                for part in compact_source_parts(
                    &segment.index,
                    &segment.active,
                    &next.catalog,
                    &cancelled,
                )? {
                    if cancelled() {
                        return Err(IndexError::Cancelled);
                    }
                    let ordinal = next.next_segment_ordinal;
                    next.next_segment_ordinal = ordinal.checked_add(1).ok_or_else(|| {
                        IndexError::ResourceLimit("segment ordinal overflow".into())
                    })?;
                    let active = ActiveRecords::all(part.record_count())
                        .map_err(IndexError::ResourceLimit)?;
                    compacted.push(Arc::new(PublishedSegment {
                        ordinal,
                        level: segment.level.saturating_add(1),
                        index: Arc::new(part),
                        active,
                        packed: None,
                    }));
                }
            }
            consolidate_same_level_runs(
                &mut compacted,
                number,
                &next.catalog,
                &definitions,
                &mut next.next_segment_ordinal,
                &cancelled,
                &budget,
                &mut work,
            )?;
            next.segments[number] = Arc::new(compacted);
        }
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
    next.catalog = FileCatalog::restored_with_liveness(
        Arc::clone(&empty),
        old.catalog.snapshot_slots_with_liveness(),
    )
    .map_err(IndexError::InvalidPublication)?;
    next.file_versions = old.file_versions.clone();
    next.next_segment_ordinal = old.next_segment_ordinal;
    next.last_size_maintenance_revision = old.last_size_maintenance_revision;
    next.reconciliation = old.reconciliation;
    let definitions = old
        .repository
        .collections()
        .iter()
        .map(|collection| collection.definition.clone())
        .collect::<Vec<_>>();
    for (number, collection) in old.repository.collections().iter().enumerate() {
        if cancelled() {
            return Err(IndexError::Cancelled);
        }
        let mut compacted = Vec::new();
        let mut compact_source = |source: &IndexedCollectionSegment,
                                  active: &ActiveRecords,
                                  source_level: u32|
         -> Result<Vec<Arc<PublishedSegment>>, IndexError> {
            if cancelled() {
                return Err(IndexError::Cancelled);
            }
            let mut replacements = Vec::new();
            if let Some(frozen) = source.packed.as_ref() {
                let parts = crate::index_storage::stream_compact::compact_packed_parts(
                    frozen,
                    &active.bitmap(),
                    number as u32,
                )
                .map_err(|error| IndexError::InvalidPublication(error.to_string()))?;
                for packed in parts {
                    if cancelled() {
                        return Err(IndexError::Cancelled);
                    }
                    let packed = Arc::new(packed);
                    let ordinal = next.next_segment_ordinal;
                    next.next_segment_ordinal = ordinal.checked_add(1).ok_or_else(|| {
                        IndexError::ResourceLimit("segment ordinal overflow".into())
                    })?;
                    let index = crate::index_storage::packed_restore::segment_from_packed(
                        &packed,
                        number,
                        &next.catalog,
                        &definitions,
                    )?;
                    let active = ActiveRecords::all(index.record_count())
                        .map_err(IndexError::ResourceLimit)?;
                    replacements.push(Arc::new(PublishedSegment {
                        ordinal,
                        level: source_level.saturating_add(1),
                        index: Arc::new(index),
                        active,
                        packed: Some(Arc::clone(&packed)),
                    }));
                }
                return Ok(replacements);
            }
            for part in compact_source_parts(source, active, &next.catalog, &cancelled)? {
                if cancelled() {
                    return Err(IndexError::Cancelled);
                }
                let ordinal = next.next_segment_ordinal;
                next.next_segment_ordinal = ordinal
                    .checked_add(1)
                    .ok_or_else(|| IndexError::ResourceLimit("segment ordinal overflow".into()))?;
                let active =
                    ActiveRecords::all(part.record_count()).map_err(IndexError::ResourceLimit)?;
                replacements.push(Arc::new(PublishedSegment {
                    ordinal,
                    level: source_level.saturating_add(1),
                    index: Arc::new(part),
                    active,
                    packed: None,
                }));
            }
            Ok(replacements)
        };
        if !collection.generation.records.is_empty() {
            let base = base_as_segment(collection, old.repository.files())?;
            compacted.extend(compact_source(&base, &old.active[number], 0)?);
        }
        for segment in old.segments[number].iter() {
            if segment.active.len() == segment.index.record_count() as u64 {
                compacted.push(Arc::clone(segment));
            } else {
                compacted.extend(compact_source(
                    &segment.index,
                    &segment.active,
                    segment.level,
                )?);
            }
        }
        drop(compact_source);
        next.segments[number] = Arc::new(compacted);
    }
    Ok((next, CompactionWork::default()))
}


impl Index {
    /// Reclaim retired records without reading source files. One call handles
    /// a bounded batch; `remaining_retired_slots` tells callers when to call
    /// again. Preparation uses a pinned root and retries publication conflicts
    /// without replacing a newer revision.
    pub fn compact(&self) -> Result<CompactionReport, IndexError> {
        self.compact_with_control(|| false, |_, _| {})
    }

    pub(crate) fn compact_with_control(
        &self,
        cancelled: impl Fn() -> bool,
        mut progress: impl FnMut(MaintenancePhase, usize),
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
            let old = &view.0;
            let live = old.active.iter().map(ActiveRecords::len).sum::<u64>()
                + old
                    .segments
                    .iter()
                    .flat_map(|group| group.iter())
                    .map(|segment| segment.active.len())
                    .sum::<u64>();
            let mergeable = old.segments.iter().any(|group| {
                let mut levels = std::collections::BTreeMap::<u32, usize>::new();
                for run in group.iter() {
                    if run.packed.is_some() {
                        *levels.entry(run.level).or_default() += 1;
                    }
                }
                levels.values().any(|count| *count >= 4)
            });
            if before == live && !mergeable {
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
                });
            }
            let (next, work) = compact_published_with_budget(
                old,
                view.revision().next()?,
                &cancelled,
                CompactionBudget::DEFAULT,
            )?;
            if cancelled() {
                return Err(IndexError::Cancelled);
            }
            progress(MaintenancePhase::Publishing, attempt);
            let after = slot_count(&next);
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

/// Merge four oldest runs at the same persisted level and promote the output.
/// Fully active runs participate, so fan-out is bounded even without tombstones.
fn consolidate_same_level_runs(
    runs: &mut Vec<Arc<PublishedSegment>>,
    collection: usize,
    catalog: &FileCatalog,
    definitions: &[crate::repository::CollectionDefinition],
    next_ordinal: &mut u64,
    cancelled: &impl Fn() -> bool,
    budget: &CompactionBudget,
    work: &mut CompactionWork,
) -> Result<(), IndexError> {
    loop {
        if cancelled() {
            return Err(IndexError::Cancelled);
        }
        let mut levels = runs
            .iter()
            .filter_map(|run| run.packed.is_some().then_some(run.level))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .filter_map(|level| {
                let positions = runs
                    .iter()
                    .enumerate()
                    .filter(|(_, run)| run.level == level && run.packed.is_some())
                    .map(|(position, _)| position)
                    .collect::<Vec<_>>();
                (positions.len() >= 4).then_some((level, positions))
            })
            .collect::<Vec<_>>();
        levels.sort_by_key(|(level, _)| *level);
        let Some((level, positions)) = levels.into_iter().next() else {
            break;
        };
        let mut selected = positions
            .iter()
            .copied()
            .map(|position| (position, runs[position].ordinal))
            .collect::<Vec<_>>();
        selected.sort_by_key(|(_, ordinal)| *ordinal);
        selected.truncate(4);
        let records = selected
            .iter()
            .map(|(position, _)| runs[*position].index.record_count() as u64)
            .sum::<u64>();
        let input_bytes = selected
            .iter()
            .map(|(position, _)| source_budget_bytes(&runs[*position].index))
            .sum::<u64>();
        let temporary = input_bytes.saturating_mul(3).saturating_add(records * 512);
        if work.records > 0
            && (work.records.saturating_add(records) > budget.records
                || work.input_bytes.saturating_add(input_bytes) > budget.input_bytes
                || work.temporary_bytes.saturating_add(temporary) > budget.temporary_bytes)
        {
            break;
        }
        let sources = selected
            .iter()
            .map(|(position, _)| {
                runs[*position]
                    .packed
                    .as_deref()
                    .ok_or_else(|| IndexError::InvalidPublication("merge run is not packed".into()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let actives = selected
            .iter()
            .map(|(position, _)| runs[*position].active.bitmap())
            .collect::<Vec<_>>();
        let Some(packed) = crate::index_storage::stream_compact::compact_packed_merge(
            &sources,
            &actives,
            collection as u32,
        )
        .map_err(|error| IndexError::InvalidPublication(error.to_string()))?
        else {
            break;
        };
        work.records = work.records.saturating_add(records);
        work.input_bytes = work.input_bytes.saturating_add(input_bytes);
        work.temporary_bytes = work.temporary_bytes.saturating_add(temporary);
        let packed = Arc::new(packed);
        let ordinal = *next_ordinal;
        *next_ordinal = next_ordinal
            .checked_add(1)
            .ok_or_else(|| IndexError::ResourceLimit("segment ordinal overflow".into()))?;
        let index = crate::index_storage::packed_restore::segment_from_packed(
            &packed,
            collection,
            catalog,
            definitions,
        )?;
        let active = ActiveRecords::all(index.record_count()).map_err(IndexError::ResourceLimit)?;
        let merged = Arc::new(PublishedSegment {
            ordinal,
            level: level.saturating_add(1),
            index: Arc::new(index),
            active,
            packed: Some(Arc::clone(&packed)),
        });
        for position in selected.iter().map(|(position, _)| *position).rev() {
            runs.remove(position);
        }
        runs.push(merged);
    }
    Ok(())
}

/// Return one resident packed object for a published native or packed shard.
pub(crate) fn resident_packed_run(
    segment: &PublishedSegment,
    catalog: &FileCatalog,
    file_versions: &crate::inventory::FileVersions,
    collections: &[crate::repository::CollectionState],
) -> Result<Arc<crate::index_storage::segment::FrozenSegment>, IndexError> {
    if let Some(packed) = &segment.packed {
        return Ok(Arc::clone(packed));
    }
    let input = crate::index_storage::native_pack::pack_segment(
        &segment.index,
        &segment.active.bitmap(),
        file_versions,
        collections,
        catalog,
    )
    .map_err(|error| IndexError::InvalidPublication(error.to_string()))?;
    let built = crate::index_storage::segment_builder::build_segment(input)
        .map_err(|error| IndexError::InvalidPublication(error.to_string()))?;
    let frozen =
        crate::index_storage::segment::FrozenSegment::from_bytes(built.object.into_bytes())
            .map_err(|error| IndexError::InvalidPublication(error.to_string()))?;
    Ok(Arc::new(frozen))
}
