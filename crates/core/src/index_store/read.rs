//! Snapshot-bound read facade.
//!
//! All lookups use the view's single redb read transaction. Cursor and range
//! helpers either stream through a callback or return a bounded owned batch;
//! no caller is forced to materialize a whole table.

use std::ops::Bound;

use redb::{ReadTransaction, ReadableTable};

use crate::inventory::{FileEntry, RecordEntry};
use crate::model::{CollectionId, FileId, RecordSlot};
use crate::posting::ScalarKey;

use super::key_codec::{Cursor, KeyWriter, prefix_successor};
use super::keys;
use super::value_codec::{
    BindingDescriptor, BindingId, BuildFileState, CollectionMetadata, CoverageKind, DomainKind,
    FieldCoverage, FileBuildState, FileMetadata, GramKind, LengthDirectoryEntry, RecordMetadata,
    StateKind, ValueDirectoryEntry,
};
use super::{StoreError, StoreResult, StoreView, tables, value_codec};

#[derive(Clone, Debug)]
pub struct FileIdAndSlot {
    pub collection: CollectionId,
    pub slot: RecordSlot,
}

fn map_table(error: redb::TableError) -> StoreError {
    StoreError::database(error)
}

fn map_storage(error: redb::StorageError) -> StoreError {
    StoreError::database(error)
}

fn row_value<'a>(
    guard: &'a redb::AccessGuard<'a, &'static [u8]>,
) -> &'a [u8] {
    guard.value()
}

pub fn metadata(view: &StoreView) -> &crate::index_store::value_codec::StoreMetadata {
    view.metadata()
}

pub fn collection(view: &StoreView, id: CollectionId) -> StoreResult<CollectionMetadata> {
    let table = view
        .read()
        .open_table(tables::COLLECTIONS)
        .map_err(map_table)?;
    let key = keys::collection(id.0);
    let value = table
        .get(key.as_slice())
        .map_err(map_storage)?
        .ok_or_else(|| StoreError::CorruptIndex(format!("missing collection {}", id.0)))?;
    value_codec::decode(row_value(&value), "collection")
}

pub fn all_collections(view: &StoreView) -> StoreResult<Vec<CollectionMetadata>> {
    let table = view
        .read()
        .open_table(tables::COLLECTIONS)
        .map_err(map_table)?;
    let mut collections = Vec::new();
    for row in table.iter().map_err(map_storage)? {
        let (_, value) = row.map_err(map_storage)?;
        collections.push(value_codec::decode::<CollectionMetadata>(
            row_value(&value),
            "collection",
        )?);
    }
    Ok(collections)
}

pub(crate) fn collection_generation_sum(read: &ReadTransaction) -> StoreResult<u64> {
    let table = read
        .open_table(tables::COLLECTIONS)
        .map_err(map_table)?;
    let mut sum = 0u64;
    for row in table.iter().map_err(map_storage)? {
        let (_, value) = row.map_err(map_storage)?;
        let collection = value_codec::decode::<CollectionMetadata>(row_value(&value), "collection")?;
        sum = sum
            .checked_add(collection.generation)
            .ok_or_else(|| StoreError::CorruptIndex("collection generation overflow".into()))?;
    }
    Ok(sum)
}

pub fn file(view: &StoreView, id: FileId) -> StoreResult<Option<FileMetadata>> {
    let table = view.read().open_table(tables::FILES).map_err(map_table)?;
    let key = keys::file(id);
    let Some(value) = table.get(key.as_slice()).map_err(map_storage)? else {
        return Ok(None);
    };
    value_codec::decode(row_value(&value), "file").map(Some)
}

pub fn file_entry(view: &StoreView, id: FileId) -> StoreResult<Option<FileEntry>> {
    Ok(file(view, id)?.map(|metadata| metadata.entry))
}

pub fn file_by_native_path(view: &StoreView, path: &[u8]) -> StoreResult<Option<FileId>> {
    let table = view.read().open_table(tables::PATHS).map_err(map_table)?;
    let Some(value) = table.get(path).map_err(map_storage)? else {
        return Ok(None);
    };
    decode_file_id(row_value(&value))
}

