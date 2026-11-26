//! Segment identity, record addresses, and sizing policy.

pub(crate) mod build;
pub(crate) mod maintenance;
pub(crate) mod transfer;

use std::{
    borrow::Cow,
    collections::HashSet,
    fmt,
    num::{NonZeroU64, NonZeroUsize},
};

use serde::{Deserialize, Serialize};

use crate::inventory::{ByteRange, FileEntry, SourceRevision};
use crate::model::{CollectionId, FieldPath, FileId, RecordSlot};
use crate::query::options::SortField;
use crate::query_access::contract::IndexAccessProvider;
use crate::query_access::membership::{MembershipView, SlotSet};
use crate::records::{RecordHeader, RecordVersion, RetainedBody};

use super::component::ComponentDescriptor;
use super::file_catalog::{FILE_TRANSFER_LIMIT, FileTransferRow};
use super::inference::InferredPostingDiagnostic;
use super::liveness::ActiveRecords;
use super::order::{InferredOrderRows, OrderColumnReader, PostingOrderRequest, SortKey};
use super::ordered_slots::OrderedSlots;
use super::posting::{FacetPosting, PostingReader};
use super::relation::{RelationMatches, RelationReader};
use super::text::{GramReader, SegmentTextAccess};
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

/// Saturating retained-size and query-planning estimates.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SegmentEstimates {
    pub retained_bytes_estimated: usize,
    pub source_bytes_estimated: u64,
    pub max_record_range_len: u64,
    pub index_family_bytes: usize,
}

#[derive(Default)]
pub(crate) struct RecordDirectoryEstimate {
    pub record_table_capacity: usize,
    pub unique_id_payload_bytes: usize,
    pub identity_lookup_capacity: usize,
    pub shared_id_references: usize,
}

/// Public read-only summary of one segment for diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct NativeSegmentSummary {
    pub id: SegmentId,
    pub collection: CollectionId,
    pub records: usize,
    pub active_records: usize,
    pub files: usize,
    pub source_bytes: u64,
    pub posting_fields: usize,
    pub sort_columns: usize,
    pub relation_count: usize,
    pub relation_edges: usize,
    pub relation_heap_bytes: usize,
    pub bigram_stores: usize,
    pub distinct_gram_keys: usize,
    pub bigram_heap_bytes: usize,
    pub index_family_bytes: usize,
    pub active: bool,
}

/// Summarizes resident backing with shared retained blocks charged once.
#[derive(Default)]
pub(crate) struct SegmentResourceUsage {
    retained_blocks: HashSet<usize>,
    pub backing_column_bytes: usize,
    pub retained_body_bytes: usize,
}

impl SegmentResourceUsage {
    pub(crate) fn add_backing_columns(&mut self, bytes: usize) {
        self.backing_column_bytes = self.backing_column_bytes.saturating_add(bytes);
    }

