//! Verified old-content admission and physical removal in an unpublished root.

use std::collections::BTreeSet;
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::build::{FilePreparationBinding, SourceFileInput, prepare_file_build_from_bytes};
use crate::lifecycle::{FileHandle, IndexView, PublishedIndex, PublishedSegment};
use crate::lifecycle_mutation::VerifiedFallbackReason;
use crate::model::CollectionId;

const MAX_VERIFIED_SCANNED_RECORDS: usize = 16_384;


/// Old segment ordinals are captured before staging. Staging may consolidate
/// them, so removal scans the affected collections again in the private root.
pub(crate) struct VerifiedPlan {
    old_shards: Vec<(CollectionId, u64)>,
}


pub(crate) fn verify_old_content(
    view: &IndexView,
    handle: FileHandle,
    old_bytes: &[u8],
) -> Result<VerifiedPlan, VerifiedFallbackReason> {
    let old = view
        .file_entry(handle.file_id)
        .ok_or(VerifiedFallbackReason::IndexedLayoutChanged)?;
    let fingerprint = view
        .file_fingerprint(handle.file_id)
        .ok_or(VerifiedFallbackReason::MissingFingerprint)?;
    if view.file_configuration(handle.file_id) != Some(view.configuration_identity()) {
        return Err(VerifiedFallbackReason::ConfigurationChanged);
    }
    let supplied_fingerprint: [u8; 32] = Sha256::digest(old_bytes).into();
    if supplied_fingerprint != fingerprint {
        return Err(VerifiedFallbackReason::ContentMismatch);
    }
    let repository = view.repository();
    let mut bindings = Vec::with_capacity(old.collections.len());
    for collection_id in &old.collections {
        let collection = repository
            .collections()
            .get(collection_id.0 as usize)
            .ok_or(VerifiedFallbackReason::IndexedLayoutChanged)?;
        if collection.source.is_extension() {
            return Err(VerifiedFallbackReason::UnsupportedReader);
        }
        bindings.push(FilePreparationBinding {
            collection: *collection_id,
            definition: &collection.definition,
            source: &collection.source,
        });
    }
    let input = SourceFileInput {
        id: handle.file_id,
        relative_path: old.relative_path.clone(),
        revision: old.revision,
        created_ns: old.created_ns,
    };
    let options = repository.options();
    let reconstructed = prepare_file_build_from_bytes(
        &repository.files().root,
        input,
        old_bytes,
        &bindings,
        options.bigram_alphabet,
        options.inference,
        1,
    )
    .map_err(|_| VerifiedFallbackReason::ReconstructionFailed)?;
    let mut current = Vec::new();
    let mut old_shards = Vec::new();
    for collection_id in &old.collections {
        let shards = view
            .segments(*collection_id)
            .ok_or(VerifiedFallbackReason::IndexedLayoutChanged)?;
        for shard in shards {
            let Some(slots) = shard.index.file_records(handle.file_id) else {
                continue;
            };
            old_shards.push((*collection_id, shard.ordinal));
            for slot in slots {
                if shard.active.contains(*slot) {
                    let record = shard
                        .index
                        .record(*slot)
                        .ok_or(VerifiedFallbackReason::IndexedLayoutChanged)?;
                    current.push((
                        *collection_id,
                        record.id.clone(),
                        record.range,
                        record.class,
                    ));
                }
            }
        }
    }
    if old_shards.is_empty() {
        return Err(VerifiedFallbackReason::BaseIndexUnsupported);
    }
    if current.len() as u64
        != view
            .file_record_count(handle.file_id)
            .ok_or(VerifiedFallbackReason::IndexedLayoutChanged)?
    {
        return Err(VerifiedFallbackReason::BaseIndexUnsupported);
    }
    let mut reconstructed = reconstructed.record_layout();
    let order = |item: &(
        CollectionId,
        crate::model::RecordId,
        crate::inventory::ByteRange,
        crate::inventory::RecordClass,
    )| { (item.0, item.2.start, item.2.end, item.1.clone()) };
    current.sort_by_key(order);
    reconstructed.sort_by_key(order);
    if current != reconstructed {
        return Err(VerifiedFallbackReason::IndexedLayoutChanged);
    }
    let mut scanned = 0usize;
    for (collection, ordinal) in &old_shards {
        let shard = view
            .segments(*collection)
            .and_then(|shards| shards.iter().find(|shard| shard.ordinal == *ordinal))
            .ok_or(VerifiedFallbackReason::IndexedLayoutChanged)?;
        scanned = scanned.saturating_add(shard.index.records.len());
        if scanned > MAX_VERIFIED_SCANNED_RECORDS {
            return Err(VerifiedFallbackReason::CompactionBudget);
        }
    }
    Ok(VerifiedPlan { old_shards })
}

/// Project all retired old shards out of a private staged root. A failure
/// leaves the caller's original staged root available for the normal path.
pub(crate) fn remove_verified_old_shards(
    next: &PublishedIndex,
    plan: &VerifiedPlan,
    replaced_file: crate::model::FileId,
) -> Result<PublishedIndex, VerifiedFallbackReason> {
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
        let replacement_ordinal = catalog
            .entry(replaced_file)
            .filter(|entry| entry.collections.contains(&collection))
            .and_then(|_| {
                group
                    .iter()
                    .filter(|shard| shard.index.file(replaced_file).is_some())
                    .map(|shard| shard.ordinal)
                    .max()
            });
        let mut position = 0;
        while position < group.len() {
            let shard = &group[position];
            if Some(shard.ordinal) == replacement_ordinal
                || shard.index.file(replaced_file).is_none()
            {
                position += 1;
                continue;
            }
            if shard.index.records.len() > MAX_VERIFIED_SCANNED_RECORDS {
                return Err(VerifiedFallbackReason::CompactionBudget);
            }
            if shard.active.len() == 0 {
                group.remove(position);
                continue;
            }
            let compacted = shard
                .index
                .compact_active(&shard.active, MAX_VERIFIED_SCANNED_RECORDS, |entry| {
                    entry.id != replaced_file
                        && catalog.entry(entry.id).is_some_and(|current| {
                            current.relative_path == entry.relative_path
                                && current.revision == entry.revision
                                && current.created_ns == entry.created_ns
                                && current.collections.contains(&collection)
                        })
                })
                .map_err(|_| VerifiedFallbackReason::CompactionUnavailable)?;
            if compacted.index.records.is_empty() {
                group.remove(position);
            } else {
                let ordinal = candidate
                    .next_segment_ordinal
                    .checked_add(1)
                    .ok_or(VerifiedFallbackReason::CompactionUnavailable)?;
                group[position] = Arc::new(PublishedSegment {
                    ordinal: candidate.next_segment_ordinal,
                    index: Arc::new(compacted.index),
                    active: compacted.active,
                });
                candidate.next_segment_ordinal = ordinal;
                position += 1;
            }
        }
    }
    Ok(candidate)
}