pub fn file_by_query_path(view: &StoreView, path_key: &str) -> StoreResult<Option<FileId>> {
    let table = view
        .read()
        .open_table(tables::QUERY_PATHS)
        .map_err(map_table)?;
    let prefix = {
        let mut writer = KeyWriter::new();
        writer.push_str(path_key);
        writer.into_bytes()
    };
    let successor = prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    let mut range = table
        .range::<&[u8]>((Bound::Included(prefix.as_slice()), upper))
        .map_err(map_storage)?;
    let Some(row) = range.next() else {
        return Ok(None);
    };
    let (key, _) = row.map_err(map_storage)?;
    let mut cursor = Cursor::new(key.value());
    let _ = cursor.read_str()?;
    let file = cursor.read_u32()?;
    Ok(Some(FileId(file)))
}

pub fn build_state(view: &StoreView, id: FileId) -> StoreResult<Option<FileBuildState>> {
    let table = view
        .read()
        .open_table(tables::FILE_BUILD_STATE)
        .map_err(map_table)?;
    let key = keys::file(id);
    let Some(value) = table.get(key.as_slice()).map_err(map_storage)? else {
        return Ok(None);
    };
    value_codec::decode(row_value(&value), "file build state").map(Some)
}

pub fn build_state_kind(view: &StoreView, id: FileId) -> StoreResult<Option<BuildFileState>> {
    Ok(build_state(view, id)?.map(|state| state.state))
}

pub fn record(
    view: &StoreView,
    collection: CollectionId,
    slot: RecordSlot,
) -> StoreResult<Option<RecordMetadata>> {
    let table = view.read().open_table(tables::RECORDS).map_err(map_table)?;
    let key = keys::record(collection.0, slot.0);
    let Some(value) = table.get(key.as_slice()).map_err(map_storage)? else {
        return Ok(None);
    };
    value_codec::decode(row_value(&value), "record").map(Some)
}

pub fn record_entry(
    view: &StoreView,
    collection: CollectionId,
    slot: RecordSlot,
) -> StoreResult<Option<RecordEntry>> {
    Ok(record(view, collection, slot)?.map(|metadata| metadata.to_entry(slot)))
}

pub fn record_by_logical_id(
    view: &StoreView,
    collection: CollectionId,
    id: &str,
) -> StoreResult<Option<(RecordSlot, u64)>> {
    let table = view
        .read()
        .open_table(tables::LOGICAL_IDS)
        .map_err(map_table)?;
    let key = keys::logical_id(collection.0, id);
    let Some(value) = table.get(key.as_slice()).map_err(map_storage)? else {
        return Ok(None);
    };
    let mut cursor = Cursor::new(row_value(&value));
    let slot = cursor.read_u32()?;
    let incarnation = cursor.read_u64()?;
    if !cursor.is_empty() {
        return Err(StoreError::CorruptIndex(
            "trailing bytes in logical ID row".into(),
        ));
    }
    Ok(Some((RecordSlot(slot), incarnation)))
}

pub fn records_for_file(
    view: &StoreView,
    file: FileId,
) -> StoreResult<Vec<FileIdAndSlot>> {
    let table = view
        .read()
        .open_table(tables::FILE_RECORDS)
        .map_err(map_table)?;
    let prefix = keys::file_records_prefix(file);
    let successor = prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    let mut records = Vec::new();
    for row in table
        .range::<&[u8]>((Bound::Included(prefix.as_slice()), upper))
        .map_err(map_storage)?
    {
        let (key, _) = row.map_err(map_storage)?;
        let mut cursor = Cursor::new(key.value());
        let _file = cursor.read_u32()?;
        let collection = cursor.read_u32()?;
        let slot = cursor.read_u32()?;
        records.push(FileIdAndSlot {
            collection: CollectionId(collection),
            slot: RecordSlot(slot),
        });
    }
    Ok(records)
}

/// Number of records in a collection. Counts live rows; deleted slots are
/// absent from `records` and therefore are not counted.
pub fn live_record_count(view: &StoreView, collection: CollectionId) -> StoreResult<u64> {
    let table = view.read().open_table(tables::RECORDS).map_err(map_table)?;
    let prefix = keys::collection_prefix(collection.0);
    let successor = prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    let mut count = 0u64;
    for row in table
        .range::<&[u8]>((Bound::Included(prefix.as_slice()), upper))
        .map_err(map_storage)?
    {
        row.map_err(map_storage)?;
        count = count.saturating_add(1);
    }
    Ok(count)
}

