//! Repository-wide files and collection-generation record ownership.

use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::ops::Bound;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use crate::generation_catalog::RestoredGenerationCatalog;
use std::time::SystemTime;

use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};

use crate::model::{CollectionId, FileId, IdentitySet, RecordId, RecordSlot};
use crate::query::Operator;
use crate::source::{DiagnosticCategory, ReaderDiagnostic};

/// Append a batch-local bitmap after its final slot base is known. Batch
/// admission is ordered, so each translated set must follow existing slots.
pub(crate) fn append_shifted_bitmap(
    destination: &mut RoaringBitmap,
    source: &RoaringBitmap,
    base: u32,
) -> Result<(), String> {
    if let Some(max) = source.max() {
        base.checked_add(max).ok_or("record slot overflow")?;
    }
    destination
        .append(source.iter().map(|slot| base + slot))
        .map_err(|_| "batch bitmap slots overlap or are out of order")?;
    Ok(())
}

pub const MAX_DIAGNOSTICS_PER_FILE: usize = 64;
pub const MAX_DIAGNOSTIC_MESSAGE_BYTES: usize = 256;
pub const MAX_DIAGNOSTIC_CODE_BYTES: usize = 128;
pub const MAX_DIAGNOSTIC_PATH_SEGMENTS: usize = 32;
pub const MAX_DIAGNOSTIC_SEGMENT_BYTES: usize = 128;

/// Stable physical-file state codes in snapshots and public results.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub enum FileStatus {
    #[default]
    Valid = 0,
    Broken = 1,
    Unreadable = 2,
}

impl FileStatus {
    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Valid => "Valid",
            Self::Broken => "Broken",
            Self::Unreadable => "Unreadable",
        }
    }
}

impl Serialize for FileStatus {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(self.code())
    }
}

impl<'de> Deserialize<'de> for FileStatus {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match u8::deserialize(deserializer)? {
            0 => Ok(Self::Valid),
            1 => Ok(Self::Broken),
            2 => Ok(Self::Unreadable),
            code => Err(serde::de::Error::custom(format!(
                "unsupported file status code {code}"
            ))),
        }
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RecordClass {
    #[default]
    Logical = 0,
    RawFallback = 1,
}

impl RecordClass {
    pub fn code(self) -> u8 {
        self as u8
    }
}

impl Serialize for RecordClass {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(self.code())
    }
}

impl<'de> Deserialize<'de> for RecordClass {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match u8::deserialize(deserializer)? {
            0 => Ok(Self::Logical),
            1 => Ok(Self::RawFallback),
            code => Err(serde::de::Error::custom(format!(
                "unsupported record class code {code}"
            ))),
        }
    }
}

/// Zero means no diagnostic, `u32::MAX` means omitted by the per-file cap,
/// and other values are one-based indexes into the file diagnostic table.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticRef(pub u32);

impl DiagnosticRef {
    pub fn stored_index(self) -> Option<usize> {
        (self.0 != 0 && self.0 != u32::MAX).then_some(self.0.saturating_sub(1) as usize)
    }

    pub fn omitted(self) -> bool {
        self.0 == u32::MAX
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceDiagnostic {
    pub file: FileId,
    pub collection: CollectionId,
    pub source_id: String,
    pub source_version: u32,
    pub record: Option<RecordSlot>,
    pub range: Option<ByteRange>,
    pub category: DiagnosticCategory,
    pub code: String,
    pub field_path: Option<Vec<String>>,
    pub byte_offset: Option<u64>,
    pub line: Option<u64>,
    pub column: Option<u64>,
    pub message: String,
}

impl SourceDiagnostic {
    pub fn from_reader(
        file: FileId,
        collection: CollectionId,
        source_id: &str,
        source_version: u32,
        range: ByteRange,
        record: Option<RecordSlot>,
        issue: ReaderDiagnostic,
    ) -> Self {
        Self {
            file,
            collection,
            source_id: source_id.into(),
            source_version,
            record,
            range: Some(range),
            category: issue.category,
            code: issue.code,
            field_path: issue.field_path,
            byte_offset: issue
                .relative_byte
                .and_then(|offset| range.start.checked_add(offset)),
            line: issue.line,
            column: issue.column,
            message: issue.message,
        }
    }
}

fn truncate_message(message: &str) -> String {
    if message.len() <= MAX_DIAGNOSTIC_MESSAGE_BYTES {
        return message.into();
    }
    let mut end = MAX_DIAGNOSTIC_MESSAGE_BYTES;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    message[..end].into()
}

/// Size and modification time recorded for a source file.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceRevision {
    pub modified_ns: i128,
    pub size: u64,
}

impl SourceRevision {
    pub fn from_metadata(metadata: &std::fs::Metadata) -> Result<Self, String> {
        let modified = metadata.modified().map_err(|e| e.to_string())?;
        let duration = modified
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|e| format!("modification time before Unix epoch: {e}"))?;
        Ok(Self {
            modified_ns: duration.as_nanos() as i128,
            size: metadata.len(),
        })
    }
}

/// Index-local incarnation token for a physical file. It is independent of
/// modification time and changes after every accepted file mutation.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct FileVersion(pub u64);

impl FileVersion {
    pub const INITIAL: Self = Self(1);

