//! Archived segment record, file-ownership, and identity lookup storage.

use std::collections::HashMap;
use std::fmt;
use std::ops::Deref;
use std::sync::{Arc, OnceLock};

use rkyv::collections::swiss_table::{ArchivedHashMap, HashMapResolver};
use rkyv::place::Place;
use rkyv::string::{ArchivedString, StringResolver};
use rkyv::vec::VecResolver;
use rkyv::{Archive, Archived, Serialize as ArchiveSerialize};

use crate::archive_storage::ValidatedArchive;
use crate::archived_path::{BorrowedStoredPath, BorrowedStoredPathResolver, StoredPath};
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

#[derive(Archive, ArchiveSerialize)]
struct SegmentRowsArchive {
    files: Vec<StoredSegmentFile>,
    records: Vec<StoredRecord>,
}

struct BorrowedString<'a>(&'a str);

impl Archive for BorrowedString<'_> {
    type Archived = ArchivedString;
    type Resolver = StringResolver;

    fn resolve(&self, resolver: Self::Resolver, out: Place<Self::Archived>) {
        ArchivedString::resolve_from_str(self.0, resolver, out);
    }
}

impl<S> ArchiveSerialize<S> for BorrowedString<'_>
where
    S: rkyv::rancor::Fallible + rkyv::ser::Writer + ?Sized,
    S::Error: rkyv::rancor::Source,
{
    fn serialize(&self, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        ArchivedString::serialize_from_str(self.0, serializer)
    }
}

struct BorrowedStringSlice<'a>(&'a [String]);

impl Archive for BorrowedStringSlice<'_> {
    type Archived = rkyv::vec::ArchivedVec<ArchivedString>;
    type Resolver = VecResolver;

    fn resolve(&self, resolver: Self::Resolver, out: Place<Self::Archived>) {
        rkyv::vec::ArchivedVec::resolve_from_len(self.0.len(), resolver, out);
    }
}

impl<S> ArchiveSerialize<S> for BorrowedStringSlice<'_>
where
    S: rkyv::rancor::Fallible + rkyv::ser::Allocator + rkyv::ser::Writer + ?Sized,
    S::Error: rkyv::rancor::Source,
{
    fn serialize(&self, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        rkyv::vec::ArchivedVec::serialize_from_iter(
            self.0.iter().map(|value| BorrowedString(value.as_str())),
            serializer,
        )
    }
}

struct BorrowedCollectionIds<'a>(&'a [CollectionId]);

impl Archive for BorrowedCollectionIds<'_> {
    type Archived = rkyv::vec::ArchivedVec<Archived<u32>>;
    type Resolver = VecResolver;

    fn resolve(&self, resolver: Self::Resolver, out: Place<Self::Archived>) {
        rkyv::vec::ArchivedVec::resolve_from_len(self.0.len(), resolver, out);
    }
}

impl<S> ArchiveSerialize<S> for BorrowedCollectionIds<'_>
where
    S: rkyv::rancor::Fallible + rkyv::ser::Allocator + rkyv::ser::Writer + ?Sized,
    S::Error: rkyv::rancor::Source,
{
    fn serialize(&self, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        rkyv::vec::ArchivedVec::serialize_from_iter(
            self.0.iter().map(|collection| collection.0),
            serializer,
        )
    }
}

struct BorrowedRecordSlots<'a>(&'a [RecordSlot]);

impl Archive for BorrowedRecordSlots<'_> {
    type Archived = rkyv::vec::ArchivedVec<Archived<u32>>;
    type Resolver = VecResolver;

    fn resolve(&self, resolver: Self::Resolver, out: Place<Self::Archived>) {
        rkyv::vec::ArchivedVec::resolve_from_len(self.0.len(), resolver, out);
    }
}

impl<S> ArchiveSerialize<S> for BorrowedRecordSlots<'_>
where
    S: rkyv::rancor::Fallible + rkyv::ser::Allocator + rkyv::ser::Writer + ?Sized,
    S::Error: rkyv::rancor::Source,
{
    fn serialize(&self, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        rkyv::vec::ArchivedVec::serialize_from_iter(self.0.iter().map(|slot| slot.0), serializer)
    }
}

struct BorrowedDiagnostic<'a>(&'a SourceDiagnostic);