/// Stream every live slot in one collection in ascending order.
pub fn for_each_slot(
    view: &StoreView,
    collection: CollectionId,
    mut visit: impl FnMut(RecordSlot, &RecordMetadata) -> StoreResult<bool>,
) -> StoreResult<()> {
    let table = view.read().open_table(tables::RECORDS).map_err(map_table)?;
    let prefix = keys::collection_prefix(collection.0);
    let successor = prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    for row in table
        .range::<&[u8]>((Bound::Included(prefix.as_slice()), upper))
        .map_err(map_storage)?
    {
        let (key, value) = row.map_err(map_storage)?;
        let mut cursor = Cursor::new(key.value());
        let _collection = cursor.read_u32()?;
        let slot = cursor.read_u32()?;
        let record = value_codec::decode::<RecordMetadata>(row_value(&value), "record")?;
        if !visit(RecordSlot(slot), &record)? {
            break;
        }
    }
    Ok(())
}

pub fn pending_files(view: &StoreView, collection: CollectionId) -> StoreResult<Vec<FileId>> {
    let table = view
        .read()
        .open_table(tables::PENDING_FILES)
        .map_err(map_table)?;
    let prefix = keys::collection_prefix(collection.0);
    let successor = prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    let mut files = Vec::new();
    for row in table
        .range::<&[u8]>((Bound::Included(prefix.as_slice()), upper))
        .map_err(map_storage)?
    {
        let (key, _) = row.map_err(map_storage)?;
        let mut cursor = Cursor::new(key.value());
        let _collection = cursor.read_u32()?;
        files.push(FileId(cursor.read_u32()?));
    }
    Ok(files)
}

pub fn pending_file_count(view: &StoreView, collection: CollectionId) -> StoreResult<u64> {
    Ok(pending_files(view, collection)?.len() as u64)
}

pub fn binding(
    view: &StoreView,
    collection: CollectionId,
    id: BindingId,
) -> StoreResult<Option<BindingDescriptor>> {
    Ok(binding_record(view, collection, id)?.map(|stored| stored.descriptor))
}

pub fn binding_record(
    view: &StoreView,
    collection: CollectionId,
    id: BindingId,
) -> StoreResult<Option<super::value_codec::StoredBinding>> {
    let table = view.read().open_table(tables::BINDINGS).map_err(map_table)?;
    let prefix = keys::collection_prefix(collection.0);
    let successor = prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    for row in table
        .range::<&[u8]>((Bound::Included(prefix.as_slice()), upper))
        .map_err(map_storage)?
    {
        let (_, value) = row.map_err(map_storage)?;
        let stored = value_codec::decode::<super::value_codec::StoredBinding>(
            row_value(&value),
            "binding",
        )?;
        if stored.id == id {
            return Ok(Some(stored));
        }
    }
    Ok(None)
}

pub fn all_bindings(
    view: &StoreView,
    collection: CollectionId,
) -> StoreResult<Vec<super::value_codec::StoredBinding>> {
    let table = view.read().open_table(tables::BINDINGS).map_err(map_table)?;
    let prefix = keys::collection_prefix(collection.0);
    let successor = prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    let mut bindings = Vec::new();
    for row in table
        .range::<&[u8]>((Bound::Included(prefix.as_slice()), upper))
        .map_err(map_storage)?
    {
        let (_, value) = row.map_err(map_storage)?;
        bindings.push(value_codec::decode::<super::value_codec::StoredBinding>(
            row_value(&value),
            "binding",
        )?);
    }
    Ok(bindings)
}

pub fn binding_by_identity(
    view: &StoreView,
    collection: CollectionId,
    identity: &[u8],
) -> StoreResult<Option<super::value_codec::StoredBinding>> {
    let table = view.read().open_table(tables::BINDINGS).map_err(map_table)?;
    let key = keys::binding_identity(collection.0, identity);
    let Some(value) = table.get(key.as_slice()).map_err(map_storage)? else {
        return Ok(None);
    };
    value_codec::decode::<super::value_codec::StoredBinding>(row_value(&value), "binding").map(Some)
}

