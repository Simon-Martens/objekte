//! Archived segment record, file-ownership, and identity lookup storage.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::ops::Deref;
use std::sync::{Arc, OnceLock};

use rkyv::collections::btree_map::{ArchivedBTreeMap, BTreeMapResolver};
use rkyv::place::Place;
use rkyv::string::{ArchivedString, StringResolver};
use rkyv::{Archive, Archived, Serialize as ArchiveSerialize};

use crate::archive_storage::ValidatedArchive;
use crate::archived_path::StoredPath;
use crate::inventory::{
    ByteRange, DiagnosticRef, FileEntry, FileStatus, RecordClass, RecordEntry, SharedPath,
    SourceDiagnostic, SourceRevision,
};
use crate::model::{CollectionId, FileId, RecordId, RecordSlot};
use crate::native_segment::SegmentFile;
use crate::source::DiagnosticCategory;

fn archived_native_path(path: &rkyv::Archived<StoredPath>) -> SharedPath {
    crate::archived_path::to_shared(path).expect("validated archived path")
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
    fn from_source(diagnostic: SourceDiagnostic) -> Self {
        Self {
            file: diagnostic.file.0,
            collection: diagnostic.collection.0,
            source_id: diagnostic.source_id.to_string(),
            source_version: diagnostic.source_version,
            record: diagnostic.record.map(|slot| slot.0),
            range_start: diagnostic.range.map(|range| range.start),
            range_end: diagnostic.range.map(|range| range.end),
            category: category_code(diagnostic.category),
            code: diagnostic.code.to_string(),
            field_path: diagnostic.field_path,
            byte_offset: diagnostic.byte_offset,
            line: diagnostic.line,
            column: diagnostic.column,
            message: diagnostic.message.to_string(),
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
    fn from_file(file: SegmentFile) -> Self {
        let SegmentFile { entry, records } = file;
        let relative_path = StoredPath::from_shared(&entry.relative_path);
        Self {
            id: entry.id.0,
            relative_path,
            modified_ns: entry.revision.modified_ns,
            size: entry.revision.size,
            created_ns: entry.created_ns,
            collections: entry
                .collections
                .iter()
                .map(|collection| collection.0)
                .collect(),
            status: entry.status.code(),
            diagnostics: entry
                .diagnostics
                .into_iter()
                .map(StoredDiagnostic::from_source)
                .collect(),
            omitted_diagnostics: entry.omitted_diagnostics,
            records: records.into_iter().map(|slot| slot.0).collect(),
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
    fn from_record(record: RecordEntry) -> Self {
        let id = Arc::try_unwrap(record.id.0)
            .unwrap_or_else(|shared| shared.as_str().to_owned());
        Self {
            slot: record.slot.0,
            id,
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
        let source = SegmentRowsArchive {
            files: files.into_iter().map(StoredSegmentFile::from_file).collect(),
            records: records.into_iter().map(StoredRecord::from_record).collect(),
        };
        let owner = Self {
            archive: ValidatedArchive::freeze(&source)?,
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
        Ok(owner)
    }

    #[allow(dead_code)]
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
                        id: RecordId(Arc::new(record.id.as_str().to_owned())),
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
        self.cache.get_or_init(|| {
            crate::storage_usage::StorageWorkCounters::update(|counters| {
                counters.metadata_cache_initializations =
                    counters.metadata_cache_initializations.saturating_add(1);
                counters.file_row_cache_initializations = counters
                    .file_row_cache_initializations
                    .saturating_add(1);
            });
            self.archive
                .root()
                .files
                .iter()
                .map(archived_segment_file)
                .collect()
        })
    }

    pub(crate) fn archive_bytes(&self) -> usize {
        self.archive.len()
    }

    pub(crate) fn capacity(&self) -> usize {
        self.archive.capacity()
    }

    pub(crate) fn archive_owner_id(&self) -> usize {
        Arc::as_ptr(&self.archive) as usize
    }

    pub(crate) fn len(&self) -> usize {
        self.archive.root().files.len()
    }

    #[allow(dead_code)]
    pub(crate) fn is_empty(&self) -> bool {
        self.archive.root().files.is_empty()
    }

    pub(crate) fn get(&self, index: usize) -> Option<&SegmentFile> {
        self.values().get(index)
    }

    pub(crate) fn cache_initialized(&self) -> bool {
        self.cache.get().is_some()
    }

    pub(crate) fn file_id(&self, index: usize) -> Option<FileId> {
        self.archive
            .root()
            .files
            .get(index)
            .map(|file| FileId(file.id.to_native()))
    }

    pub(crate) fn file_ids(&self) -> impl Iterator<Item = FileId> + '_ {
        self.archive
            .root()
            .files
            .iter()
            .map(|file| FileId(file.id.to_native()))
    }

    pub(crate) fn archived_record_slots(
        &self,
        index: usize,
    ) -> Option<impl Iterator<Item = RecordSlot> + '_> {
        self.archive.root().files.get(index).map(|file| {
            file.records
                .iter()
                .map(|slot| RecordSlot(slot.to_native()))
        })
    }

    pub(crate) fn source_bytes(&self) -> u64 {
        self.archive
            .root()
            .files
            .iter()
            .map(|file| file.size.to_native())
            .fold(0u64, u64::saturating_add)
    }

    pub(crate) fn revision(&self, index: usize) -> Option<SourceRevision> {
        self.archive.root().files.get(index).map(|file| SourceRevision {
            modified_ns: file.modified_ns.to_native(),
            size: file.size.to_native(),
        })
    }

    pub(crate) fn created_ns(&self, index: usize) -> Option<Option<i128>> {
        self.archive
            .root()
            .files
            .get(index)
            .map(|file| file.created_ns.as_ref().map(|value| value.to_native()))
    }

    pub(crate) fn archived_path(
        &self,
        index: usize,
    ) -> Option<&rkyv::Archived<crate::archived_path::StoredPath>> {
        self.archive
            .root()
            .files
            .get(index)
            .map(|file| &file.relative_path)
    }

    pub(crate) fn status_code(&self, index: usize) -> Option<u8> {
        self.archive
            .root()
            .files
            .get(index)
            .map(|file| file.status)
    }

    pub(crate) fn diagnostic_count(&self, index: usize) -> Option<usize> {
        self.archive
            .root()
            .files
            .get(index)
            .map(|file| file.diagnostics.len())
    }

    pub(crate) fn omitted_diagnostics(&self, index: usize) -> Option<u32> {
        self.archive
            .root()
            .files
            .get(index)
            .map(|file| file.omitted_diagnostics.to_native())
    }

    pub(crate) fn diagnostic(
        &self,
        index: usize,
        diagnostic: usize,
    ) -> Option<SourceDiagnostic> {
        let row = self.archive.root().files.get(index)?;
        let diagnostic = row.diagnostics.get(diagnostic)?;
        let decoded = StoredDiagnostic {
            file: diagnostic.file.to_native(),
            collection: diagnostic.collection.to_native(),
            source_id: diagnostic.source_id.as_str().into(),
            source_version: diagnostic.source_version.to_native(),
            record: diagnostic.record.as_ref().map(|slot| slot.to_native()),
            range_start: diagnostic
                .range_start
                .as_ref()
                .map(|value| value.to_native()),
            range_end: diagnostic
                .range_end
                .as_ref()
                .map(|value| value.to_native()),
            category: diagnostic.category,
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
            line: diagnostic.line.as_ref().map(|value| value.to_native()),
            column: diagnostic.column.as_ref().map(|value| value.to_native()),
            message: diagnostic.message.as_str().into(),
        };
        let range = match (decoded.range_start, decoded.range_end) {
            (Some(start), Some(end)) => Some(ByteRange { start, end }),
            (None, None) => None,
            _ => return None,
        };
        Some(SourceDiagnostic {
            file: FileId(decoded.file),
            collection: CollectionId(decoded.collection),
            source_id: decoded.source_id.to_string(),
            source_version: decoded.source_version,
            record: decoded.record.map(RecordSlot),
            range,
            category: category_from_code(decoded.category)?,
            code: decoded.code.to_string(),
            field_path: decoded.field_path,
            byte_offset: decoded.byte_offset,
            line: decoded.line,
            column: decoded.column,
            message: decoded.message.to_string(),
        })
    }

    pub(crate) fn merge_file_diagnostics(
        &self,
        index: usize,
        target: &mut FileEntry,
    ) -> Result<(), String> {
        let file = self
            .archive
            .root()
            .files
            .get(index)
            .map(archived_segment_file)
            .ok_or("segment file row missing")?;
        target.status = target.status.max(file.entry.status);
        for diagnostic in file.entry.diagnostics {
            target.record_diagnostic(diagnostic)?;
        }
        target.omitted_diagnostics = target
            .omitted_diagnostics
            .saturating_add(file.entry.omitted_diagnostics);
        Ok(())
    }

    pub(crate) fn compatibility_cache_bytes(&self) -> usize {
        let Some(files) = self.cache.get() else {
            return 0;
        };
        files
            .iter()
            .map(|file| {
                std::mem::size_of::<SegmentFile>()
                    + file.entry.relative_path.as_path().as_os_str().len()
                    + file.records.capacity() * std::mem::size_of::<RecordSlot>()
                    + file.entry.collections.capacity()
                        * std::mem::size_of::<CollectionId>()
                    + file.entry.diagnostics.capacity()
                        * std::mem::size_of::<SourceDiagnostic>()
            })
            .fold(0usize, usize::saturating_add)
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

pub(crate) struct ArchivedRecordRowRef<'a> {
    record: &'a rkyv::Archived<StoredRecord>,
}

impl ArchivedRecordRowRef<'_> {
    pub(crate) fn id(&self) -> &str {
        self.record.id.as_str()
    }

    pub(crate) fn file(&self) -> FileId {
        FileId(self.record.file.to_native())
    }

    pub(crate) fn range(&self) -> ByteRange {
        ByteRange {
            start: self.record.range_start.to_native(),
            end: self.record.range_end.to_native(),
        }
    }

    pub(crate) fn class(&self) -> RecordClass {
        match self.record.class {
            0 => RecordClass::Logical,
            1 => RecordClass::RawFallback,
            _ => unreachable!("validated segment record class"),
        }
    }

    pub(crate) fn diagnostic_ref(&self) -> DiagnosticRef {
        DiagnosticRef(self.record.diagnostic_ref.to_native())
    }
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
        self.cache.get_or_init(|| {
            crate::storage_usage::StorageWorkCounters::update(|counters| {
                counters.metadata_cache_initializations =
                    counters.metadata_cache_initializations.saturating_add(1);
                counters.record_row_cache_initializations = counters
                    .record_row_cache_initializations
                    .saturating_add(1);
            });
            self.archive
                .root()
                .records
                .iter()
                .map(archived_record)
                .collect()
        })
    }

    pub(crate) fn len(&self) -> usize {
        self.archive.root().records.len()
    }

    #[allow(dead_code)]
    pub(crate) fn is_empty(&self) -> bool {
        self.archive.root().records.is_empty()
    }

    pub(crate) fn get(&self, index: usize) -> Option<&RecordEntry> {
        self.values().get(index)
    }

    pub(crate) fn archived_record(
        &self,
        index: usize,
    ) -> Option<ArchivedRecordRowRef<'_>> {
        self.archive
            .root()
            .records
            .get(index)
            .map(|record| ArchivedRecordRowRef { record })
    }

    pub(crate) fn cache_initialized(&self) -> bool {
        self.cache.get().is_some()
    }

    pub(crate) fn record_id(&self, slot: RecordSlot) -> Option<&str> {
        self.archive
            .root()
            .records
            .get(slot.0 as usize)
            .map(|record| record.id.as_str())
    }

    pub(crate) fn max_record_range_len(&self) -> u64 {
        self.archive
            .root()
            .records
            .iter()
            .map(|record| {
                record
                    .range_end
                    .to_native()
                    .saturating_sub(record.range_start.to_native())
            })
            .max()
            .unwrap_or(0)
    }

    pub(crate) fn compatibility_cache_bytes(&self) -> usize {
        let Some(records) = self.cache.get() else {
            return 0;
        };
        records
            .iter()
            .map(|record| {
                std::mem::size_of::<RecordEntry>() + record.id.0.len()
            })
            .fold(0usize, usize::saturating_add)
    }
}

fn archived_segment_file(file: &rkyv::Archived<StoredSegmentFile>) -> SegmentFile {
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
            created_ns: file.created_ns.as_ref().map(|value| value.to_native()),
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
                        collection: CollectionId(diagnostic.collection.to_native()),
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
                        line: diagnostic.line.as_ref().map(|value| value.to_native()),
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
}

fn archived_record(record: &rkyv::Archived<StoredRecord>) -> RecordEntry {
    let class = match record.class {
        0 => RecordClass::Logical,
        1 => RecordClass::RawFallback,
        _ => unreachable!("validated segment record class"),
    };
    RecordEntry {
        slot: RecordSlot(record.slot.to_native()),
            id: RecordId(Arc::new(record.id.as_str().to_owned())),
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

    pub(crate) fn capacity(&self) -> usize {
        self.archive.capacity()
    }

    pub(crate) fn archive_owner_id(&self) -> usize {
        Arc::as_ptr(&self.archive) as usize
    }
}

#[derive(Archive, ArchiveSerialize)]
struct RecordLookupArchive {
    values: BTreeMap<String, u32>,
}

struct RecordLookupSource<'a> {
    entries: &'a [(&'a str, u32)],
}