    pub fn next(self) -> Result<Self, String> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or("file version overflow".into())
    }
}

const FILE_VERSION_CHUNK: usize = 1024;

/// Version metadata is divided into shared chunks so changing one file does
/// not copy every file's token. The outer pointer directory is shallow.
#[derive(Clone, Debug)]
pub(crate) struct FileVersions {
    chunks: Arc<Vec<Arc<Vec<FileVersion>>>>,
    persisted: Option<Arc<RestoredGenerationCatalog>>,
    overrides: Arc<Vec<Arc<HashMap<FileId, FileVersion>>>>,
    len: usize,
}

impl FileVersions {
    /// Restore version tokens without replaying every historical mutation.
    #[cfg(test)]
    pub(crate) fn from_versions(versions: Vec<FileVersion>) -> Result<Self, String> {
        let len = versions.len();
        u32::try_from(len).map_err(|_| "too many indexed file versions")?;
        if versions.iter().any(|version| version.0 == 0) {
            return Err("file version must be nonzero".into());
        }
        let chunks = versions
            .chunks(FILE_VERSION_CHUNK)
            .map(|chunk| Arc::new(chunk.to_vec()))
            .collect();
        Ok(Self {
            chunks: Arc::new(chunks),
            persisted: None,
            overrides: Arc::new(Vec::new()),
            len,
        })
    }

    pub(crate) fn from_persisted(catalog: Arc<RestoredGenerationCatalog>) -> Self {
        Self {
            chunks: Arc::new(Vec::new()),
            len: catalog.slot_count(),
            persisted: Some(catalog),
            overrides: Arc::new(vec![Arc::new(HashMap::new()); 1024]),
        }
    }

    pub(crate) fn initial(len: usize) -> Self {
        Self::filled(len, FileVersion::INITIAL)
    }

