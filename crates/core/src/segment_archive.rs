//! Archived segment record, file-ownership, and identity lookup storage.

use std::collections::HashMap;
use std::fmt;
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use rkyv::{Archive, Serialize as ArchiveSerialize};

use crate::archive_storage::ValidatedArchive;
use crate::inventory::{
    ByteRange, DiagnosticRef, FileEntry, FileStatus, RecordClass, RecordEntry, SharedPath,
    SourceDiagnostic, SourceRevision,
};
use crate::model::{CollectionId, FileId, RecordId, RecordSlot};
use crate::native_segment::SegmentFile;
use crate::source::DiagnosticCategory;

#[derive(Archive, ArchiveSerialize)]
struct StoredPath {
    query: String,
    #[cfg(unix)]
    native: Vec<u8>,
    #[cfg(windows)]
    native: Vec<u16>,
}

fn stored_path(path: &SharedPath) -> StoredPath {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        StoredPath {
            query: path.query_key().to_string(),
            native: path.as_path().as_os_str().as_bytes().to_vec(),
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        StoredPath {
            query: path.query_key().to_string(),
            native: path.as_path().as_os_str().encode_wide().collect(),
        }
    }
}

fn archived_native_path(path: &rkyv::Archived<StoredPath>) -> SharedPath {
    #[cfg(unix)]
    {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        SharedPath::from(PathBuf::from(OsString::from_vec(
            path.native.as_slice().to_vec(),
        )))
    }
    #[cfg(windows)]
    {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;
        let native = path
            .native
            .iter()
            .map(|unit| unit.to_native())
            .collect::<Vec<_>>();
        SharedPath::from(PathBuf::from(OsString::from_wide(&native)))
    }
}

fn category_code(category: DiagnosticCategory) -> u8 {
    match category {
        DiagnosticCategory::Framing => 0,
        DiagnosticCategory::Encoding => 1,
        DiagnosticCategory::Syntax => 2,
        DiagnosticCategory::LogicalValidation => 3,
        DiagnosticCategory::ResourceLimit => 4,
        DiagnosticCategory::Io => 5,
    }
}

fn category_from_code(code: u8) -> Option<DiagnosticCategory> {
    Some(match code {
        0 => DiagnosticCategory::Framing,
        1 => DiagnosticCategory::Encoding,
        2 => DiagnosticCategory::Syntax,
        3 => DiagnosticCategory::LogicalValidation,
        4 => DiagnosticCategory::ResourceLimit,
        5 => DiagnosticCategory::Io,
        _ => return None,
    })
}

#[derive(Archive, ArchiveSerialize)]
struct StoredDiagnostic {
    file: u32,
    collection: u32,
    source_id: String,
    source_version: u32,
    record: Option<u32>,
    range_start: Option<u64>,
    range_end: Option<u64>,
    category: u8,
    code: String,
    field_path: Option<Vec<String>>,
    byte_offset: Option<u64>,
    line: Option<u64>,
    column: Option<u64>,
    message: String,
}

impl StoredDiagnostic {
    fn from_source(diagnostic: &SourceDiagnostic) -> Self {
        Self {
            file: diagnostic.file.0,
            collection: diagnostic.collection.0,
            source_id: diagnostic.source_id.clone(),
            source_version: diagnostic.source_version,
            record: diagnostic.record.map(|slot| slot.0),
            range_start: diagnostic.range.map(|range| range.start),
            range_end: diagnostic.range.map(|range| range.end),
            category: category_code(diagnostic.category),
            code: diagnostic.code.clone(),
            field_path: diagnostic.field_path.clone(),
            byte_offset: diagnostic.byte_offset,
            line: diagnostic.line,
            column: diagnostic.column,
            message: diagnostic.message.clone(),
        }
    }
}

#[derive(Archive, ArchiveSerialize)]
struct StoredSegmentFile {
    id: u32,
    relative_path: StoredPath,
    modified_ns: i128,
    size: u64,
    created_ns: Option<i128>,
    collections: Vec<u32>,
    status: u8,
    diagnostics: Vec<StoredDiagnostic>,
    omitted_diagnostics: u32,
    records: Vec<u32>,
}

