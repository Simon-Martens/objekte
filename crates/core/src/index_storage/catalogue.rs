//! Paged file catalogue, path dictionary, and bounded diagnostics.
//!
//! File rows are grouped 64 per logical page. Variable-length native and query
//! paths live in a page-section arena; rows never own reconstructed strings.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use super::backing::Backing;
use super::dictionary::{DictionaryReader, KeyOrder};
use super::dictionary_builder::build_dictionary;
use super::error::{StorageError, StorageResult};
use super::format::{ObjectKind, ObjectView, SectionDescriptor, SectionKind};

pub const FILE_ROWS_MAGIC: [u8; 4] = *b"OBFR";
pub const FILE_PAGE_MAGIC: [u8; 4] = *b"OBFP";
pub const FILE_ROWS_VERSION: u16 = 1;
pub const FILE_ROW_LEN: usize = 192;
pub const FILE_PAGE_HEADER_LEN: usize = 16;
pub const FILE_ROWS_HEADER_LEN: usize = 48;
pub const FILES_PER_PAGE: u32 = 64;
pub const MAX_COLLECTIONS_PER_FILE: usize = 8;
pub const DIAGNOSTICS_MAGIC: [u8; 4] = *b"OBDG";
pub const DIAGNOSTICS_HEADER_LEN: usize = 16;
pub const CATALOGUE_STATS_MAGIC: [u8; 4] = *b"OBCS";
pub const CATALOGUE_STATS_LEN: usize = 64;

const FLAG_TOMBSTONE: u8 = 1;
const FLAG_CREATED: u8 = 2;
const FLAG_FINGERPRINT: u8 = 4;


/// One bounded source diagnostic supplied to the catalogue builder.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticInput {
    pub category: u8,
    pub code: String,
    pub field_path: Vec<String>,
    pub byte_offset: Option<u64>,
    pub line: Option<u64>,
    pub column: Option<u64>,
    pub message: String,
}


/// One file row supplied to the catalogue builder.
#[derive(Clone, Debug)]
pub struct FileRowInput {
    pub file_id: u32,
    pub status: u8,
    pub tombstone: bool,
    pub file_version: u64,
    pub content_generation: u64,
    pub size: u64,
    pub modified_ns: i128,
    pub created_ns: Option<i128>,
    pub native_path: Vec<u8>,
    pub query_path: Vec<u8>,
    pub collections: Vec<u32>,
    pub diagnostics: Vec<DiagnosticInput>,
    pub omitted_diagnostics: u32,
    pub fingerprint: Option<[u8; 32]>,
}


/// Catalogue-level counters.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CatalogueStats {
    pub high_water: u32,
    pub file_count: u32,
    pub active_count: u32,
    pub tombstone_count: u32,
    pub page_count: u32,
    pub collection_count: u32,
}


/// A fully decoded file row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileRowView {
    pub file_id: u32,
    pub status: u8,
    pub tombstone: bool,
    pub file_version: u64,
    pub content_generation: u64,
    pub size: u64,
    pub modified_ns: i128,
    pub created_ns: Option<i128>,
    pub native_path: Vec<u8>,
    pub query_path: Vec<u8>,
    pub collections: Vec<u32>,
    pub diagnostics_offset: u32,
    pub diagnostics_count: u32,
    pub omitted_diagnostics: u32,
    pub fingerprint: Option<[u8; 32]>,
}


/// One decoded diagnostic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticView {
    pub category: u8,
    pub code: String,
    pub field_path: Vec<String>,
    pub byte_offset: Option<u64>,
    pub line: Option<u64>,
    pub column: Option<u64>,
    pub message: String,
}


/// Built catalogue sections.
pub struct CatalogueSections {
    pub file_rows: Vec<u8>,
    pub path_dictionary: Vec<u8>,
    pub diagnostics: Vec<u8>,
    pub statistics: Vec<u8>,
    pub active_files: Vec<u8>,
    pub stats: CatalogueStats,
}


/// Build a complete catalogue object body.
pub fn build_catalogue(rows: &[FileRowInput]) -> StorageResult<CatalogueSections> {
    let derived = rows
        .iter()
        .map(|row| row.file_id.saturating_add(1))
        .max()
        .unwrap_or(0);
    build_catalogue_with_high_water(rows, derived)
}