pub fn field(
    view: &StoreView,
    collection: CollectionId,
    path: &[String],
) -> StoreResult<Option<super::value_codec::FieldDescriptor>> {
    let table = view.read().open_table(tables::FIELDS).map_err(map_table)?;
    let key = keys::field(collection.0, path);
    let Some(value) = table.get(key.as_slice()).map_err(map_storage)? else {
        return Ok(None);
    };
    value_codec::decode(row_value(&value), "field").map(Some)
}

pub fn field_coverage(
    view: &StoreView,
    collection: CollectionId,
    binding: BindingId,
) -> StoreResult<Option<FieldCoverage>> {
    let table = view
        .read()
        .open_table(tables::FIELD_COVERAGE)
        .map_err(map_table)?;
    let key = keys::binding(collection.0, binding);
    let Some(value) = table.get(key.as_slice()).map_err(map_storage)? else {
        return Ok(None);
    };
    value_codec::decode(row_value(&value), "field coverage").map(Some)
}

pub fn value_members(
    view: &StoreView,
    collection: CollectionId,
    binding: BindingId,
    value: &ScalarKey,
) -> StoreResult<Vec<RecordSlot>> {
    let table = view
        .read()
        .open_table(tables::VALUE_MEMBERS)
        .map_err(map_table)?;
    let prefix = keys::value_directory_key(collection.0, binding, value)?;
    let successor = prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    let mut slots = Vec::new();
    for row in table
        .range::<&[u8]>((Bound::Included(prefix.as_slice()), upper))
        .map_err(map_storage)?
    {
        let (key, _) = row.map_err(map_storage)?;
        let mut cursor = Cursor::new(key.value());
        let _collection = cursor.read_u32()?;
        let _binding = cursor.read_u32()?;
        let _value = cursor.read_scalar()?;
        slots.push(RecordSlot(cursor.read_u32()?));
    }
    Ok(slots)
}

/// Stream value memberships in key order. The callback receives the exact
/// scalar key and slot.
pub fn for_each_value_member(
    view: &StoreView,
    collection: CollectionId,
    binding: BindingId,
    mut visit: impl FnMut(&ScalarKey, RecordSlot) -> StoreResult<bool>,
) -> StoreResult<()> {
    let table = view
        .read()
        .open_table(tables::VALUE_MEMBERS)
        .map_err(map_table)?;
    let prefix = keys::value_prefix(collection.0, binding);
    let successor = prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    for row in table
        .range::<&[u8]>((Bound::Included(prefix.as_slice()), upper))
        .map_err(map_storage)?
    {
        let (key, _) = row.map_err(map_storage)?;
        let mut cursor = Cursor::new(key.value());
        let _collection = cursor.read_u32()?;
        let _binding = cursor.read_u32()?;
        let value = cursor.read_scalar()?;
        let slot = RecordSlot(cursor.read_u32()?);
        if !visit(&value, slot)? {
            break;
        }
    }
    Ok(())
}

pub fn value_directory_entry(
    view: &StoreView,
    collection: CollectionId,
    binding: BindingId,
    value: &ScalarKey,
) -> StoreResult<Option<ValueDirectoryEntry>> {
    let table = view
        .read()
        .open_table(tables::VALUE_DIRECTORY)
        .map_err(map_table)?;
    let key = keys::value_directory_key(collection.0, binding, value)?;
    let Some(value) = table.get(key.as_slice()).map_err(map_storage)? else {
        return Ok(None);
    };
    value_codec::decode(row_value(&value), "value directory").map(Some)
}

pub fn for_each_value_directory(
    view: &StoreView,
    collection: CollectionId,
    binding: BindingId,
    mut visit: impl FnMut(ScalarKey, ValueDirectoryEntry) -> StoreResult<bool>,
) -> StoreResult<()> {
    let table = view
        .read()
        .open_table(tables::VALUE_DIRECTORY)
        .map_err(map_table)?;
    let prefix = keys::value_prefix(collection.0, binding);
    let successor = prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    for row in table
        .range::<&[u8]>((Bound::Included(prefix.as_slice()), upper))
        .map_err(map_storage)?
    {
        let (key, value) = row.map_err(map_storage)?;
        let mut cursor = Cursor::new(key.value());
        let _collection = cursor.read_u32()?;
        let _binding = cursor.read_u32()?;
        let scalar = cursor.read_scalar()?;
        let entry =
            value_codec::decode::<ValueDirectoryEntry>(row_value(&value), "value directory")?;
        if !visit(scalar, entry)? {
            break;
        }
    }
    Ok(())
}

