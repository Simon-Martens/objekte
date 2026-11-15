//! Collection-scoped slot allocation and incarnation checking.
//!
//! `(collection, generation, u32 slot)` is the record namespace. Generation
//! changes on complete rebuild; slots are otherwise stable. The allocator
//! reuses the smallest free slot and advances a checked `u64` high-water
//! counter only when the free list is empty.

use std::ops::Bound;

use redb::{ReadableTable, WriteTransaction};

use crate::model::{CollectionId, RecordSlot};

use super::keys;
use super::tables;
use super::value_codec::CollectionMetadata;
use super::{StoreError, StoreResult, write};

/// Snapshot-safe record handle. The incarnation changes whenever a freed slot
/// is reused, so a stale handle is rejected even when the numeric slot matches.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SlotHandle {
    pub collection: CollectionId,
    pub generation: u64,
    pub slot: RecordSlot,
    pub incarnation: u64,
}

pub fn next_free(tx: &WriteTransaction, collection: CollectionId) -> StoreResult<Option<RecordSlot>> {
    let table = tx.open_table(tables::FREE_SLOTS).map_err(write::map_table)?;
    let prefix = keys::collection_prefix(collection.0);
    let successor = super::key_codec::prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    let mut range = table
        .range::<&[u8]>((Bound::Included(prefix.as_slice()), upper))
        .map_err(write::map_storage)?;
    let Some(row) = range.next() else {
        return Ok(None);
    };
    let (key, _) = row.map_err(write::map_storage)?;
    let mut cursor = super::key_codec::Cursor::new(key.value());
    let _collection = cursor.read_u32()?;
    Ok(Some(RecordSlot(cursor.read_u32()?)))
}

pub fn free_slot_count(tx: &WriteTransaction, collection: CollectionId) -> StoreResult<u64> {
    let keys = write::scan_prefix(tx, tables::FREE_SLOTS, &keys::collection_prefix(collection.0))?;
    Ok(keys.len() as u64)
}

pub fn release(tx: &WriteTransaction, collection: CollectionId, slot: RecordSlot) -> StoreResult<()> {
    let key = keys::free_slot(collection.0, slot.0);
    let mut table = tx.open_table(tables::FREE_SLOTS).map_err(write::map_table)?;
    table
        .insert(key.as_slice(), &[][..])
        .map_err(write::map_storage)?;
    Ok(())
}

pub fn remove_free(tx: &WriteTransaction, collection: CollectionId, slot: RecordSlot) -> StoreResult<()> {
    let key = keys::free_slot(collection.0, slot.0);
    let mut table = tx.open_table(tables::FREE_SLOTS).map_err(write::map_table)?;
    table.remove(key.as_slice()).map_err(write::map_storage)?;
    Ok(())
}

pub fn incarnation(
    tx: &WriteTransaction,
    collection: CollectionId,
    slot: RecordSlot,
) -> StoreResult<Option<u64>> {
    let key = keys::slot_incarnation(collection.0, slot.0);
    let table = tx
        .open_table(tables::SLOT_INCARNATIONS)
        .map_err(write::map_table)?;
    let Some(value) = table.get(key.as_slice()).map_err(write::map_storage)? else {
        return Ok(None);
    };
    let mut cursor = super::key_codec::Cursor::new(value.value());
    let incarnation = cursor.read_u64()?;
    if !cursor.is_empty() {
        return Err(StoreError::CorruptIndex(
            "trailing bytes in slot incarnation".into(),
        ));
    }
    Ok(Some(incarnation))
}

pub fn set_incarnation(
    tx: &WriteTransaction,
    collection: CollectionId,
    slot: RecordSlot,
    value: u64,
) -> StoreResult<()> {
    if value == 0 {
        return Err(StoreError::CorruptIndex(
            "slot incarnation must be positive".into(),
        ));
    }
    let key = keys::slot_incarnation(collection.0, slot.0);
    let mut encoded = super::key_codec::KeyWriter::with_capacity(8);
    encoded.push_u64(value);
    let mut table = tx
        .open_table(tables::SLOT_INCARNATIONS)
        .map_err(write::map_table)?;
    table
        .insert(key.as_slice(), encoded.as_slice())
        .map_err(write::map_storage)?;
    Ok(())
}

/// Allocate one slot, updating the collection's high-water counter and slot
/// incarnation in the same transaction. The caller must persist `metadata`.
pub fn allocate(
    tx: &WriteTransaction,
    metadata: &mut CollectionMetadata,
) -> StoreResult<(RecordSlot, u64)> {
    allocate_many(tx, metadata, 1).map(|mut allocated| {
        allocated
            .pop()
            .expect("one-slot allocation always returns one slot")
    })
}