/// Build a catalogue while preserving an allocated FileId high-water mark
/// that may exceed every currently active row.
pub fn build_catalogue_with_high_water(
    rows: &[FileRowInput],
    high_water: u32,
) -> StorageResult<CatalogueSections> {
    let mut seen = HashSet::new();
    let mut required_high_water = 0u32;
    for row in rows {
        if !seen.insert(row.file_id) {
            return Err(StorageError::InvalidArgument(
                "duplicate catalogue file ID".into(),
            ));
        }
        required_high_water = required_high_water.max(row.file_id.saturating_add(1));
        if row.collections.len() > MAX_COLLECTIONS_PER_FILE {
            return Err(StorageError::ResourceLimit(
                "file has more than eight collection memberships".into(),
            ));
        }
        if row.status > 2 {
            return Err(StorageError::InvalidArgument(
                "unsupported file status code".into(),
            ));
        }
    }
    let high_water = high_water.max(required_high_water);
    if high_water < required_high_water {
        return Err(StorageError::InvalidArgument(
            "catalogue high-water mark is below an allocated file".into(),
        ));
    }
    let page_count = rows.len().div_ceil(FILES_PER_PAGE as usize) as u32;
    let directory_len = 8usize
        .checked_mul(page_count as usize)
        .ok_or_else(|| StorageError::ResourceLimit("catalogue page directory overflow".into()))?;
    let pages_len = rows
        .chunks(FILES_PER_PAGE as usize)
        .map(|page| FILE_PAGE_HEADER_LEN + page.len() * FILE_ROW_LEN)
        .sum::<usize>();
    let arena_offset = FILE_ROWS_HEADER_LEN + directory_len + pages_len;
    let mut arena = Vec::new();
    let mut page_rows = Vec::with_capacity(rows.len());
    let mut diagnostics = vec![0u8; DIAGNOSTICS_HEADER_LEN];
    diagnostics[..4].copy_from_slice(&DIAGNOSTICS_MAGIC);
    diagnostics[4..6].copy_from_slice(&FILE_ROWS_VERSION.to_le_bytes());
    diagnostics[6..8].fill(0);
    diagnostics[8..16].fill(0);
    for row in rows {
        let native_offset = arena_offset as u64 + arena.len() as u64;
        arena.extend_from_slice(&row.native_path);
        let native_len = u32::try_from(row.native_path.len())
            .map_err(|_| StorageError::ResourceLimit("native path too large".into()))?;
        let query_offset = arena_offset as u64 + arena.len() as u64;
        arena.extend_from_slice(&row.query_path);
        let query_len = u32::try_from(row.query_path.len())
            .map_err(|_| StorageError::ResourceLimit("query path too large".into()))?;
        let diagnostics_offset = diagnostics.len() as u32;
        encode_diagnostics(&mut diagnostics, &row.diagnostics)?;
        let mut encoded = vec![0u8; FILE_ROW_LEN];
        encoded[..4].copy_from_slice(&row.file_id.to_le_bytes());
        encoded[4] = row.status;
        let mut flags = 0u8;
        if row.tombstone {
            flags |= FLAG_TOMBSTONE;
        }
        if row.created_ns.is_some() {
            flags |= FLAG_CREATED;
        }
        if row.fingerprint.is_some() {
            flags |= FLAG_FINGERPRINT;
        }
        encoded[5] = flags;
        encoded[6..8].copy_from_slice(&(row.collections.len() as u16).to_le_bytes());
        encoded[8..16].copy_from_slice(&row.file_version.to_le_bytes());
        encoded[16..24].copy_from_slice(&row.content_generation.to_le_bytes());
        encoded[24..32].copy_from_slice(&row.size.to_le_bytes());
        encoded[32..48].copy_from_slice(&row.modified_ns.to_le_bytes());
        encoded[48..64].copy_from_slice(&row.created_ns.unwrap_or(0).to_le_bytes());
        encoded[64..72].copy_from_slice(&native_offset.to_le_bytes());
        encoded[72..76].copy_from_slice(&native_len.to_le_bytes());
        encoded[80..88].copy_from_slice(&query_offset.to_le_bytes());
        encoded[88..92].copy_from_slice(&query_len.to_le_bytes());
        encoded[92..96].copy_from_slice(&diagnostics_offset.to_le_bytes());
        encoded[96..100].copy_from_slice(&(row.diagnostics.len() as u32).to_le_bytes());
        encoded[100..104].copy_from_slice(&row.omitted_diagnostics.to_le_bytes());
        if let Some(fingerprint) = row.fingerprint {
            encoded[104..136].copy_from_slice(&fingerprint);
        }
        for (index, collection) in row.collections.iter().enumerate() {
            let at = 136 + index * 4;
            encoded[at..at + 4].copy_from_slice(&collection.to_le_bytes());
        }
        page_rows.push(encoded);
    }
    let mut file_rows = vec![0u8; FILE_ROWS_HEADER_LEN];
    file_rows[..4].copy_from_slice(&FILE_ROWS_MAGIC);
    file_rows[4..6].copy_from_slice(&FILE_ROWS_VERSION.to_le_bytes());
    file_rows[6..8].fill(0);
    file_rows[8..12].copy_from_slice(&high_water.to_le_bytes());
    file_rows[12..16].copy_from_slice(&page_count.to_le_bytes());
    file_rows[16..20].copy_from_slice(&FILES_PER_PAGE.to_le_bytes());
    file_rows[20..24].fill(0);
    file_rows[24..32].copy_from_slice(&(arena_offset as u64).to_le_bytes());
    file_rows[32..40].copy_from_slice(&(arena.len() as u64).to_le_bytes());
    let mut page_offsets = Vec::with_capacity(page_count as usize);
    let mut pages = Vec::new();
    for (index, chunk) in page_rows.chunks(FILES_PER_PAGE as usize).enumerate() {
        let offset = FILE_ROWS_HEADER_LEN + directory_len + pages.len();
        page_offsets.push(offset as u64);
        let mut page = vec![0u8; FILE_PAGE_HEADER_LEN];
        page[..4].copy_from_slice(&FILE_PAGE_MAGIC);
        page[4..6].copy_from_slice(&(chunk.len() as u16).to_le_bytes());
        page[6..8].fill(0);
        let data_len = FILE_PAGE_HEADER_LEN + chunk.len() * FILE_ROW_LEN;
        page[8..12].copy_from_slice(&(data_len as u32).to_le_bytes());
        page[12..16].fill(0);
        for row in chunk {
            page.extend_from_slice(row);
        }
        pages.extend_from_slice(&page);
        let _ = index;
    }
    for offset in page_offsets {
        file_rows.extend_from_slice(&offset.to_le_bytes());
    }
    file_rows.extend_from_slice(&pages);
    file_rows.extend_from_slice(&arena);
    let mut path_entries = Vec::with_capacity(rows.len());
    for row in rows {
        let value = if row.tombstone {
            u32::MAX.to_le_bytes().to_vec()
        } else {
            row.file_id.to_le_bytes().to_vec()
        };
        path_entries.push((row.query_path.clone(), value));
    }
    path_entries.sort_by(|left, right| left.0.cmp(&right.0));
    if path_entries.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(StorageError::InvalidArgument(
            "duplicate query path in catalogued files".into(),
        ));
    }
    let path_dictionary = build_dictionary(path_entries, KeyOrder::Bytes, 8192)?;
    let active_count = rows.iter().filter(|row| !row.tombstone).count() as u32;
    let tombstone_count = rows.len() as u32 - active_count;
    let collection_count = rows
        .iter()
        .flat_map(|row| row.collections.iter())
        .copied()
        .max()
        .map_or(0, |max| max.saturating_add(1));
    let stats = CatalogueStats {
        high_water,
        file_count: rows.len() as u32,
        active_count,
        tombstone_count,
        page_count,
        collection_count,
    };
    let mut statistics = vec![0u8; CATALOGUE_STATS_LEN];
    statistics[..4].copy_from_slice(&CATALOGUE_STATS_MAGIC);
    statistics[4..6].copy_from_slice(&FILE_ROWS_VERSION.to_le_bytes());
    statistics[6..8].fill(0);
    statistics[8..12].copy_from_slice(&high_water.to_le_bytes());
    statistics[12..16].copy_from_slice(&(rows.len() as u32).to_le_bytes());
    statistics[16..20].copy_from_slice(&active_count.to_le_bytes());
    statistics[20..24].copy_from_slice(&tombstone_count.to_le_bytes());
    statistics[24..28].copy_from_slice(&page_count.to_le_bytes());
    statistics[28..32].copy_from_slice(&collection_count.to_le_bytes());
    let active = rows
        .iter()
        .filter(|row| !row.tombstone)
        .map(|row| row.file_id)
        .collect::<roaring::RoaringBitmap>();
    let mut active_files = Vec::new();
    active
        .serialize_into(&mut active_files)
        .map_err(|error| StorageError::corrupt(error.to_string()))?;
    Ok(CatalogueSections {
        file_rows,
        path_dictionary,
        diagnostics,
        statistics,
        active_files,
        stats,
    })
}

