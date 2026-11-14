//! Checked immutable segment envelope and portable Roaring payload access.
//!
//! Section framing is independent of byte ownership. Store readers may map
//! only immutable application-owned files; memory-only builders use owned bytes.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::fs::File;
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

use memmap2::{Mmap, MmapOptions};
use roaring::RoaringBitmap;
use sha2::{Digest, Sha256};

const MAGIC: &[u8; 8] = b"OBJHSEG1";
pub(crate) const FORMAT_VERSION: u32 = 4;
const HEADER_BYTES: usize = 24;
const DIRECTORY_ENTRY_BYTES: usize = 56;
const MAX_SECTIONS: usize = 32;
const MAX_SEGMENT_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_DECODED_POSTING_CACHE_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MIN_CACHED_POSTING_BYTES: u32 = 64;


#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(u32)]
pub(crate) enum SectionKind {
    FieldMetadata = 1,
    TextBindings = 2,
    ScalarDictionaries = 3,
    StructuralReferences = 4,
    BigramDictionaries = 5,
    PostingPayloads = 6,
    RecordSlots = 7,
    SortData = 8,
    RelationAndIdentity = 9,
    Integrity = 10,
    GramFilters = 11,
}


pub(crate) const REQUIRED_SECTIONS: [SectionKind; 11] = [
    SectionKind::FieldMetadata,
    SectionKind::TextBindings,
    SectionKind::ScalarDictionaries,
    SectionKind::StructuralReferences,
    SectionKind::BigramDictionaries,
    SectionKind::PostingPayloads,
    SectionKind::RecordSlots,
    SectionKind::SortData,
    SectionKind::RelationAndIdentity,
    SectionKind::Integrity,
    SectionKind::GramFilters,
];


impl TryFrom<u32> for SectionKind {
    type Error = String;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Ok(match value {
            1 => Self::FieldMetadata,
            2 => Self::TextBindings,
            3 => Self::ScalarDictionaries,
            4 => Self::StructuralReferences,
            5 => Self::BigramDictionaries,
            6 => Self::PostingPayloads,
            7 => Self::RecordSlots,
            8 => Self::SortData,
            9 => Self::RelationAndIdentity,
            10 => Self::Integrity,
            11 => Self::GramFilters,
            _ => return Err(format!("unknown immutable segment section {value}")),
        })
    }
}


#[derive(Clone, Debug)]
struct SectionEntry {
    kind: SectionKind,
    offset: u64,
    length: u64,
    sha256: [u8; 32],
    checked: Arc<OnceLock<Result<(), String>>>,
}


/// Exactly one owner for the bytes used by all section and posting readers.
#[derive(Clone)]
enum SegmentBytes {
    #[cfg(test)]
    Owned(Arc<[u8]>),
    OwnedVec(Arc<Vec<u8>>),
    Mapped(Arc<Mmap>),
    Constructed(Arc<ConstructedSections>),
}


struct ConstructedSections {
    buffers: [Vec<u8>; REQUIRED_SECTIONS.len()],
    encoded_len: usize,
}


impl SegmentBytes {
    fn contiguous(&self) -> Option<&[u8]> {
        match self {
            #[cfg(test)]
            Self::Owned(bytes) => Some(bytes),
            Self::OwnedVec(bytes) => Some(bytes),
            Self::Mapped(map) => Some(map),
            Self::Constructed(_) => None,
        }
    }
}


/// A segment reader validates framing at construction and section checksums
/// once per immutable byte span. Posting payloads have narrower checksums.
#[derive(Clone)]
pub(crate) struct SegmentReader {
    bytes: SegmentBytes,
    sections: HashMap<SectionKind, SectionEntry>,
    decoded: Arc<RwLock<DecodedPostingCache>>,
    #[cfg(test)]
    checked_postings: Arc<std::sync::atomic::AtomicU64>,
}


#[derive(Debug)]
pub(crate) enum SegmentOpenError {
    Io(io::Error),
    Invalid(String),
    Resource(String),
}


impl fmt::Display for SegmentOpenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Invalid(message) => formatter.write_str(message),
            Self::Resource(message) => formatter.write_str(message),
        }
    }
}


