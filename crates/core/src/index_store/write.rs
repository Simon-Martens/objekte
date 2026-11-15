//! Checked write helpers for one redb write transaction.
//!
//! The coordinator opens one transaction per file mutation or admission batch
//! and calls these helpers. No helper commits on its own and no helper catches
//! a storage error.

use std::ops::Bound;

use redb::{ReadableTable, ReadableTableMetadata, TableHandle, WriteTransaction};

use crate::inventory::{FileEntry, RecordEntry};
use crate::model::{CollectionId, FileId, RecordId, RecordSlot};
use crate::posting::ScalarKey;

use super::key_codec::{Cursor, KeyWriter, prefix_successor};
use super::keys;
use super::tables;
use super::value_codec::{
    BindingDescriptor, BindingId, CollectionMetadata, CoverageKind, DomainKind,
    FieldCoverage, FileBuildState, FileMetadata, GramKind, LengthDirectoryEntry, RecordMetadata,
    RelationStateKind, SortDefinition, StateKind, StoredBinding, StoredContributions,
    ValueDirectoryEntry,
};
use super::{StoreError, StoreResult};

pub(crate) fn map_table(error: redb::TableError) -> StoreError {
    StoreError::database(error)
}

pub(crate) fn map_storage(error: redb::StorageError) -> StoreError {
    StoreError::database(error)
}

pub(crate) fn put_metadata(
    tx: &WriteTransaction,
    metadata: &super::value_codec::StoreMetadata,
) -> StoreResult<()> {
    let encoded = super::value_codec::encode(metadata)?;
    let mut table = tx.open_table(tables::STORE_METADATA).map_err(map_table)?;
    table
        .insert(tables::meta::FORMAT, encoded.as_slice())
        .map_err(map_storage)?;
    Ok(())
}

pub fn put_collection(tx: &WriteTransaction, collection: &CollectionMetadata) -> StoreResult<()> {
    let key = keys::collection(collection.id.0);
    let encoded = super::value_codec::encode(collection)?;
    let mut table = tx.open_table(tables::COLLECTIONS).map_err(map_table)?;
    table
        .insert(key.as_slice(), encoded.as_slice())
        .map_err(map_storage)?;
    Ok(())
}

pub fn put_field(
    tx: &WriteTransaction,
    collection: CollectionId,
    path: &[String],
    descriptor: &super::value_codec::FieldDescriptor,
) -> StoreResult<()> {
    let key = keys::field(collection.0, path);
    let encoded = super::value_codec::encode(descriptor)?;
    let mut table = tx.open_table(tables::FIELDS).map_err(map_table)?;
    table
        .insert(key.as_slice(), encoded.as_slice())
        .map_err(map_storage)?;
    Ok(())
}

pub fn put_binding(
    tx: &WriteTransaction,
    collection: CollectionId,
    identity: &[u8],
    stored: &StoredBinding,
) -> StoreResult<()> {
    let key = keys::binding_identity(collection.0, identity);
    let encoded = super::value_codec::encode(stored)?;
    let mut table = tx.open_table(tables::BINDINGS).map_err(map_table)?;
    table
        .insert(key.as_slice(), encoded.as_slice())
        .map_err(map_storage)?;
    Ok(())
}

pub fn put_field_coverage(
    tx: &WriteTransaction,
    collection: CollectionId,
    binding: BindingId,
    coverage: &FieldCoverage,
) -> StoreResult<()> {
    let key = keys::binding(collection.0, binding);
    let encoded = super::value_codec::encode(coverage)?;
    let mut table = tx.open_table(tables::FIELD_COVERAGE).map_err(map_table)?;
    table
        .insert(key.as_slice(), encoded.as_slice())
        .map_err(map_storage)?;
    Ok(())
}

pub fn put_sort_definition(
    tx: &WriteTransaction,
    collection: CollectionId,
    definition: &SortDefinition,
) -> StoreResult<()> {
    let key = keys::sort_definition(collection.0, definition.id);
    let encoded = super::value_codec::encode(definition)?;
    let mut table = tx.open_table(tables::SORT_DEFINITIONS).map_err(map_table)?;
    table
        .insert(key.as_slice(), encoded.as_slice())
        .map_err(map_storage)?;
    Ok(())
}