fn encode_diagnostics(
    destination: &mut Vec<u8>,
    diagnostics: &[DiagnosticInput],
) -> StorageResult<()> {
    let count = u16::try_from(diagnostics.len())
        .map_err(|_| StorageError::ResourceLimit("too many file diagnostics".into()))?;
    destination.extend_from_slice(&count.to_le_bytes());
    for diagnostic in diagnostics {
        destination.push(diagnostic.category);
        destination.push(0);
        let flags = (diagnostic.byte_offset.is_some() as u8)
            | ((diagnostic.line.is_some() as u8) << 1)
            | ((diagnostic.column.is_some() as u8) << 2);
        destination.push(flags);
        destination.push(0);
        destination.extend_from_slice(&diagnostic.byte_offset.unwrap_or(0).to_le_bytes());
        destination.extend_from_slice(&diagnostic.line.unwrap_or(0).to_le_bytes());
        destination.extend_from_slice(&diagnostic.column.unwrap_or(0).to_le_bytes());
        destination.extend_from_slice(&[0u8; 4]);
        push_string(destination, &diagnostic.code)?;
        push_string(destination, &diagnostic.message)?;
        let fields = u32::try_from(diagnostic.field_path.len())
            .map_err(|_| StorageError::ResourceLimit("diagnostic path too large".into()))?;
        destination.extend_from_slice(&fields.to_le_bytes());
        for field in &diagnostic.field_path {
            push_string(destination, field)?;
        }
    }
    Ok(())
}

