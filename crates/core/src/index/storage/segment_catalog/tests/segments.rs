//! A small independently stored segment built by the same lifecycle producers.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::*;
use crate::construction::batch::BuildWorkProfile;
use crate::index::storage::file_catalog::FileCatalogReader;
use crate::index::storage::posting::{
    FacetPosting, PostingCoverage, PostingDescriptor, PostingKey, PostingReader,
    PostingValueCursor, ScalarKey, VisitPostingBlock,
};
use crate::index::storage::segment::maintenance::{
    ProjectedSegment, RelinkedFile, SegmentMaintenance, SegmentPartition,
};
use crate::index::storage::segment::{
    FileRangeLocator, RecordBacking, RecordOrigin, RecordWriteVersion, RetainedBlock,
    SegmentDescriptor, SegmentEstimates, SegmentHandle, SegmentPolicy, SegmentPredicateAccess,
    SegmentSlotFact, SegmentSource, UseAccessProvider,
};
use crate::index::storage::text::{GramReader, SegmentTextAccess};
use crate::index::storage::worker::{PreparedIndexes, WorkerWriter};
use crate::inventory::{ByteRange, FileEntry, RecordClass};
use crate::model::{CollectionId, RecordId, RecordSlot};
use crate::query::binding::filter::{BoundTerm, Derived};
use crate::query::options::SortField;
use crate::query_access::contract::{
    AccessError, AccessExecution, AccessId, AccessKind, AccessOffer, AccessRequest, AccessResult,
    Coverage, IndexAccessProvider, InputEstimate, OfferList, OperationId, ProofStrength,
    ProviderId, Restriction,
};
use crate::query_access::membership::{DomainId, SlotInput, SlotSet};
use crate::query_predicate::TermId;
use crate::records::{RecordHeader, RecordVersion, RetainedBody};
use crate::repository::state::GlobalInferenceSummary;
use crate::repository::statistics::BigramConstructionStats;
use crate::source::{ExtractedRecord, TextDerivation};

#[path = "segments/transfer.rs"]
mod transfer;
pub(super) use transfer::import_file_segment;

const OP: &str = "independent segment table";
const SCORE: &str = "score";
const PROVIDER: ProviderId = ProviderId(191);

fn invalid(detail: impl Into<String>) -> StorageError {
    StorageError::invalid(OP, detail)
}

#[derive(Clone, Default)]
struct RowFacts {
    score: Option<ScalarKey>,
    empty_array: bool,
}

/// This fixture admits ordinary numeric score postings and leaves other
/// optional families to source verification.
pub(super) struct TableWorker {
    score_indexed: bool,
    rows: Vec<RowFacts>,
    logical: Vec<u32>,
}

impl TableWorker {
    pub(super) fn new(schema: &Schema) -> Self {
        Self {
            score_indexed: schema
                .fields
                .get(&[SCORE.to_owned()][..])
                .is_some_and(|field| {
                    field.indexed
                        && field.required
                        && !field.nullable
                        && !field.multi
                        && field.kind == LogicalType::Number
                }),
            rows: Vec::new(),
            logical: Vec::new(),
        }
    }
}

impl WorkerWriter for TableWorker {
    fn add_logical_indexes(
        &mut self,
        slot: RecordSlot,
        _id: &RecordId,
        extracted: &ExtractedRecord,
        _file: Option<(&FileEntry, Option<ByteRange>)>,
        _root: Option<&Path>,
        _text_derivation: TextDerivation,
        _sample: bool,
        _profile: &mut BuildWorkProfile,
    ) -> StorageResult<()> {
        if slot.0 as usize != self.rows.len() {
            return Err(invalid("worker logical slots are out of order"));
        }
        let score = if self.score_indexed {
            let values = crate::query::semantics::record::data_values_small(
                &extracted.value,
                &[SCORE.to_owned()],
            );
            match values.as_deref() {
                Some([Value::Number(score)]) => Some(ScalarKey::Number(score.clone())),
                _ => return Err(invalid("expected the declared scalar score")),
            }
        } else {
            None
        };
        self.logical.push(slot.0);
        self.rows.push(RowFacts {
            score,
            empty_array: crate::model::structure::contains_empty_array(&extracted.value),
        });
        Ok(())
    }

    fn add_fallback(
        &mut self,
        slot: RecordSlot,
        _id: &RecordId,
        _bytes: &[u8],
        _file: &FileEntry,
        _range: ByteRange,
        _root: &Path,
    ) -> StorageResult<()> {
        if slot.0 as usize != self.rows.len() {
            return Err(invalid("worker fallback slots are out of order"));
        }
        self.rows.push(RowFacts::default());
        Ok(())
    }

    fn finish(self) -> StorageResult<Box<dyn PreparedIndexes>> {
        Ok(Box::new(self))
    }
}

impl PreparedIndexes for TableWorker {
    fn logical_slots(&self) -> MembershipView<'_> {
        MembershipView::Sorted(&self.logical)
    }

    fn bigram_storage_stats(&self) -> BigramConstructionStats {
        BigramConstructionStats::default()
    }
}

fn worker(batch: Box<dyn PreparedIndexes>) -> StorageResult<TableWorker> {
    (batch as Box<dyn std::any::Any + Send>)
        .downcast::<TableWorker>()
        .map(|batch| *batch)
        .map_err(|_| invalid("unexpected worker representation"))
}

#[derive(Clone, Copy)]
enum Location {
    File(FileRangeLocator),
    Retained(u32),
    Versioned(RecordVersion),
}

#[derive(Clone)]
struct Row {
    header: RecordHeader,
    location: Location,
    facts: RowFacts,
    version: Option<RecordWriteVersion>,
}

