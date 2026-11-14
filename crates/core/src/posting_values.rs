//! Read-only typed posting dictionaries and distinct record counts.

use std::ops::Bound::{self, Excluded, Included, Unbounded};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use roaring::RoaringBitmap;

use crate::build::IndexedCollectionSegment;
use crate::inference::InferenceMode;
use crate::inventory::{ActiveRecords, RecordClass};
use crate::lifecycle::IndexRevision;
use crate::lifecycle::PublishedSegment;
use crate::lifecycle_catalog::FileCatalog;
use crate::model::{FieldPath, LogicalType, PathRoot, RecordId, Value};
use crate::posting::{PostingIndex, ScalarKey};
use crate::query::Query;
use crate::record_preparation::prepare_query_record_with_execution;
use crate::reference::FieldDefinition;
use crate::repository::{CollectionState, Repository};
use crate::source::{RecordRangeReader, ScanProjection};

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
    index: &'a PostingIndex,
    binding: String,
    binding_version: u32,
    indexed_records: Option<&'a RoaringBitmap>,
    native_prefix: bool,
}

fn resolve<'a>(
    collection: &'a CollectionState,
    request: &PostingValuesRequest,
    path: &FieldPath,
) -> Result<ResolvedPosting<'a>, String> {
    if path.root != PathRoot::Data {
        return Err("posting values require a logical data field".into());
    }
    let field = collection.definition.schema.fields.get(&path.segments);
    let binding = request
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
        .unwrap_or_else(|| "default".into());
    if binding == "default" && !field.is_some_and(|field| field.indexed) {
        let inference = collection
            .inference
            .as_ref()
            .filter(|state| state.report.mode == InferenceMode::AutoIndex && state.report.complete)
            .ok_or("field has no admitted inferred posting index")?;
        let index = inference
            .indexes
            .get(&path.segments)
            .ok_or("field has no admitted inferred posting index")?;
        if index.covered_records as u64 != collection.logical_records.len() {
            return Err("incomplete inferred posting logical coverage".into());
        }
        if !index.value_complete && !inference.short_string_coverage.contains_key(&path.segments) {
            return Err("inferred field has no general value posting coverage".into());
        }
        return Ok(ResolvedPosting {
            index,
            binding: "inferred".into(),
            binding_version: inference.report.policy.version,
            indexed_records: inference.short_string_coverage.get(&path.segments),
            native_prefix: true,
        });
    }
    let field = field.ok_or("posting values require a registered field")?;
    if binding == "default" {
        debug_assert!(field.indexed);
        let index = collection
            .postings
            .fields
            .get(&path.segments)
            .ok_or("posting index missing")?;
        return Ok(ResolvedPosting {
            index,
            binding,
            binding_version: 0,
            indexed_records: None,
            native_prefix: true,
        });
    }
    let key = (path.segments.clone(), binding.clone());
    let registration = collection
        .definition
        .schema
        .operations
        .get(&key)
        .ok_or("binding has no registered posting operation")?;
    let custom = collection
        .custom_postings
        .get(&key)
        .ok_or("binding has no posting index")?;
    if !custom.complete {
        return Err("incomplete custom posting coverage".into());
    }
    Ok(ResolvedPosting {
        index: &custom.index,
        binding,
        binding_version: registration.version,
        indexed_records: None,
        native_prefix: false,
    })
}

