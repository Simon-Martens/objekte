//! Blocked FTS gram storage and snapshot-bound intersection.
//!
//! The block domain is fixed at 4,096 slots. A block value is canonical:
//!
//! ```text
//! version:      u8 = 1
//! encoding:     u8 = 0 sparse, 1 dense
//! cardinality:  u16 little-endian, 1..4096
//! payload:      sparse u16 positions or 64 little-endian u64 words
//! ```
//!
//! Sparse is mandatory below 256 positions, dense at 256 and above. Empty
//! blocks are removed rather than encoded.

use std::collections::BTreeMap;

use redb::{ReadableTable, Table, WriteTransaction};
use roaring::RoaringBitmap;

use crate::model::{CollectionId, RecordSlot};

use super::keys;
use super::bitmap_block::{BLOCK_MASK, BLOCK_SHIFT, BLOCK_SLOTS};
use super::value_codec::{BindingId, GramKind, GramProjection};
use super::{StoreError, StoreResult, StoreView, read, write};

pub use super::bitmap_block::{Block, decode_block, encode_block};

/// Accumulated bit changes for one admission or mutation batch, keyed by
/// `(binding, gram kind, gram, block)`.
type BlockKey = (BindingId, GramKind, u64, u32);
type BlockChanges = (Vec<u16>, Vec<u16>);

#[derive(Default)]
pub struct FtsBatch {
    changes: BTreeMap<BlockKey, BlockChanges>,
}