#[derive(Clone)]
struct FileRow {
    metadata: FileEntry,
    slots: Vec<RecordSlot>,
}

#[derive(Clone)]
struct Header {
    id: SegmentId,
    collection: CollectionId,
    adapter: String,
    version: u32,
    registration: Option<u32>,
    backing: RecordBacking,
}

pub(super) struct TableBuilder {
    control: Arc<TableFactory>,
    header: Header,
    policy: SegmentPolicy,
    rows: Vec<Row>,
    files: BTreeMap<FileId, FileRow>,
    score_indexed: bool,
}

impl TableBuilder {
    pub(super) fn new(input: FileSegmentBuild<'_>, control: Arc<TableFactory>) -> Self {
        Self {
            control,
            header: Header {
                id: SegmentId::new(0),
                collection: input.collection,
                adapter: input.source.id().to_owned(),
                version: input.source.version(),
                registration: None,
                backing: RecordBacking::FileRange,
            },
            policy: input.policy,
            rows: Vec::new(),
            files: BTreeMap::new(),
            score_indexed: false,
        }
    }
}

impl SegmentBuilder for TableBuilder {
    fn file_count(&self) -> usize {
        self.files.len()
    }
    fn record_count(&self) -> usize {
        self.rows.len()
    }
    fn source_bytes(&self) -> u64 {
        self.files
            .values()
            .map(|file| file.metadata.revision.size)
            .sum()
    }
    fn at_capacity(&self) -> bool {
        self.record_count() >= self.policy.target_records.get()
            || self.file_count() >= self.policy.max_files.get()
    }
    fn can_admit(&self, batch: &BatchContribution) -> StorageResult<bool> {
        let new_files = batch
            .files
            .iter()
            .filter(|file| !self.files.contains_key(&file.entry.id))
            .count();
        Ok(self.policy.can_grow(
            self.record_count(),
            self.file_count(),
            batch.records.len(),
            new_files,
        ))
    }
    fn admit(
        &mut self,
        batch: BatchContribution,
        _: Option<&mut AdmissionBreakdown>,
    ) -> StorageResult<()> {
        if !self.can_admit(&batch)? {
            return Err(invalid("admission exceeds the draft policy"));
        }
        let facts = worker(batch.indexes)?;
        if facts.rows.len() != batch.records.len()
            || (!self.rows.is_empty() && self.score_indexed != facts.score_indexed)
        {
            return Err(invalid("worker record count or schema differs"));
        }
        self.score_indexed = facts.score_indexed;
        for file in batch.files {
            let id = file.entry.id;
            if let Some(previous) = self.files.get_mut(&id) {
                if previous.metadata.revision != file.entry.revision {
                    return Err(invalid("file revision changed within a draft"));
                }
                previous.metadata.status = previous.metadata.status.max(file.entry.status);
                for diagnostic in file.entry.diagnostics {
                    previous
                        .metadata
                        .record_diagnostic(diagnostic)
                        .map_err(invalid)?;
                }
                previous.metadata.omitted_diagnostics += file.entry.omitted_diagnostics;
            } else {
                self.files.insert(
                    id,
                    FileRow {
                        metadata: file.entry,
                        slots: Vec::new(),
                    },
                );
            }
        }
        for (record, facts) in batch.records.into_iter().zip(facts.rows) {
            let slot =
                RecordSlot(u32::try_from(self.rows.len()).map_err(|_| invalid("slot overflow"))?);
            let location = Location::File(FileRangeLocator {
                file: record.file,
                range: Some(record.range),
                revision: record.revision,
            });
            self.files
                .get_mut(&record.file)
                .ok_or_else(|| invalid("worker record file is missing"))?
                .slots
                .push(slot);
            let mut header = RecordHeader::from(&record);
            header.slot = slot;
            self.rows.push(Row {
                header,
                location,
                facts,
                version: None,
            });
        }
        self.control
            .file_batches_admitted
            .fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    fn seal(mut self: Box<Self>, input: SegmentSeal<'_>) -> StorageResult<BuiltSegment> {
        if self
            .control
            .fail_file_segment_seal
            .swap(false, Ordering::Relaxed)
        {
            return Err(invalid("injected independent file seal failure"));
        }
        self.header.id = input.id;
        let index = TableSegment::seal(
            self.header,
            self.rows,
            self.files,
            None,
            self.score_indexed,
            Arc::clone(&self.control),
        )?;
        self.control
            .file_segments_sealed
            .fetch_add(1, Ordering::Relaxed);
        Ok(BuiltSegment {
            index: Arc::new(index),
            observation: None,
        })
    }
}

pub(super) fn seal_direct(
    input: DirectSegmentBuild<'_>,
    seal: SegmentSeal<'_>,
    control: Arc<TableFactory>,
) -> StorageResult<BuiltSegment> {
    use crate::index::storage::segment::build::DirectSegmentBodies;
    let facts = worker(input.contribution.indexes)?;
    let count = input.headers.len();
    let (block, versions, backing) = match input.bodies {
        DirectSegmentBodies::Retained(block) if block.len() == count => {
            (Some(block), None, RecordBacking::Retained)
        }
        DirectSegmentBodies::Versioned(versions) if versions.len() == count => {
            (None, Some(versions), RecordBacking::Provider)
        }
        _ => return Err(invalid("direct backing is not aligned with headers")),
    };
    if facts.rows.len() != count || count == 0 {
        return Err(invalid("direct worker has incomplete record coverage"));
    }
    let rows = input
        .headers
        .into_iter()
        .zip(facts.rows)
        .zip(RecordWriteVersion::allocate(count)?)
        .map(|((header, facts), version)| {
            let location = versions.map_or(Location::Retained(header.slot.0), |versions| {
                Location::Versioned(versions[header.slot.0 as usize])
            });
            Row {
                header,
                location,
                facts,
                version: RecordWriteVersion::new(version),
            }
        })
        .collect();
    let header = Header {
        id: seal.id,
        collection: input.collection,
        adapter: input.source_id.into(),
        version: input.source_version,
        registration: Some(input.registration),
        backing,
    };
    let index = TableSegment::seal(
        header,
        rows,
        BTreeMap::new(),
        block,
        facts.score_indexed,
        control,
    )?;
    Ok(BuiltSegment {
        index: Arc::new(index),
        observation: None,
    })
}

#[derive(Clone)]
struct TableSegment {
    metadata: Header,
    rows: Vec<Row>,
    by_id: BTreeMap<RecordId, RecordSlot>,
    identity_order: Arc<ShortOrder>,
    files: BTreeMap<FileId, FileRow>,
    retained: Option<RetainedBlock>,
    logical: Arc<[u32]>,
    empty_arrays: Arc<[u32]>,
    score: Option<TablePosting>,
    grams: BTreeMap<transfer::GramBinding, transfer::TableGram>,
    raw_covers_logical_fields: bool,
    control: Arc<TableFactory>,
}

impl TableSegment {
    fn seal(
        metadata: Header,
        rows: Vec<Row>,
        files: BTreeMap<FileId, FileRow>,
        retained: Option<RetainedBlock>,
        score_indexed: bool,
        control: Arc<TableFactory>,
    ) -> StorageResult<Self> {
        let mut by_id = BTreeMap::new();
        let mut logical = Vec::new();
        let mut empty_arrays = Vec::new();
        let mut scores = BTreeMap::<_, Vec<u32>>::new();
        for (index, row) in rows.iter().enumerate() {
            if row.header.slot.0 as usize != index
                || by_id
                    .insert(row.header.id.clone(), row.header.slot)
                    .is_some()
            {
                return Err(invalid("duplicate identity or misordered local slot"));
            }
            if row.header.class == RecordClass::Logical {
                logical.push(row.header.slot.0);
            }
            if row.facts.empty_array {
                empty_arrays.push(row.header.slot.0);
            }
            if let Some(key) = &row.facts.score {
                scores
                    .entry(key.clone())
                    .or_default()
                    .push(row.header.slot.0);
            }
        }
        let logical: Arc<[u32]> = logical.into();
        let score = score_indexed.then(|| TablePosting {
            path: vec![SCORE.into()],
            present: Arc::clone(&logical),
            coverage: PostingCoverage {
                multi: false,
                values_indexed: true,
                value_complete: true,
                length_complete: false,
                empty_string_complete: false,
                covered_records: logical.len() as u32,
                summary: None,
                legacy_present_equivalent: true,
                legacy_null_equivalent: true,
            },
            keys: scores
                .into_iter()
                .map(|(key, slots)| (key, slots.into()))
                .collect(),
        });
        let identity_order = Arc::new(control.short_order(by_id.values().copied().collect()));
        Ok(Self {
            metadata,
            rows,
            by_id,
            identity_order,
            files,
            retained,
            logical,
            empty_arrays: empty_arrays.into(),
            score,
            grams: BTreeMap::new(),
            raw_covers_logical_fields: false,
            control,
        })
    }