struct BorrowedDiagnosticResolver {
    file: <u32 as Archive>::Resolver,
    collection: <u32 as Archive>::Resolver,
    source_id: StringResolver,
    source_version: <u32 as Archive>::Resolver,
    record: Option<<u32 as Archive>::Resolver>,
    range_start: Option<<u64 as Archive>::Resolver>,
    range_end: Option<<u64 as Archive>::Resolver>,
    category: <u8 as Archive>::Resolver,
    code: StringResolver,
    field_path: Option<VecResolver>,
    byte_offset: Option<<u64 as Archive>::Resolver>,
    line: Option<<u64 as Archive>::Resolver>,
    column: Option<<u64 as Archive>::Resolver>,
    message: StringResolver,
}

impl Archive for BorrowedDiagnostic<'_> {
    type Archived = Archived<StoredDiagnostic>;
    type Resolver = BorrowedDiagnosticResolver;

    fn resolve(&self, resolver: Self::Resolver, out: Place<Self::Archived>) {
        // SAFETY: The adapter projects each field to the generated
        // `Archived<StoredDiagnostic>` field at the same offset.
        unsafe {
            let root = out.ptr();
            Archive::resolve(
                &self.0.file.0,
                resolver.file,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).file)),
            );
            Archive::resolve(
                &self.0.collection.0,
                resolver.collection,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).collection)),
            );
            ArchivedString::resolve_from_str(
                &self.0.source_id,
                resolver.source_id,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).source_id)),
            );
            Archive::resolve(
                &self.0.source_version,
                resolver.source_version,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).source_version)),
            );
            let record = self.0.record.map(|slot| slot.0);
            Archive::resolve(
                &record,
                resolver.record,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).record)),
            );
            let range_start = self.0.range.map(|range| range.start);
            Archive::resolve(
                &range_start,
                resolver.range_start,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).range_start)),
            );
            let range_end = self.0.range.map(|range| range.end);
            Archive::resolve(
                &range_end,
                resolver.range_end,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).range_end)),
            );
            Archive::resolve(
                &category_code(self.0.category),
                resolver.category,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).category)),
            );
            ArchivedString::resolve_from_str(
                &self.0.code,
                resolver.code,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).code)),
            );
            let field_path = self.0.field_path.as_deref().map(BorrowedStringSlice);
            Archive::resolve(
                &field_path,
                resolver.field_path,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).field_path)),
            );
            Archive::resolve(
                &self.0.byte_offset,
                resolver.byte_offset,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).byte_offset)),
            );
            Archive::resolve(
                &self.0.line,
                resolver.line,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).line)),
            );
            Archive::resolve(
                &self.0.column,
                resolver.column,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).column)),
            );
            ArchivedString::resolve_from_str(
                &self.0.message,
                resolver.message,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).message)),
            );
        }
    }
}

impl<S> ArchiveSerialize<S> for BorrowedDiagnostic<'_>
where
    S: rkyv::rancor::Fallible + rkyv::ser::Allocator + rkyv::ser::Writer + ?Sized,
    S::Error: rkyv::rancor::Source,
{
    fn serialize(&self, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        let field_path = match self.0.field_path.as_deref() {
            Some(path) => Some(ArchiveSerialize::serialize(
                &BorrowedStringSlice(path),
                serializer,
            )?),
            None => None,
        };
        Ok(BorrowedDiagnosticResolver {
            file: ArchiveSerialize::serialize(&self.0.file.0, serializer)?,
            collection: ArchiveSerialize::serialize(&self.0.collection.0, serializer)?,
            source_id: ArchivedString::serialize_from_str(&self.0.source_id, serializer)?,
            source_version: ArchiveSerialize::serialize(&self.0.source_version, serializer)?,
            record: match self.0.record {
                Some(slot) => Some(ArchiveSerialize::serialize(&slot.0, serializer)?),
                None => None,
            },
            range_start: match self.0.range {
                Some(range) => Some(ArchiveSerialize::serialize(&range.start, serializer)?),
                None => None,
            },
            range_end: match self.0.range {
                Some(range) => Some(ArchiveSerialize::serialize(&range.end, serializer)?),
                None => None,
            },
            category: ArchiveSerialize::serialize(&category_code(self.0.category), serializer)?,
            code: ArchivedString::serialize_from_str(&self.0.code, serializer)?,
            field_path,
            byte_offset: match self.0.byte_offset {
                Some(value) => Some(ArchiveSerialize::serialize(&value, serializer)?),
                None => None,
            },
            line: match self.0.line {
                Some(value) => Some(ArchiveSerialize::serialize(&value, serializer)?),
                None => None,
            },
            column: match self.0.column {
                Some(value) => Some(ArchiveSerialize::serialize(&value, serializer)?),
                None => None,
            },
            message: ArchivedString::serialize_from_str(&self.0.message, serializer)?,
        })
    }
}