fn push_string(destination: &mut Vec<u8>, value: &str) -> StorageResult<()> {
    let length = u32::try_from(value.len())
        .map_err(|_| StorageError::ResourceLimit("diagnostic string too large".into()))?;
    destination.extend_from_slice(&length.to_le_bytes());
    destination.extend_from_slice(value.as_bytes());
    Ok(())
}

/// Validate and assemble a catalogue object.
pub fn build_catalogue_object(backing: Backing) -> StorageResult<CatalogueView> {
    CatalogueView::open(backing)
}


/// Reader over a catalogue object.
pub struct CatalogueView {
    backing: Backing,
    sections: BTreeMap<SectionKind, SectionDescriptor>,
    stats: CatalogueStats,
    path_dictionary: DictionaryReader,
    active: roaring::RoaringBitmap,
}


impl std::fmt::Debug for CatalogueView {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CatalogueView")
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}


impl CatalogueView {
    pub fn open(backing: Backing) -> StorageResult<Self> {
        let view = ObjectView::new(&backing)?;
        if view.header().kind != ObjectKind::Catalogue {
            return Err(StorageError::Incompatible(
                "object is not a catalogue".into(),
            ));
        }
        let sections = view
            .sections()
            .iter()
            .filter_map(|descriptor| descriptor.kind.map(|kind| (kind, *descriptor)))
            .collect::<BTreeMap<_, _>>();
        let statistics = sections
            .get(&SectionKind::Statistics)
            .copied()
            .ok_or_else(|| StorageError::corrupt("catalogue statistics are missing"))?;
        let stats = decode_catalogue_stats(backing.read(statistics.offset, statistics.length)?)?;
        let path_section = sections
            .get(&SectionKind::PathDictionary)
            .copied()
            .ok_or_else(|| StorageError::corrupt("path dictionary is missing"))?;
        let path_dictionary =
            DictionaryReader::open(backing.clone(), path_section.offset, path_section.length)?;
        let active_section = sections
            .get(&SectionKind::ActiveFiles)
            .copied()
            .ok_or_else(|| StorageError::corrupt("catalogue active-file index is missing"))?;
        let active = roaring::RoaringBitmap::deserialize_from(std::io::Cursor::new(
            backing.read(active_section.offset, active_section.length)?,
        ))
        .map_err(|error| StorageError::corrupt(error.to_string()))?;
        if active.len() != stats.active_count as u64 {
            return Err(StorageError::corrupt(
                "catalogue active-file cardinality differs from its statistics",
            ));
        }
        Ok(Self {
            backing,
            sections,
            stats,
            path_dictionary,
            active,
        })
    }

