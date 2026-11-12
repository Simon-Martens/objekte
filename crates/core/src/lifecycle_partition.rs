//! Source-free conversion of a static lifecycle base into bounded immutable
//! shards. The static repository is released after the conversion; pinned
//! views continue to own their previous representation.

use std::collections::HashSet;
use std::sync::Arc;

use roaring::RoaringBitmap;

use crate::build::IndexedCollectionSegment;
use crate::inventory::{
    ActiveRecords, CollectionBuilder, FileEntry, FileInventory, FileStatus, RecordClass,
    SourceRevision,
};
use crate::lifecycle::{IndexError, PublishedIndex, PublishedSegment};
use crate::lifecycle_catalog::FileCatalog;
use crate::lifecycle_compaction::base_as_segment;
use crate::model::{CollectionId, FileId};
use crate::repository::CollectionStats;

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
    let projections = source
        .project_postings_for_partitions(&masks)
        .map_err(IndexError::Build)?;
    let mut parts = Vec::with_capacity(selections.len());
    for ((files, selected), projection) in selections.into_iter().zip(projections) {
        if cancelled() {
            return Err(IndexError::Cancelled);
        }
        let mut compacted = source
            .compact_active_with_postings(
                &selected,
                source.records.len(),
                |entry| files.contains(&entry.id),
                projection,
            )
            .map_err(IndexError::Build)?;
        for file in &mut compacted.index.files {
            if let Some(current) = catalog.entry(file.entry.id) {
                file.entry.relative_path = current.relative_path.clone();
            }
        }
        parts.push(compacted.index);
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

/// Reconstitute a searchable base from live stored segments for an explicit
/// later-inference request. All existing text and configured index structures
/// come from the snapshot; no source file is opened by this conversion.
pub(crate) fn unpartition_for_inference(
    old: &PublishedIndex,
) -> Result<PublishedIndex, IndexError> {
    if old.segments.iter().all(|group| group.is_empty()) {
        return Ok(old.clone());
    }
    if old
        .repository
        .collections()
        .iter()
        .any(|state| !state.generation.records.is_empty())
        || old.extended_batches.iter().any(|group| !group.is_empty())
    {
        return Err(IndexError::IncompatibleConfiguration(
            "later inference cannot reconstitute mixed base and segment coverage".into(),
        ));
    }
    let mut slots = old.catalog.snapshot_slots();
    let mut inventory = FileInventory::new(old.repository.files().root.clone());
    let active_paths = slots
        .iter()
        .flatten()
        .map(|(entry, _, _)| entry.relative_path.clone())
        .collect::<HashSet<_>>();
    for (number, slot) in slots.iter().enumerate() {
        let entry = if let Some((entry, _, _)) = slot {
            entry.clone()
        } else {
            let mut suffix = 0usize;
            let relative_path = loop {
                let path = std::path::PathBuf::from(format!(
                    ".objekte/retired-index-file/{number}-{suffix}"
                ));
                if !active_paths.contains(&path) {
                    break path;
                }
                suffix += 1;
            };
            FileEntry {
                id: FileId(number as u32),
                relative_path,
                revision: SourceRevision {
                    modified_ns: 0,
                    size: 0,
                },
                created_ns: None,
                collections: vec![CollectionId(0)],
                status: FileStatus::Valid,
                diagnostics: Vec::new(),
                omitted_diagnostics: 0,
            }
        };
        inventory.files.push(entry);
    }
    inventory
        .rebuild_lookup()
        .map_err(IndexError::InvalidPublication)?;
    let mut repository = old.repository.empty_like().map_err(IndexError::Build)?;
    repository.base_fingerprints = slots
        .iter()
        .map(|slot| slot.as_ref().and_then(|(_, fingerprint, _)| *fingerprint))
        .collect();
    let mut pending_relations = Vec::with_capacity(repository.collections.len());
    for (number, group) in old.segments.iter().enumerate() {
        let collection = CollectionId(number as u32);
        let mut parts = Vec::new();
        for segment in group.iter() {
            parts.extend(compact_source_parts(
                &segment.index,
                &segment.active,
                &old.catalog,
                || false,
            )?);
        }
        let mut builder = CollectionBuilder::new(
            collection,
            repository.collections[number].definition.name.clone(),
            1,
            inventory.files.len(),
        );
        for (file, slot) in slots.iter().enumerate() {
            if slot
                .as_ref()
                .is_some_and(|(entry, _, _)| entry.collections.contains(&collection))
            {
                builder
                    .include_file(FileId(file as u32))
                    .map_err(IndexError::Build)?;
            }
        }
        if parts.is_empty() {
            repository.collections[number].generation =
                builder.finish(&inventory).map_err(IndexError::Build)?;
            pending_relations.push(Vec::new());
            continue;
        }
        let references = parts.iter().collect::<Vec<_>>();
        let combined = IndexedCollectionSegment::consolidate(&references, u32::MAX as usize)
            .map_err(IndexError::Build)?
            .index;
        let file_entries = combined
            .files
            .iter()
            .map(|file| (file.entry.id, &file.entry))
            .collect::<std::collections::HashMap<_, _>>();
        for file in &combined.files {
            let target = &mut inventory.files[file.entry.id.0 as usize];
            target.status = target.status.max(file.entry.status);
            target.omitted_diagnostics = target
                .omitted_diagnostics
                .max(file.entry.omitted_diagnostics);
        }
        for record in &combined.records {
            let entry = &inventory.files[record.file.0 as usize];
            let slot = match record.class {
                RecordClass::Logical => builder.add_record(
                    entry,
                    record.id.clone(),
                    record.range.start,
                    record.range.end,
                ),
                RecordClass::RawFallback => {
                    let diagnostic_ref = if let Some(index) = record.diagnostic_ref.stored_index() {
                        let source = file_entries
                            .get(&record.file)
                            .ok_or_else(|| IndexError::Build("segment file missing".into()))?;
                        let mut diagnostic = source
                            .diagnostics
                            .get(index)
                            .ok_or_else(|| IndexError::Build("segment diagnostic missing".into()))?
                            .clone();
                        diagnostic.record = Some(record.slot);
                        inventory.files[record.file.0 as usize]
                            .record_diagnostic(diagnostic)
                            .map_err(IndexError::Build)?
                    } else {
                        record.diagnostic_ref
                    };
                    builder.add_raw_fallback(
                        &inventory.files[record.file.0 as usize],
                        record.id.clone(),
                        record.range,
                        diagnostic_ref,
                    )
                }
            }
            .map_err(IndexError::Build)?;
            if slot != record.slot {
                return Err(IndexError::InvalidPublication(
                    "segment record order changed during base reconstruction".into(),
                ));
            }
        }
        let generation = builder.finish(&inventory).map_err(IndexError::Build)?;
        let state = &mut repository.collections[number];
        state.stats = CollectionStats {
            file_count: generation.files.len(),
            record_count: generation.records.len(),
            total_file_bytes: generation
                .files
                .iter()
                .map(|file| inventory.files[file.0 as usize].revision.size)
                .sum(),
        };
        state.generation = generation;
        state.postings = combined.postings;
        state.inference = combined.inference;
        state.custom_postings = combined.custom_postings;
        state.raw = combined.raw;
        state.fallback_text = combined.fallback_text;
        state.logical_records = combined.logical_records;
        state.empty_array_records = combined.empty_array_records;
        state.custom_text = combined.custom_text;
        state.sorting = combined.sorting;
        state.id_lookup = combined.id_lookup;
        state.identity_order = combined.identity_order;
        pending_relations.push(combined.relations);
    }
    for (number, relations) in pending_relations.into_iter().enumerate() {
        if relations.is_empty() {
            continue;
        }
        let resolved = relations
            .into_iter()
            .enumerate()
            .map(|(ordinal, relation)| {
                relation.resolve(
                    CollectionId(number as u32),
                    ordinal as u32,
                    &repository.collections,
                )
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(IndexError::Build)?;
        repository.collections[number].relations = resolved;
    }
    repository.files = inventory;
    crate::relation::validate_indexes(&repository.collections).map_err(IndexError::Build)?;
    for (number, slot) in slots.iter_mut().enumerate() {
        if let Some((entry, _, _)) = slot {
            *entry = repository.files.files[number].clone();
        }
    }
    let repository = Arc::new(repository);
    let catalog = FileCatalog::restored(Arc::clone(&repository), slots)
        .map_err(IndexError::InvalidPublication)?;
    let mut next = old.clone();
    next.active = repository
        .collections()
        .iter()
        .map(|state| ActiveRecords::all(state.generation.records.len()).map_err(IndexError::Build))
        .collect::<Result<Vec<_>, _>>()?;
    next.segments = repository
        .collections()
        .iter()
        .map(|_| Arc::new(Vec::new()))
        .collect();
    next.repository = repository;
    next.catalog = catalog;
    next.capability_revision = next.capability_revision.next()?;
    next.saved_capability_revision = None;
    Ok(next)
}

#[cfg(test)]
#[path = "lifecycle_partition.test.rs"]
mod tests;