#[derive(Default)]
struct DecodedPostingCache {
    entries: HashMap<PostingCacheKey, CachedPosting>,
    clock: VecDeque<PostingCacheKey>,
    retained_bytes: usize,
    limit_bytes: usize,
}


#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum PostingProof {
    Constructed,
    Checked,
}


/// The source of the proof for eager dictionary validation. A stamp never
/// proves posting payloads; those still require checked access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SegmentTrust {
    Constructed,
    Stamped,
    Untrusted,
}


/// Bump when dictionary ordering, metadata semantics, collation or the
/// validator's interpretation changes. Old stamps then require a full open.
pub(crate) const VALIDATOR_VERSION: u32 = 3;


/// The retained domain owner makes the pointer identity stable for the
/// lifetime of a cache entry. A descriptor checked for one eligible domain
/// cannot prove that same payload for another dictionary's domain. Writer
/// proof is also separate from externally checked proof.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct PostingCacheKey {
    descriptor: PostingDescriptor,
    domain: usize,
    proof: PostingProof,
}


impl PostingCacheKey {
    fn new(
        descriptor: PostingDescriptor,
        allowed: &Arc<RoaringBitmap>,
        proof: PostingProof,
    ) -> Self {
        Self {
            descriptor,
            domain: Arc::as_ptr(allowed) as usize,
            proof,
        }
    }
}


struct CachedPosting {
    bitmap: Arc<RoaringBitmap>,
    _allowed: Arc<RoaringBitmap>,
    weight: usize,
    recently_used: AtomicBool,
}


impl SegmentReader {
    pub(crate) fn is_constructed(&self) -> bool {
        matches!(self.bytes, SegmentBytes::Constructed(_))
    }

    pub(crate) fn encoded_len(&self) -> usize {
        match &self.bytes {
            SegmentBytes::Constructed(sections) => sections.encoded_len,
            other => other.contiguous().unwrap().len(),
        }
    }

    #[cfg(test)]
    pub(crate) fn from_owned(bytes: Arc<[u8]>) -> Result<Self, String> {
        Self::parse(SegmentBytes::Owned(bytes))
    }

    /// Transfer a just-built segment buffer without copying its full payload
    /// into an Arc slice while the mutable builders are still live.
    pub(crate) fn from_owned_vec(bytes: Vec<u8>) -> Result<Self, String> {
        Self::parse(SegmentBytes::OwnedVec(Arc::new(bytes)))
    }

    /// Only the completed in-process segment encoder may supply these owned
    /// sections. External files must pass through `parse` and its checks.
    pub(crate) fn from_constructed_sections(
        sections: [(SectionKind, Vec<u8>); REQUIRED_SECTIONS.len()],
    ) -> Result<Self, String> {
        let mut encoded_len = HEADER_BYTES + REQUIRED_SECTIONS.len() * DIRECTORY_ENTRY_BYTES;
        let mut entries = HashMap::with_capacity(REQUIRED_SECTIONS.len());
        for (expected, (kind, bytes)) in REQUIRED_SECTIONS.iter().zip(&sections) {
            if kind != expected {
                return Err("constructed segment section order changed".into());
            }
            let offset = encoded_len as u64;
            encoded_len = encoded_len
                .checked_add(bytes.len())
                .ok_or("constructed segment length overflow")?;
            if encoded_len as u64 > MAX_SEGMENT_BYTES {
                return Err("constructed segment exceeds format limit".into());
            }
            let checked = OnceLock::new();
            checked.set(Ok(())).unwrap();
            entries.insert(
                *kind,
                SectionEntry {
                    kind: *kind,
                    offset,
                    length: bytes.len() as u64,
                    sha256: [0; 32],
                    checked: Arc::new(checked),
                },
            );
        }
        let buffers = sections.map(|(_, bytes)| bytes);
        let limit_bytes = (encoded_len / 20).min(MAX_DECODED_POSTING_CACHE_BYTES);
        Ok(Self {
            bytes: SegmentBytes::Constructed(Arc::new(ConstructedSections {
                buffers,
                encoded_len,
            })),
            sections: entries,
            decoded: Arc::new(RwLock::new(DecodedPostingCache {
                limit_bytes,
                ..Default::default()
            })),
            #[cfg(test)]
            checked_postings: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        })
    }