    pub fn stats(&self) -> CatalogueStats {
        self.stats
    }

    pub fn high_water(&self) -> u32 {
        self.stats.high_water
    }

    pub fn active_files(&self) -> &roaring::RoaringBitmap {
        &self.active
    }

    pub fn is_active(&self, file_id: u32) -> bool {
        self.active.contains(file_id)
    }

    pub fn page_count(&self) -> u32 {
        self.stats.page_count
    }

    pub fn path_dictionary(&self) -> &DictionaryReader {
        &self.path_dictionary
    }

    pub fn file(&self, file_id: u32) -> StorageResult<Option<FileRowView>> {
        if file_id >= self.stats.high_water {
            return Ok(None);
        }
        let page = file_id / FILES_PER_PAGE;
        let row = file_id % FILES_PER_PAGE;
        let descriptor = self
            .sections
            .get(&SectionKind::FileRows)
            .copied()
            .ok_or_else(|| StorageError::corrupt("file rows are missing"))?;
        let header = self
            .backing
            .read(descriptor.offset, FILE_ROWS_HEADER_LEN as u64)?;
        validate_file_rows_header(header, self.stats)?;
        if page >= self.stats.page_count {
            return Ok(None);
        }
        let directory_at = FILE_ROWS_HEADER_LEN + page as usize * 8;
        let page_offset = read_u64(
            self.backing
                .read(descriptor.offset + directory_at as u64, 8)?,
            0,
        )?;
        let page_header = self
            .backing
            .read(descriptor.offset + page_offset, FILE_PAGE_HEADER_LEN as u64)?;
        if page_header[..4] != FILE_PAGE_MAGIC {
            return Err(StorageError::corrupt("file page magic mismatch"));
        }
        let count = u16::from_le_bytes([page_header[4], page_header[5]]) as u32;
        if count > FILES_PER_PAGE {
            return Err(StorageError::corrupt("file page row count is out of range"));
        }
        if row >= count {
            return Ok(None);
        }
        let row_offset =
            page_offset + FILE_PAGE_HEADER_LEN as u64 + row as u64 * FILE_ROW_LEN as u64;
        let encoded = self
            .backing
            .read(descriptor.offset + row_offset, FILE_ROW_LEN as u64)?;
        let row = decode_file_row(encoded, &self.backing, descriptor)?;
        Ok(Some(row))
    }

    pub fn active_file(&self, file_id: u32) -> StorageResult<Option<FileRowView>> {
        Ok(self.file(file_id)?.filter(|row| !row.tombstone))
    }

    pub fn path(&self, query_path: &[u8]) -> StorageResult<Option<u32>> {
        let Some(value) = self.path_dictionary.get(query_path)? else {
            return Ok(None);
        };
        if value.len() != 4 {
            return Err(StorageError::corrupt(
                "path dictionary value is not four bytes",
            ));
        }
        let file = u32::from_le_bytes(value.try_into().expect("four-byte path value"));
        Ok((file != u32::MAX).then_some(file))
    }

    /// Active file IDs whose query path starts with `prefix`, in path order.
    pub fn paths_with_prefix(&self, prefix: &[u8], limit: usize) -> StorageResult<Vec<u32>> {
        let mut result = Vec::new();
        for entry in self.path_dictionary.range(
            std::ops::Bound::Included(prefix),
            std::ops::Bound::Unbounded,
        ) {
            let (path, value) = entry?;
            if !path.starts_with(prefix) {
                break;
            }
            let file =
                u32::from_le_bytes(value.as_slice().try_into().map_err(|_| {
                    StorageError::corrupt("path dictionary value is not four bytes")
                })?);
            if file != u32::MAX {
                result.push(file);
            }
            if result.len() >= limit {
                break;
            }
        }
        Ok(result)
    }

