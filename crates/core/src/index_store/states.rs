//! Structural-state postings as bounded redb bitmap blocks.
//!
//! Keys are `(collection, binding, predicate, block)`. Values use the shared
//! canonical bitmap codec. `state_statistics` stores exact record cardinality
//! and nonempty-block count as two little-endian `u64` values.
//!
//! Reverse contributions store one `(binding, state mask)` per record. A
//! deletion reconstructs the affected block keys from that mask without
//! reading source content or scanning unrelated state blocks.

use std::collections::{BTreeMap, HashMap};

use redb::{ReadableTable, WriteTransaction};

use crate::model::{CollectionId, RecordSlot};

use super::bitmap_block::{Block, BLOCK_SHIFT, decode_block, encode_block};
use super::keys;
use super::tables::{STATE_BLOCKS, STATE_STATISTICS};
use super::value_codec::{BindingId, STATE_MASK_BITS, StateKind};
use super::{StoreError, StoreResult, StoreView, write};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StateStatistics {
    pub cardinality: u64,
    pub blocks: u64,
}

/// Accumulated bit changes keyed by `(binding, predicate, block)`.
type StateBlockKey = (BindingId, StateKind, u32);
type StateBlockChanges = (Vec<u16>, Vec<u16>);

#[derive(Default)]
pub struct StateBatch {
    changes: BTreeMap<StateBlockKey, StateBlockChanges>,
}