pub fn length_directory_entry(
    view: &StoreView,
    collection: CollectionId,
    binding: BindingId,
    length: &ScalarKey,
) -> StoreResult<Option<LengthDirectoryEntry>> {
    let table = view
        .read()
        .open_table(tables::LENGTH_DIRECTORY)
        .map_err(map_table)?;
    let key = keys::length_directory_key(collection.0, binding, length)?;
    let Some(value) = table.get(key.as_slice()).map_err(map_storage)? else {
        return Ok(None);
    };
    value_codec::decode(row_value(&value), "length directory").map(Some)
}

pub fn state_members(
    view: &StoreView,
    collection: CollectionId,
    binding: BindingId,
    state: StateKind,
) -> StoreResult<Vec<RecordSlot>> {
    let table = view
        .read()
        .open_table(tables::STATE_MEMBERS)
        .map_err(map_table)?;
    let prefix = keys::state_prefix(collection.0, binding, state);
    let successor = prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    let mut slots = Vec::new();
    for row in table
        .range::<&[u8]>((Bound::Included(prefix.as_slice()), upper))
        .map_err(map_storage)?
    {
        let (key, _) = row.map_err(map_storage)?;
        let mut cursor = Cursor::new(key.value());
        let _collection = cursor.read_u32()?;
        let _binding = cursor.read_u32()?;
        let _state = cursor.read_u8()?;
        slots.push(RecordSlot(cursor.read_u32()?));
    }
    Ok(slots)
}

pub fn coverage_members(
    view: &StoreView,
    collection: CollectionId,
    binding: BindingId,
    coverage: CoverageKind,
) -> StoreResult<Vec<RecordSlot>> {
    let table = view
        .read()
        .open_table(tables::COVERAGE_MEMBERS)
        .map_err(map_table)?;
    let mut prefix = keys::binding(collection.0, binding);
    prefix.push(coverage.tag());
    let successor = prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    let mut slots = Vec::new();
    for row in table
        .range::<&[u8]>((Bound::Included(prefix.as_slice()), upper))
        .map_err(map_storage)?
    {
        let (key, _) = row.map_err(map_storage)?;
        let mut cursor = Cursor::new(key.value());
        let _collection = cursor.read_u32()?;
        let _binding = cursor.read_u32()?;
        let _coverage = cursor.read_u8()?;
        slots.push(RecordSlot(cursor.read_u32()?));
    }
    Ok(slots)
}

pub fn domain_members(
    view: &StoreView,
    collection: CollectionId,
    domain: DomainKind,
) -> StoreResult<Vec<RecordSlot>> {
    let table = view
        .read()
        .open_table(tables::DOMAIN_MEMBERS)
        .map_err(map_table)?;
    let mut prefix = keys::collection_prefix(collection.0);
    prefix.push(domain.tag());
    let successor = prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    let mut slots = Vec::new();
    for row in table
        .range::<&[u8]>((Bound::Included(prefix.as_slice()), upper))
        .map_err(map_storage)?
    {
        let (key, _) = row.map_err(map_storage)?;
        let mut cursor = Cursor::new(key.value());
        let _collection = cursor.read_u32()?;
        let _domain = cursor.read_u8()?;
        slots.push(RecordSlot(cursor.read_u32()?));
    }
    Ok(slots)
}

pub fn fts_block(
    view: &StoreView,
    collection: CollectionId,
    binding: BindingId,
    kind: GramKind,
    gram: u64,
    block: u32,
) -> StoreResult<Option<Vec<u8>>> {
    let table = view.read().open_table(tables::FTS_BLOCKS).map_err(map_table)?;
    let key = keys::fts_key(collection.0, binding, kind, gram, block);
    let Some(value) = table.get(key.as_slice()).map_err(map_storage)? else {
        return Ok(None);
    };
    Ok(Some(row_value(&value).to_vec()))
}