impl StoredSegmentFile {
    fn from_file(file: &SegmentFile) -> Self {
        Self {
            id: file.entry.id.0,
            relative_path: stored_path(&file.entry.relative_path),
            modified_ns: file.entry.revision.modified_ns,
            size: file.entry.revision.size,
            created_ns: file.entry.created_ns,
            collections: file
                .entry
                .collections
                .iter()
                .map(|collection| collection.0)
                .collect(),
            status: file.entry.status.code(),
            diagnostics: file
                .entry
                .diagnostics
                .iter()
                .map(StoredDiagnostic::from_source)
                .collect(),
            omitted_diagnostics: file.entry.omitted_diagnostics,
            records: file.records.iter().map(|slot| slot.0).collect(),
        }
    }
}

#[derive(Archive, ArchiveSerialize)]
struct StoredRecord {
    slot: u32,
    id: String,
    file: u32,
    range_start: u64,
    range_end: u64,
    modified_ns: i128,
    size: u64,
    class: u8,
    diagnostic_ref: u32,
}

impl StoredRecord {
    fn from_record(record: &RecordEntry) -> Self {
        Self {
            slot: record.slot.0,
            id: record.id.0.to_string(),
            file: record.file.0,
            range_start: record.range.start,
            range_end: record.range.end,
            modified_ns: record.revision.modified_ns,
            size: record.revision.size,
            class: record.class.code(),
            diagnostic_ref: record.diagnostic_ref.0,
        }
    }
}

#[derive(Archive, ArchiveSerialize)]
struct SegmentRowsArchive {
    files: Vec<StoredSegmentFile>,
    records: Vec<StoredRecord>,
}

/// Immutable archived rows with a lazy compatibility cache for callers whose
/// established API returns native references.
pub(crate) struct SegmentRows {
    archive: Arc<ValidatedArchive<SegmentRowsArchive>>,
    files: Arc<OnceLock<Vec<SegmentFile>>>,
    records: Arc<OnceLock<Vec<RecordEntry>>>,
}

impl Clone for SegmentRows {
    fn clone(&self) -> Self {
        Self {
            archive: Arc::clone(&self.archive),
            files: Arc::clone(&self.files),
            records: Arc::clone(&self.records),
        }
    }
}

impl fmt::Debug for SegmentRows {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SegmentRows")
            .field("files", &self.archive.root().files.len())
            .field("records", &self.archive.root().records.len())
            .field("archive_bytes", &self.archive.len())
            .finish()
    }
}

impl SegmentRows {
    pub(crate) fn freeze(files: Vec<SegmentFile>, records: Vec<RecordEntry>) -> Result<Self, String> {
        for (slot, record) in records.iter().enumerate() {
            if record.slot.0 as usize != slot
                || record.range.start > record.range.end
                || record.range.end > record.revision.size
            {
                return Err("segment record row is not densely valid".into());
            }
        }
        let owner = Self {
            archive: ValidatedArchive::freeze(&SegmentRowsArchive {
                files: files.iter().map(StoredSegmentFile::from_file).collect(),
                records: records.iter().map(StoredRecord::from_record).collect(),
            })?,
            files: Arc::new(OnceLock::new()),
            records: Arc::new(OnceLock::new()),
        };
        let file_ids = owner
            .archive
            .root()
            .files
            .iter()
            .map(|file| file.id.to_native())
            .collect::<std::collections::HashSet<_>>();
        if file_ids.len() != owner.archive.root().files.len() {
            return Err("segment file ownership contains duplicate file IDs".into());
        }
        for record in owner.archive.root().records.iter() {
            if !file_ids.contains(&record.file.to_native()) {
                return Err("segment record references a missing file row".into());
            }
        }
        owner.files();
        owner.records();
        Ok(owner)
    }

