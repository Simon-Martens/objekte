//! Packed file parts, record rows, logical IDs, and identity order.

use std::collections::{BTreeMap, HashSet};

use super::backing::Backing;
use super::dictionary::{DictionaryReader, KeyOrder};
use super::dictionary_builder::build_dictionary;
use super::error::{StorageError, StorageResult};
use super::format::{SectionDescriptor, SectionKind};

/// Fixed encoded width of one file-part row.
pub const FILE_PART_ROW_LEN: usize = 32;
/// Fixed encoded width of one record row.
pub const RECORD_ROW_LEN: usize = 48;
/// Segment statistics magic.
pub const SEGMENT_STATS_MAGIC: [u8; 4] = *b"OBSS";
/// Segment statistics version.
pub const SEGMENT_STATS_VERSION: u16 = 1;
/// Fixed encoded width of the segment statistics header.
pub const SEGMENT_STATS_LEN: usize = 128;

/// One contiguous contribution chunk of a physical file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FilePart {
    pub file_id: u32,
    pub content_generation: u64,
    pub local_record_start: u32,
    pub record_count: u32,
    pub segment_slot_start: u32,
}

/// One packed record row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordRow {
    pub slot: u32,
    pub part_ordinal: u32,
    pub local_record_id: u32,
    pub byte_start: u64,
    pub byte_end: u64,
    pub logical_id: String,
    pub diagnostic_ref: u32,
    pub record_class: u8,
}

/// Segment-level counters and identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SegmentStats {
    pub collection: u32,
    pub source_version: u32,
    pub source_id: String,
    pub source_id_offset: u64,
    pub source_id_len: u32,
    pub slot_count: u32,
    pub record_count: u32,
    pub file_part_count: u32,
    pub identity_count: u32,
    pub field_count: u32,
    pub bigram_binding_count: u32,
    pub sort_column_count: u32,
    pub relation_count: u32,
    pub fallback_count: u64,
    pub source_bytes: u64,
    pub estimated_index_bytes: u64,
}

/// Input row for one file part.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FilePartInput {
    pub file_id: u32,
    pub content_generation: u64,
    pub local_record_start: u32,
    pub record_count: u32,
    pub segment_slot_start: u32,
}

/// Input row for one record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordInput {
    pub part_ordinal: u32,
    pub local_record_id: u32,
    pub byte_start: u64,
    pub byte_end: u64,
    pub logical_id: String,
    pub diagnostic_ref: u32,
    pub record_class: u8,
}

/// Packed sections owned by the record table.
pub struct RecordTableSections {
    pub records: Vec<u8>,
    pub file_parts: Vec<u8>,
    pub logical_ids: Vec<u8>,
    pub identity_order: Vec<u8>,
    pub string_arena: Vec<u8>,
    pub source_id_offset: u64,
    pub source_id_len: u32,
    pub source_bytes: u64,
    pub fallback_count: u64,
}

