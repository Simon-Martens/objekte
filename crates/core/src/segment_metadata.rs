//! Source-I/O-free metadata sections for immutable hybrid segments.
//!
//! Each section has its own magic and version. Integers and lengths outside
//! complex leaf values are explicit little-endian scalars. The outer segment
//! envelope binds every section to its checksum before these decoders run.
//! Fixed-int bincode 1.x is confined to sort-column/composite and
//! relation-contribution leaves, each length-framed and validated after decode.
//! Their serde field order is part of version 1; changing it requires a bump.
//!
//! RecordSlots v2 has a checked fixed-width file table, fixed-width record
//! rows, per-file membership spans, an ID byte arena, and serialized logical
//! and fallback domains. Catalog metadata references preserve current or
//! historical FileEntry identity. See the byte-level format contract in
//! docs/development/direct_snapshot_segment_format.md.
//!
//! SortData v1: header, slot/column/composite counts, then length-framed
//! fixed-int bincode SortColumn leaves in deterministic path order and
//! CompositeSortOrder leaves in declaration order. RelationAndIdentity v1:
//! header, slot/relation counts, u32 identity-order permutation, then framed
//! RelationContribution leaves in declaration order. Decoding checks exact
//! framing, slot domains, permutations and ordering before exposing them;
//! configuration binding is checked against the prepared definition.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path};
use std::sync::Arc;

use bincode::Options;
use roaring::RoaringBitmap;
use serde::{Serialize, de::DeserializeOwned};

use crate::build::{IndexedCollectionSegment, SegmentFile};
use crate::inventory::{
    ByteRange, DiagnosticRef, FileEntry, MAX_DIAGNOSTIC_CODE_BYTES, MAX_DIAGNOSTIC_MESSAGE_BYTES,
    MAX_DIAGNOSTIC_PATH_SEGMENTS, MAX_DIAGNOSTIC_SEGMENT_BYTES, MAX_DIAGNOSTICS_PER_FILE,
    RecordClass, RecordEntry, SourceRevision,
};
use crate::model::{CollectionId, FileId, RecordId, RecordSlot};
use crate::relation::RelationContribution;
use crate::repository::CollectionDefinition;
use crate::sorting::{
    COLLATION_VERSION, CompositeSortOrder, SortCollection, SortColumn, compare_composite,
    simple_composite_part,
};
use crate::segment_format::{SectionKind, SegmentReader};

const RECORD_MAGIC: &[u8; 8] = b"OBJHREC2";
const RECORD_VERSION: u32 = 2;
const SORT_MAGIC: &[u8; 8] = b"OBJHSRT1";
const RELATION_MAGIC: &[u8; 8] = b"OBJHREL1";
const VERSION: u32 = 1;
const MAX_SECTION_BYTES: usize = 512 * 1024 * 1024;
const MAX_LEAF_BYTES: usize = 64 * 1024 * 1024;
// An initial collection shard can include a large whole-source partition;
// its metadata remains bounded independently of the repository-wide catalog.
const MAX_FILES: usize = 1_048_576;
const MAX_RECORDS: usize = 1_048_576;
const MAX_COLUMNS: usize = 4_096;
const MAX_RELATIONS: usize = 4_096;
const MAX_PATH_BYTES: usize = 1024 * 1024;
const MAX_ID_BYTES: usize = 64 * 1024;
const MAX_SOURCE_BYTES: usize = 64 * 1024;

/// Validated values needed to reconstruct a searchable segment without
/// opening source files or rebuilding an unbounded repository-wide table.
pub(crate) struct CheckedRecordSlots {
    pub source_id: String,
    pub source_version: u32,
    pub files: Vec<SegmentFile>,
    pub records: Vec<RecordEntry>,
    pub file_index: HashMap<FileId, usize>,
    pub id_lookup: HashMap<RecordId, RecordSlot>,
    pub logical_records: RoaringBitmap,
    pub fallback_records: RoaringBitmap,
}

impl CheckedRecordSlots {
    pub(crate) fn slot_count(&self) -> u32 {
        self.records.len() as u32
    }
}

pub(crate) struct CheckedRelationIdentity {
    pub identity_order: Vec<RecordSlot>,
    pub relations: Vec<RelationContribution>,
}

fn leaf_codec() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .reject_trailing_bytes()
}

fn write_leaf<T: Serialize>(out: &mut Vec<u8>, value: &T) -> Result<(), String> {
    let bytes = leaf_codec().serialize(value).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_LEAF_BYTES {
        return Err("metadata leaf exceeds size limit".into());
    }
    put_bytes(out, &bytes, MAX_LEAF_BYTES)
}

fn read_leaf<T: DeserializeOwned>(cursor: &mut Cursor<'_>) -> Result<T, String> {
    let bytes = cursor.bytes(MAX_LEAF_BYTES)?;
    leaf_codec()
        .with_limit(bytes.len() as u64)
        .deserialize(bytes)
        .map_err(|e| format!("metadata leaf invalid: {e}"))
}

fn start(out: &mut Vec<u8>, magic: &[u8; 8]) {
    out.extend_from_slice(magic);
    put_u32(out, VERSION);
    put_u32(out, crate::MODEL_VERSION);
}

fn check_header(cursor: &mut Cursor<'_>, magic: &[u8; 8]) -> Result<(), String> {
    if cursor.take(8)? != magic || cursor.u32()? != VERSION || cursor.u32()? != crate::MODEL_VERSION
    {
        return Err("unsupported metadata section magic or version".into());
    }
    Ok(())
}

fn finish(out: Vec<u8>) -> Result<Vec<u8>, String> {
    if out.len() > MAX_SECTION_BYTES {
        return Err("metadata section exceeds size limit".into());
    }
    Ok(out)
}

/// RecordSlots references each file metadata entry by an external catalog
/// index. The catalog interns current and distinct historical revisions once.
/// Direct file, slot, membership, and domain tables share these references.
pub(crate) fn encode_record_slots(
    segment: &IndexedCollectionSegment,
    intern: &mut dyn FnMut(&FileEntry) -> Result<u32, String>,
) -> Result<Vec<u8>, String> {
    encode_records_inner(
        segment.collection,
        &segment.source_id,
        segment.source_version,
        &segment.files,
        &segment.records,
        &segment.logical_records,
        intern,
        true,
    )
}