    pub(crate) fn files(&self) -> &[SegmentFile] {
        self.files.get_or_init(|| {
            self.archive
                .root()
                .files
                .iter()
                .map(|file| {
                    let status = match file.status {
                        0 => FileStatus::Valid,
                        1 => FileStatus::Broken,
                        2 => FileStatus::Unreadable,
                        _ => unreachable!("validated segment file status"),
                    };
                    SegmentFile {
                        entry: FileEntry {
                            id: FileId(file.id.to_native()),
                            relative_path: archived_native_path(&file.relative_path),
                            revision: SourceRevision {
                                modified_ns: file.modified_ns.to_native(),
                                size: file.size.to_native(),
                            },
                            created_ns: file
                                .created_ns
                                .as_ref()
                                .map(|value| value.to_native()),
                            collections: file
                                .collections
                                .iter()
                                .map(|collection| CollectionId(collection.to_native()))
                                .collect(),
                            status,
                            diagnostics: file
                                .diagnostics
                                .iter()
                                .map(|diagnostic| {
                                    let range = match (
                                        diagnostic.range_start.as_ref(),
                                        diagnostic.range_end.as_ref(),
                                    ) {
                                        (Some(start), Some(end)) => Some(ByteRange {
                                            start: start.to_native(),
                                            end: end.to_native(),
                                        }),
                                        (None, None) => None,
                                        _ => unreachable!("validated segment diagnostic range"),
                                    };
                                    SourceDiagnostic {
                                        file: FileId(diagnostic.file.to_native()),
                                        collection: CollectionId(
                                            diagnostic.collection.to_native(),
                                        ),
                                        source_id: diagnostic.source_id.as_str().into(),
                                        source_version: diagnostic.source_version.to_native(),
                                        record: diagnostic
                                            .record
                                            .as_ref()
                                            .map(|slot| RecordSlot(slot.to_native())),
                                        range,
                                        category: category_from_code(diagnostic.category)
                                        .expect("validated segment diagnostic category"),
                                        code: diagnostic.code.as_str().into(),
                                        field_path: diagnostic.field_path.as_ref().map(|path| {
                                            path.iter()
                                                .map(|part| part.as_str().to_owned())
                                                .collect()
                                        }),
                                        byte_offset: diagnostic
                                            .byte_offset
                                            .as_ref()
                                            .map(|value| value.to_native()),
                                        line: diagnostic
                                            .line
                                            .as_ref()
                                            .map(|value| value.to_native()),
                                        column: diagnostic
                                            .column
                                            .as_ref()
                                            .map(|value| value.to_native()),
                                        message: diagnostic.message.as_str().into(),
                                    }
                                })
                                .collect(),
                            omitted_diagnostics: file.omitted_diagnostics.to_native(),
                        },
                        records: file
                            .records
                            .iter()
                            .map(|slot| RecordSlot(slot.to_native()))
                            .collect(),
                    }
                })
                .collect()
        })
    }

    pub(crate) fn records(&self) -> &[RecordEntry] {
        self.records.get_or_init(|| {
            self.archive
                .root()
                .records
                .iter()
                .map(|record| {
                    let class = match record.class {
                        0 => RecordClass::Logical,
                        1 => RecordClass::RawFallback,
                        _ => unreachable!("validated segment record class"),
                    };
                    RecordEntry {
                        slot: RecordSlot(record.slot.to_native()),
                        id: RecordId(record.id.as_str().into()),
                        file: FileId(record.file.to_native()),
                        range: ByteRange {
                            start: record.range_start.to_native(),
                            end: record.range_end.to_native(),
                        },
                        revision: SourceRevision {
                            modified_ns: record.modified_ns.to_native(),
                            size: record.size.to_native(),
                        },
                        class,
                        diagnostic_ref: DiagnosticRef(record.diagnostic_ref.to_native()),
                    }
                })
                .collect()
        })
    }

    pub(crate) fn views(self) -> (SegmentFiles, RecordRows) {
        (
            SegmentFiles {
                archive: Arc::clone(&self.archive),
                cache: self.files,
            },
            RecordRows {
                archive: self.archive,
                cache: self.records,
            },
        )
    }
}

impl Deref for SegmentRows {
    type Target = [RecordEntry];

    fn deref(&self) -> &Self::Target {
        self.records()
    }
}

pub(crate) struct SegmentFiles {
    archive: Arc<ValidatedArchive<SegmentRowsArchive>>,
    cache: Arc<OnceLock<Vec<SegmentFile>>>,
}

impl Clone for SegmentFiles {
    fn clone(&self) -> Self {
        Self {
            archive: Arc::clone(&self.archive),
            cache: Arc::clone(&self.cache),
        }
    }
}

impl SegmentFiles {
    fn values(&self) -> &[SegmentFile] {
        self.cache
            .get()
            .expect("segment file compatibility cache initialized at freeze")
    }

    pub(crate) fn archive_bytes(&self) -> usize {
        self.archive.len()
    }
}

impl Deref for SegmentFiles {
    type Target = [SegmentFile];

    fn deref(&self) -> &Self::Target {
        self.values()
    }
}

impl<'a> IntoIterator for &'a SegmentFiles {
    type Item = &'a SegmentFile;
    type IntoIter = std::slice::Iter<'a, SegmentFile>;

