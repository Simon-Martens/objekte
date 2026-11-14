//! Checked, source-free metadata for one immutable managed generation.
//!
//! Segment record slots contain only a `metadata_ref`. Untagged refs address
//! the sparse current catalog. High-bit tagged refs address an append-only
//! historical table whose indices remain stable across generations.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::io::Cursor as IoCursor;
use std::ops::Bound;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use bincode::Options;
use roaring::RoaringBitmap;
use sha2::{Digest, Sha256};

use crate::inventory::{
    ByteRange, FileEntry, FileStatus, FileVersion, MAX_DIAGNOSTIC_CODE_BYTES,
    MAX_DIAGNOSTIC_MESSAGE_BYTES, MAX_DIAGNOSTIC_PATH_SEGMENTS, MAX_DIAGNOSTIC_SEGMENT_BYTES,
    MAX_DIAGNOSTICS_PER_FILE, SharedPath, SourceDiagnostic, SourceRevision,
};
use crate::lifecycle::PublishedIndex;
use crate::model::{CollectionId, FileId};
use crate::repository::GlobalInferenceSummary;
use crate::source::DiagnosticCategory;

const MAGIC: &[u8; 8] = b"OBJHCAT1";
const VERSION: u32 = 4;
const HISTORICAL_TAG: u32 = 1 << 31;
const HEADER_BYTES: usize = 8 + 4 + 8 + 32;
const BODY_HEADER_BYTES: usize = 104;
const RETIRED_OFFSET: u32 = u32::MAX;
const QUERY_ARENA_TAG: u32 = 1 << 31;
const MAX_BYTES: usize = 512 * 1024 * 1024;
const MAX_FILES: usize = 16_000_000;
const MAX_PATH_BYTES: usize = 64 * 1024;
const MAX_SOURCE_ID_BYTES: usize = 256;
const MAX_DIAGNOSTIC_BYTES: usize = 16 * 1024;
const MAX_INFERENCE_SUMMARY_BYTES: usize = 32 * 1024 * 1024;

fn offset32(value: usize) -> Result<u32, String> {
    let offset = u32::try_from(value).map_err(|_| "managed catalog offset overflow")?;
    if offset & QUERY_ARENA_TAG != 0 {
        return Err("managed catalog arena exceeds 31-bit offset limit".into());
    }
    Ok(offset)
}

type CatalogSlot = Option<(FileEntry, Option<[u8; 32]>, Option<[u8; 32]>)>;

/// A private encoder that assigns segment metadata refs while collecting the
/// one supplemental copy needed for each distinct historical incarnation.
pub(crate) struct GenerationCatalogBuilder<'a> {
    publication: &'a PublishedIndex,
    configuration_identity: [u8; 32],
    revision: u64,
    next_segment_ordinal: u64,
    collection_count: usize,
    historical_base: Option<Arc<RestoredGenerationCatalog>>,
    historical: Vec<FileEntry>,
    historical_by_file: HashMap<FileId, Vec<u32>>,
}

