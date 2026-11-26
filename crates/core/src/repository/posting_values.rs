//! Read-only typed posting dictionaries and distinct record counts.

use std::collections::{BTreeMap, HashMap};
use std::ops::Bound::{self, Excluded, Included, Unbounded};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use roaring::RoaringBitmap;

use crate::index::storage::file_catalog::FileCatalogReader;
use crate::index::storage::posting::{
    PostingCoverage, PostingDescriptor, PostingKey, PostingReader, PostingValueCursor, ScalarKey,
    VisitPostingBlock,
};
use crate::index::storage::publication::PublishedSegment;
use crate::index::storage::segment::{
    RecordOrigin, SegmentReader, SegmentSlotFact, read_slot_members, visit_checked_file_slots,
    visit_segment_files,
};
use crate::index::storage::transfer::{BatchLimit, StorageError, StorageResult};
use crate::inference::{InferenceIndexStatus, InferenceMode};
use crate::inventory::{ActiveRecords, RecordClass};
use crate::lifecycle::IndexRevision;
use crate::model::{FieldPath, LogicalType, PathRoot, RecordId, Value};
use crate::query::Query;
use crate::query::semantics::record::data_values_small;
use crate::query_access::membership::{MembershipView, SlotSet};
use crate::repository::Repository;
use crate::repository::state::CollectionState;
use crate::schema::FieldDefinition;
use crate::source::ScanProjection;
use crate::source::access::RecordRangeReader;
use crate::source::preparation::prepare_query_record_with_execution;

static NEXT_SELECTION_ID: AtomicU64 = AtomicU64::new(1);
pub const MAX_POSTING_VALUES_PAGE: usize = 4096;

/// Inclusive or exclusive typed dictionary boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostingValueBound {
    pub value: ScalarKey,
    pub inclusive: bool,
}

/// Complete verified membership for one collection in one repository instance.
#[derive(Clone, Debug)]
pub struct ExactSelection(Arc<SelectionData>);

#[derive(Debug)]
struct SelectionData {
    id: u64,
    instance_id: u64,
    collection_index: usize,
    generation: u64,
    revision: Option<IndexRevision>,
    members: RoaringBitmap,
    segment_members: Vec<RoaringBitmap>,
}

impl ExactSelection {
    pub fn len(&self) -> u64 {
        self.0
            .segment_members
            .iter()
            .fold(self.0.members.len(), |total, members| {
                total.saturating_add(members.len())
            })
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// One bounded page over an indexed field's stored value representation.
#[derive(Clone, Debug)]
pub struct PostingValuesRequest {
    pub collection: String,
    pub field: String,
    pub binding: Option<String>,
    pub limit: usize,
    pub descending: bool,
    pub continuation: Option<PostingValuesCursor>,
    pub lower: Option<PostingValueBound>,
    pub upper: Option<PostingValueBound>,
    pub prefix: Option<String>,
    pub selection: Option<ExactSelection>,
}

impl PostingValuesRequest {
    pub fn new(collection: impl Into<String>, field: impl Into<String>, limit: usize) -> Self {
        Self {
            collection: collection.into(),
            field: field.into(),
            binding: None,
            limit,
            descending: false,
            continuation: None,
            lower: None,
            upper: None,
            prefix: None,
            selection: None,
        }
    }
}

/// One typed key and the number of distinct eligible records containing it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostingValueCount {
    pub value: ScalarKey,
    pub count: u64,
}

/// Domain and special-state totals for the selected posting representation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostingValuesCoverage {
    pub eligible_records: u64,
    pub unavailable_records: u64,
    pub selected_records: Option<u64>,
    pub present_records: u64,
    pub explicit_null_records: u64,
    /// Eligible records whose entire selected value stream is represented by
    /// this dictionary. For an inferred short-string path this can be partial.
    pub indexed_records: u64,
    /// Eligible records that may contain values absent from the dictionary.
    pub unresolved_records: u64,
    /// False means the dictionary is not a complete facet enumeration, even
    /// when no continuation is returned. Counts for listed keys remain exact.
    pub value_counts_complete: bool,
}

/// In-process exclusive key that belongs to one repository generation and binding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostingValuesCursor {
    instance_id: u64,
    collection_index: usize,
    generation: u64,
    revision: Option<IndexRevision>,
    field: Vec<String>,
    binding: String,
    binding_version: u32,
    descending: bool,
    lower: Option<PostingValueBound>,
    upper: Option<PostingValueBound>,
    prefix: Option<String>,
    selection_id: Option<u64>,
    last_key: ScalarKey,
}

/// Values in posting-key order, with a cursor only when another key exists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostingValuesPage {
    /// Listed-key counts are exact. When `coverage.value_counts_complete` is
    /// false, other values may exist outside this dictionary.
    pub values: Vec<PostingValueCount>,
    pub continuation: Option<PostingValuesCursor>,
    pub coverage: PostingValuesCoverage,
    pub visited_keys: u64,
}

struct ResolvedPosting<'a> {
    index: &'a dyn PostingReader,
    binding: String,
    binding_version: u32,
    indexed_records: Option<SlotSet<'a>>,
    native_prefix: bool,
}

struct InferredFacetPolicy {
    kind: LogicalType,
    short_string_limit: Option<usize>,
    binding_version: u32,
}

fn effective_facet_binding(
    collection: &CollectionState,
    request: &PostingValuesRequest,
    path: &FieldPath,
) -> String {
    request
        .binding
        .clone()
        .or_else(|| {
            collection
                .definition
                .schema
                .defaults
                .get(&path.segments)
                .cloned()
        })
        .unwrap_or_else(|| "default".into())
}

fn finalized_inferred_facet_policy(
    collection: &CollectionState,
    effective_binding: &str,
    path: &FieldPath,
    field: Option<&FieldDefinition>,
) -> Result<Option<InferredFacetPolicy>, String> {
    if effective_binding != "default" || field.is_some_and(|field| field.indexed) {
        return Ok(None);
    }
    let Some(summary) = &collection.global_inference else {
        return Ok(None);
    };
    let Some(field) = summary
        .report
        .fields
        .iter()
        .find(|field| field.path == path.segments)
    else {
        return Ok(None);
    };
    if summary.report.mode != InferenceMode::AutoIndex || !summary.report.complete {
        return Err("finalized inference does not admit an enumerable facet".into());
    }
    if !matches!(
        field.index_status,
        InferenceIndexStatus::Built | InferenceIndexStatus::Incompatible
    ) {
        return Err("finalized inference did not admit this field".into());
    }
    let diagnostic = summary
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.path == path.segments);
    let enumerable = diagnostic.is_some_and(|diagnostic| {
        diagnostic.value_dictionary_complete || diagnostic.short_string_candidates
    });
    if !enumerable {
        return Err("inferred field has no enumerable value dictionary".into());
    }
    let short_only = diagnostic.is_some_and(|diagnostic| {
        diagnostic.short_string_candidates && !diagnostic.value_dictionary_complete
    });
    let kind = if short_only {
        LogicalType::String
    } else {
        let mut value_kinds = field.observed_kinds.iter().filter(|kind| {
            !matches!(
                kind,
                LogicalType::Array(_) | LogicalType::Object | LogicalType::Unknown
            )
        });
        let first_kind = value_kinds.next().cloned();
        let kind = if value_kinds.next().is_none() {
            first_kind
        } else {
            None
        }
        .or_else(|| diagnostic.and_then(|diagnostic| diagnostic.kind.clone()));
        kind.ok_or("inferred field has no enumerable value family")?
    };
    let short_string_limit = diagnostic
        .filter(|diagnostic| diagnostic.short_string_candidates)
        .map(|_| summary.report.policy.short_string_max_utf8_bytes);
    Ok(Some(InferredFacetPolicy {
        kind,
        short_string_limit,
        binding_version: summary.report.policy.version,
    }))
}

