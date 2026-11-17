//! Archived segment record, file-ownership, and identity lookup storage.

use std::collections::HashMap;
use std::fmt;
use std::ops::Deref;
use std::sync::{Arc, OnceLock};

use rkyv::collections::swiss_table::{ArchivedHashMap, HashMapResolver};
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

    fn from_archived(diagnostic: &rkyv::Archived<Self>) -> Self {
        Self {
            file: diagnostic.file.to_native(),
            collection: diagnostic.collection.to_native(),
            source_id: diagnostic.source_id.as_str().into(),
            source_version: diagnostic.source_version.to_native(),
            record: diagnostic.record.as_ref().map(|slot| slot.to_native()),
            range_start: diagnostic
                .range_start
                .as_ref()
                .map(|value| value.to_native()),
            range_end: diagnostic.range_end.as_ref().map(|value| value.to_native()),
            category: diagnostic.category,
            code: diagnostic.code.as_str().into(),
            field_path: diagnostic
                .field_path
                .as_ref()
                .map(|path| path.iter().map(|part| part.as_str().to_owned()).collect()),
            byte_offset: diagnostic
                .byte_offset
                .as_ref()
                .map(|value| value.to_native()),
            line: diagnostic.line.as_ref().map(|value| value.to_native()),
            column: diagnostic.column.as_ref().map(|value| value.to_native()),
            message: diagnostic.message.as_str().into(),
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
        let relative_path = StoredPath::from_shared(&file.entry.relative_path);
        Self {
            id: file.entry.id.0,
            relative_path,
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
                .cloned()
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
            id: record.id.0.as_ref().to_owned(),
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

enum SegmentRowsBacking {
    Native {
        files: Vec<SegmentFile>,
        records: Vec<RecordEntry>,
    },
    Archived {
        archive: Arc<ValidatedArchive<SegmentRowsArchive>>,
        files: OnceLock<Vec<SegmentFile>>,
        records: OnceLock<Vec<RecordEntry>>,
    },
}

struct SegmentRowsOwner {
    backing: SegmentRowsBacking,
}

/// Immutable segment rows. Native publication retains the construction rows;
/// explicit archiving replaces them with one checked shared archive.
#[derive(Clone)]
pub(crate) struct SegmentRows {
    owner: Arc<SegmentRowsOwner>,
}

impl fmt::Debug for SegmentRows {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SegmentRows")
            .field("files", &self.files().len())
            .field("records", &self.records().len())
            .field("archive_bytes", &self.archive_bytes())
            .finish()
    }
}

impl SegmentRows {
    pub(crate) fn native(
        files: Vec<SegmentFile>,
        records: Vec<RecordEntry>,
    ) -> Result<Self, String> {
        for (slot, record) in records.iter().enumerate() {
            if record.slot.0 as usize != slot
                || record.range.start > record.range.end
                || record.range.end > record.revision.size
            {
                return Err("segment record row is not densely valid".into());
            }
        }
        let file_ids = files
            .iter()
            .map(|file| file.entry.id)
            .collect::<std::collections::HashSet<_>>();
        if file_ids.len() != files.len() {
            return Err("segment file ownership contains duplicate file IDs".into());
        }
        if records
            .iter()
            .any(|record| !file_ids.contains(&record.file))
        {
            return Err("segment record references a missing file row".into());
        }
        let owner = Arc::new(SegmentRowsOwner {
            backing: SegmentRowsBacking::Native { files, records },
        });
        Ok(Self { owner })
    }

    pub(crate) fn archive(&self) -> Result<Self, String> {
        if matches!(self.owner.backing, SegmentRowsBacking::Archived { .. }) {
            return Ok(self.clone());
        }
        let source = SegmentRowsArchive {
            files: self
                .files()
                .iter()
                .map(StoredSegmentFile::from_file)
                .collect(),
            records: self
                .records()
                .iter()
                .map(StoredRecord::from_record)
                .collect(),
        };
        let archive = ValidatedArchive::freeze(&source)?;
        let file_ids = archive
            .root()
            .files
            .iter()
            .map(|file| file.id.to_native())
            .collect::<std::collections::HashSet<_>>();
        if file_ids.len() != archive.root().files.len() {
            return Err("segment file ownership contains duplicate file IDs".into());
        }
        for record in archive.root().records.iter() {
            if !file_ids.contains(&record.file.to_native()) {
                return Err("segment record references a missing file row".into());
            }
        }
        let owner = Arc::new(SegmentRowsOwner {
            backing: SegmentRowsBacking::Archived {
                archive,
                files: OnceLock::new(),
                records: OnceLock::new(),
            },
        });
        Ok(Self { owner })
    }

    #[allow(dead_code)]
    pub(crate) fn files(&self) -> &[SegmentFile] {
        match &self.owner.backing {
            SegmentRowsBacking::Native { files, .. } => files,
            SegmentRowsBacking::Archived { archive, files, .. } => files.get_or_init(|| {
                crate::storage_usage::StorageWorkCounters::update(|counters| {
                    counters.metadata_cache_initializations =
                        counters.metadata_cache_initializations.saturating_add(1);
                    counters.file_row_cache_initializations =
                        counters.file_row_cache_initializations.saturating_add(1);
                });
                archive
                    .root()
                    .files
                    .iter()
                    .map(archived_segment_file)
                    .collect()
            }),
        }
    }

    pub(crate) fn records(&self) -> &[RecordEntry] {
        match &self.owner.backing {
            SegmentRowsBacking::Native { records, .. } => records,
            SegmentRowsBacking::Archived {
                archive, records, ..
            } => records.get_or_init(|| {
                crate::storage_usage::StorageWorkCounters::update(|counters| {
                    counters.metadata_cache_initializations =
                        counters.metadata_cache_initializations.saturating_add(1);
                    counters.record_row_cache_initializations =
                        counters.record_row_cache_initializations.saturating_add(1);
                });
                archive.root().records.iter().map(archived_record).collect()
            }),
        }
    }

    pub(crate) fn archive_bytes(&self) -> usize {
        match &self.owner.backing {
            SegmentRowsBacking::Native { .. } => 0,
            SegmentRowsBacking::Archived { archive, .. } => archive.len(),
        }
    }

    pub(crate) fn views(self) -> (SegmentFiles, RecordRows) {
        (
            SegmentFiles {
                owner: Arc::clone(&self.owner),
            },
            RecordRows { owner: self.owner },
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
    owner: Arc<SegmentRowsOwner>,
}

impl Clone for SegmentFiles {
    fn clone(&self) -> Self {
        Self {
            owner: Arc::clone(&self.owner),
        }
    }
}

impl SegmentFiles {
    fn values(&self) -> &[SegmentFile] {
        match &self.owner.backing {
            SegmentRowsBacking::Native { files, .. } => files,
            SegmentRowsBacking::Archived { archive, files, .. } => files.get_or_init(|| {
                crate::storage_usage::StorageWorkCounters::update(|counters| {
                    counters.metadata_cache_initializations =
                        counters.metadata_cache_initializations.saturating_add(1);
                    counters.file_row_cache_initializations =
                        counters.file_row_cache_initializations.saturating_add(1);
                });
                archive
                    .root()
                    .files
                    .iter()
                    .map(archived_segment_file)
                    .collect()
            }),
        }
    }

    pub(crate) fn archive_bytes(&self) -> usize {
        match &self.owner.backing {
            SegmentRowsBacking::Native { .. } => 0,
            SegmentRowsBacking::Archived { archive, .. } => archive.len(),
        }
    }

    pub(crate) fn capacity(&self) -> usize {
        match &self.owner.backing {
            SegmentRowsBacking::Native { .. } => 0,
            SegmentRowsBacking::Archived { archive, .. } => archive.capacity(),
        }
    }

    pub(crate) fn archive_owner_id(&self) -> usize {
        Arc::as_ptr(&self.owner) as usize
    }

    pub(crate) fn is_archived(&self) -> bool {
        matches!(self.owner.backing, SegmentRowsBacking::Archived { .. })
    }

    pub(crate) fn archive_rows(&self) -> Result<(Self, RecordRows), String> {
        let rows = SegmentRows {
            owner: Arc::clone(&self.owner),
        }
        .archive()?;
        Ok(rows.views())
    }

    pub(crate) fn len(&self) -> usize {
        match &self.owner.backing {
            SegmentRowsBacking::Native { files, .. } => files.len(),
            SegmentRowsBacking::Archived { archive, .. } => archive.root().files.len(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn get(&self, index: usize) -> Option<&SegmentFile> {
        self.values().get(index)
    }

    pub(crate) fn cache_initialized(&self) -> bool {
        match &self.owner.backing {
            SegmentRowsBacking::Native { .. } => false,
            SegmentRowsBacking::Archived { files, .. } => files.get().is_some(),
        }
    }

    pub(crate) fn file_id(&self, index: usize) -> Option<FileId> {
        self.values().get(index).map(|file| file.entry.id)
    }

    pub(crate) fn file_ids(&self) -> impl Iterator<Item = FileId> + '_ {
        self.values().iter().map(|file| file.entry.id)
    }

    pub(crate) fn archived_record_slots(
        &self,
        index: usize,
    ) -> Option<impl Iterator<Item = RecordSlot> + '_> {
        Some(self.values().get(index)?.records.iter().copied())
    }

    pub(crate) fn source_bytes(&self) -> u64 {
        self.values()
            .iter()
            .map(|file| file.entry.revision.size)
            .fold(0u64, u64::saturating_add)
    }

    pub(crate) fn revision(&self, index: usize) -> Option<SourceRevision> {
        self.values().get(index).map(|file| file.entry.revision)
    }

    pub(crate) fn created_ns(&self, index: usize) -> Option<Option<i128>> {
        self.values().get(index).map(|file| file.entry.created_ns)
    }

    pub(crate) fn native_path(&self, index: usize) -> Option<&std::path::Path> {
        self.values()
            .get(index)
            .map(|file| file.entry.relative_path.as_path())
    }

    pub(crate) fn status_code(&self, index: usize) -> Option<u8> {
        self.values()
            .get(index)
            .map(|file| file.entry.status.code())
    }

    pub(crate) fn diagnostic_count(&self, index: usize) -> Option<usize> {
        match &self.owner.backing {
            SegmentRowsBacking::Native { files, .. } => {
                files.get(index).map(|file| file.entry.diagnostics.len())
            }
            SegmentRowsBacking::Archived { archive, .. } => archive
                .root()
                .files
                .get(index)
                .map(|file| file.diagnostics.len()),
        }
    }

    pub(crate) fn omitted_diagnostics(&self, index: usize) -> Option<u32> {
        self.values()
            .get(index)
            .map(|file| file.entry.omitted_diagnostics)
    }

    pub(crate) fn diagnostic(&self, index: usize, diagnostic: usize) -> Option<SourceDiagnostic> {
        match &self.owner.backing {
            SegmentRowsBacking::Native { files, .. } => {
                files.get(index)?.entry.diagnostics.get(diagnostic).cloned()
            }
            SegmentRowsBacking::Archived { archive, .. } => {
                let row = archive.root().files.get(index)?;
                let stored = row.diagnostics.get(diagnostic)?;
                let decoded = StoredDiagnostic::from_archived(stored);
                let range = match (decoded.range_start, decoded.range_end) {
                    (Some(start), Some(end)) => Some(ByteRange { start, end }),
                    (None, None) => None,
                    _ => return None,
                };
                Some(SourceDiagnostic {
                    file: FileId(decoded.file),
                    collection: CollectionId(decoded.collection),
                    source_id: decoded.source_id,
                    source_version: decoded.source_version,
                    record: decoded.record.map(RecordSlot),
                    range,
                    category: category_from_code(decoded.category)?,
                    code: decoded.code,
                    field_path: decoded.field_path,
                    byte_offset: decoded.byte_offset,
                    line: decoded.line,
                    column: decoded.column,
                    message: decoded.message,
                })
            }
        }
    }

    pub(crate) fn merge_file_diagnostics(
        &self,
        index: usize,
        target: &mut FileEntry,
    ) -> Result<(), String> {
        let file = self.get(index).cloned().ok_or("segment file row missing")?;
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
        let files = match &self.owner.backing {
            SegmentRowsBacking::Native { .. } => return 0,
            SegmentRowsBacking::Archived { files, .. } => files.get(),
        };
        let Some(files) = files else { return 0 };
        files
            .iter()
            .map(|file| {
                std::mem::size_of::<SegmentFile>()
                    + file.entry.relative_path.as_path().as_os_str().len()
                    + file.records.capacity() * std::mem::size_of::<RecordSlot>()
                    + file.entry.collections.capacity() * std::mem::size_of::<CollectionId>()
                    + file.entry.diagnostics.capacity() * std::mem::size_of::<SourceDiagnostic>()
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
    owner: Arc<SegmentRowsOwner>,
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

pub(crate) enum SegmentRecordRowRef<'a> {
    Native(&'a RecordEntry),
    Archived(ArchivedRecordRowRef<'a>),
}

impl SegmentRecordRowRef<'_> {
    pub(crate) fn id(&self) -> &str {
        match self {
            Self::Native(record) => record.id.0.as_ref(),
            Self::Archived(record) => record.id(),
        }
    }

    pub(crate) fn file(&self) -> FileId {
        match self {
            Self::Native(record) => record.file,
            Self::Archived(record) => record.file(),
        }
    }

    pub(crate) fn range(&self) -> ByteRange {
        match self {
            Self::Native(record) => record.range,
            Self::Archived(record) => record.range(),
        }
    }

    pub(crate) fn class(&self) -> RecordClass {
        match self {
            Self::Native(record) => record.class,
            Self::Archived(record) => record.class(),
        }
    }

    pub(crate) fn diagnostic_ref(&self) -> DiagnosticRef {
        match self {
            Self::Native(record) => record.diagnostic_ref,
            Self::Archived(record) => record.diagnostic_ref(),
        }
    }
}

impl Clone for RecordRows {
    fn clone(&self) -> Self {
        Self {
            owner: Arc::clone(&self.owner),
        }
    }
}

impl RecordRows {
    fn values(&self) -> &[RecordEntry] {
        match &self.owner.backing {
            SegmentRowsBacking::Native { records, .. } => records,
            SegmentRowsBacking::Archived {
                archive, records, ..
            } => records.get_or_init(|| {
                crate::storage_usage::StorageWorkCounters::update(|counters| {
                    counters.metadata_cache_initializations =
                        counters.metadata_cache_initializations.saturating_add(1);
                    counters.record_row_cache_initializations =
                        counters.record_row_cache_initializations.saturating_add(1);
                });
                archive.root().records.iter().map(archived_record).collect()
            }),
        }
    }

    pub(crate) fn len(&self) -> usize {
        match &self.owner.backing {
            SegmentRowsBacking::Native { records, .. } => records.len(),
            SegmentRowsBacking::Archived { archive, .. } => archive.root().records.len(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn get(&self, index: usize) -> Option<&RecordEntry> {
        self.values().get(index)
    }

    pub(crate) fn record_row(&self, index: usize) -> Option<SegmentRecordRowRef<'_>> {
        match &self.owner.backing {
            SegmentRowsBacking::Native { records, .. } => {
                records.get(index).map(SegmentRecordRowRef::Native)
            }
            SegmentRowsBacking::Archived { archive, .. } => archive
                .root()
                .records
                .get(index)
                .map(|record| SegmentRecordRowRef::Archived(ArchivedRecordRowRef { record })),
        }
    }

    pub(crate) fn cache_initialized(&self) -> bool {
        match &self.owner.backing {
            SegmentRowsBacking::Native { .. } => false,
            SegmentRowsBacking::Archived { records, .. } => records.get().is_some(),
        }
    }

    pub(crate) fn record_id(&self, slot: RecordSlot) -> Option<&str> {
        match &self.owner.backing {
            SegmentRowsBacking::Native { records, .. } => records
                .get(slot.0 as usize)
                .map(|record| record.id.0.as_ref().as_str()),
            SegmentRowsBacking::Archived { archive, .. } => archive
                .root()
                .records
                .get(slot.0 as usize)
                .map(|record| record.id.as_str()),
        }
    }

    pub(crate) fn max_record_range_len(&self) -> u64 {
        match &self.owner.backing {
            SegmentRowsBacking::Native { records, .. } => records
                .iter()
                .map(|record| record.range.end.saturating_sub(record.range.start))
                .max()
                .unwrap_or(0),
            SegmentRowsBacking::Archived { archive, .. } => archive
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
                .unwrap_or(0),
        }
    }

    pub(crate) fn compatibility_cache_bytes(&self) -> usize {
        let records = match &self.owner.backing {
            SegmentRowsBacking::Native { .. } => return 0,
            SegmentRowsBacking::Archived { records, .. } => records.get(),
        };
        let Some(records) = records else { return 0 };
        records
            .iter()
            .map(|record| std::mem::size_of::<RecordEntry>() + record.id.0.len())
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
                        field_path: diagnostic
                            .field_path
                            .as_ref()
                            .map(|path| path.iter().map(|part| part.as_str().to_owned()).collect()),
                        byte_offset: diagnostic
                            .byte_offset
                            .as_ref()
                            .map(|value| value.to_native()),
                        line: diagnostic.line.as_ref().map(|value| value.to_native()),
                        column: diagnostic.column.as_ref().map(|value| value.to_native()),
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

struct FileLookupSource<'a> {
    values: &'a HashMap<u32, u32>,
}

struct OneHashMapResolver {
    values: HashMapResolver,
}

impl Archive for FileLookupSource<'_> {
    type Archived = Archived<FileLookupArchive>;
    type Resolver = OneHashMapResolver;

    fn resolve(&self, resolver: Self::Resolver, out: Place<Self::Archived>) {
        // SAFETY: The source resolves the single hash-map field into the
        // generated archived root's exact field.
        unsafe {
            let root = out.ptr();
            let values = Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).values));
            ArchivedHashMap::<rkyv::rend::u32_le, rkyv::rend::u32_le>::resolve_from_len(
                self.values.len(),
                (7, 8),
                resolver.values,
                values,
            );
        }
    }
}

impl<S> ArchiveSerialize<S> for FileLookupSource<'_>
where
    S: rkyv::rancor::Fallible + rkyv::ser::Allocator + rkyv::ser::Writer + ?Sized,
    S::Error: rkyv::rancor::Source,
{
    fn serialize(&self, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        Ok(OneHashMapResolver {
            values: ArchivedHashMap::<
                rkyv::rend::u32_le,
                rkyv::rend::u32_le,
                rkyv::hash::FxHasher64,
            >::serialize_from_iter::<_, &u32, &u32, u32, u32, S>(
                self.values.iter(),
                (7, 8),
                serializer,
            )?,
        })
    }
}

enum FileLookupBacking {
    Native(Arc<HashMap<u32, u32>>),
    Archived(Arc<ValidatedArchive<FileLookupArchive>>),
}

impl Clone for FileLookupBacking {
    fn clone(&self) -> Self {
        match self {
            Self::Native(values) => Self::Native(Arc::clone(values)),
            Self::Archived(archive) => Self::Archived(Arc::clone(archive)),
        }
    }
}

/// Equality-only file-ID to local file-row lookup.
#[derive(Clone)]
pub(crate) struct FileLookup {
    backing: FileLookupBacking,
}

impl fmt::Debug for FileLookup {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FileLookup")
            .field("len", &self.len())
            .finish()
    }
}

impl FileLookup {
    pub(crate) fn native(values: HashMap<FileId, usize>) -> Result<Self, String> {
        let values = values
            .into_iter()
            .map(|(id, ordinal)| {
                Ok((
                    id.0,
                    u32::try_from(ordinal).map_err(|_| "segment file ordinal exceeds u32")?,
                ))
            })
            .collect::<Result<HashMap<_, _>, String>>()?;
        Ok(Self {
            backing: FileLookupBacking::Native(Arc::new(values)),
        })
    }

    pub(crate) fn get(&self, id: FileId) -> Option<usize> {
        match &self.backing {
            FileLookupBacking::Native(values) => values.get(&id.0).map(|ordinal| *ordinal as usize),
            FileLookupBacking::Archived(archive) => {
                let key = rkyv::rend::u32_le::from_native(id.0);
                archive
                    .root()
                    .values
                    .get(&key)
                    .map(|ordinal| ordinal.to_native() as usize)
            }
        }
    }

    pub(crate) fn len(&self) -> usize {
        match &self.backing {
            FileLookupBacking::Native(values) => values.len(),
            FileLookupBacking::Archived(archive) => archive.root().values.len(),
        }
    }

    pub(crate) fn archive_bytes(&self) -> usize {
        match &self.backing {
            FileLookupBacking::Native(_) => 0,
            FileLookupBacking::Archived(archive) => archive.len(),
        }
    }

    pub(crate) fn capacity(&self) -> usize {
        match &self.backing {
            FileLookupBacking::Native(_) => 0,
            FileLookupBacking::Archived(archive) => archive.capacity(),
        }
    }

    pub(crate) fn archive_owner_id(&self) -> usize {
        match &self.backing {
            FileLookupBacking::Native(values) => Arc::as_ptr(values) as usize,
            FileLookupBacking::Archived(archive) => Arc::as_ptr(archive) as usize,
        }
    }

    pub(crate) fn archive(&self) -> Result<Self, String> {
        if matches!(self.backing, FileLookupBacking::Archived(_)) {
            return Ok(self.clone());
        }
        let FileLookupBacking::Native(values) = &self.backing else {
            unreachable!();
        };
        let archive = ValidatedArchive::freeze_source(&FileLookupSource { values })?;
        Ok(Self {
            backing: FileLookupBacking::Archived(archive),
        })
    }

    pub(crate) fn is_archived(&self) -> bool {
        matches!(self.backing, FileLookupBacking::Archived(_))
    }
}

#[derive(Archive, ArchiveSerialize)]
struct RecordLookupArchive {
    values: HashMap<String, u32>,
}

struct RecordLookupSource<'a> {
    values: &'a HashMap<Arc<str>, u32>,
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
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
    values: HashMapResolver,
}

impl Archive for RecordLookupSource<'_> {
    type Archived = Archived<RecordLookupArchive>;
    type Resolver = RecordLookupSourceResolver;

    fn resolve(&self, resolver: Self::Resolver, out: Place<Self::Archived>) {
        // SAFETY: The source resolves the one map field of
        // `RecordLookupArchive` into its exact generated archived type.
        unsafe {
            let root = out.ptr();
            let values = Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).values));
            ArchivedHashMap::<ArchivedString, rkyv::rend::u32_le>::resolve_from_len(
                self.values.len(),
                (7, 8),
                resolver.values,
                values,
            );
        }
    }
}

impl<S> ArchiveSerialize<S> for RecordLookupSource<'_>
where
    S: rkyv::rancor::Fallible + rkyv::ser::Allocator + rkyv::ser::Writer + ?Sized,
    S::Error: rkyv::rancor::Source,
{
    fn serialize(&self, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        let values = ArchivedHashMap::<ArchivedString, Archived<u32>>::serialize_from_iter::<
            _,
            RecordLookupKey<'_>,
            &u32,
            RecordLookupKey<'_>,
            u32,
            S,
        >(
            self.values
                .iter()
                .map(|(id, slot)| (RecordLookupKey(id.as_ref()), slot)),
            (7, 8),
            serializer,
        )?;
        Ok(RecordLookupSourceResolver { values })
    }
}

enum RecordLookupBacking {
    Native(Arc<HashMap<Arc<str>, u32>>),
    Archived(Arc<ValidatedArchive<RecordLookupArchive>>),
}

impl Clone for RecordLookupBacking {
    fn clone(&self) -> Self {
        match self {
            Self::Native(values) => Self::Native(Arc::clone(values)),
            Self::Archived(archive) => Self::Archived(Arc::clone(archive)),
        }
    }
}

/// Equality-only logical record-ID to local-slot lookup.
#[derive(Clone)]
pub(crate) struct RecordLookup {
    backing: RecordLookupBacking,
}

impl fmt::Debug for RecordLookup {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RecordLookup")
            .field("len", &self.len())
            .finish()
    }
}

impl RecordLookup {
    pub(crate) fn native(values: HashMap<RecordId, RecordSlot>) -> Result<Self, String> {
        let mut native = HashMap::with_capacity(values.len());
        for (id, slot) in values {
            if native
                .insert(Arc::<str>::from(id.0.as_ref().as_str()), slot.0)
                .is_some()
            {
                return Err("duplicate segment record ID lookup key".into());
            }
        }
        Ok(Self {
            backing: RecordLookupBacking::Native(Arc::new(native)),
        })
    }

    pub(crate) fn get(&self, id: &str) -> Option<RecordSlot> {
        match &self.backing {
            RecordLookupBacking::Native(values) => values.get(id).map(|slot| RecordSlot(*slot)),
            RecordLookupBacking::Archived(archive) => archive
                .root()
                .values
                .get::<str>(id)
                .map(|slot| RecordSlot(slot.to_native())),
        }
    }

    #[cfg(test)]
    pub(crate) fn contains_key(&self, id: &str) -> bool {
        self.get(id).is_some()
    }

    pub(crate) fn for_each_id(&self, mut visit: impl FnMut(&str)) {
        match &self.backing {
            RecordLookupBacking::Native(values) => {
                for id in values.keys() {
                    visit(id);
                }
            }
            RecordLookupBacking::Archived(archive) => {
                for id in archive.root().values.keys() {
                    visit(id.as_str());
                }
            }
        }
    }

    pub(crate) fn len(&self) -> usize {
        match &self.backing {
            RecordLookupBacking::Native(values) => values.len(),
            RecordLookupBacking::Archived(archive) => archive.root().values.len(),
        }
    }

    pub(crate) fn archive_bytes(&self) -> usize {
        match &self.backing {
            RecordLookupBacking::Native(_) => 0,
            RecordLookupBacking::Archived(archive) => archive.len(),
        }
    }

    pub(crate) fn capacity(&self) -> usize {
        match &self.backing {
            RecordLookupBacking::Native(_) => 0,
            RecordLookupBacking::Archived(archive) => archive.capacity(),
        }
    }

    pub(crate) fn archive_owner_id(&self) -> usize {
        match &self.backing {
            RecordLookupBacking::Native(values) => Arc::as_ptr(values) as usize,
            RecordLookupBacking::Archived(archive) => Arc::as_ptr(archive) as usize,
        }
    }

    pub(crate) fn archive(&self) -> Result<Self, String> {
        if matches!(self.backing, RecordLookupBacking::Archived(_)) {
            return Ok(self.clone());
        }
        let RecordLookupBacking::Native(values) = &self.backing else {
            unreachable!();
        };
        let archive = ValidatedArchive::freeze_source(&RecordLookupSource { values })?;
        Ok(Self {
            backing: RecordLookupBacking::Archived(archive),
        })
    }

    pub(crate) fn is_archived(&self) -> bool {
        matches!(self.backing, RecordLookupBacking::Archived(_))
    }
}

pub(crate) fn validate_segment_components(
    files: &SegmentFiles,
    records: &RecordRows,
    file_index: &FileLookup,
    id_lookup: &RecordLookup,
    collection: CollectionId,
) -> Result<(), String> {
    if !Arc::ptr_eq(&files.owner, &records.owner) {
        return Err("segment file and record rows do not share one owner".into());
    }
    match (&files.owner.backing, &records.owner.backing) {
        (
            SegmentRowsBacking::Native {
                files: file_rows, ..
            },
            SegmentRowsBacking::Native {
                records: record_rows,
                ..
            },
        ) => {
            if file_index.len() != file_rows.len() {
                return Err("segment file lookup coverage mismatch".into());
            }
            let mut file_ids = std::collections::HashSet::with_capacity(file_rows.len());
            for (ordinal, file) in file_rows.iter().enumerate() {
                let id = file.entry.id;
                if !file_ids.insert(id)
                    || file_index.get(id) != Some(ordinal)
                    || !file.entry.collections.contains(&collection)
                {
                    return Err("segment file row or lookup is inconsistent".into());
                }
                for slot in &file.records {
                    let record = record_rows
                        .get(slot.0 as usize)
                        .ok_or("segment file owns an out-of-bounds record slot")?;
                    if record.file != id {
                        return Err("segment file and record ownership disagree".into());
                    }
                }
            }
            if record_rows.len() != id_lookup.len() {
                return Err("segment record lookup coverage mismatch".into());
            }
            let mut ids = std::collections::HashSet::with_capacity(record_rows.len());
            for (slot, record) in record_rows.iter().enumerate() {
                if record.slot.0 as usize != slot
                    || record.range.start > record.range.end
                    || record.range.end > record.revision.size
                    || !file_ids.contains(&record.file)
                    || !ids.insert(Arc::clone(&record.id.0))
                    || id_lookup.get(&record.id.0) != Some(RecordSlot(slot as u32))
                {
                    return Err("segment record row or identity lookup is inconsistent".into());
                }
            }
        }
        (
            SegmentRowsBacking::Archived { archive, .. },
            SegmentRowsBacking::Archived {
                archive: record_archive,
                ..
            },
        ) if Arc::ptr_eq(archive, record_archive) => {
            let root = archive.root();
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
                    let record = root
                        .records
                        .get(slot.to_native() as usize)
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
                let id = Arc::<str>::from(record.id.as_str());
                if record.slot.to_native() as usize != slot
                    || record.range_start.to_native() > record.range_end.to_native()
                    || record.range_end.to_native() > record.size.to_native()
                    || !file_ids.contains(&FileId(record.file.to_native()))
                    || !ids.insert(Arc::clone(&id))
                    || id_lookup.get(&id) != Some(RecordSlot(slot as u32))
                {
                    return Err("segment record row or identity lookup is inconsistent".into());
                }
            }
        }
        _ => return Err("segment row representations do not share an owner".into()),
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
