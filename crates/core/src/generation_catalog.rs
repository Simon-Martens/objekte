//! Checked, source-free metadata for one immutable managed generation.
//!
//! Segment record slots contain only a `metadata_ref`. Untagged refs address
//! the sparse current catalog. High-bit tagged refs address an append-only
//! historical table whose indices remain stable across generations.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::inventory::{
    ByteRange, FileEntry, FileStatus, FileVersion, SharedPath, SourceDiagnostic, SourceRevision,
    MAX_DIAGNOSTICS_PER_FILE, MAX_DIAGNOSTIC_CODE_BYTES, MAX_DIAGNOSTIC_MESSAGE_BYTES,
    MAX_DIAGNOSTIC_PATH_SEGMENTS, MAX_DIAGNOSTIC_SEGMENT_BYTES,
};
use crate::lifecycle::PublishedIndex;
use crate::model::{CollectionId, FileId};
use crate::source::DiagnosticCategory;

const MAGIC: &[u8; 8] = b"OBJHCAT1";
const VERSION: u32 = 2;
const HISTORICAL_TAG: u32 = 1 << 31;
const HEADER_BYTES: usize = 8 + 4 + 8 + 32;
const MAX_BYTES: usize = 512 * 1024 * 1024;
const MAX_RESTORED_TABLE_BYTES: usize = 1024 * 1024 * 1024;
const MAX_FILES: usize = 16_000_000;
const MAX_PATH_BYTES: usize = 64 * 1024;
const MAX_SOURCE_ID_BYTES: usize = 256;
const MAX_DIAGNOSTIC_BYTES: usize = 16 * 1024;

type CatalogSlot = Option<(FileEntry, Option<[u8; 32]>, Option<[u8; 32]>)>;

/// A private encoder that assigns segment metadata refs while collecting the
/// one supplemental copy needed for each distinct historical incarnation.
pub(crate) struct GenerationCatalogBuilder<'a> {
    publication: &'a PublishedIndex,
    configuration_identity: [u8; 32],
    revision: u64,
    next_segment_ordinal: u64,
    collection_count: usize,
    historical: Vec<FileEntry>,
    historical_by_file: HashMap<FileId, Vec<u32>>,
}

impl<'a> GenerationCatalogBuilder<'a> {
    pub(crate) fn from_publication(publication: &'a PublishedIndex) -> Result<Self, String> {
        Self::from_publication_with_history(publication, &[])
    }

    /// Carry forward the durable historical table in its original order so
    /// unchanged segment files retain the meaning of tagged references.
    pub(crate) fn from_publication_with_history(
        publication: &'a PublishedIndex,
        prior: &[FileEntry],
    ) -> Result<Self, String> {
        if !publication.catalog.construction_complete() {
            return Err("cannot persist an incomplete file catalog".into());
        }
        if publication.catalog.allocated_files() > MAX_FILES
            || publication.catalog.allocated_files() != publication.file_versions.len()
        {
            return Err("managed catalog file count exceeds limit".into());
        }
        if prior.len() > MAX_FILES {
            return Err("managed historical file count exceeds limit".into());
        }
        let mut historical_by_file = HashMap::<FileId, Vec<u32>>::new();
        for (index, entry) in prior.iter().enumerate() {
            if entry.id.0 as usize >= publication.catalog.allocated_files() {
                return Err("historical file ID is outside the catalog".into());
            }
            validate_file(entry, publication.repository.collections().len())?;
            let references = historical_by_file.entry(entry.id).or_default();
            if references
                .iter()
                .any(|reference| prior[(reference & !HISTORICAL_TAG) as usize] == *entry)
            {
                return Err("duplicate historical metadata".into());
            }
            references.push(HISTORICAL_TAG | index as u32);
        }
        let builder = Self {
            publication,
            configuration_identity: publication.configuration_identity,
            revision: publication.revision.0,
            next_segment_ordinal: publication.next_segment_ordinal,
            collection_count: publication.repository.collections().len(),
            historical: prior.to_vec(),
            historical_by_file,
        };
        builder.validate_header_state()?;
        Ok(builder)
    }

    pub(crate) fn historical_entries(&self) -> &[FileEntry] {
        &self.historical
    }