/// The admission builder already owns validated file, ID, slot and domain
/// maps. Encoding its same dense table need not recreate those maps.
pub(crate) fn encode_record_slots_constructed(
    segment: &IndexedCollectionSegment,
    intern: &mut dyn FnMut(&FileEntry) -> Result<u32, String>,
) -> Result<Vec<u8>, String> {
    encode_records_inner(
        segment.collection,
        &segment.source_id,
        segment.source_version,
        &segment.files,
        &segment.records,
        &segment.logical_records,
        intern,
        false,
    )
}

/// The in-memory reader retains these tables and encodes their sections only
/// when persisted. Preserve their domain and identity proof at sealing.
pub(crate) fn check_deferred_metadata_limits(
    segment: &IndexedCollectionSegment,
) -> Result<RoaringBitmap, String> {
    if segment.source_id.is_empty()
        || segment.source_id.len() > MAX_SOURCE_BYTES
        || segment.files.len() > MAX_FILES
        || segment.records.len() > MAX_RECORDS
        || segment.sorting.columns.len() > MAX_COLUMNS
        || segment.sorting.composites.len() > MAX_COLUMNS
        || segment.relations.len() > MAX_RELATIONS
    {
        return Err("constructed metadata exceeds format limits".into());
    }
    let (logical, fallback) = record_domains(&segment.records)?;
    if logical != segment.logical_records {
        return Err("constructed logical record domain changed".into());
    }
    validate_relation_structure(
        &segment.relations,
        &segment.records,
        &segment.identity_order,
        &logical,
        &fallback,
    )?;
    Ok(fallback)
}

#[cfg(test)]
fn encode_records(
    collection: CollectionId,
    source_id: &str,
    source_version: u32,
    files: &[SegmentFile],
    records: &[RecordEntry],
    intern: &mut dyn FnMut(&FileEntry) -> Result<u32, String>,
) -> Result<Vec<u8>, String> {
    let (logical, _) = record_domains(records)?;
    encode_records_inner(collection, source_id, source_version, files, records, &logical, intern, true)
}

fn encode_records_inner(
    collection: CollectionId,
    source_id: &str,
    source_version: u32,
    files: &[SegmentFile],
    records: &[RecordEntry],
    logical_records: &RoaringBitmap,
    intern: &mut dyn FnMut(&FileEntry) -> Result<u32, String>,
    validate: bool,
) -> Result<Vec<u8>, String> {
    if validate {
        let (_, _, checked_logical, _) = validate_records(
            collection,
            source_id.to_owned(),
            source_version,
            files,
            records,
        )?;
        if &checked_logical != logical_records {
            return Err("record logical domain mismatch before persistence".into());
        }
    } else if source_id.is_empty()
        || source_id.len() > MAX_SOURCE_BYTES
        || files.len() > MAX_FILES
        || records.len() > MAX_RECORDS
    {
        return Err("constructed record metadata exceeds format limits".into());
    }
    if logical_records.max().is_some_and(|slot| slot as usize >= records.len()) {
        return Err("record logical domain exceeds slots".into());
    }
    let fallback_records = &RoaringBitmap::from_iter(0..records.len() as u32) - logical_records;
    let mut logical_bytes = Vec::with_capacity(logical_records.serialized_size());
    logical_records.serialize_into(&mut logical_bytes).map_err(|error| error.to_string())?;
    let mut fallback_bytes = Vec::with_capacity(fallback_records.serialized_size());
    fallback_records.serialize_into(&mut fallback_bytes).map_err(|error| error.to_string())?;
    let mut ordered_files = files.iter().collect::<Vec<_>>();
    ordered_files.sort_unstable_by_key(|file| file.entry.id);
    if ordered_files.windows(2).any(|pair| pair[0].entry.id == pair[1].entry.id) {
        return Err("duplicate record file identity".into());
    }
    let id_bytes = records.iter().try_fold(0usize, |total, record| {
        if record.id.0.is_empty() || record.id.0.len() > MAX_ID_BYTES {
            return Err("record ID length invalid".into());
        }
        total.checked_add(record.id.0.len()).ok_or("record ID arena overflow")
    })?;
    let membership_count = ordered_files.iter().try_fold(0usize, |total, file| {
        total.checked_add(file.records.len()).ok_or("record membership overflow")
    })?;
    if membership_count != records.len() || id_bytes > u32::MAX as usize {
        return Err("record membership or ID arena size mismatch".into());
    }
    let mut out = Vec::new();
    out.extend_from_slice(RECORD_MAGIC);
    put_u32(&mut out, RECORD_VERSION);
    put_u32(&mut out, crate::MODEL_VERSION);
    put_u32(&mut out, collection.0);
    put_bytes(&mut out, source_id.as_bytes(), MAX_SOURCE_BYTES)?;
    put_u32(&mut out, source_version);
    put_u32(&mut out, files.len() as u32);
    put_u32(&mut out, records.len() as u32);
    put_u32(&mut out, id_bytes as u32);
    put_u32(&mut out, membership_count as u32);
    put_u32(&mut out, u32::try_from(logical_bytes.len()).map_err(|_| "logical domain size overflow")?);
    put_u32(&mut out, u32::try_from(fallback_bytes.len()).map_err(|_| "fallback domain size overflow")?);
    let mut member_start = 0u32;
    for file in &ordered_files {
        put_u32(&mut out, file.entry.id.0);
        put_u32(&mut out, intern(&file.entry)?);
        put_u32(&mut out, member_start);
        put_u32(&mut out, file.records.len() as u32);
        member_start = member_start.checked_add(file.records.len() as u32).ok_or("record membership overflow")?;
    }
    let mut id_start = 0u32;
    for record in records {
        let file_row = ordered_files.binary_search_by_key(&record.file, |file| file.entry.id)
            .map_err(|_| "record file missing")?;
        put_u32(&mut out, file_row as u32);
        put_u64(&mut out, record.range.start);
        put_u64(&mut out, record.range.end);
        out.push(record.class.code());
        out.extend_from_slice(&[0; 3]);
        put_u32(&mut out, record.diagnostic_ref.0);
        put_u32(&mut out, id_start);
        put_u32(&mut out, u32::try_from(record.id.0.len()).map_err(|_| "record ID length overflow")?);
        id_start = id_start.checked_add(record.id.0.len() as u32).ok_or("record ID arena overflow")?;
    }
    for file in ordered_files {
        for slot in &file.records {
            put_u32(&mut out, slot.0);
        }
    }
    for record in records {
        out.extend_from_slice(record.id.0.as_bytes());
    }
    out.extend_from_slice(&logical_bytes);
    out.extend_from_slice(&fallback_bytes);
    finish(out)
}