    pub(crate) fn filled(len: usize, version: FileVersion) -> Self {
        let chunks = (0..len)
            .step_by(FILE_VERSION_CHUNK)
            .map(|start| Arc::new(vec![version; (len - start).min(FILE_VERSION_CHUNK)]))
            .collect();
        Self {
            chunks: Arc::new(chunks),
            persisted: None,
            overrides: Arc::new(Vec::new()),
            len,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn get(&self, file: FileId) -> Option<FileVersion> {
        let index = file.0 as usize;
        if index >= self.len {
            return None;
        }
        if let Some(base) = &self.persisted {
            return self.overrides[index % self.overrides.len()]
                .get(&file).copied().or_else(|| base.version(file));
        }
        self.chunks
            .get(index / FILE_VERSION_CHUNK)?
            .get(index % FILE_VERSION_CHUNK)
            .copied()
    }

    /// Create a private next version without changing any published view.
    pub(crate) fn advanced(&self, file: FileId) -> Result<(Self, FileVersion), String> {
        let index = file.0 as usize;
        if index >= self.len {
            return Err("file ID out of bounds".into());
        }
        if self.persisted.is_some() {
            let next_version = self.get(file).ok_or("file version missing")?.next()?;
            let mut next = self.clone();
            let shards = Arc::make_mut(&mut next.overrides);
            let bucket = index % shards.len();
            Arc::make_mut(&mut shards[bucket]).insert(file, next_version);
            return Ok((next, next_version));
        }
        let mut chunks = (*self.chunks).clone();
        let token = Arc::make_mut(&mut chunks[index / FILE_VERSION_CHUNK])
            .get_mut(index % FILE_VERSION_CHUNK)
            .ok_or("file version missing")?;
        *token = token.next()?;
        let next_version = *token;
        Ok((
            Self {
                chunks: Arc::new(chunks),
                persisted: None,
                overrides: Arc::new(Vec::new()),
                len: self.len,
            },
            next_version,
        ))
    }

    /// Assign a never-before-used file ID its first incarnation token.
    pub(crate) fn appended(&self) -> Result<Self, String> {
        u32::try_from(self.len).map_err(|_| "too many files")?;
        if self.persisted.is_some() {
            let mut next = self.clone();
            let shards = Arc::make_mut(&mut next.overrides);
            let bucket = self.len % shards.len();
            Arc::make_mut(&mut shards[bucket])
                .insert(FileId(self.len as u32), FileVersion::INITIAL);
            next.len += 1;
            return Ok(next);
        }
        let mut chunks = (*self.chunks).clone();
        if self.len % FILE_VERSION_CHUNK == 0 {
            chunks.push(Arc::new(vec![FileVersion::INITIAL]));
        } else {
            Arc::make_mut(chunks.last_mut().ok_or("file version chunk missing")?)
                .push(FileVersion::INITIAL);
        }
        Ok(Self {
            chunks: Arc::new(chunks),
            persisted: None,
            overrides: Arc::new(Vec::new()),
            len: self.len + 1,
        })
    }
}

const ACTIVE_SLOT_CHUNK: u32 = 1 << 16;

/// Published liveness for one collection. Slot numbers are never reused by
/// ordinary mutations. Only touched chunks are copied on retirement.
#[derive(Clone, Debug)]
pub(crate) struct ActiveRecords {
    chunks: Arc<Vec<Arc<RoaringBitmap>>>,
    slot_count: u32,
    live_count: u64,
}

impl ActiveRecords {
    /// Restore a sparse active domain, including fully retired chunks.
    pub(crate) fn from_bitmap(slot_count: usize, bitmap: RoaringBitmap) -> Result<Self, String> {
        let slot_count = u32::try_from(slot_count).map_err(|_| "too many records")?;
        if bitmap.max().is_some_and(|slot| slot >= slot_count) {
            return Err("active record slot out of bounds".into());
        }
        let chunk_count = (slot_count as usize).div_ceil(ACTIVE_SLOT_CHUNK as usize);
        let mut chunks = vec![RoaringBitmap::new(); chunk_count];
        for slot in bitmap.iter() {
            chunks[(slot / ACTIVE_SLOT_CHUNK) as usize].insert(slot % ACTIVE_SLOT_CHUNK);
        }
        Ok(Self {
            chunks: Arc::new(chunks.into_iter().map(Arc::new).collect()),
            slot_count,
            live_count: bitmap.len(),
        })
    }

    pub(crate) fn all(slot_count: usize) -> Result<Self, String> {
        let slot_count = u32::try_from(slot_count).map_err(|_| "too many records")?;
        let mut chunks = Vec::new();
        let mut start = 0u32;
        while start < slot_count {
            let end = slot_count.min(start.saturating_add(ACTIVE_SLOT_CHUNK));
            let mut active = RoaringBitmap::new();
            active.insert_range(0..end - start);
            chunks.push(Arc::new(active));
            start = end;
        }
        Ok(Self {
            chunks: Arc::new(chunks),
            slot_count,
            live_count: slot_count as u64,
        })
    }

    pub(crate) fn slot_count(&self) -> u32 {
        self.slot_count
    }

    pub(crate) fn len(&self) -> u64 {
        self.live_count
    }

    pub(crate) fn contains(&self, slot: RecordSlot) -> bool {
        if slot.0 >= self.slot_count {
            return false;
        }
        self.chunks
            .get((slot.0 / ACTIVE_SLOT_CHUNK) as usize)
            .is_some_and(|chunk| chunk.contains(slot.0 % ACTIVE_SLOT_CHUNK))
    }

    pub(crate) fn retired(&self, slots: &[RecordSlot]) -> Result<Self, String> {
        let mut chunks = (*self.chunks).clone();
        let mut live_count = self.live_count;
        for slot in slots {
            if slot.0 >= self.slot_count {
                return Err("record slot out of bounds".into());
            }
            let chunk = Arc::make_mut(&mut chunks[(slot.0 / ACTIVE_SLOT_CHUNK) as usize]);
            if chunk.remove(slot.0 % ACTIVE_SLOT_CHUNK) {
                live_count -= 1;
            }
        }
        Ok(Self {
            chunks: Arc::new(chunks),
            slot_count: self.slot_count,
            live_count,
        })
    }

    #[cfg(test)]
    pub(crate) fn appended(&self, count: u32) -> Result<Self, String> {
        let slot_count = self
            .slot_count
            .checked_add(count)
            .ok_or("record slot overflow")?;
        let mut chunks = (*self.chunks).clone();
        let mut current = self.slot_count;
        while current < slot_count {
            let group = (current / ACTIVE_SLOT_CHUNK) as usize;
            let local = current % ACTIVE_SLOT_CHUNK;
            let take = (slot_count - current).min(ACTIVE_SLOT_CHUNK - local);
            if group == chunks.len() {
                chunks.push(Arc::new(RoaringBitmap::new()));
            }
            Arc::make_mut(&mut chunks[group]).insert_range(local..local + take);
            current += take;
        }
        Ok(Self {
            chunks: Arc::new(chunks),
            slot_count,
            live_count: self.live_count + count as u64,
        })
    }

    pub(crate) fn bitmap(&self) -> RoaringBitmap {
        if self.live_count == self.slot_count as u64 {
            let mut full = RoaringBitmap::new();
            full.insert_range(0..self.slot_count);
            return full;
        }
        self.chunks
            .iter()
            .enumerate()
            .flat_map(|(group, chunk)| {
                let base = group as u32 * ACTIVE_SLOT_CHUNK;
                chunk.iter().map(move |slot| base + slot)
            })
            .collect()
    }
}

/// Immutable native path shared with lookup keys and, for ordinary paths,
/// with the ordered query-path index.
#[derive(Clone, Debug)]
pub enum SharedPath {
    /// Ordinary paths have the same native and query spelling, so the ordered
    /// query index can share this allocation with the file table.
    Plain(Arc<str>),
    /// Escaped or non-UTF-8 paths retain their reversible native spelling.
    Native(Arc<Path>),
}

impl SharedPath {
    pub fn as_path(&self) -> &Path {
        match self {
            Self::Plain(text) => Path::new(text.as_ref()),
            Self::Native(path) => path,
        }
    }

    pub(crate) fn query_key(&self) -> Arc<str> {
        match self {
            Self::Plain(text) => Arc::clone(text),
            Self::Native(path) => crate::path_repr::query_path(path).into(),
        }
    }

    fn payload_bytes(&self) -> usize {
        self.as_path().as_os_str().len()
    }

    fn query_key_has_separate_payload(&self) -> bool {
        matches!(self, Self::Native(_))
    }
}

impl From<PathBuf> for SharedPath {
    fn from(path: PathBuf) -> Self {
        if let Some(text) = path.to_str()
            && crate::path_repr::query_path(&path) == text
        {
            return Self::Plain(Arc::from(text));
        }
        Self::Native(Arc::from(path))
    }
}

impl From<&str> for SharedPath {
    fn from(path: &str) -> Self {
        PathBuf::from(path).into()
    }
}

impl From<String> for SharedPath {
    fn from(path: String) -> Self {
        PathBuf::from(path).into()
    }
}

impl From<std::ffi::OsString> for SharedPath {
    fn from(path: std::ffi::OsString) -> Self {
        PathBuf::from(path).into()
    }
}

impl Deref for SharedPath {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        self.as_path()
    }
}

impl AsRef<Path> for SharedPath {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

impl std::borrow::Borrow<Path> for SharedPath {
    fn borrow(&self) -> &Path {
        self.as_path()
    }
}

impl PartialEq for SharedPath {
    fn eq(&self, other: &Self) -> bool {
        self.as_path() == other.as_path()
    }
}

impl Eq for SharedPath {}

impl PartialEq<PathBuf> for SharedPath {
    fn eq(&self, other: &PathBuf) -> bool {
        self.as_path() == other.as_path()
    }
}

impl PartialEq<&Path> for SharedPath {
    fn eq(&self, other: &&Path) -> bool {
        self.as_path() == *other
    }
}

impl Hash for SharedPath {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_path().hash(state);
    }
}

impl Serialize for SharedPath {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.as_path().to_path_buf().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SharedPath {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        PathBuf::deserialize(deserializer).map(Self::from)
    }
}

/// Repository-wide identity and metadata for one source file.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileEntry {
    pub id: FileId,
    pub relative_path: SharedPath,
    pub revision: SourceRevision,
    pub created_ns: Option<i128>,
    pub collections: Vec<CollectionId>,
    pub status: FileStatus,
    pub diagnostics: Vec<SourceDiagnostic>,
    pub omitted_diagnostics: u32,
}