    fn projected(
        &self,
        active: &ActiveRecords,
        retain: &mut dyn FnMut(&FileEntry) -> bool,
    ) -> StorageResult<(Self, Vec<Option<RecordSlot>>)> {
        if !self.grams.is_empty() {
            return Err(invalid(
                "use the target block importer for transferred grams",
            ));
        }
        if active.slot_count() as usize != self.rows.len() {
            return Err(invalid("projection mask covers another slot domain"));
        }
        let mut files: BTreeMap<_, _> = self
            .files
            .iter()
            .filter(|(_, file)| retain(&file.metadata))
            .map(|(&id, file)| {
                (
                    id,
                    FileRow {
                        metadata: file.metadata.clone(),
                        slots: Vec::new(),
                    },
                )
            })
            .collect();
        let mut old_to_new = vec![None; self.rows.len()];
        let mut output = Vec::new();
        for row in &self.rows {
            if !active.contains(row.header.slot) {
                continue;
            }
            let mut row = row.clone();
            let slot = RecordSlot(
                u32::try_from(output.len()).map_err(|_| invalid("projected slot overflow"))?,
            );
            old_to_new[row.header.slot.0 as usize] = Some(slot);
            row.header.slot = slot;
            if let Location::File(locator) = row.location {
                files
                    .get_mut(&locator.file)
                    .ok_or_else(|| invalid("live file excluded from projection"))?
                    .slots
                    .push(slot);
            }
            output.push(row);
        }
        let payload = Self::seal(
            self.metadata.clone(),
            output,
            files,
            self.retained.clone(),
            self.score.is_some(),
            Arc::clone(&self.control),
        )?;
        Ok((payload, old_to_new))
    }
}

impl SegmentReader for TableSegment {
    fn maintenance(&self) -> Option<&dyn SegmentMaintenance> {
        Some(self)
    }

    fn descriptor(&self) -> SegmentDescriptor {
        SegmentDescriptor {
            id: self.metadata.id,
            collection: self.metadata.collection,
            slots: self.rows.len(),
        }
    }

    fn estimates(&self) -> SegmentEstimates {
        SegmentEstimates {
            source_bytes_estimated: self
                .files
                .values()
                .map(|file| file.metadata.revision.size)
                .sum(),
            ..Default::default()
        }
    }

    fn active_fallback_count(&self, active: &ActiveRecords) -> u64 {
        active
            .len()
            .saturating_sub(active.intersection_len(MembershipView::Sorted(&self.logical)))
    }