impl<'a> GenerationCatalogBuilder<'a> {
    #[cfg(test)]
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
            historical_base: None,
            historical: prior.to_vec(),
            historical_by_file,
        };
        builder.validate_header_state()?;
        Ok(builder)
    }

    pub(crate) fn from_publication_with_persisted_history(
        publication: &'a PublishedIndex,
        base: Arc<RestoredGenerationCatalog>,
    ) -> Result<Self, String> {
        let mut builder = Self::from_publication_with_history(publication, &[])?;
        if base.historical_count() > MAX_FILES {
            return Err("managed historical file count exceeds limit".into());
        }
        for index in 0..base.historical_count() {
            let reference = HISTORICAL_TAG | index as u32;
            let file = base
                .file_id_for_ref(reference)
                .ok_or("historical metadata missing")?;
            if file.0 as usize >= publication.catalog.allocated_files() {
                return Err("historical file ID is outside the catalog".into());
            }
            builder
                .historical_by_file
                .entry(file)
                .or_default()
                .push(reference);
        }
        builder.historical_base = Some(base);
        Ok(builder)
    }

    #[cfg(test)]
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
                let index = (reference & !HISTORICAL_TAG) as usize;
                let base_len = self
                    .historical_base
                    .as_ref()
                    .map_or(0, |base| base.historical_count());
                let matches = if index < base_len {
                    self.historical_base
                        .as_ref()
                        .unwrap()
                        .entry_for_ref(reference)?
                        .is_some_and(|known| known == *entry)
                } else {
                    self.historical[index - base_len] == *entry
                };
                if matches {
                    return Ok(reference);
                }
            }
        }
        let base_len = self
            .historical_base
            .as_ref()
            .map_or(0, |base| base.historical_count());
        if self.historical.len() + base_len >= MAX_FILES {
            return Err("managed historical file count exceeds limit".into());
        }
        let reference = HISTORICAL_TAG | (base_len + self.historical.len()) as u32;
        self.historical.push(entry.clone());
        self.historical_by_file
            .entry(entry.id)
            .or_default()
            .push(reference);
        Ok(reference)
    }

    pub(crate) fn encode(self) -> Result<Vec<u8>, String> {
        self.validate_header_state()?;
        let mut metadata = Vec::new();
        let mut slots = Vec::with_capacity(self.publication.catalog.allocated_files());
        let mut paths = Vec::new();
        let mut current_paths = HashSet::new();
        for number in 0..self.publication.catalog.allocated_files() {
            let id = FileId(number as u32);
            let version = self
                .publication
                .file_versions
                .get(id)
                .ok_or("file version missing")?;
            if let Some((encoded, path)) = self.publication.catalog.unchanged_encoded_record(id) {
                if !current_paths.insert(path.as_path().to_path_buf()) {
                    return Err("duplicate current catalog path".into());
                }
                let offset = offset32(metadata.len())?;
                metadata.extend_from_slice(encoded);
                if metadata.len() > MAX_BYTES {
                    return Err("managed catalog exceeds byte limit".into());
                }
                slots.push((version, offset, offset32(metadata.len())? - offset));
                paths.push((id, path, offset));
                continue;
            }
            match self.publication.catalog.entry(id) {
                None => slots.push((version, RETIRED_OFFSET, 0)),
                Some(entry) => {
                    if entry.id.0 as usize != number
                        || !current_paths.insert(entry.relative_path.as_path().to_path_buf())
                    {
                        return Err("invalid or duplicate current catalog entry".into());
                    }
                    let offset = offset32(metadata.len())?;
                    encode_file(&mut metadata, entry, self.collection_count)?;
                    encode_optional_hash(&mut metadata, &self.publication.catalog.fingerprint(id));
                    encode_optional_hash(
                        &mut metadata,
                        &self.publication.catalog.configuration(id),
                    );
                    let len = offset32(metadata.len())? - offset;
                    slots.push((version, offset, len));
                    paths.push((id, entry.relative_path.clone(), offset));
                }
            }
            if metadata.len() > MAX_BYTES {
                return Err("managed catalog exceeds byte limit".into());
            }
        }
        let base_len = self
            .historical_base
            .as_ref()
            .map_or(0, |base| base.historical_count());
        let mut historical_rows = Vec::with_capacity(base_len + self.historical.len());
        if let Some(base) = &self.historical_base {
            for index in 0..base_len {
                let offset = offset32(metadata.len())?;
                metadata.extend_from_slice(
                    base.metadata_for_ref(HISTORICAL_TAG | index as u32)
                        .ok_or("historical metadata missing")?,
                );
                historical_rows.push((offset, offset32(metadata.len())? - offset));
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
            let offset = offset32(metadata.len())?;
            encode_file(&mut metadata, entry, self.collection_count)?;
            historical_rows.push((offset, offset32(metadata.len())? - offset));
            if metadata.len() > MAX_BYTES {
                return Err("managed catalog exceeds byte limit".into());
            }
        }
        let mut native = paths.clone();
        native.sort_unstable_by(|a, b| a.1.as_path().cmp(b.1.as_path()));
        if native
            .windows(2)
            .any(|pair| pair[0].1.as_path() == pair[1].1.as_path())
        {
            return Err("duplicate current catalog path".into());
        }
        let mut query = paths
            .into_iter()
            .map(|(id, path, offset)| (path.query_key(), id, path, offset))
            .collect::<Vec<_>>();
        query.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut query_arena = Vec::new();
        let mut query_rows = Vec::with_capacity(query.len());
        for (key, id, path, offset) in query {
            let (key_offset, key_length) = match path {
                SharedPath::Plain(_) => (
                    offset
                        .checked_add(8)
                        .ok_or("catalog path offset overflow")?,
                    offset32(key.len())?,
                ),
                SharedPath::Native(_) => {
                    let offset = offset32(query_arena.len())?;
                    query_arena.extend_from_slice(key.as_bytes());
                    (offset | QUERY_ARENA_TAG, offset32(key.len())?)
                }
            };
            query_rows.push((id, key_offset, key_length));
        }
        let active = RoaringBitmap::from_iter(native.iter().map(|(id, _, _)| id.0));
        let mut active_bytes = Vec::with_capacity(active.serialized_size());
        active
            .serialize_into(&mut active_bytes)
            .map_err(|error| error.to_string())?;
        let mut inference_bytes = Vec::new();
        for collection in self.publication.repository.collections() {
            let summary = bincode::DefaultOptions::new()
                .with_fixint_encoding()
                .serialize(&collection.global_inference)
                .map_err(|error| error.to_string())?;
            encode_bytes(&mut inference_bytes, &summary, MAX_INFERENCE_SUMMARY_BYTES)?;
            if inference_bytes.len() > MAX_BYTES {
                return Err("managed catalog exceeds byte limit".into());
            }
        }
        let mut body = vec![0; BODY_HEADER_BYTES];
        let slot_start = offset32(body.len())?;
        for (version, offset, len) in &slots {
            body.extend_from_slice(&version.0.to_le_bytes());
            body.extend_from_slice(&offset.to_le_bytes());
            body.extend_from_slice(&len.to_le_bytes());
        }
        let history_start = offset32(body.len())?;
        for (offset, len) in &historical_rows {
            body.extend_from_slice(&offset.to_le_bytes());
            body.extend_from_slice(&len.to_le_bytes());
        }
        let native_start = offset32(body.len())?;
        for (id, _, _) in &native {
            body.extend_from_slice(&id.0.to_le_bytes());
        }
        let query_start = offset32(body.len())?;
        for (id, offset, len) in &query_rows {
            body.extend_from_slice(&id.0.to_le_bytes());
            body.extend_from_slice(&offset.to_le_bytes());
            body.extend_from_slice(&len.to_le_bytes());
        }
        let metadata_start = offset32(body.len())?;
        body.extend_from_slice(&metadata);
        let query_arena_start = offset32(body.len())?;
        body.extend_from_slice(&query_arena);
        let active_start = offset32(body.len())?;
        body.extend_from_slice(&active_bytes);
        let inference_start = offset32(body.len())?;
        body.extend_from_slice(&inference_bytes);
        let body_end = offset32(body.len())?;
        if body.len() + HEADER_BYTES > MAX_BYTES {
            return Err("managed catalog exceeds byte limit".into());
        }
        body[..32].copy_from_slice(&self.configuration_identity);
        body[32..40].copy_from_slice(&self.revision.to_le_bytes());
        body[40..48].copy_from_slice(&self.next_segment_ordinal.to_le_bytes());
        for (at, value) in [
            (48, offset32(self.collection_count)?),
            (52, offset32(slots.len())?),
            (56, offset32(historical_rows.len())?),
            (60, offset32(native.len())?),
            (64, slot_start),
            (68, history_start),
            (72, native_start),
            (76, query_start),
            (80, metadata_start),
            (84, query_arena_start),
            (88, active_start),
            (92, inference_start),
            (96, body_end),
        ] {
            body[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        let mut bytes = vec![0; HEADER_BYTES];
        bytes.extend_from_slice(&body);
        let body_len = body.len() as u64;
        let checksum = Sha256::digest(&body);
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

#[derive(Clone, Copy, Debug)]
struct Regions {
    slots: usize,
    history: usize,
    native: usize,
    query: usize,
    metadata: usize,
    query_arena: usize,
    active: usize,
    inference: usize,
}

#[derive(Clone, Copy)]
struct SlotRow {
    version: FileVersion,
    offset: u32,
    length: u32,
}

/// Checked directory over the committed catalog bytes. It owns the immutable
/// backing but does not reconstruct a file or path table during opening.
#[derive(Debug)]
pub(crate) struct RestoredGenerationCatalog {
    bytes: Arc<[u8]>,
    regions: Regions,
    slot_count: usize,
    historical_count: usize,
    active_count: usize,
    #[cfg(test)]
    pub(crate) configuration_identity: [u8; 32],
    pub(crate) revision: u64,
    pub(crate) next_segment_ordinal: u64,
    pub(crate) inference: Vec<Option<GlobalInferenceSummary>>,
    active: RoaringBitmap,
    // Temporary compatibility fields until all lifecycle callers use the
    // persisted base. Removed before the catalog subtask is complete.
    pub(crate) slots: Vec<CatalogSlot>,
    pub(crate) file_versions: Vec<FileVersion>,
    historical: Vec<FileEntry>,
}

impl RestoredGenerationCatalog {
    /// Convenient borrowed-input form for tests and one-off validation. The
    /// production managed open uses `decode_arc` to retain the original owner.
    pub(crate) fn decode(
        bytes: &[u8],
        expected_identity: [u8; 32],
        collection_count: usize,
    ) -> Result<Self, String> {
        Self::decode_arc(Arc::from(bytes), expected_identity, collection_count)
    }

    pub(crate) fn decode_arc(
        bytes: Arc<[u8]>,
        expected_identity: [u8; 32],
        collection_count: usize,
    ) -> Result<Self, String> {
        Self::decode_owned(bytes, expected_identity, collection_count, false)
    }

    /// ManagedStore has already matched these bytes to the manifest digest.
    pub(crate) fn decode_verified_arc(
        bytes: Arc<[u8]>,
        expected_identity: [u8; 32],
        collection_count: usize,
    ) -> Result<Self, String> {
        Self::decode_owned(bytes, expected_identity, collection_count, true)
    }

    fn decode_owned(
        bytes: Arc<[u8]>,
        expected_identity: [u8; 32],
        collection_count: usize,
        manifest_verified: bool,
    ) -> Result<Self, String> {
        if bytes.len() < HEADER_BYTES + BODY_HEADER_BYTES
            || bytes.len() > MAX_BYTES
            || &bytes[..8] != MAGIC
        {
            return Err("invalid managed catalog envelope".into());
        }
        let mut header = Reader::new(&bytes[8..HEADER_BYTES]);
        if header.u32()? != VERSION {
            return Err("unsupported managed catalog version".into());
        }
        let body_len =
            usize::try_from(header.u64()?).map_err(|_| "catalog body length overflow")?;
        let digest = header.fixed::<32>()?;
        let body = &bytes[HEADER_BYTES..];
        if body_len != body.len()
            || (!manifest_verified && digest != Sha256::digest(body).as_slice())
        {
            return Err("managed catalog length or checksum mismatch".into());
        }
        let mut input = Reader::new(&body[..BODY_HEADER_BYTES]);
        let configuration_identity = input.fixed::<32>()?;
        let revision = input.u64()?;
        let next_segment_ordinal = input.u64()?;
        let stored_collection_count = input.u32()? as usize;
        let slot_count = input.u32()? as usize;
        let historical_count = input.u32()? as usize;
        let active_count = input.u32()? as usize;
        let regions = Regions {
            slots: input.u32()? as usize,
            history: input.u32()? as usize,
            native: input.u32()? as usize,
            query: input.u32()? as usize,
            metadata: input.u32()? as usize,
            query_arena: input.u32()? as usize,
            active: input.u32()? as usize,
            inference: input.u32()? as usize,
        };
        let body_end = input.u32()? as usize;
        let reserved = input.u32()?;
        if configuration_identity != expected_identity
            || revision == 0
            || next_segment_ordinal == 0
            || stored_collection_count != collection_count
            || collection_count == 0
            || slot_count > MAX_FILES
            || historical_count > MAX_FILES
            || active_count > slot_count
            || reserved != 0
            || body_end != body.len()
            || regions.slots != BODY_HEADER_BYTES
            || regions.history
                != regions
                    .slots
                    .checked_add(
                        slot_count
                            .checked_mul(16)
                            .ok_or("catalog slot table overflow")?,
                    )
                    .ok_or("catalog slot table overflow")?
            || regions.native
                != regions
                    .history
                    .checked_add(
                        historical_count
                            .checked_mul(8)
                            .ok_or("catalog history table overflow")?,
                    )
                    .ok_or("catalog history table overflow")?
            || regions.query
                != regions
                    .native
                    .checked_add(
                        active_count
                            .checked_mul(4)
                            .ok_or("catalog native table overflow")?,
                    )
                    .ok_or("catalog native table overflow")?
            || regions.metadata
                != regions
                    .query
                    .checked_add(
                        active_count
                            .checked_mul(12)
                            .ok_or("catalog query table overflow")?,
                    )
                    .ok_or("catalog query table overflow")?
            || regions.query_arena < regions.metadata
            || regions.active < regions.query_arena
            || regions.inference <= regions.active
            || regions.inference > body.len()
        {
            return Err("managed catalog identity or directory invalid".into());
        }
        let active_bytes = &body[regions.active..regions.inference];
        let mut cursor = IoCursor::new(active_bytes);
        let active =
            RoaringBitmap::deserialize_from(&mut cursor).map_err(|error| error.to_string())?;
        if cursor.position() as usize != active_bytes.len()
            || active.len() != active_count as u64
            || active.max().is_some_and(|id| id as usize >= slot_count)
        {
            return Err("managed catalog active bitmap invalid".into());
        }
        let mut inference_input = Reader::new(&body[regions.inference..]);
        let inference = (0..collection_count)
            .map(|_| {
                let bytes = inference_input.bytes(MAX_INFERENCE_SUMMARY_BYTES)?;
                bincode::DefaultOptions::new()
                    .with_fixint_encoding()
                    .reject_trailing_bytes()
                    .with_limit(bytes.len() as u64)
                    .deserialize(bytes)
                    .map_err(|error| format!("invalid managed inference summary: {error}"))
            })
            .collect::<Result<Vec<_>, String>>()?;
        if inference_input.remaining() != 0 {
            return Err("trailing managed catalog bytes".into());
        }
        let mut catalog = Self {
            bytes,
            regions,
            slot_count,
            historical_count,
            active_count,
            #[cfg(test)]
            configuration_identity,
            revision,
            next_segment_ordinal,
            inference,
            active,
            slots: Vec::new(),
            file_versions: Vec::new(),
            historical: Vec::new(),
        };
        catalog.validate_tables(collection_count)?;
        if !manifest_verified {
            // The borrowed compatibility API is retained for standalone tests.
            // Managed open uses encoded tables and decodes only requested rows.
            catalog.file_versions = (0..slot_count)
                .map(|number| catalog.version(FileId(number as u32)).unwrap())
                .collect();
            catalog.slots = (0..slot_count)
                .map(|number| {
                    let file = FileId(number as u32);
                    let Some(entry) = catalog.entry_for_ref(file.0)? else {
                        return Ok(None);
                    };
                    let (fingerprint, configuration) = catalog
                        .current_hashes(file)
                        .ok_or("catalog hashes missing")?;
                    Ok(Some((entry, fingerprint, configuration)))
                })
                .collect::<Result<Vec<_>, String>>()?;
            catalog.historical = catalog.decode_historical_entries()?;
        }
        Ok(catalog)
    }

    fn body(&self) -> &[u8] {
        &self.bytes[HEADER_BYTES..]
    }

    fn slot(&self, id: FileId) -> Option<SlotRow> {
        if id.0 as usize >= self.slot_count {
            return None;
        }
        let at = self.regions.slots + id.0 as usize * 16;
        let bytes = &self.body()[at..at + 16];
        Some(SlotRow {
            version: FileVersion(u64::from_le_bytes(bytes[..8].try_into().unwrap())),
            offset: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
            length: u32::from_le_bytes(bytes[12..16].try_into().unwrap()),
        })
    }

    fn historical_row(&self, index: usize) -> Option<(u32, u32)> {
        if index >= self.historical_count {
            return None;
        }
        let at = self.regions.history + index * 8;
        let bytes = &self.body()[at..at + 8];
        Some((
            u32::from_le_bytes(bytes[..4].try_into().unwrap()),
            u32::from_le_bytes(bytes[4..].try_into().unwrap()),
        ))
    }

    fn metadata_span(&self, offset: u32, len: u32) -> Option<&[u8]> {
        let start = self.regions.metadata.checked_add(offset as usize)?;
        let end = start.checked_add(len as usize)?;
        (end <= self.regions.query_arena).then(|| &self.body()[start..end])
    }

    fn metadata_for_ref(&self, reference: u32) -> Option<&[u8]> {
        let (offset, len) = if reference & HISTORICAL_TAG != 0 {
            self.historical_row((reference & !HISTORICAL_TAG) as usize)?
        } else {
            let slot = self.slot(FileId(reference))?;
            if slot.offset == RETIRED_OFFSET {
                return None;
            }
            (slot.offset, slot.length)
        };
        self.metadata_span(offset, len)
    }

    pub(crate) fn slot_count(&self) -> usize {
        self.slot_count
    }
    pub(crate) fn historical_count(&self) -> usize {
        self.historical_count
    }
    pub(crate) fn active(&self) -> &RoaringBitmap {
        &self.active
    }
    pub(crate) fn version(&self, file: FileId) -> Option<FileVersion> {
        self.slot(file).map(|slot| slot.version)
    }

    pub(crate) fn is_current(&self, file: FileId) -> bool {
        self.active.contains(file.0)
    }

    pub(crate) fn current_encoded_record(&self, file: FileId) -> Option<&[u8]> {
        self.metadata_for_ref(file.0)
    }

    pub(crate) fn current_path(&self, file: FileId) -> Option<SharedPath> {
        self.path_bytes_for_file(file)
            .and_then(|bytes| path_from_borrowed_bytes(bytes).ok())
            .map(|path| path.as_ref().to_path_buf().into())
    }

    pub(crate) fn path_matches(&self, file: FileId, path: &Path) -> bool {
        self.path_bytes_for_file(file)
            .and_then(|bytes| path_from_borrowed_bytes(bytes).ok())
            .is_some_and(|current| current.as_ref() == path)
    }

    pub(crate) fn native_id_for(&self, path: &Path) -> Option<FileId> {
        let mut lower = 0usize;
        let mut upper = self.active_count;
        while lower < upper {
            let middle = lower + (upper - lower) / 2;
            let id = self.native_id(middle)?;
            let native = path_from_borrowed_bytes(self.path_bytes_for_file(id)?).ok()?;
            match native.as_ref().cmp(path) {
                std::cmp::Ordering::Less => lower = middle + 1,
                std::cmp::Ordering::Equal => return Some(id),
                std::cmp::Ordering::Greater => upper = middle,
            }
        }
        None
    }

    pub(crate) fn query_path_ids_in_bounds_up_to(
        &self,
        bounds: &(Bound<Arc<str>>, Bound<Arc<str>>),
        limit: usize,
        mut include: impl FnMut(FileId) -> bool,
    ) -> Option<(Vec<FileId>, usize)> {
        let lower = match &bounds.0 {
            Bound::Unbounded => 0,
            Bound::Included(value) => self.query_bound(value, false),
            Bound::Excluded(value) => self.query_bound(value, true),
        };
        let upper = match &bounds.1 {
            Bound::Unbounded => self.active_count,
            Bound::Included(value) => self.query_bound(value, true),
            Bound::Excluded(value) => self.query_bound(value, false),
        };
        let mut ids = Vec::new();
        for position in lower..upper {
            if position - lower == limit {
                return None;
            }
            let id = self.query_row(position).expect("validated catalog row").0;
            if include(id) {
                ids.push(id);
            }
        }
        Some((ids, upper.saturating_sub(lower)))
    }

    fn query_bound(&self, value: &str, strictly_greater: bool) -> usize {
        let mut lower = 0usize;
        let mut upper = self.active_count;
        while lower < upper {
            let middle = lower + (upper - lower) / 2;
            let (_, offset, len) = self.query_row(middle).expect("validated catalog row");
            let key = self.query_key(offset, len).expect("validated catalog key");
            if key < value || (strictly_greater && key == value) {
                lower = middle + 1;
            } else {
                upper = middle;
            }
        }
        lower
    }

    pub(crate) fn entry_for_ref(&self, reference: u32) -> Result<Option<FileEntry>, String> {
        let Some(bytes) = self.metadata_for_ref(reference) else {
            return Ok(None);
        };
        let mut input = Reader::new(bytes);
        let entry = decode_file(&mut input, self.inference.len())?;
        if reference & HISTORICAL_TAG == 0 {
            decode_optional_hash(&mut input)?;
            decode_optional_hash(&mut input)?;
        }
        if input.remaining() != 0 {
            return Err("catalog metadata record trailing bytes".into());
        }
        Ok(Some(entry))
    }

    pub(crate) fn file_id_for_ref(&self, reference: u32) -> Option<FileId> {
        let bytes = self.metadata_for_ref(reference)?;
        Some(FileId(u32::from_le_bytes(bytes[..4].try_into().ok()?)))
    }

    pub(crate) fn revision_for_ref(&self, reference: u32) -> Option<SourceRevision> {
        let bytes = self.metadata_for_ref(reference)?;
        let path_len = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?) as usize;
        let at = 8usize.checked_add(path_len)?;
        Some(SourceRevision {
            modified_ns: i128::from_le_bytes(bytes.get(at..at + 16)?.try_into().ok()?),
            size: u64::from_le_bytes(bytes.get(at + 16..at + 24)?.try_into().ok()?),
        })
    }

    pub(crate) fn contains_collection_for_ref(
        &self,
        reference: u32,
        collection: CollectionId,
    ) -> Option<bool> {
        let bytes = self.metadata_for_ref(reference)?;
        let mut input = Reader::new(bytes);
        input.u32().ok()?;
        input.bytes(MAX_PATH_BYTES).ok()?;
        input.i128().ok()?;
        input.u64().ok()?;
        decode_optional_i128(&mut input).ok()?;
        let count = input.u32().ok()? as usize;
        let memberships = input.take(count.checked_mul(4)?).ok()?;
        Some(
            memberships
                .chunks_exact(4)
                .any(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()) == collection.0),
        )
    }

    pub(crate) fn current_hashes(
        &self,
        file: FileId,
    ) -> Option<(Option<[u8; 32]>, Option<[u8; 32]>)> {
        let bytes = self.metadata_for_ref(file.0)?;
        let mut input = Reader::new(bytes);
        parse_file_view(&mut input, self.inference.len()).ok()?;
        Some((
            decode_optional_hash(&mut input).ok()?,
            decode_optional_hash(&mut input).ok()?,
        ))
    }

    pub(crate) fn decode_historical_entries(&self) -> Result<Vec<FileEntry>, String> {
        (0..self.historical_count)
            .map(|index| {
                self.entry_for_ref(HISTORICAL_TAG | index as u32)?
                    .ok_or("historical metadata missing".into())
            })
            .collect()
    }

    pub(crate) fn historical_entries(&self) -> &[FileEntry] {
        &self.historical
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

    fn path_bytes_for_file(&self, id: FileId) -> Option<&[u8]> {
        let bytes = self.metadata_for_ref(id.0)?;
        let len = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?) as usize;
        bytes.get(8..8usize.checked_add(len)?)
    }

    fn native_id(&self, ordinal: usize) -> Option<FileId> {
        if ordinal >= self.active_count {
            return None;
        }
        let at = self.regions.native + ordinal * 4;
        Some(FileId(u32::from_le_bytes(
            self.body()[at..at + 4].try_into().unwrap(),
        )))
    }

    fn query_row(&self, ordinal: usize) -> Option<(FileId, u32, u32)> {
        if ordinal >= self.active_count {
            return None;
        }
        let at = self.regions.query + ordinal * 12;
        let bytes = &self.body()[at..at + 12];
        Some((
            FileId(u32::from_le_bytes(bytes[..4].try_into().unwrap())),
            u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
        ))
    }

    fn query_key(&self, offset: u32, len: u32) -> Option<&str> {
        let arena = offset & QUERY_ARENA_TAG != 0;
        let start = (offset & !QUERY_ARENA_TAG) as usize;
        let (base, limit) = if arena {
            (self.regions.query_arena, self.regions.active)
        } else {
            (self.regions.metadata, self.regions.query_arena)
        };
        let start = base.checked_add(start)?;
        let end = start.checked_add(len as usize)?;
        if end > limit {
            return None;
        }
        std::str::from_utf8(&self.body()[start..end]).ok()
    }

    fn validate_tables(&self, collection_count: usize) -> Result<(), String> {
        let mut expected_metadata_offset = 0usize;
        for number in 0..self.slot_count {
            let id = FileId(number as u32);
            let row = self.slot(id).ok_or("catalog slot missing")?;
            if row.version.0 == 0 || row.version.0 > self.revision {
                return Err("invalid managed file version".into());
            }
            if row.offset == RETIRED_OFFSET {
                if row.length != 0 || self.active.contains(id.0) {
                    return Err("retired catalog slot and active bitmap disagree".into());
                }
                continue;
            }
            if row.offset as usize != expected_metadata_offset
                || row.length == 0
                || !self.active.contains(id.0)
            {
                return Err("catalog slot offset or active state invalid".into());
            }
            let bytes = self
                .metadata_span(row.offset, row.length)
                .ok_or("catalog slot metadata outside arena")?;
            let mut input = Reader::new(bytes);
            let view = parse_file_view(&mut input, collection_count)?;
            if view.id != id {
                return Err("catalog slot file ID mismatch".into());
            }
            decode_optional_hash(&mut input)?;
            decode_optional_hash(&mut input)?;
            if input.remaining() != 0 {
                return Err("catalog current metadata trailing bytes".into());
            }
            expected_metadata_offset += row.length as usize;
        }
        let mut historical_by_file = HashMap::<FileId, Vec<&[u8]>>::new();
        for index in 0..self.historical_count {
            let (offset, len) = self
                .historical_row(index)
                .ok_or("catalog historical row missing")?;
            if len == 0 || offset as usize != expected_metadata_offset {
                return Err("catalog historical offset invalid".into());
            }
            let bytes = self
                .metadata_span(offset, len)
                .ok_or("catalog historical metadata outside arena")?;
            let mut input = Reader::new(bytes);
            let view = parse_file_view(&mut input, collection_count)?;
            if view.id.0 as usize >= self.slot_count || input.remaining() != 0 {
                return Err("catalog historical metadata invalid".into());
            }
            let known = historical_by_file.entry(view.id).or_default();
            if known.contains(&bytes) {
                return Err("duplicate historical metadata".into());
            }
            known.push(bytes);
            expected_metadata_offset += len as usize;
        }
        if expected_metadata_offset != self.regions.query_arena - self.regions.metadata {
            return Err("catalog metadata arena has unreferenced bytes".into());
        }
        let mut native_seen = RoaringBitmap::new();
        let mut previous_native = None;
        for ordinal in 0..self.active_count {
            let id = self
                .native_id(ordinal)
                .ok_or("catalog native path row missing")?;
            if !self.active.contains(id.0) || !native_seen.insert(id.0) {
                return Err("catalog native path file coverage invalid".into());
            }
            let path = path_from_borrowed_bytes(
                self.path_bytes_for_file(id)
                    .ok_or("catalog native path missing")?,
            )?;
            if let Some(previous) = previous_native {
                let prior = path_from_borrowed_bytes(
                    self.path_bytes_for_file(previous)
                        .ok_or("catalog native path missing")?,
                )?;
                if prior.as_ref() >= path.as_ref() {
                    return Err("catalog native path order or uniqueness invalid".into());
                }
            }
            previous_native = Some(id);
        }
        if native_seen != self.active {
            return Err("catalog native path index coverage invalid".into());
        }

        let mut query_seen = RoaringBitmap::new();
        let mut previous_query = None;
        let mut expected_query_offset = 0usize;
        for ordinal in 0..self.active_count {
            let (id, offset, len) = self
                .query_row(ordinal)
                .ok_or("catalog query path row missing")?;
            if !self.active.contains(id.0) || !query_seen.insert(id.0) {
                return Err("catalog query path file coverage invalid".into());
            }
            let key = self
                .query_key(offset, len)
                .ok_or("catalog query path key invalid")?;
            let path_bytes = self
                .path_bytes_for_file(id)
                .ok_or("catalog query path missing")?;
            let path = path_from_borrowed_bytes(path_bytes)?;
            let expected = query_spelling(path.as_ref(), path_bytes);
            if key != expected.as_ref() {
                return Err("catalog query path key disagrees with native path".into());
            }
            if offset & QUERY_ARENA_TAG != 0 {
                if (offset & !QUERY_ARENA_TAG) as usize != expected_query_offset {
                    return Err("catalog query key arena gap or overlap".into());
                }
                expected_query_offset += len as usize;
            } else {
                let row = self.slot(id).ok_or("catalog query slot missing")?;
                if offset
                    != row
                        .offset
                        .checked_add(8)
                        .ok_or("catalog query path offset overflow")?
                    || len as usize != path_bytes.len()
                {
                    return Err("catalog shared query path reference invalid".into());
                }
            }
            if let Some((previous_key, previous_id)) = previous_query {
                if (previous_key, previous_id) >= (key, id) {
                    return Err("catalog query path order invalid".into());
                }
            }
            previous_query = Some((key, id));
        }
        if query_seen != self.active
            || expected_query_offset != self.regions.active - self.regions.query_arena
        {
            return Err("catalog query path index coverage or arena invalid".into());
        }
        Ok(())
    }
}

struct FileView {
    id: FileId,
}

fn path_from_borrowed_bytes(bytes: &[u8]) -> Result<Cow<'_, Path>, String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(Cow::Borrowed(Path::new(std::ffi::OsStr::from_bytes(bytes))))
    }
    #[cfg(windows)]
    {
        Ok(Cow::Owned(path_from_bytes(bytes)?))
    }
}