pub fn gram_statistics(
    view: &StoreView,
    collection: CollectionId,
    binding: BindingId,
    kind: GramKind,
    gram: u64,
) -> StoreResult<Option<(u64, u64)>> {
    let table = view
        .read()
        .open_table(tables::FTS_GRAM_STATISTICS)
        .map_err(map_table)?;
    let key = keys::fts_statistics_key(collection.0, binding, kind, gram);
    let Some(value) = table.get(key.as_slice()).map_err(map_storage)? else {
        return Ok(None);
    };
    let mut cursor = Cursor::new(row_value(&value));
    Ok(Some((cursor.read_u64()?, cursor.read_u64()?)))
}

pub fn for_each_gram_statistics(
    view: &StoreView,
    collection: CollectionId,
    mut visit: impl FnMut(BindingId, GramKind, u64, u64, u64) -> StoreResult<bool>,
) -> StoreResult<()> {
    let table = view
        .read()
        .open_table(tables::FTS_GRAM_STATISTICS)
        .map_err(map_table)?;
    let prefix = keys::collection_prefix(collection.0);
    let successor = prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    for row in table
        .range::<&[u8]>((Bound::Included(prefix.as_slice()), upper))
        .map_err(map_storage)?
    {
        let (key, value) = row.map_err(map_storage)?;
        let mut cursor = Cursor::new(key.value());
        let _collection = cursor.read_u32()?;
        let binding = cursor.read_u32()?;
        let kind = cursor.read_u8()?;
        let gram = cursor.read_u64()?;
        let mut value_cursor = Cursor::new(row_value(&value));
        let cardinality = value_cursor.read_u64()?;
        let blocks = value_cursor.read_u64()?;
        let kind = if kind == super::tables::GRAM_SKIP {
            super::value_codec::GramKind::Skip
        } else {
            super::value_codec::GramKind::Normal
        };
        if !visit(binding, kind, gram, cardinality, blocks)? {
            break;
        }
    }
    Ok(())
}

pub fn for_each_fts_block(
    view: &StoreView,
    collection: CollectionId,
    binding: BindingId,
    kind: GramKind,
    gram: u64,
    mut visit: impl FnMut(u32, &[u8]) -> StoreResult<bool>,
) -> StoreResult<()> {
    let table = view.read().open_table(tables::FTS_BLOCKS).map_err(map_table)?;
    let prefix = keys::fts_prefix(collection.0, binding, kind, gram);
    let successor = prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    for row in table
        .range::<&[u8]>((Bound::Included(prefix.as_slice()), upper))
        .map_err(map_storage)?
    {
        let (key, value) = row.map_err(map_storage)?;
        let block = u32::from_be_bytes(
            key.value()[key.value().len() - 4..]
                .try_into()
                .expect("four-byte block"),
        );
        if !visit(block, row_value(&value))? {
            break;
        }
    }
    Ok(())
}

pub fn record_grams(
    view: &StoreView,
    collection: CollectionId,
    slot: RecordSlot,
    binding: BindingId,
    kind: GramKind,
) -> StoreResult<Option<Vec<u64>>> {
    let table = view
        .read()
        .open_table(tables::RECORD_GRAMS)
        .map_err(map_table)?;
    let key = keys::record_grams(collection.0, slot.0, binding, kind);
    let Some(value) = table.get(key.as_slice()).map_err(map_storage)? else {
        return Ok(None);
    };
    value_codec::decode_gram_list(row_value(&value)).map(Some)
}

pub fn sort_entries(
    view: &StoreView,
    collection: CollectionId,
    definition: u32,
    limit: usize,
) -> StoreResult<Vec<(String, RecordSlot, Vec<u8>)>> {
    let table = view
        .read()
        .open_table(tables::SORT_ENTRIES)
        .map_err(map_table)?;
    let prefix = keys::sort_prefix(collection.0, definition);
    let successor = prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    let mut entries = Vec::new();
    for row in table
        .range::<&[u8]>((Bound::Included(prefix.as_slice()), upper))
        .map_err(map_storage)?
    {
        if entries.len() >= limit {
            break;
        }
        let (key, _) = row.map_err(map_storage)?;
        let mut cursor = Cursor::new(key.value());
        let _collection = cursor.read_u32()?;
        let _definition = cursor.read_u32()?;
        let start = cursor.position();
        let _order = cursor.read_bytes()?;
        let order_end = cursor.position();
        let order = key.value()[start..order_end].to_vec();
        let logical_id = cursor.read_str()?;
        let slot = cursor.read_u32()?;
        entries.push((logical_id, RecordSlot(slot), order));
    }
    Ok(entries)
}

