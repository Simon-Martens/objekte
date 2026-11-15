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

use redb::{ReadableTable, WriteTransaction};
use roaring::RoaringBitmap;

use crate::model::{CollectionId, RecordSlot};

use super::keys;
use super::tables::{
    FTS_BLOCK_MASK, FTS_BLOCK_SHIFT, FTS_BLOCK_SLOTS, FTS_MAX_CARDINALITY, FTS_SPARSE_LIMIT,
};
use super::value_codec::{BindingId, GramKind, GramProjection};
use super::{StoreError, StoreResult, StoreView, read, write};

const BLOCK_WORDS: usize = 64;
const DENSE_PAYLOAD_BYTES: usize = BLOCK_WORDS * 8;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Block {
    words: [u64; BLOCK_WORDS],
}

impl Default for Block {
    fn default() -> Self {
        Self {
            words: [0; BLOCK_WORDS],
        }
    }
}

impl Block {
    pub fn contains(&self, offset: u16) -> bool {
        let index = usize::from(offset);
        self.words[index / 64] & (1u64 << (index % 64)) != 0
    }

    pub fn set(&mut self, offset: u16) {
        let index = usize::from(offset);
        self.words[index / 64] |= 1u64 << (index % 64);
    }

    pub fn clear(&mut self, offset: u16) {
        let index = usize::from(offset);
        self.words[index / 64] &= !(1u64 << (index % 64));
    }

    pub fn cardinality(&self) -> u16 {
        self.words.iter().map(|word| word.count_ones() as u16).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.words.iter().all(|word| *word == 0)
    }

    pub fn intersect(&mut self, other: &Block) {
        for (left, right) in self.words.iter_mut().zip(&other.words) {
            *left &= *right;
        }
    }

    pub fn for_each(&self, mut visit: impl FnMut(u16)) {
        for (word_index, word) in self.words.iter().copied().enumerate() {
            let mut word = word;
            while word != 0 {
                let bit = word.trailing_zeros();
                visit((word_index * 64 + bit as usize) as u16);
                word &= word - 1;
            }
        }
    }

    pub fn slots(&self, block: u32, out: &mut RoaringBitmap) {
        self.for_each(|offset| {
            let slot = (block << FTS_BLOCK_SHIFT) | u32::from(offset);
            out.insert(slot);
        });
    }
}

