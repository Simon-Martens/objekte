//! Verified old-content admission and physical removal in an unpublished root.

use crate::index::storage::file_catalog::FileCatalogReader;
use std::collections::BTreeSet;
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::construction::batch::SourceFileInput;
use crate::construction::file_preparation::{
    FilePreparationBinding, FileRecordLayout, reconstruct_file_layout,
};
use crate::index::segment::PublishedSegment;
use crate::index::storage::segment::{
    SegmentReader, visit_checked_file_slots, visit_segment_files,
};
use crate::lifecycle::mutation::VerifiedFallbackReason;
use crate::lifecycle::{FileHandle, IndexError, IndexView, PublishedIndex};
use crate::model::{CollectionId, FileId, RecordSlot};

const MAX_VERIFIED_SCANNED_RECORDS: usize = 16_384;

/// Old segment ordinals are captured before staging. Staging may consolidate
/// them, so removal scans the affected collections again in the private root.
pub(crate) struct VerifiedPlan {
    old_shards: Vec<(CollectionId, u64)>,
    has_active_base: bool,
}

impl VerifiedPlan {
    pub(crate) fn requires_base_compaction(&self) -> bool {
        self.has_active_base
    }
}

pub(super) enum VerificationError {
    Fallback(VerifiedFallbackReason),
    Storage(IndexError),
}

impl From<VerifiedFallbackReason> for VerificationError {
    fn from(reason: VerifiedFallbackReason) -> Self {
        Self::Fallback(reason)
    }
}

impl From<IndexError> for VerificationError {
    fn from(error: IndexError) -> Self {
        Self::Storage(error)
    }
}

impl From<crate::index::storage::transfer::StorageError> for VerificationError {
    fn from(error: crate::index::storage::transfer::StorageError) -> Self {
        Self::Storage(error.into())
    }
}

pub(super) trait VerificationResult<T> {
    fn into_fallback(self) -> Result<Result<T, VerifiedFallbackReason>, IndexError>;
}

impl<T> VerificationResult<T> for Result<T, VerificationError> {
    fn into_fallback(self) -> Result<Result<T, VerifiedFallbackReason>, IndexError> {
        match self {
            Ok(value) => Ok(Ok(value)),
            Err(VerificationError::Fallback(reason)) => Ok(Err(reason)),
            Err(VerificationError::Storage(error)) => Err(error),
        }
    }
}

pub(crate) fn verify_old_content(
    view: &IndexView,
    handle: FileHandle,
    old_bytes: &[u8],
) -> Result<VerifiedPlan, VerificationError> {
    verify_old_content_with_layout(view, handle, old_bytes, &mut None)
}