struct BorrowedSegmentFile<'a>(&'a SegmentFile);

struct BorrowedSegmentFileResolver<'a> {
    id: <u32 as Archive>::Resolver,
    relative_path: BorrowedStoredPathResolver<'a>,
    modified_ns: <i128 as Archive>::Resolver,
    size: <u64 as Archive>::Resolver,
    created_ns: Option<<i128 as Archive>::Resolver>,
    collections: VecResolver,
    status: <u8 as Archive>::Resolver,
    diagnostics: VecResolver,
    omitted_diagnostics: <u32 as Archive>::Resolver,
    records: VecResolver,
}

impl<'a> Archive for BorrowedSegmentFile<'a> {
    type Archived = Archived<StoredSegmentFile>;
    type Resolver = BorrowedSegmentFileResolver<'a>;

    fn resolve(&self, resolver: Self::Resolver, out: Place<Self::Archived>) {
        // SAFETY: Every projection targets the corresponding generated field
        // of `Archived<StoredSegmentFile>` and uses that field's resolver.
        unsafe {
            let root = out.ptr();
            Archive::resolve(
                &self.0.entry.id.0,
                resolver.id,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).id)),
            );
            let path = BorrowedStoredPath::from_shared(&self.0.entry.relative_path);
            Archive::resolve(
                &path,
                resolver.relative_path,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).relative_path)),
            );
            Archive::resolve(
                &self.0.entry.revision.modified_ns,
                resolver.modified_ns,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).modified_ns)),
            );
            Archive::resolve(
                &self.0.entry.revision.size,
                resolver.size,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).size)),
            );
            Archive::resolve(
                &self.0.entry.created_ns,
                resolver.created_ns,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).created_ns)),
            );
            Archive::resolve(
                &BorrowedCollectionIds(&self.0.entry.collections),
                resolver.collections,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).collections)),
            );
            Archive::resolve(
                &self.0.entry.status.code(),
                resolver.status,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).status)),
            );
            let diagnostics =
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).diagnostics));
            rkyv::vec::ArchivedVec::<Archived<StoredDiagnostic>>::resolve_from_len(
                self.0.entry.diagnostics.len(),
                resolver.diagnostics,
                diagnostics,
            );
            Archive::resolve(
                &self.0.entry.omitted_diagnostics,
                resolver.omitted_diagnostics,
                Place::from_field_unchecked(
                    out,
                    std::ptr::addr_of_mut!((*root).omitted_diagnostics),
                ),
            );
            Archive::resolve(
                &BorrowedRecordSlots(&self.0.records),
                resolver.records,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).records)),
            );
        }
    }
}

impl<'a, S> ArchiveSerialize<S> for BorrowedSegmentFile<'a>
where
    S: rkyv::rancor::Fallible + rkyv::ser::Allocator + rkyv::ser::Writer + ?Sized,
    S::Error: rkyv::rancor::Source,
{
    fn serialize(&self, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        Ok(BorrowedSegmentFileResolver {
            id: ArchiveSerialize::serialize(&self.0.entry.id.0, serializer)?,
            relative_path: ArchiveSerialize::serialize(
                &BorrowedStoredPath::from_shared(&self.0.entry.relative_path),
                serializer,
            )?,
            modified_ns: ArchiveSerialize::serialize(
                &self.0.entry.revision.modified_ns,
                serializer,
            )?,
            size: ArchiveSerialize::serialize(&self.0.entry.revision.size, serializer)?,
            created_ns: ArchiveSerialize::serialize(&self.0.entry.created_ns, serializer)?,
            collections: rkyv::vec::ArchivedVec::serialize_from_iter(
                self.0
                    .entry
                    .collections
                    .iter()
                    .map(|collection| collection.0),
                serializer,
            )?,
            status: ArchiveSerialize::serialize(&self.0.entry.status.code(), serializer)?,
            diagnostics: rkyv::vec::ArchivedVec::serialize_from_iter(
                self.0.entry.diagnostics.iter().map(BorrowedDiagnostic),
                serializer,
            )?,
            omitted_diagnostics: ArchiveSerialize::serialize(
                &self.0.entry.omitted_diagnostics,
                serializer,
            )?,
            records: rkyv::vec::ArchivedVec::serialize_from_iter(
                self.0.records.iter().map(|slot| slot.0),
                serializer,
            )?,
        })
    }
}