    #[cfg(test)]
    pub(crate) fn from_index_file(file: &File) -> Result<Self, String> {
        Self::from_index_file_typed(file).map_err(|error| error.to_string())
    }

    /// Open an immutable Objekte segment protected against replacement by the
    /// store's reader/writer protocol. Source files must never use this path.
    pub(crate) fn from_index_file_typed(file: &File) -> Result<Self, SegmentOpenError> {
        let metadata = file.metadata().map_err(SegmentOpenError::Io)?;
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_SEGMENT_BYTES {
            return Err(SegmentOpenError::Invalid(
                "index segment is not a supported regular file".into(),
            ));
        }
        // SAFETY: the store opens application-owned immutable segment files;
        // publication never truncates or modifies an opened generation.
        let map = unsafe { MmapOptions::new().map(file) }.map_err(SegmentOpenError::Io)?;
        Self::parse(SegmentBytes::Mapped(Arc::new(map))).map_err(SegmentOpenError::Invalid)
    }

    /// Fully resident storage owns all file bytes. A short read or allocation
    /// failure is an explicit error; no mapping remains in the returned reader.
    pub(crate) fn from_index_file_owned_typed(mut file: &File) -> Result<Self, SegmentOpenError> {
        let metadata = file.metadata().map_err(SegmentOpenError::Io)?;
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_SEGMENT_BYTES {
            return Err(SegmentOpenError::Invalid(
                "index segment is not a supported regular file".into(),
            ));
        }
        let length = usize::try_from(metadata.len()).map_err(|_| {
            SegmentOpenError::Invalid("segment length exceeds address space".into())
        })?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(length).map_err(|error| {
            SegmentOpenError::Resource(format!("full index residency allocation: {error}"))
        })?;
        bytes.resize(length, 0);
        file.read_exact(&mut bytes).map_err(SegmentOpenError::Io)?;
        Self::from_owned_vec(bytes).map_err(SegmentOpenError::Invalid)
    }

    #[cfg(test)]
    pub(crate) fn is_owned(&self) -> bool {
        matches!(
            self.bytes,
            SegmentBytes::Owned(_) | SegmentBytes::OwnedVec(_) | SegmentBytes::Constructed(_)
        )
    }

    fn parse(bytes: SegmentBytes) -> Result<Self, String> {
        let raw = bytes
            .contiguous()
            .ok_or("constructed bytes require internal opener")?;
        if raw.len() < HEADER_BYTES || raw.len() as u64 > MAX_SEGMENT_BYTES {
            return Err("immutable segment header is truncated or oversized".into());
        }
        if &raw[..8] != MAGIC {
            return Err("immutable segment magic mismatch".into());
        }
        if u32_at(raw, 8)? != FORMAT_VERSION {
            return Err("unsupported immutable segment format".into());
        }
        let count = u32_at(raw, 12)? as usize;
        if count == 0 || count > MAX_SECTIONS {
            return Err("invalid immutable segment section count".into());
        }
        let directory_end = u64_at(raw, 16)?;
        let expected_end = HEADER_BYTES
            .checked_add(
                count
                    .checked_mul(DIRECTORY_ENTRY_BYTES)
                    .ok_or("directory overflow")?,
            )
            .ok_or("directory overflow")?;
        if directory_end != expected_end as u64 || expected_end > raw.len() {
            return Err("immutable segment directory bounds mismatch".into());
        }
        let mut sections = HashMap::with_capacity(count);
        let mut spans = Vec::with_capacity(count);
        let mut previous_kind = 0u32;
        for index in 0..count {
            let position = HEADER_BYTES + index * DIRECTORY_ENTRY_BYTES;
            let kind = SectionKind::try_from(u32_at(raw, position)?)?;
            if kind as u32 <= previous_kind {
                return Err("immutable segment directory order mismatch".into());
            }
            previous_kind = kind as u32;
            if u32_at(raw, position + 4)? != 0 {
                return Err("unsupported immutable segment section flags".into());
            }
            let offset = u64_at(raw, position + 8)?;
            let length = u64_at(raw, position + 16)?;
            let end = offset
                .checked_add(length)
                .ok_or("immutable segment section offset overflow")?;
            if offset < directory_end || end > raw.len() as u64 {
                return Err("immutable segment section out of bounds".into());
            }
            let mut sha256 = [0u8; 32];
            sha256.copy_from_slice(&raw[position + 24..position + 56]);
            if sections
                .insert(
                    kind,
                    SectionEntry {
                        kind,
                        offset,
                        length,
                        sha256,
                        checked: Arc::new(OnceLock::new()),
                    },
                )
                .is_some()
            {
                return Err("duplicate immutable segment section".into());
            }
            if length > 0 {
                spans.push((offset, end));
            }
        }
        spans.sort_unstable();
        if spans.windows(2).any(|pair| pair[0].1 > pair[1].0) {
            return Err("overlapping immutable segment sections".into());
        }
        // Cap retained decoded postings relative to this segment's encoded
        // bytes so many small shards do not imply an unbounded global cache.
        let limit_bytes = (raw.len() / 20).min(MAX_DECODED_POSTING_CACHE_BYTES);
        Ok(Self {
            bytes,
            sections,
            decoded: Arc::new(RwLock::new(DecodedPostingCache {
                limit_bytes,
                ..Default::default()
            })),
            #[cfg(test)]
            checked_postings: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        })
    }