impl FileEntry {
    pub(crate) fn record_diagnostic(
        &mut self,
        mut diagnostic: SourceDiagnostic,
    ) -> Result<DiagnosticRef, String> {
        if diagnostic.code.is_empty()
            || diagnostic.code.len() > MAX_DIAGNOSTIC_CODE_BYTES
            || diagnostic.field_path.as_ref().is_some_and(|path| {
                path.len() > MAX_DIAGNOSTIC_PATH_SEGMENTS
                    || path
                        .iter()
                        .any(|part| part.len() > MAX_DIAGNOSTIC_SEGMENT_BYTES)
            })
        {
            return Err("reader diagnostic contract exceeds bounded metadata".into());
        }
        if diagnostic.file != self.id || !self.collections.contains(&diagnostic.collection) {
            return Err("diagnostic ownership mismatch".into());
        }
        self.status = self.status.max(match diagnostic.category {
            DiagnosticCategory::Io => FileStatus::Unreadable,
            _ => FileStatus::Broken,
        });
        if self.diagnostics.len() == MAX_DIAGNOSTICS_PER_FILE {
            self.omitted_diagnostics = self.omitted_diagnostics.saturating_add(1);
            return Ok(DiagnosticRef(u32::MAX));
        }
        diagnostic.message = truncate_message(&diagnostic.message);
        let index =
            u32::try_from(self.diagnostics.len() + 1).map_err(|_| "diagnostic index overflow")?;
        self.diagnostics.push(diagnostic);
        Ok(DiagnosticRef(index))
    }
}

/// Authoritative source-file table shared by collections.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileInventory {
    pub root: PathBuf,
    pub files: Vec<FileEntry>,
    #[serde(skip)]
    by_path: HashMap<SharedPath, FileId>,
    /// Ordered root-relative query keys, rebuilt with every restored index.
    #[serde(skip)]
    pub(crate) relative_paths: BTreeMap<Arc<str>, FileId>,
}

/// Opt-in component accounting for the file table and its path indexes.
/// Hash-table control bytes, B-tree nodes and allocator rounding are omitted.
#[derive(Clone, Debug, Serialize)]
pub struct FileStorageDiagnostic {
    pub files_len: usize,
    pub files_capacity: usize,
    pub file_entry_size: usize,
    pub file_table_slot_bytes: usize,
    pub native_path_payload_bytes: usize,
    pub native_path_payload_allocations: usize,
    pub exact_lookup_capacity: usize,
    pub exact_lookup_slot_bytes: usize,
    pub ordered_lookup_len: usize,
    pub ordered_lookup_slot_bytes: usize,
    pub escaped_query_keys: usize,
    pub escaped_query_payload_bytes: usize,
    pub collection_membership_bytes: usize,
    pub root_payload_bytes: usize,
    pub estimated_live_bytes: usize,
}