    /// Return a stable ref accepted by `segment_metadata::encode_record_slots`.
    /// A current ref is used only for full metadata equality, including path,
    /// diagnostics and revision. Retired holes never resolve as current refs.
    pub(crate) fn intern(&mut self, entry: &FileEntry) -> Result<u32, String> {
        let number = entry.id.0 as usize;
        if number >= self.publication.catalog.allocated_files() {
            return Err("historical file ID is outside the catalog".into());
        }
        validate_file(entry, self.collection_count)?;
        if self.publication.catalog.entry(entry.id) == Some(entry) {
            return Ok(entry.id.0);
        }
        if let Some(existing) = self.historical_by_file.get(&entry.id) {
            for &reference in existing {
                if self.historical[(reference & !HISTORICAL_TAG) as usize] == *entry {
                    return Ok(reference);
                }
            }
        }
        if self.historical.len() >= MAX_FILES {
            return Err("managed historical file count exceeds limit".into());
        }
        let reference = HISTORICAL_TAG | self.historical.len() as u32;
        self.historical.push(entry.clone());
        self.historical_by_file
            .entry(entry.id)
            .or_default()
            .push(reference);
        Ok(reference)
    }

    pub(crate) fn encode(self) -> Result<Vec<u8>, String> {
        self.validate_header_state()?;
        let mut current_paths = HashSet::new();
        let mut bytes = vec![0; HEADER_BYTES];
        bytes.extend_from_slice(&self.configuration_identity);
        bytes.extend_from_slice(&self.revision.to_le_bytes());
        bytes.extend_from_slice(&self.next_segment_ordinal.to_le_bytes());
        bytes.extend_from_slice(
            &u32::try_from(self.collection_count)
                .map_err(|_| "too many collections")?
                .to_le_bytes(),
        );
        bytes.extend_from_slice(
            &u32::try_from(self.publication.catalog.allocated_files())
                .map_err(|_| "too many files")?
                .to_le_bytes(),
        );
        bytes.extend_from_slice(
            &u32::try_from(self.historical.len())
                .map_err(|_| "too many historical files")?
                .to_le_bytes(),
        );
        for number in 0..self.publication.catalog.allocated_files() {
            let id = FileId(number as u32);
            let version = self
                .publication
                .file_versions
                .get(id)
                .ok_or("file version missing")?;
            bytes.extend_from_slice(&version.0.to_le_bytes());
            match self.publication.catalog.entry(id) {
                None => bytes.push(0),
                Some(entry) => {
                    if entry.id.0 as usize != number
                        || !current_paths.insert(entry.relative_path.as_path())
                    {
                        return Err("invalid or duplicate current catalog entry".into());
                    }
                    bytes.push(1);
                    encode_file(&mut bytes, entry, self.collection_count)?;
                    encode_optional_hash(&mut bytes, &self.publication.catalog.fingerprint(id));
                    encode_optional_hash(&mut bytes, &self.publication.catalog.configuration(id));
                }
            }
            if bytes.len() > MAX_BYTES {
                return Err("managed catalog exceeds byte limit".into());
            }
        }
        let mut distinct = HashMap::<FileId, Vec<&FileEntry>>::new();
        for entry in &self.historical {
            if entry.id.0 as usize >= self.publication.catalog.allocated_files()
                || distinct
                    .get(&entry.id)
                    .is_some_and(|known| known.contains(&entry))
            {
                return Err("duplicate or invalid historical metadata".into());
            }
            distinct.entry(entry.id).or_default().push(entry);
            encode_file(&mut bytes, entry, self.collection_count)?;
            if bytes.len() > MAX_BYTES {
                return Err("managed catalog exceeds byte limit".into());
            }
        }
        if bytes.len() > MAX_BYTES {
            return Err("managed catalog exceeds byte limit".into());
        }
        let body_len = (bytes.len() - HEADER_BYTES) as u64;
        let checksum = Sha256::digest(&bytes[HEADER_BYTES..]);
        bytes[..8].copy_from_slice(MAGIC);
        bytes[8..12].copy_from_slice(&VERSION.to_le_bytes());
        bytes[12..20].copy_from_slice(&body_len.to_le_bytes());
        bytes[20..HEADER_BYTES].copy_from_slice(&checksum);
        Ok(bytes)
    }

    fn validate_header_state(&self) -> Result<(), String> {
        if self.revision == 0
            || self.next_segment_ordinal == 0
            || self.collection_count == 0
            || self.collection_count > u32::MAX as usize
            || self.publication.catalog.allocated_files() > MAX_FILES
            || self.publication.file_versions.len() != self.publication.catalog.allocated_files()
            || (0..self.publication.file_versions.len()).any(|number| {
                self.publication
                    .file_versions
                    .get(FileId(number as u32))
                    .is_none_or(|version| version.0 == 0 || version.0 > self.revision)
            })
        {
            return Err("managed catalog dimensions or revision invalid".into());
        }
        Ok(())
    }
}