    pub fn diagnostics(&self, file: &FileRowView) -> StorageResult<Vec<DiagnosticView>> {
        if file.diagnostics_count == 0 {
            return Ok(Vec::new());
        }
        let section = self
            .sections
            .get(&SectionKind::Diagnostics)
            .copied()
            .ok_or_else(|| StorageError::corrupt("diagnostics are missing"))?;
        let mut at = file.diagnostics_offset as u64;
        let count_bytes = self.backing.read(section.offset + at, 2)?;
        at += 2;
        let count = u16::from_le_bytes([count_bytes[0], count_bytes[1]]) as usize;
        if count as u32 != file.diagnostics_count {
            return Err(StorageError::corrupt(
                "diagnostic count differs from its file row",
            ));
        }
        let mut diagnostics = Vec::with_capacity(count);
        for _ in 0..count {
            let header = self.backing.read(section.offset + at, 32)?;
            at += 32;
            let category = header[0];
            let flags = header[2];
            if header[1] != 0 || header[3] != 0 {
                return Err(StorageError::corrupt(
                    "diagnostic reserved bytes are not zero",
                ));
            }
            let byte_offset = (flags & 1 != 0).then(|| read_u64(header, 4)).transpose()?;
            let line = (flags & 2 != 0).then(|| read_u64(header, 12)).transpose()?;
            let column = (flags & 4 != 0).then(|| read_u64(header, 20)).transpose()?;
            let (code, next) = read_string(&self.backing, section.offset, at)?;
            at = next;
            let (message, next) = read_string(&self.backing, section.offset, at)?;
            at = next;
            let fields_bytes = self.backing.read(section.offset + at, 4)?;
            at += 4;
            let fields = u32::from_le_bytes(fields_bytes.try_into().expect("field count")) as usize;
            let mut field_path = Vec::with_capacity(fields);
            for _ in 0..fields {
                let (field, next) = read_string(&self.backing, section.offset, at)?;
                at = next;
                field_path.push(field);
            }
            diagnostics.push(DiagnosticView {
                category,
                code,
                field_path,
                byte_offset,
                line,
                column,
                message,
            });
        }
        Ok(diagnostics)
    }

    pub fn verify_full(&self) -> StorageResult<()> {
        ObjectView::new(&self.backing)?.verify_full()?;
        for file in 0..self.stats.high_water {
            if let Some(row) = self.file(file)? {
                self.diagnostics(&row)?;
            }
        }
        self.path_dictionary.verify_full()?;
        Ok(())
    }
}


fn validate_file_rows_header(header: &[u8], stats: CatalogueStats) -> StorageResult<()> {
    if header[..4] != FILE_ROWS_MAGIC {
        return Err(StorageError::corrupt("file rows magic mismatch"));
    }
    let version = u16::from_le_bytes([header[4], header[5]]);
    if version != FILE_ROWS_VERSION {
        return Err(StorageError::UnsupportedFormat(format!(
            "file rows version {version}"
        )));
    }
    if header[6..8] != [0, 0] {
        return Err(StorageError::corrupt(
            "file rows header reserved bytes are not zero",
        ));
    }
    let high_water = read_u32(header, 8)?;
    let page_count = read_u32(header, 12)?;
    let rows_per_page = read_u32(header, 16)?;
    if high_water != stats.high_water
        || page_count != stats.page_count
        || rows_per_page != FILES_PER_PAGE
    {
        return Err(StorageError::corrupt(
            "file rows header disagrees with catalogue statistics",
        ));
    }
    Ok(())
}

fn decode_file_row(
    encoded: &[u8],
    backing: &Backing,
    descriptor: SectionDescriptor,
) -> StorageResult<FileRowView> {
    if encoded.len() != FILE_ROW_LEN {
        return Err(StorageError::corrupt("file row length mismatch"));
    }
    if encoded[168..].iter().any(|byte| *byte != 0) {
        return Err(StorageError::corrupt(
            "file row reserved bytes are not zero",
        ));
    }
    let flags = encoded[5];
    if flags & !(FLAG_TOMBSTONE | FLAG_CREATED | FLAG_FINGERPRINT) != 0 {
        return Err(StorageError::corrupt("file row has unknown flag bits"));
    }
    let tombstone = flags & FLAG_TOMBSTONE != 0;
    let created_ns = (flags & FLAG_CREATED != 0)
        .then(|| i128::from_le_bytes(encoded[48..64].try_into().expect("created time")));
    let fingerprint = (flags & FLAG_FINGERPRINT != 0).then(|| {
        encoded[104..136]
            .try_into()
            .expect("thirty-two-byte fingerprint")
    });
    let native = read_blob(
        backing,
        descriptor,
        read_u64(encoded, 64)?,
        read_u32(encoded, 72)?,
    )?;
    let query = read_blob(
        backing,
        descriptor,
        read_u64(encoded, 80)?,
        read_u32(encoded, 88)?,
    )?;
    let collection_count = u16::from_le_bytes([encoded[6], encoded[7]]) as usize;
    if collection_count > MAX_COLLECTIONS_PER_FILE {
        return Err(StorageError::corrupt(
            "file row has too many collection memberships",
        ));
    }
    let mut collections = Vec::with_capacity(collection_count);
    for index in 0..collection_count {
        let at = 136 + index * 4;
        collections.push(u32::from_le_bytes(
            encoded[at..at + 4].try_into().expect("collection ID"),
        ));
    }
    Ok(FileRowView {
        file_id: read_u32(encoded, 0)?,
        status: encoded[4],
        tombstone,
        file_version: read_u64(encoded, 8)?,
        content_generation: read_u64(encoded, 16)?,
        size: read_u64(encoded, 24)?,
        modified_ns: i128::from_le_bytes(encoded[32..48].try_into().expect("modified time")),
        created_ns,
        native_path: native.to_vec(),
        query_path: query.to_vec(),
        collections,
        diagnostics_offset: read_u32(encoded, 92)?,
        diagnostics_count: read_u32(encoded, 96)?,
        omitted_diagnostics: read_u32(encoded, 100)?,
        fingerprint,
    })
}