#[derive(Clone, Copy)]
struct RecordLookupKey<'a>(&'a str);

impl Archive for RecordLookupKey<'_> {
    type Archived = ArchivedString;
    type Resolver = StringResolver;

    fn resolve(&self, resolver: Self::Resolver, out: Place<Self::Archived>) {
        ArchivedString::resolve_from_str(self.0, resolver, out);
    }
}

impl<S> ArchiveSerialize<S> for RecordLookupKey<'_>
where
    S: rkyv::rancor::Fallible + rkyv::ser::Writer + ?Sized,
    S::Error: rkyv::rancor::Source,
{
    fn serialize(&self, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        ArchivedString::serialize_from_str(self.0, serializer)
    }
}

struct RecordLookupSourceResolver {
    values: BTreeMapResolver,
}

impl Archive for RecordLookupSource<'_> {
    type Archived = Archived<RecordLookupArchive>;
    type Resolver = RecordLookupSourceResolver;

    fn resolve(&self, resolver: Self::Resolver, out: Place<Self::Archived>) {
        // SAFETY: The source resolves the one map field of
        // `RecordLookupArchive` into its exact generated archived type.
        unsafe {
            let root = out.ptr();
            let values = Place::from_field_unchecked(
                out,
                std::ptr::addr_of_mut!((*root).values),
            );
            ArchivedBTreeMap::resolve_from_len(
                self.entries.len(),
                resolver.values,
                values,
            );
        }
    }
}