pub(super) fn verify_old_content_with_layout(
    view: &IndexView,
    handle: FileHandle,
    old_bytes: &[u8],
    old_layout: &mut Option<FileRecordLayout>,
) -> Result<VerifiedPlan, VerificationError> {
    let old = view
        .file_entry(handle.file_id)?
        .ok_or(VerifiedFallbackReason::IndexedLayoutChanged)?;
    let fingerprint = view
        .file_fingerprint(handle.file_id)?
        .ok_or(VerifiedFallbackReason::MissingFingerprint)?;
    if view.file_configuration(handle.file_id)? != Some(view.configuration_identity()) {
        return Err(VerifiedFallbackReason::ConfigurationChanged.into());
    }
    let repository = view.repository();
    let mut bindings = Vec::with_capacity(old.collections.len());
    for collection_id in &old.collections {
        let collection = repository
            .collections()
            .get(collection_id.0 as usize)
            .ok_or(VerifiedFallbackReason::IndexedLayoutChanged)?;
        if !collection
            .required_file_reader()
            .reader()
            .supports_verified_layout()
        {
            return Err(VerifiedFallbackReason::UnsupportedReader.into());
        }
        bindings.push(FilePreparationBinding {
            factory: &view.repository().storage_factory,
            collection: *collection_id,
            definition: collection.required_file_definition(),
            source: &collection.required_file_reader(),
        });
    }
    let input = SourceFileInput {
        id: handle.file_id,
        relative_path: old.relative_path.clone(),
        revision: old.revision,
        created_ns: old.created_ns,
    };
    let mut current = Vec::new();
    let mut old_shards = Vec::new();
    let mut scanned = 0usize;
    let mut base_owned = false;
    let mut has_active_base = false;
    for collection_id in &old.collections {
        let number = collection_id.0 as usize;
        let collection = repository
            .collections()
            .get(number)
            .ok_or(VerifiedFallbackReason::IndexedLayoutChanged)?;
        if collection.generation.files.contains(&handle.file_id) {
            base_owned = true;
        }
        if let Some(slots) = collection
            .generation
            .file_records
            .records_for(handle.file_id)
        {
            for slot in slots.iter() {
                if view.0.active[number].contains(*slot) {
                    let record = collection
                        .generation
                        .records
                        .get(slot.0 as usize)
                        .ok_or(VerifiedFallbackReason::IndexedLayoutChanged)?;
                    current.push((
                        *collection_id,
                        record.id.clone(),
                        record.range,
                        record.class,
                    ));
                    has_active_base = true;
                }
            }
        }
        let routes = view
            .0
            .record_routes
            .get(number)
            .ok_or(VerifiedFallbackReason::IndexedLayoutChanged)?;
        let shards = view
            .segments(*collection_id)
            .ok_or(VerifiedFallbackReason::IndexedLayoutChanged)?;
        for id in routes.segments_for_file(handle.file_id)?.iter() {
            let shard = routes
                .position(*id)?
                .and_then(|position| shards.get(position))
                .filter(|segment| segment.id == *id)
                .ok_or(VerifiedFallbackReason::IndexedLayoutChanged)?;
            scanned = scanned.saturating_add(shard.descriptor().slots);
            if scanned > MAX_VERIFIED_SCANNED_RECORDS {
                return Err(VerifiedFallbackReason::CompactionBudget.into());
            }
            if !shard.contains_file(handle.file_id)? {
                continue;
            }
            old_shards.push((*collection_id, shard.id.get()));
            let mut layout_known = true;
            visit_checked_file_slots(shard, handle.file_id, &mut |slots| {
                for &slot in slots {
                    if !shard.active.contains(slot) {
                        continue;
                    }
                    let header = shard.header(slot)?;
                    let range = shard
                        .origin(slot)?
                        .and_then(|origin| origin.file())
                        .filter(|locator| locator.file == handle.file_id)
                        .and_then(|locator| locator.range);
                    if let (Some(header), Some(range)) = (header, range) {
                        layout_known &= header.slot == slot;
                        current.push((*collection_id, header.id.clone(), range, header.class));
                    } else {
                        layout_known = false;
                    }
                }
                Ok(())
            })?;
            if !layout_known {
                return Err(VerifiedFallbackReason::IndexedLayoutChanged.into());
            }
        }
    }
    // An empty base file still owns its current file incarnation, even though
    // it has no active record slot to prove that ownership through.
    if old_shards.is_empty() && base_owned {
        has_active_base = true;
    }
    if old_shards.is_empty() && !has_active_base {
        return Err(VerifiedFallbackReason::BaseIndexUnsupported.into());
    }
    if current.len() as u64
        != view
            .file_record_count(handle.file_id)?
            .ok_or(VerifiedFallbackReason::IndexedLayoutChanged)?
    {
        return Err(VerifiedFallbackReason::IndexedLayoutChanged.into());
    }
    if current.len() > MAX_VERIFIED_SCANNED_RECORDS {
        return Err(VerifiedFallbackReason::CompactionBudget.into());
    }
    let sort_layout = |layout: &mut FileRecordLayout| {
        layout.sort_unstable_by(|left, right| {
            (left.0, left.2.start, left.2.end, &left.1).cmp(&(
                right.0,
                right.2.start,
                right.2.end,
                &right.1,
            ))
        });
    };
    let reconstructed = match old_layout {
        Some(layout) => layout,
        None => {
            let supplied_fingerprint: [u8; 32] = Sha256::digest(old_bytes).into();
            if supplied_fingerprint != fingerprint {
                return Err(VerifiedFallbackReason::ContentMismatch.into());
            }
            #[cfg(test)]
            super::tests::reach_file_preparation_checkpoint("old-layout");
            let mut layout = reconstruct_file_layout(
                repository.required_file_root(),
                &input,
                old_bytes,
                &bindings,
            )
            .map_err(|_| VerifiedFallbackReason::ReconstructionFailed)?;
            sort_layout(&mut layout);
            old_layout.insert(layout)
        }
    };
    sort_layout(&mut current);
    if &current != reconstructed {
        return Err(VerifiedFallbackReason::IndexedLayoutChanged.into());
    }
    Ok(VerifiedPlan {
        old_shards,
        has_active_base,
    })
}