/// Build the record table sections from validated inputs.
pub fn build_record_table(
    file_parts: &[FilePartInput],
    records: &[RecordInput],
    identity_order: &[u32],
    source_id: &str,
    slot_count: u32,
) -> StorageResult<RecordTableSections> {
    if records.len() != slot_count as usize {
        return Err(StorageError::InvalidArgument(
            "record count does not match the segment slot domain".into(),
        ));
    }
    let mut expected_slot = 0u32;
    for part in file_parts {
        if part.segment_slot_start != expected_slot {
            return Err(StorageError::InvalidArgument(
                "file parts must cover the segment slot domain contiguously".into(),
            ));
        }
        expected_slot = expected_slot
            .checked_add(part.record_count)
            .ok_or_else(|| StorageError::ResourceLimit("segment slot domain overflow".into()))?;
    }
    if expected_slot != slot_count {
        return Err(StorageError::InvalidArgument(
            "file parts do not cover the segment slot domain".into(),
        ));
    }
    let mut string_arena = Vec::new();
    let source_id_offset = 0u64;
    string_arena.extend_from_slice(source_id.as_bytes());
    let source_id_len = u32::try_from(source_id.len())
        .map_err(|_| StorageError::ResourceLimit("source ID exceeds 4 GiB".into()))?;
    let mut records_bytes = Vec::with_capacity(records.len() * RECORD_ROW_LEN);
    let mut logical_entries = Vec::with_capacity(records.len());
    let mut seen_ids = HashSet::with_capacity(records.len());
    let mut fallback_count = 0u64;
    let mut source_bytes = 0u64;
    for (slot, record) in records.iter().enumerate() {
        let part = file_parts.get(record.part_ordinal as usize).ok_or_else(|| {
            StorageError::InvalidArgument("record references an unknown file part".into())
        })?;
        let slot = u32::try_from(slot)
            .map_err(|_| StorageError::ResourceLimit("record slot overflow".into()))?;
        if slot < part.segment_slot_start
            || slot >= part.segment_slot_start.saturating_add(part.record_count)
        {
            return Err(StorageError::InvalidArgument(
                "record slot lies outside its file part".into(),
            ));
        }
        if record.local_record_id >= part.record_count {
            return Err(StorageError::InvalidArgument(
                "local record ID lies outside its file part".into(),
            ));
        }
        if record.byte_end < record.byte_start {
            return Err(StorageError::InvalidArgument(
                "record byte range is reversed".into(),
            ));
        }
        if record.logical_id.is_empty() || !seen_ids.insert(record.logical_id.clone()) {
            return Err(StorageError::InvalidArgument(
                "logical record IDs must be nonempty and unique".into(),
            ));
        }
        if record.record_class > 1 {
            return Err(StorageError::InvalidArgument(
                "record class code is unsupported".into(),
            ));
        }
        if record.record_class == 1 {
            fallback_count += 1;
        }
        source_bytes = source_bytes.saturating_add(record.byte_end - record.byte_start);
        let logical_offset = u64::try_from(string_arena.len())
            .map_err(|_| StorageError::ResourceLimit("string arena overflow".into()))?;
        string_arena.extend_from_slice(record.logical_id.as_bytes());
        let logical_len = u32::try_from(record.logical_id.len())
            .map_err(|_| StorageError::ResourceLimit("logical ID exceeds 4 GiB".into()))?;
        records_bytes.extend_from_slice(&record.part_ordinal.to_le_bytes());
        records_bytes.extend_from_slice(&record.local_record_id.to_le_bytes());
        records_bytes.extend_from_slice(&record.byte_start.to_le_bytes());
        records_bytes.extend_from_slice(&record.byte_end.to_le_bytes());
        records_bytes.extend_from_slice(&logical_offset.to_le_bytes());
        records_bytes.extend_from_slice(&logical_len.to_le_bytes());
        records_bytes.extend_from_slice(&record.diagnostic_ref.to_le_bytes());
        records_bytes.push(record.record_class);
        records_bytes.extend_from_slice(&[0u8; 7]);
        logical_entries.push((
            record.logical_id.as_bytes().to_vec(),
            slot.to_le_bytes().to_vec(),
        ));
    }
    logical_entries.sort_by(|left, right| left.0.cmp(&right.0));
    let logical_ids = build_dictionary(logical_entries, KeyOrder::Bytes, 8192)?;
    let mut parts_bytes = Vec::with_capacity(file_parts.len() * FILE_PART_ROW_LEN);
    for part in file_parts {
        parts_bytes.extend_from_slice(&part.file_id.to_le_bytes());
        parts_bytes.extend_from_slice(&[0u8; 4]);
        parts_bytes.extend_from_slice(&part.content_generation.to_le_bytes());
        parts_bytes.extend_from_slice(&part.local_record_start.to_le_bytes());
        parts_bytes.extend_from_slice(&part.record_count.to_le_bytes());
        parts_bytes.extend_from_slice(&part.segment_slot_start.to_le_bytes());
        parts_bytes.extend_from_slice(&[0u8; 4]);
    }
    validate_identity_order(identity_order, slot_count)?;
    let mut identity_bytes = Vec::with_capacity(identity_order.len() * 4);
    for slot in identity_order {
        identity_bytes.extend_from_slice(&slot.to_le_bytes());
    }
    Ok(RecordTableSections {
        records: records_bytes,
        file_parts: parts_bytes,
        logical_ids,
        identity_order: identity_bytes,
        string_arena,
        source_id_offset,
        source_id_len,
        source_bytes,
        fallback_count,
    })
}