pub fn put_file(
    tx: &WriteTransaction,
    entry: &FileEntry,
    file_version: u64,
    fingerprint: Option<[u8; 32]>,
) -> StoreResult<()> {
    let metadata = FileMetadata {
        entry: entry.clone(),
        file_version,
        fingerprint,
    };
    // Reconcile path rows against the previous metadata before replacing it.
    put_paths(tx, entry)?;
    let key = keys::file(entry.id);
    let encoded = super::value_codec::encode(&metadata)?;
    let mut table = tx.open_table(tables::FILES).map_err(map_table)?;
    table
        .insert(key.as_slice(), encoded.as_slice())
        .map_err(map_storage)?;
    Ok(())
}

pub fn put_paths(tx: &WriteTransaction, entry: &FileEntry) -> StoreResult<()> {
    remove_paths(tx, entry.id)?;
    let native = crate::path_repr::native_bytes(entry.relative_path.as_path());
    let native_key = keys::native_path(&native);
    let mut paths = tx.open_table(tables::PATHS).map_err(map_table)?;
    paths
        .insert(native_key.as_slice(), entry.id.0.to_be_bytes().as_slice())
        .map_err(map_storage)?;
    drop(paths);
    let query_key = crate::path_repr::query_path(entry.relative_path.as_path());
    let key = keys::query_path(&query_key, entry.id);
    let mut query = tx.open_table(tables::QUERY_PATHS).map_err(map_table)?;
    query
        .insert(key.as_slice(), &[][..])
        .map_err(map_storage)?;
    Ok(())
}

pub fn remove_paths(tx: &WriteTransaction, id: FileId) -> StoreResult<()> {
    let Some(existing) = get_file(tx, id)? else {
        return Ok(());
    };
    let native = crate::path_repr::native_bytes(existing.entry.relative_path.as_path());
    let mut paths = tx.open_table(tables::PATHS).map_err(map_table)?;
    paths.remove(native.as_slice()).map_err(map_storage)?;
    drop(paths);
    let query_key = crate::path_repr::query_path(existing.entry.relative_path.as_path());
    let key = keys::query_path(&query_key, id);
    let mut query = tx.open_table(tables::QUERY_PATHS).map_err(map_table)?;
    query.remove(key.as_slice()).map_err(map_storage)?;
    Ok(())
}

pub fn get_file(tx: &WriteTransaction, id: FileId) -> StoreResult<Option<FileMetadata>> {
    let table = tx.open_table(tables::FILES).map_err(map_table)?;
    let key = keys::file(id);
    let Some(value) = table.get(key.as_slice()).map_err(map_storage)? else {
        return Ok(None);
    };
    super::value_codec::decode(value.value(), "file").map(Some)
}

pub fn remove_file(tx: &WriteTransaction, id: FileId) -> StoreResult<()> {
    remove_paths(tx, id)?;
    let key = keys::file(id);
    let mut table = tx.open_table(tables::FILES).map_err(map_table)?;
    table.remove(key.as_slice()).map_err(map_storage)?;
    drop(table);
    let mut state = tx.open_table(tables::FILE_BUILD_STATE).map_err(map_table)?;
    state.remove(key.as_slice()).map_err(map_storage)?;
    Ok(())
}

pub fn put_build_state(
    tx: &WriteTransaction,
    file: FileId,
    state: &FileBuildState,
) -> StoreResult<()> {
    let key = keys::file(file);
    let encoded = super::value_codec::encode(state)?;
    let mut table = tx.open_table(tables::FILE_BUILD_STATE).map_err(map_table)?;
    table
        .insert(key.as_slice(), encoded.as_slice())
        .map_err(map_storage)?;
    Ok(())
}

pub fn get_build_state(tx: &WriteTransaction, file: FileId) -> StoreResult<Option<FileBuildState>> {
    let table = tx.open_table(tables::FILE_BUILD_STATE).map_err(map_table)?;
    let key = keys::file(file);
    let Some(value) = table.get(key.as_slice()).map_err(map_storage)? else {
        return Ok(None);
    };
    super::value_codec::decode(value.value(), "file build state").map(Some)
}

pub fn add_pending_file(tx: &WriteTransaction, collection: CollectionId, file: FileId) -> StoreResult<()> {
    let key = keys::pending_file(collection.0, file);
    let mut table = tx.open_table(tables::PENDING_FILES).map_err(map_table)?;
    table
        .insert(key.as_slice(), &[][..])
        .map_err(map_storage)?;
    Ok(())
}