/// Replace a single-file, single-record shard with an independently built
/// replacement while retaining its ordinal and local slot zero in the new
/// published revision. The old shard Arc remains in every pinned old view.
/// Any uncertainty leaves `next` intact for verified physical removal.
pub(crate) fn preserve_single_record_slots(
    view: &IndexView,
    next: &PublishedIndex,
    plan: &VerifiedPlan,
    file: FileId,
) -> Result<Option<PublishedIndex>, IndexError> {
    if plan.has_active_base {
        return Ok(None);
    }
    let (Some(old_entry), Some(new_entry)) = (view.file_entry(file)?, next.catalog.file(file)?)
    else {
        return Ok(None);
    };
    let preserve = || -> Option<PublishedIndex> {
        if old_entry.collections != new_entry.collections || old_entry.collections.is_empty() {
            return None;
        }
        let mut replacements = Vec::with_capacity(old_entry.collections.len());
        for collection in &old_entry.collections {
            let mut ordinals = plan
                .old_shards
                .iter()
                .filter(|(id, _)| id == collection)
                .map(|(_, ordinal)| *ordinal);
            let old_ordinal = ordinals.next()?;
            if ordinals.next().is_some() {
                return None;
            }
            let old = view
                .segments(*collection)?
                .iter()
                .find(|shard| shard.id.get() == old_ordinal)?;
            if old.index.file_count() != 1
                || old.index.record_count() != 1
                || old.active.len() != 1
                || !old.active.contains(RecordSlot(0))
                || old.index.file_records(file)? != [RecordSlot(0)]
            {
                return None;
            }
            let group = next.segments.get(collection.0 as usize)?;
            let old_position = group
                .iter()
                .position(|shard| shard.id.get() == old_ordinal)?;
            if group[old_position].active.len() != 0 {
                return None;
            }
            let new_position = group
                .iter()
                .enumerate()
                .filter(|(_, shard)| shard.index.file(file).is_some())
                .max_by_key(|(_, shard)| shard.id)?
                .0;
            if new_position == old_position {
                return None;
            }
            let new = &group[new_position];
            if new.index.file_count() != 1
                || new.index.record_count() != 1
                || new.active.len() != 1
                || !new.active.contains(RecordSlot(0))
                || new.index.file_records(file)? != [RecordSlot(0)]
                || old.index.source_id != new.index.source_id
                || old.index.source_version != new.index.source_version
                || old.index.record(RecordSlot(0))?.id != new.index.record(RecordSlot(0))?.id
                || old.index.record(RecordSlot(0))?.class != new.index.record(RecordSlot(0))?.class
            {
                return None;
            }
            replacements.push((*collection, old_position, new_position, old_ordinal));
        }
        let mut candidate = next.clone();
        for (collection, old_position, new_position, old_ordinal) in replacements {
            let group = Arc::make_mut(candidate.segments.get_mut(collection.0 as usize)?);
            let replacement = Arc::clone(&group[new_position]);
            group[old_position] = Arc::new(PublishedSegment {
                id: crate::index::segment::SegmentId::new(old_ordinal),
                index: Arc::clone(&replacement.index),
                active: replacement.active.clone(),
                observation: None,
            });
            group.remove(new_position);
        }
        Some(candidate)
    };
    Ok(preserve())
}

