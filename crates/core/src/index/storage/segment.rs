//! Segment identity, record addresses, and sizing policy.

pub(crate) mod build;

use std::{
    borrow::Cow,
    fmt,
    num::{NonZeroU64, NonZeroUsize},
};

use serde::{Deserialize, Serialize};

use crate::inventory::{ByteRange, FileEntry, SourceRevision};
use crate::model::{CollectionId, FieldPath, FileId, RecordSlot};
use crate::query::options::SortField;
use crate::records::{RecordHeader, RecordVersion, RetainedBody};

use super::order::{InferredOrderRows, OrderColumnReader, PostingOrderRequest, SortKey};
use super::ordered_slots::OrderedSlots;
use super::transfer::{BatchLimit, BatchStatus, StorageError, StorageResult};

/// Deterministic soft target used by every ordinary segment-producing path.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SegmentPolicy {
    pub target_records: NonZeroUsize,
    pub max_files: NonZeroUsize,
}

impl Default for SegmentPolicy {
    fn default() -> Self {
        Self {
            target_records: NonZeroUsize::new(16_384).expect("nonzero default"),
            max_files: NonZeroUsize::new(16_384).expect("nonzero default"),
        }
    }
}

impl SegmentPolicy {
    pub(crate) fn validate(self) -> Result<Self, String> {
        let local_limit = u32::MAX as usize;
        if self.target_records.get() > local_limit {
            return Err("segment target exceeds the supported local-slot range".into());
        }
        if self.max_files.get() > local_limit {
            return Err("segment file limit exceeds the supported local-slot range".into());
        }
        Ok(self)
    }

    pub fn with_target_records(target_records: NonZeroUsize) -> Result<Self, String> {
        Self {
            target_records,
            max_files: target_records,
        }
        .validate()
    }

    /// Apply the soft boundary only between completed contributions. An empty
    /// draft always accepts one oversized indivisible contribution; the next
    /// contribution starts a fresh segment.
    pub(crate) fn can_grow(
        self,
        current_records: usize,
        current_files: usize,
        contribution_records: usize,
        contribution_new_files: usize,
    ) -> bool {
        if current_records == 0 && current_files == 0 {
            return true;
        }
        current_records
            .checked_add(contribution_records)
            .is_some_and(|records| records <= self.target_records.get())
            && current_files
                .checked_add(contribution_new_files)
                .is_some_and(|files| files <= self.max_files.get())
    }
}

/// Stable identity for one immutable segment.
///
/// Segment-local record slots are meaningful only together with this ID and
/// the owning collection.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct SegmentId(u64);

impl SegmentId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for SegmentId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Address that remains unambiguous after crossing an immutable segment
/// boundary. Record slots are meaningful only with the collection and segment
/// components of this value.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct SegmentRecordAddress {
    pub collection: CollectionId,
    pub segment: SegmentId,
    pub slot: RecordSlot,
}

/// Immutable logical segment metadata, captured at seal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SegmentDescriptor {
    pub(crate) id: SegmentId,
    pub(crate) collection: CollectionId,
    pub(crate) slots: usize,
}

/// Identity of one direct write in this index process. The same token follows
/// a record when maintenance changes its segment or slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RecordWriteVersion(NonZeroU64);

impl RecordWriteVersion {
    pub(crate) fn new(value: u64) -> Option<Self> {
        NonZeroU64::new(value).map(Self)
    }

    pub(crate) fn allocate(count: usize) -> StorageResult<std::ops::Range<u64>> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let count = u64::try_from(count)
            .map_err(|_| StorageError::ResourceLimit("write version count overflow".into()))?;
        let mut start = NEXT.load(Ordering::Relaxed);
        loop {
            let end = start.checked_add(count).ok_or_else(|| {
                StorageError::ResourceLimit("record write versions exhausted".into())
            })?;
            match NEXT.compare_exchange_weak(start, end, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => return Ok(start..end),
                Err(current) => start = current,
            }
        }
    }
}

/// Adapter identity captured when a segment is sealed.
pub(crate) struct SegmentSource<'a> {
    pub adapter: &'a str,
    pub version: u32,
    pub registration: Option<u32>,
}