    fn into_iter(self) -> Self::IntoIter {
        self.values().iter()
    }
}

pub(crate) struct RecordRows {
    archive: Arc<ValidatedArchive<SegmentRowsArchive>>,
    cache: Arc<OnceLock<Vec<RecordEntry>>>,
}

impl Clone for RecordRows {
    fn clone(&self) -> Self {
        Self {
            archive: Arc::clone(&self.archive),
            cache: Arc::clone(&self.cache),
        }
    }
}

impl RecordRows {
    fn values(&self) -> &[RecordEntry] {
        self.cache
            .get()
            .expect("segment record compatibility cache initialized at freeze")
    }

    pub(crate) fn archive_bytes(&self) -> usize {
        self.archive.len()
    }
}

impl Deref for RecordRows {
    type Target = [RecordEntry];

    fn deref(&self) -> &Self::Target {
        self.values()
    }
}

impl<'a> IntoIterator for &'a RecordRows {
    type Item = &'a RecordEntry;
    type IntoIter = std::slice::Iter<'a, RecordEntry>;

    fn into_iter(self) -> Self::IntoIter {
        self.values().iter()
    }
}

#[derive(Archive, ArchiveSerialize)]
struct FileLookupArchive {
    values: HashMap<u32, u32>,
}

/// Archived equality-only file-ID to local file-row lookup.
#[derive(Clone)]
pub(crate) struct FileLookup {
    archive: Arc<ValidatedArchive<FileLookupArchive>>,
}

impl fmt::Debug for FileLookup {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FileLookup")
            .field("len", &self.archive.root().values.len())
            .finish()
    }
}

impl FileLookup {
    pub(crate) fn freeze(values: HashMap<FileId, usize>) -> Result<Self, String> {
        let values = values
            .into_iter()
            .map(|(id, ordinal)| {
                Ok((
                    id.0,
                    u32::try_from(ordinal)
                        .map_err(|_| "segment file ordinal exceeds u32")?,
                ))
            })
            .collect::<Result<HashMap<_, _>, String>>()?;
        Ok(Self {
            archive: ValidatedArchive::freeze(&FileLookupArchive { values })?,
        })
    }

    pub(crate) fn get(&self, id: FileId) -> Option<usize> {
        let key = rkyv::rend::u32_le::from_native(id.0);
        self.archive
            .root()
            .values
            .get(&key)
            .map(|ordinal| ordinal.to_native() as usize)
    }

    pub(crate) fn len(&self) -> usize {
        self.archive.root().values.len()
    }

    pub(crate) fn archive_bytes(&self) -> usize {
        self.archive.len()
    }
}

#[derive(Archive, ArchiveSerialize)]
struct RecordLookupArchive {
    values: HashMap<String, u32>,
}

/// Archived equality-only logical record-ID to local-slot lookup.
#[derive(Clone)]
pub(crate) struct RecordLookup {
    archive: Arc<ValidatedArchive<RecordLookupArchive>>,
}

impl fmt::Debug for RecordLookup {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RecordLookup")
            .field("len", &self.archive.root().values.len())
            .finish()
    }
}

impl RecordLookup {
    pub(crate) fn freeze(values: HashMap<RecordId, RecordSlot>) -> Result<Self, String> {
        let values = values
            .into_iter()
            .map(|(id, slot)| (id.0.to_string(), slot.0))
            .collect();
        Ok(Self {
            archive: ValidatedArchive::freeze(&RecordLookupArchive { values })?,
        })
    }

    pub(crate) fn get(&self, id: &str) -> Option<RecordSlot> {
        self.archive
            .root()
            .values
            .get::<str>(id)
            .map(|slot| RecordSlot(slot.to_native()))
    }

    #[cfg(test)]
    pub(crate) fn contains_key(&self, id: &str) -> bool {
        self.get(id).is_some()
    }

    pub(crate) fn for_each_id(&self, mut visit: impl FnMut(&str)) {
        for id in self.archive.root().values.keys() {
            visit(id.as_str());
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.archive.root().values.len()
    }

    pub(crate) fn archive_bytes(&self) -> usize {
        self.archive.len()
    }
}

impl PartialEq for RecordLookup {
    fn eq(&self, other: &Self) -> bool {
        if self.len() != other.len() {
            return false;
        }
        let mut equal = true;
        self.for_each_id(|id| {
            equal &= self.get(id) == other.get(id);
        });
        equal
    }
}

impl Eq for RecordLookup {}