    fn slot_members(&self, fact: SegmentSlotFact) -> StorageResult<SlotSet<'_>> {
        let slots = match fact {
            SegmentSlotFact::Logical => &self.logical,
            SegmentSlotFact::EmptyArray => &self.empty_arrays,
        };
        SlotSet::from_sorted(Arc::clone(slots)).map_err(|error| invalid(error.to_string()))
    }

    fn source(&self) -> SegmentSource<'_> {
        SegmentSource {
            adapter: &self.metadata.adapter,
            version: self.metadata.version,
            registration: self.metadata.registration,
        }
    }

    fn raw_covers_logical_fields(&self) -> bool {
        self.raw_covers_logical_fields
    }

    fn read_header_batch(
        &self,
        start: usize,
        limit: BatchLimit,
    ) -> StorageResult<(Cow<'_, [RecordHeader]>, BatchStatus)> {
        let rows = self
            .rows
            .get(start..)
            .ok_or_else(|| invalid("header start is outside the table"))?;
        let mut headers = Vec::new();
        let mut bytes = 0;
        for row in rows.iter().take(2) {
            let size = size_of::<RecordHeader>() + row.header.id.0.len();
            if !limit.accepts(OP, headers.len(), bytes, size)? {
                break;
            }
            bytes += size;
            headers.push(row.header.clone());
        }
        let status = if headers.len() == rows.len() {
            BatchStatus::Done
        } else {
            BatchStatus::More
        };
        Ok((Cow::Owned(headers), status))
    }

    fn backing_kind(&self) -> RecordBacking {
        self.metadata.backing
    }

    fn slot_for_record(&self, id: &str) -> StorageResult<Option<RecordSlot>> {
        Ok(self.by_id.get(id).copied())
    }

    fn write_version(&self, slot: RecordSlot) -> StorageResult<Option<RecordWriteVersion>> {
        Ok(self.rows.get(slot.0 as usize).and_then(|row| row.version))
    }

    fn header(&self, slot: RecordSlot) -> StorageResult<Option<Cow<'_, RecordHeader>>> {
        Ok(self
            .rows
            .get(slot.0 as usize)
            .map(|row| Cow::Owned(row.header.clone())))
    }

    fn origin(&self, slot: RecordSlot) -> StorageResult<Option<RecordOrigin>> {
        self.rows
            .get(slot.0 as usize)
            .map(|row| {
                Ok(match row.location {
                    Location::File(locator) => RecordOrigin::FileRange(locator),
                    Location::Retained(_) => RecordOrigin::Retained(None),
                    Location::Versioned(version) => RecordOrigin::Provider {
                        registration: self
                            .metadata
                            .registration
                            .ok_or_else(|| invalid("provider registration missing"))?,
                        version,
                        file: None,
                    },
                })
            })
            .transpose()
    }

    fn retained_body(&self, slot: RecordSlot) -> StorageResult<Option<Cow<'_, RetainedBody>>> {
        if let Some(row) = self.rows.get(slot.0 as usize)
            && let Location::Retained(offset) = row.location
        {
            return self
                .retained
                .as_ref()
                .ok_or_else(|| invalid("retained block missing"))?
                .body(offset)
                .map(Some);
        }
        Ok(None)
    }

    fn file_count(&self) -> usize {
        self.files.len()
    }

    fn read_file_batch<'a>(
        &'a self,
        start: usize,
        limit: BatchLimit,
        output: &mut Vec<Cow<'a, FileEntry>>,
    ) -> StorageResult<BatchStatus> {
        if start > self.files.len() {
            return Err(invalid("file batch starts beyond last row"));
        }
        let maximum = limit
            .fixed_batch_len(OP, size_of::<Cow<'_, FileEntry>>())?
            .min(2);
        output.extend(
            self.files
                .values()
                .skip(start)
                .take(maximum)
                .map(|row| Cow::Borrowed(&row.metadata)),
        );
        Ok(if start + output.len() == self.files.len() {
            BatchStatus::Done
        } else {
            BatchStatus::More
        })
    }

    fn file_metadata(&self, file: FileId) -> StorageResult<Option<Cow<'_, FileEntry>>> {
        Ok(self
            .files
            .get(&file)
            .map(|file| Cow::Borrowed(&file.metadata)))
    }

    fn visit_file_slots(
        &self,
        file: FileId,
        limit: BatchLimit,
        visit: &mut dyn FnMut(&[RecordSlot]) -> StorageResult<()>,
    ) -> StorageResult<()> {
        if let Some(file) = self.files.get(&file) {
            let maximum = limit.fixed_batch_len(OP, size_of::<RecordSlot>())?.min(2);
            for slots in file.slots.chunks(maximum) {
                visit(slots)?;
            }
        }
        Ok(())
    }

    fn facet_posting(
        &self,
        path: &FieldPath,
        binding: &str,
    ) -> StorageResult<Option<FacetPosting<'_>>> {
        Ok(self
            .score
            .as_ref()
            .filter(|posting| binding == "default" && posting.path == path.segments)
            .map(|reader| FacetPosting {
                reader,
                inference_version: None,
                short_strings: None,
            }))
    }

    fn components(
        &self,
    ) -> StorageResult<Vec<crate::index::storage::component::ComponentDescriptor<'_>>> {
        use crate::index::storage::component::{ComponentDescriptor, ComponentKind};
        let mut components: Vec<_> = self
            .score
            .iter()
            .map(|posting| ComponentDescriptor {
                collection: self.metadata.collection,
                segment: self.metadata.id,
                kind: ComponentKind::DeclaredPosting,
                path: Some(Cow::Borrowed(&posting.path)),
                binding: None,
            })
            .collect();
        components.extend(
            self.grams
                .keys()
                .map(|key| key.component(self.descriptor())),
        );
        if !self.grams.is_empty() {
            for kind in [ComponentKind::Sorting, ComponentKind::Relations] {
                components.push(ComponentDescriptor {
                    collection: self.metadata.collection,
                    segment: self.metadata.id,
                    kind,
                    path: None,
                    binding: None,
                });
            }
        }
        Ok(components)
    }

    fn grams(
        &self,
        component: &crate::index::storage::component::ComponentDescriptor<'_>,
    ) -> StorageResult<Option<&dyn crate::index::storage::text::GramReader>> {
        if component.collection != self.metadata.collection || component.segment != self.metadata.id
        {
            return Err(invalid("gram descriptor belongs to a different payload"));
        }
        Ok(transfer::GramBinding::from_component(component)?
            .and_then(|key| self.grams.get(&key))
            .map(|reader| reader as &dyn crate::index::storage::text::GramReader))
    }

    fn posting(
        &self,
        component: &crate::index::storage::component::ComponentDescriptor<'_>,
    ) -> StorageResult<Option<&dyn PostingReader>> {
        use crate::index::storage::component::ComponentKind;
        if component.collection != self.metadata.collection || component.segment != self.metadata.id
        {
            return Err(invalid("posting descriptor belongs to a different payload"));
        }
        Ok(self
            .score
            .as_ref()
            .filter(|posting| {
                component.kind == ComponentKind::DeclaredPosting
                    && component.path.as_deref() == Some(&posting.path)
            })
            .map(|posting| posting as &dyn PostingReader))
    }

    fn with_predicate_access(
        &self,
        access: SegmentPredicateAccess<'_>,
        use_provider: &mut UseAccessProvider<'_>,
    ) -> StorageResult<()> {
        if let Some(posting) = &self.score {
            let provider = ScoreAccess {
                domain: access.text.domain,
                posting,
                check: self.control.as_ref(),
            };
            use_provider(Some(&provider))
        } else {
            use_provider(None)
        }
    }

    fn with_text_access(
        &self,
        access: SegmentTextAccess<'_>,
        use_provider: &mut UseAccessProvider<'_>,
    ) -> StorageResult<()> {
        let raw = self
            .grams
            .get(&transfer::GramBinding::Raw)
            .filter(|reader| {
                access.policy.schema.raw.is_none()
                    && self.logical.len() == self.rows.len()
                    && reader.descriptor().covered_records as usize == self.rows.len()
            });
        if let Some(raw) = raw {
            let provider = transfer::TableText {
                raw,
                domain: access.domain,
                control: self.control.as_ref(),
            };
            use_provider(Some(&provider))
        } else {
            use_provider(None)
        }
    }

    fn stored_key(
        &self,
        _path: &FieldPath,
        _slot: RecordSlot,
    ) -> StorageResult<Option<Cow<'_, SortKey>>> {
        Ok(None)
    }

    fn ordered_slots(
        &self,
        fields: &[SortField],
        inferred: Option<(&FieldPath, bool)>,
    ) -> StorageResult<Option<&dyn OrderedSlots>> {
        Ok((fields.is_empty() && inferred.is_none()).then_some(self.identity_order.as_ref()))
    }
}

