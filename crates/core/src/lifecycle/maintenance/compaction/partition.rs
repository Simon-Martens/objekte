//! Source-free conversion of a static lifecycle base into bounded immutable
//! shards. The static repository is released after the conversion; pinned
//! views continue to own their previous representation.

use std::collections::HashSet;
#[cfg(test)]
use std::sync::Arc;

use roaring::RoaringBitmap;

use crate::index::segment::IndexedCollectionSegment;
use crate::index::segment::PartitionRouting;
#[cfg(test)]
use crate::index::segment::PublishedSegment;
use crate::index::storage::file_catalog::FileCatalogReader;
use crate::inventory::ActiveRecords;
use crate::lifecycle::IndexError;
#[cfg(test)]
use crate::lifecycle::PublishedIndex;
#[cfg(test)]
use crate::lifecycle::maintenance::compaction::base_as_segment;
use crate::model::FileId;

#[cfg(test)]
pub(crate) const BASE_PARTITION_RECORDS: usize = 2_048;

pub(crate) fn compact_source_parts_with_policy(
    source: &IndexedCollectionSegment,
    active: &ActiveRecords,
    catalog: &dyn FileCatalogReader,
    policy: crate::index::segment::SegmentPolicy,
    cancelled: impl FnMut() -> bool,
) -> Result<Vec<IndexedCollectionSegment>, IndexError> {
    compact_source_parts_with_limits(
        source,
        active,
        catalog,
        policy.target_records.get(),
        policy.max_files.get(),
        cancelled,
    )
}

fn compact_source_parts_with_limits(
    source: &IndexedCollectionSegment,
    active: &ActiveRecords,
    catalog: &dyn FileCatalogReader,
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
    let mut assigned = RoaringBitmap::new();
    let mut count = 0usize;
    let seal = |selections: &mut Vec<(HashSet<FileId>, ActiveRecords)>,
                files: &mut HashSet<FileId>,
                slots: &mut RoaringBitmap,
                count: &mut usize|
     -> Result<(), IndexError> {
        if files.is_empty() && *count == 0 {
            return Ok(());
        }
        let selected = ActiveRecords::from_bitmap(source.record_count(), std::mem::take(slots))
            .map_err(IndexError::InvalidPublication)?;
        selections.push((std::mem::take(files), selected));
        *count = 0;
        Ok(())
    };
    let mut selected_files = HashSet::new();
    for old in 0..source.record_count() {
        if cancelled() {
            return Err(IndexError::Cancelled);
        }
        let slot = crate::model::RecordSlot(old as u32);
        if !active.contains(slot) {
            continue;
        }
        let link = source.backing.file_locator(slot);
        if let Some(link) = link {
            let file = source
                .file_index
                .get(&link.file)
                .and_then(|index| source.files.get(*index))
                .map(|file| &file.entry)
                .ok_or_else(|| {
                    IndexError::InvalidPublication("active record file row missing".into())
                })?;
            if matches!(
                &source.backing,
                crate::index::segment::SegmentBacking::FileRange { .. }
            ) && !catalog.file(link.file)?.is_some_and(|current| {
                current.revision == file.revision
                    && current.created_ns == file.created_ns
                    && current.collections.contains(&source.collection)
            }) {
                return Err(IndexError::InvalidPublication(
                    "active imported record has obsolete file".into(),
                ));
            }
        }
        if count == max_records
            || link.is_some_and(|link| files.len() >= max_files && !files.contains(&link.file))
        {
            seal(&mut selections, &mut files, &mut slots, &mut count)?;
        }
        if let Some(link) = link {
            files.insert(link.file);
            selected_files.insert(link.file);
        }
        if !assigned.insert(slot.0) {
            return Err(IndexError::InvalidPublication(
                "compaction selected an active slot twice".into(),
            ));
        }
        slots.insert(slot.0);
        count += 1;
    }
    // A successfully imported empty file remains in the catalog even when it
    // contributes no live slots to this segment.
    for file in &source.files {
        if selected_files.contains(&file.entry.id) {
            continue;
        }
        let Some(current) = catalog.file(file.entry.id)? else {
            continue;
        };
        if current.revision != file.entry.revision
            || current.created_ns != file.entry.created_ns
            || !current.collections.contains(&source.collection)
        {
            continue;
        }
        if files.len() >= max_files {
            seal(&mut selections, &mut files, &mut slots, &mut count)?;
        }
        files.insert(file.entry.id);
    }
    seal(&mut selections, &mut files, &mut slots, &mut count)?;
    if assigned.len() != active.len() {
        return Err(IndexError::InvalidPublication(
            "compaction did not conserve live slots".into(),
        ));
    }
    let masks = selections
        .iter()
        .map(|(_, active)| active.clone())
        .collect::<Vec<_>>();
    let routing = PartitionRouting::new(source, &masks).map_err(IndexError::Build)?;
    let projections = source
        .project_postings_for_partitions(&routing)
        .map_err(IndexError::Build)?;
    let bigram_projections = source
        .project_bigrams_for_partitions(&routing)
        .map_err(IndexError::Build)?;
    let relation_projections = source
        .project_relations_for_partitions(&routing)
        .map_err(IndexError::Build)?;
    let sorting_projections = source
        .project_sorting_for_partitions(&routing)
        .map_err(IndexError::Build)?;
    let domain_projections = source
        .project_domains_for_partitions(&routing)
        .map_err(IndexError::Build)?;
    let inference_projections = source
        .project_inference_for_partitions(&routing, &domain_projections)
        .map_err(IndexError::Build)?;
    let mut parts = Vec::with_capacity(selections.len());
    for (
        ((((((files, _selected), projection), bigrams), relations), sorting), domains),
        inference,
    ) in selections
        .into_iter()
        .zip(projections)
        .zip(bigram_projections)
        .zip(relation_projections)
        .zip(sorting_projections)
        .zip(domain_projections)
        .zip(inference_projections)
    {
        if cancelled() {
            return Err(IndexError::Cancelled);
        }
        let mut compacted = source
            .compact_partition_with_postings(
                &routing,
                parts.len(),
                &files,
                projection,
                bigrams,
                relations,
                sorting,
                domains,
                inference,
            )
            .map_err(IndexError::Build)?;
        for file in &mut compacted.files {
            if let Some(current) = catalog.file(file.entry.id)? {
                file.entry.relative_path = current.relative_path.clone();
            }
        }
        parts.push(compacted);
    }
    if parts
        .iter()
        .map(IndexedCollectionSegment::record_count)
        .sum::<usize>()
        != active.len() as usize
    {
        return Err(IndexError::InvalidPublication(
            "compaction output live count mismatch".into(),
        ));
    }
    Ok(parts)
}

#[cfg(test)]
pub(crate) fn partition_base_with_limits(
    root: PublishedIndex,
    max_records: usize,
    max_files: usize,
) -> Result<PublishedIndex, IndexError> {
    partition_base_with_limits_inner(root, max_records, max_files)
}

#[cfg(test)]
fn partition_base_with_limits_inner(
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
    let catalog = empty.storage_factory.restore_file_catalog(
        &empty.file_catalog_base(),
        &root.catalog,
        &|| false,
    )?;
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
                id: crate::index::segment::SegmentId::new(ordinal),
                index: Arc::new(part),
                active,
                observation: None,
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
    root.rebuild_all_record_routes()?;
    Ok(root)
}

#[cfg(test)]
mod tests;