/// Lexical bounds over the same root-relative representation as `@file.path`.
pub(crate) fn relative_path_bounds(
    operator: Operator,
    literal: &str,
) -> Option<(Bound<Arc<str>>, Bound<Arc<str>>)> {
    let key: Arc<str> = literal.into();
    Some(match operator {
        Operator::Eq => (Bound::Included(key.clone()), Bound::Included(key)),
        Operator::Gt => (Bound::Excluded(key), Bound::Unbounded),
        Operator::Ge => (Bound::Included(key), Bound::Unbounded),
        Operator::Lt => (Bound::Unbounded, Bound::Excluded(key)),
        Operator::Le => (Bound::Unbounded, Bound::Included(key)),
        _ => return None,
    })
}

pub(crate) fn empty_path_bounds(bounds: &(Bound<Arc<str>>, Bound<Arc<str>>)) -> bool {
    match (&bounds.0, &bounds.1) {
        (Bound::Unbounded, _) | (_, Bound::Unbounded) => false,
        (
            Bound::Included(lower) | Bound::Excluded(lower),
            Bound::Included(upper) | Bound::Excluded(upper),
        ) => {
            lower > upper
                || (lower == upper
                    && !matches!(
                        (&bounds.0, &bounds.1),
                        (Bound::Included(_), Bound::Included(_))
                    ))
        }
    }
}

impl FileInventory {
    pub fn storage_diagnostic(&self) -> FileStorageDiagnostic {
        let native_path_payload_bytes = self
            .files
            .iter()
            .map(|file| file.relative_path.payload_bytes() + 2 * std::mem::size_of::<usize>())
            .sum::<usize>();
        let escaped_query_keys = self
            .files
            .iter()
            .filter(|file| file.relative_path.query_key_has_separate_payload())
            .count();
        let escaped_query_payload_bytes = self
            .files
            .iter()
            .filter(|file| file.relative_path.query_key_has_separate_payload())
            .map(|file| {
                crate::path_repr::query_path(&file.relative_path).len()
                    + 2 * std::mem::size_of::<usize>()
            })
            .sum::<usize>();
        let file_table_slot_bytes = self.files.capacity() * std::mem::size_of::<FileEntry>();
        let exact_lookup_slot_bytes =
            self.by_path.capacity() * std::mem::size_of::<(SharedPath, FileId)>();
        let ordered_lookup_slot_bytes =
            self.relative_paths.len() * std::mem::size_of::<(Arc<str>, FileId)>();
        let collection_membership_bytes = self
            .files
            .iter()
            .map(|file| file.collections.capacity() * std::mem::size_of::<CollectionId>())
            .sum::<usize>();
        let root_payload_bytes = self.root.capacity();
        FileStorageDiagnostic {
            files_len: self.files.len(),
            files_capacity: self.files.capacity(),
            file_entry_size: std::mem::size_of::<FileEntry>(),
            file_table_slot_bytes,
            native_path_payload_bytes,
            native_path_payload_allocations: self.files.len(),
            exact_lookup_capacity: self.by_path.capacity(),
            exact_lookup_slot_bytes,
            ordered_lookup_len: self.relative_paths.len(),
            ordered_lookup_slot_bytes,
            escaped_query_keys,
            escaped_query_payload_bytes,
            collection_membership_bytes,
            root_payload_bytes,
            estimated_live_bytes: file_table_slot_bytes
                + native_path_payload_bytes
                + exact_lookup_slot_bytes
                + ordered_lookup_slot_bytes
                + escaped_query_payload_bytes
                + collection_membership_bytes
                + root_payload_bytes,
        }
    }

