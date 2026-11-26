//! Verified old-content admission and physical removal in an unpublished root.

use crate::index::storage::file_catalog::FileCatalogReader;
use std::collections::BTreeSet;
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::construction::batch::SourceFileInput;
use crate::construction::file_preparation::{
    FilePreparationBinding, FileRecordLayout, reconstruct_file_layout,
};
use crate::index::storage::publication::PublishedSegment;
use crate::index::storage::segment::maintenance::{
    check_projection_source, project_active_checked,
};
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
        base_owned |= collection.base_contains_file(handle.file_id)?;
        collection.visit_base_file_slots(handle.file_id, &mut |slots| {
            for &slot in slots {
                if view.0.active[number].contains(slot)? {
                    let record = collection.base_record(slot)?;
                    current.push((
                        *collection_id,
                        record.id.clone(),
                        record.range,
                        record.class,
                    ));
                    has_active_base = true;
                }
            }
            Ok(())
        })?;
        let routes = view
            .0
            .record_routes
            .get(number)
            .ok_or(VerifiedFallbackReason::IndexedLayoutChanged)?;
        for id in routes.segments_for_file(handle.file_id)?.iter() {
            let position = routes
                .position(*id)?
                .ok_or(VerifiedFallbackReason::IndexedLayoutChanged)?;
            let shard = routes
                .placement_at(position)?
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
            visit_checked_file_slots(shard.as_ref().as_ref(), handle.file_id, &mut |slots| {
                for &slot in slots {
                    if !shard.active.contains(slot)? {
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

struct VerifiedSlotReplacement {
    old: Arc<PublishedSegment>,
    replacement: Arc<PublishedSegment>,
    slot: RecordSlot,
    old_position: usize,
    new_position: usize,
}

fn unique_file_slot(
    segment: &PublishedSegment,
    file: FileId,
) -> Result<Option<RecordSlot>, IndexError> {
    let mut slot = None;
    let mut multiple = false;
    visit_checked_file_slots(segment, file, &mut |slots| {
        if slots.len() != 1 || slot.replace(slots[0]).is_some() {
            multiple = true;
        }
        Ok(())
    })?;
    Ok(slot.filter(|_| !multiple))
}

fn verified_slot_replacement(
    view: &IndexView,
    next: &PublishedIndex,
    plan: &VerifiedPlan,
    file: FileId,
    collection: CollectionId,
    slots: std::ops::RangeInclusive<usize>,
) -> Result<Option<VerifiedSlotReplacement>, IndexError> {
    let mut ordinals = plan
        .old_shards
        .iter()
        .filter(|(id, _)| *id == collection)
        .map(|(_, ordinal)| *ordinal);
    let Some(old_ordinal) = ordinals.next() else {
        return Ok(None);
    };
    if ordinals.next().is_some() {
        return Ok(None);
    }
    let old_group = view.segments(collection)?;
    let Some(old) = old_group.iter().find(|shard| shard.id.get() == old_ordinal) else {
        return Ok(None);
    };
    let old_slots = old.descriptor().slots;
    if !slots.contains(&old_slots)
        || old.active.len() as usize != old_slots
        || (old_slots == 1 && old.file_count() != 1)
    {
        return Ok(None);
    }
    let Some(slot) = unique_file_slot(old, file)? else {
        return Ok(None);
    };
    if !old.active.contains(slot)? {
        return Ok(None);
    }
    let group = next.segment_group(collection.0 as usize)?;
    let Some(old_position) = group.iter().position(|shard| shard.id == old.id) else {
        return Ok(None);
    };
    if group[old_position].active.contains(slot)? {
        return Ok(None);
    }
    let mut newest = None;
    for (position, shard) in group.iter().enumerate() {
        if newest.is_none_or(|(_, id)| shard.id > id) && shard.contains_file(file)? {
            newest = Some((position, shard.id));
        }
    }
    let Some((new_position, _)) = newest else {
        return Ok(None);
    };
    if new_position == old_position {
        return Ok(None);
    }
    let replacement = &group[new_position];
    let new_slot = RecordSlot(0);
    let old_source = old.source();
    let new_source = replacement.source();
    if replacement.descriptor().slots != 1
        || replacement.file_count() != 1
        || replacement.active.len() != 1
        || !replacement.active.contains(new_slot)?
        || old_source.adapter != new_source.adapter
        || old_source.version != new_source.version
        || unique_file_slot(replacement, file)? != Some(new_slot)
    {
        return Ok(None);
    }
    let missing_header = || IndexError::InvalidPublication("verified record header missing".into());
    let old_header = old.header(slot)?.ok_or_else(missing_header)?;
    let new_header = replacement.header(new_slot)?.ok_or_else(missing_header)?;
    if old_header.id != new_header.id || old_header.class != new_header.class {
        return Ok(None);
    }
    Ok(Some(VerifiedSlotReplacement {
        old: Arc::clone(old),
        replacement: Arc::clone(replacement),
        slot,
        old_position,
        new_position,
    }))
}

/// Replace an isolated shard with an independently built payload at its
/// original address. Pinned views retain the original shard.
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
    if old_entry.collections != new_entry.collections || old_entry.collections.is_empty() {
        return Ok(None);
    }
    let mut candidate = next.clone();
    for &collection in &old_entry.collections {
        let Some(replacement) =
            verified_slot_replacement(view, next, plan, file, collection, 1..=1)?
        else {
            return Ok(None);
        };
        let number = collection.0 as usize;
        let mut group = candidate.segment_group(number)?;
        let group = Arc::make_mut(&mut group);
        group[replacement.old_position] = Arc::new(PublishedSegment {
            id: replacement.old.id,
            index: Arc::clone(&replacement.replacement.index),
            active: replacement.replacement.active.clone(),
            observation: None,
        });
        group.remove(replacement.new_position);
        candidate.replace_segment_group(number, group)?;
    }
    Ok(Some(candidate))
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
    if old_entry.collections != new_entry.collections || old_entry.collections.is_empty() {
        return Ok(None);
    }
    let mut candidate = next.clone();
    for &collection in &old_entry.collections {
        let Some(found) = verified_slot_replacement(
            view,
            next,
            plan,
            file,
            collection,
            2..=MAX_VERIFIED_SCANNED_RECORDS,
        )?
        else {
            return Ok(None);
        };
        let active_without_old = found.old.active.retired(&[found.slot])?;
        let mut retained = BTreeSet::new();
        visit_segment_files(found.old.as_ref(), |entry| {
            if entry.id != file
                && next.catalog.file(entry.id)?.is_some_and(|current| {
                    current.revision == entry.revision
                        && current.created_ns == entry.created_ns
                        && current.collections.contains(&collection)
                })
            {
                retained.insert(entry.id);
            }
            Ok::<_, IndexError>(())
        })?;
        let Some(editor) = found.old.maintenance() else {
            return Ok(None);
        };
        let compacted = project_active_checked(
            found.old.as_ref(),
            editor,
            &active_without_old,
            MAX_VERIFIED_SCANNED_RECORDS,
            &mut |entry| retained.contains(&entry.id),
        )?;
        let Some(consolidated) = candidate.repository.storage_factory.consolidate_segments(
            &[compacted.payload.as_ref(), found.replacement.index.as_ref()],
            MAX_VERIFIED_SCANNED_RECORDS,
        )?
        else {
            return Ok(None);
        };
        let mut remap = vec![found.slot; consolidated.payload.descriptor().slots];
        for (original, projected) in compacted.old_to_new.iter().enumerate() {
            if let Some(projected) = projected {
                remap[projected.0 as usize] = RecordSlot(original as u32);
            }
        }
        let Some(editor) = consolidated.payload.maintenance() else {
            return Ok(None);
        };
        let replacement = editor.remap_dense(&remap)?;
        check_projection_source(found.old.as_ref(), replacement.as_ref(), remap.len())?;
        let active = candidate.repository.storage_factory.live_records(
            replacement.descriptor().slots,
            crate::index::storage::liveness::InitialLiveness::All,
        )?;
        let number = collection.0 as usize;
        let mut group = candidate.segment_group(number)?;
        let group = Arc::make_mut(&mut group);
        group[found.old_position] = Arc::new(PublishedSegment {
            id: found.old.id,
            index: replacement,
            active,
            observation: None,
        });
        group.remove(found.new_position);
        candidate.replace_segment_group(number, group)?;
    }
    Ok(Some(candidate))
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
        let number = collection.0 as usize;
        let mut group = candidate.segment_group(number)?;
        let group = Arc::make_mut(&mut group);
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
            let editor = shard
                .maintenance()
                .ok_or(VerifiedFallbackReason::CompactionUnavailable)?;
            let compacted = project_active_checked(
                shard.as_ref(),
                editor,
                &shard.active,
                MAX_VERIFIED_SCANNED_RECORDS,
                &mut |entry| retained.contains(&entry.id),
            )?;
            if compacted.payload.descriptor().slots == 0 {
                group.remove(position);
            } else {
                let ordinal = candidate
                    .next_segment_ordinal
                    .checked_add(1)
                    .ok_or(VerifiedFallbackReason::CompactionUnavailable)?;
                let active = candidate.repository.storage_factory.live_records(
                    compacted.payload.descriptor().slots,
                    crate::index::storage::liveness::InitialLiveness::All,
                )?;
                group[position] = Arc::new(PublishedSegment {
                    id: crate::index::segment::SegmentId::new(candidate.next_segment_ordinal),
                    index: compacted.payload,
                    active,
                    observation: None,
                });
                candidate.next_segment_ordinal = ordinal;
                position += 1;
            }
        }
        candidate.replace_segment_group(number, group)?;
    }
    Ok(candidate)
}