    #[cfg(test)]
    pub(crate) fn checked_posting_calls(&self) -> u64 {
        self.checked_postings
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// A bounded hot-posting cache is shared by every dictionary in this
    /// segment. Entries are used only after the caller has validated the
    /// descriptor against its own eligible record domain.
    pub(crate) fn cached_posting(
        &self,
        descriptor: PostingDescriptor,
        allowed: &Arc<RoaringBitmap>,
        proof: PostingProof,
    ) -> Option<Arc<RoaringBitmap>> {
        let key = PostingCacheKey::new(descriptor, allowed, proof);
        let cache = self.decoded.read().ok()?;
        let entry = cache.entries.get(&key)?;
        entry.recently_used.store(true, Ordering::Relaxed);
        Some(Arc::clone(&entry.bitmap))
    }

    pub(crate) fn decoded_cache_usage(&self) -> (usize, usize, usize) {
        self.decoded
            .read()
            .map(|cache| (cache.entries.len(), cache.retained_bytes, cache.limit_bytes))
            .unwrap_or((0, 0, 0))
    }

    #[cfg(test)]
    pub(crate) fn set_decoded_cache_limit_for_test(&self, limit_bytes: usize) {
        self.decoded.write().unwrap().limit_bytes = limit_bytes;
    }

    pub(crate) fn retain_posting_owned(
        &self,
        descriptor: PostingDescriptor,
        allowed: &Arc<RoaringBitmap>,
        proof: PostingProof,
        bitmap: RoaringBitmap,
    ) -> Arc<RoaringBitmap> {
        let key = PostingCacheKey::new(descriptor, allowed, proof);
        let weight = (bitmap.serialized_size() as usize)
            .saturating_mul(2)
            .saturating_add(128);
        let bitmap = Arc::new(bitmap);
        if descriptor.length < MIN_CACHED_POSTING_BYTES {
            return bitmap;
        }
        // Roaring's in-memory containers and map entry cost are larger than
        // the portable payload, so charge a conservative weight to the cap.
        let Ok(mut cache) = self.decoded.write() else {
            return bitmap;
        };
        if weight > cache.limit_bytes {
            return bitmap;
        }
        if let Some(existing) = cache.entries.get(&key) {
            existing.recently_used.store(true, Ordering::Relaxed);
            return Arc::clone(&existing.bitmap);
        }
        while cache.retained_bytes.saturating_add(weight) > cache.limit_bytes {
            let Some(victim) = cache.clock.pop_front() else {
                return bitmap;
            };
            if cache
                .entries
                .get(&victim)
                .is_some_and(|entry| entry.recently_used.swap(false, Ordering::Relaxed))
            {
                cache.clock.push_back(victim);
                continue;
            }
            if let Some(removed) = cache.entries.remove(&victim) {
                cache.retained_bytes -= removed.weight;
            }
        }
        cache.clock.push_back(key);
        cache.entries.insert(
            key,
            CachedPosting {
                bitmap: Arc::clone(&bitmap),
                _allowed: Arc::clone(allowed),
                weight,
                recently_used: AtomicBool::new(false),
            },
        );
        cache.retained_bytes += weight;
        bitmap
    }

    pub(crate) fn require_complete_sections(&self) -> Result<(), String> {
        if self.sections.len() != REQUIRED_SECTIONS.len()
            || REQUIRED_SECTIONS
                .iter()
                .any(|kind| !self.sections.contains_key(kind))
        {
            return Err("immutable segment required section missing".into());
        }
        Ok(())
    }

    pub(crate) fn section(&self, kind: SectionKind) -> Result<Option<&[u8]>, String> {
        let Some(entry) = self.sections.get(&kind) else {
            return Ok(None);
        };
        let result = entry.checked.get_or_init(|| {
            let bytes = self.section_bytes(entry);
            let digest: [u8; 32] = Sha256::digest(bytes).into();
            if digest == entry.sha256 {
                Ok(())
            } else {
                Err("immutable segment section checksum mismatch".into())
            }
        });
        result.as_ref().map_err(Clone::clone)?;
        Ok(Some(self.section_bytes(entry)))
    }

    fn section_bytes(&self, entry: &SectionEntry) -> &[u8] {
        match &self.bytes {
            SegmentBytes::Constructed(sections) => &sections.buffers[entry.kind as usize - 1],
            other => {
                let start = entry.offset as usize;
                &other.contiguous().unwrap()[start..start + entry.length as usize]
            }
        }
    }

    /// Check descriptor framing before it can guide a dictionary lookup.
    pub(crate) fn validate_posting_descriptor(
        &self,
        descriptor: PostingDescriptor,
    ) -> Result<(), String> {
        if descriptor.length == 0 {
            if descriptor != PostingDescriptor::EMPTY {
                return Err("invalid empty posting descriptor".into());
            }
            return Ok(());
        }
        if descriptor.cardinality == 0 {
            return Err("nonempty posting advertises zero cardinality".into());
        }
        let region = self
            .sections
            .get(&SectionKind::PostingPayloads)
            .ok_or("posting section missing")?;
        let end = descriptor
            .offset
            .checked_add(descriptor.length as u64)
            .ok_or("posting descriptor offset overflow")?;
        if end > region.length {
            return Err("posting descriptor out of bounds".into());
        }
        Ok(())
    }

    /// Validate one portable bitmap without hashing the complete posting region.
    pub(crate) fn materialize_posting(
        &self,
        descriptor: PostingDescriptor,
        allowed: &RoaringBitmap,
    ) -> Result<RoaringBitmap, String> {
        #[cfg(test)]
        self.checked_postings
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.validate_posting_descriptor(descriptor)?;
        if descriptor.length == 0 {
            return Ok(RoaringBitmap::new());
        }
        let payload = self.posting_payload(descriptor)?;
        if short_checksum(payload) != descriptor.checksum {
            return Err("posting checksum mismatch".into());
        }
        let mut cursor = Cursor::new(payload);
        let bitmap = RoaringBitmap::deserialize_from(&mut cursor)
            .map_err(|error| format!("invalid portable posting: {error}"))?;
        if cursor.position() != descriptor.length as u64
            || bitmap.len() != descriptor.cardinality as u64
            || !bitmap.is_subset(allowed)
        {
            return Err("posting cardinality or record domain mismatch".into());
        }
        Ok(bitmap)
    }

    /// Use only after checked validation of this exact descriptor, eligible
    /// domain and immutable segment bytes has been published by its owner.
    pub(crate) fn materialize_validated_posting(
        &self,
        descriptor: PostingDescriptor,
    ) -> Result<RoaringBitmap, String> {
        if descriptor == PostingDescriptor::EMPTY {
            return Ok(RoaringBitmap::new());
        }
        let payload = self.posting_payload(descriptor)?;
        RoaringBitmap::deserialize_unchecked_from(payload)
            .map_err(|error| format!("validated posting decode: {error}"))
    }

    /// Restricted intersection avoids a full posting materialization once
    /// the immutable payload has been checked for this dictionary's domain.
    pub(crate) fn intersect_validated_posting(
        &self,
        descriptor: PostingDescriptor,
        input: &RoaringBitmap,
    ) -> Result<RoaringBitmap, String> {
        if descriptor == PostingDescriptor::EMPTY {
            return Ok(RoaringBitmap::new());
        }
        let payload = self.posting_payload(descriptor)?;
        input
            .intersection_with_serialized_unchecked(Cursor::new(payload))
            .map_err(|error| format!("validated posting intersection: {error}"))
    }

    fn posting_payload(&self, descriptor: PostingDescriptor) -> Result<&[u8], String> {
        self.validate_posting_descriptor(descriptor)?;
        let region = self
            .sections
            .get(&SectionKind::PostingPayloads)
            .ok_or("posting section missing")?;
        let start =
            usize::try_from(descriptor.offset).map_err(|_| "posting payload offset overflow")?;
        let end = start
            .checked_add(descriptor.length as usize)
            .ok_or("posting payload length overflow")?;
        self.section_bytes(region)
            .get(start..end)
            .ok_or("posting payload out of bounds".into())
    }
}


/// `u64` posting-region offset, `u32` payload length and cardinality, and an
/// 8-byte payload checksum. No page is reserved for an individual posting.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct PostingDescriptor {
    pub offset: u64,
    pub length: u32,
    pub cardinality: u32,
    pub checksum: u64,
    /// Writer-assigned ordinal implicit in the checked leaf offset table.
    /// Not serialized inside the 24-byte descriptor; dictionary lookup adds it.
    pub ordinal: u32,
}