fn resolve_segment<'a>(
    collection: &CollectionState,
    segment: &'a IndexedCollectionSegment,
    path: &FieldPath,
    binding: &str,
    binding_version: u32,
) -> Result<ResolvedPosting<'a>, String> {
    let (index, indexed_records) = if binding == "inferred" {
        let inference = segment
            .inference
            .as_ref()
            .filter(|state| state.report.mode == InferenceMode::AutoIndex && state.report.complete)
            .ok_or("segment has no complete inferred posting index")?;
        if inference.report.policy.version != binding_version {
            return Err("segment inferred posting policy changed".into());
        }
        let index = inference
            .indexes
            .get(&path.segments)
            .ok_or("segment inferred posting field missing")?;
        if !index.value_complete && !inference.short_string_coverage.contains_key(&path.segments) {
            return Err("segment inferred field has no general value posting coverage".into());
        }
        (index, inference.short_string_coverage.get(&path.segments))
    } else if binding == "default" {
        (
            segment
                .postings
                .fields
                .get(&path.segments)
                .ok_or("segment posting field missing")?,
            None,
        )
    } else {
        let key = (path.segments.clone(), binding.to_owned());
        let registration = collection
            .definition
            .schema
            .operations
            .get(&key)
            .ok_or("segment posting binding missing")?;
        if registration.version != binding_version {
            return Err("segment posting binding version changed".into());
        }
        let custom = segment
            .custom_postings
            .get(&key)
            .ok_or("segment custom posting missing")?;
        if !custom.complete {
            return Err("incomplete segment custom posting coverage".into());
        }
        (&custom.index, None)
    };
    if index.covered_records as u64 != segment.logical_records.len() {
        return Err("incomplete segment posting logical coverage".into());
    }
    Ok(ResolvedPosting {
        index,
        binding: binding.to_owned(),
        binding_version,
        indexed_records,
        native_prefix: binding == "default" || binding == "inferred",
    })
}

struct ScannedSegmentPosting {
    index: PostingIndex,
    complete_values: RoaringBitmap,
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
    catalog: &FileCatalog,
    path: &FieldPath,
    kind: &LogicalType,
    multi: bool,
    max_short_string: Option<usize>,
) -> Result<ScannedSegmentPosting, String> {
    let field = FieldDefinition {
        path: path.segments.clone(),
        kind: kind.clone(),
        required: false,
        nullable: true,
        indexed: true,
        multi,
        element_nullable: Vec::new(),
    };
    let mut index = PostingIndex::new(&field);
    let mut complete_values = RoaringBitmap::new();
    for file_index in 0..segment.index.file_count() {
        let file = segment.index.try_file_at(file_index)?.ok_or("segment file row missing")?;
        let ranges = file
            .records
            .iter()
            .filter(|slot| segment.active.contains(**slot))
            .map(|slot| {
                let record = segment.index.try_record(*slot)?.ok_or("segment record row missing")?;
                Ok((record.class == RecordClass::Logical).then_some((slot.0, record.range)))
            })
            .collect::<Result<Vec<_>, String>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        if ranges.is_empty() {
            continue;
        }
        let current = catalog
            .entry(file.entry.id)
            .ok_or("active inferred segment file retired")?;
        if current.relative_path != file.entry.relative_path
            || current.revision != file.entry.revision
        {
            return Err("active inferred segment file changed without retirement".into());
        }
        let mut reader = RecordRangeReader::open_for_search(
            &repo.files.root.join(&current.relative_path),
            current.revision,
            None,
        )?;
        for batch in ranges.chunks(128) {
            for (slot, bytes) in reader.read(batch)?.0 {
                let value = prepare_query_record_with_execution(
                    collection.generation.records[slot as usize].class,
                    &collection.source,
                    &collection.definition.schema,
                    &bytes,
                    &ScanProjection::full(false),
                    false,
                    None,
                )?
                .value;
                index.add(crate::model::RecordSlot(slot), &value)?;
                if crate::reference::data_values(&value, &path.segments).map_or(
                    max_short_string.is_none(),
                    |values| {
                        (max_short_string.is_none() || !values.is_empty())
                            && values
                                .iter()
                                .all(|value| fully_represented(value, max_short_string))
                    },
                ) {
                    complete_values.insert(slot);
                }
            }
        }
    }
    if let Some(limit) = max_short_string {
        index
            .values
            .retain(|key, _| !matches!(key, ScalarKey::String(text) if text.len() > limit));
    }
    index.finish_stats();
    Ok(ScannedSegmentPosting {
        index,
        complete_values,
    })
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
    catalog: &FileCatalog,
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
    let mut request = query.resolve_current_clock()?.into_owned();
    request.collections = vec![collection.to_owned()];
    request.options.count = true;
    request.options.offset = 0;
    request.options.sort.clear();
    request.options.evidence = false;
    request.options.return_mode = crate::execution_options::ReturnMode::Locator;
    request.options.limit = segments[collection_index]
        .iter()
        .try_fold(
            base_active[collection_index].len() as usize,
            |total, segment| total.checked_add(segment.active.len() as usize),
        )
        .ok_or("selection domain too large")?;
    let result =
        crate::run::search_segmented(repo, &request, base_active, segments, catalog, revision)?;
    if result.count != Some(result.hits.len()) {
        return Err("exact selection result incomplete".into());
    }
    let mut members = RoaringBitmap::new();
    let mut segment_members = vec![RoaringBitmap::new(); segments[collection_index].len()];
    for hit in result.hits {
        let id = RecordId(hit.record_id.into());
        if let Some(slot) = state.id_lookup.get(&id)
            && base_active[collection_index].contains(*slot)
        {
            members.insert(slot.0);
            continue;
        }
        let mut found = false;
        for (index, segment) in segments[collection_index].iter().enumerate() {
            if let Some(slot) = segment.index.try_id_lookup(id.0.as_ref())?
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
        generation: state.generation.generation,
        revision: Some(revision),
        members,
        segment_members,
    })))
}