struct BorrowedRecord<'a>(&'a RecordEntry);

struct BorrowedRecordResolver {
    slot: <u32 as Archive>::Resolver,
    id: StringResolver,
    file: <u32 as Archive>::Resolver,
    range_start: <u64 as Archive>::Resolver,
    range_end: <u64 as Archive>::Resolver,
    modified_ns: <i128 as Archive>::Resolver,
    size: <u64 as Archive>::Resolver,
    class: <u8 as Archive>::Resolver,
    diagnostic_ref: <u32 as Archive>::Resolver,
}

impl Archive for BorrowedRecord<'_> {
    type Archived = Archived<StoredRecord>;
    type Resolver = BorrowedRecordResolver;

    fn resolve(&self, resolver: Self::Resolver, out: Place<Self::Archived>) {
        // SAFETY: The adapter projects each field to the generated
        // `Archived<StoredRecord>` field at the same offset.
        unsafe {
            let root = out.ptr();
            Archive::resolve(
                &self.0.slot.0,
                resolver.slot,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).slot)),
            );
            ArchivedString::resolve_from_str(
                &self.0.id.0,
                resolver.id,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).id)),
            );
            Archive::resolve(
                &self.0.file.0,
                resolver.file,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).file)),
            );
            Archive::resolve(
                &self.0.range.start,
                resolver.range_start,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).range_start)),
            );
            Archive::resolve(
                &self.0.range.end,
                resolver.range_end,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).range_end)),
            );
            Archive::resolve(
                &self.0.revision.modified_ns,
                resolver.modified_ns,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).modified_ns)),
            );
            Archive::resolve(
                &self.0.revision.size,
                resolver.size,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).size)),
            );
            Archive::resolve(
                &self.0.class.code(),
                resolver.class,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).class)),
            );
            Archive::resolve(
                &self.0.diagnostic_ref.0,
                resolver.diagnostic_ref,
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).diagnostic_ref)),
            );
        }
    }
}

impl<S> ArchiveSerialize<S> for BorrowedRecord<'_>
where
    S: rkyv::rancor::Fallible + rkyv::ser::Writer + ?Sized,
    S::Error: rkyv::rancor::Source,
{
    fn serialize(&self, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        Ok(BorrowedRecordResolver {
            slot: ArchiveSerialize::serialize(&self.0.slot.0, serializer)?,
            id: ArchivedString::serialize_from_str(&self.0.id.0, serializer)?,
            file: ArchiveSerialize::serialize(&self.0.file.0, serializer)?,
            range_start: ArchiveSerialize::serialize(&self.0.range.start, serializer)?,
            range_end: ArchiveSerialize::serialize(&self.0.range.end, serializer)?,
            modified_ns: ArchiveSerialize::serialize(&self.0.revision.modified_ns, serializer)?,
            size: ArchiveSerialize::serialize(&self.0.revision.size, serializer)?,
            class: ArchiveSerialize::serialize(&self.0.class.code(), serializer)?,
            diagnostic_ref: ArchiveSerialize::serialize(&self.0.diagnostic_ref.0, serializer)?,
        })
    }
}

struct SegmentRowsArchiveSource<'a> {
    files: &'a [SegmentFile],
    records: &'a [RecordEntry],
}

struct SegmentRowsArchiveSourceResolver {
    files: VecResolver,
    records: VecResolver,
}

