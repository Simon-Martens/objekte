//! Bounded exact-value and exact-length posting blocks.
//!
//! Keys are `(collection, binding, ordered scalar key, block)` and values use
//! the shared canonical 4,096-slot bitmap codec. One block belongs to exactly
//! one value or length. Directories keep exact logical record cardinalities.

use std::collections::BTreeMap;
use std::ops::Bound;

use redb::{ReadableTable, WriteTransaction};

use crate::model::{CollectionId, RecordSlot};
use crate::posting::ScalarKey;

use super::bitmap_block::{Block, BLOCK_MASK, BLOCK_SHIFT, decode_block, encode_block};
use super::keys;
use super::tables::{LENGTH_BLOCKS, VALUE_BLOCKS};
use super::value_codec::BindingId;
use super::{StoreResult, StoreView, write};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PostingFamily {
    Value,
    Length,
}

impl PostingFamily {
    fn table(self) -> super::tables::ByteTable {
        match self {
            Self::Value => VALUE_BLOCKS,
            Self::Length => LENGTH_BLOCKS,
        }
    }
}

fn block_key(
    _family: PostingFamily,
    collection: CollectionId,
    binding: BindingId,
    key: &ScalarKey,
    block: u32,
) -> StoreResult<Vec<u8>> {
    let mut writer = super::key_codec::KeyWriter::with_capacity(48);
    writer.push_u32(collection.0);
    writer.push_u32(binding);
    writer.push_scalar(key)?;
    writer.push_u32(block);
    Ok(writer.into_bytes())
}

fn exact_prefix(
    collection: CollectionId,
    binding: BindingId,
    key: &ScalarKey,
) -> StoreResult<Vec<u8>> {
    let mut writer = super::key_codec::KeyWriter::with_capacity(44);
    writer.push_u32(collection.0);
    writer.push_u32(binding);
    writer.push_scalar(key)?;
    Ok(writer.into_bytes())
}

pub fn insert_member(
    tx: &WriteTransaction,
    family: PostingFamily,
    collection: CollectionId,
    binding: BindingId,
    key: &ScalarKey,
    slot: RecordSlot,
) -> StoreResult<bool> {
    let block_number = slot.0 >> BLOCK_SHIFT;
    let offset = (slot.0 & BLOCK_MASK) as u16;
    let block_key = block_key(family, collection, binding, key, block_number)?;
    let mut table = tx.open_table(family.table()).map_err(write::map_table)?;
    let existing = table
        .get(block_key.as_slice())
        .map_err(write::map_storage)?
        .map(|guard| decode_block(guard.value()))
        .transpose()?;
    let mut block = existing.unwrap_or_default();
    if block.contains(offset) {
        return Ok(false);
    }
    block.set(offset);
    let encoded = encode_block(&block)?;
    table
        .insert(block_key.as_slice(), encoded.as_slice())
        .map_err(write::map_storage)?;
    drop(table);
    adjust_directory(tx, family, collection, binding, key, 1)?;
    Ok(true)
}

pub fn remove_member(
    tx: &WriteTransaction,
    family: PostingFamily,
    collection: CollectionId,
    binding: BindingId,
    key: &ScalarKey,
    slot: RecordSlot,
) -> StoreResult<bool> {
    let block_number = slot.0 >> BLOCK_SHIFT;
    let offset = (slot.0 & BLOCK_MASK) as u16;
    let block_key = block_key(family, collection, binding, key, block_number)?;
    let mut table = tx.open_table(family.table()).map_err(write::map_table)?;
    let existing = table
        .get(block_key.as_slice())
        .map_err(write::map_storage)?
        .map(|guard| decode_block(guard.value()))
        .transpose()?;
    let Some(mut block) = existing else {
        return Ok(false);
    };
    if !block.contains(offset) {
        return Ok(false);
    }
    block.clear(offset);
    if block.is_empty() {
        table.remove(block_key.as_slice()).map_err(write::map_storage)?;
    } else {
        let encoded = encode_block(&block)?;
        table
            .insert(block_key.as_slice(), encoded.as_slice())
            .map_err(write::map_storage)?;
    }
    drop(table);
    adjust_directory(tx, family, collection, binding, key, -1)?;
    Ok(true)
}

