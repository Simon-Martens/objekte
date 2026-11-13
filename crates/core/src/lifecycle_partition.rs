//! Source-free conversion of a static lifecycle base into bounded immutable
//! shards. The static repository is released after the conversion; pinned
//! views continue to own their previous representation.

use std::collections::HashSet;
use std::sync::Arc;

use roaring::RoaringBitmap;

use crate::build::{IndexedCollectionSegment, PartitionRouting};
use crate::inventory::ActiveRecords;
use crate::lifecycle::{IndexError, PublishedIndex, PublishedSegment};
use crate::lifecycle_catalog::FileCatalog;
use crate::lifecycle_compaction::base_as_segment;
use crate::model::FileId;

pub(crate) const BASE_PARTITION_RECORDS: usize = 2_048;
const BASE_PARTITION_FILES: usize = 2_048;

pub(crate) fn partition_base(root: PublishedIndex) -> Result<PublishedIndex, IndexError> {
    partition_base_with_limits(root, BASE_PARTITION_RECORDS, BASE_PARTITION_FILES)
}

/// Split one old shard into active-only parts without opening source files.
/// A multi-record file may own slots in several parts; each part remains a
/// complete, independently queryable contribution container.
pub(crate) fn compact_source_parts(
    source: &IndexedCollectionSegment,
    active: &ActiveRecords,
    catalog: &FileCatalog,
    cancelled: impl FnMut() -> bool,
) -> Result<Vec<IndexedCollectionSegment>, IndexError> {
    compact_source_parts_with_limits(
        source,
        active,
        catalog,
        BASE_PARTITION_RECORDS,
        BASE_PARTITION_FILES,
        cancelled,
    )
}

fn compact_source_parts_with_limits(
    source: &IndexedCollectionSegment,
    active: &ActiveRecords,
    catalog: &FileCatalog,
    max_records: usize,
    max_files: usize,
    mut cancelled: impl FnMut() -> bool,
) -> Result<Vec<IndexedCollectionSegment>, IndexError> {
    if max_records == 0 || max_files == 0 {
        return Err(IndexError::ResourceLimit(
            "compaction part limit must be positive".into(),
        ));
    }
    let mut selections = Vec::<(HashSet<FileId>, ActiveRecords)>::new();
    let mut files = HashSet::<FileId>::new();
    let mut slots = RoaringBitmap::new();
    let mut count = 0usize;
    let seal = |selections: &mut Vec<(HashSet<FileId>, ActiveRecords)>,
                files: &mut HashSet<FileId>,
                slots: &mut RoaringBitmap,
                count: &mut usize|
     -> Result<(), IndexError> {
        if files.is_empty() {
            return Ok(());
        }
        let selected = ActiveRecords::from_bitmap(source.records.len(), std::mem::take(slots))
            .map_err(IndexError::InvalidPublication)?;
        selections.push((std::mem::take(files), selected));
        *count = 0;
        Ok(())
    };
    for file in &source.files {
        if cancelled() {
            return Err(IndexError::Cancelled);
        }
        let Some(current) = catalog.entry(file.entry.id) else {
            continue;
        };
        if current.revision != file.entry.revision
            || current.created_ns != file.entry.created_ns
            || !current.collections.contains(&source.collection)
        {
            continue;
        }
        let owned = file
            .records
            .iter()
            .copied()
            .filter(|slot| active.contains(*slot))
            .collect::<Vec<_>>();
        if owned.is_empty() {
            if files.len() >= max_files {
                seal(&mut selections, &mut files, &mut slots, &mut count)?;
            }
            files.insert(file.entry.id);
            continue;
        }
        let mut offset = 0;
        while offset < owned.len() {
            if count == max_records || (files.len() >= max_files && !files.contains(&file.entry.id))
            {
                seal(&mut selections, &mut files, &mut slots, &mut count)?;
            }
            let end = offset
                .checked_add(max_records - count)
                .ok_or_else(|| IndexError::ResourceLimit("compaction part offset overflow".into()))?
                .min(owned.len());
            files.insert(file.entry.id);
            for slot in &owned[offset..end] {
                slots.insert(slot.0);
            }
            count = count.checked_add(end - offset).ok_or_else(|| {
                IndexError::ResourceLimit("compaction part count overflow".into())
            })?;
            offset = end;
        }
    }
    seal(&mut selections, &mut files, &mut slots, &mut count)?;
    let masks = selections
        .iter()
        .map(|(_, active)| active.clone())
        .collect::<Vec<_>>();
    let routing = PartitionRouting::new(source, &masks).map_err(IndexError::Build)?;
    let projections = source
        .project_postings_for_partitions(&routing)
        .map_err(IndexError::Build)?;
    let mut parts = Vec::with_capacity(selections.len());
    for ((files, _selected), projection) in selections.into_iter().zip(projections) {
        if cancelled() {
            return Err(IndexError::Cancelled);
        }
        let mut compacted = source
            .compact_partition_with_postings(&routing, parts.len(), &files, projection)
            .map_err(IndexError::Build)?;
        for file in &mut compacted.files {
            if let Some(current) = catalog.entry(file.entry.id) {
                file.entry.relative_path = current.relative_path.clone();
            }
        }
        parts.push(compacted);
    }
    Ok(parts)
}

pub(crate) fn partition_base_with_limits(
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
        let mut group = Arc::unwrap_or_clone(groups[number].clone());
        for part in compact_source_parts_with_limits(
            &source,
            &root.active[number],
            &catalog,
            max_records,
            max_files,
            || false,
        )? {
            let ordinal = root.next_segment_ordinal;
            root.next_segment_ordinal = ordinal
                .checked_add(1)
                .ok_or_else(|| IndexError::ResourceLimit("segment ordinal overflow".into()))?;
            let active =
                ActiveRecords::all(part.records.len()).map_err(IndexError::ResourceLimit)?;
            group.push(Arc::new(PublishedSegment {
                ordinal,
                index: Arc::new(part),
                active,
            }));
        }
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
