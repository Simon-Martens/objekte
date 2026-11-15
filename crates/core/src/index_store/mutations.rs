//! Atomic file admission, update, deletion and move at the store boundary.
//!
//! One call uses one redb write transaction. A file and all its collection
//! memberships are one atomic unit. Reverse contributions make physical
//! removal independent of the removed source file.

use std::collections::{BTreeMap, HashMap};

use redb::ReadableTable;

use crate::inventory::FileEntry;
use crate::model::{CollectionId, FileId, RecordId, RecordSlot};

use super::construction::{mark_indexed, mark_unavailable};
use super::contributions::diff;
use super::value_codec::{
    BuildFileState, CollectionMetadata, ConstructionState, FileBuildState, StoredContributions,
};
use super::{IndexStore, StoreError, StoreResult, read, slot_allocator, write};

#[derive(Clone, Debug)]
pub struct PreparedFile {
    pub file: FileEntry,
    pub file_version: u64,
    pub fingerprint: Option<[u8; 32]>,
    pub records: Vec<StoredContributions>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AdmissionReport {
    pub slots: Vec<RecordSlot>,
    pub revision: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FileMutationReport {
    pub collection: Option<CollectionId>,
    pub retained: usize,
    pub removed: usize,
    pub added: usize,
    pub revision: u64,
}

fn read_collection_tx(
    tx: &redb::WriteTransaction,
    collection: CollectionId,
) -> StoreResult<CollectionMetadata> {
    let table = tx
        .open_table(super::tables::COLLECTIONS)
        .map_err(write::map_table)?;
    let key = super::keys::collection(collection.0);
    let value = table
        .get(key.as_slice())
        .map_err(write::map_storage)?
        .ok_or_else(|| StoreError::CorruptIndex(format!("missing collection {}", collection.0)))?;
    super::value_codec::decode(value.value(), "collection")
}

/// Admit one complete prepared file. The file must already be pending in the
/// collection after metadata discovery; every record and index family is
/// written before the pending membership changes.
pub fn admit_file(
    store: &IndexStore,
    collection: CollectionId,
    prepared: &PreparedFile,
    expected_epoch: u64,
) -> StoreResult<AdmissionReport> {
    let tx = store.begin_write()?;
    let mut metadata = read::metadata_from_write(&tx)?;
    if metadata.build_epoch != expected_epoch {
        return Err(StoreError::StaleHandle(format!(
            "prepared file epoch {expected_epoch} is not current {}",
            metadata.build_epoch
        )));
    }
    if matches!(
        metadata.construction,
        ConstructionState::Empty | ConstructionState::Discovering
    ) {
        return Err(StoreError::State(
            "admission requires a complete catalog".into(),
        ));
    }
    let mut collection_metadata = read_collection_tx(&tx, collection)?;
    let state = write::get_build_state(&tx, prepared.file.id)?.ok_or_else(|| {
        StoreError::State(format!(
            "file {} was not discovered",
            prepared.file.id.0
        ))
    })?;
    if state.expected_file_version != prepared.file_version {
        return Err(StoreError::StaleHandle(format!(
            "file {} version {} is not expected {}",
            prepared.file.id.0, prepared.file_version, state.expected_file_version
        )));
    }
    if state.state == BuildFileState::Indexed {
        return Err(StoreError::State(format!(
            "file {} is already indexed",
            prepared.file.id.0
        )));
    }
    if !prepared.file.collections.contains(&collection) {
        return Err(StoreError::State(
            "file does not belong to the admitting collection".into(),
        ));
    }
    for record in &prepared.records {
        if record.record.file != prepared.file.id {
            return Err(StoreError::State(
                "record contribution belongs to another file".into(),
            ));
        }
    }

    write::put_file(
        &tx,
        &prepared.file,
        prepared.file_version,
        prepared.fingerprint,
    )?;
    let mut slots = Vec::with_capacity(prepared.records.len());
    for contribution in &prepared.records {
        let (slot, incarnation) = slot_allocator::allocate(&tx, &mut collection_metadata)?;
        let mut contribution = contribution.clone();
        contribution.record.incarnation = incarnation;
        contribution.record.range = crate::inventory::ByteRange::checked(
            contribution.record.range.start,
            contribution.record.range.end,
            prepared.file.revision.size,
        )
        .map_err(StoreError::CorruptIndex)?;
        write::put_record(&tx, collection, slot, &contribution.record)?;
        write::put_logical_id(
            &tx,
            collection,
            &contribution.record.id,
            slot,
            incarnation,
        )?;
        write::put_file_record(&tx, prepared.file.id, collection, slot)?;
        write::insert_structured(&tx, collection, slot, &contribution)?;
        super::sorting::insert_sort_projection(
            &tx,
            collection,
            slot,
            &contribution.record.id.0,
            &contribution.sorts,
            &write::sort_definitions(&tx, collection)?,
        )?;
        super::relations::insert_relation_occurrences(
            &tx,
            collection,
            slot,
            &contribution.relations,
        )?;
        super::fts::insert_record_grams(&tx, collection, slot, &contribution.grams)?;
        write::put_contributions(&tx, collection, slot, &contribution)?;
        slots.push(slot);
    }
    let next_revision = metadata
        .revision
        .checked_add(1)
        .ok_or_else(|| StoreError::State("publication revision overflow".into()))?;
    mark_indexed(
        &tx,
        &mut collection_metadata,
        prepared.file.id,
        prepared.file_version,
        expected_epoch,
        next_revision,
    )?;
    metadata.revision = next_revision;
    write::put_collection(&tx, &collection_metadata)?;
    write::put_metadata(&tx, &metadata)?;
    tx.commit().map_err(StoreError::database)?;
    Ok(AdmissionReport {
        slots,
        revision: next_revision,
    })
}

/// Mark a file unavailable. Any pending membership is removed atomically and
/// no indexed record is fabricated.
pub fn mark_file_unavailable(
    store: &IndexStore,
    collection: CollectionId,
    file: FileId,
    expected_version: u64,
    expected_epoch: u64,
    error: String,
) -> StoreResult<u64> {
    let tx = store.begin_write()?;
    let mut metadata = read::metadata_from_write(&tx)?;
    if metadata.build_epoch != expected_epoch {
        return Err(StoreError::StaleHandle("build epoch changed".into()));
    }
    let mut collection_metadata = read_collection_tx(&tx, collection)?;
    let next_revision = metadata
        .revision
        .checked_add(1)
        .ok_or_else(|| StoreError::State("publication revision overflow".into()))?;
    mark_unavailable(
        &tx,
        &mut collection_metadata,
        file,
        expected_version,
        expected_epoch,
        next_revision,
        error,
    )?;
    metadata.revision = next_revision;
    write::put_collection(&tx, &collection_metadata)?;
    write::put_metadata(&tx, &metadata)?;
    tx.commit().map_err(StoreError::database)?;
    Ok(next_revision)
}

/// Delete one source file from every collection that owns it. No source file
/// is opened and unrelated records are not scanned.
pub fn delete_file(store: &IndexStore, file: FileId) -> StoreResult<FileMutationReport> {
    let tx = store.begin_write()?;
    let mut metadata = read::metadata_from_write(&tx)?;
    let entry = write::get_file(&tx, file)?.ok_or_else(|| {
        StoreError::State(format!("file {} is not in the catalog", file.0))
    })?;
    let mut report = FileMutationReport::default();
    let rows = read::records_for_file_for_write(&tx, file)?;
    let mut collections = BTreeMap::<u32, CollectionMetadata>::new();
    for row in &rows {
        collections
            .entry(row.collection.0)
            .or_insert(read_collection_tx(&tx, row.collection)?);
    }
    for row in rows {
        let collection_metadata = collections
            .get_mut(&row.collection.0)
            .expect("collection metadata was loaded");
        let record = write::get_record(&tx, row.collection, row.slot)?;
        write::remove_contributions(&tx, row.collection, row.slot)?;
        if let Some(record) = record {
            write::remove_logical_id(&tx, row.collection, &record.id)?;
        }
        write::remove_file_record(&tx, file, row.collection, row.slot)?;
        write::delete_record_row(&tx, row.collection, row.slot)?;
        slot_allocator::release_record(&tx, collection_metadata, row.slot)?;
        report.removed += 1;
    }
    for collection_metadata in collections.values_mut() {
        write::remove_pending_file(&tx, collection_metadata.id, file)?;
        if collection_metadata.indexed_files > 0 {
            collection_metadata.indexed_files -= 1;
        }
        if collection_metadata.discovered_files > 0 {
            collection_metadata.discovered_files -= 1;
        }
        if collection_metadata.processed_files > 0 {
            collection_metadata.processed_files -= 1;
        }
        write::put_collection(&tx, collection_metadata)?;
    }
    for collection in &entry.entry.collections {
        let mut collection_metadata = read_collection_tx(&tx, *collection)?;
        if collection_metadata.pending_files > 0 {
            collection_metadata.pending_files -= 1;
        }
        write::put_collection(&tx, &collection_metadata)?;
    }
    write::remove_file(&tx, file)?;
    metadata.revision = metadata
        .revision
        .checked_add(1)
        .ok_or_else(|| StoreError::State("publication revision overflow".into()))?;
    report.revision = metadata.revision;
    report.collection = entry.entry.collections.first().copied();
    write::put_metadata(&tx, &metadata)?;
    tx.commit().map_err(StoreError::database)?;
    Ok(report)
}

/// Replace one file's content, matching old and new records by logical ID.
///
/// Retained records keep their slot and incarnation; disappeared records free
/// their slot; new records allocate from the free list. Structured, sort,
/// relation and FTS families receive exact old/new differences.
pub fn update_file(
    store: &IndexStore,
    collection: CollectionId,
    prepared: &PreparedFile,
    expected_version: u64,
    expected_epoch: u64,
) -> StoreResult<FileMutationReport> {
    let tx = store.begin_write()?;
    let mut metadata = read::metadata_from_write(&tx)?;
    if metadata.build_epoch != expected_epoch {
        return Err(StoreError::StaleHandle("build epoch changed".into()));
    }
    let existing = write::get_file(&tx, prepared.file.id)?.ok_or_else(|| {
        StoreError::State(format!("file {} is not in the catalog", prepared.file.id.0))
    })?;
    if existing.file_version != expected_version {
        return Err(StoreError::StaleHandle(format!(
            "file {} version {} is not expected {}",
            prepared.file.id.0, existing.file_version, expected_version
        )));
    }
    let mut collection_metadata = read_collection_tx(&tx, collection)?;
    let old_rows = read::records_for_file_for_write(&tx, prepared.file.id)?
        .into_iter()
        .filter(|row| row.collection == collection)
        .collect::<Vec<_>>();
    let mut old_by_id = HashMap::<RecordId, (RecordSlot, StoredContributions)>::new();
    for row in &old_rows {
        let record = write::get_record(&tx, collection, row.slot)?
            .ok_or_else(|| StoreError::CorruptIndex("record row disappeared".into()))?;
        let contributions = write::get_contributions(&tx, collection, row.slot)?
            .ok_or_else(|| StoreError::CorruptIndex("missing contributions".into()))?;
        old_by_id.insert(record.id, (row.slot, contributions));
    }
    let mut new_by_id = HashMap::<RecordId, &StoredContributions>::new();
    for contribution in &prepared.records {
        if new_by_id
            .insert(contribution.record.id.clone(), contribution)
            .is_some()
        {
            return Err(StoreError::State(format!(
                "duplicate logical record ID {} in file {}",
                contribution.record.id.0, prepared.file.id.0
            )));
        }
    }
    let definitions = write::sort_definitions(&tx, collection)?;
    let mut report = FileMutationReport {
        collection: Some(collection),
        ..FileMutationReport::default()
    };
    // Remove disappeared records and apply retained differences.
    for (id, (slot, old)) in &old_by_id {
        let Some(new) = new_by_id.remove(id) else {
            write::remove_contributions(&tx, collection, *slot)?;
            write::remove_logical_id(&tx, collection, id)?;
            write::remove_file_record(&tx, prepared.file.id, collection, *slot)?;
            write::delete_record_row(&tx, collection, *slot)?;
            slot_allocator::release_record(&tx, &mut collection_metadata, *slot)?;
            report.removed += 1;
            continue;
        };
        let mut new = new.clone();
        new.record.incarnation = old.record.incarnation;
        new.record.range = crate::inventory::ByteRange::checked(
            new.record.range.start,
            new.record.range.end,
            prepared.file.revision.size,
        )
        .map_err(StoreError::CorruptIndex)?;
        let differences = diff(old, &new);
        write::apply_contribution_diff(
            &tx,
            collection,
            *slot,
            &differences,
            old,
            &new,
            &definitions,
        )?;
        write::put_record(&tx, collection, *slot, &new.record)?;
        write::put_contributions(&tx, collection, *slot, &new)?;
        report.retained += 1;
    }
    // Allocate added records.
    for (id, new) in new_by_id {
        let (slot, incarnation) = slot_allocator::allocate(&tx, &mut collection_metadata)?;
        let mut new = new.clone();
        new.record.incarnation = incarnation;
        new.record.range = crate::inventory::ByteRange::checked(
            new.record.range.start,
            new.record.range.end,
            prepared.file.revision.size,
        )
        .map_err(StoreError::CorruptIndex)?;
        write::put_record(&tx, collection, slot, &new.record)?;
        write::put_logical_id(&tx, collection, &id, slot, incarnation)?;
        write::put_file_record(&tx, prepared.file.id, collection, slot)?;
        write::insert_structured(&tx, collection, slot, &new)?;
        super::sorting::insert_sort_projection(
            &tx,
            collection,
            slot,
            &id.0,
            &new.sorts,
            &definitions,
        )?;
        super::relations::insert_relation_occurrences(&tx, collection, slot, &new.relations)?;
        super::fts::insert_record_grams(&tx, collection, slot, &new.grams)?;
        write::put_contributions(&tx, collection, slot, &new)?;
        report.added += 1;
    }
    write::put_file(
        &tx,
        &prepared.file,
        prepared.file_version,
        prepared.fingerprint,
    )?;
    let state = write::get_build_state(&tx, prepared.file.id)?;
    if state
        .as_ref()
        .is_some_and(|state| state.state == BuildFileState::Pending)
    {
        // The file remains pending; its new preparation is current.
        write::put_build_state(
            &tx,
            prepared.file.id,
            &FileBuildState {
                state: BuildFileState::Pending,
                expected_file_version: prepared.file_version,
                build_epoch: expected_epoch,
                last_admission_revision: metadata.revision,
                error: None,
            },
        )?;
    }
    let next_revision = metadata
        .revision
        .checked_add(1)
        .ok_or_else(|| StoreError::State("publication revision overflow".into()))?;
    metadata.revision = next_revision;
    report.revision = next_revision;
    write::put_collection(&tx, &collection_metadata)?;
    write::put_metadata(&tx, &metadata)?;
    tx.commit().map_err(StoreError::database)?;
    Ok(report)
}

/// Metadata-only move. Record slots and all content contributions are
/// preserved; path tables, file metadata and discovery memberships update.
pub fn move_file(
    store: &IndexStore,
    file: FileId,
    destination: FileEntry,
    expected_version: u64,
) -> StoreResult<u64> {
    if destination.id != file {
        return Err(StoreError::State(
            "move destination must keep the file ID".into(),
        ));
    }
    let tx = store.begin_write()?;
    let mut metadata = read::metadata_from_write(&tx)?;
    let existing = write::get_file(&tx, file)?.ok_or_else(|| {
        StoreError::State(format!("file {} is not in the catalog", file.0))
    })?;
    if existing.file_version != expected_version {
        return Err(StoreError::StaleHandle("file version changed".into()));
    }
    let next_version = expected_version
        .checked_add(1)
        .ok_or_else(|| StoreError::State("file version overflow".into()))?;
    write::put_file(&tx, &destination, next_version, existing.fingerprint)?;
    let next_revision = metadata
        .revision
        .checked_add(1)
        .ok_or_else(|| StoreError::State("publication revision overflow".into()))?;
    metadata.revision = next_revision;
    write::put_metadata(&tx, &metadata)?;
    tx.commit().map_err(StoreError::database)?;
    Ok(next_revision)
}

#[cfg(test)]
#[path = "mutations.test.rs"]
mod tests;