#[derive(Clone, Copy)]
struct FileRow {
    id: FileId,
    metadata_ref: u32,
    membership_start: u32,
    membership_count: u32,
}

struct RecordRow<'a> {
    file_row: usize,
    range: ByteRange,
    class: RecordClass,
    diagnostic_ref: DiagnosticRef,
    id: &'a str,
}

struct RecordSlotsLayout {
    source_id: String,
    source_version: u32,
    file_count: usize,
    record_count: usize,
    file_start: usize,
    record_start: usize,
    membership_start: usize,
    id_start: usize,
    id_bytes: usize,
    logical_records: RoaringBitmap,
    fallback_records: RoaringBitmap,
}

fn u32_slice(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn u64_slice(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

fn decode_domain(bytes: &[u8]) -> Result<RoaringBitmap, String> {
    let mut cursor = std::io::Cursor::new(bytes);
    let bitmap = RoaringBitmap::deserialize_from(&mut cursor)
        .map_err(|error| format!("record domain invalid: {error}"))?;
    if cursor.position() != bytes.len() as u64 {
        return Err("record domain has trailing bytes".into());
    }
    Ok(bitmap)
}

impl RecordSlotsLayout {
    fn parse(
        bytes: &[u8],
        expected_collection: CollectionId,
        expected_source: (&str, u32),
    ) -> Result<Self, String> {
        let mut cursor = Cursor::new(bytes)?;
        if cursor.take(8)? != RECORD_MAGIC
            || cursor.u32()? != RECORD_VERSION
            || cursor.u32()? != crate::MODEL_VERSION
        {
            return Err("unsupported record section magic or version".into());
        }
        let collection = CollectionId(cursor.u32()?);
        let source_id = std::str::from_utf8(cursor.bytes(MAX_SOURCE_BYTES)?)
            .map_err(|_| "record source ID is not UTF-8")?;
        let source_version = cursor.u32()?;
        if collection != expected_collection
            || source_id.is_empty()
            || source_id != expected_source.0
            || source_version != expected_source.1
        {
            return Err("record section source or collection identity mismatch".into());
        }
        let file_count = cursor.u32()? as usize;
        let record_count = cursor.u32()? as usize;
        let id_bytes = cursor.u32()? as usize;
        let membership_count = cursor.u32()? as usize;
        let logical_bytes = cursor.u32()? as usize;
        let fallback_bytes = cursor.u32()? as usize;
        if file_count > MAX_FILES || record_count > MAX_RECORDS || membership_count != record_count {
            return Err("record table count invalid".into());
        }
        let file_start = cursor.offset;
        cursor.take(file_count.checked_mul(16).ok_or("record file rows overflow")?)?;
        let record_start = cursor.offset;
        cursor.take(record_count.checked_mul(36).ok_or("record rows overflow")?)?;
        let membership_start = cursor.offset;
        cursor.take(membership_count.checked_mul(4).ok_or("record membership overflow")?)?;
        let id_start = cursor.offset;
        cursor.take(id_bytes)?;
        let logical_records = decode_domain(cursor.take(logical_bytes)?)?;
        let fallback_records = decode_domain(cursor.take(fallback_bytes)?)?;
        cursor.end()?;
        if logical_records.max().is_some_and(|slot| slot as usize >= record_count)
            || fallback_records.max().is_some_and(|slot| slot as usize >= record_count)
            || !logical_records.is_disjoint(&fallback_records)
            || logical_records.len() + fallback_records.len() != record_count as u64
        {
            return Err("record class domains invalid".into());
        }
        Ok(Self {
            source_id: source_id.to_owned(),
            source_version,
            file_count,
            record_count,
            file_start,
            record_start,
            membership_start,
            id_start,
            id_bytes,
            logical_records,
            fallback_records,
        })
    }

    fn file_row(&self, bytes: &[u8], index: usize) -> Option<FileRow> {
        (index < self.file_count).then(|| {
            let start = self.file_start + index * 16;
            FileRow {
                id: FileId(u32_slice(bytes, start)),
                metadata_ref: u32_slice(bytes, start + 4),
                membership_start: u32_slice(bytes, start + 8),
                membership_count: u32_slice(bytes, start + 12),
            }
        })
    }

    fn record_row<'a>(&self, bytes: &'a [u8], slot: usize) -> Result<Option<RecordRow<'a>>, String> {
        if slot >= self.record_count {
            return Ok(None);
        }
        let start = self.record_start + slot * 36;
        let file_row = u32_slice(bytes, start) as usize;
        let range = ByteRange {
            start: u64_slice(bytes, start + 4),
            end: u64_slice(bytes, start + 12),
        };
        let class = match bytes[start + 20] {
            0 => RecordClass::Logical,
            1 => RecordClass::RawFallback,
            _ => return Err("record class invalid".into()),
        };
        let diagnostic_ref = DiagnosticRef(u32_slice(bytes, start + 24));
        let id_offset = u32_slice(bytes, start + 28) as usize;
        let id_len = u32_slice(bytes, start + 32) as usize;
        let id_end = id_offset.checked_add(id_len).ok_or("record ID arena offset overflow")?;
        if file_row >= self.file_count
            || bytes[start + 21..start + 24] != [0; 3]
            || id_len == 0
            || id_len > MAX_ID_BYTES
            || id_end > self.id_bytes
        {
            return Err("record row field invalid".into());
        }
        let id = std::str::from_utf8(&bytes[self.id_start + id_offset..self.id_start + id_end])
            .map_err(|_| "record ID is not UTF-8")?;
        Ok(Some(RecordRow { file_row, range, class, diagnostic_ref, id }))
    }

    fn membership(&self, bytes: &[u8], index: usize) -> Option<RecordSlot> {
        (index < self.record_count)
            .then(|| RecordSlot(u32_slice(bytes, self.membership_start + index * 4)))
    }
}

