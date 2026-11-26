//! Source-free conversion of a static lifecycle base into bounded immutable
//! shards. The static repository is released after the conversion; pinned
//! views continue to own their previous representation.

use std::collections::HashSet;
#[cfg(test)]
use std::sync::Arc;

use roaring::RoaringBitmap;

use crate::index::storage::StorageFactory;
use crate::index::storage::file_catalog::FileCatalogReader;
#[cfg(test)]
use crate::index::storage::publication::PublishedSegment;
use crate::index::storage::segment::maintenance::{SegmentPartition, check_projection_source};
use crate::index::storage::segment::transfer::{RecordLayout, visit_record_rows};
use crate::index::storage::segment::{
    RecordBacking, SegmentHandle, SegmentReader, visit_segment_files,
};
use crate::index::storage::transfer::StorageError;
use crate::inventory::ActiveRecords;
use crate::lifecycle::IndexError;
#[cfg(test)]
use crate::lifecycle::PublishedIndex;
use crate::model::FileId;

#[cfg(test)]
pub(crate) const BASE_PARTITION_RECORDS: usize = 2_048;

pub(crate) fn compact_source_parts_with_policy(
    source: &dyn SegmentReader,
    active: &ActiveRecords,
    catalog: &dyn FileCatalogReader,
    target: &dyn StorageFactory,
    policy: crate::index::segment::SegmentPolicy,
    cancelled: impl FnMut() -> bool,
) -> Result<Vec<SegmentHandle>, IndexError> {
    compact_source_parts_with_limits(
        source,
        active,
        catalog,
        target,
        policy.target_records.get(),
        policy.max_files.get(),
        cancelled,
    )
}

fn compact_source_parts_with_limits(
    source: &dyn SegmentReader,
    active: &ActiveRecords,
    catalog: &dyn FileCatalogReader,
    target: &dyn StorageFactory,
    max_records: usize,
    max_files: usize,
    mut cancelled: impl FnMut() -> bool,
) -> Result<Vec<SegmentHandle>, IndexError> {
    if max_records == 0 || max_files == 0 {
        return Err(IndexError::ResourceLimit(
            "compaction part limit must be positive".into(),
        ));
    }
    let selections = plan_partitions(
        source,
        active,
        catalog,
        max_records,
        max_files,
        &mut cancelled,
    )?;
    let parts = target.project_segment_partitions(source, &selections, catalog, &mut cancelled)?;
    if parts.len() != selections.len() {
        return Err(IndexError::InvalidPublication(
            "compaction output partition count mismatch".into(),
        ));
    }
    for (part, partition) in parts.iter().zip(&selections) {
        check_projection_source(source, part.as_ref(), partition.slots.len() as usize)?;
        if part.file_count() != partition.files.len() {
            return Err(IndexError::InvalidPublication(
                "compaction output file count mismatch".into(),
            ));
        }
    }
    Ok(parts)
}