impl Archive for SegmentRowsArchiveSource<'_> {
    type Archived = Archived<SegmentRowsArchive>;
    type Resolver = SegmentRowsArchiveSourceResolver;

    fn resolve(&self, resolver: Self::Resolver, out: Place<Self::Archived>) {
        // SAFETY: The source mirrors the two generated fields of
        // `SegmentRowsArchive`.
        unsafe {
            let root = out.ptr();
            let files = Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).files));
            let records = Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).records));
            rkyv::vec::ArchivedVec::<Archived<StoredSegmentFile>>::resolve_from_len(
                self.files.len(),
                resolver.files,
                files,
            );
            rkyv::vec::ArchivedVec::<Archived<StoredRecord>>::resolve_from_len(
                self.records.len(),
                resolver.records,
                records,
            );
        }
    }
}

impl<S> ArchiveSerialize<S> for SegmentRowsArchiveSource<'_>
where
    S: rkyv::rancor::Fallible + rkyv::ser::Allocator + rkyv::ser::Writer + ?Sized,
    S::Error: rkyv::rancor::Source,
{
    fn serialize(&self, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        Ok(SegmentRowsArchiveSourceResolver {
            files: rkyv::vec::ArchivedVec::serialize_from_iter(
                self.files.iter().map(BorrowedSegmentFile),
                serializer,
            )?,
            records: rkyv::vec::ArchivedVec::serialize_from_iter(
                self.records.iter().map(BorrowedRecord),
                serializer,
            )?,
        })
    }
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
        let source = SegmentRowsArchiveSource {
            files: self.files(),
            records: self.records(),
        };
        let archive: Arc<ValidatedArchive<SegmentRowsArchive>> =
            ValidatedArchive::freeze_source(&source)?;
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

pub(crate) enum SegmentRecordSlotsIter<'a> {
    Native(std::slice::Iter<'a, RecordSlot>),
    Archived(std::slice::Iter<'a, Archived<u32>>),
}

impl Iterator for SegmentRecordSlotsIter<'_> {
    type Item = RecordSlot;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Native(slots) => slots.next().copied(),
            Self::Archived(slots) => slots.next().map(|slot| RecordSlot(slot.to_native())),
        }
    }
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
        match &self.owner.backing {
            SegmentRowsBacking::Native { files, .. } => files.get(index).map(|file| file.entry.id),
            SegmentRowsBacking::Archived { archive, .. } => archive
                .root()
                .files
                .get(index)
                .map(|file| FileId(file.id.to_native())),
        }
    }

    pub(crate) fn file_ids(&self) -> impl Iterator<Item = FileId> + '_ {
        self.values().iter().map(|file| file.entry.id)
    }

    pub(crate) fn archived_record_slots(&self, index: usize) -> Option<SegmentRecordSlotsIter<'_>> {
        match &self.owner.backing {
            SegmentRowsBacking::Native { files, .. } => Some(SegmentRecordSlotsIter::Native(
                files.get(index)?.records.iter(),
            )),
            SegmentRowsBacking::Archived { archive, .. } => Some(SegmentRecordSlotsIter::Archived(
                archive.root().files.get(index)?.records.iter(),
            )),
        }
    }

    pub(crate) fn source_bytes(&self) -> u64 {
        match &self.owner.backing {
            SegmentRowsBacking::Native { files, .. } => files
                .iter()
                .map(|file| file.entry.revision.size)
                .fold(0u64, u64::saturating_add),
            SegmentRowsBacking::Archived { archive, .. } => archive
                .root()
                .files
                .iter()
                .map(|file| file.size.to_native())
                .fold(0u64, u64::saturating_add),
        }
    }

    pub(crate) fn revision(&self, index: usize) -> Option<SourceRevision> {
        match &self.owner.backing {
            SegmentRowsBacking::Native { files, .. } => {
                files.get(index).map(|file| file.entry.revision)
            }
            SegmentRowsBacking::Archived { archive, .. } => {
                let file = archive.root().files.get(index)?;
                Some(SourceRevision {
                    modified_ns: file.modified_ns.to_native(),
                    size: file.size.to_native(),
                })
            }
        }
    }

    pub(crate) fn created_ns(&self, index: usize) -> Option<Option<i128>> {
        match &self.owner.backing {
            SegmentRowsBacking::Native { files, .. } => {
                files.get(index).map(|file| file.entry.created_ns)
            }
            SegmentRowsBacking::Archived { archive, .. } => archive
                .root()
                .files
                .get(index)
                .map(|file| file.created_ns.as_ref().map(|value| value.to_native())),
        }
    }

    pub(crate) fn native_path(&self, index: usize) -> Option<&std::path::Path> {
        self.values()
            .get(index)
            .map(|file| file.entry.relative_path.as_path())
    }

    pub(crate) fn status_code(&self, index: usize) -> Option<u8> {
        match &self.owner.backing {
            SegmentRowsBacking::Native { files, .. } => {
                files.get(index).map(|file| file.entry.status.code())
            }
            SegmentRowsBacking::Archived { archive, .. } => {
                archive.root().files.get(index).map(|file| file.status)
            }
        }
    }

    pub(crate) fn collections(&self, index: usize) -> Option<Vec<CollectionId>> {
        match &self.owner.backing {
            SegmentRowsBacking::Native { files, .. } => {
                files.get(index).map(|file| file.entry.collections.clone())
            }
            SegmentRowsBacking::Archived { archive, .. } => {
                archive.root().files.get(index).map(|file| {
                    file.collections
                        .iter()
                        .map(|collection| CollectionId(collection.to_native()))
                        .collect()
                })
            }
        }
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
        match &self.owner.backing {
            SegmentRowsBacking::Native { files, .. } => {
                files.get(index).map(|file| file.entry.omitted_diagnostics)
            }
            SegmentRowsBacking::Archived { archive, .. } => archive
                .root()
                .files
                .get(index)
                .map(|file| file.omitted_diagnostics.to_native()),
        }
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
    values: &'a HashMap<RecordId, RecordSlot>,
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
            u32,
            RecordLookupKey<'_>,
            u32,
            S,
        >(
            self.values
                .iter()
                .map(|(id, slot)| (RecordLookupKey(id.0.as_ref()), slot.0)),
            (7, 8),
            serializer,
        )?;
        Ok(RecordLookupSourceResolver { values })
    }
}

