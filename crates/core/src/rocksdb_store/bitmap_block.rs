//! Shared canonical 4,096-slot bitmap block codec.
//!
//! One block covers exactly 4,096 record slots. The value format is the
//! existing FTS version-1 format:
//!
//! ```text
//! version:      u8 = 1
//! encoding:     u8 = 0 sparse, 1 dense
//! cardinality:  u16 little-endian, 1..4096
//! payload:      sparse u16 positions or 64 little-endian u64 words
//! ```
//!
//! Sparse is mandatory for cardinality 1–255 and dense for 256–4096. Empty
//! blocks have no row. The codec owns no database and no decoded cache.

use super::StoreError;

pub const BLOCK_SLOTS: u32 = 4_096;
pub const BLOCK_SHIFT: u32 = 12;
pub const BLOCK_MASK: u32 = BLOCK_SLOTS - 1;
pub const MAX_CARDINALITY: u16 = 4_096;
pub const SPARSE_LIMIT: u16 = 255;

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

    pub fn insert_global(&mut self, block: u32, out: &mut roaring::RoaringBitmap) {
        self.for_each(|offset| {
            out.insert((block << BLOCK_SHIFT) | u32::from(offset));
        });
    }
}

pub fn encode_block(block: &Block) -> super::StoreResult<Vec<u8>> {
    let cardinality = block.cardinality();
    if cardinality == 0 {
        return Err(StoreError::State("cannot encode an empty bitmap block".into()));
    }
    let mut bytes = Vec::with_capacity(DENSE_PAYLOAD_BYTES + 4);
    bytes.push(1);
    if cardinality <= SPARSE_LIMIT {
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

pub fn decode_block(bytes: &[u8]) -> super::StoreResult<Block> {
    let version = *bytes
        .first()
        .ok_or_else(|| StoreError::CorruptIndex("empty bitmap block".into()))?;
    if version != 1 {
        return Err(StoreError::CorruptIndex(format!(
            "unknown bitmap block version {version}"
        )));
    }
    let encoding = *bytes
        .get(1)
        .ok_or_else(|| StoreError::CorruptIndex("truncated bitmap block header".into()))?;
    let cardinality_bytes = bytes
        .get(2..4)
        .ok_or_else(|| StoreError::CorruptIndex("truncated bitmap cardinality".into()))?;
    let cardinality = u16::from_le_bytes(
        cardinality_bytes
            .try_into()
            .expect("two-byte cardinality"),
    );
    if cardinality == 0 || cardinality > MAX_CARDINALITY {
        return Err(StoreError::CorruptIndex(format!(
            "invalid bitmap block cardinality {cardinality}"
        )));
    }
    let payload = &bytes[4..];
    let mut block = Block::default();
    match encoding {
        0 => {
            if cardinality > SPARSE_LIMIT {
                return Err(StoreError::CorruptIndex(
                    "sparse bitmap block above the dense threshold".into(),
                ));
            }
            if payload.len() != usize::from(cardinality) * 2 {
                return Err(StoreError::CorruptIndex(
                    "sparse bitmap payload length mismatch".into(),
                ));
            }
            let mut previous = None;
            for chunk in payload.chunks(2) {
                let offset = u16::from_le_bytes(chunk.try_into().expect("two-byte offset"));
                if offset >= BLOCK_SLOTS as u16
                    || previous.is_some_and(|previous| offset <= previous)
                {
                    return Err(StoreError::CorruptIndex(
                        "noncanonical sparse bitmap positions".into(),
                    ));
                }
                previous = Some(offset);
                block.set(offset);
            }
        }
        1 => {
            if cardinality <= SPARSE_LIMIT {
                return Err(StoreError::CorruptIndex(
                    "dense bitmap block at or below the sparse threshold".into(),
                ));
            }
            if payload.len() != DENSE_PAYLOAD_BYTES {
                return Err(StoreError::CorruptIndex(
                    "dense bitmap payload length mismatch".into(),
                ));
            }
            for (index, chunk) in payload.chunks(8).enumerate() {
                block.words[index] =
                    u64::from_le_bytes(chunk.try_into().expect("eight-byte word"));
            }
        }
        other => {
            return Err(StoreError::CorruptIndex(format!(
                "unknown bitmap block encoding {other}"
            )));
        }
    }
    if block.cardinality() != cardinality {
        return Err(StoreError::CorruptIndex(
            "bitmap block bit count does not match its header".into(),
        ));
    }
    Ok(block)
}

#[cfg(test)]
#[path = "bitmap_block.test.rs"]
mod tests;