pub fn remove_pending_file(
    tx: &WriteTransaction,
    collection: CollectionId,
    file: FileId,
) -> StoreResult<bool> {
    let key = keys::pending_file(collection.0, file);
    let mut table = tx.open_table(tables::PENDING_FILES).map_err(map_table)?;
    Ok(table.remove(key.as_slice()).map_err(map_storage)?.is_some())
}

pub fn put_record(
    tx: &WriteTransaction,
    collection: CollectionId,
    slot: RecordSlot,
    record: &RecordMetadata,
) -> StoreResult<()> {
    let key = keys::record(collection.0, slot.0);
    let encoded = super::value_codec::encode(record)?;
    let mut table = tx.open_table(tables::RECORDS).map_err(map_table)?;
    table
        .insert(key.as_slice(), encoded.as_slice())
        .map_err(map_storage)?;
    Ok(())
}

pub fn delete_record_row(
    tx: &WriteTransaction,
    collection: CollectionId,
    slot: RecordSlot,
) -> StoreResult<()> {
    let key = keys::record(collection.0, slot.0);
    let mut table = tx.open_table(tables::RECORDS).map_err(map_table)?;
    table.remove(key.as_slice()).map_err(map_storage)?;
    Ok(())
}

pub fn get_record(
    tx: &WriteTransaction,
    collection: CollectionId,
    slot: RecordSlot,
) -> StoreResult<Option<RecordMetadata>> {
    let table = tx.open_table(tables::RECORDS).map_err(map_table)?;
    let key = keys::record(collection.0, slot.0);
    let Some(value) = table.get(key.as_slice()).map_err(map_storage)? else {
        return Ok(None);
    };
    super::value_codec::decode(value.value(), "record").map(Some)
}

pub fn put_logical_id(
    tx: &WriteTransaction,
    collection: CollectionId,
    id: &RecordId,
    slot: RecordSlot,
    incarnation: u64,
) -> StoreResult<()> {
    let key = keys::logical_id(collection.0, &id.0);
    let mut value = KeyWriter::with_capacity(12);
    value.push_u32(slot.0);
    value.push_u64(incarnation);
    let mut table = tx.open_table(tables::LOGICAL_IDS).map_err(map_table)?;
    table
        .insert(key.as_slice(), value.as_slice())
        .map_err(map_storage)?;
    Ok(())
}

pub fn remove_logical_id(
    tx: &WriteTransaction,
    collection: CollectionId,
    id: &RecordId,
) -> StoreResult<()> {
    let key = keys::logical_id(collection.0, &id.0);
    let mut table = tx.open_table(tables::LOGICAL_IDS).map_err(map_table)?;
    table.remove(key.as_slice()).map_err(map_storage)?;
    Ok(())
}

pub fn get_logical_id(
    tx: &WriteTransaction,
    collection: CollectionId,
    id: &RecordId,
) -> StoreResult<Option<(RecordSlot, u64)>> {
    let table = tx.open_table(tables::LOGICAL_IDS).map_err(map_table)?;
    let key = keys::logical_id(collection.0, &id.0);
    let Some(value) = table.get(key.as_slice()).map_err(map_storage)? else {
        return Ok(None);
    };
    let mut cursor = Cursor::new(value.value());
    Ok(Some((RecordSlot(cursor.read_u32()?), cursor.read_u64()?)))
}

pub fn put_file_record(
    tx: &WriteTransaction,
    file: FileId,
    collection: CollectionId,
    slot: RecordSlot,
) -> StoreResult<()> {
    let key = keys::file_record(file, collection.0, slot.0);
    let mut table = tx.open_table(tables::FILE_RECORDS).map_err(map_table)?;
    table
        .insert(key.as_slice(), &[][..])
        .map_err(map_storage)?;
    Ok(())
}

pub fn remove_file_record(
    tx: &WriteTransaction,
    file: FileId,
    collection: CollectionId,
    slot: RecordSlot,
) -> StoreResult<()> {
    let key = keys::file_record(file, collection.0, slot.0);
    let mut table = tx.open_table(tables::FILE_RECORDS).map_err(map_table)?;
    table.remove(key.as_slice()).map_err(map_storage)?;
    Ok(())
}

pub fn insert_value_member(
    tx: &WriteTransaction,
    collection: CollectionId,
    binding: BindingId,
    value: &ScalarKey,
    slot: RecordSlot,
) -> StoreResult<bool> {
    let key = keys::value_member(collection.0, binding, value, slot.0)?;
    let mut members = tx.open_table(tables::VALUE_MEMBERS).map_err(map_table)?;
    let inserted = members.insert(key.as_slice(), &[][..]).map_err(map_storage)?.is_none();
    drop(members);
    if inserted {
        adjust_value_directory(tx, collection, binding, value, 1)?;
    }
    Ok(inserted)
}