pub fn encode_block(block: &Block) -> StoreResult<Vec<u8>> {
    let cardinality = block.cardinality();
    if cardinality == 0 {
        return Err(StoreError::State("cannot encode an empty FTS block".into()));
    }
    let mut bytes = Vec::with_capacity(DENSE_PAYLOAD_BYTES + 4);
    bytes.push(1);
    if cardinality <= FTS_SPARSE_LIMIT {
        bytes.push(0);
        bytes.extend_from_slice(&cardinality.to_le_bytes());
        block.for_each(|offset| bytes.extend_from_slice(&offset.to_le_bytes()));
    } else {
        bytes.push(1);
        bytes.extend_from_slice(&cardinality.to_le_bytes());
        for word in block.words {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
    }
    debug_assert!(bytes.len() <= 516);
    Ok(bytes)
}

pub fn decode_block(bytes: &[u8]) -> StoreResult<Block> {
    let version = *bytes
        .first()
        .ok_or_else(|| StoreError::CorruptIndex("empty FTS block".into()))?;
    if version != 1 {
        return Err(StoreError::CorruptIndex(format!(
            "unknown FTS block version {version}"
        )));
    }
    let encoding = *bytes
        .get(1)
        .ok_or_else(|| StoreError::CorruptIndex("truncated FTS block header".into()))?;
    let cardinality_bytes = bytes
        .get(2..4)
        .ok_or_else(|| StoreError::CorruptIndex("truncated FTS cardinality".into()))?;
    let cardinality = u16::from_le_bytes(
        cardinality_bytes
            .try_into()
            .expect("two-byte cardinality"),
    );
    if cardinality == 0 || cardinality > FTS_MAX_CARDINALITY {
        return Err(StoreError::CorruptIndex(format!(
            "invalid FTS block cardinality {cardinality}"
        )));
    }
    let payload = &bytes[4..];
    let mut block = Block::default();
    match encoding {
        0 => {
            if cardinality > FTS_SPARSE_LIMIT {
                return Err(StoreError::CorruptIndex(
                    "sparse FTS block above the dense threshold".into(),
                ));
            }
            if payload.len() != usize::from(cardinality) * 2 {
                return Err(StoreError::CorruptIndex(
                    "sparse FTS payload length mismatch".into(),
                ));
            }
            let mut previous = None;
            for chunk in payload.chunks(2) {
                let offset = u16::from_le_bytes(chunk.try_into().expect("two-byte offset"));
                if offset >= FTS_BLOCK_SLOTS as u16
                    || previous.is_some_and(|previous| offset <= previous)
                {
                    return Err(StoreError::CorruptIndex(
                        "noncanonical sparse FTS positions".into(),
                    ));
                }
                previous = Some(offset);
                block.set(offset);
            }
        }
        1 => {
            if cardinality <= FTS_SPARSE_LIMIT {
                return Err(StoreError::CorruptIndex(
                    "dense FTS block below the sparse threshold".into(),
                ));
            }
            if payload.len() != DENSE_PAYLOAD_BYTES {
                return Err(StoreError::CorruptIndex(
                    "dense FTS payload length mismatch".into(),
                ));
            }
            for (index, chunk) in payload.chunks(8).enumerate() {
                block.words[index] =
                    u64::from_le_bytes(chunk.try_into().expect("eight-byte word"));
            }
            if block.cardinality() != cardinality {
                return Err(StoreError::CorruptIndex(
                    "dense FTS popcount does not match its header".into(),
                ));
            }
        }
        other => {
            return Err(StoreError::CorruptIndex(format!(
                "unknown FTS block encoding {other}"
            )));
        }
    }
    if block.cardinality() != cardinality {
        return Err(StoreError::CorruptIndex(
            "FTS block bit count does not match its header".into(),
        ));
    }
    Ok(block)
}

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
        let block = slot >> FTS_BLOCK_SHIFT;
        let offset = (slot & FTS_BLOCK_MASK) as u16;
        self.changes
            .entry((binding, kind, gram, block))
            .or_default()
            .1
            .push(offset);
    }

    pub fn clear(&mut self, binding: BindingId, kind: GramKind, gram: u64, slot: u32) {
        let block = slot >> FTS_BLOCK_SHIFT;
        let offset = (slot & FTS_BLOCK_MASK) as u16;
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
        drop(table);
        adjust_statistics(
            tx,
            collection,
            *binding,
            *kind,
            *gram,
            i64::from(new_cardinality) - i64::from(old_cardinality),
            i64::from(new_nonempty) - i64::from(old_nonempty),
        )?;
        table = tx.open_table(super::tables::FTS_BLOCKS).map_err(write::map_table)?;
    }
    Ok(())
}

fn adjust_statistics(
    tx: &WriteTransaction,
    collection: CollectionId,
    binding: BindingId,
    kind: GramKind,
    gram: u64,
    cardinality_delta: i64,
    block_delta: i64,
) -> StoreResult<()> {
    let existing = write::get_fts_statistics(tx, collection, binding, kind, gram)?;
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
        write::remove_fts_statistics(tx, collection, binding, kind, gram)?;
    } else {
        write::put_fts_statistics(
            tx,
            collection,
            binding,
            kind,
            gram,
            cardinality,
            blocks,
        )?;
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
            let start = block_number << FTS_BLOCK_SHIFT;
            for slot in restriction.range(start..start + FTS_BLOCK_SLOTS) {
                let offset = (slot & FTS_BLOCK_MASK) as u16;
                if block.contains(offset) {
                    restricted.insert(slot);
                }
            }
            if restricted.is_empty() {
                continue;
            }
            block = Block::default();
            for slot in restricted.iter() {
                block.set((slot & FTS_BLOCK_MASK) as u16);
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
        block.slots(block_number, &mut result);
    }
    Ok(Some(result))
}

#[cfg(test)]
#[path = "fts.test.rs"]
mod tests;