fn resolve_inferred_representative<'a>(
    collection: &CollectionState,
    segments: &'a [Arc<PublishedSegment>],
    path: &FieldPath,
    policy: &InferredFacetPolicy,
) -> Result<ResolvedPosting<'a>, String> {
    for segment in segments {
        if segment.active.len() == 0 {
            continue;
        }
        let Some(resolved) = resolve_segment(
            collection,
            segment.as_ref(),
            path,
            "inferred",
            Some(policy.binding_version),
        )?
        else {
            continue;
        };
        let compatible_family =
            key_kind(resolved.index.descriptor().kind) == key_kind(&policy.kind);
        let enumerable = if policy.short_string_limit.is_some() {
            resolved.indexed_records.is_some()
        } else {
            resolved.index.descriptor().coverage.value_complete
        };
        if compatible_family && enumerable {
            return Ok(resolved);
        }
    }
    Err("no active segment has a compatible enumerable inferred value dictionary".into())
}

fn resolve<'a>(
    collection: &'a CollectionState,
    binding: &str,
    path: &FieldPath,
) -> Result<ResolvedPosting<'a>, String> {
    let field = collection.definition.schema.fields.get(&path.segments);
    if let Some(base) = &collection.base_records {
        let binding = if binding == "default" && !field.is_some_and(|field| field.indexed) {
            "inferred"
        } else {
            binding
        };
        return resolve_segment(collection, base.as_ref(), path, binding, None)?
            .ok_or_else(|| "base posting has no compatible value dictionary".into());
    }
    if binding == "default" && !field.is_some_and(|field| field.indexed) {
        #[cfg(test)]
        {
            let inference = collection
                .inference
                .as_ref()
                .filter(|state| {
                    state.report.mode == InferenceMode::AutoIndex && state.report.complete
                })
                .ok_or("field has no admitted inferred posting index")?;
            let index = inference
                .indexes
                .get(&path.segments)
                .ok_or("field has no admitted inferred posting index")?;
            if index.covered_records as u64 != collection.base_logical_count()? {
                return Err("incomplete inferred posting logical coverage".into());
            }
            if !index.value_complete
                && !inference.short_string_coverage.contains_key(&path.segments)
            {
                return Err("inferred field has no general value posting coverage".into());
            }
            return Ok(ResolvedPosting {
                index,
                binding: "inferred".into(),
                binding_version: inference.report.policy.version,
                indexed_records: inference
                    .short_string_coverage
                    .get(&path.segments)
                    .map(SlotSet::borrowed),
                native_prefix: true,
            });
        }
        #[cfg(not(test))]
        return Err("field has no admitted inferred posting index".into());
    }
    let field = field.ok_or("posting values require a registered field")?;
    if binding == "default" {
        debug_assert!(field.indexed);
        #[cfg(test)]
        let index = collection
            .postings
            .fields
            .get(&path.segments)
            .ok_or("posting index missing")?;
        #[cfg(not(test))]
        let index = collection
            .declared_postings
            .facet(path, binding)
            .ok_or("posting index missing")?;
        return Ok(ResolvedPosting {
            index,
            binding: binding.to_owned(),
            binding_version: 0,
            indexed_records: None,
            native_prefix: true,
        });
    }
    let key = (path.segments.clone(), binding.to_owned());
    let registration = collection
        .definition
        .schema
        .operations
        .get(&key)
        .ok_or("binding has no registered posting operation")?;
    if !registration.operation.posting() {
        return Err("binding has no posting index".into());
    }
    if !registration.operation.complete_coverage() {
        return Err("incomplete custom posting coverage".into());
    }
    #[cfg(test)]
    let index = &collection
        .custom_postings
        .get(&key)
        .ok_or("binding has no posting index")?
        .index;
    #[cfg(not(test))]
    let index = collection
        .declared_postings
        .facet(path, binding)
        .ok_or("binding has no posting index")?;
    Ok(ResolvedPosting {
        index,
        binding: binding.to_owned(),
        binding_version: registration.version,
        indexed_records: None,
        native_prefix: false,
    })
}

fn resolve_segment<'a>(
    collection: &CollectionState,
    segment: &'a dyn SegmentReader,
    path: &FieldPath,
    binding: &str,
    binding_version: Option<u32>,
) -> Result<Option<ResolvedPosting<'a>>, String> {
    let Some(facet) = segment.facet_posting(path, binding)? else {
        return Ok(None);
    };
    let actual_version = match binding {
        "inferred" => facet
            .inference_version
            .ok_or("inferred facet policy version missing")?,
        "default" => 0,
        _ => {
            collection
                .definition
                .schema
                .operations
                .get(&(path.segments.clone(), binding.to_owned()))
                .ok_or("segment posting binding missing")?
                .version
        }
    };
    if binding_version.is_some_and(|expected| expected != actual_version) {
        return Ok(None);
    }
    let logical = read_slot_members(segment, SegmentSlotFact::Logical)?;
    if facet.reader.descriptor().coverage.covered_records as usize != logical.view().len() {
        return Ok(None);
    }
    Ok(Some(ResolvedPosting {
        index: facet.reader,
        binding: binding.to_owned(),
        binding_version: actual_version,
        indexed_records: facet.short_strings.map(|(_, covered)| covered),
        native_prefix: binding == "default" || binding == "inferred",
    }))
}

/// Query-local dictionary for segments where an inferred facet requires source verification.
struct ScannedSegmentPosting {
    path: Vec<String>,
    kind: LogicalType,
    coverage: PostingCoverage,
    presence: RoaringBitmap,
    values: BTreeMap<ScalarKey, RoaringBitmap>,
    complete_values: RoaringBitmap,
}

impl ScannedSegmentPosting {
    fn add(&mut self, slot: u32, record: &Value, max_short_string: Option<usize>) {
        self.coverage.covered_records += 1;
        let Some(values) = data_values_small(record, &self.path) else {
            if max_short_string.is_none() {
                self.complete_values.insert(slot);
            }
            return;
        };
        if values.is_empty() {
            self.coverage.legacy_present_equivalent = false;
        } else {
            self.presence.insert(slot);
        }
        if (max_short_string.is_none() || !values.is_empty())
            && values
                .iter()
                .all(|value| fully_represented(value, max_short_string))
        {
            self.complete_values.insert(slot);
        }
        for value in &values {
            match value {
                Value::Null => {
                    self.coverage.legacy_null_equivalent &= values.len() == 1;
                    self.values.entry(ScalarKey::Null).or_default().insert(slot);
                }
                Value::Array(array) => {
                    for item in &array.values {
                        self.add_scalar(slot, item, max_short_string);
                    }
                }
                value => self.add_scalar(slot, value, max_short_string),
            }
        }
    }