/// Immutable catalog metadata required by a segment. An owned catalog reader
/// implements this interface without transferring all of its entries into a
/// segment-local table. The owner is retained by the persisted record view.
pub(crate) trait SegmentMetadataRefs: Send + Sync {
    fn entry_for_ref(&self, reference: u32) -> Result<Option<FileEntry>, String>;
    fn file_id_for_ref(&self, reference: u32) -> Option<FileId>;
    fn revision_for_ref(&self, reference: u32) -> Option<SourceRevision>;
    fn contains_collection_for_ref(&self, reference: u32, collection: CollectionId) -> Option<bool>;
}

/// Checked offsets and domains over immutable mapped or resident record
/// section bytes. No record table, file table, membership list, or lookup map
/// is reconstructed on opening.
pub(crate) struct PersistedRecordSlots {
    reader: Arc<SegmentReader>,
    metadata: Arc<dyn SegmentMetadataRefs>,
    layout: RecordSlotsLayout,
}

impl PersistedRecordSlots {
    pub(crate) fn open(
        reader: Arc<SegmentReader>,
        collection: CollectionId,
        source: (&str, u32),
        metadata: Arc<dyn SegmentMetadataRefs>,
    ) -> Result<Self, String> {
        let bytes = reader.section(SectionKind::RecordSlots)?.ok_or("record section missing")?;
        let layout = RecordSlotsLayout::parse(bytes, collection, source)?;
        let result = Self { reader, metadata, layout };
        result.validate(collection)?;
        Ok(result)
    }

    fn bytes(&self) -> Result<&[u8], String> {
        self.reader.section(SectionKind::RecordSlots)?.ok_or("record section missing".into())
    }

    pub(crate) fn record_count(&self) -> usize { self.layout.record_count }

    pub(crate) fn file_count(&self) -> usize { self.layout.file_count }

    pub(crate) fn logical_records(&self) -> &RoaringBitmap { &self.layout.logical_records }

    pub(crate) fn fallback_records(&self) -> &RoaringBitmap { &self.layout.fallback_records }

    pub(crate) fn source_id(&self) -> &str { &self.layout.source_id }

    pub(crate) fn source_version(&self) -> u32 { self.layout.source_version }

    pub(crate) fn record_id(&self, slot: RecordSlot) -> Result<Option<&str>, String> {
        Ok(self.layout.record_row(self.bytes()?, slot.0 as usize)?.map(|row| row.id))
    }

    pub(crate) fn record(&self, slot: RecordSlot) -> Result<Option<RecordEntry>, String> {
        let bytes = self.bytes()?;
        let Some(row) = self.layout.record_row(bytes, slot.0 as usize)? else {
            return Ok(None);
        };
        let file_row = self.layout.file_row(bytes, row.file_row).ok_or("record file row missing")?;
        let revision = self.metadata.revision_for_ref(file_row.metadata_ref)
            .ok_or("record catalog revision missing")?;
        Ok(Some(RecordEntry {
            slot,
            id: RecordId(row.id.into()),
            file: file_row.id,
            range: row.range,
            revision,
            class: row.class,
            diagnostic_ref: row.diagnostic_ref,
        }))
    }

    pub(crate) fn file_index(&self, id: FileId) -> Result<Option<usize>, String> {
        let bytes = self.bytes()?;
        let mut low = 0usize;
        let mut high = self.layout.file_count;
        while low < high {
            let middle = low + (high - low) / 2;
            let row = self.layout.file_row(bytes, middle).ok_or("record file row missing")?;
            if row.id < id { low = middle + 1; } else { high = middle; }
        }
        Ok((low < self.layout.file_count
            && self.layout.file_row(bytes, low).is_some_and(|row| row.id == id)).then_some(low))
    }

    pub(crate) fn file_entry(&self, id: FileId) -> Result<Option<FileEntry>, String> {
        let Some(index) = self.file_index(id)? else { return Ok(None); };
        let row = self.layout.file_row(self.bytes()?, index).ok_or("record file row missing")?;
        let entry = self.metadata.entry_for_ref(row.metadata_ref)?.ok_or("record metadata reference missing")?;
        if entry.id != id { return Err("record metadata file identity mismatch".into()); }
        Ok(Some(entry))
    }

    pub(crate) fn file_records(&self, id: FileId) -> Result<Option<Vec<RecordSlot>>, String> {
        let Some(index) = self.file_index(id)? else { return Ok(None); };
        let bytes = self.bytes()?;
        let row = self.layout.file_row(bytes, index).ok_or("record file row missing")?;
        let start = row.membership_start as usize;
        let end = start + row.membership_count as usize;
        Ok(Some((start..end).map(|number| self.layout.membership(bytes, number).expect("validated membership span")).collect()))
    }

    pub(crate) fn lookup_id(&self, order: &[RecordSlot], id: &str) -> Result<Option<RecordSlot>, String> {
        let mut low = 0usize;
        let mut high = order.len();
        while low < high {
            let middle = low + (high - low) / 2;
            let key = self.record_id(order[middle])?.ok_or("identity slot missing")?;
            if key < id { low = middle + 1; } else { high = middle; }
        }
        if low < order.len() && self.record_id(order[low])? == Some(id) {
            Ok(Some(order[low]))
        } else {
            Ok(None)
        }
    }