fn adjust_directory(
    tx: &WriteTransaction,
    family: PostingFamily,
    collection: CollectionId,
    binding: BindingId,
    key: &ScalarKey,
    delta: i64,
) -> StoreResult<()> {
    match family {
        PostingFamily::Value => write::adjust_value_directory(tx, collection, binding, key, delta),
        PostingFamily::Length => {
            write::adjust_length_directory(tx, collection, binding, key, delta)
        }
    }
}

/// Accumulated value/length bit changes for one admission batch or collection
/// import. Every affected block is read and written once.
type PostingBlockKey = (PostingFamily, BindingId, ScalarKey, u32);
type BlockChanges = (Vec<u16>, Vec<u16>);

#[derive(Default)]
pub struct PostingBatch {
    changes: BTreeMap<PostingBlockKey, BlockChanges>,
}

impl PostingBatch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(
        &mut self,
        family: PostingFamily,
        binding: BindingId,
        key: &ScalarKey,
        slot: RecordSlot,
    ) {
        let block = slot.0 >> BLOCK_SHIFT;
        let offset = (slot.0 & BLOCK_MASK) as u16;
        self.changes
            .entry((family, binding, key.clone(), block))
            .or_default()
            .1
            .push(offset);
    }

    pub fn clear(
        &mut self,
        family: PostingFamily,
        binding: BindingId,
        key: &ScalarKey,
        slot: RecordSlot,
    ) {
        let block = slot.0 >> BLOCK_SHIFT;
        let offset = (slot.0 & BLOCK_MASK) as u16;
        self.changes
            .entry((family, binding, key.clone(), block))
            .or_default()
            .0
            .push(offset);
    }

    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    pub fn changed_blocks(&self) -> usize {
        self.changes.len()
    }
}

pub fn apply_batch(
    tx: &WriteTransaction,
    collection: CollectionId,
    batch: &PostingBatch,
) -> StoreResult<()> {
    let mut tables = [
        tx.open_table(VALUE_BLOCKS).map_err(write::map_table)?,
        tx.open_table(LENGTH_BLOCKS).map_err(write::map_table)?,
    ];
    for ((family, binding, key, block), (clears, sets)) in &batch.changes {
        let table = match family {
            PostingFamily::Value => &mut tables[0],
            PostingFamily::Length => &mut tables[1],
        };
        let block_key = block_key(*family, collection, *binding, key, *block)?;
        let existing = table
            .get(block_key.as_slice())
            .map_err(write::map_storage)?
            .map(|guard| decode_block(guard.value()))
            .transpose()?;
        let mut decoded = existing.clone().unwrap_or_default();
        let mut changed = false;
        for offset in clears {
            if decoded.contains(*offset) {
                decoded.clear(*offset);
                changed = true;
            }
        }
        for offset in sets {
            if !decoded.contains(*offset) {
                decoded.set(*offset);
                changed = true;
            }
        }
        if !changed {
            continue;
        }
        let old_cardinality = existing.as_ref().map_or(0, Block::cardinality);
        let new_cardinality = decoded.cardinality();
        if new_cardinality == 0 {
            table.remove(block_key.as_slice()).map_err(write::map_storage)?;
        } else {
            let encoded = encode_block(&decoded)?;
            table
                .insert(block_key.as_slice(), encoded.as_slice())
                .map_err(write::map_storage)?;
        }
        adjust_directory(
            tx,
            *family,
            collection,
            *binding,
            key,
            i64::from(new_cardinality) - i64::from(old_cardinality),
        )?;
    }
    Ok(())
}

/// Enumerate every member of one exact value/length in ascending slot order.
pub fn members(
    view: &StoreView,
    family: PostingFamily,
    collection: CollectionId,
    binding: BindingId,
    key: &ScalarKey,
) -> StoreResult<Vec<RecordSlot>> {
    let prefix = exact_prefix(collection, binding, key)?;
    let successor = super::key_codec::prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    let table = view
        .read()
        .open_table(family.table())
        .map_err(write::map_table)?;
    let mut slots = Vec::new();
    for row in table
        .range::<&[u8]>((Bound::Included(prefix.as_slice()), upper))
        .map_err(write::map_storage)?
    {
        let (key, value) = row.map_err(write::map_storage)?;
        let block = u32::from_be_bytes(
            key.value()[key.value().len() - 4..]
                .try_into()
                .expect("four-byte block"),
        );
        let decoded = decode_block(value.value())?;
        decoded.for_each(|offset| {
            slots.push(RecordSlot((block << BLOCK_SHIFT) | u32::from(offset)));
        });
    }
    Ok(slots)
}