enum RecordLookupBacking {
    Native(Arc<HashMap<RecordId, RecordSlot>>),
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
        if values.keys().any(|id| id.0.is_empty()) {
            return Err("invalid segment record ID lookup".into());
        }
        Ok(Self {
            backing: RecordLookupBacking::Native(Arc::new(values)),
        })
    }

    pub(crate) fn get(&self, id: &str) -> Option<RecordSlot> {
        match &self.backing {
            RecordLookupBacking::Native(values) => values.get(id).copied(),
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
                    visit(id.0.as_ref());
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
            let mut owned = vec![false; record_rows.len()];
            for (ordinal, file) in file_rows.iter().enumerate() {
                let id = file.entry.id;
                if !file_ids.insert(id)
                    || file_index.get(id) != Some(ordinal)
                    || !file.entry.collections.contains(&collection)
                {
                    return Err("segment file row or lookup is inconsistent".into());
                }
                for slot in &file.records {
                    let owned = owned
                        .get_mut(slot.0 as usize)
                        .ok_or("segment file owns an out-of-bounds record slot")?;
                    if *owned {
                        return Err("segment record has duplicate file ownership".into());
                    }
                    *owned = true;
                    let record = record_rows
                        .get(slot.0 as usize)
                        .ok_or("segment file owns an out-of-bounds record slot")?;
                    if record.file != id {
                        return Err("segment file and record ownership disagree".into());
                    }
                }
            }
            if owned.iter().any(|owned| !owned) {
                return Err("segment record has no file ownership".into());
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
                    || !ids.insert(record.id.0.as_str())
                    || id_lookup.get(record.id.0.as_str()) != Some(RecordSlot(slot as u32))
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
            let mut owned = vec![false; root.records.len()];
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
                    let owned = owned
                        .get_mut(slot.to_native() as usize)
                        .ok_or("segment file owns an out-of-bounds record slot")?;
                    if *owned {
                        return Err("segment record has duplicate file ownership".into());
                    }
                    *owned = true;
                    let record = root
                        .records
                        .get(slot.to_native() as usize)
                        .ok_or("segment file owns an out-of-bounds record slot")?;
                    if record.file.to_native() != file.id.to_native() {
                        return Err("segment file and record ownership disagree".into());
                    }
                }
            }
            if owned.iter().any(|owned| !owned) {
                return Err("segment record has no file ownership".into());
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