pub(crate) fn posting_values_segmented(
    repo: &Repository,
    request: &PostingValuesRequest,
    revision: IndexRevision,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
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
    let base = resolve(collection, request, &path)?;
    if base.index.covered_records as u64 != collection.logical_records.len() {
        return Err("incomplete posting logical coverage".into());
    }
    let binding = base.binding.clone();
    let binding_version = base.binding_version;
    let short_limit = (binding == "inferred"
        && base.indexed_records.is_some()
        && key_kind(&base.index.kind) == &LogicalType::String)
        .then_some(repo.options.inference.policy.short_string_max_utf8_bytes);
    let mut fallbacks = Vec::with_capacity(segments[collection_index].len());
    for segment in segments[collection_index].iter() {
        let missing = binding == "inferred"
            && segment.active.len() > 0
            && resolve_segment(collection, &segment.index, &path, &binding, binding_version)
                .map_or(true, |posting| posting.index.kind != base.index.kind);
        fallbacks.push(if missing {
            Some(scan_inferred_segment(
                repo,
                collection,
                segment,
                catalog,
                &path,
                &base.index.kind,
                base.index.multi,
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
                index: &fallback.index,
                binding: binding.clone(),
                binding_version,
                indexed_records: Some(&fallback.complete_values),
                native_prefix: true,
            });
            continue;
        }
        for file_index in 0..segment.index.file_count() {
            let file = segment.index.try_file_at(file_index)?.ok_or("segment file row missing")?;
            if file
                .records
                .iter()
                .any(|slot| segment.active.contains(*slot))
            {
                let current = catalog
                    .entry(file.entry.id)
                    .ok_or("active segment posting file retired")?;
                if current.relative_path != file.entry.relative_path
                    || current.revision != file.entry.revision
                {
                    return Err("active segment posting file changed without retirement".into());
                }
            }
        }
        resolved.push(resolve_segment(
            collection,
            &segment.index,
            &path,
            &binding,
            binding_version,
        )?);
    }
    if resolved
        .iter()
        .any(|shard| shard.index.kind != resolved[0].index.kind)
    {
        return Err("segment posting key type changed".into());
    }
    let selection = request.selection.as_ref().map(|selection| &selection.0);
    if selection.is_some_and(|selection| {
        selection.instance_id != repo.instance_id
            || selection.collection_index != collection_index
            || selection.generation != collection.generation.generation
            || selection.revision != Some(revision)
            || selection.segment_members.len() != segments[collection_index].len()
    }) {
        return Err("exact selection scope mismatch".into());
    }
    for bound in [&request.lower, &request.upper].into_iter().flatten() {
        if !compatible(&resolved[0].index.kind, &bound.value) {
            return Err("posting value bound has incompatible type".into());
        }
    }
    let prepared_prefix = if let Some(prefix) = &request.prefix {
        if key_kind(&resolved[0].index.kind) != &LogicalType::String {
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
        let logical = if index == 0 {
            &collection.logical_records
        } else {
            &segments[collection_index][index - 1].index.logical_records
        };
        let domain = &domains[index];
        selected_total += domain.len();
        eligible += domain.intersection_len(logical);
        present += domain.intersection_len(&shard.index.presence);
        nulls += domain.intersection_len(&shard.index.nulls);
        indexed += shard.indexed_records.map_or_else(
            || domain.intersection_len(logical),
            |covered| domain.intersection_len(covered),
        );
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
            || cursor.generation != collection.generation.generation
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
        .map(|shard| {
            shard
                .index
                .value_range(lower.clone(), upper.clone(), request.descending)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut heads = iterators
        .iter_mut()
        .map(|iterator| iterator.next())
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
                count += head.as_ref().unwrap().1.intersection_len(&domains[index]);
                *head = iterators[index].next()?;
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
            generation: collection.generation.generation,
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
        let members = crate::run::exact_members_with_active(
            self,
            resolved.as_ref(),
            collection_index,
            active,
        )?;
        Ok(ExactSelection(Arc::new(SelectionData {
            id: NEXT_SELECTION_ID.fetch_add(1, Ordering::Relaxed),
            instance_id: self.instance_id,
            collection_index,
            generation: state.generation.generation,
            revision,
            members,
            segment_members: Vec::new(),
        })))
    }

    pub fn posting_values(
        &self,
        request: &PostingValuesRequest,
    ) -> Result<PostingValuesPage, String> {
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
        let resolved = resolve(collection, request, &path)?;
        let ResolvedPosting {
            index,
            binding,
            binding_version,
            indexed_records,
            native_prefix,
        } = resolved;
        if index.covered_records as u64 != collection.logical_records.len() {
            return Err("incomplete posting logical coverage".into());
        }
        let selection = request.selection.as_ref().map(|handle| &handle.0);
        if selection.is_some_and(|selection| {
            selection.instance_id != self.instance_id
                || selection.collection_index != collection_index
                || selection.generation != collection.generation.generation
                || selection.revision != revision
        }) {
            return Err("exact selection scope mismatch".into());
        }
        for bound in [&request.lower, &request.upper].into_iter().flatten() {
            if !compatible(&index.kind, &bound.value) {
                return Err("posting value bound has incompatible type".into());
            }
        }
        let prepared_prefix = if let Some(prefix) = &request.prefix {
            if key_kind(&index.kind) != &LogicalType::String {
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
        let eligible = selected_domain.map_or(collection.logical_records.len(), |selected| {
            selected.intersection_len(&collection.logical_records)
        });
        let live_total = active.map_or(collection.generation.records.len() as u64, |domains| {
            domains[collection_index].len()
        });
        let unavailable = selected_total.unwrap_or(live_total) - eligible;
        let indexed = indexed_records.map_or(eligible, |covered| {
            selected_domain.map_or(covered.len(), |selected| selected.intersection_len(covered))
        });
        let unresolved = eligible.saturating_sub(indexed);
        let coverage = PostingValuesCoverage {
            eligible_records: eligible,
            unavailable_records: unavailable,
            selected_records: selected_total,
            present_records: selected_domain.map_or(index.presence.len(), |selected| {
                selected.intersection_len(&index.presence)
            }),
            explicit_null_records: selected_domain.map_or(index.nulls.len(), |selected| {
                selected.intersection_len(&index.nulls)
            }),
            indexed_records: indexed,
            unresolved_records: unresolved,
            value_counts_complete: unresolved == 0,
        };
        let last = if let Some(cursor) = &request.continuation {
            if cursor.instance_id != self.instance_id
                || cursor.collection_index != collection_index
                || cursor.generation != collection.generation.generation
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
        let mut iterator = index.value_range(lower, upper, request.descending)?;
        let mut values = Vec::new();
        let mut visited_keys = 0;
        while values.len() < request.limit {
            let Some((value, posting)) = iterator.next()? else {
                break;
            };
            visited_keys += 1;
            let count = selected_domain
                .map_or(posting.len(), |selected| posting.intersection_len(selected));
            if count > 0 {
                values.push(PostingValueCount { value, count });
            }
        }
        let mut has_more = false;
        if values.len() == request.limit {
            while let Some((_, posting)) = iterator.next()? {
                visited_keys += 1;
                if selected_domain
                    .map_or(posting.len(), |selected| posting.intersection_len(selected))
                    > 0
                {
                    has_more = true;
                    break;
                }
            }
        }
        let continuation = if has_more {
            values.last().map(|entry| PostingValuesCursor {
                instance_id: self.instance_id,
                collection_index,
                generation: collection.generation.generation,
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
#[path = "posting_values.test.rs"]
mod tests;