const HEADER_LIMIT: BatchLimit = BatchLimit::new(
    NonZeroUsize::new(256).unwrap(),
    NonZeroUsize::new(16 * 1024 * 1024).unwrap(),
);

const READ_HEADERS: &str = "read segment headers";

/// Check slot order and completion before passing a batch to publication.
pub(crate) fn visit_record_headers<E: From<StorageError>>(
    reader: &dyn SegmentReader,
    mut visit: impl FnMut(&[RecordHeader]) -> Result<(), E>,
) -> Result<(), E> {
    let total = reader.descriptor().slots;
    let mut start = 0usize;
    let max_rows = HEADER_LIMIT.fixed_batch_len(READ_HEADERS, size_of::<RecordHeader>())?;
    loop {
        let (headers, status) = reader.read_header_batch(start, HEADER_LIMIT)?;
        let end = start
            .checked_add(headers.len())
            .ok_or_else(|| StorageError::invalid(READ_HEADERS, "record slot overflow"))?;
        if end > total
            || (status == BatchStatus::Done) != (end == total)
            || (headers.is_empty() && status == BatchStatus::More)
            || headers.len() > max_rows
            || headers
                .iter()
                .enumerate()
                .any(|(offset, header)| header.slot.0 as usize != start + offset)
        {
            return Err(
                StorageError::invalid(READ_HEADERS, "invalid header batch or slot order").into(),
            );
        }
        if !headers.is_empty() {
            visit(&headers)?;
        }
        if status == BatchStatus::Done {
            return Ok(());
        }
        start = end;
    }
}

/// Logical provenance for a file-backed address or an optional direct link.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FileRangeLocator {
    pub file: FileId,
    pub range: Option<ByteRange>,
    pub revision: SourceRevision,
}

/// Source service used by the segment to materialize record data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RecordBacking {
    FileRange,
    Retained,
    Provider,
}

#[derive(Clone, Copy)]
pub(crate) enum RecordOrigin {
    FileRange(FileRangeLocator),
    Retained(Option<FileRangeLocator>),
    Provider {
        registration: u32,
        version: RecordVersion,
        file: Option<FileRangeLocator>,
    },
}

impl RecordOrigin {
    pub(crate) fn file(self) -> Option<FileRangeLocator> {
        match self {
            Self::FileRange(file) => Some(file),
            Self::Retained(file) | Self::Provider { file, .. } => file,
        }
    }
}

/// An immutable batch of retained bodies. Segments can share blocks when
/// projected or consolidated, and only requested bodies need to be decoded.
pub(crate) trait RetainedBlockReader: Send + Sync {
    fn len(&self) -> usize;
    fn body(&self, offset: u32) -> StorageResult<Cow<'_, RetainedBody>>;
    fn estimated_body_bytes(&self) -> usize;
}

pub(crate) type RetainedBlock = std::sync::Arc<dyn RetainedBlockReader>;

/// Reads from a pinned segment. Returned keys and headers can borrow the
/// pinned representation or hold an owned value decoded by the reader.
pub(crate) trait SegmentReader: Send + Sync {
    fn descriptor(&self) -> SegmentDescriptor;

    fn source(&self) -> SegmentSource<'_>;