    pub(crate) fn add_retained_block(&mut self, block: &RetainedBlock) {
        let key = std::sync::Arc::as_ptr(block).cast::<()>() as usize;
        if self.retained_blocks.insert(key) {
            self.retained_body_bytes = self
                .retained_body_bytes
                .saturating_add(block.estimated_body_bytes());
        }
    }
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

pub(crate) const FILE_SLOT_LIMIT: BatchLimit = BatchLimit::new(
    NonZeroUsize::new(256).unwrap(),
    NonZeroUsize::new(1024).unwrap(),
);

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
        if let Cow::Owned(rows) = &headers {
            let mut bytes = 0;
            for (items, header) in rows.iter().enumerate() {
                let row_bytes = size_of::<RecordHeader>().saturating_add(header.id.0.len());
                if !HEADER_LIMIT.accepts(READ_HEADERS, items, bytes, row_bytes)? {
                    return Err(StorageError::invalid(
                        READ_HEADERS,
                        "header batch exceeds its limits",
                    )
                    .into());
                }
                bytes += row_bytes;
            }
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

/// Visit the local file membership of a sealed segment. The buffer contains
/// one bounded page and is reused across refills.
pub(crate) fn visit_segment_files<E: From<StorageError>>(
    reader: &dyn SegmentReader,
    mut visit: impl FnMut(&FileEntry) -> Result<(), E>,
) -> Result<(), E> {
    any_segment_file(reader, |file| {
        visit(file)?;
        Ok::<_, E>(false)
    })?;
    Ok(())
}

pub(crate) fn any_segment_file<E: From<StorageError>>(
    reader: &dyn SegmentReader,
    mut matches: impl FnMut(&FileEntry) -> Result<bool, E>,
) -> Result<bool, E> {
    visit_segment_file_batches(reader, |files| {
        for file in files {
            if matches(file)? {
                return Ok(true);
            }
        }
        Ok(false)
    })
}

/// Check a bounded file page before passing its guarded rows to the caller.
/// Returning true stops without opening another page.
pub(crate) fn visit_segment_file_batches<E: From<StorageError>>(
    reader: &dyn SegmentReader,
    mut visit: impl FnMut(&[Cow<'_, FileEntry>]) -> Result<bool, E>,
) -> Result<bool, E> {
    const OP: &str = "read segment file entries";
    let total = reader.file_count();
    if total == 0 {
        return Ok(false);
    }
    let mut entries = Vec::new();
    let mut start = 0usize;
    loop {
        let status = reader.read_file_batch(start, FILE_TRANSFER_LIMIT, &mut entries)?;
        let end = start
            .checked_add(entries.len())
            .ok_or_else(|| StorageError::invalid(OP, "file count overflow"))?;
        if end > total || (status == BatchStatus::Done) != (end == total) || entries.is_empty() {
            return Err(StorageError::invalid(OP, "invalid batch progress").into());
        }
        let mut bytes = 0;
        for (index, entry) in entries.iter().enumerate() {
            let next_bytes = match entry {
                Cow::Borrowed(_) => size_of::<Cow<'_, FileEntry>>(),
                Cow::Owned(file) => FileTransferRow {
                    entry: Cow::Borrowed(file),
                    fingerprint: None,
                    configuration: None,
                }
                .bytes(),
            };
            if !FILE_TRANSFER_LIMIT.accepts(OP, index, bytes, next_bytes)? {
                return Err(StorageError::invalid(OP, "file batch exceeds its limits").into());
            }
            bytes += next_bytes;
        }
        if visit(&entries)? {
            return Ok(true);
        }
        entries.clear();
        if status == BatchStatus::Done {
            return Ok(false);
        }
        start = end;
    }
}

/// Enforce the local-slot and bounded-batch contract before publishing file
/// membership to callers. Empty associations have no batches.
pub(crate) fn visit_checked_file_slots<S: SegmentReader + ?Sized>(
    segment: &S,
    file: FileId,
    visit: &mut dyn FnMut(&[RecordSlot]) -> StorageResult<()>,
) -> StorageResult<()> {
    const OP: &str = "read segment file membership";
    let end = segment.descriptor().slots;
    let max = FILE_SLOT_LIMIT.fixed_batch_len(OP, size_of::<RecordSlot>())?;
    segment.visit_file_slots(file, FILE_SLOT_LIMIT, &mut |slots| {
        if slots.is_empty() || slots.len() > max || slots.iter().any(|slot| slot.0 as usize >= end)
        {
            return Err(StorageError::invalid(OP, "invalid file slot batch"));
        }
        visit(slots)
    })
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

#[derive(Clone, Copy)]
pub(crate) struct SegmentPredicateAccess<'a> {
    pub text: SegmentTextAccess<'a>,
    pub collection_name: &'a str,
    pub files: Option<&'a dyn super::file_catalog::FileCatalogReader>,
}

pub(crate) type UseAccessProvider<'a> =
    dyn FnMut(Option<&dyn IndexAccessProvider>) -> StorageResult<()> + 'a;

#[derive(Clone, Copy)]
pub(crate) enum SegmentSlotFact {
    Logical,
    EmptyArray,
}

/// Guard around exact slot facts for one segment. The encoded data can
/// borrow from storage, retain shared encoded slots, or own a decoded buffer.
pub(crate) struct SegmentSlotMembers<'a>(SlotSet<'a>);

impl SegmentSlotMembers<'_> {
    pub(crate) fn view(&self) -> MembershipView<'_> {
        self.0.explicit_view().expect("validated slot membership")
    }
}

pub(crate) fn read_slot_members<S: SegmentReader + ?Sized>(
    segment: &S,
    fact: SegmentSlotFact,
) -> StorageResult<SegmentSlotMembers<'_>> {
    let slots = segment.slot_members(fact)?;
    let view = slots.explicit_view().ok_or_else(|| {
        StorageError::invalid("read segment slot facts", "missing explicit membership")
    })?;
    if view
        .last()
        .is_some_and(|slot| slot as usize >= segment.descriptor().slots)
    {
        return Err(StorageError::invalid(
            "read segment slot facts",
            "slot outside the segment",
        ));
    }
    Ok(SegmentSlotMembers(slots))
}

/// Reads from a pinned segment. Returned keys and headers can borrow the
/// pinned representation or hold an owned value decoded by the reader.
pub(crate) type SegmentHandle = std::sync::Arc<dyn SegmentReader>;

