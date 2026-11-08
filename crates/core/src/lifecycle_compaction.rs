//! Active-only, source-free maintenance of the published index.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::build::{IndexedCollectionSegment, SegmentFile};
use crate::inventory::ActiveRecords;
use crate::lifecycle::{Index, IndexError, IndexRevision, PublishedIndex, PublishedSegment};
use crate::lifecycle_catalog::FileCatalog;
use crate::repository::CollectionState;

const MAX_COMPACTION_RECORDS_PER_COLLECTION: usize = 262_144;

/// Dense maintenance result. Old pinned views may keep retired memory until
/// their readers finish; the new root contains only active contributions.
#[derive(Clone, Debug)]
pub struct CompactionReport {
    pub revision: IndexRevision,
    pub attempts: usize,
    pub slots_before: u64,
    pub slots_after: u64,
    pub retired_slots_removed: u64,
    pub pinned_prior_views: usize,
    pub duration: Duration,
}

fn base_as_segment(
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
    Ok(IndexedCollectionSegment {
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
    })
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
                    .map(|segment| segment.index.records.len() as u64)
                    .sum::<u64>()
        })
        .sum()
}

impl Index {
    /// Reclaim retired base and delta records without reading source files.
    /// Preparation runs on a pinned root; a concurrent commit causes a
    /// bounded retry rather than overwriting that newer publication.
    pub fn compact(&self) -> Result<CompactionReport, IndexError> {
        let started = Instant::now();
        let mut conflict = None;
        for attempt in 1..=3 {
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
            if before == live {
                return Ok(CompactionReport {
                    revision: view.revision(),
                    attempts: attempt,
                    slots_before: before,
                    slots_after: before,
                    retired_slots_removed: 0,
                    pinned_prior_views: 0,
                    duration: started.elapsed(),
                });
            }
            let empty = Arc::new(old.repository.empty_like().map_err(IndexError::Build)?);
            let next_revision = view.revision().next()?;
            let mut next = PublishedIndex::from_repository(
                self.instance_id(),
                next_revision,
                Arc::clone(&empty),
            )?;
            if next.configuration_identity != old.configuration_identity {
                return Err(IndexError::IncompatibleConfiguration(
                    "compaction configuration changed".into(),
                ));
            }
            next.catalog = FileCatalog::restored(Arc::clone(&empty), old.catalog.snapshot_slots())
                .map_err(IndexError::InvalidPublication)?;
            next.file_versions = old.file_versions.clone();
            next.next_segment_ordinal = old.next_segment_ordinal;
            next.saved_revision = old.saved_revision;
            next.reconciliation = old.reconciliation;
            for (number, collection) in old.repository.collections().iter().enumerate() {
                let scanned = collection.generation.records.len()
                    + old.segments[number]
                        .iter()
                        .map(|segment| segment.index.records.len())
                        .sum::<usize>();
                if scanned > MAX_COMPACTION_RECORDS_PER_COLLECTION {
                    return Err(IndexError::ResourceLimit(
                        "compaction record budget exceeded".into(),
                    ));
                }
                let mut compacted = Vec::new();
                let mut compact_source = |source: &IndexedCollectionSegment,
                                          active: &ActiveRecords|
                 -> Result<(), IndexError> {
                    let dense = source
                        .compact_active(active, MAX_COMPACTION_RECORDS_PER_COLLECTION, |entry| {
                            next.catalog.entry(entry.id).is_some_and(|current| {
                                current.relative_path == entry.relative_path
                                    && current.revision == entry.revision
                                    && current.created_ns == entry.created_ns
                                    && current.collections.contains(&source.collection)
                            })
                        })
                        .map_err(IndexError::Build)?;
                    if dense.index.records.is_empty() {
                        return Ok(());
                    }
                    let ordinal = next.next_segment_ordinal;
                    next.next_segment_ordinal = ordinal.checked_add(1).ok_or_else(|| {
                        IndexError::ResourceLimit("segment ordinal overflow".into())
                    })?;
                    compacted.push(Arc::new(PublishedSegment {
                        ordinal,
                        index: Arc::new(dense.index),
                        active: dense.active,
                    }));
                    Ok(())
                };
                if !collection.generation.records.is_empty() {
                    let base = base_as_segment(collection, old.repository.files())?;
                    compact_source(&base, &old.active[number])?;
                }
                for segment in old.segments[number].iter() {
                    compact_source(&segment.index, &segment.active)?;
                }
                drop(compact_source);
                next.segments[number] = Arc::new(compacted);
                while next.segments[number].len() > super::lifecycle::MAX_SEGMENTS_PER_COLLECTION {
                    next.make_room_for_segment(number)?;
                }
            }
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