pub fn remove_value_member(
    tx: &WriteTransaction,
    collection: CollectionId,
    binding: BindingId,
    value: &ScalarKey,
    slot: RecordSlot,
) -> StoreResult<bool> {
    let key = keys::value_member(collection.0, binding, value, slot.0)?;
    let mut members = tx.open_table(tables::VALUE_MEMBERS).map_err(map_table)?;
    let removed = members.remove(key.as_slice()).map_err(map_storage)?.is_some();
    drop(members);
    if removed {
        adjust_value_directory(tx, collection, binding, value, -1)?;
    }
    Ok(removed)
}

pub fn adjust_value_directory(
    tx: &WriteTransaction,
    collection: CollectionId,
    binding: BindingId,
    value: &ScalarKey,
    delta: i64,
) -> StoreResult<()> {
    let key = keys::value_directory_key(collection.0, binding, value)?;
    let mut table = tx.open_table(tables::VALUE_DIRECTORY).map_err(map_table)?;
    let existing = table
        .get(key.as_slice())
        .map_err(map_storage)?
        .map(|guard| {
            super::value_codec::decode::<ValueDirectoryEntry>(guard.value(), "value directory")
        })
        .transpose()?;
    match (existing, delta) {
        (Some(entry), 1) => {
            let updated = ValueDirectoryEntry {
                cardinality: entry.cardinality.checked_add(1).ok_or_else(|| {
                    StoreError::CorruptIndex("value directory cardinality overflow".into())
                })?,
                representative: entry.representative,
            };
            let encoded = super::value_codec::encode(&updated)?;
            table
                .insert(key.as_slice(), encoded.as_slice())
                .map_err(map_storage)?;
        }
        (Some(entry), -1) => {
            if entry.cardinality <= 1 {
                table.remove(key.as_slice()).map_err(map_storage)?;
            } else {
                let updated = ValueDirectoryEntry {
                    cardinality: entry.cardinality - 1,
                    representative: entry.representative,
                };
                let encoded = super::value_codec::encode(&updated)?;
                table
                    .insert(key.as_slice(), encoded.as_slice())
                    .map_err(map_storage)?;
            }
        }
        (None, 1) => {
            let encoded = super::value_codec::encode(&ValueDirectoryEntry {
                cardinality: 1,
                representative: value.clone(),
            })?;
            table
                .insert(key.as_slice(), encoded.as_slice())
                .map_err(map_storage)?;
        }
        (None, -1) => {
            return Err(StoreError::CorruptIndex(
                "removing a value without a directory entry".into(),
            ));
        }
        (_, other) => {
            return Err(StoreError::State(format!(
                "invalid value directory delta {other}"
            )));
        }
    }
    Ok(())
}

pub fn set_value_directory(
    tx: &WriteTransaction,
    collection: CollectionId,
    binding: BindingId,
    value: &ScalarKey,
    entry: &ValueDirectoryEntry,
) -> StoreResult<()> {
    let key = keys::value_directory_key(collection.0, binding, value)?;
    let encoded = super::value_codec::encode(entry)?;
    let mut table = tx.open_table(tables::VALUE_DIRECTORY).map_err(map_table)?;
    table
        .insert(key.as_slice(), encoded.as_slice())
        .map_err(map_storage)?;
    Ok(())
}

pub fn insert_length_member(
    tx: &WriteTransaction,
    collection: CollectionId,
    binding: BindingId,
    length: &ScalarKey,
    slot: RecordSlot,
) -> StoreResult<bool> {
    let key = keys::length_member(collection.0, binding, length, slot.0)?;
    let mut members = tx.open_table(tables::LENGTH_MEMBERS).map_err(map_table)?;
    let inserted = members.insert(key.as_slice(), &[][..]).map_err(map_storage)?.is_none();
    drop(members);
    if inserted {
        adjust_length_directory(tx, collection, binding, length, 1)?;
    }
    Ok(inserted)
}

pub fn remove_length_member(
    tx: &WriteTransaction,
    collection: CollectionId,
    binding: BindingId,
    length: &ScalarKey,
    slot: RecordSlot,
) -> StoreResult<bool> {
    let key = keys::length_member(collection.0, binding, length, slot.0)?;
    let mut members = tx.open_table(tables::LENGTH_MEMBERS).map_err(map_table)?;
    let removed = members.remove(key.as_slice()).map_err(map_storage)?.is_some();
    drop(members);
    if removed {
        adjust_length_directory(tx, collection, binding, length, -1)?;
    }
    Ok(removed)
}