    fn validate(&self, collection: CollectionId) -> Result<(), String> {
        let bytes = self.bytes()?;
        let mut files = Vec::with_capacity(self.layout.file_count);
        let mut paths = HashSet::with_capacity(self.layout.file_count);
        let mut previous_file = None;
        let mut next_member = 0usize;
        for number in 0..self.layout.file_count {
            let row = self.layout.file_row(bytes, number).ok_or("record file row missing")?;
            if previous_file.is_some_and(|id| id >= row.id)
                || row.membership_start as usize != next_member
                || self.metadata.file_id_for_ref(row.metadata_ref) != Some(row.id)
                || self.metadata.contains_collection_for_ref(row.metadata_ref, collection) != Some(true)
            {
                return Err("record file lookup or metadata reference invalid".into());
            }
            next_member = next_member.checked_add(row.membership_count as usize)
                .ok_or("record membership extent overflow")?;
            if next_member > self.layout.record_count {
                return Err("record membership extent out of bounds".into());
            }
            let entry = self.metadata.entry_for_ref(row.metadata_ref)?
                .ok_or("record catalog metadata missing")?;
            if !valid_source_path(&entry.relative_path)
                || native_path_bytes(&entry.relative_path).len() > MAX_PATH_BYTES
                || !paths.insert(entry.relative_path.clone())
                || entry.id != row.id
            {
                return Err("record file path or identity invalid".into());
            }
            files.push(entry);
            previous_file = Some(row.id);
        }
        if next_member != self.layout.record_count {
            return Err("record membership incomplete".into());
        }
        let mut next_id = 0usize;
        for slot in 0..self.layout.record_count {
            let row = self.layout.record_row(bytes, slot)?.ok_or("record row missing")?;
            let id_offset = u32_slice(bytes, self.layout.record_start + slot * 36 + 28) as usize;
            if id_offset != next_id { return Err("record ID arena gap or overlap".into()); }
            next_id += row.id.len();
            let entry = &files[row.file_row];
            if row.range.start > row.range.end || row.range.end > entry.revision.size
                || self.layout.logical_records.contains(slot as u32) != (row.class == RecordClass::Logical)
                || self.layout.fallback_records.contains(slot as u32) != (row.class == RecordClass::RawFallback)
            {
                return Err("record source span or domain invalid".into());
            }
            if let Some(index) = row.diagnostic_ref.stored_index() {
                if entry.diagnostics.get(index).is_none_or(|diagnostic| diagnostic.record != Some(RecordSlot(slot as u32))) {
                    return Err("record diagnostic reference invalid".into());
                }
            }
            if row.class == RecordClass::RawFallback
                && row.id != RecordId::fallback(collection, &self.layout.source_id, self.layout.source_version,
                    &entry.relative_path, entry.id, entry.revision.modified_ns, row.range.start, row.range.end).0.as_ref()
            {
                return Err("fallback record identity mismatch".into());
            }
        }
        if next_id != self.layout.id_bytes { return Err("record ID arena has trailing bytes".into()); }
        let mut seen = vec![false; self.layout.record_count];
        for file_number in 0..self.layout.file_count {
            let row = self.layout.file_row(bytes, file_number).ok_or("record file row missing")?;
            let mut previous_slot = None;
            for number in row.membership_start as usize..row.membership_start as usize + row.membership_count as usize {
                let slot = self.layout.membership(bytes, number).ok_or("record membership missing")?;
                let index = slot.0 as usize;
                if index >= self.layout.record_count || seen[index]
                    || previous_slot.is_some_and(|prior| prior >= slot)
                    || self.layout.record_row(bytes, index)?.is_none_or(|record| record.file_row != file_number)
                {
                    return Err("record file membership invalid".into());
                }
                seen[index] = true;
                previous_slot = Some(slot);
            }
        }
        if seen.iter().any(|seen| !seen) { return Err("record file membership incomplete".into()); }
        Ok(())
    }
}

pub(crate) fn decode_record_slots<'a>(
    bytes: &[u8],
    expected_collection: CollectionId,
    expected_source: (&str, u32),
    resolve: &dyn Fn(u32) -> Option<&'a FileEntry>,
) -> Result<CheckedRecordSlots, String> {
    let layout = RecordSlotsLayout::parse(bytes, expected_collection, expected_source)?;
    let mut files = Vec::with_capacity(layout.file_count);
    let mut previous_file = None;
    let mut next_member = 0usize;
    for number in 0..layout.file_count {
        let file_row = layout.file_row(bytes, number).ok_or("record file row missing")?;
        if previous_file.is_some_and(|previous| previous >= file_row.id)
            || file_row.membership_start as usize != next_member
        {
            return Err("record file row order or membership span invalid".into());
        }
        previous_file = Some(file_row.id);
        next_member = next_member.checked_add(file_row.membership_count as usize)
            .ok_or("record membership extent overflow")?;
        if next_member > layout.record_count {
            return Err("record membership extent out of bounds".into());
        }
        let metadata_ref = file_row.metadata_ref;
        let entry = resolve(metadata_ref)
            .ok_or_else(|| format!("record file metadata reference {metadata_ref} missing"))?;
        if entry.id != file_row.id {
            return Err("record file metadata identity mismatch".into());
        }
        files.push(SegmentFile {
            entry: entry.clone(),
            records: Vec::new(),
        });
    }
    if next_member != layout.record_count {
        return Err("record membership incomplete".into());
    }
    let mut records = Vec::with_capacity(layout.record_count);
    let mut id_position = 0usize;
    for slot in 0..layout.record_count {
        let row = layout.record_row(bytes, slot)?.ok_or("record row missing")?;
        let row_offset = u32_slice(bytes, layout.record_start + slot * 36 + 28) as usize;
        if row_offset != id_position {
            return Err("record ID arena gap or overlap".into());
        }
        id_position += row.id.len();
        records.push(RecordEntry {
            slot: RecordSlot(slot as u32),
            id: RecordId(row.id.into()),
            file: files[row.file_row].entry.id,
            range: row.range,
            revision: files[row.file_row].entry.revision,
            class: row.class,
            diagnostic_ref: row.diagnostic_ref,
        });
    }
    if id_position != layout.id_bytes {
        return Err("record ID arena has trailing bytes".into());
    }
    for (file_number, file) in files.iter_mut().enumerate() {
        let file_row = layout.file_row(bytes, file_number).ok_or("record file row missing")?;
        let mut previous_slot = None;
        for index in file_row.membership_start as usize
            ..file_row.membership_start as usize + file_row.membership_count as usize
        {
            let slot = layout.membership(bytes, index).ok_or("record membership missing")?;
            if slot.0 as usize >= layout.record_count
                || previous_slot.is_some_and(|previous| previous >= slot)
                || records[slot.0 as usize].file != file.entry.id
            {
                return Err("record membership invalid".into());
            }
            previous_slot = Some(slot);
            file.records.push(slot);
        }
    }
    let (file_index, id_lookup, logical_records, fallback_records) = validate_records(
        expected_collection,
        layout.source_id.clone(),
        layout.source_version,
        &files,
        &records,
    )?;
    if logical_records != layout.logical_records || fallback_records != layout.fallback_records {
        return Err("record stored domain disagrees with classes".into());
    }
    Ok(CheckedRecordSlots {
        source_id: layout.source_id,
        source_version: layout.source_version,
        files,
        records,
        file_index,
        id_lookup,
        logical_records,
        fallback_records,
    })
}