    fn add_scalar(&mut self, slot: u32, value: &Value, max_short_string: Option<usize>) {
        if max_short_string
            .is_none_or(|limit| matches!(value, Value::String(text) if text.len() <= limit))
            && let Some(key) = ScalarKey::from_value(value)
        {
            self.values.entry(key).or_default().insert(slot);
        }
    }
}

struct ScannedValueCursor<'a> {
    rows: Box<dyn Iterator<Item = (&'a ScalarKey, &'a RoaringBitmap)> + Send + 'a>,
    within: Option<MembershipView<'a>>,
}

impl PostingValueCursor for ScannedValueCursor<'_> {
    fn next(&mut self) -> StorageResult<Option<(ScalarKey, u64)>> {
        let row = self.rows.find(|(key, _)| **key != ScalarKey::Null);
        Ok(row.map(|(key, slots)| {
            let count = self.within.map_or_else(
                || slots.len(),
                |within| within.intersection_len(MembershipView::Borrowed(slots)),
            );
            (key.clone(), count)
        }))
    }
}

impl PostingReader for ScannedSegmentPosting {
    fn descriptor(&self) -> PostingDescriptor<'_> {
        PostingDescriptor {
            path: &self.path,
            kind: &self.kind,
            coverage: self.coverage,
        }
    }

    fn members(&self, key: PostingKey<'_>) -> StorageResult<Option<SlotSet<'_>>> {
        let slots = match key {
            PostingKey::Presence => Some(&self.presence),
            PostingKey::Value(value) => self.values.get(value),
            PostingKey::Length(_) => None,
        };
        Ok(slots.map(SlotSet::borrowed))
    }

    fn values<'a>(
        &'a self,
        lower: Bound<ScalarKey>,
        upper: Bound<ScalarKey>,
        descending: bool,
        within: Option<MembershipView<'a>>,
    ) -> StorageResult<Box<dyn PostingValueCursor + 'a>> {
        let range = self.values.range((lower, upper));
        Ok(Box::new(ScannedValueCursor {
            rows: if descending {
                Box::new(range.rev())
            } else {
                Box::new(range)
            },
            within,
        }))
    }

    fn visit_blocks(&self, _: BatchLimit, _: &mut VisitPostingBlock<'_>) -> StorageResult<()> {
        Err(StorageError::invalid(
            "transfer posting blocks",
            "query-local facet dictionary is not a sealed index",
        ))
    }
}

fn fully_represented(value: &Value, max_short_string: Option<usize>) -> bool {
    match value {
        Value::Array(array) => {
            (max_short_string.is_none() || !array.values.is_empty())
                && array.values.iter().all(|value| {
                    !matches!(value, Value::Array(_) | Value::Object(_))
                        && fully_represented(value, max_short_string)
                })
        }
        Value::Null => max_short_string.is_none(),
        Value::String(text) => max_short_string.is_none_or(|limit| text.len() <= limit),
        value => max_short_string.is_none() && ScalarKey::from_value(value).is_some(),
    }
}

fn scan_inferred_segment(
    repo: &Repository,
    collection: &CollectionState,
    segment: &PublishedSegment,
    catalog: &dyn FileCatalogReader,
    path: &FieldPath,
    kind: &LogicalType,
    multi: bool,
    max_short_string: Option<usize>,
) -> Result<ScannedSegmentPosting, String> {
    let mut posting = ScannedSegmentPosting {
        path: path.segments.clone(),
        kind: kind.clone(),
        coverage: PostingCoverage {
            multi,
            values_indexed: true,
            value_complete: max_short_string.is_none(),
            length_complete: false,
            empty_string_complete: *kind == LogicalType::String && !multi,
            covered_records: 0,
            summary: None,
            legacy_present_equivalent: true,
            legacy_null_equivalent: true,
        },
        presence: RoaringBitmap::new(),
        values: BTreeMap::new(),
        complete_values: RoaringBitmap::new(),
    };
    let mut ranges = Vec::new();
    visit_segment_files(segment, |file| {
        const OP: &str = "read inferred facet file slots";
        ranges.clear();
        visit_checked_file_slots(segment, file.id, &mut |slots| {
            for &slot in slots.iter().filter(|slot| segment.active.contains(**slot)) {
                let record = segment
                    .header(slot)?
                    .ok_or_else(|| StorageError::invalid(OP, "record header missing"))?;
                let range = segment
                    .origin(slot)?
                    .and_then(RecordOrigin::file)
                    .filter(|locator| locator.file == file.id)
                    .and_then(|locator| locator.range)
                    .ok_or_else(|| StorageError::invalid(OP, "record range missing"))?;
                if record.class == RecordClass::Logical {
                    ranges.push((slot.0, range));
                }
            }
            Ok(())
        })?;
        if ranges.is_empty() {
            return Ok::<_, String>(());
        }
        let current = catalog
            .file(file.id)?
            .ok_or("active inferred segment file retired")?;
        if current.relative_path != file.relative_path || current.revision != file.revision {
            return Err("active inferred segment file changed without retirement".into());
        }
        let mut reader = RecordRangeReader::open_for_search(
            &repo.required_file_root().join(&current.relative_path),
            current.revision,
            None,
        )?
        .with_backend(collection.required_file_reader().reader_handle());
        for batch in ranges.chunks(128) {
            for (slot, bytes) in reader.read(batch)?.0 {
                let value = prepare_query_record_with_execution(
                    RecordClass::Logical,
                    &collection.required_file_reader(),
                    &collection.definition.schema,
                    &bytes,
                    &ScanProjection::full(false),
                    false,
                    None,
                )?
                .value;
                posting.add(slot, &value, max_short_string);
            }
        }
        Ok(())
    })?;
    Ok(posting)
}

fn legacy_facts(value: &Value, path: &[String]) -> (bool, bool) {
    let selected = crate::query::semantics::record::data_values_small(value, path);
    let present = selected.is_some();
    let singleton_null = selected
        .as_deref()
        .is_some_and(|values| matches!(values, [Value::Null]));
    (present, singleton_null)
}

