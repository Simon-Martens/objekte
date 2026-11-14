//! Packed coverage descriptors for logical and structural record domains.

use super::backing::Backing;
use super::error::{StorageError, StorageResult};
use super::format::SectionDescriptor;
use super::record_set::{encode_record_set, RecordSetRef, RecordSetTag, RecordSetView};

pub const COVERAGE_MAGIC: [u8; 4] = *b"OBCO";
pub const COVERAGE_VERSION: u16 = 1;
pub const COVERAGE_HEADER_LEN: usize = 24;

/// Encode the logical-record and empty-array coverage sets.
pub fn build_coverage(
    logical_records: &[u32],
    empty_arrays: &[u32],
    slot_count: u32,
) -> StorageResult<Vec<u8>> {
    let mut arena = Vec::new();
    let logical = embed(logical_records, slot_count, &mut arena)?;
    let empty = embed(empty_arrays, slot_count, &mut arena)?;
    let mut bytes = vec![0u8; COVERAGE_HEADER_LEN];
    bytes[..4].copy_from_slice(&COVERAGE_MAGIC);
    bytes[4..6].copy_from_slice(&COVERAGE_VERSION.to_le_bytes());
    bytes[6..8].fill(0);
    bytes.extend_from_slice(&logical.encode());
    bytes.extend_from_slice(&empty.encode());
    let arena_offset = (COVERAGE_HEADER_LEN + 64) as u64;
    bytes.extend_from_slice(&arena);
    // Rewrite the header with the arena offset and length.
    bytes[8..16].copy_from_slice(&arena_offset.to_le_bytes());
    bytes[16..24].copy_from_slice(&(arena.len() as u64).to_le_bytes());
    Ok(bytes)
}

fn embed(
    slots: &[u32],
    slot_count: u32,
    arena: &mut Vec<u8>,
) -> StorageResult<RecordSetRef> {
    let mut encoded = encode_record_set(slots, slot_count)?;
    if encoded.reference.tag == RecordSetTag::Blocks {
        encoded.reference.a = encoded
            .reference
            .a
            .checked_add(arena.len() as u64)
            .ok_or_else(|| StorageError::ResourceLimit("coverage arena overflow".into()))?;
    }
    arena.extend_from_slice(&encoded.arena);
    Ok(encoded.reference)
}

/// Borrowed coverage reader.
pub struct CoverageView {
    backing: Backing,
    section: SectionDescriptor,
    slot_count: u32,
    logical: RecordSetRef,
    empty_arrays: RecordSetRef,
    arena_offset: u64,
    arena_len: u64,
}

impl CoverageView {
    pub fn open(
        backing: Backing,
        section: SectionDescriptor,
        slot_count: u32,
    ) -> StorageResult<Self> {
        let header = backing.read(section.offset, COVERAGE_HEADER_LEN as u64 + 64)?;
        if header[..4] != COVERAGE_MAGIC {
            return Err(StorageError::corrupt("coverage magic mismatch"));
        }
        let version = u16::from_le_bytes([header[4], header[5]]);
        if version != COVERAGE_VERSION {
            return Err(StorageError::UnsupportedFormat(format!(
                "coverage version {version}"
            )));
        }
        if header[6..8] != [0, 0] {
            return Err(StorageError::corrupt(
                "coverage header reserved bytes are not zero",
            ));
        }
        let arena_offset = u64::from_le_bytes(header[8..16].try_into().expect("coverage offset"));
        let arena_len = u64::from_le_bytes(header[16..24].try_into().expect("coverage length"));
        if arena_offset < COVERAGE_HEADER_LEN as u64 + 64
            || arena_offset
                .checked_add(arena_len)
                .is_none_or(|end| end > section.length)
        {
            return Err(StorageError::corrupt("coverage arena is out of bounds"));
        }
        let logical = RecordSetRef::decode(&header[24..56])?;
        let empty_arrays = RecordSetRef::decode(&header[56..88])?;
        Ok(Self {
            backing,
            section,
            slot_count,
            logical,
            empty_arrays,
            arena_offset,
            arena_len,
        })
    }

    pub fn logical_records(&self) -> StorageResult<RecordSetView<'_>> {
        let arena = self
            .backing
            .read(self.section.offset + self.arena_offset, self.arena_len)?;
        self.logical.view(arena, self.slot_count)
    }

    pub fn empty_arrays(&self) -> StorageResult<RecordSetView<'_>> {
        let arena = self
            .backing
            .read(self.section.offset + self.arena_offset, self.arena_len)?;
        self.empty_arrays.view(arena, self.slot_count)
    }

    pub fn verify_full(&self) -> StorageResult<()> {
        self.logical_records()?.verify_full()?;
        self.empty_arrays()?.verify_full()?;
        Ok(())
    }
}