pub(crate) trait SegmentReader: std::any::Any + Send + Sync {
    fn maintenance(&self) -> Option<&dyn maintenance::SegmentMaintenance> {
        None
    }

    fn descriptor(&self) -> SegmentDescriptor;

    fn summary(&self, active_records: usize) -> NativeSegmentSummary {
        let descriptor = self.descriptor();
        NativeSegmentSummary {
            id: descriptor.id,
            collection: descriptor.collection,
            records: descriptor.slots,
            active_records,
            files: self.file_count(),
            source_bytes: self.estimates().source_bytes_estimated,
            posting_fields: 0,
            sort_columns: 0,
            relation_count: self.relation_count(),
            relation_edges: 0,
            relation_heap_bytes: 0,
            bigram_stores: 0,
            distinct_gram_keys: 0,
            bigram_heap_bytes: 0,
            index_family_bytes: self.estimates().index_family_bytes,
            active: active_records != 0,
        }
    }

    fn estimates(&self) -> SegmentEstimates {
        SegmentEstimates::default()
    }

    fn record_directory_estimate(&self) -> RecordDirectoryEstimate {
        RecordDirectoryEstimate::default()
    }

    fn observe_backing(&self, _usage: &mut SegmentResourceUsage) {}

    fn index_family_estimate(&self) -> crate::lifecycle::IndexFamilyEstimate {
        crate::lifecycle::IndexFamilyEstimate::default()
    }

    fn components(&self) -> StorageResult<Vec<ComponentDescriptor<'_>>> {
        Ok(Vec::new())
    }

    fn posting(
        &self,
        _component: &ComponentDescriptor<'_>,
    ) -> StorageResult<Option<&dyn PostingReader>> {
        Ok(None)
    }

    fn inferred_posting_diagnostic(
        &self,
        _path: &[String],
    ) -> StorageResult<Option<InferredPostingDiagnostic>> {
        Ok(None)
    }

    fn facet_posting(
        &self,
        _path: &FieldPath,
        _binding: &str,
    ) -> StorageResult<Option<FacetPosting<'_>>> {
        Ok(None)
    }

    fn grams(
        &self,
        _component: &ComponentDescriptor<'_>,
    ) -> StorageResult<Option<&dyn GramReader>> {
        Ok(None)
    }

    fn order_columns(&self) -> StorageResult<Vec<&dyn OrderColumnReader>> {
        Ok(Vec::new())
    }

    /// Poll counters using immutable, already captured slot information.
    fn active_fallback_count(&self, active: &ActiveRecords) -> u64;

    fn slot_members(&self, fact: SegmentSlotFact) -> StorageResult<SlotSet<'_>>;

    fn source(&self) -> SegmentSource<'_>;

    fn raw_covers_logical_fields(&self) -> bool {
        false
    }

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

    fn file_count(&self) -> usize {
        0
    }

    /// Append a batch of metadata for the local file membership, without
    /// reading records or original sources.
    fn read_file_batch<'a>(
        &'a self,
        _start: usize,
        _limit: BatchLimit,
        _entries: &mut Vec<Cow<'a, FileEntry>>,
    ) -> StorageResult<BatchStatus> {
        Err(StorageError::invalid(
            "read segment file entries",
            "file access unavailable",
        ))
    }

    fn contains_file(&self, file: FileId) -> StorageResult<bool> {
        Ok(self.file_metadata(file)?.is_some())
    }

    fn file_metadata(&self, file: FileId) -> StorageResult<Option<Cow<'_, FileEntry>>>;

    fn visit_file_slots(
        &self,
        file: FileId,
        limit: BatchLimit,
        visit: &mut dyn FnMut(&[RecordSlot]) -> StorageResult<()>,
    ) -> StorageResult<()>;

    fn with_predicate_access(
        &self,
        _access: SegmentPredicateAccess<'_>,
        use_provider: &mut UseAccessProvider<'_>,
    ) -> StorageResult<()> {
        use_provider(None)
    }

    /// Borrow provider state for this access request. Optional families can
    /// decline without opening index contents.
    fn with_text_access(
        &self,
        _access: SegmentTextAccess<'_>,
        use_provider: &mut UseAccessProvider<'_>,
    ) -> StorageResult<()> {
        use_provider(None)
    }

    fn supports_posting_order(&self, _path: &FieldPath) -> StorageResult<bool> {
        Ok(false)
    }

    fn build_posting_order(
        &self,
        _request: PostingOrderRequest<'_>,
    ) -> StorageResult<Option<InferredOrderRows>> {
        Ok(None)
    }

    fn relation_count(&self) -> usize {
        0
    }

    fn relation(&self, _ordinal: usize) -> StorageResult<Option<&dyn RelationReader>> {
        Ok(None)
    }

    /// Return exact sources of an ID within the pinned relation. An absent
    /// reverse access is distinct from a present access with no matches.
    fn reverse_relation(
        &self,
        _ordinal: usize,
        _target: &str,
    ) -> StorageResult<Option<SlotSet<'_>>> {
        Ok(None)
    }

    fn reverse_relation_batch(
        &self,
        _ordinal: usize,
        _targets: &HashSet<String>,
    ) -> StorageResult<Option<RelationMatches<'_>>> {
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