fn adjust_length_directory(
    tx: &WriteTransaction,
    collection: CollectionId,
    binding: BindingId,
    length: &ScalarKey,
    delta: i64,
) -> StoreResult<()> {
    let key = keys::length_directory_key(collection.0, binding, length)?;
    let mut table = tx.open_table(tables::LENGTH_DIRECTORY).map_err(map_table)?;
    let existing = table
        .get(key.as_slice())
        .map_err(map_storage)?
        .map(|guard| {
            super::value_codec::decode::<LengthDirectoryEntry>(guard.value(), "length directory")
        })
        .transpose()?;
    match (existing, delta) {
        (Some(entry), 1) => {
            let encoded = super::value_codec::encode(&LengthDirectoryEntry {
                cardinality: entry.cardinality.checked_add(1).ok_or_else(|| {
                    StoreError::CorruptIndex("length directory cardinality overflow".into())
                })?,
            })?;
            table
                .insert(key.as_slice(), encoded.as_slice())
                .map_err(map_storage)?;
        }
        (Some(entry), -1) if entry.cardinality <= 1 => {
            table.remove(key.as_slice()).map_err(map_storage)?;
        }
        (Some(entry), -1) => {
            let encoded = super::value_codec::encode(&LengthDirectoryEntry {
                cardinality: entry.cardinality - 1,
            })?;
            table
                .insert(key.as_slice(), encoded.as_slice())
                .map_err(map_storage)?;
        }
        (None, 1) => {
            let encoded =
                super::value_codec::encode(&LengthDirectoryEntry { cardinality: 1 })?;
            table
                .insert(key.as_slice(), encoded.as_slice())
                .map_err(map_storage)?;
        }
        (None, -1) => {
            return Err(StoreError::CorruptIndex(
                "removing a length without a directory entry".into(),
            ));
        }
        (_, other) => {
            return Err(StoreError::State(format!(
                "invalid length directory delta {other}"
            )));
        }
    }
    Ok(())
}

pub fn insert_state_member(
    tx: &WriteTransaction,
    collection: CollectionId,
    binding: BindingId,
    state: StateKind,
    slot: RecordSlot,
) -> StoreResult<bool> {
    let key = keys::state_member(collection.0, binding, state, slot.0);
    let mut table = tx.open_table(tables::STATE_MEMBERS).map_err(map_table)?;
    Ok(table.insert(key.as_slice(), &[][..]).map_err(map_storage)?.is_none())
}

pub fn remove_state_member(
    tx: &WriteTransaction,
    collection: CollectionId,
    binding: BindingId,
    state: StateKind,
    slot: RecordSlot,
) -> StoreResult<bool> {
    let key = keys::state_member(collection.0, binding, state, slot.0);
    let mut table = tx.open_table(tables::STATE_MEMBERS).map_err(map_table)?;
    Ok(table.remove(key.as_slice()).map_err(map_storage)?.is_some())
}

pub fn insert_coverage_member(
    tx: &WriteTransaction,
    collection: CollectionId,
    binding: BindingId,
    coverage: CoverageKind,
    slot: RecordSlot,
) -> StoreResult<bool> {
    let key = keys::coverage_member(collection.0, binding, coverage, slot.0);
    let mut table = tx.open_table(tables::COVERAGE_MEMBERS).map_err(map_table)?;
    Ok(table.insert(key.as_slice(), &[][..]).map_err(map_storage)?.is_none())
}

pub fn remove_coverage_member(
    tx: &WriteTransaction,
    collection: CollectionId,
    binding: BindingId,
    coverage: CoverageKind,
    slot: RecordSlot,
) -> StoreResult<bool> {
    let key = keys::coverage_member(collection.0, binding, coverage, slot.0);
    let mut table = tx.open_table(tables::COVERAGE_MEMBERS).map_err(map_table)?;
    Ok(table.remove(key.as_slice()).map_err(map_storage)?.is_some())
}

pub fn insert_domain_member(
    tx: &WriteTransaction,
    collection: CollectionId,
    domain: DomainKind,
    slot: RecordSlot,
) -> StoreResult<bool> {
    let key = keys::domain_member(collection.0, domain, slot.0);
    let mut table = tx.open_table(tables::DOMAIN_MEMBERS).map_err(map_table)?;
    Ok(table.insert(key.as_slice(), &[][..]).map_err(map_storage)?.is_none())
}