fn read_blob(
    backing: &Backing,
    descriptor: SectionDescriptor,
    offset: u64,
    length: u32,
) -> StorageResult<&[u8]> {
    let end = offset
        .checked_add(length as u64)
        .ok_or_else(|| StorageError::corrupt("catalogue string reference overflow"))?;
    if end > descriptor.length {
        return Err(StorageError::corrupt(
            "catalogue string reference is out of bounds",
        ));
    }
    backing.read(descriptor.offset + offset, length as u64)
}

fn read_string(backing: &Backing, base: u64, at: u64) -> StorageResult<(String, u64)> {
    let length_bytes = backing.read(base + at, 4)?;
    let length = u32::from_le_bytes(length_bytes.try_into().expect("string length")) as u64;
    let value = backing.read(base + at + 4, length)?;
    let value = std::str::from_utf8(value)
        .map_err(|_| StorageError::corrupt("diagnostic string is not UTF-8"))?;
    Ok((value.to_owned(), at + 4 + length))
}

fn decode_catalogue_stats(bytes: &[u8]) -> StorageResult<CatalogueStats> {
    if bytes.len() < CATALOGUE_STATS_LEN {
        return Err(StorageError::corrupt("truncated catalogue statistics"));
    }
    if bytes[..4] != CATALOGUE_STATS_MAGIC {
        return Err(StorageError::corrupt("catalogue statistics magic mismatch"));
    }
    if bytes[6..8] != [0, 0] || bytes[32..].iter().any(|byte| *byte != 0) {
        return Err(StorageError::corrupt(
            "catalogue statistics reserved bytes are not zero",
        ));
    }
    Ok(CatalogueStats {
        high_water: read_u32(bytes, 8)?,
        file_count: read_u32(bytes, 12)?,
        active_count: read_u32(bytes, 16)?,
        tombstone_count: read_u32(bytes, 20)?,
        page_count: read_u32(bytes, 24)?,
        collection_count: read_u32(bytes, 28)?,
    })
}

fn read_u32(bytes: &[u8], at: usize) -> StorageResult<u32> {
    let end = at
        .checked_add(4)
        .ok_or_else(|| StorageError::corrupt("catalogue u32 offset overflow"))?;
    let slice = bytes
        .get(at..end)
        .ok_or_else(|| StorageError::corrupt("truncated catalogue u32"))?;
    Ok(u32::from_le_bytes(
        slice.try_into().expect("four-byte catalogue field"),
    ))
}

fn read_u64(bytes: &[u8], at: usize) -> StorageResult<u64> {
    let end = at
        .checked_add(8)
        .ok_or_else(|| StorageError::corrupt("catalogue u64 offset overflow"))?;
    let slice = bytes
        .get(at..end)
        .ok_or_else(|| StorageError::corrupt("truncated catalogue u64"))?;
    Ok(u64::from_le_bytes(
        slice.try_into().expect("eight-byte catalogue field"),
    ))
}

/// Convenience used by the segment builder for its resident backing.
pub fn resident_catalogue(bytes: Arc<[u8]>) -> StorageResult<CatalogueView> {
    CatalogueView::open(Backing::resident(bytes))
}
