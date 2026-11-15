//! Individual value/length membership rows (A2 control variant).
//!
//! Compiled with the `store-posting-rows` feature for the corrected A1
//! control and reverse-compression variants. The final selected A2 candidate
//! uses bounded blocks instead.

use std::ops::Bound;

use redb::{ReadableTable, WriteTransaction};

use crate::model::{CollectionId, RecordSlot};
use crate::posting::ScalarKey;

use super::keys;
use super::tables::{LENGTH_MEMBERS, VALUE_MEMBERS};
use super::value_codec::BindingId;
use super::{StoreResult, StoreView, write};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostingFamily {
    Value,
    Length,
}

fn table(family: PostingFamily) -> super::tables::ByteTable {
    match family {
        PostingFamily::Value => VALUE_MEMBERS,
        PostingFamily::Length => LENGTH_MEMBERS,
    }
}

pub fn insert_member(
    tx: &WriteTransaction,
    family: PostingFamily,
    collection: CollectionId,
    binding: BindingId,
    value: &ScalarKey,
    slot: RecordSlot,
) -> StoreResult<bool> {
    let key = keys::value_member(collection.0, binding, value, slot.0)?;
    let mut members = tx.open_table(table(family)).map_err(write::map_table)?;
    let inserted = members
        .insert(key.as_slice(), &[][..])
        .map_err(write::map_storage)?
        .is_none();
    drop(members);
    if inserted {
        match family {
            PostingFamily::Value => {
                write::adjust_value_directory(tx, collection, binding, value, 1)?
            }
            PostingFamily::Length => {
                write::adjust_length_directory(tx, collection, binding, value, 1)?
            }
        }
    }
    Ok(inserted)
}

pub fn remove_member(
    tx: &WriteTransaction,
    family: PostingFamily,
    collection: CollectionId,
    binding: BindingId,
    value: &ScalarKey,
    slot: RecordSlot,
) -> StoreResult<bool> {
    let key = keys::value_member(collection.0, binding, value, slot.0)?;
    let mut members = tx.open_table(table(family)).map_err(write::map_table)?;
    let removed = members
        .remove(key.as_slice())
        .map_err(write::map_storage)?
        .is_some();
    drop(members);
    if removed {
        match family {
            PostingFamily::Value => {
                write::adjust_value_directory(tx, collection, binding, value, -1)?
            }
            PostingFamily::Length => {
                write::adjust_length_directory(tx, collection, binding, value, -1)?
            }
        }
    }
    Ok(removed)
}

pub fn members(
    view: &StoreView,
    family: PostingFamily,
    collection: CollectionId,
    binding: BindingId,
    value: &ScalarKey,
) -> StoreResult<Vec<RecordSlot>> {
    let prefix = keys::value_directory_key(collection.0, binding, value)?;
    let successor = super::key_codec::prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    let table = view
        .read()
        .open_table(table(family))
        .map_err(write::map_table)?;
    let mut slots = Vec::new();
    for row in table
        .range::<&[u8]>((Bound::Included(prefix.as_slice()), upper))
        .map_err(write::map_storage)?
    {
        let (key, _) = row.map_err(write::map_storage)?;
        let mut cursor = super::key_codec::Cursor::new(key.value());
        let _collection = cursor.read_u32()?;
        let _binding = cursor.read_u32()?;
        let _value = cursor.read_scalar()?;
        slots.push(RecordSlot(cursor.read_u32()?));
    }
    Ok(slots)
}

/// Count exact membership rows for a value/length.
pub fn validate_directory(
    view: &StoreView,
    family: PostingFamily,
    collection: CollectionId,
    binding: BindingId,
    value: &ScalarKey,
    expected: u64,
) -> StoreResult<Option<String>> {
    let count = members(view, family, collection, binding, value)?.len() as u64;
    if count != expected {
        return Ok(Some(format!(
            "directory count {expected} != membership rows {count}"
        )));
    }
    Ok(None)
}

pub fn contains(
    view: &StoreView,
    family: PostingFamily,
    collection: CollectionId,
    binding: BindingId,
    value: &ScalarKey,
    slot: RecordSlot,
) -> StoreResult<bool> {
    let key = keys::value_member(collection.0, binding, value, slot.0)?;
    let table = view
        .read()
        .open_table(table(family))
        .map_err(write::map_table)?;
    Ok(table.get(key.as_slice()).map_err(write::map_storage)?.is_some())
}