fn validate_identity_order(identity_order: &[u32], slot_count: u32) -> StorageResult<()> {
    if identity_order.len() != slot_count as usize {
        return Err(StorageError::InvalidArgument(
            "identity order must contain every segment slot exactly once".into(),
        ));
    }
    let mut seen = vec![false; slot_count as usize];
    for slot in identity_order {
        let Some(seen) = seen.get_mut(*slot as usize) else {
            return Err(StorageError::InvalidArgument(
                "identity order slot is outside the domain".into(),
            ));
        };
        if *seen {
            return Err(StorageError::InvalidArgument(
                "identity order contains a duplicate slot".into(),
            ));
        }
        *seen = true;
    }
    Ok(())
}

/// Encode one segment statistics header.
pub fn encode_stats(stats: &SegmentStats) -> Vec<u8> {
    let mut bytes = vec![0u8; SEGMENT_STATS_LEN];
    bytes[..4].copy_from_slice(&SEGMENT_STATS_MAGIC);
    bytes[4..6].copy_from_slice(&SEGMENT_STATS_VERSION.to_le_bytes());
    bytes[8..12].copy_from_slice(&stats.collection.to_le_bytes());
    bytes[12..16].copy_from_slice(&stats.source_version.to_le_bytes());
    bytes[16..20].copy_from_slice(&stats.slot_count.to_le_bytes());
    bytes[20..24].copy_from_slice(&stats.record_count.to_le_bytes());
    bytes[24..28].copy_from_slice(&stats.file_part_count.to_le_bytes());
    bytes[28..32].copy_from_slice(&stats.identity_count.to_le_bytes());
    bytes[32..36].copy_from_slice(&stats.field_count.to_le_bytes());
    bytes[36..40].copy_from_slice(&stats.bigram_binding_count.to_le_bytes());
    bytes[40..44].copy_from_slice(&stats.sort_column_count.to_le_bytes());
    bytes[44..48].copy_from_slice(&stats.relation_count.to_le_bytes());
    bytes[48..56].copy_from_slice(&stats.fallback_count.to_le_bytes());
    bytes[56..64].copy_from_slice(&stats.source_bytes.to_le_bytes());
    bytes[64..72].copy_from_slice(&stats.estimated_index_bytes.to_le_bytes());
    bytes[72..80].copy_from_slice(&stats.source_id_offset.to_le_bytes());
    bytes[80..84].copy_from_slice(&stats.source_id_len.to_le_bytes());
    bytes
}