impl SegmentMaintenance for TableSegment {
    fn project_active(
        &self,
        active: &ActiveRecords,
        max_slots: usize,
        retain_file: &mut dyn FnMut(&FileEntry) -> bool,
    ) -> StorageResult<ProjectedSegment> {
        if self.rows.len() > max_slots {
            return Err(invalid("active projection exceeds budget"));
        }
        let (payload, old_to_new) = self.projected(active, retain_file)?;
        Ok(ProjectedSegment {
            payload: Arc::new(payload),
            old_to_new,
        })
    }

    fn project_partitions(
        &self,
        partitions: &[SegmentPartition],
        catalog: &dyn FileCatalogReader,
        cancelled: &mut dyn FnMut() -> bool,
    ) -> StorageResult<Vec<SegmentHandle>> {
        partitions
            .iter()
            .map(|partition| {
                if cancelled() {
                    return Err(StorageError::Cancelled);
                }
                let (mut payload, _) = self.projected(&partition.slots, &mut |file| {
                    partition.files.contains(&file.id)
                })?;
                for file in payload.files.values_mut() {
                    if let Some(current) = catalog.file(file.metadata.id)? {
                        file.metadata.relative_path = current.relative_path.clone();
                    }
                }
                self.control
                    .independent_projections
                    .fetch_add(1, Ordering::Relaxed);
                Ok(Arc::new(payload) as SegmentHandle)
            })
            .collect()
    }

    fn remap_dense(&self, new_slots: &[RecordSlot]) -> StorageResult<SegmentHandle> {
        if !self.grams.is_empty() {
            return Err(invalid(
                "use the target block importer for transferred grams",
            ));
        }
        if new_slots.len() != self.rows.len()
            || new_slots.iter().copied().collect::<BTreeSet<_>>().len() != self.rows.len()
            || new_slots
                .iter()
                .any(|slot| slot.0 as usize >= self.rows.len())
        {
            return Err(invalid("invalid slot permutation"));
        }
        let mut rows = self.rows.clone();
        for (row, &slot) in rows.iter_mut().zip(new_slots) {
            row.header.slot = slot;
        }
        rows.sort_by_key(|row| row.header.slot);
        let mut files = self.files.clone();
        for file in files.values_mut() {
            for slot in &mut file.slots {
                *slot = new_slots[slot.0 as usize];
            }
            file.slots.sort_unstable();
        }
        Ok(Arc::new(Self::seal(
            self.metadata.clone(),
            rows,
            files,
            self.retained.clone(),
            self.score.is_some(),
            Arc::clone(&self.control),
        )?))
    }