impl StateBatch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, binding: BindingId, state: StateKind, slot: u32) {
        let block = slot >> BLOCK_SHIFT;
        let offset = (slot & super::bitmap_block::BLOCK_MASK) as u16;
        self.changes
            .entry((binding, state, block))
            .or_default()
            .1
            .push(offset);
    }

    pub fn clear(&mut self, binding: BindingId, state: StateKind, slot: u32) {
        let block = slot >> BLOCK_SHIFT;
        let offset = (slot & super::bitmap_block::BLOCK_MASK) as u16;
        self.changes
            .entry((binding, state, block))
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

/// Apply a batch once per affected block. Clears and sets are coalesced so a
/// slot deleted and reused in one transaction ends in its final state.
pub fn apply_batch(
    tx: &WriteTransaction,
    collection: CollectionId,
    batch: &StateBatch,
) -> StoreResult<()> {
    let mut table = tx.open_table(STATE_BLOCKS).map_err(write::map_table)?;
    for ((binding, state, block), (clears, sets)) in &batch.changes {
        let key = keys::state_block(collection.0, *binding, *state, *block);
        let existing = table
            .get(key.as_slice())
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
        let old_nonempty = old_cardinality > 0;
        let new_nonempty = new_cardinality > 0;
        if new_nonempty {
            let encoded = encode_block(&decoded)?;
            table
                .insert(key.as_slice(), encoded.as_slice())
                .map_err(write::map_storage)?;
        } else {
            table.remove(key.as_slice()).map_err(write::map_storage)?;
        }
        drop(table);
        adjust_statistics(
            tx,
            collection,
            *binding,
            *state,
            i64::from(new_cardinality) - i64::from(old_cardinality),
            i64::from(new_nonempty) - i64::from(old_nonempty),
        )?;
        table = tx.open_table(STATE_BLOCKS).map_err(write::map_table)?;
    }
    Ok(())
}

fn adjust_statistics(
    tx: &WriteTransaction,
    collection: CollectionId,
    binding: BindingId,
    state: StateKind,
    cardinality_delta: i64,
    block_delta: i64,
) -> StoreResult<()> {
    let existing = read_statistics_tx(tx, collection, binding, state)?;
    let cardinality = if cardinality_delta >= 0 {
        existing
            .cardinality
            .checked_add(cardinality_delta as u64)
    } else {
        existing
            .cardinality
            .checked_sub((-cardinality_delta) as u64)
    }
    .ok_or_else(|| StoreError::CorruptIndex("state cardinality overflow".into()))?;
    let blocks = if block_delta >= 0 {
        existing.blocks.checked_add(block_delta as u64)
    } else {
        existing.blocks.checked_sub((-block_delta) as u64)
    }
    .ok_or_else(|| StoreError::CorruptIndex("state block count underflow".into()))?;
    if blocks == 0 {
        if cardinality != 0 {
            return Err(StoreError::CorruptIndex(
                "state cardinality without blocks".into(),
            ));
        }
        let key = keys::state_statistics(collection.0, binding, state);
        let mut table = tx.open_table(STATE_STATISTICS).map_err(write::map_table)?;
        table.remove(key.as_slice()).map_err(write::map_storage)?;
    } else {
        put_statistics_tx(tx, collection, binding, state, cardinality, blocks)?;
    }
    Ok(())
}

pub fn put_statistics_tx(
    tx: &WriteTransaction,
    collection: CollectionId,
    binding: BindingId,
    state: StateKind,
    cardinality: u64,
    blocks: u64,
) -> StoreResult<()> {
    let key = keys::state_statistics(collection.0, binding, state);
    let mut value = [0u8; 16];
    value[..8].copy_from_slice(&cardinality.to_le_bytes());
    value[8..].copy_from_slice(&blocks.to_le_bytes());
    let mut table = tx.open_table(STATE_STATISTICS).map_err(write::map_table)?;
    table
        .insert(key.as_slice(), value.as_slice())
        .map_err(write::map_storage)?;
    Ok(())
}

pub fn read_statistics_tx(
    tx: &WriteTransaction,
    collection: CollectionId,
    binding: BindingId,
    state: StateKind,
) -> StoreResult<StateStatistics> {
    let key = keys::state_statistics(collection.0, binding, state);
    let table = tx.open_table(STATE_STATISTICS).map_err(write::map_table)?;
    let Some(value) = table.get(key.as_slice()).map_err(write::map_storage)? else {
        return Ok(StateStatistics::default());
    };
    decode_statistics(value.value())
}

fn decode_statistics(bytes: &[u8]) -> StoreResult<StateStatistics> {
    if bytes.len() != 16 {
        return Err(StoreError::CorruptIndex(
            "malformed state statistics row".into(),
        ));
    }
    Ok(StateStatistics {
        cardinality: u64::from_le_bytes(bytes[..8].try_into().expect("eight-byte cardinality")),
        blocks: u64::from_le_bytes(bytes[8..].try_into().expect("eight-byte block count")),
    })
}

/// Apply exact old/new state masks for one retained slot.
pub fn apply_masks(
    tx: &WriteTransaction,
    collection: CollectionId,
    slot: RecordSlot,
    old: &[(BindingId, u16)],
    new: &[(BindingId, u16)],
) -> StoreResult<()> {
    let old = mask_map(old)?;
    let new = mask_map(new)?;
    let mut batch = StateBatch::new();
    let mut bindings = old.keys().chain(new.keys()).copied().collect::<Vec<_>>();
    bindings.sort_unstable();
    bindings.dedup();
    for binding in bindings {
        let old_mask = old.get(&binding).copied().unwrap_or(0);
        let new_mask = new.get(&binding).copied().unwrap_or(0);
        let removed = old_mask & !new_mask;
        let added = new_mask & !old_mask;
        for tag in 0..=super::tables::MAX_STATE_TAG {
            let bit = 1u16 << tag;
            let Some(state) = StateKind::from_tag(tag) else {
                continue;
            };
            if removed & bit != 0 {
                batch.clear(binding, state, slot.0);
            }
            if added & bit != 0 {
                batch.set(binding, state, slot.0);
            }
        }
    }
    apply_batch(tx, collection, &batch)
}

/// Insert all bits in newly stored masks for a new record.
pub fn insert_masks(
    tx: &WriteTransaction,
    collection: CollectionId,
    slot: RecordSlot,
    masks: &[(BindingId, u16)],
) -> StoreResult<()> {
    let mut batch = StateBatch::new();
    for (binding, mask) in masks {
        for tag in 0..=super::tables::MAX_STATE_TAG {
            let Some(state) = StateKind::from_tag(tag) else {
                continue;
            };
            if mask & (1u16 << tag) != 0 {
                batch.set(*binding, state, slot.0);
            }
        }
    }
    apply_batch(tx, collection, &batch)
}

pub fn remove_masks(
    tx: &WriteTransaction,
    collection: CollectionId,
    slot: RecordSlot,
    masks: &[(BindingId, u16)],
) -> StoreResult<()> {
    let mut batch = StateBatch::new();
    for (binding, mask) in masks {
        for tag in 0..=super::tables::MAX_STATE_TAG {
            let Some(state) = StateKind::from_tag(tag) else {
                continue;
            };
            if mask & (1u16 << tag) != 0 {
                batch.clear(*binding, state, slot.0);
            }
        }
    }
    apply_batch(tx, collection, &batch)
}

fn mask_map(masks: &[(BindingId, u16)]) -> StoreResult<HashMap<BindingId, u16>> {
    let mut map = HashMap::with_capacity(masks.len());
    for (binding, mask) in masks {
        if *mask == 0 || *mask & !STATE_MASK_BITS != 0 {
            return Err(StoreError::CorruptIndex("invalid state mask".into()));
        }
        if map.insert(*binding, *mask).is_some() {
            return Err(StoreError::CorruptIndex(
                "duplicate binding in state masks".into(),
            ));
        }
    }
    Ok(map)
}

/// Semantic membership iterator: stream every bit in ascending slot order.
pub fn state_members(
    view: &StoreView,
    collection: CollectionId,
    binding: BindingId,
    state: StateKind,
) -> StoreResult<Vec<RecordSlot>> {
    let prefix = keys::state_prefix(collection.0, binding, state);
    let successor = super::key_codec::prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(std::ops::Bound::Excluded)
        .unwrap_or(std::ops::Bound::Unbounded);
    let table = view
        .read()
        .open_table(STATE_BLOCKS)
        .map_err(write::map_table)?;
    let mut slots = Vec::new();
    for row in table
        .range::<&[u8]>((std::ops::Bound::Included(prefix.as_slice()), upper))
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

pub fn for_each_state_block(
    view: &StoreView,
    collection: CollectionId,
    binding: BindingId,
    state: StateKind,
    mut visit: impl FnMut(u32, &[u8]) -> StoreResult<bool>,
) -> StoreResult<()> {
    let prefix = keys::state_prefix(collection.0, binding, state);
    let successor = super::key_codec::prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(std::ops::Bound::Excluded)
        .unwrap_or(std::ops::Bound::Unbounded);
    let table = view
        .read()
        .open_table(STATE_BLOCKS)
        .map_err(write::map_table)?;
    for row in table
        .range::<&[u8]>((std::ops::Bound::Included(prefix.as_slice()), upper))
        .map_err(write::map_storage)?
    {
        let (key, value) = row.map_err(write::map_storage)?;
        let block = u32::from_be_bytes(
            key.value()[key.value().len() - 4..]
                .try_into()
                .expect("four-byte block"),
        );
        if !visit(block, value.value())? {
            break;
        }
    }
    Ok(())
}

pub fn state_contains(
    view: &StoreView,
    collection: CollectionId,
    binding: BindingId,
    state: StateKind,
    slot: RecordSlot,
) -> StoreResult<bool> {
    let block = slot.0 >> BLOCK_SHIFT;
    let key = keys::state_block(collection.0, binding, state, block);
    let table = view
        .read()
        .open_table(STATE_BLOCKS)
        .map_err(write::map_table)?;
    let Some(value) = table.get(key.as_slice()).map_err(write::map_storage)? else {
        return Ok(false);
    };
    let decoded = decode_block(value.value())?;
    Ok(decoded.contains((slot.0 & super::bitmap_block::BLOCK_MASK) as u16))
}

pub fn state_cardinality(
    view: &StoreView,
    collection: CollectionId,
    binding: BindingId,
    state: StateKind,
) -> StoreResult<u64> {
    let key = keys::state_statistics(collection.0, binding, state);
    let table = view
        .read()
        .open_table(STATE_STATISTICS)
        .map_err(write::map_table)?;
    let Some(value) = table.get(key.as_slice()).map_err(write::map_storage)? else {
        return Ok(0);
    };
    Ok(decode_statistics(value.value())?.cardinality)
}

pub fn for_each_statistics(
    view: &StoreView,
    collection: CollectionId,
    mut visit: impl FnMut(BindingId, StateKind, StateStatistics) -> StoreResult<bool>,
) -> StoreResult<()> {
    let prefix = keys::collection_prefix(collection.0);
    let successor = super::key_codec::prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(std::ops::Bound::Excluded)
        .unwrap_or(std::ops::Bound::Unbounded);
    let table = view
        .read()
        .open_table(STATE_STATISTICS)
        .map_err(write::map_table)?;
    for row in table
        .range::<&[u8]>((std::ops::Bound::Included(prefix.as_slice()), upper))
        .map_err(write::map_storage)?
    {
        let (key, value) = row.map_err(write::map_storage)?;
        if key.value().len() != 9 {
            return Err(StoreError::CorruptIndex(
                "malformed state statistics key".into(),
            ));
        }
        let binding = u32::from_be_bytes(key.value()[4..8].try_into().expect("four bytes"));
        let state = StateKind::from_tag(key.value()[8]).ok_or_else(|| {
            StoreError::CorruptIndex("unknown state tag in statistics".into())
        })?;
        if !visit(binding, state, decode_statistics(value.value())?)? {
            break;
        }
    }
    Ok(())
}

pub fn state_fact_count(view: &StoreView, collection: CollectionId) -> StoreResult<u64> {
    let mut total = 0u64;
    for_each_statistics(view, collection, |_, _, statistics| {
        total = total
            .checked_add(statistics.cardinality)
            .ok_or_else(|| StoreError::CorruptIndex("state fact count overflow".into()))?;
        Ok(true)
    })?;
    Ok(total)
}
#[cfg(test)]
#[path = "states.test.rs"]
mod tests;