/// Decode and validate one segment statistics header.
pub fn decode_stats(bytes: &[u8]) -> StorageResult<SegmentStats> {
    if bytes.len() < SEGMENT_STATS_LEN {
        return Err(StorageError::corrupt("truncated segment statistics"));
    }
    if bytes[..4] != SEGMENT_STATS_MAGIC {
        return Err(StorageError::corrupt("segment statistics magic mismatch"));
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if version != SEGMENT_STATS_VERSION {
        return Err(StorageError::UnsupportedFormat(format!(
            "segment statistics version {version}"
        )));
    }
    if bytes[6..8] != [0, 0] || bytes[84..].iter().any(|byte| *byte != 0) {
        return Err(StorageError::corrupt(
            "segment statistics reserved bytes are not zero",
        ));
    }
    Ok(SegmentStats {
        collection: read_u32(bytes, 8)?,
        source_version: read_u32(bytes, 12)?,
        slot_count: read_u32(bytes, 16)?,
        record_count: read_u32(bytes, 20)?,
        file_part_count: read_u32(bytes, 24)?,
        identity_count: read_u32(bytes, 28)?,
        field_count: read_u32(bytes, 32)?,
        bigram_binding_count: read_u32(bytes, 36)?,
        sort_column_count: read_u32(bytes, 40)?,
        relation_count: read_u32(bytes, 44)?,
        source_id: String::new(),
        source_id_offset: read_u64(bytes, 72)?,
        source_id_len: read_u32(bytes, 80)?,
        fallback_count: read_u64(bytes, 48)?,
        source_bytes: read_u64(bytes, 56)?,
        estimated_index_bytes: read_u64(bytes, 64)?,
    })
}

/// Reader over the packed record table.
pub struct RecordTableView {
    backing: Backing,
    sections: BTreeMap<SectionKind, SectionDescriptor>,
    stats: SegmentStats,
    logical: DictionaryReader,
}

impl std::fmt::Debug for RecordTableView {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RecordTableView")
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

impl RecordTableView {
    pub fn open(
        backing: Backing,
        sections: BTreeMap<SectionKind, SectionDescriptor>,
        stats: SegmentStats,
    ) -> StorageResult<Self> {
        let logical = sections
            .get(&SectionKind::LogicalIds)
            .copied()
            .ok_or_else(|| StorageError::corrupt("logical-ID dictionary is missing"))?;
        let dictionary =
            DictionaryReader::open(backing.clone(), logical.offset, logical.length)?;
        if dictionary.entry_count() != stats.record_count as u64 {
            return Err(StorageError::corrupt(
                "logical-ID dictionary does not cover every record",
            ));
        }
        let arena = sections
            .get(&SectionKind::StringArena)
            .copied()
            .ok_or_else(|| StorageError::corrupt("string arena is missing"))?;
        let source_end = stats
            .source_id_offset
            .checked_add(stats.source_id_len as u64)
            .ok_or_else(|| StorageError::corrupt("source ID reference overflow"))?;
        if source_end > arena.length {
            return Err(StorageError::corrupt("source ID reference is out of bounds"));
        }
        let source_bytes = backing.read(arena.offset + stats.source_id_offset, stats.source_id_len as u64)?;
        let mut stats = stats;
        stats.source_id = std::str::from_utf8(source_bytes)
            .map_err(|_| StorageError::corrupt("source ID is not UTF-8"))?
            .to_owned();
        Ok(Self {
            backing,
            sections,
            stats,
            logical: dictionary,
        })
    }

    pub fn stats(&self) -> &SegmentStats {
        &self.stats
    }

    pub fn slot_count(&self) -> u32 {
        self.stats.slot_count
    }

    pub fn record_count(&self) -> u32 {
        self.stats.record_count
    }

    pub fn file_part_count(&self) -> u32 {
        self.stats.file_part_count
    }

    pub fn file_part(&self, index: u32) -> StorageResult<Option<FilePart>> {
        if index >= self.stats.file_part_count {
            return Ok(None);
        }
        let section = self.section(SectionKind::FileParts)?;
        let offset = (index as u64)
            .checked_mul(FILE_PART_ROW_LEN as u64)
            .ok_or_else(|| StorageError::corrupt("file part offset overflow"))?;
        if offset + FILE_PART_ROW_LEN as u64 > section.length {
            return Err(StorageError::corrupt("file part row is out of bounds"));
        }
        let bytes = self
            .backing
            .read(section.offset + offset, FILE_PART_ROW_LEN as u64)?;
        if bytes[4..8] != [0, 0, 0, 0] || bytes[28..32] != [0, 0, 0, 0] {
            return Err(StorageError::corrupt(
                "file part row reserved bytes are not zero",
            ));
        }
        Ok(Some(FilePart {
            file_id: read_u32(bytes, 0)?,
            content_generation: read_u64(bytes, 8)?,
            local_record_start: read_u32(bytes, 16)?,
            record_count: read_u32(bytes, 20)?,
            segment_slot_start: read_u32(bytes, 24)?,
        }))
    }

    /// Read one record row. `slot` is the physical segment slot.
    pub fn record(&self, slot: u32) -> StorageResult<Option<RecordRow>> {
        if slot >= self.stats.record_count {
            return Ok(None);
        }
        let section = self.section(SectionKind::Records)?;
        let offset = (slot as u64)
            .checked_mul(RECORD_ROW_LEN as u64)
            .ok_or_else(|| StorageError::corrupt("record row offset overflow"))?;
        if offset + RECORD_ROW_LEN as u64 > section.length {
            return Err(StorageError::corrupt("record row is out of bounds"));
        }
        let bytes = self
            .backing
            .read(section.offset + offset, RECORD_ROW_LEN as u64)?;
        if bytes[41..48].iter().any(|byte| *byte != 0) {
            return Err(StorageError::corrupt(
                "record row reserved bytes are not zero",
            ));
        }
        let logical_offset = read_u64(bytes, 24)?;
        let logical_len = read_u32(bytes, 32)?;
        let logical = self.read_arena(logical_offset, logical_len)?;
        let logical_id = std::str::from_utf8(logical)
            .map_err(|_| StorageError::corrupt("logical record ID is not UTF-8"))?
            .to_owned();
        Ok(Some(RecordRow {
            slot,
            part_ordinal: read_u32(bytes, 0)?,
            local_record_id: read_u32(bytes, 4)?,
            byte_start: read_u64(bytes, 8)?,
            byte_end: read_u64(bytes, 16)?,
            logical_id,
            diagnostic_ref: read_u32(bytes, 36)?,
            record_class: bytes[40],
        }))
    }

    pub fn logical_id(&self, slot: u32) -> StorageResult<Option<String>> {
        Ok(self.record(slot)?.map(|record| record.logical_id))
    }

    pub fn slot_for_logical_id(&self, id: &str) -> StorageResult<Option<u32>> {
        let Some(value) = self.logical.get(id.as_bytes())? else {
            return Ok(None);
        };
        if value.len() != 4 {
            return Err(StorageError::corrupt(
                "logical-ID dictionary value is not four bytes",
            ));
        }
        let slot = u32::from_le_bytes(value.try_into().expect("four-byte slot"));
        if slot >= self.stats.record_count {
            return Err(StorageError::corrupt(
                "logical-ID dictionary slot is out of bounds",
            ));
        }
        Ok(Some(slot))
    }

    /// Identity order slots, decoded from the packed array.
    pub fn identity_order(&self) -> StorageResult<Vec<u32>> {
        let section = self.section(SectionKind::IdentityOrder)?;
        if section.length % 4 != 0 || section.length / 4 != self.stats.identity_count as u64 {
            return Err(StorageError::corrupt(
                "identity order length does not match its count",
            ));
        }
        let bytes = self.backing.read(section.offset, section.length)?;
        let mut slots = Vec::with_capacity(section.length as usize / 4);
        for chunk in bytes.chunks_exact(4) {
            let slot = u32::from_le_bytes(chunk.try_into().expect("four-byte identity slot"));
            if slot >= self.stats.record_count {
                return Err(StorageError::corrupt(
                    "identity order slot is out of bounds",
                ));
            }
            slots.push(slot);
        }
        Ok(slots)
    }

    pub fn record_dictionary(&self) -> &DictionaryReader {
        &self.logical
    }

    pub fn string_arena(&self) -> StorageResult<&[u8]> {
        let section = self.section(SectionKind::StringArena)?;
        self.backing.read(section.offset, section.length)
    }

    fn read_arena(&self, offset: u64, length: u32) -> StorageResult<&[u8]> {
        let section = self.section(SectionKind::StringArena)?;
        let end = offset
            .checked_add(length as u64)
            .ok_or_else(|| StorageError::corrupt("string arena reference overflow"))?;
        if end > section.length {
            return Err(StorageError::corrupt(
                "string arena reference is out of bounds",
            ));
        }
        self.backing.read(section.offset + offset, length as u64)
    }

    fn section(&self, kind: SectionKind) -> StorageResult<SectionDescriptor> {
        self.sections
            .get(&kind)
            .copied()
            .ok_or_else(|| StorageError::corrupt(format!("required section {kind:?} is missing")))
    }
}

fn read_u32(bytes: &[u8], at: usize) -> StorageResult<u32> {
    let end = at
        .checked_add(4)
        .ok_or_else(|| StorageError::corrupt("u32 offset overflow"))?;
    let slice = bytes
        .get(at..end)
        .ok_or_else(|| StorageError::corrupt("truncated u32"))?;
    Ok(u32::from_le_bytes(
        slice.try_into().expect("four-byte checked slice"),
    ))
}

fn read_u64(bytes: &[u8], at: usize) -> StorageResult<u64> {
    let end = at
        .checked_add(8)
        .ok_or_else(|| StorageError::corrupt("u64 offset overflow"))?;
    let slice = bytes
        .get(at..end)
        .ok_or_else(|| StorageError::corrupt("truncated u64"))?;
    Ok(u64::from_le_bytes(
        slice.try_into().expect("eight-byte checked slice"),
    ))
}