    /// Approximate heap used by inventory and retained diagnostics, separately.
    /// Hash-table control bytes and allocator overhead are not included.
    pub fn estimated_heap_bytes(&self) -> (usize, usize) {
        let mut inventory = self.root.capacity()
            + self.files.capacity() * std::mem::size_of::<FileEntry>()
            + self.by_path.capacity() * std::mem::size_of::<(SharedPath, FileId)>();
        let mut diagnostics = 0;
        for file in &self.files {
            inventory += file.relative_path.payload_bytes()
                + file.collections.capacity() * std::mem::size_of::<CollectionId>();
            diagnostics += file.diagnostics.capacity() * std::mem::size_of::<SourceDiagnostic>();
            for diagnostic in &file.diagnostics {
                diagnostics += diagnostic.source_id.capacity()
                    + diagnostic.code.capacity()
                    + diagnostic.message.capacity();
                if let Some(path) = &diagnostic.field_path {
                    diagnostics += path.capacity() * std::mem::size_of::<String>();
                    diagnostics += path.iter().map(String::capacity).sum::<usize>();
                }
            }
        }
        inventory += self.relative_paths.len() * std::mem::size_of::<(Arc<str>, FileId)>();
        inventory += self
            .files
            .iter()
            .filter(|file| file.relative_path.query_key_has_separate_payload())
            .map(|file| crate::path_repr::query_path(&file.relative_path).len())
            .sum::<usize>();
        (inventory, diagnostics)
    }

    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            files: Vec::new(),
            by_path: HashMap::new(),
            relative_paths: BTreeMap::new(),
        }
    }

    /// Restore an already decoded file table without first populating lookup
    /// maps through `add`. `rebuild_lookup` validates identities and paths,
    /// constructs both final maps once, and checks diagnostic ownership.
    #[cfg(test)]
    pub(crate) fn from_restored_entries(
        root: PathBuf,
        entries: Vec<FileEntry>,
    ) -> Result<Self, String> {
        let mut inventory = Self {
            root,
            files: entries,
            by_path: HashMap::new(),
            relative_paths: BTreeMap::new(),
        };
        inventory.rebuild_lookup()?;
        Ok(inventory)
    }

    pub fn add(
        &mut self,
        relative_path: PathBuf,
        revision: SourceRevision,
        created_ns: Option<i128>,
        collection: CollectionId,
    ) -> Result<FileId, String> {
        if !valid_relative_path(&relative_path) {
            return Err("file path must be repository-relative and contained".into());
        }
        if let Some(&id) = self.by_path.get(relative_path.as_path()) {
            let file = &mut self.files[id.0 as usize];
            if file.revision != revision {
                return Err("one file has inconsistent revisions".into());
            }
            if !file.collections.contains(&collection) {
                file.collections.push(collection);
            }
            return Ok(id);
        }
        let id = FileId(u32::try_from(self.files.len()).map_err(|_| "too many files")?);
        let relative_path = SharedPath::from(relative_path);
        self.by_path.insert(relative_path.clone(), id);
        self.relative_paths.insert(relative_path.query_key(), id);
        self.files.push(FileEntry {
            id,
            relative_path,
            revision,
            created_ns,
            collections: vec![collection],
            status: FileStatus::Valid,
            diagnostics: Vec::new(),
            omitted_diagnostics: 0,
        });
        Ok(id)
    }

    pub fn record_diagnostic(
        &mut self,
        file: FileId,
        diagnostic: SourceDiagnostic,
    ) -> Result<DiagnosticRef, String> {
        let entry = self
            .files
            .get_mut(file.0 as usize)
            .ok_or("diagnostic file out of bounds")?;
        entry.record_diagnostic(diagnostic)
    }

    pub fn diagnostic(&self, file: FileId, reference: DiagnosticRef) -> Option<&SourceDiagnostic> {
        self.files
            .get(file.0 as usize)?
            .diagnostics
            .get(reference.stored_index()?)
    }

    pub fn validate_diagnostics(&self) -> Result<(), String> {
        for (index, file) in self.files.iter().enumerate() {
            if file.id.0 as usize != index || file.diagnostics.len() > MAX_DIAGNOSTICS_PER_FILE {
                return Err("invalid file diagnostic table".into());
            }
            let derived = file
                .diagnostics
                .iter()
                .fold(FileStatus::Valid, |status, diagnostic| {
                    status.max(match diagnostic.category {
                        DiagnosticCategory::Io => FileStatus::Unreadable,
                        _ => FileStatus::Broken,
                    })
                });
            if (file.omitted_diagnostics == 0 && file.status != derived)
                || (file.omitted_diagnostics != 0
                    && (file.status == FileStatus::Valid
                        || derived == FileStatus::Valid
                        || file.status < derived))
            {
                return Err("file status and diagnostics disagree".into());
            }
            for diagnostic in &file.diagnostics {
                if diagnostic.file != file.id
                    || !file.collections.contains(&diagnostic.collection)
                    || diagnostic.source_id.is_empty()
                    || diagnostic.source_version == 0
                    || diagnostic.code.is_empty()
                    || diagnostic.code.len() > MAX_DIAGNOSTIC_CODE_BYTES
                    || diagnostic.field_path.as_ref().is_some_and(|path| {
                        path.len() > MAX_DIAGNOSTIC_PATH_SEGMENTS
                            || path
                                .iter()
                                .any(|part| part.len() > MAX_DIAGNOSTIC_SEGMENT_BYTES)
                    })
                    || diagnostic.message.len() > MAX_DIAGNOSTIC_MESSAGE_BYTES
                    || diagnostic.range.is_some_and(|range| {
                        range.start > range.end || range.end > file.revision.size
                    })
                    || diagnostic
                        .byte_offset
                        .is_some_and(|offset| offset > file.revision.size)
                {
                    return Err("invalid source diagnostic".into());
                }
            }
        }
        Ok(())
    }

    pub fn rebuild_lookup(&mut self) -> Result<(), String> {
        self.by_path.clear();
        self.by_path.reserve(self.files.len());
        self.relative_paths.clear();
        for (slot, file) in self.files.iter().enumerate() {
            if !valid_relative_path(&file.relative_path)
                || file.id.0 as usize != slot
                || self
                    .by_path
                    .insert(file.relative_path.clone(), file.id)
                    .is_some()
            {
                return Err("invalid restored file inventory".into());
            }
            if self
                .relative_paths
                .insert(file.relative_path.query_key(), file.id)
                .is_some()
            {
                return Err("invalid restored relative file path index".into());
            }
        }
        self.validate_diagnostics()?;
        Ok(())
    }

    pub fn native_path(&self, id: FileId) -> Option<PathBuf> {
        self.files
            .get(id.0 as usize)
            .map(|f| self.root.join(&f.relative_path))
    }

    pub fn id_for(&self, path: &Path) -> Option<FileId> {
        self.by_path.get(path).copied()
    }

    pub(crate) fn relative_path_ids_in_bounds(
        &self,
        bounds: (Bound<Arc<str>>, Bound<Arc<str>>),
    ) -> Vec<FileId> {
        if empty_path_bounds(&bounds) {
            return Vec::new();
        }
        self.relative_paths
            .range(bounds)
            .map(|(_, id)| *id)
            .collect()
    }

    pub(crate) fn relative_path_ids_up_to(
        &self,
        operator: Operator,
        literal: &str,
        limit: usize,
    ) -> Option<Vec<FileId>> {
        let bounds = relative_path_bounds(operator, literal)?;
        let mut ids = Vec::new();
        for (_, id) in self.relative_paths.range(bounds) {
            if ids.len() == limit {
                return None;
            }
            ids.push(*id);
        }
        Some(ids)
    }
}