fn validate_records(
    collection: CollectionId,
    source_id: String,
    source_version: u32,
    files: &[SegmentFile],
    records: &[RecordEntry],
) -> Result<
    (
        HashMap<FileId, usize>,
        HashMap<RecordId, RecordSlot>,
        RoaringBitmap,
        RoaringBitmap,
    ),
    String,
> {
    if source_id.is_empty()
        || source_id.len() > MAX_SOURCE_BYTES
        || files.len() > MAX_FILES
        || records.len() > MAX_RECORDS
    {
        return Err(format!(
            "record metadata count or source identity invalid: files={}, records={}, source_bytes={}",
            files.len(),
            records.len(),
            source_id.len()
        ));
    }
    let mut file_index = HashMap::with_capacity(files.len());
    let mut paths = HashSet::with_capacity(files.len());
    for (index, file) in files.iter().enumerate() {
        let entry = &file.entry;
        if !valid_source_path(&entry.relative_path)
            || native_path_bytes(&entry.relative_path).len() > MAX_PATH_BYTES
            || !paths.insert(entry.relative_path.clone())
            || !entry.collections.contains(&collection)
            || entry.collections.len() > MAX_FILES
            || entry.collections.iter().collect::<HashSet<_>>().len() != entry.collections.len()
            || entry.diagnostics.len() > MAX_DIAGNOSTICS_PER_FILE
            || file_index.insert(entry.id, index).is_some()
        {
            return Err("record file identity or metadata invalid".into());
        }
        for diagnostic in &entry.diagnostics {
            if diagnostic.file != entry.id
                || !entry.collections.contains(&diagnostic.collection)
                || diagnostic.code.is_empty()
                || diagnostic.code.len() > MAX_DIAGNOSTIC_CODE_BYTES
                || diagnostic.message.len() > MAX_DIAGNOSTIC_MESSAGE_BYTES
                || diagnostic.field_path.as_ref().is_some_and(|parts| {
                    parts.len() > MAX_DIAGNOSTIC_PATH_SEGMENTS
                        || parts
                            .iter()
                            .any(|part| part.len() > MAX_DIAGNOSTIC_SEGMENT_BYTES)
                })
                || diagnostic
                    .record
                    .is_some_and(|slot| slot.0 as usize >= records.len())
                || diagnostic
                    .range
                    .is_some_and(|span| span.start > span.end || span.end > entry.revision.size)
            {
                return Err("record file diagnostic invalid".into());
            }
        }
    }
    let mut id_lookup = HashMap::with_capacity(records.len());
    let mut logical_records = RoaringBitmap::new();
    let mut fallback_records = RoaringBitmap::new();
    let mut expected_file_slots = vec![Vec::new(); files.len()];
    for (number, record) in records.iter().enumerate() {
        let file_number = *file_index.get(&record.file).ok_or("record file missing")?;
        let file = &files[file_number].entry;
        if record.slot.0 as usize != number
            || record.id.0.is_empty()
            || record.id.0.len() > MAX_ID_BYTES
            || record.revision != file.revision
            || record.range.start > record.range.end
            || record.range.end > file.revision.size
            || id_lookup.insert(record.id.clone(), record.slot).is_some()
        {
            return Err("record slot identity or source span invalid".into());
        }
        if let Some(index) = record.diagnostic_ref.stored_index() {
            if file
                .diagnostics
                .get(index)
                .is_none_or(|diagnostic| diagnostic.record != Some(record.slot))
            {
                return Err("record diagnostic reference invalid".into());
            }
        }
        expected_file_slots[file_number].push(record.slot);
        match record.class {
            RecordClass::Logical => {
                logical_records.insert(record.slot.0);
            }
            RecordClass::RawFallback => {
                fallback_records.insert(record.slot.0);
                if record.id
                    != RecordId::fallback(
                        collection,
                        &source_id,
                        source_version,
                        &file.relative_path,
                        record.file,
                        record.revision.modified_ns,
                        record.range.start,
                        record.range.end,
                    )
                {
                    return Err("fallback record identity mismatch".into());
                }
            }
        }
    }
    if files
        .iter()
        .zip(&expected_file_slots)
        .any(|(file, expected)| &file.records != expected)
    {
        return Err("record file slot mapping mismatch".into());
    }
    Ok((file_index, id_lookup, logical_records, fallback_records))
}

/// SortData keeps each configured column as a separately framed leaf. Its
/// precomputed collation keys and permutations are never regenerated at open.
pub(crate) fn encode_sort_data(
    sorting: &SortCollection,
    records: &[RecordEntry],
) -> Result<Vec<u8>, String> {
    encode_sort_data_inner(sorting, records, true)
}