/// Allocate a contiguous admission window while opening the free-list,
/// high-water and incarnation tables once. Free slots are consumed first in
/// ascending order, then the unused high-water suffix is advanced atomically.
pub fn allocate_many(
    tx: &WriteTransaction,
    metadata: &mut CollectionMetadata,
    count: usize,
) -> StoreResult<Vec<(RecordSlot, u64)>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let collection = metadata.id;
    let prefix = keys::collection_prefix(collection.0);
    let successor = super::key_codec::prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    let free = {
        let table = tx.open_table(tables::FREE_SLOTS).map_err(write::map_table)?;
        let mut range = table
            .range::<&[u8]>((Bound::Included(prefix.as_slice()), upper))
            .map_err(write::map_storage)?;
        let mut free = Vec::with_capacity(count.min(4096));
        while free.len() < count {
            let Some(row) = range.next() else {
                break;
            };
            let (key, _) = row.map_err(write::map_storage)?;
            let mut cursor = super::key_codec::Cursor::new(key.value());
            let _collection = cursor.read_u32()?;
            free.push(RecordSlot(cursor.read_u32()?));
        }
        free
    };
    if !free.is_empty() {
        let mut table = tx.open_table(tables::FREE_SLOTS).map_err(write::map_table)?;
        for slot in &free {
            let key = keys::free_slot(collection.0, slot.0);
            table.remove(key.as_slice()).map_err(write::map_storage)?;
        }
    }
    let unused = count - free.len();
    if unused > 0
        && metadata.slot_high_water.saturating_add(unused as u64) > u32::MAX as u64 + 1
    {
        return Err(StoreError::State(format!(
            "collection {} has exhausted its record slots",
            collection.0
        )));
    }
    let mut slots = free;
    slots.reserve(unused);
    for _ in 0..unused {
        slots.push(RecordSlot(metadata.slot_high_water as u32));
        metadata.slot_high_water = metadata
            .slot_high_water
            .checked_add(1)
            .ok_or_else(|| StoreError::State("slot high-water overflow".into()))?;
    }
    let mut incarnations =
        tx.open_table(tables::SLOT_INCARNATIONS).map_err(write::map_table)?;
    let mut allocated = Vec::with_capacity(slots.len());
    for slot in slots {
        let key = keys::slot_incarnation(collection.0, slot.0);
        let previous = incarnations
            .get(key.as_slice())
            .map_err(write::map_storage)?
            .map(|value| {
                let mut cursor = super::key_codec::Cursor::new(value.value());
                cursor.read_u64()
            })
            .transpose()?
            .unwrap_or(0);
        let incarnation = previous
            .checked_add(1)
            .ok_or_else(|| StoreError::State("slot incarnation overflow".into()))?;
        let mut encoded = super::key_codec::KeyWriter::with_capacity(8);
        encoded.push_u64(incarnation);
        incarnations
            .insert(key.as_slice(), encoded.as_slice())
            .map_err(write::map_storage)?;
        allocated.push((slot, incarnation));
    }
    metadata.live_records = metadata
        .live_records
        .checked_add(count as u64)
        .ok_or_else(|| StoreError::State("live record count overflow".into()))?;
    Ok(allocated)
}

pub fn release_record(
    tx: &WriteTransaction,
    metadata: &mut CollectionMetadata,
    slot: RecordSlot,
) -> StoreResult<()> {
    release(tx, metadata.id, slot)?;
    metadata.live_records = metadata
        .live_records
        .checked_sub(1)
        .ok_or_else(|| StoreError::CorruptIndex("live record count underflow".into()))?;
    Ok(())
}

/// Reject a handle that does not match the current slot incarnation.
pub fn validate_handle(
    tx: &WriteTransaction,
    handle: SlotHandle,
    current_generation: u64,
) -> StoreResult<()> {
    if handle.generation != current_generation {
        return Err(StoreError::StaleHandle(format!(
            "handle generation {} does not match current generation {current_generation}",
            handle.generation
        )));
    }
    let current = incarnation(tx, handle.collection, handle.slot)?;
    if current != Some(handle.incarnation) {
        return Err(StoreError::StaleHandle(format!(
            "slot {} incarnation {:?} does not match handle {}",
            handle.slot.0, current, handle.incarnation
        )));
    }
    Ok(())
}