/// Decoded state suitable for `FileCatalog::restored` and segment metadata
/// resolution without inspecting source files.
#[derive(Debug)]
pub(crate) struct RestoredGenerationCatalog {
    pub(crate) configuration_identity: [u8; 32],
    pub(crate) revision: u64,
    pub(crate) next_segment_ordinal: u64,
    pub(crate) slots: Vec<CatalogSlot>,
    pub(crate) file_versions: Vec<FileVersion>,
    historical: Vec<FileEntry>,
}

impl RestoredGenerationCatalog {
    pub(crate) fn decode(
        bytes: &[u8],
        expected_identity: [u8; 32],
        collection_count: usize,
    ) -> Result<Self, String> {
        if bytes.len() < HEADER_BYTES || bytes.len() > MAX_BYTES || &bytes[..8] != MAGIC {
            return Err("invalid managed catalog envelope".into());
        }
        let mut header = Reader::new(&bytes[8..HEADER_BYTES]);
        if header.u32()? != VERSION {
            return Err("unsupported managed catalog version".into());
        }
        let body_len =
            usize::try_from(header.u64()?).map_err(|_| "catalog body length overflow")?;
        let digest = header.fixed::<32>()?;
        if body_len != bytes.len() - HEADER_BYTES
            || digest != Sha256::digest(&bytes[HEADER_BYTES..]).as_slice()
        {
            return Err("managed catalog length or checksum mismatch".into());
        }
        let mut input = Reader::new(&bytes[HEADER_BYTES..]);
        let configuration_identity = input.fixed::<32>()?;
        let revision = input.u64()?;
        let next_segment_ordinal = input.u64()?;
        let stored_collection_count = input.u32()? as usize;
        let slot_count = input.u32()? as usize;
        let historical_count = input.u32()? as usize;
        let table_bytes = slot_count
            .checked_mul(std::mem::size_of::<CatalogSlot>() + std::mem::size_of::<FileVersion>())
            .and_then(|current| {
                historical_count
                    .checked_mul(std::mem::size_of::<FileEntry>())
                    .and_then(|historical| current.checked_add(historical))
            });
        if configuration_identity != expected_identity
            || revision == 0
            || next_segment_ordinal == 0
            || stored_collection_count != collection_count
            || collection_count == 0
            || slot_count > MAX_FILES
            || historical_count > MAX_FILES
            || slot_count > input.remaining() / 9
            || table_bytes.is_none_or(|bytes| bytes > MAX_RESTORED_TABLE_BYTES)
        {
            return Err("managed catalog identity or dimensions mismatch".into());
        }
        let mut slots = Vec::with_capacity(slot_count);
        let mut versions = Vec::with_capacity(slot_count);
        for number in 0..slot_count {
            let version = FileVersion(input.u64()?);
            if version.0 == 0 || version.0 > revision {
                return Err("invalid managed file version".into());
            }
            versions.push(version);
            let slot = match input.u8()? {
                0 => None,
                1 => {
                    let entry = decode_file(&mut input, collection_count)?;
                    if entry.id.0 as usize != number {
                        return Err("invalid or duplicate current catalog entry".into());
                    }
                    Some((
                        entry,
                        decode_optional_hash(&mut input)?,
                        decode_optional_hash(&mut input)?,
                    ))
                }
                _ => return Err("invalid managed file slot marker".into()),
            };
            slots.push(slot);
        }
        let mut paths = slots
            .iter()
            .filter_map(|slot| {
                slot.as_ref()
                    .map(|(entry, _, _)| entry.relative_path.as_path())
            })
            .collect::<Vec<_>>();
        paths.sort_unstable();
        if paths.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err("duplicate current catalog path".into());
        }
        if historical_count > input.remaining() / 4 {
            return Err("managed historical metadata count exceeds bytes".into());
        }
        let mut historical = Vec::with_capacity(historical_count.min(input.remaining() / 4));
        let mut distinct = HashMap::<FileId, Vec<usize>>::new();
        for _ in 0..historical_count {
            let entry = decode_file(&mut input, collection_count)?;
            if entry.id.0 as usize >= slot_count
                || distinct
                    .get(&entry.id)
                    .is_some_and(|known| known.iter().any(|&index| historical[index] == entry))
            {
                return Err("duplicate or invalid historical metadata".into());
            }
            distinct.entry(entry.id).or_default().push(historical.len());
            historical.push(entry);
        }
        if input.remaining() != 0 {
            return Err("trailing managed catalog bytes".into());
        }
        Ok(Self {
            configuration_identity,
            revision,
            next_segment_ordinal,
            slots,
            file_versions: versions,
            historical,
        })
    }

    pub(crate) fn resolve_metadata(&self, reference: u32) -> Option<&FileEntry> {
        if reference & HISTORICAL_TAG != 0 {
            self.historical.get((reference & !HISTORICAL_TAG) as usize)
        } else {
            self.slots
                .get(reference as usize)?
                .as_ref()
                .map(|(entry, _, _)| entry)
        }
    }

    pub(crate) fn historical_entries(&self) -> &[FileEntry] {
        &self.historical
    }
}

