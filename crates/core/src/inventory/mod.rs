//! Repository-wide files and collection-generation record ownership.

use std::hash::{Hash, Hasher};
use std::ops::Bound;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::model::{CollectionId, FileId, RecordId, RecordSlot};
use crate::query::Operator;
use crate::source::{DiagnosticCategory, ReaderDiagnostic};

pub mod discovery;
#[cfg(test)]
pub(crate) use crate::index::memory::inventory::{
    CollectionBuilder, CollectionGeneration, FileInventory, FileRecordIndex,
};
pub(crate) use crate::index::storage::liveness::ActiveRecords;

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

/// Filesystem object observed together with a source file's revision.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceObjectId {
    pub device: u64,
    pub inode: u64,
}

/// Size, modification time and available object identity of a source file.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceRevision {
    pub modified_ns: i128,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_id: Option<SourceObjectId>,
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
            object_id: {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    Some(SourceObjectId {
                        device: metadata.dev(),
                        inode: metadata.ino(),
                    })
                }
                #[cfg(not(unix))]
                {
                    None
                }
            },
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
            Self::Native(path) => crate::model::path_representation::query_path(path).into(),
        }
    }
}

impl From<PathBuf> for SharedPath {
    fn from(path: PathBuf) -> Self {
        if let Some(text) = path.to_str()
            && crate::model::path_representation::query_path(&path) == text
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

/// Opt-in component accounting for the file table and its path indexes.
/// Hash-table control bytes, B-tree nodes and allocator rounding are omitted.
#[derive(Clone, Debug, Default, Serialize)]
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
    pub scalar_index_bytes: usize,
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