fn verify_legacy_facts(
    repo: &Repository,
    collection: &CollectionState,
    catalog: &dyn FileCatalogReader,
    path: &[String],
    operation: Option<&dyn crate::schema::operation::BindingOperation>,
    records: impl Iterator<
        Item = (
            u32,
            crate::model::FileId,
            crate::inventory::ByteRange,
            crate::inventory::SourceRevision,
        ),
    >,
) -> Result<(u64, u64), String> {
    let mut by_file = HashMap::<
        crate::model::FileId,
        Vec<(
            u32,
            crate::inventory::ByteRange,
            crate::inventory::SourceRevision,
        )>,
    >::new();
    for (slot, file, range, revision) in records {
        by_file
            .entry(file)
            .or_default()
            .push((slot, range, revision));
    }
    let mut present = 0u64;
    let mut singleton_null = 0u64;
    for (file_id, mut entries) in by_file {
        let current = catalog
            .file(file_id)?
            .ok_or("legacy count verification file is not active")?;
        if entries
            .iter()
            .any(|(_, _, revision)| *revision != current.revision)
        {
            return Err("legacy count verification source revision changed".into());
        }
        let mut reader = RecordRangeReader::open_for_search(
            &repo.required_file_root().join(&current.relative_path),
            current.revision,
            None,
        )?
        .with_backend(collection.required_file_reader().reader_handle());
        entries.sort_unstable_by_key(|(slot, _, _)| *slot);
        for batch in entries.chunks(128) {
            let ranges = batch
                .iter()
                .map(|(slot, range, _)| (*slot, *range))
                .collect::<Vec<_>>();
            for (_, bytes) in reader.read(&ranges)?.0 {
                let value = prepare_query_record_with_execution(
                    RecordClass::Logical,
                    &collection.required_file_reader(),
                    &collection.definition.schema,
                    &bytes,
                    &ScanProjection::full(false),
                    false,
                    None,
                )?
                .value;
                let (is_present, is_singleton_null) = if let Some(operation) = operation {
                    let selected = crate::query::semantics::record::data_values_small(&value, path);
                    let mapped = selected.and_then(|values| {
                        values
                            .into_iter()
                            .map(|value| {
                                crate::construction::batch::map_binding_value(operation, value)
                            })
                            .collect::<Option<Vec<_>>>()
                    });
                    (
                        mapped.is_some(),
                        mapped.is_some_and(|values| {
                            values.len() == 1 && matches!(values.first(), Some(Value::Null))
                        }),
                    )
                } else {
                    legacy_facts(&value, path)
                };
                present = present.saturating_add(u64::from(is_present));
                singleton_null = singleton_null.saturating_add(u64::from(is_singleton_null));
            }
        }
    }
    Ok((present, singleton_null))
}

fn posting_members(
    reader: &dyn PostingReader,
    key: PostingKey<'_>,
    slot_count: usize,
    domain: &RoaringBitmap,
) -> Result<RoaringBitmap, String> {
    let Some(members) = reader.members(key)? else {
        if key == PostingKey::Presence {
            return Err("posting presence membership missing".into());
        }
        return Ok(RoaringBitmap::new());
    };
    let view = members
        .explicit_view()
        .ok_or("invalid posting membership")?;
    if view.last().is_some_and(|slot| slot as usize >= slot_count) {
        return Err("posting member outside the slot domain".into());
    }
    Ok(view.intersect_bitmap(domain))
}

fn coverage_members<'a>(
    covered: &'a SlotSet<'_>,
    slots: usize,
) -> Result<MembershipView<'a>, String> {
    let view = covered
        .explicit_view()
        .ok_or("invalid facet coverage membership")?;
    if view.last().is_some_and(|slot| slot as usize >= slots) {
        return Err("facet coverage outside the slot domain".into());
    }
    Ok(view)
}

fn key_kind(kind: &LogicalType) -> &LogicalType {
    match kind {
        LogicalType::Array(inner) => key_kind(inner),
        other => other,
    }
}

fn compatible(kind: &LogicalType, key: &ScalarKey) -> bool {
    matches!(
        (key_kind(kind), key),
        (LogicalType::String, ScalarKey::String(_))
            | (LogicalType::Id, ScalarKey::Id(_))
            | (LogicalType::Number, ScalarKey::Number(_))
            | (LogicalType::Bool, ScalarKey::Bool(_))
            | (LogicalType::Date, ScalarKey::Date(_))
            | (LogicalType::Datetime, ScalarKey::Datetime(_))
            | (LogicalType::Unixtime, ScalarKey::Unixtime(_))
    )
}

fn prefix_successor(prefix: &str) -> Option<String> {
    let mut chars: Vec<char> = prefix.chars().collect();
    for index in (0..chars.len()).rev() {
        let mut next = chars[index] as u32 + 1;
        if next > char::MAX as u32 {
            continue;
        }
        if (0xD800..=0xDFFF).contains(&next) {
            next = 0xE000;
        }
        chars[index] = char::from_u32(next)?;
        chars.truncate(index + 1);
        return Some(chars.into_iter().collect());
    }
    None
}

fn request_bound(bound: &Option<PostingValueBound>) -> Bound<ScalarKey> {
    match bound {
        Some(bound) if bound.inclusive => Included(bound.value.clone()),
        Some(bound) => Excluded(bound.value.clone()),
        None => Unbounded,
    }
}

fn bound_key(bound: &Bound<ScalarKey>) -> Option<(&ScalarKey, bool)> {
    match bound {
        Included(key) => Some((key, false)),
        Excluded(key) => Some((key, true)),
        Unbounded => None,
    }
}

fn tighten_lower(a: Bound<ScalarKey>, b: Bound<ScalarKey>) -> Bound<ScalarKey> {
    match (bound_key(&a), bound_key(&b)) {
        (None, _) => b,
        (_, None) => a,
        (Some((ak, ax)), Some((bk, bx))) => {
            if ak > bk || (ak == bk && (ax || !bx)) {
                a
            } else {
                b
            }
        }
    }
}

fn tighten_upper(a: Bound<ScalarKey>, b: Bound<ScalarKey>) -> Bound<ScalarKey> {
    match (bound_key(&a), bound_key(&b)) {
        (None, _) => b,
        (_, None) => a,
        (Some((ak, ax)), Some((bk, bx))) => {
            if ak < bk || (ak == bk && (ax || !bx)) {
                a
            } else {
                b
            }
        }
    }
}

fn empty_range(lower: &Bound<ScalarKey>, upper: &Bound<ScalarKey>) -> bool {
    match (bound_key(lower), bound_key(upper)) {
        (Some((lo, lo_exclusive)), Some((hi, hi_exclusive))) => {
            lo > hi || (lo == hi && (lo_exclusive || hi_exclusive))
        }
        _ => false,
    }
}