impl PostingDescriptor {
    pub(crate) const EMPTY: Self = Self {
        offset: 0,
        length: 0,
        cardinality: 0,
        checksum: 0,
        ordinal: u32::MAX,
    };

    pub(crate) fn for_bitmap(
        offset: u64,
        bitmap: &RoaringBitmap,
        payload: &[u8],
    ) -> Result<Self, String> {
        if bitmap.is_empty() {
            return Ok(Self::EMPTY);
        }
        Ok(Self {
            offset,
            length: u32::try_from(payload.len()).map_err(|_| "posting payload too large")?,
            cardinality: u32::try_from(bitmap.len()).map_err(|_| "posting too many slots")?,
            checksum: short_checksum(payload),
            ordinal: u32::MAX,
        })
    }

    pub(crate) fn encode(self) -> [u8; 24] {
        let mut bytes = [0u8; 24];
        bytes[..8].copy_from_slice(&self.offset.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.length.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.cardinality.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.checksum.to_le_bytes());
        bytes
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() != 24 {
            return Err("posting descriptor size mismatch".into());
        }
        Ok(Self {
            offset: u64_at(bytes, 0)?,
            length: u32_at(bytes, 8)?,
            cardinality: u32_at(bytes, 12)?,
            checksum: u64_at(bytes, 16)?,
            ordinal: u32::MAX,
        })
    }