impl FtsBatch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, binding: BindingId, kind: GramKind, gram: u64, slot: u32) {
        let block = slot >> BLOCK_SHIFT;
        let offset = (slot & BLOCK_MASK) as u16;
        self.changes
            .entry((binding, kind, gram, block))
            .or_default()
            .1
            .push(offset);
    }

    pub fn clear(&mut self, binding: BindingId, kind: GramKind, gram: u64, slot: u32) {
        let block = slot >> BLOCK_SHIFT;
        let offset = (slot & BLOCK_MASK) as u16;
        self.changes
            .entry((binding, kind, gram, block))
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

/// Apply every accumulated change once per block. Statistics follow the true
/// popcount difference and the zero/non-zero block transition.
pub fn apply_batch(
    tx: &WriteTransaction,
    collection: CollectionId,
    batch: &FtsBatch,
) -> StoreResult<()> {
    let mut table = tx.open_table(super::tables::FTS_BLOCKS).map_err(write::map_table)?;
    let mut statistics = tx
        .open_table(super::tables::FTS_GRAM_STATISTICS)
        .map_err(write::map_table)?;
    let mut statistic_deltas =
        std::collections::BTreeMap::<(BindingId, GramKind, u64), (i64, i64)>::new();
    for ((binding, kind, gram, block), (clears, sets)) in &batch.changes {
        let key = keys::fts_key(collection.0, *binding, *kind, *gram, *block);
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
        let delta = statistic_deltas
            .entry((*binding, *kind, *gram))
            .or_default();
        delta.0 += i64::from(new_cardinality) - i64::from(old_cardinality);
        delta.1 += i64::from(new_nonempty) - i64::from(old_nonempty);
    }
    for ((binding, kind, gram), (cardinality_delta, block_delta)) in statistic_deltas {
        adjust_statistics(
            &mut statistics,
            collection,
            binding,
            kind,
            gram,
            cardinality_delta,
            block_delta,
        )?;
    }
    Ok(())
}

fn adjust_statistics(
    table: &mut Table<'_, &'static [u8], &'static [u8]>,
    collection: CollectionId,
    binding: BindingId,
    kind: GramKind,
    gram: u64,
    cardinality_delta: i64,
    block_delta: i64,
) -> StoreResult<()> {
    let key = keys::fts_statistics_key(collection.0, binding, kind, gram);
    let existing = table
        .get(key.as_slice())
        .map_err(write::map_storage)?
        .map(|value| {
            let mut cursor = super::key_codec::Cursor::new(value.value());
            Ok::<_, StoreError>((cursor.read_u64()?, cursor.read_u64()?))
        })
        .transpose()?;
    let (cardinality, blocks) = existing.unwrap_or((0, 0));
    let cardinality = if cardinality_delta >= 0 {
        cardinality.checked_add(cardinality_delta as u64)
    } else {
        cardinality.checked_sub((-cardinality_delta) as u64)
    }
    .ok_or_else(|| StoreError::CorruptIndex("FTS gram cardinality overflow".into()))?;
    let blocks = if block_delta >= 0 {
        blocks.checked_add(block_delta as u64)
    } else {
        blocks.checked_sub((-block_delta) as u64)
    }
    .ok_or_else(|| StoreError::CorruptIndex("FTS block count underflow".into()))?;
    if blocks == 0 {
        if cardinality != 0 {
            return Err(StoreError::CorruptIndex(
                "FTS gram has cardinality without blocks".into(),
            ));
        }
        table.remove(key.as_slice()).map_err(write::map_storage)?;
    } else {
        let mut value = super::key_codec::KeyWriter::with_capacity(16);
        value.push_u64(cardinality);
        value.push_u64(blocks);
        table
            .insert(key.as_slice(), value.as_slice())
            .map_err(write::map_storage)?;
    }
    Ok(())
}

pub fn insert_record_grams(
    tx: &WriteTransaction,
    collection: CollectionId,
    slot: RecordSlot,
    projections: &[GramProjection],
) -> StoreResult<()> {
    let mut batch = FtsBatch::new();
    for projection in projections {
        for gram in &projection.grams {
            batch.set(projection.binding, projection.kind, *gram, slot.0);
        }
    }
    apply_batch(tx, collection, &batch)?;
    let mut table = tx.open_table(super::tables::RECORD_GRAMS).map_err(write::map_table)?;
    for projection in projections {
        let key = keys::record_grams(collection.0, slot.0, projection.binding, projection.kind);
        let encoded = super::value_codec::encode_gram_list(&projection.grams);
        table
            .insert(key.as_slice(), encoded.as_slice())
            .map_err(write::map_storage)?;
    }
    Ok(())
}

pub fn remove_record_grams(
    tx: &WriteTransaction,
    collection: CollectionId,
    slot: RecordSlot,
    projections: &[GramProjection],
) -> StoreResult<()> {
    let mut batch = FtsBatch::new();
    for projection in projections {
        for gram in &projection.grams {
            batch.clear(projection.binding, projection.kind, *gram, slot.0);
        }
    }
    apply_batch(tx, collection, &batch)?;
    let mut table = tx.open_table(super::tables::RECORD_GRAMS).map_err(write::map_table)?;
    for projection in projections {
        let key = keys::record_grams(collection.0, slot.0, projection.binding, projection.kind);
        table.remove(key.as_slice()).map_err(write::map_storage)?;
    }
    Ok(())
}

/// Snapshot-bound gram intersection.
///
/// Selects the gram with the smallest cardinality as the driver, streams its
/// non-empty blocks and intersects the other grams block by block. A missing
/// gram in a complete binding yields an empty candidate set; `None` means the
/// request had no usable grams (no safe restriction).
pub fn candidates(
    view: &StoreView,
    collection: CollectionId,
    binding: BindingId,
    kind: GramKind,
    grams: &[u64],
    restriction: Option<&RoaringBitmap>,
) -> StoreResult<Option<RoaringBitmap>> {
    if grams.is_empty() {
        return Ok(None);
    }
    let mut requested = grams.to_vec();
    requested.sort_unstable();
    requested.dedup();
    let mut ordered = Vec::with_capacity(requested.len());
    for gram in requested {
        let statistics = read::gram_statistics(view, collection, binding, kind, gram)?;
        // A missing required gram is an empty candidate set in a complete
        // binding. The caller has already checked binding coverage.
        let Some((cardinality, _)) = statistics else {
            return Ok(Some(RoaringBitmap::new()));
        };
        if cardinality == 0 {
            return Ok(Some(RoaringBitmap::new()));
        }
        ordered.push((cardinality, gram));
    }
    ordered.sort_unstable();
    let driver = ordered[0].1;
    let mut result = RoaringBitmap::new();
    let mut driver_blocks = Vec::new();
    read::for_each_fts_block(view, collection, binding, kind, driver, |block, bytes| {
        driver_blocks.push((block, bytes.to_vec()));
        Ok(true)
    })?;
    for (block_number, bytes) in driver_blocks {
        let mut block = decode_block(&bytes)?;
        if let Some(restriction) = restriction {
            let mut restricted = RoaringBitmap::new();
            let start = block_number << BLOCK_SHIFT;
            for slot in restriction.range(start..start + BLOCK_SLOTS) {
                let offset = (slot & BLOCK_MASK) as u16;
                if block.contains(offset) {
                    restricted.insert(slot);
                }
            }
            if restricted.is_empty() {
                continue;
            }
            block = Block::default();
            for slot in restricted.iter() {
                block.set((slot & BLOCK_MASK) as u16);
            }
        }
        for (_, gram) in &ordered[1..] {
            let other = read::fts_block(view, collection, binding, kind, *gram, block_number)?;
            let Some(other) = other else {
                block = Block::default();
                break;
            };
            let other = decode_block(&other)?;
            block.intersect(&other);
            if block.is_empty() {
                break;
            }
        }
        block.insert_global(block_number, &mut result);
    }
    Ok(Some(result))
}

#[cfg(test)]
#[path = "fts.test.rs"]
mod tests;