fn validate_file(entry: &FileEntry, collection_count: usize) -> Result<(), String> {
    let path = entry.relative_path.as_path();
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || !path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        || path_bytes(path).len() > MAX_PATH_BYTES
        || entry.collections.is_empty()
        || entry.collections.len() > collection_count
        || entry
            .collections
            .iter()
            .any(|id| id.0 as usize >= collection_count)
        || entry
            .collections
            .iter()
            .copied()
            .collect::<HashSet<_>>()
            .len()
            != entry.collections.len()
        || entry.diagnostics.len() > MAX_DIAGNOSTICS_PER_FILE
    {
        return Err("invalid managed file path, membership or diagnostic count".into());
    }
    let derived_status = entry
        .diagnostics
        .iter()
        .fold(FileStatus::Valid, |status, diagnostic| {
            status.max(match diagnostic.category {
                DiagnosticCategory::Io => FileStatus::Unreadable,
                _ => FileStatus::Broken,
            })
        });
    if (entry.omitted_diagnostics == 0 && entry.status != derived_status)
        || (entry.omitted_diagnostics != 0
            && (entry.status == FileStatus::Valid
                || derived_status == FileStatus::Valid
                || entry.status < derived_status))
    {
        return Err("managed file status and diagnostics disagree".into());
    }
    for diagnostic in &entry.diagnostics {
        if diagnostic.file != entry.id
            || !entry.collections.contains(&diagnostic.collection)
            || diagnostic.source_id.is_empty()
            || diagnostic.source_id.len() > MAX_SOURCE_ID_BYTES
            || diagnostic.source_version == 0
            || diagnostic.code.is_empty()
            || diagnostic.code.len() > MAX_DIAGNOSTIC_CODE_BYTES
            || diagnostic.message.len() > MAX_DIAGNOSTIC_MESSAGE_BYTES
            || diagnostic.field_path.as_ref().is_some_and(|parts| {
                parts.len() > MAX_DIAGNOSTIC_PATH_SEGMENTS
                    || parts
                        .iter()
                        .any(|part| part.len() > MAX_DIAGNOSTIC_SEGMENT_BYTES)
            })
            || diagnostic.range.is_some_and(|range| {
                ByteRange::checked(range.start, range.end, entry.revision.size).is_err()
            })
            || diagnostic
                .byte_offset
                .is_some_and(|offset| offset > entry.revision.size)
        {
            return Err("invalid managed source diagnostic".into());
        }
    }
    Ok(())
}

fn encode_file(
    out: &mut Vec<u8>,
    entry: &FileEntry,
    collection_count: usize,
) -> Result<(), String> {
    validate_file(entry, collection_count)?;
    out.extend_from_slice(&entry.id.0.to_le_bytes());
    encode_bytes(
        out,
        &path_bytes(entry.relative_path.as_path()),
        MAX_PATH_BYTES,
    )?;
    out.extend_from_slice(&entry.revision.modified_ns.to_le_bytes());
    out.extend_from_slice(&entry.revision.size.to_le_bytes());
    encode_optional_i128(out, entry.created_ns);
    out.extend_from_slice(
        &u32::try_from(entry.collections.len())
            .map_err(|_| "too many memberships")?
            .to_le_bytes(),
    );
    for id in &entry.collections {
        out.extend_from_slice(&id.0.to_le_bytes());
    }
    out.push(entry.status.code());
    out.extend_from_slice(&entry.omitted_diagnostics.to_le_bytes());
    out.extend_from_slice(
        &u32::try_from(entry.diagnostics.len())
            .map_err(|_| "too many diagnostics")?
            .to_le_bytes(),
    );
    for diagnostic in &entry.diagnostics {
        let encoded = serde_json::to_vec(diagnostic).map_err(|error| error.to_string())?;
        encode_bytes(out, &encoded, MAX_DIAGNOSTIC_BYTES)?;
    }
    Ok(())
}

