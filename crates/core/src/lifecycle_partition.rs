//! Source-free conversion of a static lifecycle base into bounded immutable
//! shards. The static repository is released after the conversion; pinned
//! views continue to own their previous representation.

use std::collections::HashSet;
use std::sync::Arc;

use roaring::RoaringBitmap;

use crate::inventory::ActiveRecords;
use crate::lifecycle::{IndexError, PublishedIndex, PublishedSegment};
use crate::lifecycle_catalog::FileCatalog;
use crate::lifecycle_compaction::base_as_segment;
use crate::model::FileId;

pub(crate) const BASE_PARTITION_RECORDS: usize = 16_384;
const BASE_PARTITION_FILES: usize = 16_384;

pub(crate) fn partition_base(root: PublishedIndex) -> Result<PublishedIndex, IndexError> {
    partition_base_with_limits(root, BASE_PARTITION_RECORDS, BASE_PARTITION_FILES)
}

fn partition_base_with_limits(
    mut root: PublishedIndex,
    max_records: usize,
    max_files: usize,
) -> Result<PublishedIndex, IndexError> {
    if max_records == 0 || max_files == 0 {
        return Err(IndexError::ResourceLimit(
            "base partition limit must be positive".into(),
        ));
    }
    if root
        .repository
        .collections()
        .iter()
        .all(|collection| collection.generation.files.is_empty())
    {
        return Ok(root);
    }
    let empty = Arc::new(root.repository.empty_like().map_err(IndexError::Build)?);
    let slots = root.catalog.snapshot_slots();
    let catalog =
        FileCatalog::restored(Arc::clone(&empty), slots).map_err(IndexError::InvalidPublication)?;
    let mut groups = root.segments.clone();
    for (number, collection) in root.repository.collections().iter().enumerate() {
        if collection.generation.files.is_empty() {
            continue;
        }
        let source = base_as_segment(collection, root.repository.files())?;
        let mut selected = Vec::<FileId>::new();
        let mut records = 0usize;
        let mut group = Arc::unwrap_or_clone(groups[number].clone());
        let seal = |selected: &mut Vec<FileId>,
                    records: &mut usize,
                    group: &mut Vec<Arc<PublishedSegment>>,
                    ordinal: &mut u64|
         -> Result<(), IndexError> {
            if selected.is_empty() {
                return Ok(());
            }
            let files = selected.drain(..).collect::<HashSet<_>>();
            let mut bitmap = RoaringBitmap::new();
            for file in &files {
                if let Some(owned) = collection.generation.file_records.records_for(*file) {
                    for slot in owned {
                        if root.active[number].contains(*slot) {
                            bitmap.insert(slot.0);
                        }
                    }
                }
            }
            let active = ActiveRecords::from_bitmap(source.records.len(), bitmap)
                .map_err(IndexError::InvalidPublication)?;
            let compacted = source
                .compact_active(&active, source.records.len(), |entry| {
                    files.contains(&entry.id)
                })
                .map_err(IndexError::Build)?;
            let assigned = *ordinal;
            *ordinal = ordinal
                .checked_add(1)
                .ok_or_else(|| IndexError::ResourceLimit("segment ordinal overflow".into()))?;
            group.push(Arc::new(PublishedSegment {
                ordinal: assigned,
                index: Arc::new(compacted.index),
                active: compacted.active,
            }));
            *records = 0;
            Ok(())
        };
        for file in &collection.generation.files {
            let old = root
                .repository
                .files()
                .files
                .get(file.0 as usize)
                .ok_or_else(|| IndexError::InvalidPublication("base file missing".into()))?;
            let Some(current) = catalog.entry(*file) else {
                continue;
            };
            if current.revision != old.revision
                || current.created_ns != old.created_ns
                || !current
                    .collections
                    .contains(&collection.generation.collection)
            {
                continue;
            }
            let count = collection
                .generation
                .file_records
                .records_for(*file)
                .ok_or_else(|| {
                    IndexError::InvalidPublication("base file ownership missing".into())
                })?
                .iter()
                .filter(|slot| root.active[number].contains(**slot))
                .count();
            if !selected.is_empty()
                && (selected.len() >= max_files || records.saturating_add(count) > max_records)
            {
                seal(
                    &mut selected,
                    &mut records,
                    &mut group,
                    &mut root.next_segment_ordinal,
                )?;
            }
            selected.push(*file);
            records = records.checked_add(count).ok_or_else(|| {
                IndexError::ResourceLimit("partition record count overflow".into())
            })?;
        }
        seal(
            &mut selected,
            &mut records,
            &mut group,
            &mut root.next_segment_ordinal,
        )?;
        groups[number] = Arc::new(group);
    }
    root.repository = empty;
    root.catalog = catalog;
    root.active = root
        .active
        .iter()
        .map(|_| ActiveRecords::all(0).map_err(IndexError::ResourceLimit))
        .collect::<Result<Vec<_>, _>>()?;
    root.segments = groups;
    Ok(root)
}

#[cfg(test)]
#[path = "lifecycle_partition.test.rs"]
mod tests;