pub fn remove_domain_member(
    tx: &WriteTransaction,
    collection: CollectionId,
    domain: DomainKind,
    slot: RecordSlot,
) -> StoreResult<bool> {
    let key = keys::domain_member(collection.0, domain, slot.0);
    let mut table = tx.open_table(tables::DOMAIN_MEMBERS).map_err(map_table)?;
    Ok(table.remove(key.as_slice()).map_err(map_storage)?.is_some())
}

pub fn put_contributions(
    tx: &WriteTransaction,
    collection: CollectionId,
    slot: RecordSlot,
    contributions: &StoredContributions,
) -> StoreResult<()> {
    let key = keys::record(collection.0, slot.0);
    let encoded = super::value_codec::encode(contributions)?;
    let mut table = tx.open_table(tables::RECORD_CONTRIBUTIONS).map_err(map_table)?;
    table
        .insert(key.as_slice(), encoded.as_slice())
        .map_err(map_storage)?;
    Ok(())
}

pub fn get_contributions(
    tx: &WriteTransaction,
    collection: CollectionId,
    slot: RecordSlot,
) -> StoreResult<Option<StoredContributions>> {
    let table = tx.open_table(tables::RECORD_CONTRIBUTIONS).map_err(map_table)?;
    let key = keys::record(collection.0, slot.0);
    let Some(value) = table.get(key.as_slice()).map_err(map_storage)? else {
        return Ok(None);
    };
    super::value_codec::decode(value.value(), "record contributions").map(Some)
}

pub fn remove_contributions(
    tx: &WriteTransaction,
    collection: CollectionId,
    slot: RecordSlot,
) -> StoreResult<Option<StoredContributions>> {
    let contributions = get_contributions(tx, collection, slot)?;
    if let Some(contributions) = &contributions {
        for (binding, value) in &contributions.values {
            remove_value_member(tx, collection, *binding, value, slot)?;
        }
        for (binding, length) in &contributions.lengths {
            remove_length_member(tx, collection, *binding, &ScalarKey::Number(length.clone()), slot)?;
        }
        for (binding, state) in &contributions.states {
            remove_state_member(tx, collection, *binding, *state, slot)?;
        }
        for (binding, coverage) in &contributions.coverage {
            remove_coverage_member(tx, collection, *binding, *coverage, slot)?;
        }
        for domain in &contributions.domains {
            remove_domain_member(tx, collection, *domain, slot)?;
        }
        super::sorting::remove_sort_projection(
            tx,
            collection,
            slot,
            &contributions.record.id.0,
            &contributions.sorts,
            &sort_definitions(tx, collection)?,
        )?;
        super::relations::remove_relation_occurrences(tx, collection, slot, &contributions.relations)?;
        super::fts::remove_record_grams(tx, collection, slot, &contributions.grams)?;
    }
    let key = keys::record(collection.0, slot.0);
    let mut table = tx.open_table(tables::RECORD_CONTRIBUTIONS).map_err(map_table)?;
    table.remove(key.as_slice()).map_err(map_storage)?;
    Ok(contributions)
}

/// Insert only the structured membership portion of a contribution. Sort,
/// relation and FTS application is delegated to their modules so a caller can
/// aggregate block work across a batch.
pub fn insert_structured(
    tx: &WriteTransaction,
    collection: CollectionId,
    slot: RecordSlot,
    contributions: &StoredContributions,
) -> StoreResult<()> {
    for (binding, value) in &contributions.values {
        insert_value_member(tx, collection, *binding, value, slot)?;
    }
    for (binding, length) in &contributions.lengths {
        insert_length_member(tx, collection, *binding, &ScalarKey::Number(length.clone()), slot)?;
    }
    for (binding, state) in &contributions.states {
        insert_state_member(tx, collection, *binding, *state, slot)?;
    }
    for (binding, coverage) in &contributions.coverage {
        insert_coverage_member(tx, collection, *binding, *coverage, slot)?;
    }
    for domain in &contributions.domains {
        insert_domain_member(tx, collection, *domain, slot)?;
    }
    Ok(())
}