fn query_spelling<'a>(path: &Path, bytes: &'a [u8]) -> Cow<'a, str> {
    #[cfg(unix)]
    if let Ok(text) = std::str::from_utf8(bytes)
        && !bytes.contains(&b'%')
        && !bytes.contains(&b'\\')
    {
        return Cow::Borrowed(text);
    }
    Cow::Owned(crate::path_repr::query_path(path))
}

/// Validate one metadata leaf while retaining only references to its fixed
/// fields. Diagnostics are bounded JSON leaves decoded and dropped here.
fn parse_file_view(input: &mut Reader<'_>, collection_count: usize) -> Result<FileView, String> {
    let id = FileId(input.u32()?);
    let path_bytes = input.bytes(MAX_PATH_BYTES)?;
    let path = path_from_borrowed_bytes(path_bytes)?;
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || !path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err("invalid managed file path".into());
    }
    let revision = SourceRevision {
        modified_ns: input.i128()?,
        size: input.u64()?,
    };
    decode_optional_i128(input)?;
    let count = input.u32()? as usize;
    if count == 0 || count > collection_count || count > input.remaining() / 4 {
        return Err("invalid managed collection memberships".into());
    }
    let collections = input.take(count * 4)?;
    for (number, bytes) in collections.chunks_exact(4).enumerate() {
        let member = u32::from_le_bytes(bytes.try_into().unwrap());
        if member as usize >= collection_count
            || collections[..number * 4]
                .chunks_exact(4)
                .any(|prior| u32::from_le_bytes(prior.try_into().unwrap()) == member)
        {
            return Err("duplicate or invalid managed collection membership".into());
        }
    }
    let status = match input.u8()? {
        0 => FileStatus::Valid,
        1 => FileStatus::Broken,
        2 => FileStatus::Unreadable,
        _ => return Err("invalid managed file status".into()),
    };
    let omitted = input.u32()?;
    let diagnostic_count = input.u32()? as usize;
    if diagnostic_count > MAX_DIAGNOSTICS_PER_FILE || diagnostic_count > input.remaining() / 4 {
        return Err("invalid managed diagnostic count".into());
    }
    let mut derived_status = FileStatus::Valid;
    for _ in 0..diagnostic_count {
        let encoded = input.bytes(MAX_DIAGNOSTIC_BYTES)?;
        let diagnostic: SourceDiagnostic = serde_json::from_slice(encoded)
            .map_err(|error| format!("invalid managed source diagnostic: {error}"))?;
        if serde_json::to_vec(&diagnostic).map_err(|error| error.to_string())? != encoded {
            return Err("noncanonical managed diagnostic JSON".into());
        }
        derived_status = derived_status.max(match diagnostic.category {
            DiagnosticCategory::Io => FileStatus::Unreadable,
            _ => FileStatus::Broken,
        });
        let belongs = collections
            .chunks_exact(4)
            .any(|part| u32::from_le_bytes(part.try_into().unwrap()) == diagnostic.collection.0);
        if diagnostic.file != id
            || !belongs
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
                ByteRange::checked(range.start, range.end, revision.size).is_err()
            })
            || diagnostic
                .byte_offset
                .is_some_and(|offset| offset > revision.size)
        {
            return Err("invalid managed source diagnostic".into());
        }
    }
    if (omitted == 0 && status != derived_status)
        || (omitted != 0
            && (status == FileStatus::Valid
                || derived_status == FileStatus::Valid
                || status < derived_status))
    {
        return Err("managed file status and diagnostics disagree".into());
    }
    Ok(FileView { id })
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