fn valid_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

/// Standalone byte span of one logical record.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
}

impl ByteRange {
    pub fn checked(start: u64, end: u64, file_size: u64) -> Result<Self, String> {
        if start > end || end > file_size {
            return Err(format!(
                "invalid byte range {start}..{end} for file size {file_size}"
            ));
        }
        Ok(Self { start, end })
    }

    pub fn len(self) -> u64 {
        self.end - self.start
    }
}

/// Collection record identity, slot, file, and source span.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecordEntry {
    pub slot: RecordSlot,
    pub id: RecordId,
    pub file: FileId,
    pub range: ByteRange,
    pub revision: SourceRevision,
    pub class: RecordClass,
    pub diagnostic_ref: DiagnosticRef,
}

/// Compact per-file offset table. `slots[offsets[f]..offsets[f+1]]` belongs
/// exactly to file `f`; empty files have equal consecutive offsets.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileRecordIndex {
    /// Valid file-ID domain, including empty generations with no offsets.
    pub file_count: u32,
    pub offsets: Vec<u32>,
    pub slots: Vec<RecordSlot>,
}

impl FileRecordIndex {
    pub fn records_for(&self, file: FileId) -> Option<&[RecordSlot]> {
        if file.0 >= self.file_count {
            return None;
        }
        if self.offsets.is_empty() {
            return self.slots.is_empty().then_some(&[]);
        }
        let a = *self.offsets.get(file.0 as usize)? as usize;
        let b = *self.offsets.get(file.0 as usize + 1)? as usize;
        self.slots.get(a..b)
    }
}

/// Immutable record inventory for one published collection.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CollectionGeneration {
    pub collection: CollectionId,
    pub name: String,
    pub generation: u64,
    pub files: Vec<FileId>,
    pub records: Vec<RecordEntry>,
    pub file_records: FileRecordIndex,
}

impl CollectionGeneration {
    pub fn validate(&self, inventory: &FileInventory) -> Result<(), String> {
        let mut included = std::collections::HashSet::new();
        for file_id in &self.files {
            if !included.insert(*file_id)
                || inventory
                    .files
                    .get(file_id.0 as usize)
                    .is_none_or(|file| !file.collections.contains(&self.collection))
            {
                return Err("invalid collection file membership".into());
            }
        }
        if self.file_records.file_count as usize != inventory.files.len()
            || (self.file_records.offsets.is_empty()
                && (!self.records.is_empty() || !self.file_records.slots.is_empty()))
            || (!self.file_records.offsets.is_empty()
                && (self.file_records.offsets.len() != inventory.files.len() + 1
                    || self.file_records.offsets.first() != Some(&0)
                    || self.file_records.offsets.last().copied()
                        != Some(self.file_records.slots.len() as u32)))
        {
            return Err("invalid file-record offset table".into());
        }
        let mut seen = vec![false; self.records.len()];
        for (file_idx, window) in self.file_records.offsets.windows(2).enumerate() {
            let (a, b) = (window[0] as usize, window[1] as usize);
            if a > b || b > self.file_records.slots.len() {
                return Err("file-record offsets out of bounds".into());
            }
            for slot in &self.file_records.slots[a..b] {
                let i = slot.0 as usize;
                let entry = self
                    .records
                    .get(i)
                    .ok_or("file-record slot out of bounds")?;
                if seen[i]
                    || entry.slot != *slot
                    || entry.file.0 as usize != file_idx
                    || !included.contains(&entry.file)
                {
                    return Err("file-record ownership mismatch".into());
                }
                let file = &inventory.files[file_idx];
                if !file.collections.contains(&self.collection)
                    || entry.revision != file.revision
                    || (entry.range.start >= entry.range.end
                        && !(entry.class == RecordClass::RawFallback
                            && entry.range.start == 0
                            && file.revision.size == 0
                            && entry.range.end == 0))
                    || entry.range.end > file.revision.size
                    || (entry.class == RecordClass::Logical
                        && entry.diagnostic_ref != DiagnosticRef(0))
                    || (entry.class == RecordClass::Logical && entry.id.0.starts_with("~fallback:"))
                    || (entry.class == RecordClass::RawFallback
                        && (file.status == FileStatus::Valid
                            || !entry.id.0.starts_with("~fallback:")
                            || entry.diagnostic_ref == DiagnosticRef(0)
                            || entry.diagnostic_ref.stored_index().is_some_and(|i| {
                                file.diagnostics.get(i).is_none_or(|d| {
                                    d.collection != self.collection || d.record != Some(*slot)
                                })
                            })
                            || (entry.diagnostic_ref.omitted() && file.omitted_diagnostics == 0)))
                {
                    return Err("record range or revision mismatch".into());
                }
                seen[i] = true;
            }
        }
        if seen.iter().any(|v| !v) {
            return Err("unmapped record slot".into());
        }
        Ok(())
    }
}