fn decode_file(input: &mut Reader<'_>, collection_count: usize) -> Result<FileEntry, String> {
    let id = FileId(input.u32()?);
    let relative_path = path_from_bytes(input.bytes(MAX_PATH_BYTES)?)?;
    let modified_ns = input.i128()?;
    let size = input.u64()?;
    let created_ns = decode_optional_i128(input)?;
    let memberships = input.u32()? as usize;
    if memberships == 0 || memberships > collection_count || memberships > input.remaining() / 4 {
        return Err("invalid managed collection memberships".into());
    }
    let mut collections = Vec::with_capacity(memberships);
    for _ in 0..memberships {
        collections.push(CollectionId(input.u32()?));
    }
    let status = match input.u8()? {
        0 => FileStatus::Valid,
        1 => FileStatus::Broken,
        2 => FileStatus::Unreadable,
        _ => return Err("invalid managed file status".into()),
    };
    let omitted_diagnostics = input.u32()?;
    let diagnostic_count = input.u32()? as usize;
    if diagnostic_count > MAX_DIAGNOSTICS_PER_FILE || diagnostic_count > input.remaining() / 4 {
        return Err("invalid managed diagnostic count".into());
    }
    let mut diagnostics = Vec::with_capacity(diagnostic_count);
    for _ in 0..diagnostic_count {
        let bytes = input.bytes(MAX_DIAGNOSTIC_BYTES)?;
        diagnostics.push(
            serde_json::from_slice::<SourceDiagnostic>(bytes)
                .map_err(|error| format!("invalid managed source diagnostic: {error}"))?,
        );
    }
    let entry = FileEntry {
        id,
        relative_path: SharedPath::from(relative_path),
        revision: SourceRevision { modified_ns, size },
        created_ns,
        collections,
        status,
        diagnostics,
        omitted_diagnostics,
    };
    validate_file(&entry, collection_count)?;
    Ok(entry)
}

fn encode_optional_hash(out: &mut Vec<u8>, value: &Option<[u8; 32]>) {
    match value {
        Some(value) => {
            out.push(1);
            out.extend_from_slice(value);
        }
        None => out.push(0),
    }
}

fn decode_optional_hash(input: &mut Reader<'_>) -> Result<Option<[u8; 32]>, String> {
    match input.u8()? {
        0 => Ok(None),
        1 => Ok(Some(input.fixed::<32>()?)),
        _ => Err("invalid managed hash marker".into()),
    }
}

fn encode_optional_i128(out: &mut Vec<u8>, value: Option<i128>) {
    match value {
        Some(value) => {
            out.push(1);
            out.extend_from_slice(&value.to_le_bytes());
        }
        None => out.push(0),
    }
}

fn decode_optional_i128(input: &mut Reader<'_>) -> Result<Option<i128>, String> {
    match input.u8()? {
        0 => Ok(None),
        1 => Ok(Some(input.i128()?)),
        _ => Err("invalid managed timestamp marker".into()),
    }
}

fn encode_bytes(out: &mut Vec<u8>, bytes: &[u8], maximum: usize) -> Result<(), String> {
    if bytes.len() > maximum {
        return Err("managed metadata field exceeds byte limit".into());
    }
    out.extend_from_slice(
        &u32::try_from(bytes.len())
            .map_err(|_| "metadata length overflow")?
            .to_le_bytes(),
    );
    out.extend_from_slice(bytes);
    Ok(())
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }
    fn remaining(&self) -> usize {
        self.bytes.len() - self.at
    }
    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        let end = self
            .at
            .checked_add(count)
            .ok_or("managed catalog offset overflow")?;
        let bytes = self
            .bytes
            .get(self.at..end)
            .ok_or("truncated managed catalog")?;
        self.at = end;
        Ok(bytes)
    }
    fn fixed<const N: usize>(&mut self) -> Result<[u8; N], String> {
        self.take(N)?
            .try_into()
            .map_err(|_| "truncated managed catalog".into())
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.fixed::<1>()?[0])
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.fixed()?))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.fixed()?))
    }
    fn i128(&mut self) -> Result<i128, String> {
        Ok(i128::from_le_bytes(self.fixed()?))
    }
    fn bytes(&mut self, maximum: usize) -> Result<&'a [u8], String> {
        let len = self.u32()? as usize;
        if len > maximum {
            return Err("managed metadata field exceeds byte limit".into());
        }
        self.take(len)
    }
}

fn path_bytes(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect()
    }
}

fn path_from_bytes(bytes: &[u8]) -> Result<PathBuf, String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(std::ffi::OsString::from_vec(bytes.to_vec()).into())
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        if bytes.len() % 2 != 0 {
            return Err("invalid Windows path byte count".into());
        }
        let wide = bytes
            .chunks_exact(2)
            .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
            .collect::<Vec<_>>();
        Ok(std::ffi::OsString::from_wide(&wide).into())
    }
}

#[cfg(test)]
#[path = "generation_catalog.test.rs"]
mod tests;