    fn reconcile_final_metadata(
        &self,
        catalog: &dyn FileCatalogReader,
        _: Option<&GlobalInferenceSummary>,
    ) -> StorageResult<Option<SegmentHandle>> {
        let mut changed = None;
        for (&id, file) in &self.files {
            if let Some(current) = catalog.file(id)?
                && current.status != file.metadata.status
            {
                let payload = changed.get_or_insert_with(|| self.clone());
                payload
                    .files
                    .get_mut(&id)
                    .expect("copied file row")
                    .metadata = current.into_owned();
            }
        }
        Ok(changed.map(|payload| Arc::new(payload) as SegmentHandle))
    }

    fn relink_file(
        &self,
        _: FileId,
        _: Option<&FileEntry>,
        _: Option<&Path>,
        _: &str,
    ) -> StorageResult<Option<RelinkedFile>> {
        Ok(None)
    }
}

#[derive(Clone)]
struct TablePosting {
    path: Vec<String>,
    present: Arc<[u32]>,
    keys: BTreeMap<ScalarKey, Arc<[u32]>>,
    coverage: PostingCoverage,
}

impl PostingReader for TablePosting {
    fn descriptor(&self) -> PostingDescriptor<'_> {
        PostingDescriptor {
            path: &self.path,
            kind: &LogicalType::Number,
            coverage: self.coverage,
        }
    }

    fn members(&self, key: PostingKey<'_>) -> StorageResult<Option<SlotSet<'_>>> {
        let slots = match key {
            PostingKey::Presence => Some(&self.present),
            PostingKey::Value(key) => self.keys.get(key),
            PostingKey::Length(_) => None,
        };
        slots
            .map(|slots| {
                SlotSet::from_sorted(Arc::clone(slots)).map_err(|error| invalid(error.to_string()))
            })
            .transpose()
    }

    fn values<'a>(
        &'a self,
        lower: std::ops::Bound<ScalarKey>,
        upper: std::ops::Bound<ScalarKey>,
        descending: bool,
        selected: Option<MembershipView<'a>>,
    ) -> StorageResult<Box<dyn PostingValueCursor + 'a>> {
        let range = self.keys.range((lower, upper));
        let rows: Box<dyn Iterator<Item = (&ScalarKey, &Arc<[u32]>)> + Send> = if descending {
            Box::new(range.rev())
        } else {
            Box::new(range)
        };
        Ok(Box::new(TableValues { rows, selected }))
    }

    fn visit_blocks(
        &self,
        limit: BatchLimit,
        visit: &mut VisitPostingBlock<'_>,
    ) -> StorageResult<()> {
        let mut write = |key: PostingKey<'_>, slots: &[u32]| {
            if slots.is_empty() {
                limit.accepts(OP, 0, 0, key.bytes())?;
                return visit(key, &[]);
            }
            let maximum = limit
                .fixed_batch_len_after(OP, key.bytes(), size_of::<RecordSlot>())?
                .min(2);
            let mut scratch = Vec::new();
            for chunk in slots.chunks(maximum) {
                scratch.clear();
                scratch.extend(chunk.iter().copied().map(RecordSlot));
                visit(key, &scratch)?;
            }
            Ok(())
        };
        write(PostingKey::Presence, &self.present)?;
        for (key, slots) in &self.keys {
            write(PostingKey::Value(key), slots)?;
        }
        Ok(())
    }
}

struct TableValues<'a> {
    rows: Box<dyn Iterator<Item = (&'a ScalarKey, &'a Arc<[u32]>)> + Send + 'a>,
    selected: Option<MembershipView<'a>>,
}

impl PostingValueCursor for TableValues<'_> {
    fn next(&mut self) -> StorageResult<Option<(ScalarKey, u64)>> {
        Ok(self.rows.next().map(|(key, slots)| {
            let count = self.selected.map_or(slots.len() as u64, |selected| {
                selected.intersection_len(MembershipView::Sorted(slots))
            });
            (key.clone(), count)
        }))
    }
}

struct ScoreAccess<'a> {
    domain: DomainId,
    posting: &'a TablePosting,
    check: &'a TableFactory,
}

impl ScoreAccess<'_> {
    fn accepts(&self, term: &BoundTerm) -> bool {
        term.path.root == crate::model::PathRoot::Data
            && term.path.segments == self.posting.path
            && term.operator == crate::query::Operator::Eq
            && term.derived == Derived::None
            && term.binding_name == "default"
            && term.operation.is_none()
            && term.matcher.is_none()
            && term.related.is_none()
            && term.rhs.is_none()
            && !term.query_path.has_position
            && !term.query_path.has_explicit_array_step
            && matches!(term.literal, Value::Number(_))
    }

    fn slots(&self, term: &BoundTerm) -> &[u32] {
        ScalarKey::from_value(&term.literal)
            .and_then(|key| self.posting.keys.get(&key))
            .map_or(&[], |slots| slots)
    }
}