impl<S> ArchiveSerialize<S> for RecordLookupSource<'_>
where
    S: rkyv::rancor::Fallible
        + rkyv::ser::Allocator
        + rkyv::ser::Writer
        + ?Sized,
    S::Error: rkyv::rancor::Source,
{
    fn serialize(&self, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        let values = ArchivedBTreeMap::<ArchivedString, Archived<u32>, 5>::
            serialize_from_ordered_iter::<_, RecordLookupKey<'_>, u32, RecordLookupKey<'_>, u32, S>(
                self.entries
                    .iter()
                    .map(|(id, slot)| (RecordLookupKey(id), *slot)),
                serializer,
            )?;
        Ok(RecordLookupSourceResolver { values })
    }
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
        let mut entries = values
            .iter()
            .map(|(id, slot)| (id.0.as_str(), slot.0))
            .collect::<Vec<_>>();
        entries.sort_unstable_by(|left, right| left.0.cmp(right.0));
        if entries
            .windows(2)
            .any(|window| window[0].0 == window[1].0)
        {
            return Err("duplicate segment record ID lookup key".into());
        }
        let source = RecordLookupSource {
            entries: &entries,
        };
        Ok(Self {
            archive: ValidatedArchive::freeze_source(&source)?,
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

    pub(crate) fn capacity(&self) -> usize {
        self.archive.capacity()
    }

    pub(crate) fn archive_owner_id(&self) -> usize {
        Arc::as_ptr(&self.archive) as usize
    }
}

pub(crate) fn validate_segment_components(
    files: &SegmentFiles,
    records: &RecordRows,
    file_index: &FileLookup,
    id_lookup: &RecordLookup,
    collection: CollectionId,
) -> Result<(), String> {
    if !Arc::ptr_eq(&files.archive, &records.archive) {
        return Err("segment file and record rows do not share one archive owner".into());
    }
    let root = files.archive.root();
    if file_index.len() != root.files.len() {
        return Err("segment file lookup coverage mismatch".into());
    }
    let mut file_ids = std::collections::HashSet::with_capacity(root.files.len());
    for (ordinal, file) in root.files.iter().enumerate() {
        let id = FileId(file.id.to_native());
        if !file_ids.insert(id)
            || file_index.get(id) != Some(ordinal)
            || !file
                .collections
                .iter()
                .any(|candidate| candidate.to_native() == collection.0)
        {
            return Err("segment file row or lookup is inconsistent".into());
        }
        for slot in file.records.iter() {
            let slot = slot.to_native() as usize;
            let record = root
                .records
                .get(slot)
                .ok_or("segment file owns an out-of-bounds record slot")?;
            if record.file.to_native() != file.id.to_native() {
                return Err("segment file and record ownership disagree".into());
            }
        }
    }
    if root.records.len() != id_lookup.len() {
        return Err("segment record lookup coverage mismatch".into());
    }
    let mut ids = std::collections::HashSet::with_capacity(root.records.len());
    for (slot, record) in root.records.iter().enumerate() {
        if record.slot.to_native() as usize != slot
            || record.range_start.to_native() > record.range_end.to_native()
            || record.range_end.to_native() > record.size.to_native()
            || !file_ids.contains(&FileId(record.file.to_native()))
            || !ids.insert(record.id.as_str())
            || id_lookup.get(record.id.as_str()) != Some(RecordSlot(slot as u32))
        {
            return Err("segment record row or identity lookup is inconsistent".into());
        }
    }
    Ok(())
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