pub(crate) fn select_exact_segmented(
    repo: &Repository,
    collection: &str,
    query: &Query,
    revision: IndexRevision,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &dyn FileCatalogReader,
) -> Result<ExactSelection, String> {
    let (collection_index, state) = repo
        .collections
        .iter()
        .enumerate()
        .find(|(_, state)| state.definition.name == collection)
        .ok_or("unknown collection")?;
    if !query.collections.is_empty() && !query.collections.iter().any(|name| name == collection) {
        return Err("selection query excludes requested collection".into());
    }
    crate::query::sort_binding::BoundSort::bind(repo, query)?;
    let mut request = query.resolve_current_clock()?.into_owned();
    request.collections = vec![collection.to_owned()];
    request.options.count = true;
    request.options.offset = 0;
    request.options.sort.clear();
    request.options.evidence = false;
    request.options.return_mode = crate::query::options::ReturnMode::Locator;
    request.options.limit = segments[collection_index]
        .iter()
        .try_fold(
            base_active[collection_index].len() as usize,
            |total, segment| total.checked_add(segment.active.len() as usize),
        )
        .ok_or("selection domain too large")?;
    if request.fts.is_none() && base_active[collection_index].len() == 0 {
        let prepared = crate::query::prepared::prepare(repo, &request)?;
        let filter = prepared
            .collections
            .iter()
            .find(|entry| entry.collection_index == collection_index)
            .and_then(|entry| entry.filter());
        let mut segment_members = vec![RoaringBitmap::new(); segments[collection_index].len()];
        let mut exact = true;
        for (segment_index, segment) in segments[collection_index].iter().enumerate() {
            let members = if let Some(filter) = filter {
                let bounds =
                    crate::query_execution::restrictions::segment_filter_bounds_with_catalog(
                        segment.id,
                        segment.as_ref(),
                        &segment.active,
                        filter,
                        state,
                        Some(catalog),
                    )?;
                if bounds.may == bounds.must {
                    bounds.must.as_ref().clone()
                } else if filter_excludes_raw_fallback(filter) {
                    let logical = read_slot_members(segment, SegmentSlotFact::Logical)?;
                    if !crate::query_execution::runner::same_proven_members(&bounds, logical.view())
                    {
                        exact = false;
                        break;
                    }
                    logical.view().intersect_bitmap(&bounds.must)
                } else {
                    exact = false;
                    break;
                }
            } else {
                segment.active.bitmap()
            };
            segment_members[segment_index] = members;
        }
        if exact {
            return Ok(ExactSelection(Arc::new(SelectionData {
                id: NEXT_SELECTION_ID.fetch_add(1, Ordering::Relaxed),
                instance_id: repo.instance_id,
                collection_index,
                generation: state.base_generation(),
                revision: Some(revision),
                members: RoaringBitmap::new(),
                segment_members,
            })));
        }
    }
    let result = crate::query_execution::runner::search_segmented(
        repo,
        &request,
        base_active,
        segments,
        catalog,
        revision,
    )?;
    if result.count != Some(result.hits.len()) {
        return Err("exact selection result incomplete".into());
    }
    let mut members = RoaringBitmap::new();
    let mut segment_members = vec![RoaringBitmap::new(); segments[collection_index].len()];
    for hit in result.hits {
        let id = RecordId(hit.record_id.into());
        if let Some(slot) = state.base_slot_for_record(id.0.as_ref())?
            && base_active[collection_index].contains(slot)
        {
            members.insert(slot.0);
            continue;
        }
        let mut found = false;
        for (index, segment) in segments[collection_index].iter().enumerate() {
            if let Some(slot) = segment.slot_for_record(id.0.as_ref())?
                && segment.active.contains(slot)
            {
                if found {
                    return Err("duplicate active selection ID".into());
                }
                segment_members[index].insert(slot.0);
                found = true;
            }
        }
        if !found {
            return Err("exact selection hit no longer resolves".into());
        }
    }
    Ok(ExactSelection(Arc::new(SelectionData {
        id: NEXT_SELECTION_ID.fetch_add(1, Ordering::Relaxed),
        instance_id: repo.instance_id,
        collection_index,
        generation: state.base_generation(),
        revision: Some(revision),
        members,
        segment_members,
    })))
}

fn filter_excludes_raw_fallback(expr: &crate::query::binding::filter::BoundExpr) -> bool {
    match expr {
        crate::query::binding::filter::BoundExpr::Term(term) => {
            term.path.root == crate::model::PathRoot::Data
                && term.related.is_none()
                && !matches!(
                    term.operator,
                    crate::query::Operator::Contains | crate::query::Operator::NotContains
                )
        }
        crate::query::binding::filter::BoundExpr::And(parts) => {
            parts.iter().all(filter_excludes_raw_fallback)
        }
        crate::query::binding::filter::BoundExpr::Or(_)
        | crate::query::binding::filter::BoundExpr::Not(_) => false,
    }
}