pub fn remove_structured(
    tx: &WriteTransaction,
    collection: CollectionId,
    slot: RecordSlot,
    contributions: &StoredContributions,
) -> StoreResult<()> {
    for (binding, value) in &contributions.values {
        remove_value_member(tx, collection, *binding, value, slot)?;
    }
    for (binding, length) in &contributions.lengths {
        remove_length_member(tx, collection, *binding, &ScalarKey::Number(length.clone()), slot)?;
    }
    for (binding, state) in &contributions.states {
        remove_state_member(tx, collection, *binding, *state, slot)?;
    }
    for (binding, coverage) in &contributions.coverage {
        remove_coverage_member(tx, collection, *binding, *coverage, slot)?;
    }
    for domain in &contributions.domains {
        remove_domain_member(tx, collection, *domain, slot)?;
    }
    Ok(())
}

pub fn sort_definitions(
    tx: &WriteTransaction,
    collection: CollectionId,
) -> StoreResult<Vec<SortDefinition>> {
    let prefix = keys::collection_prefix(collection.0);
    let table = tx
        .open_table(tables::SORT_DEFINITIONS)
        .map_err(map_table)?;
    let successor = prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    let mut definitions = Vec::new();
    for row in table
        .range::<&[u8]>((Bound::Included(prefix.as_slice()), upper))
        .map_err(map_storage)?
    {
        let (_, value) = row.map_err(map_storage)?;
        definitions.push(super::value_codec::decode::<SortDefinition>(
            value.value(),
            "sort definition",
        )?);
    }
    Ok(definitions)
}

/// Apply exact old/new contribution differences for one retained slot.
pub fn apply_contribution_diff(
    tx: &WriteTransaction,
    collection: CollectionId,
    slot: RecordSlot,
    diff: &super::contributions::ContributionDiff,
    old: &StoredContributions,
    new: &StoredContributions,
    definitions: &[SortDefinition],
) -> StoreResult<()> {
    for (binding, value) in &diff.values_removed {
        remove_value_member(tx, collection, *binding, value, slot)?;
    }
    for (binding, value) in &diff.values_added {
        insert_value_member(tx, collection, *binding, value, slot)?;
    }
    for (binding, length) in &diff.lengths_removed {
        remove_length_member(
            tx,
            collection,
            *binding,
            &ScalarKey::Number(length.clone()),
            slot,
        )?;
    }
    for (binding, length) in &diff.lengths_added {
        insert_length_member(
            tx,
            collection,
            *binding,
            &ScalarKey::Number(length.clone()),
            slot,
        )?;
    }
    for (binding, state) in &diff.states_removed {
        remove_state_member(tx, collection, *binding, *state, slot)?;
    }
    for (binding, state) in &diff.states_added {
        insert_state_member(tx, collection, *binding, *state, slot)?;
    }
    for (binding, coverage) in &diff.coverage_removed {
        remove_coverage_member(tx, collection, *binding, *coverage, slot)?;
    }
    for (binding, coverage) in &diff.coverage_added {
        insert_coverage_member(tx, collection, *binding, *coverage, slot)?;
    }
    for domain in &diff.domains_removed {
        remove_domain_member(tx, collection, *domain, slot)?;
    }
    for domain in &diff.domains_added {
        insert_domain_member(tx, collection, *domain, slot)?;
    }
    super::sorting::remove_sort_projection(
        tx,
        collection,
        slot,
        &old.record.id.0,
        &diff.sorts_removed,
        definitions,
    )?;
    super::sorting::insert_sort_projection(
        tx,
        collection,
        slot,
        &new.record.id.0,
        &diff.sorts_added,
        definitions,
    )?;
    super::relations::remove_relation_occurrences(
        tx,
        collection,
        slot,
        &diff.relations_removed,
    )?;
    super::relations::insert_relation_occurrences(
        tx,
        collection,
        slot,
        &diff.relations_added,
    )?;
    let mut batch = super::fts::FtsBatch::new();
    for projection in &diff.grams_removed {
        for gram in &projection.grams {
            batch.clear(projection.binding, projection.kind, *gram, slot.0);
        }
    }
    for projection in &diff.grams_added {
        for gram in &projection.grams {
            batch.set(projection.binding, projection.kind, *gram, slot.0);
        }
    }
    super::fts::apply_batch(tx, collection, &batch)?;
    // Record gram rows are small and are rewritten from the new set.
    let mut grams_table = tx.open_table(tables::RECORD_GRAMS).map_err(map_table)?;
    for projection in &old.grams {
        let key = keys::record_grams(collection.0, slot.0, projection.binding, projection.kind);
        grams_table.remove(key.as_slice()).map_err(map_storage)?;
    }
    for projection in &new.grams {
        let key = keys::record_grams(collection.0, slot.0, projection.binding, projection.kind);
        let encoded = super::value_codec::encode_gram_list(&projection.grams);
        grams_table
            .insert(key.as_slice(), encoded.as_slice())
            .map_err(map_storage)?;
    }
    Ok(())
}

