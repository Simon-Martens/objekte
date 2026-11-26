//! Active-only, source-free maintenance of the published index.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::index::storage::publication::PublishedSegment;
use crate::index::storage::segment::SegmentReader;
use crate::inventory::ActiveRecords;
use crate::lifecycle::maintenance::MaintenancePhase;
use crate::lifecycle::maintenance::compaction::partition::compact_source_parts_with_policy;
use crate::lifecycle::{Index, IndexError, IndexRevision, PublishedIndex};

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

fn source_budget_bytes(source: &dyn crate::index::storage::segment::SegmentReader) -> u64 {
    let estimates = source.estimates();
    u64::try_from(estimates.retained_bytes_estimated)
        .unwrap_or(u64::MAX)
        .max(estimates.source_bytes_estimated)
}

fn slot_count(root: &PublishedIndex) -> u64 {
    root.active
        .iter()
        .map(|active| u64::from(active.slot_count()))
        .sum::<u64>()
        + root
            .record_routes
            .iter()
            .map(|directory| directory.record_counts().slots)
            .sum::<u64>()
}

fn live_count(root: &PublishedIndex) -> u64 {
    root.active.iter().map(ActiveRecords::len).sum::<u64>()
        + root
            .record_routes
            .iter()
            .map(|directory| directory.record_counts().live)
            .sum::<u64>()
}

/// Rebuild one staged publication from its active contributions. The caller
/// chooses the already reserved revision and performs the single CAS publish.
pub(crate) fn compact_published(
    old: &PublishedIndex,
    next_revision: IndexRevision,
) -> Result<PublishedIndex, IndexError> {
    let (next, _) =
        compact_published_with_budget(old, next_revision, || false, CompactionBudget::DEFAULT)?;
    let live = live_count(old);
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
    if old.active.iter().all(|active| active.slot_count() == 0) {
        let started = Instant::now();
        let mut work = CompactionWork::default();
        let mut next = old.clone();
        next.revision = next_revision;
        'collections: for number in 0..old.record_routes.len() {
            if cancelled() {
                return Err(IndexError::Cancelled);
            }
            let counts = old.record_routes[number].record_counts();
            if counts.live == counts.slots {
                continue;
            }
            let group = old.segment_group(number)?;
            let mut compacted = Vec::with_capacity(group.len());
            for (position, segment) in group.iter().enumerate() {
                if cancelled() {
                    return Err(IndexError::Cancelled);
                }
                if segment.active.len() == segment.descriptor().slots as u64 {
                    compacted.push(Arc::clone(segment));
                    continue;
                }
                let records = segment.descriptor().slots as u64;
                let input_bytes = source_budget_bytes(segment.as_ref());
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
                    next.replace_segment_group(number, &compacted)?;
                    break 'collections;
                }
                work.records = work.records.saturating_add(records);
                work.input_bytes = work.input_bytes.saturating_add(input_bytes);
                work.temporary_bytes = work.temporary_bytes.saturating_add(temporary_bytes);
                for part in compact_source_parts_with_policy(
                    segment.as_ref(),
                    &segment.active,
                    &next.catalog,
                    next.repository.options().segment_policy,
                    &cancelled,
                )? {
                    if cancelled() {
                        return Err(IndexError::Cancelled);
                    }
                    let ordinal = next.next_segment_ordinal;
                    next.next_segment_ordinal = ordinal.checked_add(1).ok_or_else(|| {
                        IndexError::ResourceLimit("segment ordinal overflow".into())
                    })?;
                    let active = next.repository.storage_factory.live_records(
                        part.descriptor().slots,
                        crate::index::storage::liveness::InitialLiveness::All,
                    )?;
                    compacted.push(Arc::new(PublishedSegment {
                        id: crate::index::segment::SegmentId::new(ordinal),
                        index: part,
                        active,
                        observation: None,
                    }));
                }
            }
            next.replace_segment_group(number, &compacted)?;
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
    next.catalog = empty.storage_factory.restore_file_catalog(
        &empty.empty_file_catalog_base(),
        old.catalog.as_ref(),
        &cancelled,
    )?;
    next.next_segment_ordinal = old.next_segment_ordinal;
    next.last_size_maintenance_revision = old.last_size_maintenance_revision;
    next.reconciliation = old.reconciliation;
    for (number, collection) in old.repository.collections().iter().enumerate() {
        if cancelled() {
            return Err(IndexError::Cancelled);
        }
        let mut compacted = Vec::new();
        let mut compact_source = |source: &dyn SegmentReader,
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
                let active = next.repository.storage_factory.live_records(
                    part.descriptor().slots,
                    crate::index::storage::liveness::InitialLiveness::All,
                )?;
                replacements.push(Arc::new(PublishedSegment {
                    id: crate::index::segment::SegmentId::new(ordinal),
                    index: part,
                    active,
                    observation: None,
                }));
            }
            Ok(replacements)
        };
        if old.active[number].slot_count() != 0 {
            let base = old
                .repository
                .storage_factory
                .base_segment(collection, old.repository.catalog.as_ref())?;
            compacted.extend(compact_source(base.as_ref(), &old.active[number])?);
        }
        for segment in old.segment_group(number)?.iter() {
            if segment.active.len() == segment.descriptor().slots as u64 {
                compacted.push(Arc::clone(segment));
            } else {
                compacted.extend(compact_source(segment.as_ref(), &segment.active)?);
            }
        }
        drop(compact_source);
        next.replace_segment_group(number, &compacted)?;
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
            let live = live_count(old);
            if before == live {
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

pub mod partition;
#[cfg(test)]
mod tests;