    pub(crate) fn with_ordinal(mut self, ordinal: u32) -> Self {
        if self.length != 0 {
            self.ordinal = ordinal;
        }
        self
    }
}


/// Stream sections directly into the final owned buffer or immutable file.
/// Only the small section directory is retained by this writer.
pub(crate) struct SegmentWriter<W: Write + Seek> {
    writer: W,
    expected_sections: usize,
    sections: Vec<(SectionKind, u64, u64, [u8; 32])>,
}


impl<W: Write + Seek> SegmentWriter<W> {
    pub(crate) fn new(mut writer: W, expected_sections: usize) -> Result<Self, String> {
        if expected_sections == 0 || expected_sections > MAX_SECTIONS {
            return Err("invalid segment section count".into());
        }
        let directory_bytes = HEADER_BYTES
            .checked_add(
                expected_sections
                    .checked_mul(DIRECTORY_ENTRY_BYTES)
                    .ok_or("segment directory overflow")?,
            )
            .ok_or("segment directory overflow")?;
        writer
            .write_all(&vec![0; directory_bytes])
            .map_err(|error| error.to_string())?;
        Ok(Self {
            writer,
            expected_sections,
            sections: Vec::with_capacity(expected_sections),
        })
    }

    pub(crate) fn write_section(
        &mut self,
        kind: SectionKind,
        produce: impl FnOnce(&mut dyn Write) -> Result<(), String>,
    ) -> Result<(), String> {
        if self.sections.len() >= self.expected_sections
            || self.sections.iter().any(|entry| entry.0 == kind)
        {
            return Err("segment section duplicate or count exceeded".into());
        }
        let start = self
            .writer
            .stream_position()
            .map_err(|error| error.to_string())?;
        let mut hashing = HashingWriter::new(&mut self.writer);
        produce(&mut hashing)?;
        let (length, digest) = hashing.finish();
        if start.checked_add(length).ok_or("segment size overflow")? > MAX_SEGMENT_BYTES {
            return Err("segment too large".into());
        }
        self.sections.push((kind, start, length, digest));
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<W, String> {
        if self.sections.len() != self.expected_sections {
            return Err("segment section count mismatch".into());
        }
        self.sections.sort_unstable_by_key(|entry| entry.0 as u32);
        let directory_end = HEADER_BYTES + self.expected_sections * DIRECTORY_ENTRY_BYTES;
        self.writer
            .seek(SeekFrom::Start(0))
            .map_err(|error| error.to_string())?;
        self.writer
            .write_all(MAGIC)
            .map_err(|error| error.to_string())?;
        self.writer
            .write_all(&FORMAT_VERSION.to_le_bytes())
            .map_err(|error| error.to_string())?;
        self.writer
            .write_all(&(self.expected_sections as u32).to_le_bytes())
            .map_err(|error| error.to_string())?;
        self.writer
            .write_all(&(directory_end as u64).to_le_bytes())
            .map_err(|error| error.to_string())?;
        for (kind, offset, length, checksum) in self.sections {
            self.writer
                .write_all(&(kind as u32).to_le_bytes())
                .and_then(|_| self.writer.write_all(&0u32.to_le_bytes()))
                .and_then(|_| self.writer.write_all(&offset.to_le_bytes()))
                .and_then(|_| self.writer.write_all(&length.to_le_bytes()))
                .and_then(|_| self.writer.write_all(&checksum))
                .map_err(|error| error.to_string())?;
        }
        self.writer.flush().map_err(|error| error.to_string())?;
        Ok(self.writer)
    }
}


impl SegmentWriter<std::io::Cursor<Vec<u8>>> {
    /// The owned writer learns the exact remaining section lengths after the
    /// posting stream is complete. Reserve once instead of doubling the final
    /// resident segment buffer while those section buffers still exist.
    pub(crate) fn reserve_remaining(&mut self, bytes: usize) -> Result<(), String> {
        let final_len = self
            .writer
            .get_ref()
            .len()
            .checked_add(bytes)
            .ok_or("owned segment size overflow")?;
        if u64::try_from(final_len).map_err(|_| "owned segment size overflow")? > MAX_SEGMENT_BYTES
        {
            return Err("owned segment exceeds format limit".into());
        }
        self.writer
            .get_mut()
            .try_reserve_exact(bytes)
            .map_err(|error| format!("owned segment reserve: {error}"))
    }
}


struct HashingWriter<'a, W: Write> {
    writer: &'a mut W,
    hash: Sha256,
    length: u64,
}