impl IndexAccessProvider for ScoreAccess<'_> {
    fn provider_id(&self) -> ProviderId {
        PROVIDER
    }

    fn propose(
        &self,
        term: TermId,
        request: AccessRequest<'_>,
        input: InputEstimate,
        offers: &mut OfferList,
    ) -> AccessResult<()> {
        if input.domain == self.domain
            && let AccessRequest::Structured(bound) = request
            && self.accepts(bound)
        {
            offers.push(AccessOffer::new(
                AccessId {
                    provider: PROVIDER,
                    operation: OperationId(0),
                },
                term,
                AccessKind::ExactMembership,
                Coverage::Complete,
                ProofStrength::Exact,
                (self.slots(bound).len() as u64).min(input.input_len),
                1.0,
                0.0,
                false,
                false,
            ));
        }
        Ok(())
    }

    fn apply<'a>(
        &self,
        _: TermId,
        request: AccessRequest<'_>,
        choice: AccessId,
        input: SlotInput<'a>,
        execution: &AccessExecution<'_>,
    ) -> AccessResult<Restriction<'a>> {
        execution.check()?;
        if choice.provider != PROVIDER
            || choice.operation != OperationId(0)
            || self.domain != input.domain()
        {
            return Err(AccessError::StaleHandle);
        }
        let AccessRequest::Structured(bound) = request else {
            return Err(AccessError::CapabilityUnavailable);
        };
        if !self.accepts(bound) {
            return Err(AccessError::CapabilityUnavailable);
        }
        self.check
            .independent_applications
            .fetch_add(1, Ordering::Relaxed);
        let filtered: Arc<[u32]> = self
            .slots(bound)
            .iter()
            .copied()
            .filter(|&slot| input.contains(slot))
            .collect();
        Ok(Restriction::exact(
            input.replace_slots(SlotSet::from_sorted(filtered)?),
        ))
    }
}