pub fn contains(
    view: &StoreView,
    family: PostingFamily,
    collection: CollectionId,
    binding: BindingId,
    key: &ScalarKey,
    slot: RecordSlot,
) -> StoreResult<bool> {
    let block = slot.0 >> BLOCK_SHIFT;
    let block_key = block_key(family, collection, binding, key, block)?;
    let table = view
        .read()
        .open_table(family.table())
        .map_err(write::map_table)?;
    let Some(value) = table.get(block_key.as_slice()).map_err(write::map_storage)? else {
        return Ok(false);
    };
    Ok(decode_block(value.value())?.contains((slot.0 & BLOCK_MASK) as u16))
}

pub fn block_count(
    view: &StoreView,
    family: PostingFamily,
    collection: CollectionId,
    binding: BindingId,
    key: &ScalarKey,
) -> StoreResult<u64> {
    let prefix = exact_prefix(collection, binding, key)?;
    let successor = super::key_codec::prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    let table = view
        .read()
        .open_table(family.table())
        .map_err(write::map_table)?;
    let mut count = 0u64;
    for row in table
        .range::<&[u8]>((Bound::Included(prefix.as_slice()), upper))
        .map_err(write::map_storage)?
    {
        row.map_err(write::map_storage)?;
        count += 1;
    }
    Ok(count)
}

/// Stream all members of one family in `(scalar key, block)` order.
pub fn for_each_member(
    view: &StoreView,
    family: PostingFamily,
    collection: CollectionId,
    binding: BindingId,
    mut visit: impl FnMut(&ScalarKey, RecordSlot) -> StoreResult<bool>,
) -> StoreResult<()> {
    let prefix = keys::binding(collection.0, binding);
    let successor = super::key_codec::prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    let table = view
        .read()
        .open_table(family.table())
        .map_err(write::map_table)?;
    for row in table
        .range::<&[u8]>((Bound::Included(prefix.as_slice()), upper))
        .map_err(write::map_storage)?
    {
        let (key, value) = row.map_err(write::map_storage)?;
        let mut cursor = super::key_codec::Cursor::new(key.value());
        let _collection = cursor.read_u32()?;
        let _binding = cursor.read_u32()?;
        let scalar = cursor.read_scalar()?;
        let block = cursor.read_u32()?;
        let decoded = decode_block(value.value())?;
        let mut stop = false;
        decoded.for_each(|offset| {
            if stop {
                return;
            }
            let slot = RecordSlot((block << BLOCK_SHIFT) | u32::from(offset));
            match visit(&scalar, slot) {
                Ok(true) => {}
                Ok(false) => stop = true,
                Err(_) => stop = true,
            }
        });
    }
    Ok(())
}

/// Validate that a directory count equals the sum of block popcounts.
pub fn validate_directory(
    view: &StoreView,
    family: PostingFamily,
    collection: CollectionId,
    binding: BindingId,
    key: &ScalarKey,
    expected: u64,
) -> StoreResult<Option<String>> {
    let prefix = exact_prefix(collection, binding, key)?;
    let successor = super::key_codec::prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    let table = view
        .read()
        .open_table(family.table())
        .map_err(write::map_table)?;
    let mut count = 0u64;
    for row in table
        .range::<&[u8]>((Bound::Included(prefix.as_slice()), upper))
        .map_err(write::map_storage)?
    {
        let (_, value) = row.map_err(write::map_storage)?;
        let decoded = decode_block(value.value())?;
        count = count.saturating_add(u64::from(decoded.cardinality()));
    }
    if count != expected {
        return Ok(Some(format!(
            "directory count {expected} != block popcount {count}"
        )));
    }
    Ok(None)
}

// Keep the semantic read facade names available to callers that used the
// individual-row tables before A2.
pub fn value_members(
    view: &StoreView,
    collection: CollectionId,
    binding: BindingId,
    value: &ScalarKey,
) -> StoreResult<Vec<RecordSlot>> {
    members(view, PostingFamily::Value, collection, binding, value)
}

pub fn length_members(
    view: &StoreView,
    collection: CollectionId,
    binding: BindingId,
    length: &ScalarKey,
) -> StoreResult<Vec<RecordSlot>> {
    members(view, PostingFamily::Length, collection, binding, length)
}

#[cfg(test)]
#[path = "posting_blocks.test.rs"]
mod tests;