pub(crate) fn posting_values_segmented(
    repo: &Repository,
    request: &PostingValuesRequest,
    revision: IndexRevision,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &dyn FileCatalogReader,
) -> Result<PostingValuesPage, String> {
    let (collection_index, collection) = repo
        .collections
        .iter()
        .enumerate()
        .find(|(_, collection)| collection.definition.name == request.collection)
        .ok_or("unknown collection")?;
    if base_active.len() != repo.collections.len() || segments.len() != repo.collections.len() {
        return Err("posting value collection count mismatch".into());
    }
    if request.limit > MAX_POSTING_VALUES_PAGE {
        return Err("posting value page limit exceeds maximum".into());
    }
    let path = FieldPath::parse(&request.field)?;
    if path.root != PathRoot::Data {
        return Err("posting values require a logical data field".into());
    }
    let effective_binding = effective_facet_binding(collection, request, &path);
    let mut finalized_policy = if base_active[collection_index].len() == 0 {
        finalized_inferred_facet_policy(
            collection,
            &effective_binding,
            &path,
            collection.definition.schema.fields.get(&path.segments),
        )?
    } else {
        None
    };
    if let Some(policy) = &mut finalized_policy {
        for segment in segments[collection_index].iter() {
            if let Some((limit, _)) = segment
                .facet_posting(&path, "inferred")?
                .and_then(|facet| facet.short_strings)
            {
                policy.kind = LogicalType::String;
                policy.short_string_limit = Some(limit);
                break;
            }
        }
    }
    let metadata = if let Some(policy) = &finalized_policy {
        resolve_inferred_representative(collection, &segments[collection_index], &path, policy)
    } else {
        match resolve(collection, &effective_binding, &path) {
            Ok(metadata) => Ok(metadata),
            Err(primary_error)
                if base_active[collection_index].len() == 0 && effective_binding == "default" =>
            {
                let mut representative = None;
                for segment in segments[collection_index]
                    .iter()
                    .filter(|segment| segment.active.len() != 0)
                {
                    representative =
                        resolve_segment(collection, segment.as_ref(), &path, "inferred", None)?;
                    if representative.is_some() {
                        break;
                    }
                }
                representative.ok_or(primary_error)
            }
            Err(error) => Err(error),
        }
    };
    let base = metadata?;
    if base.index.descriptor().coverage.covered_records as u64 != collection.base_logical_count()?
        && base_active[collection_index].len() != 0
    {
        return Err("incomplete posting logical coverage".into());
    }
    let binding = base.binding.clone();
    let binding_version = base.binding_version;
    let short_limit = (binding == "inferred"
        && base.indexed_records.is_some()
        && key_kind(base.index.descriptor().kind) == &LogicalType::String)
        .then_some(repo.options.inference.policy.short_string_max_utf8_bytes);
    let mut fallbacks = Vec::with_capacity(segments[collection_index].len());
    for segment in segments[collection_index].iter() {
        let missing = binding == "inferred"
            && segment.active.len() > 0
            && resolve_segment(
                collection,
                segment.as_ref(),
                &path,
                &binding,
                Some(binding_version),
            )?
            .is_none_or(|posting| {
                posting.index.descriptor().kind != base.index.descriptor().kind
                    || !posting.index.descriptor().coverage.value_complete
            });
        fallbacks.push(if missing {
            Some(scan_inferred_segment(
                repo,
                collection,
                segment,
                catalog,
                &path,
                base.index.descriptor().kind,
                base.index.descriptor().coverage.multi,
                short_limit,
            )?)
        } else {
            None
        });
    }
    let mut resolved = vec![base];
    for (segment_index, segment) in segments[collection_index].iter().enumerate() {
        if segment.active.len() == 0 {
            resolved.push(ResolvedPosting {
                index: resolved[0].index,
                binding: binding.clone(),
                binding_version,
                indexed_records: None,
                native_prefix: resolved[0].native_prefix,
            });
            continue;
        }
        if let Some(fallback) = &fallbacks[segment_index] {
            resolved.push(ResolvedPosting {
                index: fallback,
                binding: binding.clone(),
                binding_version,
                indexed_records: Some(SlotSet::borrowed(&fallback.complete_values)),
                native_prefix: true,
            });
            continue;
        }
        visit_segment_files(segment.as_ref(), |file| {
            let mut live = false;
            visit_checked_file_slots(segment, file.id, &mut |slots| {
                live |= slots.iter().any(|slot| segment.active.contains(*slot));
                Ok(())
            })?;
            if live {
                let current = catalog
                    .file(file.id)?
                    .ok_or("active segment posting file retired")?;
                if current.relative_path != file.relative_path || current.revision != file.revision
                {
                    return Err("active segment posting file changed without retirement".into());
                }
            }
            Ok::<_, String>(())
        })?;
        resolved.push(
            resolve_segment(
                collection,
                segment.as_ref(),
                &path,
                &binding,
                Some(binding_version),
            )?
            .ok_or("segment has no complete posting coverage for the binding")?,
        );
    }
    if resolved
        .iter()
        .any(|shard| shard.index.descriptor().kind != resolved[0].index.descriptor().kind)
    {
        return Err("segment posting key type changed".into());
    }
    let selection = request.selection.as_ref().map(|selection| &selection.0);
    let binding_operation = (!matches!(binding.as_str(), "default" | "inferred"))
        .then(|| {
            collection
                .definition
                .schema
                .operations
                .get(&(path.segments.clone(), binding.clone()))
                .map(|registration| registration.operation.as_ref())
        })
        .flatten();
    if selection.is_some_and(|selection| {
        selection.instance_id != repo.instance_id
            || selection.collection_index != collection_index
            || selection.generation != collection.base_generation()
            || selection.revision != Some(revision)
            || selection.segment_members.len() != segments[collection_index].len()
    }) {
        return Err("exact selection scope mismatch".into());
    }
    for bound in [&request.lower, &request.upper].into_iter().flatten() {
        if !compatible(resolved[0].index.descriptor().kind, &bound.value) {
            return Err("posting value bound has incompatible type".into());
        }
    }
    let prepared_prefix = if let Some(prefix) = &request.prefix {
        if key_kind(resolved[0].index.descriptor().kind) != &LogicalType::String {
            return Err("posting prefix requires a string field".into());
        }
        if resolved[0].native_prefix {
            Some(prefix.clone())
        } else {
            let key = (path.segments.clone(), binding.clone());
            Some(
                collection.definition.schema.operations[&key]
                    .operation
                    .prepare_posting_prefix(prefix)?
                    .ok_or("binding does not preserve prefix semantics")?,
            )
        }
    } else {
        None
    };
    let mut domains = Vec::with_capacity(resolved.len());
    domains.push(base_active[collection_index].bitmap());
    for segment in segments[collection_index].iter() {
        domains.push(segment.active.bitmap());
    }
    if let Some(selection) = selection {
        domains[0] &= &selection.members;
        for (domain, members) in domains.iter_mut().skip(1).zip(&selection.segment_members) {
            *domain &= members;
        }
    }
    let mut eligible = 0u64;
    let mut present = 0u64;
    let mut nulls = 0u64;
    let mut indexed = 0u64;
    let mut selected_total = 0u64;
    for (index, shard) in resolved.iter().enumerate() {
        let domain = &domains[index];
        if domain.is_empty() {
            continue;
        }
        let logical_domain = if index == 0 {
            collection
                .base_slot_members(crate::index::storage::segment::SegmentSlotFact::Logical)?
                .view()
                .intersect_bitmap(&domain)
        } else {
            read_slot_members(
                segments[collection_index][index - 1].as_ref(),
                SegmentSlotFact::Logical,
            )?
            .view()
            .intersect_bitmap(domain)
        };
        selected_total += domain.len();
        eligible += logical_domain.len();
        let slot_count = if index == 0 {
            collection.base_slot_count()
        } else {
            segments[collection_index][index - 1].descriptor().slots
        };
        let presence = posting_members(
            shard.index,
            PostingKey::Presence,
            slot_count,
            &logical_domain,
        )?;
        present += presence.len();
        let selected_nulls = posting_members(
            shard.index,
            PostingKey::Value(&ScalarKey::Null),
            slot_count,
            &logical_domain,
        )?;
        let coverage = shard.index.descriptor().coverage;
        if coverage.legacy_null_equivalent {
            nulls += selected_nulls.len();
        }
        if !coverage.legacy_present_equivalent || !coverage.legacy_null_equivalent {
            let present_candidates = if coverage.legacy_present_equivalent {
                RoaringBitmap::new()
            } else {
                &logical_domain - &presence
            };
            let records_for = |candidates: &RoaringBitmap| -> Result<Vec<_>, String> {
                let mut records = Vec::new();
                for slot in candidates.iter() {
                    if index == 0 {
                        let record = collection.base_record(crate::model::RecordSlot(slot))?;
                        records.push((slot, record.file, record.range, record.revision));
                    } else {
                        let segment = &segments[collection_index][index - 1];
                        let origin = segment
                            .origin(crate::model::RecordSlot(slot))?
                            .ok_or("legacy facet record origin missing")?;
                        if let Some(locator) = origin.file()
                            && let Some(range) = locator.range
                        {
                            records.push((slot, locator.file, range, locator.revision));
                        }
                    }
                }
                Ok(records)
            };
            if !present_candidates.is_empty() {
                let (verified_present, _) = verify_legacy_facts(
                    repo,
                    collection,
                    catalog,
                    &path.segments,
                    binding_operation,
                    records_for(&present_candidates)?.into_iter(),
                )?;
                present += verified_present;
            }
            if !coverage.legacy_null_equivalent && !selected_nulls.is_empty() {
                let (_, verified_nulls) = verify_legacy_facts(
                    repo,
                    collection,
                    catalog,
                    &path.segments,
                    binding_operation,
                    records_for(&selected_nulls)?.into_iter(),
                )?;
                nulls += verified_nulls;
            }
        }
        indexed += match &shard.indexed_records {
            Some(covered) => coverage_members(covered, slot_count)?
                .intersection_len(MembershipView::Borrowed(domain)),
            None => logical_domain.len(),
        };
    }
    let unresolved = eligible.saturating_sub(indexed);
    let coverage = PostingValuesCoverage {
        eligible_records: eligible,
        unavailable_records: selected_total.saturating_sub(eligible),
        selected_records: selection.map(|_| selected_total),
        present_records: present,
        explicit_null_records: nulls,
        indexed_records: indexed,
        unresolved_records: unresolved,
        value_counts_complete: unresolved == 0,
    };
    let last = if let Some(cursor) = &request.continuation {
        if cursor.instance_id != repo.instance_id
            || cursor.collection_index != collection_index
            || cursor.generation != collection.base_generation()
            || cursor.revision != Some(revision)
            || cursor.field != path.segments
            || cursor.binding != binding
            || cursor.binding_version != binding_version
            || cursor.descending != request.descending
            || cursor.lower != request.lower
            || cursor.upper != request.upper
            || cursor.prefix != request.prefix
            || cursor.selection_id != selection.map(|selection| selection.id)
        {
            return Err("posting value continuation scope mismatch".into());
        }
        Some(&cursor.last_key)
    } else {
        None
    };
    if request.limit == 0 {
        return Ok(PostingValuesPage {
            values: Vec::new(),
            continuation: None,
            coverage,
            visited_keys: 0,
        });
    }
    let mut lower = request_bound(&request.lower);
    let mut upper = request_bound(&request.upper);
    if let Some(prefix) = &prepared_prefix
        && !prefix.is_empty()
    {
        lower = tighten_lower(lower, Included(ScalarKey::String(prefix.clone())));
        if let Some(end) = prefix_successor(prefix) {
            upper = tighten_upper(upper, Excluded(ScalarKey::String(end)));
        }
    }
    if let Some(last) = last {
        if request.descending {
            upper = tighten_upper(upper, Excluded(last.clone()));
        } else {
            lower = tighten_lower(lower, Excluded(last.clone()));
        }
    }
    if empty_range(&lower, &upper) {
        return Ok(PostingValuesPage {
            values: Vec::new(),
            continuation: None,
            coverage,
            visited_keys: 0,
        });
    }
    let mut iterators = resolved
        .iter()
        .zip(&domains)
        .map(|(shard, domain)| {
            if domain.is_empty() {
                return Ok(None);
            }
            shard
                .index
                .values(
                    lower.clone(),
                    upper.clone(),
                    request.descending,
                    Some(MembershipView::Borrowed(domain)),
                )
                .map(Some)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut heads = iterators
        .iter_mut()
        .map(|iterator| iterator.as_mut().map_or(Ok(None), |cursor| cursor.next()))
        .collect::<Result<Vec<_>, _>>()?;
    let mut values = Vec::new();
    let mut visited_keys = 0;
    let mut has_more = false;
    loop {
        let key = heads
            .iter()
            .filter_map(|head| head.as_ref().map(|(key, _)| key))
            .reduce(|current, next| {
                if (request.descending && next > current) || (!request.descending && next < current)
                {
                    next
                } else {
                    current
                }
            });
        let Some(key) = key.cloned() else { break };
        visited_keys += 1;
        let mut count = 0u64;
        for (index, head) in heads.iter_mut().enumerate() {
            if head
                .as_ref()
                .is_some_and(|(candidate, _)| *candidate == key)
            {
                count += head.as_ref().unwrap().1;
                *head = iterators[index]
                    .as_mut()
                    .expect("head belongs to a cursor")
                    .next()?;
            }
        }
        if count == 0 {
            continue;
        }
        if values.len() == request.limit {
            has_more = true;
            break;
        }
        values.push(PostingValueCount { value: key, count });
    }
    let continuation = if has_more {
        values.last().map(|entry| PostingValuesCursor {
            instance_id: repo.instance_id,
            collection_index,
            generation: collection.base_generation(),
            revision: Some(revision),
            field: path.segments,
            binding,
            binding_version,
            descending: request.descending,
            lower: request.lower.clone(),
            upper: request.upper.clone(),
            prefix: request.prefix.clone(),
            selection_id: selection.map(|selection| selection.id),
            last_key: entry.value.clone(),
        })
    } else {
        None
    };
    Ok(PostingValuesPage {
        values,
        continuation,
        coverage,
        visited_keys,
    })
}

impl Repository {
    pub fn select_exact(&self, collection: &str, query: &Query) -> Result<ExactSelection, String> {
        if self.has_native_segments() {
            return select_exact_segmented(
                self,
                collection,
                query,
                self.revision,
                &self.active,
                &self.segment_groups()?,
                &self.catalog,
            );
        }
        self.select_exact_with_active(collection, query, None, None)
    }

    pub(crate) fn select_exact_for_view(
        &self,
        collection: &str,
        query: &Query,
        revision: IndexRevision,
        active: &[ActiveRecords],
    ) -> Result<ExactSelection, String> {
        self.select_exact_with_active(collection, query, Some(revision), Some(active))
    }

    fn select_exact_with_active(
        &self,
        collection: &str,
        query: &Query,
        revision: Option<IndexRevision>,
        active: Option<&[ActiveRecords]>,
    ) -> Result<ExactSelection, String> {
        let (collection_index, state) = self
            .collections
            .iter()
            .enumerate()
            .find(|(_, state)| state.definition.name == collection)
            .ok_or("unknown collection")?;
        if !query.collections.is_empty() && !query.collections.iter().any(|name| name == collection)
        {
            return Err("selection query excludes requested collection".into());
        }
        let resolved = query.resolve_current_clock()?;
        let members = crate::query_execution::runner::exact_members_with_active(
            self,
            resolved.as_ref(),
            collection_index,
            active,
        )?;
        Ok(ExactSelection(Arc::new(SelectionData {
            id: NEXT_SELECTION_ID.fetch_add(1, Ordering::Relaxed),
            instance_id: self.instance_id,
            collection_index,
            generation: state.base_generation(),
            revision,
            members,
            segment_members: Vec::new(),
        })))
    }

    pub fn posting_values(
        &self,
        request: &PostingValuesRequest,
    ) -> Result<PostingValuesPage, String> {
        if self.has_native_segments() {
            return posting_values_segmented(
                self,
                request,
                self.revision,
                &self.active,
                &self.segment_groups()?,
                &self.catalog,
            );
        }
        self.posting_values_with_active(request, None, None)
    }

    pub(crate) fn posting_values_for_view(
        &self,
        request: &PostingValuesRequest,
        revision: IndexRevision,
        active: &[ActiveRecords],
    ) -> Result<PostingValuesPage, String> {
        self.posting_values_with_active(request, Some(revision), Some(active))
    }

    fn posting_values_with_active(
        &self,
        request: &PostingValuesRequest,
        revision: Option<IndexRevision>,
        active: Option<&[ActiveRecords]>,
    ) -> Result<PostingValuesPage, String> {
        let (collection_index, collection) = self
            .collections
            .iter()
            .enumerate()
            .find(|(_, collection)| collection.definition.name == request.collection)
            .ok_or("unknown collection")?;
        let path = FieldPath::parse(&request.field)?;
        if request.limit > MAX_POSTING_VALUES_PAGE {
            return Err("posting value page limit exceeds maximum".into());
        }
        if path.root != PathRoot::Data {
            return Err("posting values require a logical data field".into());
        }
        let effective_binding = effective_facet_binding(collection, request, &path);
        let resolved = resolve(collection, &effective_binding, &path)?;
        let ResolvedPosting {
            index,
            binding,
            binding_version,
            indexed_records,
            native_prefix,
        } = resolved;
        let descriptor = index.descriptor();
        if descriptor.coverage.covered_records as u64 != collection.base_logical_count()? {
            return Err("incomplete posting logical coverage".into());
        }
        let selection = request.selection.as_ref().map(|handle| &handle.0);
        if selection.is_some_and(|selection| {
            selection.instance_id != self.instance_id
                || selection.collection_index != collection_index
                || selection.generation != collection.base_generation()
                || selection.revision != revision
        }) {
            return Err("exact selection scope mismatch".into());
        }
        for bound in [&request.lower, &request.upper].into_iter().flatten() {
            if !compatible(descriptor.kind, &bound.value) {
                return Err("posting value bound has incompatible type".into());
            }
        }
        let prepared_prefix = if let Some(prefix) = &request.prefix {
            if key_kind(descriptor.kind) != &LogicalType::String {
                return Err("posting prefix requires a string field".into());
            }
            if native_prefix {
                Some(prefix.clone())
            } else {
                let key = (path.segments.clone(), binding.clone());
                Some(
                    collection.definition.schema.operations[&key]
                        .operation
                        .prepare_posting_prefix(prefix)?
                        .ok_or("binding does not preserve prefix semantics")?,
                )
            }
        } else {
            None
        };
        let live_domain = active.map(|domains| domains[collection_index].bitmap());
        let selected_domain = match (selection, live_domain) {
            (Some(selection), Some(live)) => Some(&selection.members & &live),
            (Some(selection), None) => Some(selection.members.clone()),
            (None, live) => live,
        };
        let selected_domain = selected_domain.as_ref();
        let selected_total = selection.map(|selection| selection.members.len());
        let logical = collection
            .base_slot_members(crate::index::storage::segment::SegmentSlotFact::Logical)?;
        let logical = logical.view();
        let eligible = selected_domain.map_or(logical.len() as u64, |selected| {
            logical.intersection_len(crate::query_access::membership::MembershipView::Borrowed(
                selected,
            ))
        });
        let live_total = active.map_or(collection.base_slot_count() as u64, |domains| {
            domains[collection_index].len()
        });
        let unavailable = selected_total.unwrap_or(live_total) - eligible;
        let indexed = match &indexed_records {
            Some(covered) => {
                let members = coverage_members(covered, collection.base_slot_count())?;
                selected_domain.map_or(members.len() as u64, |selected| {
                    members.intersection_len(MembershipView::Borrowed(selected))
                })
            }
            None => eligible,
        };
        let unresolved = eligible.saturating_sub(indexed);
        let logical_domain = selected_domain.map_or_else(
            || logical.bitmap_for_set_algebra().into_owned(),
            |selected| logical.intersect_bitmap(selected),
        );
        let slot_count = collection.base_slot_count();
        let presence = posting_members(index, PostingKey::Presence, slot_count, &logical_domain)?;
        let selected_nulls = posting_members(
            index,
            PostingKey::Value(&ScalarKey::Null),
            slot_count,
            &logical_domain,
        )?;
        let mut present_records = presence.len();
        let mut explicit_null_records = if descriptor.coverage.legacy_null_equivalent {
            selected_nulls.len()
        } else {
            0
        };
        let binding_operation = (!matches!(binding.as_str(), "default" | "inferred"))
            .then(|| {
                collection
                    .definition
                    .schema
                    .operations
                    .get(&(path.segments.clone(), binding.clone()))
                    .map(|registration| registration.operation.as_ref())
            })
            .flatten();
        if !descriptor.coverage.legacy_present_equivalent
            || !descriptor.coverage.legacy_null_equivalent
        {
            let records_for = |candidates: &RoaringBitmap| {
                candidates
                    .iter()
                    .map(|slot| {
                        let record = collection.base_record(crate::model::RecordSlot(slot))?;
                        Ok::<_, crate::index::storage::transfer::StorageError>((
                            slot,
                            record.file,
                            record.range,
                            record.revision,
                        ))
                    })
                    .collect::<Result<Vec<_>, _>>()
            };
            if !descriptor.coverage.legacy_present_equivalent {
                let candidates = &logical_domain - &presence;
                let (verified, _) = verify_legacy_facts(
                    self,
                    collection,
                    &self.catalog,
                    &path.segments,
                    binding_operation,
                    records_for(&candidates)?.into_iter(),
                )?;
                present_records += verified;
            }
            if !descriptor.coverage.legacy_null_equivalent && !selected_nulls.is_empty() {
                let (_, verified) = verify_legacy_facts(
                    self,
                    collection,
                    &self.catalog,
                    &path.segments,
                    binding_operation,
                    records_for(&selected_nulls)?.into_iter(),
                )?;
                explicit_null_records = verified;
            }
        }
        let coverage = PostingValuesCoverage {
            eligible_records: eligible,
            unavailable_records: unavailable,
            selected_records: selected_total,
            present_records,
            explicit_null_records,
            indexed_records: indexed,
            unresolved_records: unresolved,
            value_counts_complete: unresolved == 0,
        };
        let last = if let Some(cursor) = &request.continuation {
            if cursor.instance_id != self.instance_id
                || cursor.collection_index != collection_index
                || cursor.generation != collection.base_generation()
                || cursor.revision != revision
                || cursor.field != path.segments
                || cursor.binding != binding
                || cursor.binding_version != binding_version
                || cursor.descending != request.descending
                || cursor.lower != request.lower
                || cursor.upper != request.upper
                || cursor.prefix != request.prefix
                || cursor.selection_id != selection.map(|selected| selected.id)
            {
                return Err("posting value continuation scope mismatch".into());
            }
            Some(&cursor.last_key)
        } else {
            None
        };
        if request.limit == 0 {
            return Ok(PostingValuesPage {
                values: Vec::new(),
                continuation: None,
                coverage,
                visited_keys: 0,
            });
        }
        let mut lower = request_bound(&request.lower);
        let mut upper = request_bound(&request.upper);
        if let Some(prefix) = &prepared_prefix {
            if !prefix.is_empty() {
                lower = tighten_lower(lower, Included(ScalarKey::String(prefix.clone())));
                if let Some(end) = prefix_successor(prefix) {
                    upper = tighten_upper(upper, Excluded(ScalarKey::String(end)));
                }
            }
        }
        if let Some(last) = last {
            if request.descending {
                upper = tighten_upper(upper, Excluded(last.clone()));
            } else {
                lower = tighten_lower(lower, Excluded(last.clone()));
            }
        }
        if empty_range(&lower, &upper) {
            return Ok(PostingValuesPage {
                values: Vec::new(),
                continuation: None,
                coverage,
                visited_keys: 0,
            });
        }
        let mut iterator = index.values(
            lower,
            upper,
            request.descending,
            selected_domain.map(MembershipView::Borrowed),
        )?;
        let mut values = Vec::new();
        let mut visited_keys = 0;
        while values.len() < request.limit {
            let Some((value, count)) = iterator.next()? else {
                break;
            };
            visited_keys += 1;
            if count > 0 {
                values.push(PostingValueCount { value, count });
            }
        }
        let mut has_more = false;
        if values.len() == request.limit {
            while let Some((_, count)) = iterator.next()? {
                visited_keys += 1;
                if count > 0 {
                    has_more = true;
                    break;
                }
            }
        }
        let continuation = if has_more {
            values.last().map(|entry| PostingValuesCursor {
                instance_id: self.instance_id,
                collection_index,
                generation: collection.base_generation(),
                revision,
                field: path.segments.clone(),
                binding,
                binding_version,
                descending: request.descending,
                lower: request.lower.clone(),
                upper: request.upper.clone(),
                prefix: request.prefix.clone(),
                selection_id: selection.map(|selected| selected.id),
                last_key: entry.value.clone(),
            })
        } else {
            None
        };
        Ok(PostingValuesPage {
            values,
            continuation,
            coverage,
            visited_keys,
        })
    }
}

#[cfg(test)]
mod tests;