/// Replace one active record inside a bounded shared shard. The old shard is
/// copied privately, its retired contribution is projected away, and the new
/// singleton is projected into exactly the old local slot. All index families
/// use the same dense slot permutation; pinned readers retain the old shard.
pub(crate) fn preserve_partition_record_slots(
    view: &IndexView,
    next: &PublishedIndex,
    plan: &VerifiedPlan,
    file: FileId,
) -> Result<Option<PublishedIndex>, IndexError> {
    if plan.has_active_base {
        return Ok(None);
    }
    let (Some(old_entry), Some(new_entry)) = (view.file_entry(file)?, next.catalog.file(file)?)
    else {
        return Ok(None);
    };
    let mut read_error = None;
    let mut preserve = || -> Option<PublishedIndex> {
        if old_entry.collections != new_entry.collections || old_entry.collections.is_empty() {
            return None;
        }
        let mut replacements = Vec::with_capacity(old_entry.collections.len());
        for collection in &old_entry.collections {
            let mut ordinals = plan
                .old_shards
                .iter()
                .filter(|(id, _)| id == collection)
                .map(|(_, ordinal)| *ordinal);
            let old_ordinal = ordinals.next()?;
            if ordinals.next().is_some() {
                return None;
            }
            let old = view
                .segments(*collection)?
                .iter()
                .find(|shard| shard.id.get() == old_ordinal)?;
            let old_slots = old.index.file_records(file)?;
            if old_slots.len() != 1
                || old.index.record_count() <= 1
                || old.index.record_count() > MAX_VERIFIED_SCANNED_RECORDS
                || old.active.len() as usize != old.index.record_count()
                || !old.active.contains(old_slots[0])
            {
                return None;
            }
            let old_slot = old_slots[0];
            let group = next.segments.get(collection.0 as usize)?;
            let old_position = group
                .iter()
                .position(|shard| shard.id.get() == old_ordinal)?;
            if group[old_position].active.contains(old_slot) {
                return None;
            }
            let new_position = group
                .iter()
                .enumerate()
                .filter(|(_, shard)| shard.index.file(file).is_some())
                .max_by_key(|(_, shard)| shard.id)?
                .0;
            if new_position == old_position {
                return None;
            }
            let new = &group[new_position];
            if new.index.file_count() != 1
                || new.index.record_count() != 1
                || new.active.len() != 1
                || !new.active.contains(RecordSlot(0))
                || new.index.file_records(file)? != [RecordSlot(0)]
                || old.index.source_id != new.index.source_id
                || old.index.source_version != new.index.source_version
                || old.index.record(old_slot)?.id != new.index.record(RecordSlot(0))?.id
                || old.index.record(old_slot)?.class != new.index.record(RecordSlot(0))?.class
            {
                return None;
            }
            let active_without_old = match old.active.retired(&[old_slot]) {
                Ok(active) => active,
                Err(error) => {
                    read_error = Some(error.into());
                    return None;
                }
            };
            let compacted = old
                .index
                .compact_active(&active_without_old, MAX_VERIFIED_SCANNED_RECORDS, |entry| {
                    entry.id != file
                        && match next.catalog.file(entry.id) {
                            Ok(current) => current.is_some_and(|current| {
                                current.revision == entry.revision
                                    && current.created_ns == entry.created_ns
                                    && current.collections.contains(collection)
                            }),
                            Err(error) => {
                                read_error = Some(error.into());
                                false
                            }
                        }
                })
                .ok()?;
            let consolidated = crate::index::segment::IndexedCollectionSegment::consolidate(
                &[&compacted.index, &new.index],
                MAX_VERIFIED_SCANNED_RECORDS,
            )
            .ok()?;
            let mut remap = vec![old_slot; consolidated.index.record_count()];
            for (original, projected) in compacted.old_to_new.iter().enumerate() {
                if let Some(projected) = projected {
                    remap[projected.0 as usize] = RecordSlot(original as u32);
                }
            }
            let replacement = consolidated.index.remap_dense(&remap).ok()?;
            replacements.push((
                *collection,
                old_position,
                new_position,
                old_ordinal,
                replacement,
            ));
        }
        let mut candidate = next.clone();
        for (collection, old_position, new_position, old_ordinal, replacement) in replacements {
            let group = Arc::make_mut(candidate.segments.get_mut(collection.0 as usize)?);
            group[old_position] = Arc::new(PublishedSegment {
                id: crate::index::segment::SegmentId::new(old_ordinal),
                active: match candidate.repository.storage_factory.live_records(
                    replacement.record_count(),
                    crate::index::storage::liveness::InitialLiveness::All,
                ) {
                    Ok(active) => active,
                    Err(error) => {
                        read_error = Some(error.into());
                        return None;
                    }
                },
                index: Arc::new(replacement),
                observation: None,
            });
            group.remove(new_position);
        }
        Some(candidate)
    };
    let candidate = preserve();
    if let Some(error) = read_error {
        return Err(error);
    }
    Ok(candidate)
}