pub fn outgoing_relations(
    view: &StoreView,
    collection: CollectionId,
    declaration: u32,
) -> StoreResult<Vec<(RecordSlot, Vec<u8>)>> {
    let table = view
        .read()
        .open_table(tables::RELATION_OUTGOING)
        .map_err(map_table)?;
    let prefix = keys::relation_outgoing_prefix(collection.0, declaration);
    let successor = prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    let mut rows = Vec::new();
    for row in table
        .range::<&[u8]>((Bound::Included(prefix.as_slice()), upper))
        .map_err(map_storage)?
    {
        let (key, value) = row.map_err(map_storage)?;
        let mut cursor = Cursor::new(key.value());
        let _collection = cursor.read_u32()?;
        let _declaration = cursor.read_u32()?;
        let slot = cursor.read_u32()?;
        rows.push((RecordSlot(slot), row_value(&value).to_vec()));
    }
    Ok(rows)
}

pub fn incoming_relations(
    view: &StoreView,
    target_collection: CollectionId,
    target_id: &str,
) -> StoreResult<Vec<Vec<u8>>> {
    let table = view
        .read()
        .open_table(tables::RELATION_INCOMING)
        .map_err(map_table)?;
    let mut prefix = KeyWriter::new();
    prefix.push_u32(target_collection.0);
    prefix.push_str(target_id);
    let prefix = prefix.into_bytes();
    let successor = prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    let mut rows = Vec::new();
    for row in table
        .range::<&[u8]>((Bound::Included(prefix.as_slice()), upper))
        .map_err(map_storage)?
    {
        let (_, value) = row.map_err(map_storage)?;
        rows.push(row_value(&value).to_vec());
    }
    Ok(rows)
}

fn decode_file_id(bytes: &[u8]) -> StoreResult<Option<FileId>> {
    if bytes.len() != 4 {
        return Err(StoreError::CorruptIndex(
            "malformed path-to-file row".into(),
        ));
    }
    Ok(Some(FileId(u32::from_be_bytes(
        bytes.try_into().expect("four-byte file ID"),
    ))))
}

pub(crate) fn records_for_file_for_write(
    tx: &redb::WriteTransaction,
    file: FileId,
) -> StoreResult<Vec<FileIdAndSlot>> {
    let table = tx
        .open_table(tables::FILE_RECORDS)
        .map_err(map_table)?;
    let prefix = keys::file_records_prefix(file);
    let successor = prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    let mut records = Vec::new();
    for row in table
        .range::<&[u8]>((Bound::Included(prefix.as_slice()), upper))
        .map_err(map_storage)?
    {
        let (key, _) = row.map_err(map_storage)?;
        let mut cursor = Cursor::new(key.value());
        let _file = cursor.read_u32()?;
        let collection = cursor.read_u32()?;
        let slot = cursor.read_u32()?;
        records.push(FileIdAndSlot {
            collection: CollectionId(collection),
            slot: RecordSlot(slot),
        });
    }
    Ok(records)
}

pub(crate) fn metadata_from_write(
    tx: &redb::WriteTransaction,
) -> StoreResult<crate::index_store::value_codec::StoreMetadata> {
    let table = tx.open_table(tables::STORE_METADATA).map_err(map_table)?;
    let value = table
        .get(tables::meta::FORMAT)
        .map_err(map_storage)?
        .ok_or_else(|| StoreError::CorruptIndex("missing store metadata".into()))?;
    value_codec::decode(value.value(), "store metadata")
}

pub(crate) fn pending_files_for_write(
    tx: &redb::WriteTransaction,
    collection: CollectionId,
) -> StoreResult<Vec<FileId>> {
    let table = tx
        .open_table(tables::PENDING_FILES)
        .map_err(map_table)?;
    let prefix = keys::collection_prefix(collection.0);
    let successor = prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    let mut files = Vec::new();
    for row in table
        .range::<&[u8]>((Bound::Included(prefix.as_slice()), upper))
        .map_err(map_storage)?
    {
        let (key, _) = row.map_err(map_storage)?;
        let mut cursor = Cursor::new(key.value());
        let _collection = cursor.read_u32()?;
        files.push(FileId(cursor.read_u32()?));
    }
    Ok(files)
}