impl<'a, W: Write> HashingWriter<'a, W> {
    fn new(writer: &'a mut W) -> Self {
        Self {
            writer,
            hash: Sha256::new(),
            length: 0,
        }
    }

    fn finish(self) -> (u64, [u8; 32]) {
        (self.length, self.hash.finalize().into())
    }
}


impl<W: Write> Write for HashingWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let allowed =
            usize::try_from(MAX_SEGMENT_BYTES.saturating_sub(self.length)).unwrap_or(usize::MAX);
        if bytes.len() > allowed {
            return Err(io::Error::other("segment section too large"));
        }
        let written = self.writer.write(bytes)?;
        self.hash.update(&bytes[..written]);
        self.length += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}


fn short_checksum(bytes: &[u8]) -> u64 {
    let digest = Sha256::digest(bytes);
    u64::from_le_bytes(digest[..8].try_into().unwrap())
}

fn u32_at(bytes: &[u8], start: usize) -> Result<u32, String> {
    Ok(u32::from_le_bytes(
        bytes
            .get(start..start + 4)
            .ok_or("truncated segment integer")?
            .try_into()
            .unwrap(),
    ))
}

fn u64_at(bytes: &[u8], start: usize) -> Result<u64, String> {
    Ok(u64::from_le_bytes(
        bytes
            .get(start..start + 8)
            .ok_or("truncated segment integer")?
            .try_into()
            .unwrap(),
    ))
}

#[cfg(test)]
#[path = "segment_format.test.rs"]
mod tests;