/// Project all retired old shards out of a private staged root. A failure
/// leaves the caller's original staged root available for the normal path.
pub(crate) fn remove_verified_old_shards(
    next: &PublishedIndex,
    plan: &VerifiedPlan,
    replaced_file: crate::model::FileId,
) -> Result<PublishedIndex, VerificationError> {
    if plan.has_active_base {
        return Err(VerifiedFallbackReason::BaseIndexUnsupported.into());
    }
    let mut candidate = next.clone();
    let catalog = candidate.catalog.clone();
    let collections = plan
        .old_shards
        .iter()
        .map(|(collection, _)| *collection)
        .collect::<BTreeSet<_>>();
    for collection in collections {
        let group = candidate
            .segments
            .get_mut(collection.0 as usize)
            .ok_or(VerifiedFallbackReason::IndexedLayoutChanged)?;
        let group = Arc::make_mut(group);
        // The replacement is appended after any consolidation performed by
        // staging, so it has the highest ordinal for this FileId. Deletion
        // has no replacement and removes every historical shard of the file.
        let mut replacement_ordinal = None;
        if catalog
            .file(replaced_file)?
            .is_some_and(|entry| entry.collections.contains(&collection))
        {
            for shard in group.iter() {
                if shard.contains_file(replaced_file)? {
                    replacement_ordinal = replacement_ordinal.max(Some(shard.id.get()));
                }
            }
        }
        let mut position = 0;
        while position < group.len() {
            let shard = &group[position];
            if Some(shard.id.get()) == replacement_ordinal || !shard.contains_file(replaced_file)? {
                position += 1;
                continue;
            }
            if shard.descriptor().slots > MAX_VERIFIED_SCANNED_RECORDS {
                return Err(VerifiedFallbackReason::CompactionBudget.into());
            }
            if shard.active.len() == 0 {
                group.remove(position);
                continue;
            }
            let mut retained = std::collections::HashSet::new();
            visit_segment_files(shard.as_ref(), |entry| {
                if entry.id != replaced_file
                    && catalog.file(entry.id)?.is_some_and(|current| {
                        current.revision == entry.revision
                            && current.created_ns == entry.created_ns
                            && current.collections.contains(&collection)
                    })
                {
                    retained.insert(entry.id);
                }
                Ok::<_, VerificationError>(())
            })?;
            let compacted = shard
                .index
                .compact_active(&shard.active, MAX_VERIFIED_SCANNED_RECORDS, |entry| {
                    retained.contains(&entry.id)
                })
                .map_err(|_| VerifiedFallbackReason::CompactionUnavailable)?;
            if compacted.index.record_count() == 0 {
                group.remove(position);
            } else {
                let ordinal = candidate
                    .next_segment_ordinal
                    .checked_add(1)
                    .ok_or(VerifiedFallbackReason::CompactionUnavailable)?;
                let active = candidate.repository.storage_factory.live_records(
                    compacted.index.record_count(),
                    crate::index::storage::liveness::InitialLiveness::All,
                )?;
                group[position] = Arc::new(PublishedSegment {
                    id: crate::index::segment::SegmentId::new(candidate.next_segment_ordinal),
                    index: Arc::new(compacted.index),
                    active,
                    observation: None,
                });
                candidate.next_segment_ordinal = ordinal;
                position += 1;
            }
        }
        candidate.rebuild_record_routes(collection.0 as usize)?;
    }
    Ok(candidate)
}