fn plan_partitions(
    source: &dyn SegmentReader,
    active: &ActiveRecords,
    catalog: &dyn FileCatalogReader,
    max_records: usize,
    max_files: usize,
    mut cancelled: impl FnMut() -> bool,
) -> Result<Vec<SegmentPartition>, IndexError> {
    let descriptor = source.descriptor();
    let by_range = source.backing_kind() == RecordBacking::FileRange;
    let mut selections = Vec::<SegmentPartition>::new();
    let mut files = HashSet::<FileId>::new();
    let mut slots = RoaringBitmap::new();
    let mut assigned = RoaringBitmap::new();
    let mut count = 0usize;
    let seal = |selections: &mut Vec<SegmentPartition>,
                files: &mut HashSet<FileId>,
                slots: &mut RoaringBitmap,
                count: &mut usize|
     -> Result<(), IndexError> {
        if files.is_empty() && *count == 0 {
            return Ok(());
        }
        let selected = ActiveRecords::from_bitmap(descriptor.slots, std::mem::take(slots))
            .map_err(IndexError::InvalidPublication)?;
        selections.push(SegmentPartition {
            files: std::mem::take(files),
            slots: selected,
        });
        *count = 0;
        Ok(())
    };
    let mut selected_files = HashSet::new();
    let layout = if active.len() == u64::from(active.slot_count()) {
        RecordLayout::Identity
    } else {
        RecordLayout::Dense
    };
    visit_record_rows(source, active, layout, &mut cancelled, &mut |records| {
        const OP: &str = "plan projected segment records";
        for record in records.iter().filter(|record| record.live) {
            let slot = record.header.slot;
            let link = record.origin.file();
            if let Some(link) = link {
                let file = source
                    .file_metadata(link.file)?
                    .ok_or_else(|| StorageError::invalid(OP, "active record file row missing"))?;
                if by_range
                    && !catalog.file(link.file)?.is_some_and(|current| {
                        current.revision == file.revision
                            && current.created_ns == file.created_ns
                            && current.collections.contains(&descriptor.collection)
                    })
                {
                    return Err(StorageError::invalid(
                        OP,
                        "active imported record has obsolete file",
                    ));
                }
            }
            if count == max_records
                || link.is_some_and(|link| files.len() >= max_files && !files.contains(&link.file))
            {
                seal(&mut selections, &mut files, &mut slots, &mut count)
                    .map_err(|error| StorageError::invalid(OP, error.to_string()))?;
            }
            if let Some(link) = link {
                files.insert(link.file);
                selected_files.insert(link.file);
            }
            if u64::from(record.destination.0) != assigned.len() || !assigned.insert(slot.0) {
                return Err(StorageError::invalid(
                    OP,
                    "compaction transfer selected an invalid slot",
                ));
            }
            slots.insert(slot.0);
            count += 1;
        }
        Ok(())
    })?;
    // A successfully imported empty file remains in the catalog even when it
    // contributes no live slots to this segment.
    visit_segment_files(source, |file| {
        if cancelled() {
            return Err(IndexError::Cancelled);
        }
        if selected_files.contains(&file.id) {
            return Ok(());
        }
        let Some(current) = catalog.file(file.id)? else {
            return Ok(());
        };
        if current.revision != file.revision
            || current.created_ns != file.created_ns
            || !current.collections.contains(&descriptor.collection)
        {
            return Ok(());
        }
        if files.len() >= max_files {
            seal(&mut selections, &mut files, &mut slots, &mut count)?;
        }
        files.insert(file.id);
        Ok(())
    })?;
    seal(&mut selections, &mut files, &mut slots, &mut count)?;
    if assigned.len() != active.len() {
        return Err(IndexError::InvalidPublication(
            "compaction did not conserve live slots".into(),
        ));
    }
    Ok(selections)
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
        &empty.empty_file_catalog_base(),
        root.catalog.as_ref(),
        &|| false,
    )?;
    let mut groups = root.segment_groups()?;
    for (number, collection) in root.repository.collections().iter().enumerate() {
        if collection.generation.files.is_empty() {
            continue;
        }
        let source = root
            .repository
            .storage_factory
            .base_segment(collection, root.repository.catalog.as_ref())?;
        let inactive_base;
        let active = if root.active[number].slot_count() == 0 {
            // Finalization fixtures can keep dormant base rows solely for
            // compatibility inspection, with all live routes on segments.
            inactive_base =
                ActiveRecords::from_bitmap(source.descriptor().slots, Default::default())
                    .map_err(IndexError::InvalidPublication)?;
            &inactive_base
        } else {
            &root.active[number]
        };
        let mut group = Arc::unwrap_or_clone(groups[number].clone());
        for part in compact_source_parts_with_limits(
            &source,
            active,
            &catalog,
            empty.storage_factory.as_ref(),
            max_records,
            max_files,
            || false,
        )? {
            let ordinal = root.next_segment_ordinal;
            root.next_segment_ordinal = ordinal
                .checked_add(1)
                .ok_or_else(|| IndexError::ResourceLimit("segment ordinal overflow".into()))?;
            let active = empty.storage_factory.live_records(
                part.descriptor().slots,
                crate::index::storage::liveness::InitialLiveness::All,
            )?;
            group.push(Arc::new(PublishedSegment {
                version: crate::index::storage::publication::PlacementVersion::new(),
                id: crate::index::segment::SegmentId::new(ordinal),
                index: part,
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
        .map(|_| ActiveRecords::empty())
        .collect::<Vec<_>>();
    for (number, group) in groups.iter().enumerate() {
        root.replace_segment_group(number, group)?;
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    super::super::tests::partition_tests!();
}