/// A shared handle or a placed segment delegates data reads to its payload.
/// Placing a reusable segment can assign an address independently of how the
/// backend staged the data.
pub(crate) trait SegmentReaderDelegate: Send + Sync {
    type Payload: SegmentReader + ?Sized;

    fn payload(&self) -> &Self::Payload;

    fn placement_id(&self) -> Option<SegmentId> {
        None
    }

    fn payload_component<'a>(
        &self,
        component: &'a ComponentDescriptor<'_>,
        placement: SegmentId,
    ) -> StorageResult<ComponentDescriptor<'a>> {
        let payload = self.payload().descriptor();
        if component.collection != payload.collection || component.segment != placement {
            return Err(StorageError::invalid(
                "open placed component",
                "component belongs to a different segment",
            ));
        }
        Ok(ComponentDescriptor {
            collection: component.collection,
            segment: payload.id,
            kind: component.kind,
            path: component.path.as_deref().map(Cow::Borrowed),
            binding: component.binding.as_deref().map(Cow::Borrowed),
        })
    }
}

impl<S: SegmentReader + ?Sized> SegmentReaderDelegate for std::sync::Arc<S> {
    type Payload = S;

    fn payload(&self) -> &S {
        self.as_ref()
    }
}

impl<D: SegmentReaderDelegate + 'static> SegmentReader for D {
    fn maintenance(&self) -> Option<&dyn maintenance::SegmentMaintenance> {
        self.payload().maintenance()
    }

    fn descriptor(&self) -> SegmentDescriptor {
        let mut descriptor = self.payload().descriptor();
        if let Some(id) = self.placement_id() {
            descriptor.id = id;
        }
        descriptor
    }

    fn summary(&self, active_records: usize) -> NativeSegmentSummary {
        let mut summary = self.payload().summary(active_records);
        if let Some(id) = self.placement_id() {
            summary.id = id;
        }
        summary
    }

    fn estimates(&self) -> SegmentEstimates {
        self.payload().estimates()
    }

    fn record_directory_estimate(&self) -> RecordDirectoryEstimate {
        self.payload().record_directory_estimate()
    }

    fn observe_backing(&self, usage: &mut SegmentResourceUsage) {
        self.payload().observe_backing(usage);
    }

    fn index_family_estimate(&self) -> crate::lifecycle::IndexFamilyEstimate {
        self.payload().index_family_estimate()
    }

    fn components(&self) -> StorageResult<Vec<ComponentDescriptor<'_>>> {
        let mut components = self.payload().components()?;
        if let Some(id) = self.placement_id() {
            for component in &mut components {
                component.segment = id;
            }
        }
        Ok(components)
    }

    fn posting(
        &self,
        component: &ComponentDescriptor<'_>,
    ) -> StorageResult<Option<&dyn PostingReader>> {
        if let Some(placement) = self.placement_id() {
            return self
                .payload()
                .posting(&self.payload_component(component, placement)?);
        }
        self.payload().posting(component)
    }

    fn inferred_posting_diagnostic(
        &self,
        path: &[String],
    ) -> StorageResult<Option<InferredPostingDiagnostic>> {
        self.payload().inferred_posting_diagnostic(path)
    }

    fn facet_posting(
        &self,
        path: &FieldPath,
        binding: &str,
    ) -> StorageResult<Option<FacetPosting<'_>>> {
        self.payload().facet_posting(path, binding)
    }

    fn grams(&self, component: &ComponentDescriptor<'_>) -> StorageResult<Option<&dyn GramReader>> {
        if let Some(placement) = self.placement_id() {
            return self
                .payload()
                .grams(&self.payload_component(component, placement)?);
        }
        self.payload().grams(component)
    }

    fn order_columns(&self) -> StorageResult<Vec<&dyn OrderColumnReader>> {
        self.payload().order_columns()
    }

    fn active_fallback_count(&self, active: &ActiveRecords) -> u64 {
        self.payload().active_fallback_count(active)
    }

    fn slot_members(&self, fact: SegmentSlotFact) -> StorageResult<SlotSet<'_>> {
        self.payload().slot_members(fact)
    }

    fn source(&self) -> SegmentSource<'_> {
        self.payload().source()
    }

    fn raw_covers_logical_fields(&self) -> bool {
        self.payload().raw_covers_logical_fields()
    }

    fn read_header_batch(
        &self,
        start: usize,
        limit: BatchLimit,
    ) -> StorageResult<(Cow<'_, [RecordHeader]>, BatchStatus)> {
        self.payload().read_header_batch(start, limit)
    }

    fn backing_kind(&self) -> RecordBacking {
        self.payload().backing_kind()
    }

    fn slot_for_record(&self, id: &str) -> StorageResult<Option<RecordSlot>> {
        self.payload().slot_for_record(id)
    }

    fn write_version(&self, slot: RecordSlot) -> StorageResult<Option<RecordWriteVersion>> {
        self.payload().write_version(slot)
    }

    fn header(&self, slot: RecordSlot) -> StorageResult<Option<Cow<'_, RecordHeader>>> {
        self.payload().header(slot)
    }

    fn origin(&self, slot: RecordSlot) -> StorageResult<Option<RecordOrigin>> {
        self.payload().origin(slot)
    }

    fn retained_body(&self, slot: RecordSlot) -> StorageResult<Option<Cow<'_, RetainedBody>>> {
        self.payload().retained_body(slot)
    }

    fn file_count(&self) -> usize {
        self.payload().file_count()
    }

    fn read_file_batch<'a>(
        &'a self,
        start: usize,
        limit: BatchLimit,
        entries: &mut Vec<Cow<'a, FileEntry>>,
    ) -> StorageResult<BatchStatus> {
        self.payload().read_file_batch(start, limit, entries)
    }

    fn contains_file(&self, file: FileId) -> StorageResult<bool> {
        self.payload().contains_file(file)
    }

    fn file_metadata(&self, file: FileId) -> StorageResult<Option<Cow<'_, FileEntry>>> {
        self.payload().file_metadata(file)
    }

    fn visit_file_slots(
        &self,
        file: FileId,
        limit: BatchLimit,
        visit: &mut dyn FnMut(&[RecordSlot]) -> StorageResult<()>,
    ) -> StorageResult<()> {
        self.payload().visit_file_slots(file, limit, visit)
    }

    fn with_predicate_access(
        &self,
        access: SegmentPredicateAccess<'_>,
        use_provider: &mut UseAccessProvider<'_>,
    ) -> StorageResult<()> {
        self.payload().with_predicate_access(access, use_provider)
    }

    fn with_text_access(
        &self,
        access: SegmentTextAccess<'_>,
        use_provider: &mut UseAccessProvider<'_>,
    ) -> StorageResult<()> {
        self.payload().with_text_access(access, use_provider)
    }

    fn supports_posting_order(&self, path: &FieldPath) -> StorageResult<bool> {
        self.payload().supports_posting_order(path)
    }

    fn build_posting_order(
        &self,
        request: PostingOrderRequest<'_>,
    ) -> StorageResult<Option<InferredOrderRows>> {
        self.payload().build_posting_order(request)
    }

    fn relation_count(&self) -> usize {
        self.payload().relation_count()
    }

    fn relation(&self, ordinal: usize) -> StorageResult<Option<&dyn RelationReader>> {
        self.payload().relation(ordinal)
    }

    fn reverse_relation(&self, ordinal: usize, target: &str) -> StorageResult<Option<SlotSet<'_>>> {
        self.payload().reverse_relation(ordinal, target)
    }

    fn reverse_relation_batch(
        &self,
        ordinal: usize,
        targets: &HashSet<String>,
    ) -> StorageResult<Option<RelationMatches<'_>>> {
        self.payload().reverse_relation_batch(ordinal, targets)
    }

    fn order_column(&self, path: &FieldPath) -> StorageResult<Option<&dyn OrderColumnReader>> {
        self.payload().order_column(path)
    }

    fn stored_key(
        &self,
        path: &FieldPath,
        slot: RecordSlot,
    ) -> StorageResult<Option<Cow<'_, SortKey>>> {
        self.payload().stored_key(path, slot)
    }

    fn ordered_slots(
        &self,
        sort: &[SortField],
        scalar: Option<(&FieldPath, bool)>,
    ) -> StorageResult<Option<&dyn OrderedSlots>> {
        self.payload().ordered_slots(sort, scalar)
    }
}

#[cfg(test)]
mod tests;