/// Mutable accumulator used to construct a collection generation.
pub struct CollectionBuilder {
    collection: CollectionId,
    name: String,
    generation: u64,
    files: Vec<FileId>,
    included_files: Vec<bool>,
    records: Vec<RecordEntry>,
    ids: IdentitySet,
}

impl CollectionBuilder {
    pub(crate) fn next_slot(&self) -> Result<RecordSlot, String> {
        Ok(RecordSlot(
            u32::try_from(self.records.len()).map_err(|_| "too many records")?,
        ))
    }

    pub fn collection_id(&self) -> CollectionId {
        self.collection
    }

    pub fn record(&self, slot: RecordSlot) -> Option<&RecordEntry> {
        self.records.get(slot.0 as usize)
    }

    pub fn new(collection: CollectionId, name: String, generation: u64, file_count: usize) -> Self {
        Self {
            collection,
            name,
            generation,
            files: Vec::new(),
            included_files: vec![false; file_count],
            records: Vec::new(),
            ids: IdentitySet::default(),
        }
    }

    pub fn include_file(&mut self, file: FileId) -> Result<(), String> {
        let included = self
            .included_files
            .get_mut(file.0 as usize)
            .ok_or("file ID out of bounds")?;
        if !*included {
            *included = true;
            self.files.push(file);
        }
        Ok(())
    }

    pub(crate) fn contains_file(&self, file: FileId) -> bool {
        self.included_files.get(file.0 as usize) == Some(&true)
    }

    pub fn add_record(
        &mut self,
        file: &FileEntry,
        id: RecordId,
        start: u64,
        end: u64,
    ) -> Result<RecordSlot, String> {
        if self.included_files.get(file.id.0 as usize) != Some(&true) {
            return Err("file is not in collection".into());
        }
        let range = ByteRange::checked(start, end, file.revision.size)?;
        if range.start == range.end {
            return Err("record range must be nonempty".into());
        }
        self.ids.insert(id.clone())?;
        let slot = RecordSlot(u32::try_from(self.records.len()).map_err(|_| "too many records")?);
        self.records.push(RecordEntry {
            slot,
            id,
            file: file.id,
            range,
            revision: file.revision,
            class: RecordClass::Logical,
            diagnostic_ref: DiagnosticRef(0),
        });
        Ok(slot)
    }

    pub fn add_raw_fallback(
        &mut self,
        file: &FileEntry,
        id: RecordId,
        range: ByteRange,
        diagnostic_ref: DiagnosticRef,
    ) -> Result<RecordSlot, String> {
        if self.included_files.get(file.id.0 as usize) != Some(&true)
            || diagnostic_ref == DiagnosticRef(0)
        {
            return Err("fallback file or diagnostic missing".into());
        }
        if range.end > file.revision.size
            || range.start > range.end
            || (range.start == range.end && !(range.start == 0 && file.revision.size == 0))
        {
            return Err("invalid fallback range".into());
        }
        self.ids.insert(id.clone())?;
        let slot = RecordSlot(u32::try_from(self.records.len()).map_err(|_| "too many records")?);
        self.records.push(RecordEntry {
            slot,
            id,
            file: file.id,
            range,
            revision: file.revision,
            class: RecordClass::RawFallback,
            diagnostic_ref,
        });
        Ok(slot)
    }

    pub fn finish(self, inventory: &FileInventory) -> Result<CollectionGeneration, String> {
        let file_count = u32::try_from(self.included_files.len()).map_err(|_| "too many files")?;
        if self.records.is_empty() {
            let result = CollectionGeneration {
                collection: self.collection,
                name: self.name,
                generation: self.generation,
                files: self.files,
                records: self.records,
                file_records: FileRecordIndex {
                    file_count,
                    offsets: Vec::new(),
                    slots: Vec::new(),
                },
            };
            result.validate(inventory)?;
            return Ok(result);
        }
        let mut cursors = vec![0u32; self.included_files.len()];
        for entry in &self.records {
            let count = cursors
                .get_mut(entry.file.0 as usize)
                .ok_or("file ID out of bounds")?;
            *count = count.checked_add(1).ok_or("too many records")?;
        }
        let mut offsets = Vec::with_capacity(cursors.len() + 1);
        offsets.push(0u32);
        for count in &cursors {
            offsets.push(
                (*offsets.last().unwrap())
                    .checked_add(*count)
                    .ok_or("too many records")?,
            );
        }
        let mut slots = vec![RecordSlot(0); self.records.len()];
        for (file, cursor) in cursors.iter_mut().enumerate() {
            *cursor = offsets[file];
        }
        for entry in &self.records {
            let cursor = &mut cursors[entry.file.0 as usize];
            slots[*cursor as usize] = entry.slot;
            *cursor += 1;
        }
        let result = CollectionGeneration {
            collection: self.collection,
            name: self.name,
            generation: self.generation,
            files: self.files,
            records: self.records,
            file_records: FileRecordIndex {
                file_count,
                offsets,
                slots,
            },
        };
        result.validate(inventory)?;
        Ok(result)
    }
}

#[cfg(test)]
#[path = "inventory.test.rs"]
mod tests;