/// `finish_for_records` already checked this builder-owned sort structure.
pub(crate) fn encode_sort_data_constructed(
    sorting: &SortCollection,
    records: &[RecordEntry],
) -> Result<Vec<u8>, String> {
    encode_sort_data_inner(sorting, records, false)
}

fn encode_sort_data_inner(
    sorting: &SortCollection,
    records: &[RecordEntry],
    validate: bool,
) -> Result<Vec<u8>, String> {
    if validate {
        validate_sort_structure(sorting, records)?;
    } else if sorting.columns.len() > MAX_COLUMNS
        || sorting.composites.len() > MAX_COLUMNS
        || records.len() > MAX_RECORDS
    {
        return Err("constructed sort metadata exceeds format limits".into());
    }
    let mut out = Vec::new();
    start(&mut out, SORT_MAGIC);
    put_u32(&mut out, records.len() as u32);
    put_u32(&mut out, sorting.columns.len() as u32);
    put_u32(&mut out, sorting.composites.len() as u32);
    let mut columns = sorting
        .columns
        .iter()
        .map(|(path, column)| {
            leaf_codec()
                .serialize(path)
                .map(|key| (key, column))
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    columns.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, column) in columns {
        write_leaf(&mut out, column)?;
    }
    for composite in &sorting.composites {
        write_leaf(&mut out, composite)?;
    }
    finish(out)
}

pub(crate) fn decode_sort_data(
    bytes: &[u8],
    records: &CheckedRecordSlots,
    definition: &CollectionDefinition,
) -> Result<SortCollection, String> {
    let mut cursor = Cursor::new(bytes)?;
    check_header(&mut cursor, SORT_MAGIC)?;
    if cursor.u32()? != records.slot_count() {
        return Err("sort slot domain mismatch".into());
    }
    let column_count = cursor.count(MAX_COLUMNS, 4)?;
    let composite_count = cursor.count(MAX_COLUMNS, 4)?;
    let mut columns = HashMap::with_capacity(column_count);
    for _ in 0..column_count {
        let column: SortColumn = read_leaf(&mut cursor)?;
        if columns.insert(column.path.clone(), column).is_some() {
            return Err("duplicate sort column".into());
        }
    }
    let mut composites = Vec::with_capacity(composite_count);
    for _ in 0..composite_count {
        composites.push(read_leaf::<CompositeSortOrder>(&mut cursor)?);
    }
    cursor.end()?;
    let sorting = SortCollection {
        columns,
        composites,
    };
    validate_sort_structure(&sorting, &records.records)?;
    if sorting.columns.len() != definition.sort_fields.len() {
        return Err("sort field coverage mismatch".into());
    }
    for path in &definition.sort_fields {
        let column = sorting.columns.get(path).ok_or("sort path missing")?;
        let config = definition.sort_overrides.get(path);
        if column.kind != definition.schema.field(path)?.kind
            || column.locale
                != config.map_or(definition.sort_locale.as_str(), |v| v.locale.as_str())
            || column.options != config.map_or(definition.sort_options, |v| v.options)
        {
            return Err("sort binding mismatch".into());
        }
    }
    if sorting.composites.len() != definition.schema.composite_sorts.len() {
        return Err("composite sort coverage mismatch".into());
    }
    for (composite, fields) in sorting
        .composites
        .iter()
        .zip(&definition.schema.composite_sorts)
    {
        let expected = fields
            .iter()
            .map(simple_composite_part)
            .collect::<Option<Vec<_>>>()
            .ok_or("invalid composite sort declaration")?;
        if composite.fields != expected {
            return Err("composite sort binding mismatch".into());
        }
    }
    Ok(sorting)
}

fn validate_sort_structure(
    sorting: &SortCollection,
    records: &[RecordEntry],
) -> Result<(), String> {
    if sorting.columns.len() > MAX_COLUMNS
        || sorting.composites.len() > MAX_COLUMNS
        || records.len() > MAX_RECORDS
    {
        return Err("sort metadata count exceeds limit".into());
    }
    for (path, column) in &sorting.columns {
        if &column.path != path
            || column.collation_version != COLLATION_VERSION
            || column.keys.len() != records.len()
            || !valid_permutation(&column.ascending, records.len())
            || !valid_permutation(&column.descending, records.len())
        {
            return Err("sort column identity, collation, or permutation invalid".into());
        }
        for (ordered, descending) in [(&column.ascending, false), (&column.descending, true)] {
            if ordered.windows(2).any(|pair| {
                let a = pair[0].0 as usize;
                let b = pair[1].0 as usize;
                column.keys[a]
                    .compare_with(&column.keys[b], descending, column.options)
                    .then_with(|| records[a].id.cmp(&records[b].id))
                    .is_gt()
            }) {
                return Err("sort key order invalid".into());
            }
        }
    }
    for composite in &sorting.composites {
        if composite.fields.is_empty() || !valid_permutation(&composite.order, records.len()) {
            return Err("composite sort permutation invalid".into());
        }
        let columns = composite
            .fields
            .iter()
            .map(|part| {
                sorting
                    .columns
                    .get(&part.path)
                    .map(|column| (column, part.descending))
                    .ok_or("composite sort column missing")
            })
            .collect::<Result<Vec<_>, _>>()?;
        if composite.order.windows(2).any(|pair| {
            compare_composite(&columns, pair[0], pair[1])
                .then_with(|| {
                    records[pair[0].0 as usize]
                        .id
                        .cmp(&records[pair[1].0 as usize].id)
                })
                .is_gt()
        }) {
            return Err("composite sort order invalid".into());
        }
    }
    Ok(())
}

fn valid_permutation(order: &[RecordSlot], count: usize) -> bool {
    if order.len() != count {
        return false;
    }
    let mut seen = vec![false; count];
    for slot in order {
        let Some(entry) = seen.get_mut(slot.0 as usize) else {
            return false;
        };
        if *entry {
            return false;
        }
        *entry = true;
    }
    true
}

/// RelationAndIdentity stores one identity-order permutation and individually
/// framed relation contributions. Its bitmaps are checked against record
/// classes before they may drive relation absence or fallback results.
pub(crate) fn encode_relation_identity(
    relations: &[RelationContribution],
    records: &[RecordEntry],
    identity_order: &[RecordSlot],
) -> Result<Vec<u8>, String> {
    let (logical, fallback) = record_domains(records)?;
    validate_relation_structure(relations, records, identity_order, &logical, &fallback)?;
    let mut out = Vec::new();
    start(&mut out, RELATION_MAGIC);
    put_u32(&mut out, records.len() as u32);
    put_u32(&mut out, relations.len() as u32);
    for slot in identity_order {
        put_u32(&mut out, slot.0);
    }
    for relation in relations {
        write_leaf(&mut out, relation)?;
    }
    finish(out)
}

pub(crate) fn decode_relation_identity(
    bytes: &[u8],
    records: &CheckedRecordSlots,
    definition: &CollectionDefinition,
) -> Result<CheckedRelationIdentity, String> {
    let mut cursor = Cursor::new(bytes)?;
    check_header(&mut cursor, RELATION_MAGIC)?;
    let slot_count = cursor.u32()? as usize;
    if slot_count != records.records.len() || slot_count > MAX_RECORDS {
        return Err("relation identity slot domain mismatch".into());
    }
    let relation_count = cursor.count(MAX_RELATIONS, 4)?;
    if slot_count > cursor.remaining() / 4 {
        return Err("identity order truncated".into());
    }
    let mut identity_order = Vec::with_capacity(slot_count);
    for _ in 0..slot_count {
        identity_order.push(RecordSlot(cursor.u32()?));
    }
    let mut relations = Vec::with_capacity(relation_count);
    for _ in 0..relation_count {
        relations.push(read_leaf::<RelationContribution>(&mut cursor)?);
    }
    cursor.end()?;
    validate_relation_structure(
        &relations,
        &records.records,
        &identity_order,
        &records.logical_records,
        &records.fallback_records,
    )?;
    let declarations = definition.schema.relations().values().collect::<Vec<_>>();
    if relations.len() != declarations.len()
        || relations
            .iter()
            .zip(declarations)
            .any(|(relation, declaration)| relation.declaration() != declaration)
    {
        return Err("relation declaration coverage mismatch".into());
    }
    Ok(CheckedRelationIdentity {
        identity_order,
        relations,
    })
}

fn record_domains(records: &[RecordEntry]) -> Result<(RoaringBitmap, RoaringBitmap), String> {
    if records.len() > MAX_RECORDS {
        return Err("record domain exceeds limit".into());
    }
    let mut logical = RoaringBitmap::new();
    let mut fallback = RoaringBitmap::new();
    for (index, record) in records.iter().enumerate() {
        if record.slot.0 as usize != index {
            return Err("record slots are not dense".into());
        }
        match record.class {
            RecordClass::Logical => {
                logical.insert(record.slot.0);
            }
            RecordClass::RawFallback => {
                fallback.insert(record.slot.0);
            }
        }
    }
    Ok((logical, fallback))
}

fn validate_relation_structure(
    relations: &[RelationContribution],
    records: &[RecordEntry],
    identity_order: &[RecordSlot],
    logical: &RoaringBitmap,
    fallback: &RoaringBitmap,
) -> Result<(), String> {
    if relations.len() > MAX_RELATIONS || !valid_permutation(identity_order, records.len()) {
        return Err("relation count or identity permutation invalid".into());
    }
    if identity_order
        .windows(2)
        .any(|pair| records[pair[0].0 as usize].id > records[pair[1].0 as usize].id)
    {
        return Err("record identity order invalid".into());
    }
    let mut declarations = HashSet::with_capacity(relations.len());
    for relation in relations {
        if !declarations.insert(relation.declaration().path.as_slice())
            || relation.record_count() as usize != records.len()
            || relation.unavailable() != fallback
            || !relation.present().is_subset(logical)
            || !relation.nulls().is_subset(relation.present())
            || !relation.empty_arrays().is_subset(relation.present())
        {
            return Err("relation identity or class domain invalid".into());
        }
        let mut previous = None;
        for (source, _target_id, _positions) in relation.references() {
            if source.0 as usize >= records.len()
                || !relation.present().contains(source.0)
                || !logical.contains(source.0)
                || previous.is_some_and(|last| source.0 < last)
            {
                return Err("relation reference source or target invalid".into());
            }
            previous = Some(source.0);
        }
    }
    Ok(())
}

fn valid_source_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

#[cfg(unix)]
fn native_path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(windows)]
fn native_path_bytes(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}
fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}
fn put_bytes(out: &mut Vec<u8>, bytes: &[u8], max: usize) -> Result<(), String> {
    if bytes.len() > max {
        return Err("metadata byte string exceeds limit".into());
    }
    put_u32(out, bytes.len() as u32);
    out.extend_from_slice(bytes);
    Ok(())
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self, String> {
        if bytes.len() < 16 || bytes.len() > MAX_SECTION_BYTES {
            return Err("metadata section length invalid".into());
        }
        Ok(Self { bytes, offset: 0 })
    }
    fn remaining(&self) -> usize {
        self.bytes.len() - self.offset
    }
    fn take(&mut self, len: usize) -> Result<&'a [u8], String> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or("metadata offset overflow")?;
        let slice = self
            .bytes
            .get(self.offset..end)
            .ok_or("metadata section truncated")?;
        self.offset = end;
        Ok(slice)
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn bytes(&mut self, max: usize) -> Result<&'a [u8], String> {
        let len = self.u32()? as usize;
        if len > max {
            return Err("metadata byte string exceeds limit".into());
        }
        self.take(len)
    }
    fn count(&mut self, max: usize, min_bytes_each: usize) -> Result<usize, String> {
        let count = self.u32()? as usize;
        if count > max || count > self.remaining() / min_bytes_each {
            return Err("metadata count or bounds invalid".into());
        }
        Ok(count)
    }
    fn end(&self) -> Result<(), String> {
        if self.remaining() != 0 {
            return Err("metadata section has trailing bytes".into());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "segment_metadata.test.rs"]
mod tests;