    /// Read records in slot order. Borrowed batches charge their row storage,
    /// and decoded batches also charge their owned IDs to the supplied limit.
    fn read_header_batch(
        &self,
        start: usize,
        limit: BatchLimit,
    ) -> StorageResult<(Cow<'_, [RecordHeader]>, BatchStatus)>;

    fn backing_kind(&self) -> RecordBacking;

    fn slot_for_record(&self, id: &str) -> StorageResult<Option<RecordSlot>>;

    fn write_version(&self, _slot: RecordSlot) -> StorageResult<Option<RecordWriteVersion>> {
        Ok(None)
    }

    fn header(&self, slot: RecordSlot) -> StorageResult<Option<Cow<'_, RecordHeader>>>;

    fn origin(&self, slot: RecordSlot) -> StorageResult<Option<RecordOrigin>>;

    fn retained_body(&self, slot: RecordSlot) -> StorageResult<Option<Cow<'_, RetainedBody>>>;

    fn file_metadata(&self, file: FileId) -> StorageResult<Option<Cow<'_, FileEntry>>>;

    fn visit_file_slots(
        &self,
        file: FileId,
        limit: BatchLimit,
        visit: &mut dyn FnMut(&[RecordSlot]) -> StorageResult<()>,
    ) -> StorageResult<()>;

    fn supports_posting_order(&self, _path: &FieldPath) -> StorageResult<bool> {
        Ok(false)
    }

    fn build_posting_order(
        &self,
        _request: PostingOrderRequest<'_>,
    ) -> StorageResult<Option<InferredOrderRows>> {
        Ok(None)
    }

    fn order_column(&self, _path: &FieldPath) -> StorageResult<Option<&dyn OrderColumnReader>> {
        Ok(None)
    }

    fn stored_key(
        &self,
        path: &FieldPath,
        slot: RecordSlot,
    ) -> StorageResult<Option<Cow<'_, SortKey>>>;

    fn ordered_slots(
        &self,
        sort: &[SortField],
        scalar: Option<(&FieldPath, bool)>,
    ) -> StorageResult<Option<&dyn OrderedSlots>>;
}

impl<S: SegmentReader + ?Sized> SegmentReader for std::sync::Arc<S> {
    fn descriptor(&self) -> SegmentDescriptor {
        self.as_ref().descriptor()
    }

    fn source(&self) -> SegmentSource<'_> {
        self.as_ref().source()
    }

    fn read_header_batch(
        &self,
        start: usize,
        limit: BatchLimit,
    ) -> StorageResult<(Cow<'_, [RecordHeader]>, BatchStatus)> {
        self.as_ref().read_header_batch(start, limit)
    }

    fn backing_kind(&self) -> RecordBacking {
        self.as_ref().backing_kind()
    }

    fn slot_for_record(&self, id: &str) -> StorageResult<Option<RecordSlot>> {
        self.as_ref().slot_for_record(id)
    }

    fn write_version(&self, slot: RecordSlot) -> StorageResult<Option<RecordWriteVersion>> {
        self.as_ref().write_version(slot)
    }

    fn header(&self, slot: RecordSlot) -> StorageResult<Option<Cow<'_, RecordHeader>>> {
        self.as_ref().header(slot)
    }

    fn origin(&self, slot: RecordSlot) -> StorageResult<Option<RecordOrigin>> {
        self.as_ref().origin(slot)
    }

    fn retained_body(&self, slot: RecordSlot) -> StorageResult<Option<Cow<'_, RetainedBody>>> {
        self.as_ref().retained_body(slot)
    }

    fn file_metadata(&self, file: FileId) -> StorageResult<Option<Cow<'_, FileEntry>>> {
        self.as_ref().file_metadata(file)
    }

    fn visit_file_slots(
        &self,
        file: FileId,
        limit: BatchLimit,
        visit: &mut dyn FnMut(&[RecordSlot]) -> StorageResult<()>,
    ) -> StorageResult<()> {
        self.as_ref().visit_file_slots(file, limit, visit)
    }

    fn supports_posting_order(&self, path: &FieldPath) -> StorageResult<bool> {
        self.as_ref().supports_posting_order(path)
    }

    fn build_posting_order(
        &self,
        request: PostingOrderRequest<'_>,
    ) -> StorageResult<Option<InferredOrderRows>> {
        self.as_ref().build_posting_order(request)
    }

    fn order_column(&self, path: &FieldPath) -> StorageResult<Option<&dyn OrderColumnReader>> {
        self.as_ref().order_column(path)
    }

    fn stored_key(
        &self,
        path: &FieldPath,
        slot: RecordSlot,
    ) -> StorageResult<Option<Cow<'_, SortKey>>> {
        self.as_ref().stored_key(path, slot)
    }

    fn ordered_slots(
        &self,
        sort: &[SortField],
        scalar: Option<(&FieldPath, bool)>,
    ) -> StorageResult<Option<&dyn OrderedSlots>> {
        self.as_ref().ordered_slots(sort, scalar)
    }
}

#[cfg(test)]
mod tests;