fn score_schema() -> Schema {
    let mut schema = Schema::default();
    schema
        .register_field(crate::schema::FieldDefinition {
            path: vec![SCORE.into()],
            kind: LogicalType::Number,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    schema
}

fn score(value: i64) -> Value {
    Value::Object(BTreeMap::from([(
        SCORE.into(),
        Value::Number(crate::model::Number::Integer(value.into())),
    )]))
}

fn score_query() -> crate::query::Query {
    crate::query::Query {
        collections: Vec::new(),
        fts: None,
        filter: Some(crate::query::parse_filter("score = 7").unwrap()),
        options: crate::query::QueryOptions {
            count: true,
            evidence: true,
            limit: 10,
            return_mode: ReturnMode::Logical,
            sort: vec![SortField::simple("@record.id", false)],
            offset: 0,
        },
    }
}

struct Reader(AtomicUsize);

impl crate::records::reader::RecordReader for Reader {
    fn read_projected(
        &self,
        key: &crate::records::RecordKey,
        expected: RecordVersion,
        _: &crate::records::reader::RecordProjection,
    ) -> Result<crate::records::reader::RecordRead, crate::records::reader::RecordReadError> {
        assert_eq!(key.record.0.as_ref(), "reader");
        assert_eq!(expected, RecordVersion::new(3));
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(crate::records::reader::RecordRead {
            version: expected,
            value: score(7),
            text: Vec::new(),
            original_bytes: None,
        })
    }
}

#[test]
fn independent_and_native_segments_share_the_real_query_and_ingestion_path() {
    let factory = Arc::new(TableFactory::default());
    factory.independent_segments.store(true, Ordering::Relaxed);
    let mut builder = Index::builder();
    builder.use_storage_factory(Arc::new(Arc::clone(&factory)));
    let collection = builder.collection("entries", score_schema()).unwrap();
    let retained = builder
        .register_records(collection, "table-retained", 1)
        .unwrap();
    let versioned = builder
        .register_records(collection, "table-versioned", 1)
        .unwrap();
    let reader = Arc::new(Reader(AtomicUsize::new(0)));
    builder.register_reader(versioned, reader.clone()).unwrap();
    let index = builder.build().unwrap();
    let session = index.ingestion();
    let retained = session.writer(retained).unwrap();
    let versioned = session.writer(versioned).unwrap();
    let mut batch = retained.batch();
    batch.insert("drop", score(2)).unwrap();
    batch.insert("retained", score(7)).unwrap();
    retained.submit(batch).unwrap().wait_published().unwrap();
    let mut batch = versioned.batch();
    batch
        .insert_from_reader("reader", score(7), RecordVersion::new(3))
        .unwrap();
    versioned.submit(batch).unwrap().wait_published().unwrap();
    factory.independent_segments.store(false, Ordering::Relaxed);
    let mut batch = retained.batch();
    batch.insert("native", score(7)).unwrap();
    retained.submit(batch).unwrap().wait_published().unwrap();
    let pinned = index.view().unwrap();
    assert!(pinned.repository().repository_root().is_none());
    let owned = pinned.segments(collection).unwrap();
    let table: Vec<_> = owned
        .iter()
        .filter_map(|segment| {
            (segment.index.as_ref() as &dyn std::any::Any).downcast_ref::<TableSegment>()
        })
        .collect();
    assert_eq!(table.len(), 2);
    assert_eq!(owned.len(), 3);
    assert_eq!(factory.direct_seals.load(Ordering::Relaxed), 3);
    assert_eq!(factory.worker_calls.load(Ordering::Relaxed), 3);
    let query = score_query();
    let actual = pinned.search(&query).unwrap();
    let reference = pinned.search_reference(&query).unwrap();
    assert_eq!(actual.count, Some(3));
    assert_eq!(actual.count, reference.count);
    assert_eq!(
        actual
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["native", "reader", "retained"]
    );
    for (actual, reference) in actual.hits.iter().zip(&reference.hits) {
        assert_eq!(actual.record_id, reference.record_id);
        assert_eq!(actual.matched_fields, reference.matched_fields);
        assert!(matches!((&actual.payload, &reference.payload),
            (Some(crate::query_results::Payload::Logical(left)), Some(crate::query_results::Payload::Logical(right))) if left == right && *left == score(7)));
    }
    assert!(factory.independent_applications.load(Ordering::Relaxed) > 0);
    let reads_before = reader.0.load(Ordering::Relaxed);
    assert!(reads_before > 0);
    let mut batch = retained.batch();
    batch.delete("drop").unwrap();
    retained.submit(batch).unwrap().wait_published().unwrap();
    assert_eq!(index.compact().unwrap().retired_slots_removed, 1);
    assert_eq!(reader.0.load(Ordering::Relaxed), reads_before);
    assert_eq!(factory.independent_projections.load(Ordering::Relaxed), 1);
    let current = index.view().unwrap();
    let old_route = pinned.0.record_routes[0]
        .route("retained")
        .unwrap()
        .unwrap();
    let new_route = current.0.record_routes[0]
        .route("retained")
        .unwrap()
        .unwrap();
    assert_eq!(old_route.slot, RecordSlot(1));
    assert_eq!(new_route.slot, RecordSlot(0));
    assert!(
        pinned
            .get_record("entries", "drop", ReturnMode::Logical)
            .unwrap()
            .is_some()
    );
    assert!(
        current
            .get_record("entries", "drop", ReturnMode::Logical)
            .unwrap()
            .is_none()
    );
    assert_eq!(current.search(&query).unwrap().count, actual.count);
    session.finish().unwrap();
}

#[test]
fn independent_initial_and_managed_files_publish_and_compact_without_source_reads() {
    use crate::repository::configuration::{CollectionDefinition, RepositoryOptions};
    use crate::source::test_support::{fixture_registry, line_format};
    let factory = Arc::new(TableFactory::default());
    factory.independent_segments.store(true, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "objekte-independent-file-segments-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, content) in [
        ("a.rows", "(\"score\"=7)\n(\"score\"=5)\n"),
        ("b.rows", "(\"score\"=7)\n(\"score\"=8)\n"),
    ] {
        std::fs::write(root.join("data").join(name), content).unwrap();
    }
    let mut definition = CollectionDefinition::folder("entries", "data", line_format());
    definition.schema = score_schema();
    let index = Index::start_build_with_factory(
        &root,
        vec![definition],
        RepositoryOptions::default(),
        fixture_registry(),
        Arc::new(Arc::clone(&factory)),
    )
    .unwrap()
    .wait_complete()
    .unwrap();
    let pinned = index.view().unwrap();
    assert_eq!(pinned.search(&score_query()).unwrap().count, Some(2));
    let group = pinned.segments(CollectionId(0)).unwrap();
    assert_eq!(group.len(), 1);
    assert!((group[0].index.as_ref() as &dyn std::any::Any).is::<TableSegment>());
    let mut descriptor = group[0].components().unwrap().pop().unwrap();
    assert_eq!(descriptor.segment, group[0].id);
    assert!(group[0].posting(&descriptor).unwrap().is_some());
    descriptor.segment = SegmentId::new(u64::MAX);
    assert!(group[0].posting(&descriptor).is_err());
    let failing = root.join("data/new.rows");
    std::fs::write(&failing, "(\"score\"=7)\n(\"score\"=9)\n").unwrap();
    factory
        .fail_file_segment_seal
        .store(true, Ordering::Relaxed);
    let error = index.add_file(&failing).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("injected independent file seal failure")
    );
    let still = index.view().unwrap();
    assert_eq!(still.revision(), pinned.revision());
    assert!(
        still
            .file_catalog()
            .unwrap()
            .file_by_path(Path::new("data/new.rows"))
            .unwrap()
            .is_none()
    );
    index.add_file(&failing).unwrap();
    let managed = index.view().unwrap().segments(CollectionId(0)).unwrap();
    assert!(
        managed
            .iter()
            .any(|segment| segment.id != segment.index.descriptor().id)
    );
    for segment in managed.iter() {
        let component = segment.components().unwrap().pop().unwrap();
        assert!(segment.posting(&component).unwrap().is_some());
    }
    assert_eq!(
        index.view().unwrap().search(&score_query()).unwrap().count,
        Some(3)
    );
    let files = pinned.file_catalog().unwrap();
    let a = files
        .file_by_path(Path::new("data/a.rows"))
        .unwrap()
        .unwrap();
    assert_eq!(pinned.file_record_count(a.id).unwrap(), Some(2));
    let b = files
        .file_by_path(Path::new("data/b.rows"))
        .unwrap()
        .unwrap();
    let ids = files.records_for_file(b.id).unwrap();
    assert_eq!(ids.len(), 2);
    index
        .delete_file(index.view().unwrap().file_handle(a.id).unwrap().unwrap())
        .unwrap();
    std::fs::remove_file(root.join("data/b.rows")).unwrap();
    let before_compaction = index.view().unwrap();
    factory.omit_projection.store(true, Ordering::Relaxed);
    assert!(
        index
            .compact()
            .unwrap_err()
            .to_string()
            .contains("partition count mismatch")
    );
    assert_eq!(
        index.view().unwrap().revision(),
        before_compaction.revision()
    );
    assert_eq!(before_compaction.file_record_count(b.id).unwrap(), Some(2));
    assert_eq!(index.compact().unwrap().retired_slots_removed, 2);
    assert!(factory.independent_projections.load(Ordering::Relaxed) > 0);
    let current = index.view().unwrap();
    assert_eq!(current.file_record_count(b.id).unwrap(), Some(2));
    assert_eq!(pinned.file_record_count(a.id).unwrap(), Some(2));
    let mut values =
        crate::repository::posting_values::PostingValuesRequest::new("entries", "score", 5);
    values.limit = 1;
    let page = current.posting_values(&values).unwrap();
    assert_eq!(
        page.values[0].value,
        ScalarKey::from_value(&Value::Number(crate::model::Number::Integer(7.into()))).unwrap()
    );
    assert_eq!(page.values[0].count, 2);
    assert!(page.continuation.is_some());
    assert!(factory.file_batches_admitted.load(Ordering::Relaxed) >= 3);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