pub fn put_relation_state(
    tx: &WriteTransaction,
    collection: CollectionId,
    declaration: u32,
    state: RelationStateKind,
    slot: RecordSlot,
) -> StoreResult<()> {
    let key = keys::relation_state(collection.0, declaration, state, slot.0);
    let mut table = tx.open_table(tables::RELATION_STATES).map_err(map_table)?;
    table
        .insert(key.as_slice(), &[][..])
        .map_err(map_storage)?;
    Ok(())
}

pub fn remove_relation_state(
    tx: &WriteTransaction,
    collection: CollectionId,
    declaration: u32,
    state: RelationStateKind,
    slot: RecordSlot,
) -> StoreResult<()> {
    let key = keys::relation_state(collection.0, declaration, state, slot.0);
    let mut table = tx.open_table(tables::RELATION_STATES).map_err(map_table)?;
    table.remove(key.as_slice()).map_err(map_storage)?;
    Ok(())
}

pub fn put_fts_statistics(
    tx: &WriteTransaction,
    collection: CollectionId,
    binding: BindingId,
    kind: GramKind,
    gram: u64,
    cardinality: u64,
    blocks: u64,
) -> StoreResult<()> {
    let key = keys::fts_statistics_key(collection.0, binding, kind, gram);
    let mut value = KeyWriter::with_capacity(16);
    value.push_u64(cardinality);
    value.push_u64(blocks);
    let mut table = tx.open_table(tables::FTS_GRAM_STATISTICS).map_err(map_table)?;
    table
        .insert(key.as_slice(), value.as_slice())
        .map_err(map_storage)?;
    Ok(())
}

pub fn remove_fts_statistics(
    tx: &WriteTransaction,
    collection: CollectionId,
    binding: BindingId,
    kind: GramKind,
    gram: u64,
) -> StoreResult<()> {
    let key = keys::fts_statistics_key(collection.0, binding, kind, gram);
    let mut table = tx.open_table(tables::FTS_GRAM_STATISTICS).map_err(map_table)?;
    table.remove(key.as_slice()).map_err(map_storage)?;
    Ok(())
}

pub fn get_fts_statistics(
    tx: &WriteTransaction,
    collection: CollectionId,
    binding: BindingId,
    kind: GramKind,
    gram: u64,
) -> StoreResult<Option<(u64, u64)>> {
    let key = keys::fts_statistics_key(collection.0, binding, kind, gram);
    let table = tx.open_table(tables::FTS_GRAM_STATISTICS).map_err(map_table)?;
    let Some(value) = table.get(key.as_slice()).map_err(map_storage)? else {
        return Ok(None);
    };
    let mut cursor = Cursor::new(value.value());
    Ok(Some((cursor.read_u64()?, cursor.read_u64()?)))
}

pub fn list_table_names(tx: &WriteTransaction) -> StoreResult<Vec<String>> {
    let mut names = Vec::new();
    for handle in tx.list_tables().map_err(map_storage)? {
        names.push(handle.name().to_string());
    }
    Ok(names)
}

pub fn table_row_count(tx: &WriteTransaction, definition: super::tables::ByteTable) -> StoreResult<u64> {
    let table = tx.open_table(definition).map_err(map_table)?;
    table.len().map_err(map_storage)
}

pub fn scan_prefix(
    tx: &WriteTransaction,
    definition: super::tables::ByteTable,
    prefix: &[u8],
) -> StoreResult<Vec<Vec<u8>>> {
    let table = tx.open_table(definition).map_err(map_table)?;
    let successor = prefix_successor(prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    let mut keys = Vec::new();
    for row in table
        .range::<&[u8]>((Bound::Included(prefix), upper))
        .map_err(map_storage)?
    {
        let (key, _) = row.map_err(map_storage)?;
        keys.push(key.value().to_vec());
    }
    Ok(keys)
}

pub fn record_entry(
    metadata: &RecordMetadata,
    slot: RecordSlot,
) -> RecordEntry {
    metadata.to_entry(slot)
}

pub fn binding_descriptor(descriptor: &BindingDescriptor) -> &BindingDescriptor {
    descriptor
}
