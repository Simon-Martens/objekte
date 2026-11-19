//! Bounded result retention, grouped source access, and payload materialization.

use crate::measurement::timer::DetailedInstant;
use std::cmp::Ordering;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use rayon::prelude::*;
use roaring::RoaringBitmap;
use smallvec::SmallVec;

use crate::index::posting::PostingIndex;
use crate::index::segment::IndexedCollectionSegment;
use crate::index::segment::PublishedSegment;
use crate::index::sorting::simple_composite_part;
use crate::index::sorting::{InferredOrderKey, SortColumn};
#[cfg(test)]
use crate::index::sorting::{SortKey, SortOptions};
use crate::inventory::{ActiveRecords, ByteRange, FileEntry, RecordClass, RecordEntry};
use crate::lifecycle::IndexRevision;
use crate::lifecycle::catalog::FileCatalog;
use crate::measurement::{ScanWork, SourceAvailabilityLog, SourceDiagnostic, WorkCounters};
use crate::model::{FieldPath, LogicalType, PathRoot, RecordSlot, Value};
#[cfg(test)]
use crate::query::FtsMode;
use crate::query::binding::fts::BoundFts;
use crate::query::options::ReturnMode;
use crate::query::prepared::PreparedCollectionQuery;
use crate::query::semantics::folding::FoldRecordGuard;
use crate::query::semantics::record::RecordContext;
use crate::query::sort_binding::{
    BoundSort, BoundSortSource, BoundSortTerm, MergedSortTargetCache,
};
use crate::query::{Operator, Quantifier, Query};
use crate::query::{
    binding::filter::BoundExpr, binding::filter::BoundTerm, binding::filter::Derived,
};
use crate::query_diagnostics::trace::{PlanActual, QueryPlanTrace};
pub(crate) use crate::query_diagnostics::trace::{
    attach_execution_observations, bound_term_diagnostics, finish_trace, fts_covered_target,
    regex_prefilter_reason,
};
use crate::query_execution::restrictions::{
    Plan, PlanningCounters, estimate_expr, estimate_fts_candidates, estimate_plan_expr,
    fts_candidates, plan_expr_restricted_with_execution, segment_filter_bounds,
    segment_filter_bounds_with_catalog, segment_filter_bounds_with_input_catalog_and_predicates,
    segment_filter_estimate, segment_fts_bounds_with_term, segment_fts_candidates,
    segment_fts_estimate_with_term,
};
use crate::query_execution::streaming::{
    Abort, CancellationToken, Delivery, ExecutionContext, ExecutionErrorKind, ExecutionLimits,
    ResultScope, StreamControl, StreamError, StreamEvent, StreamMode, StreamOptions, StreamOutcome,
    StreamSummary,
};
use crate::query_predicate::{
    DirectSourcePlan, PreparedExpr, Residual, borrowed_relation_source_text, borrowed_value_text,
    local_segment_projection, prepare_residual, prepare_value_text_projection, proven_evidence,
    scan_projection, segment_filter_evidence_bytes, segment_filter_source_fields,
    shared_fold_bound, shared_fold_residual, source_fts_eligible,
};
use crate::query_results::cache::{CacheBudget, materialize_lifecycle_cached};
pub(crate) use crate::query_results::cache::{
    CachedSortRead, cached_sort_fixed_bytes, materialize_cached, materialize_cached_stream,
    record_base_cache_hits, sort_cached_members,
};
use crate::query_results::delivery::emit_provisional_page;
use crate::query_results::materialize::materialize_winner_page;
use crate::query_results::payload::{
    materialize, payload_from_bytes, payload_from_bytes_with_execution,
};
use crate::query_results::ranking::{
    MergedPageHit, RankKey, ResolvedSort, Winner, base_rank_key_merged, offer, offer_merged,
    rank_key, rank_key_bytes, rank_key_from_context, relation_rank_key_merged,
    segment_compare_to_rank, segment_rank_key, winner_bytes, would_enter_page,
    would_enter_rank_key_with,
};
use crate::query_results::verified::{CachedMember, CachedResult, VerifiedCache};
use crate::query_results::{Payload, SearchHit, SearchResult};
use crate::reference::fts::eval_bound_fts;
use crate::reference::{eval_unavailable_filter, filter::eval_term};
use crate::repository::Repository;
use crate::repository::state::CollectionState;
use crate::source::access::{
    RecordRangeReader, SourceAccessSession, classify_source_read_failure_for_search,
    probe_source_unavailable_for_search, read_record_range_for_search,
};
use crate::source::preparation::prepare_query_record_with_execution;
use crate::source::verification::{BorrowedScanQuery, prepare_decoded_record_with_borrowed_fts};
use crate::source::{ExtractedRecord, ScanProjection};

mod ordered;
mod relations;
mod segment_scan;
mod source_scan;
#[cfg(test)]
use ordered::cached_inferred_order;
pub(crate) use ordered::direct_payload;
use ordered::{
    LifecycleOrderCursor, RejectedOrderPlans, flush_all_batch_if_full, inferred_order_index,
    offer_cached_key, posting_order_eligible, read_lifecycle_ordered_record,
    try_inferred_order_page, try_inferred_segment_order_page, try_stored_order_page,
};
#[cfg(test)]
pub(crate) use relations::arm_reference_row_visits;
use relations::search_segmented_relations;
pub(crate) use relations::{
    explain_segmented, search_segmented_reference, search_segmented_reference_records,
};
use segment_scan::{scan_segment_file_parallel, verify_local_segment_record};
use source_scan::scan_file;

const MAX_SORT_KEY_BYTES: u64 = 64 * 1024 * 1024;

#[cfg(test)]
thread_local! {
    static SEGMENT_SLOT_EVIDENCE_CALLS: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_segment_slot_evidence_calls() {
    SEGMENT_SLOT_EVIDENCE_CALLS.with(|calls| calls.set(0));
}

#[cfg(test)]
pub(crate) fn segment_slot_evidence_calls() -> usize {
    SEGMENT_SLOT_EVIDENCE_CALLS.with(std::cell::Cell::get)
}

/// The native scheduling boundary for parallel file scans. A segment whose
/// longest record is exactly 64 KiB may still enter the parallel path; one
/// byte more keeps it on the bounded serial path.
pub(crate) fn record_ranges_allow_parallel_scan(max_record_range_len: u64) -> bool {
    max_record_range_len <= 64 * 1024
}
const MAX_COMPUTED_SORT_BYTES: u64 = 128 * 1024 * 1024;

/// One lifecycle query's cache namespace. The store is shared by all pinned
/// publications of an index, so retention is bounded across revisions.
#[derive(Clone, Copy)]
pub(crate) struct LifecycleCacheScope<'a> {
    pub(crate) store: &'a Mutex<VerifiedCache>,
    pub(crate) instance_id: u64,
}

fn check_computed_sort_budget(payload: u64, keys: u64, targets: u64) -> Result<(), String> {
    check_computed_sort_budget_with_limit(MAX_COMPUTED_SORT_BYTES, payload, keys, targets)
}

fn check_computed_sort_budget_with_limit(
    limit: u64,
    payload: u64,
    keys: u64,
    targets: u64,
) -> Result<(), String> {
    let total = payload
        .checked_add(keys)
        .and_then(|used| used.checked_add(targets))
        .ok_or("computed sort memory count overflow")?;
    if total > limit {
        return Err("computed sort memory budget exceeded".into());
    }
    Ok(())
}

fn computed_sort_limit(execution: Option<&ExecutionContext>) -> u64 {
    execution.map_or(MAX_COMPUTED_SORT_BYTES, |execution| {
        execution
            .limits
            .computed_sort_bytes
            .min(MAX_COMPUTED_SORT_BYTES)
    })
}

fn computed_sort_resource(execution: Option<&ExecutionContext>, message: &str) -> String {
    execution.map_or_else(
        || message.to_owned(),
        |execution| execution.fail(ExecutionErrorKind::Resource, message),
    )
}

fn check_execution_sort_budget(
    execution: Option<&ExecutionContext>,
    payload: u64,
    keys: u64,
    targets: u64,
) -> Result<(), String> {
    check_computed_sort_budget_with_limit(computed_sort_limit(execution), payload, keys, targets)
        .map_err(|message| computed_sort_resource(execution, &message))
}

fn check_cached_sort_live(
    sort: &ResolvedSort<'_>,
    fixed_bytes: u64,
    key_bytes: u64,
    has_related_sort: bool,
) -> Result<(), String> {
    if !has_related_sort {
        return check_computed_sort_budget(fixed_bytes, key_bytes, 0);
    }
    let target_bytes = sort
        .target_cache
        .lock()
        .map_err(|error| error.to_string())?
        .retained_bytes();
    check_computed_sort_budget(fixed_bytes, key_bytes, target_bytes)
}

/// Verified record and any evidence or retained payload.
struct ScannedMatch {
    slot: u32,
    key: Option<RankKey>,
    evidence: Arc<[String]>,
    payload: Option<Payload>,
    retained_bytes: u64,
    buffered_bytes: usize,
}

/// Per-file scan work and verified matches returned by a worker.
#[derive(Default)]
struct ScannedFile {
    files_opened: u64,
    bytes_read: u64,
    record_parses: u64,
    raw_text_verifications: u64,
    scanned_records: u64,
    residual_evaluations: u64,
    relation_work: crate::query_execution::relations::work::RelationWork,
    match_count: u64,
    peak_payload_bytes: u64,
    peak_sort_key_bytes: u64,
    sort_key_evaluations: u64,
    matches: SmallVec<[ScannedMatch; 1]>,
    cache_members: Vec<CachedMember>,
    exact_members: RoaringBitmap,
}

impl From<&ScannedFile> for ScanWork {
    fn from(scanned: &ScannedFile) -> Self {
        Self {
            files_opened: scanned.files_opened,
            bytes_read: scanned.bytes_read,
            record_parses: scanned.record_parses,
            raw_text_verifications: scanned.raw_text_verifications,
            scanned_records: scanned.scanned_records,
            sort_key_evaluations: scanned.sort_key_evaluations,
            residual_evaluations: scanned.residual_evaluations,
        }
    }
}

/// Count-only and exact-selection requests share the normal planner and scanner,
/// but never retain ranked hits or verified-cache members.
struct ExactCollector<'a> {
    members: Option<&'a mut RoaringBitmap>,
}

impl ExactCollector<'_> {
    fn add_set(&mut self, slots: &RoaringBitmap, counters: &mut WorkCounters) {
        counters.total_matches += slots.len();
        if let Some(members) = self.members.as_deref_mut() {
            *members |= slots;
        }
    }
}

struct FileWinner {
    key: RankKey,
    matched: ScannedMatch,
}

impl PartialEq for FileWinner {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for FileWinner {}

impl PartialOrd for FileWinner {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FileWinner {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key.cmp(&other.key)
    }
}

fn inventory_fts_evidence(
    repo: &Repository,
    collection: &crate::repository::state::CollectionState,
    fts: &BoundFts,
    input: &RoaringBitmap,
) -> Result<HashMap<u32, HashSet<String>>, String> {
    let empty = Value::Object(Default::default());
    let mut matches = HashMap::new();
    for slot in input.iter() {
        let record = &collection.generation.records[slot as usize];
        let file = &repo.files.files[record.file.0 as usize];
        let ctx = RecordContext {
            value: &empty,
            source_bytes: None,
            record,
            file,
            collection_name: &collection.definition.name,
            repository_root: &repo.files.root,
        };
        let mut evidence = HashSet::new();
        if eval_bound_fts(fts, &[], &ctx, &mut evidence)? {
            matches.insert(slot, evidence);
        }
    }
    Ok(matches)
}

fn segment_metadata_fts_evidence(
    repo: &Repository,
    collection: &CollectionState,
    segment: &IndexedCollectionSegment,
    active: &ActiveRecords,
    catalog: &FileCatalog,
    fts: &BoundFts,
    input: &RoaringBitmap,
    retain_evidence: bool,
) -> Result<HashMap<u32, Vec<String>>, String> {
    let empty = Value::Object(Default::default());
    let mut matches = HashMap::new();
    for slot in input.iter() {
        let resolved = segment_record_ref(segment, active, catalog, RecordSlot(slot))?
            .ok_or("active metadata FTS slot missing")?;
        let ctx = RecordContext {
            value: &empty,
            source_bytes: None,
            record: resolved.record,
            file: resolved.file,
            collection_name: &collection.definition.name,
            repository_root: &repo.files.root,
        };
        let mut evidence = HashSet::new();
        if eval_bound_fts(fts, &[], &ctx, &mut evidence)? {
            matches.insert(
                slot,
                if retain_evidence {
                    evidence.into_iter().collect()
                } else {
                    Vec::new()
                },
            );
        }
    }
    Ok(matches)
}

pub(crate) fn make_hit(
    repo: &Repository,
    collection_index: usize,
    record: &RecordEntry,
    mut matched_fields: Vec<String>,
    payload: Option<Payload>,
) -> SearchHit {
    matched_fields.sort();
    let file = &repo.files.files[record.file.0 as usize];
    SearchHit {
        collection: repo.collections[collection_index].definition.name.clone(),
        record_id: record.id.0.to_string(),
        generation: repo.collections[collection_index].generation.generation,
        file_id: file.id.0,
        file_path: file.relative_path.to_path_buf(),
        file_size: file.revision.size,
        file_created_ns: file.created_ns,
        byte_start: record.range.start,
        byte_end: record.range.end,
        modified_ns: record.revision.modified_ns,
        matched_fields,
        file_status: file.status,
        record_class: record.class,
        diagnostic_ref: record.diagnostic_ref,
        payload,
    }
}

/// A segment record has a local slot but retains a repository-global file ID.
/// Resolve both through the same pinned segment; the base inventory cannot
/// address a file added after the base generation was built.
pub(crate) struct SegmentRecordRef<'a> {
    pub record: &'a RecordEntry,
    pub file: &'a FileEntry,
}

fn segment_active_file<'a>(
    segment: &IndexedCollectionSegment,
    active: &ActiveRecords,
    catalog: &'a FileCatalog,
    file_id: crate::model::FileId,
) -> Result<&'a FileEntry, String> {
    if active.slot_count() as usize != segment.record_count() {
        return Err("segment active domain length mismatch".into());
    }
    let file_index = segment
        .file_index
        .get(&file_id)
        .copied()
        .ok_or("segment record file missing")?;
    let segment_revision = segment
        .files
        .get(file_index)
        .map(|file| file.entry.revision)
        .ok_or("segment record file missing")?;
    let file = catalog
        .entry(file_id)
        .ok_or("active segment record file is retired")?;
    if file.revision != segment_revision || !file.collections.contains(&segment.collection) {
        return Err("active segment record file changed without retirement".into());
    }
    Ok(file)
}

pub(crate) fn segment_record_ref<'a>(
    segment: &'a IndexedCollectionSegment,
    active: &ActiveRecords,
    catalog: &'a FileCatalog,
    slot: RecordSlot,
) -> Result<Option<SegmentRecordRef<'a>>, String> {
    if active.slot_count() as usize != segment.record_count() {
        return Err("segment active domain length mismatch".into());
    }
    if !active.contains(slot) {
        return Ok(None);
    }
    let record = segment.record(slot).ok_or("segment record slot missing")?;
    let file_index = segment
        .file_index
        .get(&record.file)
        .copied()
        .ok_or("segment record file missing")?;
    let segment_revision = segment
        .files
        .get(file_index)
        .map(|file| file.entry.revision)
        .ok_or("segment record file missing")?;
    let file = catalog
        .entry(record.file)
        .ok_or("active segment record file is retired")?;
    // The catalog owns the path in this publication. A compatible move may
    // route an unchanged segment to a new path without rebuilding postings.
    if file.revision != segment_revision || !file.collections.contains(&segment.collection) {
        return Err("active segment record file changed without retirement".into());
    }
    Ok(Some(SegmentRecordRef { record, file }))
}

/// Direct lookup for one indexed delta. The publication owner chooses the
/// active segment for an ID and invokes this with that segment's active mask.
pub(crate) fn get_segment_record_by_id(
    repo: &Repository,
    collection_index: usize,
    segment: &IndexedCollectionSegment,
    active: &ActiveRecords,
    catalog: &FileCatalog,
    revision: IndexRevision,
    record_id: &str,
    return_mode: ReturnMode,
) -> Result<Option<SearchHit>, String> {
    let collection = repo
        .collections
        .get(collection_index)
        .ok_or("segment collection missing")?;
    if segment.collection != collection.generation.collection {
        return Err("segment collection mismatch".into());
    }
    let Some(slot) = segment.id_lookup.get(record_id).copied() else {
        return Ok(None);
    };
    let Some(resolved) = segment_record_ref(segment, active, catalog, slot)? else {
        return Ok(None);
    };
    let record = resolved.record;
    let file = resolved.file;
    let payload = match return_mode {
        ReturnMode::Locator => None,
        ReturnMode::Logical if record.class == RecordClass::RawFallback => {
            Some(Payload::LogicalUnavailable {
                diagnostic_ref: record.diagnostic_ref,
            })
        }
        ReturnMode::Bytes | ReturnMode::Logical => {
            let path = repo.files.root.join(&file.relative_path);
            let bytes = read_record_range_for_search(&path, file.revision, record.range, None)?;
            payload_from_bytes(record.class, Some(collection), &return_mode, bytes)?.0
        }
    };
    Ok(Some(SearchHit {
        collection: collection.definition.name.clone(),
        record_id: record.id.0.to_string(),
        generation: revision.0,
        file_id: file.id.0,
        file_path: file.relative_path.to_path_buf(),
        file_size: file.revision.size,
        file_created_ns: file.created_ns,
        byte_start: record.range.start,
        byte_end: record.range.end,
        modified_ns: record.revision.modified_ns,
        matched_fields: Vec::new(),
        file_status: file.status,
        record_class: record.class,
        diagnostic_ref: record.diagnostic_ref,
        payload,
    }))
}

/// One current-view direct lookup. A missing source is distinct from an ID
/// that is absent from the pinned active domain.
#[derive(Clone, Debug)]
pub enum DirectLookupResult {
    Found(SearchHit),
    NotFound,
    SourceUnavailable(SourceDiagnostic),
}

pub(crate) fn direct_lookup_segmented(
    repo: &Repository,
    collection_name: &str,
    record_id: &str,
    return_mode: ReturnMode,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
    revision: IndexRevision,
) -> Result<DirectLookupResult, String> {
    let collection_index = repo
        .collections
        .iter()
        .position(|collection| collection.definition.name == collection_name)
        .ok_or("unknown collection")?;
    let collection = &repo.collections[collection_index];
    let base = collection
        .id_lookup
        .get(record_id)
        .copied()
        .filter(|slot| base_active[collection_index].contains(*slot));
    let mut selected_segment = None;
    for (index, segment) in segments[collection_index].iter().enumerate() {
        if let Some(slot) = segment.index.id_lookup.get(record_id).copied()
            && segment.active.contains(slot)
        {
            if base.is_some() || selected_segment.is_some() {
                return Err("duplicate active direct-lookup ID".into());
            }
            selected_segment = Some((index, slot));
        }
    }
    let (record, file, result) = if let Some(slot) = base {
        let record = collection
            .generation
            .records
            .get(slot.0 as usize)
            .ok_or("base direct-lookup slot missing")?;
        let file = catalog
            .entry(record.file)
            .ok_or("active base direct-lookup file retired")?;
        if !file.collections.contains(&collection.generation.collection) {
            return Err("active base direct-lookup file lost collection membership".into());
        }
        let payload = match return_mode {
            ReturnMode::Locator => Ok(None),
            ReturnMode::Logical if record.class == RecordClass::RawFallback => {
                Ok(Some(Payload::LogicalUnavailable {
                    diagnostic_ref: record.diagnostic_ref,
                }))
            }
            ReturnMode::Bytes | ReturnMode::Logical => {
                let path = repo.files.root.join(&file.relative_path);
                read_record_range_for_search(&path, file.revision, record.range, None)
                    .and_then(|bytes| {
                        payload_from_bytes(record.class, Some(collection), &return_mode, bytes)
                    })
                    .map(|(payload, _)| payload)
            }
        };
        let result = payload.map(|payload| {
            Some(SearchHit {
                collection: collection.definition.name.clone(),
                record_id: record.id.0.to_string(),
                generation: revision.0,
                file_id: file.id.0,
                file_path: file.relative_path.to_path_buf(),
                file_size: file.revision.size,
                file_created_ns: file.created_ns,
                byte_start: record.range.start,
                byte_end: record.range.end,
                modified_ns: file.revision.modified_ns,
                matched_fields: Vec::new(),
                file_status: file.status,
                record_class: record.class,
                diagnostic_ref: record.diagnostic_ref,
                payload,
            })
        });
        (record, file, result)
    } else if let Some((index, slot)) = selected_segment {
        let segment = &segments[collection_index][index];
        let resolved = segment_record_ref(&segment.index, &segment.active, catalog, slot)?
            .ok_or("active segment direct-lookup slot retired")?;
        (
            resolved.record,
            resolved.file,
            get_segment_record_by_id(
                repo,
                collection_index,
                &segment.index,
                &segment.active,
                catalog,
                revision,
                record_id,
                return_mode,
            ),
        )
    } else {
        return Ok(DirectLookupResult::NotFound);
    };
    match result {
        Ok(Some(mut hit)) => {
            repo.stamp_native_hit_at(&mut hit, revision);
            Ok(DirectLookupResult::Found(hit))
        }
        Ok(None) => Err("active direct-lookup record disappeared".into()),
        Err(message) => {
            let path = repo.files.root.join(&file.relative_path);
            if let Some(kind) =
                probe_source_unavailable_for_search(&path, file.revision, Some(record.range))
            {
                Ok(DirectLookupResult::SourceUnavailable(SourceDiagnostic {
                    file_id: file.id.0,
                    file_path: file.relative_path.to_path_buf(),
                    kind,
                    message,
                }))
            } else {
                Err(message)
            }
        }
    }
}

/// Optional complete ordering captured during an already exhaustive lifecycle
/// query. Once its budget is exhausted, query execution continues without a
/// cache entry; a prefix is never published as complete membership.
struct LifecycleCacheBuilder {
    ranked: Option<Vec<(RankKey, CachedMember)>>,
    bytes: usize,
    budget: usize,
}

impl LifecycleCacheBuilder {
    fn new(budget: usize) -> Self {
        Self {
            ranked: Some(Vec::new()),
            bytes: 0,
            budget,
        }
    }

    fn is_accepting(&self) -> bool {
        self.ranked.is_some()
    }

    fn push(&mut self, key: &RankKey, member: CachedMember) {
        let Some(ranked) = self.ranked.as_mut() else {
            return;
        };
        let fields = member
            .matched_fields
            .iter()
            .try_fold(0usize, |bytes, field| {
                bytes.checked_add(field.len().saturating_add(std::mem::size_of::<String>()))
            });
        let cost = fields.and_then(|fields| {
            fields
                .checked_add(std::mem::size_of::<CachedMember>())
                .and_then(|bytes| {
                    usize::try_from(rank_key_bytes(key))
                        .ok()
                        .and_then(|rank| bytes.checked_add(rank))
                })
        });
        let next = cost.and_then(|cost| self.bytes.checked_add(cost));
        if next.is_none_or(|next| next > self.budget) || ranked.try_reserve(1).is_err() {
            self.ranked = None;
            return;
        }
        self.bytes = next.unwrap();
        ranked.push((key.clone(), member));
    }

    fn finish(mut self, total: u64) -> Option<Vec<CachedMember>> {
        let ranked = self.ranked.as_mut()?;
        if ranked.len() as u64 != total {
            return None;
        }
        ranked.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        Some(
            self.ranked
                .take()
                .unwrap()
                .into_iter()
                .map(|(_, member)| member)
                .collect(),
        )
    }
}

pub(crate) fn segment_locator_hit(
    collection: &CollectionState,
    resolved: SegmentRecordRef<'_>,
    revision: IndexRevision,
    mut matched_fields: Vec<String>,
) -> SearchHit {
    matched_fields.sort();
    let record = resolved.record;
    let file = resolved.file;
    SearchHit {
        collection: collection.definition.name.clone(),
        record_id: record.id.0.to_string(),
        generation: revision.0,
        file_id: file.id.0,
        file_path: file.relative_path.to_path_buf(),
        file_size: file.revision.size,
        file_created_ns: file.created_ns,
        byte_start: record.range.start,
        byte_end: record.range.end,
        modified_ns: record.revision.modified_ns,
        matched_fields,
        file_status: file.status,
        record_class: record.class,
        diagnostic_ref: record.diagnostic_ref,
        payload: None,
    }
}

#[derive(Clone, Copy)]
pub(crate) struct SourceHitLocator {
    file_id: u32,
    byte_start: u64,
    byte_end: u64,
}

impl From<&SearchHit> for SourceHitLocator {
    fn from(hit: &SearchHit) -> Self {
        Self {
            file_id: hit.file_id,
            byte_start: hit.byte_start,
            byte_end: hit.byte_end,
        }
    }
}

pub(crate) fn record_unavailable_hit(
    repo: &Repository,
    catalog: &FileCatalog,
    hit: SourceHitLocator,
    message: String,
    counters: &mut WorkCounters,
) -> Result<bool, String> {
    let file = catalog
        .entry(crate::model::FileId(hit.file_id))
        .ok_or("materialized hit file missing from pinned catalog")?;
    let path = repo.files.root.join(&file.relative_path);
    let range = ByteRange {
        start: hit.byte_start,
        end: hit.byte_end,
    };
    let Some(kind) = probe_source_unavailable_for_search(&path, file.revision, Some(range)) else {
        return Ok(false);
    };
    counters.record_source_unavailable(SourceDiagnostic {
        file_id: file.id.0,
        file_path: file.relative_path.to_path_buf(),
        kind,
        message,
    });
    Ok(true)
}

pub(crate) fn bound_expr_has_relations(expr: &BoundExpr) -> bool {
    match expr {
        BoundExpr::Term(term) => {
            term.related.is_some()
                || term.path.root == PathRoot::Backlinks
                || term
                    .rhs
                    .as_ref()
                    .is_some_and(|rhs| rhs.path.root == PathRoot::Backlinks)
        }
        BoundExpr::And(parts) | BoundExpr::Or(parts) => parts.iter().any(bound_expr_has_relations),
        BoundExpr::Not(inner) => bound_expr_has_relations(inner),
    }
}

/// Search the base and bounded indexed deltas as one pinned page. Segment
/// candidates use local slot domains; only ranked hits cross shard boundaries.
/// Relation and related sort keys use the pinned merged relation resolver.
pub(crate) fn search_segmented(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
    revision: IndexRevision,
) -> Result<SearchResult, String> {
    search_segmented_lifecycle(repo, query, base_active, segments, catalog, revision, None)
}

pub(crate) fn search_segmented_lifecycle(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
    revision: IndexRevision,
    cache: Option<LifecycleCacheScope<'_>>,
) -> Result<SearchResult, String> {
    let mut result = match search_segmented_impl(
        repo,
        query,
        base_active,
        segments,
        catalog,
        revision,
        None,
        cache,
        None,
    ) {
        Err(_error) if has_unavailable_active_source(repo, base_active, segments, catalog) => {
            search_segmented_reference(repo, query, base_active, segments, catalog, revision)
        }
        result => result,
    }?;
    if query.options.return_mode != ReturnMode::Locator
        && result.counters.source_unavailable_events > 0
    {
        result.count = None;
        result.counters.match_count_complete = false;
    }
    if query.options.return_mode != ReturnMode::Locator
        && query.options.offset == 0
        && query.options.limit > 0
        && result.hits.is_empty()
        && result.count.is_some_and(|count| count > 0)
    {
        result.count = None;
        result.counters.match_count_complete = false;
    }
    Ok(result)
}

#[cfg(test)]
pub(crate) fn search_segmented_with_context(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
    revision: IndexRevision,
    context: &ExecutionContext,
) -> Result<SearchResult, crate::query_execution::streaming::ExecutionError> {
    search_segmented_with_context_lifecycle(
        repo,
        query,
        base_active,
        segments,
        catalog,
        revision,
        context,
        None,
    )
}

pub(crate) fn search_segmented_with_context_lifecycle(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
    revision: IndexRevision,
    context: &ExecutionContext,
    cache: Option<LifecycleCacheScope<'_>>,
) -> Result<SearchResult, crate::query_execution::streaming::ExecutionError> {
    search_segmented_with_context_lifecycle_budget(
        repo,
        query,
        base_active,
        segments,
        catalog,
        revision,
        context,
        cache,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn search_segmented_with_context_lifecycle_budget(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
    revision: IndexRevision,
    context: &ExecutionContext,
    cache: Option<LifecycleCacheScope<'_>>,
    retained_budget: Option<usize>,
) -> Result<SearchResult, crate::query_execution::streaming::ExecutionError> {
    context.check()?;
    let result = search_segmented_impl(
        repo,
        query,
        base_active,
        segments,
        catalog,
        revision,
        Some(context),
        cache,
        retained_budget,
    );
    match result {
        Err(message) => {
            let error = context.recorded_error(message);
            if matches!(
                error.kind,
                ExecutionErrorKind::SourceChanged | ExecutionErrorKind::Execution
            ) && has_unavailable_active_source(repo, base_active, segments, catalog)
            {
                context.clear_recovered_source_changed();
                let sort = ResolvedSort::new(repo, query)
                    .map_err(|message| context.recorded_error(message))?;
                let prepared = crate::query::prepared::prepare(repo, query)
                    .map_err(|message| context.recorded_error(message))?;
                search_segmented_relations(
                    repo,
                    query,
                    base_active,
                    segments,
                    catalog,
                    revision,
                    &sort,
                    &prepared.collections,
                    Some(context),
                    None,
                    None,
                    false,
                    false,
                )
                .map_err(|message| context.recorded_error(message))
            } else {
                Err(error)
            }
        }
        Ok(result) => Ok(result),
    }
}

/// Preserve the base fast path when no segment is present, while letting a
/// failed source read fall back to the pinned availability scan.
#[cfg(test)]
pub(crate) fn search_active_available(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    catalog: &FileCatalog,
    revision: IndexRevision,
) -> Result<SearchResult, String> {
    search_active_available_lifecycle(repo, query, base_active, catalog, revision, None)
}

pub(crate) fn search_active_available_lifecycle(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    catalog: &FileCatalog,
    revision: IndexRevision,
    _cache: Option<LifecycleCacheScope<'_>>,
) -> Result<SearchResult, String> {
    let empty = vec![Arc::new(Vec::new()); repo.collections.len()];
    match search_active(repo, query, base_active) {
        Err(_error) if has_unavailable_active_source(repo, base_active, &empty, catalog) => {
            search_segmented_reference(repo, query, base_active, &empty, catalog, revision)
        }
        result => result,
    }
}

pub(crate) fn search_active_available_with_context_lifecycle(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    catalog: &FileCatalog,
    revision: IndexRevision,
    context: &ExecutionContext,
    _cache: Option<LifecycleCacheScope<'_>>,
) -> Result<SearchResult, crate::query_execution::streaming::ExecutionError> {
    let empty = vec![Arc::new(Vec::new()); repo.collections.len()];
    match search_with_context_active(repo, query, context, base_active) {
        Err(error)
            if matches!(
                error.kind,
                ExecutionErrorKind::SourceChanged | ExecutionErrorKind::Execution
            ) && has_unavailable_active_source(repo, base_active, &empty, catalog) =>
        {
            context.clear_recovered_source_changed();
            let sort = ResolvedSort::new(repo, query)
                .map_err(|message| context.recorded_error(message))?;
            let prepared = crate::query::prepared::prepare(repo, query)
                .map_err(|message| context.recorded_error(message))?;
            search_segmented_relations(
                repo,
                query,
                base_active,
                &empty,
                catalog,
                revision,
                &sort,
                &prepared.collections,
                Some(context),
                None,
                None,
                false,
                false,
            )
            .map_err(|message| context.recorded_error(message))
        }
        result => result,
    }
}

/// Probe active files only after an optimized query failed. Ordinary queries
/// keep their index/cache fast path without a per-file metadata pass.
fn has_unavailable_active_source(
    repo: &Repository,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
) -> bool {
    let unavailable = |file: &FileEntry| {
        let path = repo.files.root.join(&file.relative_path);
        probe_source_unavailable_for_search(&path, file.revision, None).is_some()
    };
    for (index, collection) in repo.collections.iter().enumerate() {
        for file_id in &collection.generation.files {
            if let Some(slots) = collection.generation.file_records.records_for(*file_id)
                && slots.iter().any(|slot| base_active[index].contains(*slot))
                && let Some(file) = catalog.entry(*file_id)
            {
                if unavailable(file)
                    || slots
                        .iter()
                        .filter(|slot| base_active[index].contains(**slot))
                        .any(|slot| {
                            let range = collection.generation.records[slot.0 as usize].range;
                            range.start > range.end || range.end > file.revision.size
                        })
                {
                    return true;
                }
            }
        }
        for segment in segments[index].iter() {
            for file_index in 0..segment.index.file_count() {
                let Some(file) = segment.index.files.get(file_index) else {
                    return true;
                };
                if file
                    .records
                    .iter()
                    .any(|slot| segment.active.contains(*slot))
                    && let Some(current) = catalog.entry(file.entry.id)
                {
                    if unavailable(current)
                        || file
                            .records
                            .iter()
                            .filter(|slot| segment.active.contains(**slot))
                            .any(|slot| {
                                segment.index.record(*slot).is_none_or(|record| {
                                    record.range.start > record.range.end
                                        || record.range.end > current.revision.size
                                })
                            })
                    {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Evidence shared by every possible true branch. Branches with different
/// fields retain source verification even when index membership is exact.
fn segment_static_evidence_fields(expr: &BoundExpr) -> Option<Vec<String>> {
    match expr {
        BoundExpr::Term(term) => {
            if term.related.is_some()
                || (term.rhs.is_some()
                    && !crate::query_access::catalog::self_comparison_shape(term)
                    && !term.rhs.as_ref().is_some_and(|rhs| {
                        matches!(term.path.root, PathRoot::Metadata(_))
                            && matches!(rhs.path.root, PathRoot::Metadata(_))
                    }))
            {
                return None;
            }
            let mut fields = if term.derived == Derived::None
                && !matches!(term.operator, Operator::Ne | Operator::NotContains)
            {
                vec![term.query_path.display.clone()]
            } else {
                Vec::new()
            };
            if let Some(rhs) = &term.rhs
                && matches!(term.path.root, PathRoot::Metadata(_))
                && matches!(rhs.path.root, PathRoot::Metadata(_))
            {
                fields.push(rhs.query_path.display.clone());
                fields.sort();
                fields.dedup();
            }
            Some(fields)
        }
        BoundExpr::And(parts) => {
            let mut fields = Vec::new();
            for part in parts {
                fields.extend(segment_static_evidence_fields(part)?);
            }
            fields.sort();
            fields.dedup();
            Some(fields)
        }
        BoundExpr::Or(parts) => {
            let mut shared: Option<Vec<String>> = None;
            for part in parts {
                let mut fields = segment_static_evidence_fields(part)?;
                fields.sort();
                fields.dedup();
                if shared.as_ref().is_some_and(|shared| *shared != fields) {
                    return None;
                }
                shared = Some(fields);
            }
            shared
        }
        BoundExpr::Not(_) => Some(Vec::new()),
    }
}

/// A single file's local residual work can run independently of other files. Only its
/// bounded local top page crosses back to the ordered merge thread.
struct SegmentFileScan {
    bytes_read: u64,
    scanned_records: u64,
    record_parses: u64,
    residual_evaluations: u64,
    raw_text_verifications: u64,
    sort_key_evaluations: u64,
    matches: u64,
    winners: Vec<SegmentLocalWinner>,
}

/// A file worker retains source bytes only for its bounded local winners when
/// evidence is requested. The merge thread evaluates evidence for page offers.
struct SegmentLocalWinner {
    key: RankKey,
    slot: RecordSlot,
    matched_fields: Vec<String>,
    source_bytes: Option<Vec<u8>>,
}

impl PartialEq for SegmentLocalWinner {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for SegmentLocalWinner {}

impl PartialOrd for SegmentLocalWinner {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SegmentLocalWinner {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key.cmp(&other.key)
    }
}

fn order_segment_file_groups(
    mut files: Vec<(u32, SmallVec<[(u32, ByteRange); 1]>)>,
    ordered: bool,
) -> Vec<(u32, SmallVec<[(u32, ByteRange); 1]>)> {
    if ordered {
        return files;
    }
    files.sort_unstable_by_key(|(file, ranges)| (*file, ranges[0].0));
    let mut grouped: Vec<(u32, SmallVec<[(u32, ByteRange); 1]>)> = Vec::new();
    for (file, ranges) in files {
        if let Some((previous_file, previous_ranges)) = grouped.last_mut()
            && *previous_file == file
        {
            previous_ranges.extend(ranges);
        } else {
            grouped.push((file, ranges));
        }
    }
    grouped
}

/// Fully proven shard membership can be merged from resident sort orders.
/// Keep only one cursor per shard and the requested page; fall back to the
/// general planner if their measured retained bytes exceed this route's cap.
#[allow(clippy::too_many_arguments)]
fn push_exact_order_cursor(
    repo: &Repository,
    query: &Query,
    sort: &ResolvedSort<'_>,
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    collection_index: usize,
    segment_index: usize,
    selected: &[(RoaringBitmap, Vec<String>)],
    mut position: usize,
    heap: &mut BinaryHeap<Reverse<LifecycleOrderCursor>>,
    retained_bytes: &mut usize,
) -> Result<bool, String> {
    let published = &segments[collection_index][segment_index];
    let Some(order) = lifecycle_segment_order(&published.index, sort, query, collection_index)
    else {
        return Ok(false);
    };
    while let Some(slot) = order.get(position).copied() {
        position += 1;
        if !selected[segment_index].0.contains(slot.0) {
            continue;
        }
        let record = published
            .index
            .record(slot)
            .ok_or("exact ordered slot missing")?;
        let key = segment_rank_key(repo, sort, collection_index, &published.index, record, None)?;
        let Some(bytes) = usize::try_from(rank_key_bytes(&key))
            .ok()
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<LifecycleOrderCursor>()))
        else {
            return Ok(false);
        };
        let Some(next) = retained_bytes.checked_add(bytes) else {
            return Ok(false);
        };
        if next > 64 * 1024 * 1024 {
            return Ok(false);
        }
        *retained_bytes = next;
        heap.push(Reverse(LifecycleOrderCursor {
            key,
            bytes,
            collection_index,
            segment_index,
            position,
            slot,
        }));
        break;
    }
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
fn exact_ordered_page(
    repo: &Repository,
    query: &Query,
    sort: &ResolvedSort<'_>,
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
    revision: IndexRevision,
    collection_index: usize,
    selected: &[(RoaringBitmap, Vec<String>)],
    mut counters: WorkCounters,
) -> Result<Option<SearchResult>, String> {
    let Some(cap) = query.options.offset.checked_add(query.options.limit) else {
        return Err("page size overflow".into());
    };
    if selected.len() != segments[collection_index].len() {
        return Err("exact ordered partition count mismatch".into());
    }
    let Some(mut retained_bytes) = selected
        .len()
        .checked_mul(std::mem::size_of::<(RoaringBitmap, Vec<String>)>())
    else {
        return Ok(None);
    };
    let mut total = 0u64;
    for (bitmap, evidence) in selected {
        total = total
            .checked_add(bitmap.len())
            .ok_or("exact count overflow")?;
        let Some(evidence_bytes) = evidence.iter().try_fold(0usize, |bytes, field| {
            bytes
                .checked_add(field.len())?
                .checked_add(std::mem::size_of::<String>())
        }) else {
            return Ok(None);
        };
        let Some(next) = retained_bytes
            .checked_add(bitmap.serialized_size())
            .and_then(|bytes| bytes.checked_add(evidence_bytes))
        else {
            return Ok(None);
        };
        retained_bytes = next;
        if retained_bytes > 64 * 1024 * 1024 {
            return Ok(None);
        }
    }
    let mut heap = BinaryHeap::new();
    for segment_index in 0..selected.len() {
        if !push_exact_order_cursor(
            repo,
            query,
            sort,
            segments,
            collection_index,
            segment_index,
            selected,
            0,
            &mut heap,
            &mut retained_bytes,
        )? {
            return Ok(None);
        }
    }
    let mut hits = Vec::new();
    let mut seen = 0usize;
    while seen < cap {
        let Some(Reverse(cursor)) = heap.pop() else {
            break;
        };
        retained_bytes -= cursor.bytes;
        let published = &segments[collection_index][cursor.segment_index];
        if seen >= query.options.offset {
            let resolved =
                segment_record_ref(&published.index, &published.active, catalog, cursor.slot)?
                    .ok_or("exact ordered source retired")?;
            let hit = segment_locator_hit(
                &repo.collections[collection_index],
                resolved,
                revision,
                selected[cursor.segment_index].1.clone(),
            );
            let Some(next) =
                retained_bytes.checked_add(crate::query_execution::streaming::hit_bytes(&hit))
            else {
                return Ok(None);
            };
            if next > 64 * 1024 * 1024 {
                return Ok(None);
            }
            retained_bytes = next;
            hits.push(hit);
        }
        seen += 1;
        if seen < cap
            && !push_exact_order_cursor(
                repo,
                query,
                sort,
                segments,
                collection_index,
                cursor.segment_index,
                selected,
                cursor.position,
                &mut heap,
                &mut retained_bytes,
            )?
        {
            return Ok(None);
        }
    }
    counters.exact_posting_matches += total;
    counters.total_matches += total;
    counters.match_count_complete = true;
    counters.returned_records = hits.len() as u64;
    Ok(Some(SearchResult {
        count: query
            .options
            .count
            .then_some(counters.total_matches as usize),
        hits,
        counters,
    }))
}

#[allow(clippy::too_many_arguments)]
fn try_exact_segment_ordered_page(
    repo: &Repository,
    query: &Query,
    sort: &ResolvedSort<'_>,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
    revision: IndexRevision,
    prepared: &[PreparedCollectionQuery],
    counters: &WorkCounters,
) -> Result<Option<SearchResult>, String> {
    if query.options.return_mode != ReturnMode::Locator
        || sort.bound.has_computed
        || sort
            .bound
            .terms
            .iter()
            .flatten()
            .flatten()
            .any(|term| matches!(&term.source, BoundSortSource::Computed(_)))
        || prepared.len() != 1
        || base_active.iter().any(|active| active.len() != 0)
    {
        return Ok(None);
    }
    let entry = &prepared[0];
    let collection_index = entry.collection_index;
    let filter = entry.filter.as_ref();
    let fts = entry.fts.as_ref();
    if fts.is_some() {
        return Ok(None);
    }
    let collection = &repo.collections[collection_index];
    if segments[collection_index].len() > 4096 {
        return Ok(None);
    }
    let mut selected = Vec::with_capacity(segments[collection_index].len());
    let mut counters = counters.clone();
    for published in segments[collection_index].iter() {
        if lifecycle_segment_order(&published.index, sort, query, collection_index).is_none() {
            return Ok(None);
        }
        let bounds = filter
            .map(|filter| {
                segment_filter_bounds_with_catalog(
                    &published.index,
                    &published.active,
                    filter,
                    collection,
                    Some(catalog),
                )
            })
            .transpose()?;
        let candidates = match bounds.as_ref() {
            Some(bounds) if bounds.may == bounds.must => bounds.may.as_ref().clone(),
            Some(bounds)
                if filter.is_some_and(excludes_raw_fallback)
                    && (bounds.may.as_ref() & &published.index.logical_records)
                        == (bounds.must.as_ref() & &published.index.logical_records) =>
            {
                bounds.must.as_ref() & &published.index.logical_records
            }
            Some(_) => return Ok(None),
            None => published.active.bitmap().clone(),
        };
        let evidence = if query.options.evidence {
            if matches!(filter, Some(BoundExpr::And(_))) && candidates.len() <= 4 {
                return Ok(None);
            }
            if let Some(filter) = filter {
                let Some(fields) = segment_static_evidence_fields(filter) else {
                    return Ok(None);
                };
                fields
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        counters.candidates += candidates.len();
        selected.push((candidates, evidence));
    }
    exact_ordered_page(
        repo,
        query,
        sort,
        segments,
        catalog,
        revision,
        collection_index,
        &selected,
        counters,
    )
}

fn excludes_raw_fallback(filter: &BoundExpr) -> bool {
    match filter {
        BoundExpr::Term(term) => {
            term.path.root == PathRoot::Data
                && term.related.is_none()
                && !matches!(term.operator, Operator::Contains | Operator::NotContains)
        }
        BoundExpr::And(parts) => parts.iter().all(excludes_raw_fallback),
        BoundExpr::Or(_) | BoundExpr::Not(_) => false,
    }
}

fn exact_or_branch_members(
    repo: &Repository,
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    collection_index: usize,
    filter: &BoundExpr,
) -> Result<u64, String> {
    let BoundExpr::Or(parts) = filter else {
        return Ok(0);
    };
    let collection = &repo.collections[collection_index];
    let mut members = crate::index::segment::SegmentRecordSet::default();
    for part in parts {
        if let BoundExpr::Term(term) = part
            && term.path.root == PathRoot::Metadata(crate::model::MetadataField::RecordClass)
            && term.operator == Operator::Eq
            && let Value::Number(expected) = &term.literal
        {
            for published in segments[collection_index].iter() {
                for slot in published.active.bitmap().iter() {
                    let record = published
                        .index
                        .record(RecordSlot(slot))
                        .ok_or("exact record-class branch slot missing")?;
                    if crate::model::Number::integer(record.class.code() as i128) == *expected {
                        members.insert(crate::index::segment::SegmentRecordAddress {
                            collection: published.index.collection,
                            segment: published.id,
                            slot: record.slot,
                        });
                    }
                }
            }
            continue;
        }
        if let BoundExpr::Term(term) = part
            && term.path.root == PathRoot::Data
            && !collection
                .definition
                .schema
                .fields()
                .get(&term.path.segments)
                .is_some_and(|field| field.indexed)
        {
            continue;
        }
        if matches!(
            part,
            BoundExpr::Term(term)
                if matches!(term.operator, Operator::Contains | Operator::NotContains)
                    || term.related.is_some()
        ) {
            continue;
        }
        let mut branch = crate::index::segment::SegmentRecordSet::default();
        let mut exact = true;
        for published in segments[collection_index].iter() {
            let bounds =
                segment_filter_bounds(&published.index, &published.active, part, collection)?;
            let logical = &published.active.bitmap() & &published.index.logical_records;
            if bounds.may != bounds.must
                && !(excludes_raw_fallback(part)
                    && (bounds.may.as_ref() & &logical) == (bounds.must.as_ref() & &logical))
            {
                exact = false;
                break;
            }
            for slot in bounds.must.iter() {
                let record = published
                    .index
                    .record(RecordSlot(slot))
                    .ok_or("exact OR branch slot missing")?;
                branch.insert(crate::index::segment::SegmentRecordAddress {
                    collection: published.index.collection,
                    segment: published.id,
                    slot: record.slot,
                });
            }
        }
        if exact {
            for address in branch.iter() {
                members.insert(address);
            }
        }
    }
    Ok(members.len())
}

#[allow(clippy::too_many_arguments)]
fn try_lifecycle_ordered_page(
    repo: &Repository,
    query: &Query,
    prepared: &crate::query::prepared::PreparedQuery,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
    revision: IndexRevision,
    context: Option<&ExecutionContext>,
    cache: Option<LifecycleCacheScope<'_>>,
) -> Result<Option<SearchResult>, String> {
    if query
        .options
        .offset
        .checked_add(query.options.limit)
        .is_none()
        || query.options.limit == 0
    {
        return Ok(None);
    }
    if query.options.sort.is_empty()
        && !matches!(query.filter, Some(crate::query::FilterExpr::Term(_)))
    {
        return Ok(None);
    }
    if prepared
        .collections
        .iter()
        .any(|entry| entry.filter.is_none())
    {
        return Ok(None);
    }
    let single_collection = prepared.collections.len() == 1;
    let mut all_exact = true;
    for entry in &prepared.collections {
        let collection_index = entry.collection_index;
        let filter = entry.filter.as_ref();
        let fts = entry.fts.as_ref();
        if fts.is_some() {
            all_exact = false;
            continue;
        }
        let filter = filter.ok_or("ordered filter missing")?;
        for segment in segments[collection_index].iter() {
            let bounds = segment_filter_bounds_with_catalog(
                &segment.index,
                &segment.active,
                filter,
                &repo.collections[collection_index],
                Some(catalog),
            )?;
            let logical = &segment.active.bitmap() & &segment.index.logical_records;
            let exact = bounds.may == bounds.must
                || (excludes_raw_fallback(filter)
                    && (bounds.may.as_ref() & &logical) == (bounds.must.as_ref() & &logical));
            all_exact &= exact;
        }
    }
    if single_collection && all_exact && !query.options.sort.is_empty() {
        return Ok(None);
    }
    if query.options.sort.is_empty() && !all_exact {
        return Ok(None);
    }
    let fallback_context;
    let execution = if let Some(context) = context {
        context
    } else {
        fallback_context = ExecutionContext::new(
            crate::query::clock::ClockContext::new(time::OffsetDateTime::now_utc()),
            ExecutionLimits::default(),
            None,
        );
        &fallback_context
    };
    let cancellation = execution.cancellation().cloned().unwrap_or_default();
    let mut page_query = query.clone();
    page_query.options.count = false;
    let mut hits = Vec::new();
    let mut sink = |event: crate::query_execution::streaming::StreamEvent<'_>| {
        match event {
            crate::query_execution::streaming::StreamEvent::OrderedRows { rows, .. }
            | crate::query_execution::streaming::StreamEvent::PageReplacement { rows, .. } => {
                hits.extend(rows.iter().cloned());
            }
            crate::query_execution::streaming::StreamEvent::Progress { .. } => {}
        }
        Ok(crate::query_execution::streaming::StreamControl::Continue)
    };
    let mut delivery = Delivery::new(
        crate::query_execution::streaming::StreamOptions::default(),
        &cancellation,
        &mut sink,
    );
    let scope = crate::query_execution::streaming::ResultScope::Page {
        offset: query.options.offset,
        limit: query.options.limit,
    };
    let _ordered_phase = crate::query_execution::request::observation_phase("ordered_page");
    if let Some(collector) = context.and_then(|context| context.observation_collector()) {
        collector.set_phase("ordered_page");
    }
    let outcome = try_lifecycle_ordered_stream(
        repo,
        &page_query,
        query.options.return_mode.clone(),
        scope,
        base_active,
        segments,
        catalog,
        revision,
        execution,
        cache,
        &mut delivery,
    )?;
    let Some(crate::query_execution::streaming::StreamOutcome::Complete(summary)) = outcome else {
        return Ok(None);
    };
    let mut counters = summary.counters;
    let mut count = None;
    if query.options.count {
        let _count_phase = crate::query_execution::request::observation_phase("exact_count");
        if let Some(collector) = context.and_then(|context| context.observation_collector()) {
            collector.set_phase("exact_count");
        }
        let mut count_query = query.clone();
        count_query.options.offset = 0;
        count_query.options.limit = 0;
        count_query.options.return_mode = ReturnMode::Locator;
        let counted = search_segmented_with_context_lifecycle(
            repo,
            &count_query,
            base_active,
            segments,
            catalog,
            revision,
            execution,
            cache,
        )
        .map_err(|error| error.message)?;
        counters = counted.counters;
        count = counted.count;
    }
    if let Some(collector) = context.and_then(|context| context.observation_collector()) {
        collector.set_phase("search");
    }
    if query.options.return_mode != ReturnMode::Locator {
        let selected_files = hits.iter().map(|hit| hit.file_id).collect::<HashSet<_>>();
        counters.distinct_files = counters.distinct_files.max(selected_files.len() as u64);
        counters.files_opened = counters.files_opened.max(selected_files.len() as u64);
        if query.options.return_mode == ReturnMode::Logical {
            counters.record_parses = counters.record_parses.max(
                hits.iter()
                    .filter(|hit| hit.record_class == RecordClass::Logical)
                    .count() as u64,
            );
        }
    }
    if counters.source_unavailable_events > 0 {
        count = None;
        counters.match_count_complete = false;
    }
    let exact_or_members = prepared.collections.iter().try_fold(0u64, |total, entry| {
        entry
            .filter
            .as_ref()
            .map(|filter| exact_or_branch_members(repo, segments, entry.collection_index, filter))
            .transpose()
            .map(|members| total.saturating_add(members.unwrap_or(0)))
    })?;
    counters.exact_posting_matches = counters.exact_posting_matches.max(exact_or_members);
    counters.returned_records = hits.len() as u64;
    Ok(Some(SearchResult {
        count,
        hits,
        counters,
    }))
}

#[allow(clippy::too_many_arguments)]
fn offer_admitted_segment_slot(
    repo: &Repository,
    sort: &ResolvedSort<'_>,
    collection: &CollectionState,
    segment: &IndexedCollectionSegment,
    published: &PublishedSegment,
    catalog: &FileCatalog,
    collection_index: usize,
    segment_index: usize,
    revision: IndexRevision,
    slot: RecordSlot,
    evidence: impl FnOnce() -> Vec<String>,
    cache_builder: Option<&mut LifecycleCacheBuilder>,
    winners: &mut BinaryHeap<MergedPageHit>,
    cap: usize,
    winner_budget: Option<(usize, &ExecutionContext)>,
    winner_bytes: &mut usize,
) -> Result<(), String> {
    let enters_page = would_enter_rank_key_with(
        winners.len(),
        winners.peek().map(|winner| &winner.key),
        cap,
        |worst| {
            segment_compare_to_rank(repo, sort, collection_index, segment, slot, worst)
                .map(|order| order.is_lt())
        },
    )?;
    let cache_accepting = cache_builder
        .as_ref()
        .is_some_and(|builder| builder.is_accepting());
    if !enters_page && !cache_accepting {
        return Ok(());
    }
    #[cfg(test)]
    SEGMENT_SLOT_EVIDENCE_CALLS.with(|calls| calls.set(calls.get() + 1));
    let matched_fields = evidence();
    let resolved = segment_record_ref(segment, &published.active, catalog, slot)?
        .ok_or("active segment candidate missing")?;
    let key = segment_rank_key(repo, sort, collection_index, segment, resolved.record, None)?;
    let hit_fields = match cache_builder {
        Some(builder) => {
            if !builder.is_accepting() {
                enters_page.then_some(matched_fields)
            } else {
                let matched_fields: Arc<[String]> = matched_fields.into();
                builder.push(
                    &key,
                    CachedMember {
                        collection_index,
                        segment_index: Some(segment_index),
                        slot: slot.0,
                        matched_fields: Arc::clone(&matched_fields),
                    },
                );
                enters_page.then(|| matched_fields.to_vec())
            }
        }
        None => Some(matched_fields),
    };
    let Some(hit_fields) = hit_fields else {
        return Ok(());
    };
    let hit = segment_locator_hit(collection, resolved, revision, hit_fields);
    offer_merged(
        winners,
        cap,
        MergedPageHit {
            key,
            hit,
            segment: Some((collection_index, segment_index)),
            record_address: None,
            retained_bytes: 0,
        },
        winner_budget,
        winner_bytes,
    )
}

fn search_segmented_impl(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
    revision: IndexRevision,
    context: Option<&ExecutionContext>,
    cache: Option<LifecycleCacheScope<'_>>,
    retained_budget: Option<usize>,
) -> Result<SearchResult, String> {
    if query.options.count
        && query.options.limit == 0
        && let Some(collector) = context.and_then(|context| context.observation_collector())
    {
        collector.set_phase("exact_count");
    }
    let _count_phase = (query.options.count && query.options.limit == 0)
        .then(|| crate::query_execution::request::observation_phase("exact_count"));
    if let Some(context) = context {
        context.check_string()?;
    }
    if segments.len() != repo.collections.len() || base_active.len() != repo.collections.len() {
        return Err("segment query collection count mismatch".into());
    }
    query
        .options
        .offset
        .checked_add(query.options.limit)
        .ok_or("page size overflow")?;
    let cache_key = if retained_budget.is_none()
        && let Some(scope) = cache
        && context.is_none_or(|context| context.limits == ExecutionLimits::default())
    {
        let key =
            crate::query_results::verified::lifecycle_key(query, scope.instance_id, revision.0)?;
        let mut store = scope.store.lock().map_err(|error| error.to_string())?;
        if store.enabled() {
            if let Some(cached) = store.get(&key) {
                drop(store);
                return materialize_lifecycle_cached(
                    repo,
                    query,
                    &cached,
                    base_active,
                    segments,
                    catalog,
                    revision,
                    context,
                );
            }
            let budget = store.budget().saturating_sub(key.len().saturating_add(128));
            Some((scope, key, budget))
        } else {
            None
        }
    } else {
        None
    };
    let sort = ResolvedSort::new(repo, query).map_err(|message| match context {
        Some(context) => context.fail(ExecutionErrorKind::InvalidQuery, message),
        None => message,
    })?;
    let related_sort = sort.bound.terms.iter().flatten().flatten().any(|term| {
        matches!(&term.source, BoundSortSource::Computed(computed) if computed.related.is_some())
    });
    let prepared =
        crate::query::prepared::prepare(repo, query).map_err(|message| match context {
            Some(context) => context.fail(ExecutionErrorKind::InvalidQuery, message),
            None => message,
        })?;
    let has_relations = prepared.collections.iter().any(|entry| {
        entry.filter.as_ref().is_some_and(bound_expr_has_relations)
            || entry
                .fts
                .as_ref()
                .is_some_and(|fts| fts.fields.iter().any(|field| field.related.is_some()))
    });
    if !has_relations
        && let Some(result) = try_inferred_segment_order_page(
            repo,
            query,
            &prepared,
            base_active,
            segments,
            catalog,
            revision,
            None,
            context,
        )?
    {
        return Ok(result);
    }
    if !has_relations
        && let Some(result) = try_lifecycle_ordered_page(
            repo,
            query,
            &prepared,
            base_active,
            segments,
            catalog,
            revision,
            context,
            cache,
        )?
    {
        return Ok(result);
    }
    if let Some(collector) = context.and_then(|context| context.observation_collector()) {
        collector.set_phase("search");
    }
    if has_relations || related_sort {
        let mut relation_cache = cache_key.as_ref().and_then(|(_, _, budget)| {
            (*budget > 0 && base_active.iter().all(|active| active.len() == 0))
                .then(|| LifecycleCacheBuilder::new(*budget))
        });
        let result = search_segmented_relations(
            repo,
            query,
            base_active,
            segments,
            catalog,
            revision,
            &sort,
            &prepared.collections,
            context,
            relation_cache.as_mut(),
            retained_budget,
            true,
            false,
        )?;
        if result.counters.match_count_complete
            && let (Some(builder), Some((scope, key, _))) = (relation_cache, cache_key)
            && let Some(members) = builder.finish(result.counters.total_matches)
        {
            if let Some(context) = context {
                context.check_string()?;
            }
            let displaced = scope
                .store
                .lock()
                .map_err(|error| error.to_string())?
                .insert(key, members);
            drop(displaced);
        }
        return Ok(result);
    }
    // This merged planner already verifies the full segment domain, even when
    // the caller does not request a count. Retain compact ordered addresses
    // only if every base record has moved to lifecycle partitions; otherwise
    // the base top-k call does not expose all members.
    let mut cache_builder = cache_key.as_ref().and_then(|(_, _, budget)| {
        (*budget > 0 && base_active.iter().all(|active| active.len() == 0))
            .then(|| LifecycleCacheBuilder::new(*budget))
    });
    let cap = query
        .options
        .offset
        .checked_add(query.options.limit)
        .ok_or("page size overflow")?;
    let base_empty = base_active.iter().all(|active| active.len() == 0);
    let base = if base_empty {
        // The bound query and sort above already validate the request. An
        // empty base domain contributes no rows, so avoid planning it again.
        SearchResult {
            count: query.options.count.then_some(0),
            hits: Vec::new(),
            counters: WorkCounters {
                match_count_complete: true,
                ..WorkCounters::default()
            },
        }
    } else {
        let mut base_query = query.clone();
        base_query.options.offset = 0;
        base_query.options.limit = cap;
        // A page request must not exhaust the base merely to calculate a
        // total. Its top `cap` rows suffice for the merged page.
        base_query.options.count = query.options.count;
        if let Some(context) = context {
            search_with_context_active(repo, &base_query, context, base_active)
                .map_err(|error| error.message)?
        } else {
            search_active(repo, &base_query, base_active)?
        }
    };
    let mut total = base
        .count
        .map_or(base.counters.total_matches, |count| count as u64);
    let base_count_complete = base_empty || base.counters.match_count_complete;
    let mut counters = base.counters;
    let mut exact_or_members = 0;
    for entry in &prepared.collections {
        if let Some(filter) = entry.filter.as_ref() {
            exact_or_members +=
                exact_or_branch_members(repo, segments, entry.collection_index, filter)?;
        }
    }
    let mut winners = BinaryHeap::new();
    let mut winner_bytes = 0usize;
    let winner_budget = retained_budget.zip(context);
    for mut hit in base.hits {
        let collection_index = repo
            .collections
            .iter()
            .position(|collection| collection.definition.name == hit.collection)
            .ok_or("base hit collection missing")?;
        let collection = &repo.collections[collection_index];
        let slot = collection
            .id_lookup
            .get(hit.record_id.as_str())
            .ok_or("base hit ID missing")?;
        let record = collection
            .generation
            .records
            .get(slot.0 as usize)
            .ok_or("base hit slot missing")?;
        let key = base_rank_key_merged(repo, &sort, collection_index, record, catalog, context)?;
        repo.stamp_native_hit_at(&mut hit, revision);
        offer_merged(
            &mut winners,
            cap,
            MergedPageHit {
                key,
                hit,
                segment: None,
                record_address: None,
                retained_bytes: 0,
            },
            winner_budget,
            &mut winner_bytes,
        )?;
    }
    if context.is_none()
        && retained_budget.is_none()
        && cache_key.is_none()
        && let Some(ordered) = try_exact_segment_ordered_page(
            repo,
            query,
            &sort,
            base_active,
            segments,
            catalog,
            revision,
            &prepared.collections,
            &counters,
        )?
    {
        return Ok(ordered);
    }
    let mut delta_opened = HashSet::new();
    let predicates = Arc::clone(&prepared.predicates);
    for entry in prepared.collections {
        let collection_index = entry.collection_index;
        let filter = entry.filter;
        let fts = entry.fts;
        let collection = &repo.collections[collection_index];
        let direct_plan =
            DirectSourcePlan::prepare_bound(collection, &sort, fts.as_ref(), filter.as_ref());
        let projection = local_segment_projection(
            collection,
            filter.as_ref(),
            fts.as_ref(),
            sort.bound.has_computed,
        );
        for (segment_index, published) in segments[collection_index].iter().enumerate() {
            if let Some(context) = context {
                context.check_string()?;
            }
            if published.index.collection != collection.generation.collection {
                return Err("segment collection mismatch".into());
            }
            let segment = &published.index;
            let domain = published.active.bitmap();
            let text_term = entry.text_term;
            let text_estimate = fts
                .as_ref()
                .zip(text_term)
                .map(|(fts, term)| {
                    segment_fts_estimate_with_term(segment, collection, fts, term, domain.len())
                })
                .transpose()?
                .flatten();
            let filter_estimate = filter
                .as_ref()
                .map(|filter| {
                    segment_filter_estimate(
                        segment,
                        collection,
                        Some(catalog),
                        filter,
                        domain.len(),
                    )
                })
                .transpose()?
                .flatten();
            let text_first =
                crate::query_plan::selection::prefer_text_first(text_estimate, filter_estimate);
            let text_first_bounds = if text_first {
                fts.as_ref()
                    .zip(text_term)
                    .map(|(fts, term)| {
                        segment_fts_bounds_with_term(
                            segment, collection, fts, term, &domain, context,
                        )
                    })
                    .transpose()?
            } else {
                None
            };
            let filter_input = if text_first {
                text_first_bounds.as_ref().map_or_else(
                    || Arc::new(domain.clone()),
                    |bounds| Arc::clone(&bounds.may),
                )
            } else {
                Arc::new(domain.clone())
            };
            let bounds = filter
                .as_ref()
                .map(|filter| {
                    segment_filter_bounds_with_input_catalog_and_predicates(
                        segment,
                        &published.active,
                        filter,
                        collection,
                        Some(catalog),
                        filter_input.as_ref(),
                        Some(&predicates),
                        context,
                    )
                })
                .transpose()?;
            let mut candidates = bounds
                .as_ref()
                .map_or(filter_input, |bounds| Arc::clone(&bounds.may));
            if filter.as_ref().is_some_and(excludes_raw_fallback)
                && segment.logical_records.len() != segment.record_count() as u64
                && !candidates.is_subset(&segment.logical_records)
            {
                candidates = Arc::new(candidates.as_ref() & &segment.logical_records);
            }
            let fts_bounds = if text_first {
                text_first_bounds
            } else {
                fts.as_ref()
                    .zip(text_term)
                    .map(|(fts, term)| {
                        segment_fts_bounds_with_term(
                            segment,
                            collection,
                            fts,
                            term,
                            candidates.as_ref(),
                            context,
                        )
                    })
                    .transpose()?
            };
            if let Some(fts_may) = fts_bounds.as_ref()
                && !text_first
            {
                candidates = Arc::clone(&fts_may.may);
            }
            counters.candidates += candidates.len();
            let metadata_evidence = if filter.is_none()
                && query.options.return_mode == ReturnMode::Locator
                && !sort.bound.has_computed
                && !sort.bound.needs_source
                && let Some(fts) = fts.as_ref()
                && !fts.fields.is_empty()
                && fts
                    .fields
                    .iter()
                    .all(|field| matches!(field.path.root, PathRoot::Metadata(_)))
            {
                Some(segment_metadata_fts_evidence(
                    repo,
                    collection,
                    segment,
                    &published.active,
                    catalog,
                    fts,
                    &candidates,
                    query.options.evidence,
                )?)
            } else {
                None
            };
            if let Some(evidence) = &metadata_evidence {
                candidates = Arc::new(evidence.keys().copied().collect());
            }
            if candidates.is_empty() {
                continue;
            }
            let proven = bounds
                .as_ref()
                .map(|bounds| candidates.as_ref() & bounds.must.as_ref())
                .unwrap_or_default();
            if !query.options.evidence
                && (fts.is_none() || metadata_evidence.is_some())
                && !sort.bound.needs_source
                && !sort
                    .bound
                    .terms
                    .get(collection_index)
                    .and_then(|terms| terms.as_ref())
                    .is_some_and(|terms| {
                        terms
                            .iter()
                            .any(|term| matches!(&term.source, BoundSortSource::Computed(_)))
                    })
                && !proven.is_empty()
            {
                for slot in proven.iter() {
                    total += 1;
                    counters.exact_posting_matches += 1;
                    offer_admitted_segment_slot(
                        repo,
                        &sort,
                        collection,
                        segment,
                        published,
                        catalog,
                        collection_index,
                        segment_index,
                        revision,
                        RecordSlot(slot),
                        Vec::new,
                        cache_builder.as_mut(),
                        &mut winners,
                        cap,
                        winner_budget,
                        &mut winner_bytes,
                    )?;
                }
                *Arc::make_mut(&mut candidates) -= &proven;
                if candidates.is_empty() {
                    continue;
                }
            }
            let exact_evidence = if query.options.evidence
                && fts.is_none()
                && !(matches!(filter, Some(BoundExpr::And(_))) && candidates.len() <= 4)
            {
                if let Some(filter) = filter.as_ref() {
                    if bounds
                        .as_ref()
                        .is_some_and(|bounds| candidates.is_subset(&bounds.must))
                    {
                        segment_static_evidence_fields(filter)
                    } else {
                        None
                    }
                } else {
                    Some(Vec::new())
                }
            } else {
                None
            };
            let index_exact = (!query.options.evidence
                || exact_evidence.is_some()
                || metadata_evidence.is_some())
                && !sort.bound.has_computed
                && !sort.bound.needs_source
                && !sort
                    .bound
                    .terms
                    .get(collection_index)
                    .and_then(|terms| terms.as_ref())
                    .is_some_and(|terms| {
                        terms
                            .iter()
                            .any(|term| matches!(&term.source, BoundSortSource::Computed(_)))
                    })
                && (fts.is_none() || metadata_evidence.is_some())
                && bounds.as_ref().is_none_or(|bounds| {
                    bounds.may == bounds.must
                        || (filter.as_ref().is_some_and(excludes_raw_fallback)
                            && (bounds.may.as_ref() & &segment.logical_records)
                                == (bounds.must.as_ref() & &segment.logical_records))
                });
            if index_exact {
                let exact_fields = |slot: u32| {
                    metadata_evidence
                        .as_ref()
                        .and_then(|evidence| evidence.get(&slot))
                        .cloned()
                        .unwrap_or_else(|| exact_evidence.clone().unwrap_or_default())
                };
                // Stored order is already available in each shard. A global
                // top `cap` page cannot contain a shard row ranked below that
                // shard's own top `cap`; count the complete bitmap but only
                // materialize those possible page rows.
                if cache_builder.is_none()
                    && context.is_none()
                    && cap < candidates.len() as usize
                    && let Some(order) =
                        lifecycle_segment_order(segment, &sort, query, collection_index)
                {
                    total += candidates.len();
                    counters.exact_posting_matches += candidates.len();
                    let mut selected = 0usize;
                    for slot in order {
                        if selected == cap {
                            break;
                        }
                        if !candidates.contains(slot.0) {
                            continue;
                        }
                        let resolved =
                            segment_record_ref(segment, &published.active, catalog, *slot)?
                                .ok_or("active segment ordered candidate missing")?;
                        let key = segment_rank_key(
                            repo,
                            &sort,
                            collection_index,
                            segment,
                            resolved.record,
                            None,
                        )?;
                        let hit = segment_locator_hit(
                            collection,
                            resolved,
                            revision,
                            exact_fields(slot.0),
                        );
                        offer_merged(
                            &mut winners,
                            cap,
                            MergedPageHit {
                                key,
                                hit,
                                segment: Some((collection_index, segment_index)),
                                record_address: None,
                                retained_bytes: 0,
                            },
                            winner_budget,
                            &mut winner_bytes,
                        )?;
                        selected += 1;
                    }
                    continue;
                }
                for slot in candidates.iter() {
                    if let Some(context) = context {
                        context.check_string()?;
                    }
                    total += 1;
                    counters.exact_posting_matches += 1;
                    offer_admitted_segment_slot(
                        repo,
                        &sort,
                        collection,
                        segment,
                        published,
                        catalog,
                        collection_index,
                        segment_index,
                        revision,
                        RecordSlot(slot),
                        || exact_fields(slot),
                        cache_builder.as_mut(),
                        &mut winners,
                        cap,
                        winner_budget,
                        &mut winner_bytes,
                    )?;
                }
                continue;
            }
            let filter_proof = if fts.is_some()
                && let (Some(filter), Some(bounds)) = (filter.as_ref(), bounds.as_ref())
                && candidates.is_subset(&bounds.must)
            {
                if query.options.evidence {
                    segment_static_evidence_fields(filter)
                } else {
                    Some(Vec::new())
                }
            } else {
                None
            };
            let fused_filter_fts_terms = (filter_proof.is_none()
                && filter.is_some()
                && fts.is_some()
                && metadata_evidence.is_none())
            .then(|| {
                entry
                    .filter_shape
                    .as_ref()
                    .map(PreparedExpr::term_ids)
                    .unwrap_or_default()
                    .into_iter()
                    .chain(entry.text_term)
                    .collect::<Vec<_>>()
            });
            // Segment slots normally follow file order. Group consecutive
            // file IDs without a tree node per candidate; sort and merge if
            // a mutation has produced an out-of-order slot layout.
            let mut files: Vec<(u32, SmallVec<[(u32, ByteRange); 1]>)> = Vec::new();
            let mut ordered_files = true;
            for slot in candidates.iter() {
                let record = segment
                    .record(RecordSlot(slot))
                    .ok_or("segment candidate missing")?;
                if let Some(context) = context
                    && record.range.len() > context.limits.source_record_bytes
                {
                    return Err(context.fail(
                        ExecutionErrorKind::Resource,
                        "segment record exceeds source read allowance",
                    ));
                }
                if let Some((previous_file, ranges)) = files.last_mut() {
                    ordered_files &= *previous_file <= record.file.0;
                    if *previous_file == record.file.0 {
                        ranges.push((slot, record.range));
                        continue;
                    }
                }
                files.push((record.file.0, SmallVec::from_buf([(slot, record.range)])));
            }
            let files = order_segment_file_groups(files, ordered_files);
            // Local residual verification is independent by source file. Limit the
            // parallel route to small ordered pages and bounded source rows;
            // all other plans keep the existing serial, cache-aware path.
            let single_record_files = files.iter().all(|(_, ranges)| ranges.len() == 1);
            let small_single_record_files = single_record_files
                && files
                    .iter()
                    .all(|(_, ranges)| ranges[0].1.len() <= 8 * 1024);
            let parallel_scan = context.is_none()
                && retained_budget.is_none()
                && cache_builder.is_none()
                && (filter.is_some() || fts.is_some())
                && (!sort.bound.has_computed
                    || (query.options.sort.len() <= 4 && small_single_record_files))
                && repo.options.query_workers > 1
                && files.len() > 1
                && (cap <= 64 || (cap <= 256 && small_single_record_files))
                && collection.definition.name.len() <= 1024
                && filter.as_ref().is_none_or(|filter| {
                    segment_filter_evidence_bytes(filter).is_some_and(|bytes| bytes <= 1024)
                })
                && fts.as_ref().is_none_or(|fts| {
                    fts.fields
                        .iter()
                        .try_fold(0usize, |bytes, field| {
                            bytes.checked_add(field.evidence_path.len())
                        })
                        .is_some_and(|bytes| bytes <= 1024)
                })
                && files.iter().all(|(file_id, ranges)| {
                    catalog
                        .entry(crate::model::FileId(*file_id))
                        .is_some_and(|file| file.relative_path.to_string_lossy().len() <= 4096)
                        && ranges.iter().all(|(slot, range)| {
                            range.len() <= 1024 * 1024
                                && segment
                                    .record(RecordSlot(*slot))
                                    .is_some_and(|record| record.id.0.len() <= 1024)
                        })
                });
            if parallel_scan {
                let jobs = files;
                // One-record files retain at most one compact winner per job.
                // Group their work to amortize pool setup while active workers
                // still bound source buffers by the query worker count.
                let batch_size = if single_record_files {
                    2048
                } else {
                    repo.options.query_workers.clamp(1, 32)
                };
                let mut fused_scanned = 0u64;
                let mut fused_matches = 0u64;
                let mut fused_residuals = 0u64;
                for chunk in jobs.chunks(batch_size) {
                    let scans = repo.workers.install(|| {
                        chunk
                            .par_iter()
                            .map(|(file_id, ranges)| {
                                scan_segment_file_parallel(
                                    repo,
                                    query,
                                    &sort,
                                    collection_index,
                                    published,
                                    catalog,
                                    filter.as_ref(),
                                    fts.as_ref(),
                                    direct_plan.as_ref(),
                                    &projection,
                                    filter_proof.as_deref(),
                                    *file_id,
                                    ranges,
                                    cap,
                                )
                            })
                            .collect::<Vec<_>>()
                    });
                    // Rayon preserves the order of this indexed iterator, so
                    // errors, counters and tie handling merge in file order.
                    for ((file_id, _), scan) in chunk.iter().zip(scans) {
                        let scan = scan?;
                        fused_scanned += scan.scanned_records;
                        fused_matches += scan.matches;
                        fused_residuals += scan.residual_evaluations;
                        delta_opened.insert(*file_id);
                        counters.files_opened += 1;
                        counters.bytes_read += scan.bytes_read;
                        counters.scanned_records += scan.scanned_records;
                        counters.record_parses += scan.record_parses;
                        counters.residual_evaluations += scan.residual_evaluations;
                        counters.raw_text_verifications += scan.raw_text_verifications;
                        counters.sort_key_evaluations += scan.sort_key_evaluations;
                        total += scan.matches;
                        for winner in scan.winners {
                            if cap == 0
                                || (winners.len() >= cap
                                    && winners.peek().is_some_and(|worst| winner.key >= worst.key))
                            {
                                continue;
                            }
                            let resolved = segment_record_ref(
                                segment,
                                &published.active,
                                catalog,
                                winner.slot,
                            )?
                            .ok_or("segment winner disappeared before merge")?;
                            let matched_fields = if let Some(source_bytes) =
                                winner.source_bytes.as_deref()
                            {
                                let (_, matched, evidence, _) = verify_local_segment_record(
                                    repo,
                                    collection,
                                    resolved.record,
                                    resolved.file,
                                    source_bytes,
                                    filter.as_ref(),
                                    fts.as_ref(),
                                    direct_plan.as_ref(),
                                    &projection,
                                    filter_proof.as_deref(),
                                    true,
                                    None,
                                )?;
                                if !matched {
                                    return Err(
                                        "segment winner changed during evidence evaluation".into(),
                                    );
                                }
                                evidence.into_iter().collect()
                            } else {
                                winner.matched_fields
                            };
                            let hit =
                                segment_locator_hit(collection, resolved, revision, matched_fields);
                            offer_merged(
                                &mut winners,
                                cap,
                                MergedPageHit {
                                    key: winner.key,
                                    hit,
                                    segment: Some((collection_index, segment_index)),
                                    record_address: None,
                                    retained_bytes: 0,
                                },
                                winner_budget,
                                &mut winner_bytes,
                            )?;
                        }
                    }
                }
                if let Some(terms) = fused_filter_fts_terms.as_deref()
                    && terms.len() > 1
                    && fused_scanned > 0
                {
                    crate::query_execution::request::record_fused_observation_with_residual(
                        repo.instance_id,
                        terms,
                        crate::query_execution::domain::domain_id(
                            repo.instance_id,
                            collection.generation.collection.0,
                            collection.generation.generation,
                            Some(segment.id.get()),
                        ),
                        collection.generation.collection.0,
                        fused_scanned,
                        fused_matches,
                        fused_matches,
                        fused_residuals,
                        "fused_filter_fts_source",
                    );
                }
                continue;
            }
            let mut fused_scanned = 0u64;
            let mut fused_matches = 0u64;
            let mut fused_residuals = 0u64;
            for (file_id, ranges) in files {
                if let Some(context) = context {
                    context.check_string()?;
                    context.charge_reads(1)?;
                }
                let file = segment_active_file(
                    segment,
                    &published.active,
                    catalog,
                    crate::model::FileId(file_id),
                )?;
                let path = repo.files.root.join(&file.relative_path);
                let mut reader = RecordRangeReader::open_for_search(&path, file.revision, context)?;
                delta_opened.insert(file_id);
                counters.files_opened += 1;
                let mut cursor = 0;
                while cursor < ranges.len() {
                    let start = cursor;
                    let mut batch_bytes = 0u64;
                    while cursor < ranges.len() && cursor - start < 128 {
                        let next = ranges[cursor].1.len();
                        if cursor > start && batch_bytes.saturating_add(next) > 1024 * 1024 {
                            break;
                        }
                        batch_bytes = batch_bytes.saturating_add(next);
                        cursor += 1;
                    }
                    let (read, bytes_read) = reader.read(&ranges[start..cursor])?;
                    if let Some(context) = context {
                        context.charge_bytes(bytes_read)?;
                    }
                    counters.bytes_read += bytes_read;
                    for (slot, bytes) in read {
                        if let Some(context) = context {
                            context.check_string()?;
                        }
                        if !published.active.contains(RecordSlot(slot)) {
                            return Err("inactive segment scan slot".into());
                        }
                        let record = segment
                            .record(RecordSlot(slot))
                            .ok_or("segment scan slot missing")?;
                        if record.file.0 != file_id {
                            return Err("segment scan file ownership mismatch".into());
                        }
                        let resolved = SegmentRecordRef { record, file };
                        let record = resolved.record;
                        counters.record_parses += u64::from(
                            record.class == RecordClass::Logical
                                && (direct_plan.is_none()
                                    || fts.as_ref().is_some_and(|fts| {
                                        fts.fields.is_empty()
                                            || fts
                                                .fields
                                                .iter()
                                                .any(|field| field.binding_name != "default")
                                    })),
                        );
                        counters.residual_evaluations +=
                            u64::from(filter.is_some() && filter_proof.is_none());
                        let (extracted, matched, first_evidence, raw_checked) =
                            verify_local_segment_record(
                                repo,
                                collection,
                                record,
                                resolved.file,
                                &bytes,
                                filter.as_ref(),
                                fts.as_ref(),
                                direct_plan.as_ref(),
                                &projection,
                                filter_proof.as_deref(),
                                query.options.evidence
                                    && (context.is_some() || direct_plan.is_none()),
                                context,
                            )?;
                        counters.raw_text_verifications += u64::from(raw_checked);
                        counters.scanned_records += 1;
                        fused_scanned += 1;
                        fused_residuals += u64::from(filter.is_some() && filter_proof.is_none());
                        if !matched {
                            continue;
                        }
                        fused_matches += 1;
                        let ctx = RecordContext {
                            value: &extracted.value,
                            source_bytes: Some(&bytes),
                            record,
                            file: resolved.file,
                            collection_name: &collection.definition.name,
                            repository_root: &repo.files.root,
                        };
                        total += 1;
                        counters.sort_key_evaluations += u64::from(sort.bound.has_computed);
                        let key = segment_rank_key(
                            repo,
                            &sort,
                            collection_index,
                            segment,
                            record,
                            Some(&ctx),
                        )?;
                        if sort.bound.has_computed {
                            check_execution_sort_budget(context, 0, rank_key_bytes(&key), 0)?;
                        }
                        if cache_builder.is_none()
                            && (cap == 0
                                || (winners.len() >= cap
                                    && winners.peek().is_some_and(|worst| key >= worst.key)))
                        {
                            continue;
                        }
                        let matched_fields = if query.options.evidence {
                            if context.is_some() || direct_plan.is_none() {
                                first_evidence.into_iter().collect()
                            } else {
                                let (_, still_matched, evidence, _) = verify_local_segment_record(
                                    repo,
                                    collection,
                                    record,
                                    resolved.file,
                                    &bytes,
                                    filter.as_ref(),
                                    fts.as_ref(),
                                    direct_plan.as_ref(),
                                    &projection,
                                    filter_proof.as_deref(),
                                    true,
                                    context,
                                )?;
                                if !still_matched {
                                    return Err(
                                        "segment match changed during evidence evaluation".into()
                                    );
                                }
                                evidence.into_iter().collect()
                            }
                        } else {
                            Vec::new()
                        };
                        let matched_fields: Arc<[String]> = matched_fields.into();
                        let hit = segment_locator_hit(
                            collection,
                            resolved,
                            revision,
                            matched_fields.to_vec(),
                        );
                        if let Some(builder) = cache_builder.as_mut() {
                            builder.push(
                                &key,
                                CachedMember {
                                    collection_index,
                                    segment_index: Some(segment_index),
                                    slot,
                                    matched_fields,
                                },
                            );
                        }
                        offer_merged(
                            &mut winners,
                            cap,
                            MergedPageHit {
                                key,
                                hit,
                                segment: Some((collection_index, segment_index)),
                                record_address: None,
                                retained_bytes: 0,
                            },
                            winner_budget,
                            &mut winner_bytes,
                        )?;
                    }
                }
            }
            if let Some(terms) = fused_filter_fts_terms.as_deref()
                && terms.len() > 1
                && fused_scanned > 0
            {
                crate::query_execution::request::record_fused_observation_with_residual(
                    repo.instance_id,
                    terms,
                    crate::query_execution::domain::domain_id(
                        repo.instance_id,
                        collection.generation.collection.0,
                        collection.generation.generation,
                        Some(segment.id.get()),
                    ),
                    collection.generation.collection.0,
                    fused_scanned,
                    fused_matches,
                    fused_matches,
                    fused_residuals,
                    "fused_filter_fts_source",
                );
            }
        }
    }
    let mut page = winners.into_sorted_vec();
    page = page
        .into_iter()
        .skip(query.options.offset)
        .take(query.options.limit)
        .collect();
    if query.options.return_mode != ReturnMode::Locator {
        let mut retained = Vec::with_capacity(page.len());
        let mut selected_logical_parses = 0u64;
        let mut selected_files = HashSet::new();
        for mut winner in page {
            if let Some((collection_index, segment_index)) = winner.segment {
                if let Some(context) = context {
                    context.check_string()?;
                    let bytes = winner.hit.byte_end.saturating_sub(winner.hit.byte_start);
                    if bytes > context.limits.source_record_bytes {
                        return Err(context.fail(
                            ExecutionErrorKind::Resource,
                            "selected source record exceeds read allowance",
                        ));
                    }
                    context.charge_reads(1)?;
                    context.charge_bytes(bytes)?;
                }
                let published = &segments[collection_index][segment_index];
                let materialized = get_segment_record_by_id(
                    repo,
                    collection_index,
                    &published.index,
                    &published.active,
                    catalog,
                    revision,
                    &winner.hit.record_id,
                    query.options.return_mode.clone(),
                );
                let mut materialized = match materialized {
                    Ok(Some(materialized)) => materialized,
                    Ok(None) => return Err("selected segment record retired".into()),
                    Err(message) => {
                        if record_unavailable_hit(
                            repo,
                            catalog,
                            SourceHitLocator::from(&winner.hit),
                            message.clone(),
                            &mut counters,
                        )? {
                            continue;
                        }
                        return Err(message);
                    }
                };
                if query.options.return_mode == ReturnMode::Bytes
                    || winner.hit.record_class != RecordClass::RawFallback
                {
                    let bytes = winner.hit.byte_end.saturating_sub(winner.hit.byte_start);
                    selected_files.insert(winner.hit.file_id);
                    counters.selected_payload_reads += 1;
                    counters.selected_payload_bytes += bytes;
                }
                if query.options.return_mode == ReturnMode::Logical
                    && winner.hit.record_class == RecordClass::Logical
                {
                    selected_logical_parses += 1;
                }
                materialized.matched_fields = winner.hit.matched_fields.clone();
                winner.hit = materialized;
            }
            retained.push(winner);
        }
        page = retained;
        counters.record_parses = counters.record_parses.max(selected_logical_parses);
        counters.files_opened = counters.files_opened.saturating_add(
            selected_files
                .iter()
                .filter(|file| !delta_opened.contains(file))
                .count() as u64,
        );
        counters.distinct_files = counters
            .distinct_files
            .saturating_add(selected_files.len() as u64);
    }
    counters.total_matches = total;
    if exact_or_members > 0 {
        counters.exact_posting_matches = exact_or_members;
        if query.options.evidence {
            counters.residual_evaluations = counters
                .residual_evaluations
                .saturating_sub(exact_or_members);
        }
    }
    counters.match_count_complete = base_count_complete && counters.source_unavailable_events == 0;
    counters.returned_records = page.len() as u64;
    counters.distinct_files += delta_opened.len() as u64;
    if counters.match_count_complete
        && let (Some(builder), Some((scope, key, _))) = (cache_builder, cache_key)
        && let Some(members) = builder.finish(total)
    {
        if let Some(context) = context {
            context.check_string()?;
        }
        let displaced = scope
            .store
            .lock()
            .map_err(|error| error.to_string())?
            .insert(key, members);
        drop(displaced);
    }
    Ok(SearchResult {
        count: (query.options.count && counters.match_count_complete).then_some(total as usize),
        hits: page.into_iter().map(|winner| winner.hit).collect(),
        counters,
    })
}

/// Reuse complete verified ordering without repeating residual evaluation or
/// sorting. Source payloads are always read against this pinned catalog.

fn collect_positive_relation_terms<'a>(
    expr: &'a BoundExpr,
    output: &mut Vec<&'a crate::query::binding::filter::BoundTerm>,
) {
    match expr {
        BoundExpr::Term(term) if term.related.is_some() => output.push(term),
        BoundExpr::And(parts) => {
            for part in parts {
                collect_positive_relation_terms(part, output);
            }
        }
        BoundExpr::Term(_) | BoundExpr::Or(_) | BoundExpr::Not(_) => {}
    }
}

fn collect_plain_terms<'a>(expr: &'a BoundExpr, output: &mut Vec<&'a BoundTerm>) {
    match expr {
        BoundExpr::Term(term)
            if term.related.is_none() && term.rhs.is_none() && !term.query_path.has_position =>
        {
            output.push(term);
        }
        BoundExpr::And(parts) | BoundExpr::Or(parts) => {
            for part in parts {
                collect_plain_terms(part, output);
            }
        }
        BoundExpr::Term(_) | BoundExpr::Not(_) => {}
    }
}

fn collect_file_path_terms<'a>(
    expr: &'a BoundExpr,
    output: &mut Vec<&'a crate::query::binding::filter::BoundTerm>,
) {
    match expr {
        BoundExpr::Term(term)
            if term.path.root == PathRoot::Metadata(crate::model::MetadataField::FilePath) =>
        {
            output.push(term);
        }
        BoundExpr::And(parts) | BoundExpr::Or(parts) => {
            for part in parts {
                collect_file_path_terms(part, output);
            }
        }
        BoundExpr::Term(_) | BoundExpr::Not(_) => {}
    }
}

fn or_branch_counts(
    repo: &Repository,
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    collection_index: usize,
    filter: &BoundExpr,
) -> Result<HashMap<(usize, usize), u64>, String> {
    let BoundExpr::Or(parts) = filter else {
        return Ok(HashMap::new());
    };
    let collection = &repo.collections[collection_index];
    let mut counts = HashMap::new();
    for part in parts {
        let BoundExpr::Term(term) = part else {
            continue;
        };
        let mut count = 0u64;
        for published in segments[collection_index].iter() {
            let (_, estimated) = estimate_expr(collection, part, published.active.len())?;
            count = count.saturating_add(estimated.min(published.active.len()));
        }
        counts.insert(term.span, count);
    }
    Ok(counts)
}

fn relation_source_input_estimate(
    repo: &Repository,
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    collection_index: usize,
    filter: &BoundExpr,
    relation_term: &crate::query::binding::filter::BoundTerm,
) -> Result<u64, String> {
    let _ = relation_term;
    segment_filter_input_estimate(repo, segments, collection_index, Some(filter))
}

fn segment_filter_input_estimate(
    repo: &Repository,
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    collection_index: usize,
    filter: Option<&BoundExpr>,
) -> Result<u64, String> {
    let Some(filter) = filter else {
        return Ok(segments[collection_index]
            .iter()
            .map(|published| published.active.len())
            .sum());
    };
    let collection = &repo.collections[collection_index];
    let mut estimate = 0u64;
    for published in segments[collection_index].iter() {
        let (_, estimated) = estimate_expr(collection, filter, published.active.len())?;
        estimate = estimate.saturating_add(estimated.min(published.active.len()));
    }
    Ok(estimate)
}

pub(crate) fn search_analyze_segmented(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
    revision: IndexRevision,
) -> Result<(SearchResult, Vec<QueryPlanTrace>), String> {
    // Build the explanation from planning metadata before execution. It is
    // never patched afterwards with guessed scanner, relation or ordering
    // strategies.
    let mut traces = explain_segmented(repo, query, base_active, segments, revision)?;
    crate::query_execution::request::begin_observation_capture(repo.instance_id);
    let collector = Arc::new(crate::query_execution::request::ObservationCollector::new());
    let execution = ExecutionContext::new(
        crate::query::clock::ClockContext::new(time::OffsetDateTime::now_utc()),
        ExecutionLimits::default(),
        None,
    )
    .with_observation_collector(Arc::clone(&collector));
    let cache = LifecycleCacheScope {
        store: &repo.verified,
        instance_id: repo.instance_id,
    };
    let result = match search_segmented_with_context_lifecycle(
        repo,
        query,
        base_active,
        segments,
        catalog,
        revision,
        &execution,
        Some(cache),
    ) {
        Ok(result) => result,
        Err(error) => {
            crate::query_execution::request::cancel_observation_capture(repo.instance_id);
            return Err(error.message);
        }
    };
    let mut observations = collector.take();
    observations.extend(crate::query_execution::request::finish_observation_capture(
        repo.instance_id,
    ));
    for (node_id, observation) in observations.iter_mut().enumerate() {
        observation.node_id = node_id as u32;
    }
    let authoritative_phase = ["exact_count", "search", "ordered_page"]
        .into_iter()
        .find(|phase| {
            observations
                .iter()
                .any(|observation| observation.phase == *phase)
        });
    let source_candidate_domains = observations
        .iter()
        .filter(|observation| observation.access_kind.as_deref() == Some("source_candidates"))
        .map(|observation| (observation.collection, observation.domain.clone()))
        .collect::<HashSet<_>>();
    let mut domain_terminal = HashMap::<(u32, String), (u32, u64)>::new();
    for observation in &observations {
        if Some(observation.phase.as_str()) != authoritative_phase {
            continue;
        }
        let domain_key = (observation.collection, observation.domain.clone());
        if source_candidate_domains.contains(&domain_key)
            && observation.access_kind.as_deref() != Some("source_candidates")
        {
            continue;
        }
        let entry = domain_terminal
            .entry(domain_key)
            .or_insert((observation.node_id, observation.candidate_records));
        if observation.node_id >= entry.0 {
            *entry = (observation.node_id, observation.candidate_records);
        }
    }
    let collection_candidate_counts = domain_terminal.into_iter().fold(
        HashMap::<u32, u64>::new(),
        |mut counts, ((collection, _), (_, count))| {
            *counts.entry(collection).or_default() += count;
            counts
        },
    );
    let collection_actuals = repo
        .collections
        .iter()
        .map(|collection| {
            let actual = observations
                .iter()
                .filter(|observation| observation.collection == collection.generation.collection.0)
                .fold(PlanActual::default(), |mut actual, observation| {
                    actual.index_proven_matches = actual
                        .index_proven_matches
                        .saturating_add(observation.proven_records);
                    actual.residual_evaluations = actual
                        .residual_evaluations
                        .saturating_add(observation.residual_verifications);
                    if observation.access_kind.as_deref() == Some("relation_target_first")
                        || observation.access_kind.as_deref() == Some("relation_source_restricted")
                    {
                        actual.relation_target_candidates = actual
                            .relation_target_candidates
                            .saturating_add(observation.candidate_records);
                        actual.relation_target_matches = actual
                            .relation_target_matches
                            .saturating_add(observation.proven_records);
                    }
                    actual
                });
            (collection.definition.name.as_str(), actual)
        })
        .collect::<HashMap<_, _>>();
    attach_execution_observations(repo, &mut traces, observations);
    if traces.len() == 1 {
        let trace = &mut traces[0];
        let counters = &result.counters;
        let selected_collection_id = repo
            .collections
            .iter()
            .find(|collection| collection.definition.name == trace.collection)
            .map(|collection| collection.generation.collection.0);
        trace.actual = Some(PlanActual {
            files_opened: counters.files_opened,
            bytes_read: counters.bytes_read,
            record_parses: counters.record_parses,
            raw_text_verifications: counters.raw_text_verifications,
            residual_evaluations: counters.residual_evaluations,
            index_proven_matches: counters.exact_posting_matches,
            relation_target_candidates: counters.relation_target_candidates,
            relation_target_matches: counters.relation_target_matches,
            relation_target_reads: counters.relation_target_reads,
            relation_target_file_opens: counters.relation_target_file_opens,
            relation_edge_visits: counters.relation_edge_visits,
            relation_backlink_probe_visits: counters.relation_backlink_probe_visits,
            relation_bytes_read: counters.relation_bytes_read,
            relation_reverse_postings_visited: counters.relation_reverse_postings_visited,
            relation_reverse_members_visited: counters.relation_reverse_members_visited,
            relation_verified_targets_reused: counters.relation_verified_targets_reused,
        });
        if let Some(candidates) = selected_collection_id
            .and_then(|collection| collection_candidate_counts.get(&collection))
        {
            trace.candidates = *candidates;
            trace.candidates_estimated = false;
        }
        return Ok((result, traces));
    }
    for trace in &mut traces {
        if let Some(collection) = repo
            .collections
            .iter()
            .find(|collection| collection.definition.name == trace.collection)
            && let Some(candidates) =
                collection_candidate_counts.get(&collection.generation.collection.0)
        {
            trace.candidates = *candidates;
            trace.candidates_estimated = false;
        }
        if let Some(actual) = collection_actuals.get(trace.collection.as_str()) {
            trace.actual = Some(actual.clone());
            trace.candidates_estimated = result.counters.match_count_complete;
        }
    }
    Ok((result, traces))
}

fn segmented_stream_error(
    delivery: &mut Delivery<'_>,
    context: &ExecutionContext,
    message: String,
) -> Result<StreamOutcome, StreamError> {
    match delivery.abort.take() {
        Some(Abort::Cancelled) => Ok(StreamOutcome::Cancelled(delivery.progress)),
        Some(Abort::Sink(message)) => Err(StreamError::Sink {
            message,
            progress: delivery.progress,
        }),
        Some(Abort::Resource(message)) => Err(StreamError::Resource {
            message,
            progress: delivery.progress,
        }),
        None => {
            let error = context.recorded_error(message);
            match error.kind {
                ExecutionErrorKind::Cancelled => Ok(StreamOutcome::Cancelled(delivery.progress)),
                ExecutionErrorKind::Resource => Err(StreamError::Resource {
                    message: error.message,
                    progress: delivery.progress,
                }),
                kind => Err(StreamError::Execution {
                    kind,
                    message: error.message,
                    progress: delivery.progress,
                }),
            }
        }
    }
}

fn materialize_segmented_stream_hit(
    repo: &Repository,
    mut hit: SearchHit,
    mode: ReturnMode,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
    revision: IndexRevision,
    context: &ExecutionContext,
) -> Result<SearchHit, String> {
    if mode == ReturnMode::Locator {
        return Ok(hit);
    }
    context.check_string()?;
    if hit.byte_end.saturating_sub(hit.byte_start) > context.limits.source_record_bytes {
        return Err(context.fail(
            ExecutionErrorKind::Resource,
            "stream record exceeds source read allowance",
        ));
    }
    context.charge_reads(1)?;
    context.charge_bytes(hit.byte_end.saturating_sub(hit.byte_start))?;
    let collection_index = repo
        .collections
        .iter()
        .position(|collection| collection.definition.name == hit.collection)
        .ok_or("stream hit collection missing")?;
    let materialized = repo.get_record_with_active(
        &hit.collection,
        &hit.record_id,
        mode.clone(),
        Some(base_active),
    )?;
    let mut materialized = if let Some(materialized) = materialized {
        materialized
    } else {
        let mut found = None;
        for segment in segments[collection_index].iter() {
            if let Some(value) = get_segment_record_by_id(
                repo,
                collection_index,
                &segment.index,
                &segment.active,
                catalog,
                revision,
                &hit.record_id,
                mode.clone(),
            )? {
                found = Some(value);
                break;
            }
        }
        found.ok_or("stream hit no longer resolves in pinned view")?
    };
    repo.stamp_native_hit_at(&mut materialized, revision);
    materialized.matched_fields = std::mem::take(&mut hit.matched_fields);
    Ok(materialized)
}

fn lifecycle_segment_order<'a>(
    segment: &'a IndexedCollectionSegment,
    sort: &ResolvedSort<'_>,
    query: &Query,
    collection_index: usize,
) -> Option<&'a [RecordSlot]> {
    if query.options.sort.len() > 1 {
        return segment
            .sorting
            .composite(&query.options.sort)
            .map(|order| order.order.as_slice());
    }
    if query.options.sort.is_empty() {
        return Some(&segment.identity_order);
    }
    let term = sort.bound.terms[collection_index].as_ref()?.first()?;
    let BoundSortSource::Stored(base_column) = &term.source else {
        return None;
    };
    let column = segment.sorting.columns.get(&base_column.path)?;
    Some(if term.descending {
        &column.descending
    } else {
        &column.ascending
    })
}

fn push_lifecycle_order_cursor(
    repo: &Repository,
    query: &Query,
    sort: &ResolvedSort<'_>,
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    candidates: &[Vec<Option<RoaringBitmap>>],
    collection_index: usize,
    segment_index: usize,
    mut position: usize,
    heap: &mut BinaryHeap<Reverse<LifecycleOrderCursor>>,
    heap_bytes: &mut usize,
    budget: usize,
    context: &ExecutionContext,
) -> Result<(), String> {
    let published = &segments[collection_index][segment_index];
    let order = lifecycle_segment_order(&published.index, sort, query, collection_index)
        .ok_or("lifecycle stored order unavailable")?;
    while let Some(slot) = order.get(position).copied() {
        position += 1;
        if !published.active.contains(slot)
            || candidates[collection_index][segment_index]
                .as_ref()
                .is_some_and(|may| !may.contains(slot.0))
        {
            continue;
        }
        let record = published
            .index
            .record(slot)
            .ok_or("lifecycle ordered slot missing")?;
        let key = segment_rank_key(repo, sort, collection_index, &published.index, record, None)?;
        let bytes = usize::try_from(rank_key_bytes(&key)).map_err(|_| {
            context.fail(
                ExecutionErrorKind::Resource,
                "ordered sort key size overflow",
            )
        })?;
        let next = heap_bytes.checked_add(bytes).ok_or_else(|| {
            context.fail(
                ExecutionErrorKind::Resource,
                "ordered cursor memory overflow",
            )
        })?;
        if next > budget {
            return Err(context.fail(
                ExecutionErrorKind::Resource,
                "ordered cursors exceed stream memory budget",
            ));
        }
        *heap_bytes = next;
        heap.push(Reverse(LifecycleOrderCursor {
            key,
            bytes,
            collection_index,
            segment_index,
            position,
            slot,
        }));
        break;
    }
    Ok(())
}

/// Reuse one checked source handle while adjacent rows in a stored order
/// belong to the same file. This avoids reopening a large JSONL file for
/// every ordered row without retaining file bodies or an unbounded handle set.
/// Traverse resident sort/identity orders across lifecycle partitions. The
/// first final page can be emitted before an optional exact-count pass reaches
/// later source records. Computed and relation-dependent ordering retain the
/// bounded general planner path.
#[allow(clippy::too_many_arguments)]
fn try_lifecycle_ordered_stream(
    repo: &Repository,
    request: &Query,
    output_mode: ReturnMode,
    scope: ResultScope,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
    revision: IndexRevision,
    context: &ExecutionContext,
    cache: Option<LifecycleCacheScope<'_>>,
    delivery: &mut Delivery<'_>,
) -> Result<Option<StreamOutcome>, String> {
    if base_active.iter().any(|active| active.len() != 0)
        || matches!(scope, ResultScope::Page { limit: 0, .. })
    {
        return Ok(None);
    }
    let sort = ResolvedSort::new(repo, request)?;
    if sort.bound.has_computed
        || sort
            .bound
            .terms
            .iter()
            .flatten()
            .flatten()
            .any(|term| matches!(&term.source, BoundSortSource::Computed(_)))
    {
        return Ok(None);
    }
    let prepared = crate::query::prepared::prepare(repo, request)?;
    let predicates = Arc::clone(&prepared.predicates);
    if prepared.collections.iter().any(|entry| {
        entry.filter.as_ref().is_some_and(bound_expr_has_relations)
            || entry
                .fts
                .as_ref()
                .is_some_and(|fts| fts.fields.iter().any(|field| field.related.is_some()))
    }) {
        return Ok(None);
    }
    for entry in &prepared.collections {
        let collection_index = entry.collection_index;
        for segment in segments[collection_index].iter() {
            if lifecycle_segment_order(&segment.index, &sort, request, collection_index).is_none() {
                return Ok(None);
            }
        }
    }
    let mut filters = (0..repo.collections.len())
        .map(|_| None)
        .collect::<Vec<_>>();
    let mut candidates = segments
        .iter()
        .map(|group| (0..group.len()).map(|_| None).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    let mut filter_exact = segments
        .iter()
        .map(|group| vec![false; group.len()])
        .collect::<Vec<_>>();
    for entry in prepared.collections {
        let collection_index = entry.collection_index;
        let filter = entry.filter;
        let fts = entry.fts;
        for (segment_index, segment) in segments[collection_index].iter().enumerate() {
            let collection = &repo.collections[collection_index];
            let filter_may = filter
                .as_ref()
                .map(|filter| {
                    let active = segment.active.bitmap();
                    let bounds = segment_filter_bounds_with_input_catalog_and_predicates(
                        &segment.index,
                        &segment.active,
                        filter,
                        collection,
                        Some(catalog),
                        &active,
                        Some(&predicates),
                        Some(context),
                    )?;
                    let logical = &segment.active.bitmap() & &segment.index.logical_records;
                    filter_exact[collection_index][segment_index] = bounds.may == bounds.must
                        || (excludes_raw_fallback(filter)
                            && (bounds.may.as_ref() & &logical)
                                == (bounds.must.as_ref() & &logical));
                    Ok::<_, String>(if filter_exact[collection_index][segment_index] {
                        bounds.must
                    } else {
                        bounds.may
                    })
                })
                .transpose()?;
            let fts_may = fts
                .as_ref()
                .zip(entry.text_term)
                .map(|(fts, term)| {
                    let active = segment.active.bitmap();
                    let input = filter_may
                        .as_ref()
                        .map_or(&active, |filter| filter.as_ref());
                    segment_fts_bounds_with_term(
                        &segment.index,
                        collection,
                        fts,
                        term,
                        input,
                        Some(context),
                    )
                    .map(|bounds| bounds.may.as_ref().clone())
                })
                .transpose()?;
            candidates[collection_index][segment_index] = match (filter_may, fts_may) {
                (Some(filter), Some(fts)) => Some(filter.as_ref() & fts),
                (Some(filter), None) => Some(filter.as_ref().clone()),
                (None, Some(fts)) => Some(fts),
                (None, None) => None,
            };
        }
        filters[collection_index] = Some((filter, fts));
    }
    let candidate_total = candidates
        .iter()
        .flatten()
        .flatten()
        .try_fold(0usize, |total, candidates| {
            total.checked_add(candidates.len() as usize)
        })
        .ok_or_else(|| {
            context.fail(
                ExecutionErrorKind::Resource,
                "ordered candidate count overflow",
            )
        })?;
    if candidate_total > 256
        && repo.options.query_workers > 1
        && filters.iter().any(|selected| {
            selected
                .as_ref()
                .is_some_and(|(filter, fts)| filter.is_some() || fts.is_some())
        })
    {
        // A stored cursor is best for selective verification and for one
        // large JSONL source. Above this small distinct-source threshold, the
        // file-parallel route amortizes verification across query workers.
        let mut candidate_files = HashSet::new();
        let mut parallel_sources = false;
        'segments: for (collection_index, group) in candidates.iter().enumerate() {
            for (segment_index, selected) in group.iter().enumerate() {
                let Some(selected) = selected else {
                    continue;
                };
                let segment = &segments[collection_index][segment_index].index;
                for slot in selected {
                    let record = segment
                        .record(RecordSlot(slot))
                        .ok_or("ordered candidate record missing")?;
                    candidate_files.insert(record.file);
                    if candidate_files.len() > 64 {
                        parallel_sources = true;
                        break 'segments;
                    }
                }
            }
        }
        if parallel_sources {
            return Ok(None);
        }
    }
    let direct_plans = filters
        .iter()
        .enumerate()
        .map(|(index, selected)| {
            selected.as_ref().and_then(|(filter, fts)| {
                DirectSourcePlan::prepare_bound(
                    &repo.collections[index],
                    &sort,
                    fts.as_ref(),
                    filter.as_ref(),
                )
            })
        })
        .collect::<Vec<_>>();
    let partition_count = filters
        .iter()
        .enumerate()
        .try_fold(0usize, |count, (index, selected)| {
            count.checked_add(if selected.is_some() {
                segments[index].len()
            } else {
                0
            })
        })
        .ok_or_else(|| {
            context.fail(
                ExecutionErrorKind::Resource,
                "ordered partition count overflow",
            )
        })?;
    let mut heap_bytes = partition_count
        .checked_mul(std::mem::size_of::<LifecycleOrderCursor>())
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<RecordRangeReader>()))
        .ok_or_else(|| {
            context.fail(
                ExecutionErrorKind::Resource,
                "ordered cursor memory overflow",
            )
        })?;
    if heap_bytes > delivery.max_buffered_bytes() {
        return Err(context.fail(
            ExecutionErrorKind::Resource,
            "ordered cursors exceed stream memory budget",
        ));
    }
    let mut heap = BinaryHeap::new();
    heap.try_reserve(partition_count).map_err(|_| {
        context.fail(
            ExecutionErrorKind::Resource,
            "ordered cursor allocation failed",
        )
    })?;
    for (collection_index, selected) in filters.iter().enumerate() {
        if selected.is_none() {
            continue;
        }
        for segment_index in 0..segments[collection_index].len() {
            push_lifecycle_order_cursor(
                repo,
                request,
                &sort,
                segments,
                &candidates,
                collection_index,
                segment_index,
                0,
                &mut heap,
                &mut heap_bytes,
                delivery.max_buffered_bytes(),
                context,
            )?;
        }
    }
    let mut page = Vec::new();
    let mut page_bytes = 0usize;
    let mut reader = None;
    let mut opened_files = HashSet::new();
    let mut files_since_checkpoint = 0usize;
    let mut counters = WorkCounters {
        match_count_complete: true,
        ..WorkCounters::default()
    };
    let offset = scope.offset();
    let limit = match scope {
        ResultScope::Page { limit, .. } => Some(limit),
        ResultScope::All { .. } => None,
    };
    let mut stopped_early = false;
    while let Some(Reverse(cursor)) = heap.pop() {
        delivery.check()?;
        context.check_string()?;
        heap_bytes = heap_bytes.saturating_sub(cursor.bytes);
        push_lifecycle_order_cursor(
            repo,
            request,
            &sort,
            segments,
            &candidates,
            cursor.collection_index,
            cursor.segment_index,
            cursor.position,
            &mut heap,
            &mut heap_bytes,
            delivery.max_buffered_bytes().saturating_sub(page_bytes),
            context,
        )?;
        let published = &segments[cursor.collection_index][cursor.segment_index];
        let Some(resolved) =
            segment_record_ref(&published.index, &published.active, catalog, cursor.slot)?
        else {
            continue;
        };
        let record = resolved.record;
        let file = resolved.file;
        let collection = &repo.collections[cursor.collection_index];
        counters.candidates += 1;
        let (filter, fts) = filters[cursor.collection_index]
            .as_ref()
            .ok_or("ordered collection filter missing")?;
        let mut evidence = HashSet::new();
        let must_verify = fts.is_some()
            || (filter.is_some() && !filter_exact[cursor.collection_index][cursor.segment_index]);
        if limit.is_none() && !page.is_empty() {
            let upcoming = usize::try_from(record.range.len()).unwrap_or(usize::MAX);
            if page.len() >= delivery.options().batch_rows
                || page_bytes
                    .saturating_add(upcoming)
                    .saturating_add(heap_bytes)
                    > delivery.max_buffered_bytes()
            {
                delivery.ordered_batch(&page, &counters)?;
                page.clear();
                page_bytes = 0;
            }
        }
        let mut bytes = None;
        let matched = if must_verify {
            if record.range.len() > context.limits.source_record_bytes {
                return Err(context.fail(
                    ExecutionErrorKind::Resource,
                    "ordered source record exceeds read allowance",
                ));
            }
            let source_len = usize::try_from(record.range.len()).map_err(|_| {
                context.fail(ExecutionErrorKind::Resource, "ordered source size overflow")
            })?;
            delivery.check_record_bytes(source_len)?;
            delivery.check_buffer(
                page_bytes
                    .checked_add(heap_bytes)
                    .and_then(|used| used.checked_add(source_len))
                    .ok_or_else(|| {
                        context.fail(
                            ExecutionErrorKind::Resource,
                            "ordered source memory overflow",
                        )
                    })?,
            )?;
            context.charge_reads(1)?;
            context.charge_bytes(record.range.len())?;
            let read = match read_lifecycle_ordered_record(
                repo,
                catalog,
                file,
                record,
                cursor.slot,
                &mut reader,
                delivery
                    .max_buffered_bytes()
                    .saturating_sub(heap_bytes)
                    .saturating_sub(page_bytes),
                context,
            ) {
                Ok((read, opened)) => {
                    counters.files_opened += u64::from(opened);
                    if opened {
                        opened_files.insert(file.id);
                        files_since_checkpoint += 1;
                    }
                    read
                }
                Err(message) => {
                    if delivery.options().mode == StreamMode::PageReplacements
                        && delivery.last_page_sequence() > 0
                    {
                        return Err(message);
                    }
                    let locator = segment_locator_hit(collection, resolved, revision, Vec::new());
                    if record_unavailable_hit(
                        repo,
                        catalog,
                        SourceHitLocator::from(&locator),
                        message.clone(),
                        &mut counters,
                    )? {
                        continue;
                    }
                    return Err(message);
                }
            };
            counters.bytes_read += read.len() as u64;
            counters.scanned_records += 1;
            counters.record_parses += u64::from(record.class == RecordClass::Logical);
            counters.residual_evaluations += u64::from(filter.is_some());
            let (_, matched, found_evidence, raw_checked) = verify_local_segment_record(
                repo,
                collection,
                record,
                file,
                &read,
                filter.as_ref(),
                fts.as_ref(),
                direct_plans[cursor.collection_index].as_ref(),
                &ScanProjection::full(
                    fts.is_some() && !borrowed_value_text(collection, fts.as_ref()),
                ),
                None,
                request.options.evidence,
                Some(context),
            )?;
            counters.raw_text_verifications += u64::from(raw_checked);
            evidence = found_evidence;
            bytes = Some(read);
            matched
        } else {
            counters.exact_posting_matches += 1;
            if request.options.evidence
                && let Some(fields) = filter.as_ref().and_then(segment_static_evidence_fields)
            {
                evidence.extend(fields);
            }
            true
        };
        if !matched {
            continue;
        }
        let position = counters.total_matches as usize;
        counters.total_matches += 1;
        if position < offset || limit.is_some_and(|limit| page.len() >= limit) {
            continue;
        }
        let mut hit = segment_locator_hit(
            collection,
            SegmentRecordRef { record, file },
            revision,
            if request.options.evidence {
                evidence.into_iter().collect()
            } else {
                Vec::new()
            },
        );
        repo.stamp_native_hit_at(&mut hit, revision);
        if output_mode != ReturnMode::Locator {
            if output_mode == ReturnMode::Logical && record.class == RecordClass::RawFallback {
                hit.payload = Some(Payload::LogicalUnavailable {
                    diagnostic_ref: record.diagnostic_ref,
                });
            } else {
                let source = if let Some(bytes) = bytes.take() {
                    bytes
                } else {
                    if record.range.len() > context.limits.source_record_bytes {
                        return Err(context.fail(
                            ExecutionErrorKind::Resource,
                            "ordered output record exceeds read allowance",
                        ));
                    }
                    let source_len = usize::try_from(record.range.len()).map_err(|_| {
                        context.fail(ExecutionErrorKind::Resource, "ordered output size overflow")
                    })?;
                    delivery.check_record_bytes(source_len)?;
                    delivery.check_buffer(
                        page_bytes
                            .checked_add(heap_bytes)
                            .and_then(|used| used.checked_add(source_len))
                            .ok_or_else(|| {
                                context.fail(
                                    ExecutionErrorKind::Resource,
                                    "ordered output memory overflow",
                                )
                            })?,
                    )?;
                    context.charge_reads(1)?;
                    context.charge_bytes(record.range.len())?;
                    let read = read_lifecycle_ordered_record(
                        repo,
                        catalog,
                        file,
                        record,
                        cursor.slot,
                        &mut reader,
                        delivery
                            .max_buffered_bytes()
                            .saturating_sub(heap_bytes)
                            .saturating_sub(page_bytes),
                        context,
                    );
                    let (read, opened) = match read {
                        Ok(read) => read,
                        Err(message) => {
                            if delivery.options().mode == StreamMode::PageReplacements
                                && delivery.last_page_sequence() > 0
                            {
                                return Err(message);
                            }
                            if record_unavailable_hit(
                                repo,
                                catalog,
                                SourceHitLocator::from(&hit),
                                message.clone(),
                                &mut counters,
                            )? {
                                continue;
                            }
                            return Err(message);
                        }
                    };
                    counters.files_opened += u64::from(opened);
                    if opened {
                        opened_files.insert(file.id);
                        files_since_checkpoint += 1;
                    }
                    counters.bytes_read += read.len() as u64;
                    read
                };
                let (payload, parses) = payload_from_bytes_with_execution(
                    record.class,
                    Some(collection),
                    &output_mode,
                    source,
                    Some(context),
                )?;
                hit.payload = payload;
                counters.record_parses += parses;
            }
        }
        let bytes = crate::query_execution::streaming::hit_bytes(&hit);
        if limit.is_none()
            && !page.is_empty()
            && (page.len() >= delivery.options().batch_rows
                || page_bytes.saturating_add(bytes) > delivery.options().max_event_bytes)
        {
            delivery.ordered_batch(&page, &counters)?;
            page.clear();
            page_bytes = 0;
        }
        let retained = page_bytes.checked_add(bytes).ok_or_else(|| {
            context.fail(
                ExecutionErrorKind::Resource,
                "ordered page byte count overflow",
            )
        })?;
        delivery.check_buffer(retained.saturating_add(heap_bytes))?;
        page_bytes = retained;
        page.push(hit);
        if delivery.options().mode == StreamMode::PageReplacements
            && files_since_checkpoint >= delivery.options().checkpoint_files
        {
            delivery.replacement(&page, &counters, heap_bytes)?;
            files_since_checkpoint = 0;
        }
        if let Some(limit) = limit
            && page.len() == limit
        {
            match delivery.options().mode {
                StreamMode::OrderedRows => {
                    delivery.ordered_rows(&page, &counters, true)?;
                    stopped_early = true;
                }
                StreamMode::PageReplacements => {
                    delivery.replacement(&page, &counters, heap_bytes)?
                }
            }
            if stopped_early {
                break;
            }
        }
    }
    if !stopped_early {
        if limit.is_none() {
            delivery.ordered_batch(&page, &counters)?;
            page.clear();
        } else {
            match delivery.options().mode {
                StreamMode::OrderedRows => delivery.ordered_rows(&page, &counters, true)?,
                StreamMode::PageReplacements => {
                    delivery.replacement(&page, &counters, heap_bytes)?
                }
            }
        }
    }
    let mut count = None;
    // A Locator-only count pass cannot erase a failed Bytes/Logical page read.
    if request.options.count && limit.is_some() && counters.source_unavailable_events == 0 {
        let mut count_query = request.clone();
        count_query.options.offset = 0;
        count_query.options.limit = 0;
        count_query.options.return_mode = ReturnMode::Locator;
        let result = search_segmented_with_context_lifecycle(
            repo,
            &count_query,
            base_active,
            segments,
            catalog,
            revision,
            context,
            cache,
        )
        .map_err(|error| error.message)?;
        count = result.count;
        counters = result.counters;
    } else if (request.options.count || limit.is_none()) && counters.source_unavailable_events == 0
    {
        count = Some(counters.total_matches as usize);
    }
    if stopped_early && count.is_none() {
        counters.match_count_complete = false;
    }
    counters.returned_records = delivery.progress.emitted_rows;
    counters.files_opened = counters.files_opened.max(opened_files.len() as u64);
    counters.distinct_files = opened_files.len() as u64;
    delivery.observe(&counters);
    delivery.check()?;
    Ok(Some(StreamOutcome::Complete(Box::new(StreamSummary {
        count,
        returned_rows: delivery.progress.emitted_rows as usize,
        final_sequence: delivery.last_page_sequence(),
        counters,
        progress: delivery.progress,
    }))))
}

#[cfg(test)]
pub(crate) fn search_stream_segmented_with_context(
    repo: &Repository,
    query: &Query,
    scope: ResultScope,
    options: StreamOptions,
    context: &ExecutionContext,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
    revision: IndexRevision,
    sink: &mut dyn for<'e> FnMut(StreamEvent<'e>) -> Result<StreamControl, String>,
) -> Result<StreamOutcome, StreamError> {
    search_stream_segmented_with_context_lifecycle(
        repo,
        query,
        scope,
        options,
        context,
        base_active,
        segments,
        catalog,
        revision,
        sink,
        None,
    )
}

pub(crate) fn search_stream_segmented_with_context_lifecycle(
    repo: &Repository,
    query: &Query,
    scope: ResultScope,
    options: StreamOptions,
    context: &ExecutionContext,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
    revision: IndexRevision,
    sink: &mut dyn for<'e> FnMut(StreamEvent<'e>) -> Result<StreamControl, String>,
    cache: Option<LifecycleCacheScope<'_>>,
) -> Result<StreamOutcome, StreamError> {
    options
        .validate()
        .map_err(|message| StreamError::Execution {
            kind: ExecutionErrorKind::InvalidQuery,
            message,
            progress: Default::default(),
        })?;
    if matches!(scope, ResultScope::All { .. })
        && options.mode != crate::query_execution::streaming::StreamMode::OrderedRows
    {
        return Err(StreamError::Execution {
            kind: ExecutionErrorKind::InvalidQuery,
            message: "all-results scope requires ordered rows".into(),
            progress: Default::default(),
        });
    }
    let cancellation = context.cancellation().cloned().unwrap_or_default();
    let mut delivery = Delivery::new(options, &cancellation, sink);
    if delivery.check().is_err() {
        return Ok(StreamOutcome::Cancelled(delivery.progress));
    }
    if let Err(error) = context.enter() {
        return segmented_stream_error(&mut delivery, context, error.message);
    }
    let mut request =
        query
            .with_clock(context.clock.now)
            .map_err(|message| StreamError::Execution {
                kind: ExecutionErrorKind::InvalidQuery,
                message,
                progress: delivery.progress,
            })?;
    let all = matches!(scope, ResultScope::All { .. });
    match scope {
        ResultScope::Page { offset, limit } => {
            request.options.offset = offset;
            request.options.limit = limit;
        }
        ResultScope::All { .. } => {
            let slots = base_active.iter().try_fold(0usize, |count, active| {
                count.checked_add(active.len() as usize)
            });
            let slots = segments
                .iter()
                .flat_map(|group| group.iter())
                .try_fold(
                    slots.ok_or_else(|| StreamError::Resource {
                        message: "all-results record count overflow".into(),
                        progress: delivery.progress,
                    })?,
                    |count, segment| count.checked_add(segment.active.len() as usize),
                )
                .ok_or_else(|| StreamError::Resource {
                    message: "all-results record count overflow".into(),
                    progress: delivery.progress,
                })?;
            request.options.offset = 0;
            request.options.limit = slots;
            request.options.count = query.options.count;
            request.options.return_mode = ReturnMode::Locator;
        }
    }
    crate::query::sort_binding::BoundSort::bind(repo, &request).map_err(|message| {
        StreamError::Execution {
            kind: ExecutionErrorKind::InvalidQuery,
            message,
            progress: delivery.progress,
        }
    })?;
    let prepared = crate::query::prepared::prepare(repo, &request).map_err(|message| {
        StreamError::Execution {
            kind: ExecutionErrorKind::InvalidQuery,
            message,
            progress: delivery.progress,
        }
    })?;
    if let ResultScope::Page { .. } = scope
        && let Some(cache) = cache
        && context.limits == ExecutionLimits::default()
    {
        let key =
            crate::query_results::verified::lifecycle_key(&request, cache.instance_id, revision.0)
                .map_err(|message| StreamError::Execution {
                    kind: ExecutionErrorKind::Execution,
                    message,
                    progress: delivery.progress,
                })?;
        let cached = cache
            .store
            .lock()
            .map_err(|error| StreamError::Execution {
                kind: ExecutionErrorKind::Execution,
                message: error.to_string(),
                progress: delivery.progress,
            })?
            .get(&key);
        if let Some(cached) = cached {
            let result = materialize_lifecycle_cached(
                repo,
                &request,
                &cached,
                base_active,
                segments,
                catalog,
                revision,
                Some(context),
            );
            let mut result = match result {
                Ok(result) => result,
                Err(message) => return segmented_stream_error(&mut delivery, context, message),
            };
            stamp_native_rows(repo, revision, &mut result.hits);
            let delivered = match options.mode {
                StreamMode::OrderedRows => result
                    .hits
                    .chunks(options.batch_rows)
                    .try_for_each(|rows| delivery.ordered_batch(rows, &result.counters)),
                StreamMode::PageReplacements => delivery
                    .check_buffer(
                        result
                            .hits
                            .iter()
                            .map(crate::query_execution::streaming::hit_bytes)
                            .sum(),
                    )
                    .and_then(|_| delivery.replacement(&result.hits, &result.counters, 0)),
            };
            if let Err(message) = delivered {
                return segmented_stream_error(&mut delivery, context, message);
            }
            if let Err(message) = delivery.check() {
                return segmented_stream_error(&mut delivery, context, message);
            }
            return Ok(StreamOutcome::Complete(Box::new(StreamSummary {
                count: result.count,
                returned_rows: result.hits.len(),
                final_sequence: delivery.last_page_sequence(),
                counters: result.counters,
                progress: delivery.progress,
            })));
        }
    }
    let relation_scope = prepared.collections.iter().any(|entry| {
        entry.filter.as_ref().is_some_and(bound_expr_has_relations)
            || entry
                .fts
                .as_ref()
                .is_some_and(|fts| fts.fields.iter().any(|field| field.related.is_some()))
    });
    match try_lifecycle_ordered_stream(
        repo,
        &request,
        query.options.return_mode.clone(),
        scope,
        base_active,
        segments,
        catalog,
        revision,
        context,
        cache,
        &mut delivery,
    ) {
        Ok(Some(outcome)) => return Ok(outcome),
        Ok(None) => {}
        Err(message) => return segmented_stream_error(&mut delivery, context, message),
    }
    let page_cache = if all { None } else { cache };
    let mut deferred_cache = None;
    let query_cache = if let Some(scope) = page_cache {
        let budget = scope
            .store
            .lock()
            .map_err(|error| StreamError::Execution {
                kind: ExecutionErrorKind::Execution,
                message: error.to_string(),
                progress: delivery.progress,
            })?
            .budget();
        deferred_cache = Some((
            scope,
            Mutex::new(crate::query_results::verified::VerifiedCache::new(budget)),
        ));
        deferred_cache
            .as_ref()
            .map(|(scope, store)| LifecycleCacheScope {
                store,
                instance_id: scope.instance_id,
            })
    } else {
        None
    };
    let mut result = match search_segmented_with_context_lifecycle_budget(
        repo,
        &request,
        base_active,
        segments,
        catalog,
        revision,
        context,
        query_cache,
        (all && !relation_scope).then(|| delivery.max_buffered_bytes() / 2),
    ) {
        Ok(result) => result,
        Err(error) => return segmented_stream_error(&mut delivery, context, error.message),
    };
    if !all {
        if let Err(message) =
            delivery.check_buffer(result.hits.iter().fold(0usize, |bytes, hit| {
                bytes.saturating_add(crate::query_execution::streaming::hit_bytes(hit))
            }))
        {
            return segmented_stream_error(&mut delivery, context, message);
        }
    }
    if all && result.counters.match_count_complete {
        result.count = usize::try_from(result.counters.total_matches).ok();
    }
    if all {
        let mut batch = Vec::new();
        let mut batch_bytes = 0usize;
        let mut event_counters = result.counters.clone();
        event_counters.total_matches = 0;
        event_counters.returned_records = 0;
        for hit in std::mem::take(&mut result.hits)
            .into_iter()
            .skip(scope.offset())
        {
            if let Err(message) = delivery.check() {
                return segmented_stream_error(&mut delivery, context, message);
            }
            let locator = SourceHitLocator::from(&hit);
            let mut hit = match materialize_segmented_stream_hit(
                repo,
                hit,
                query.options.return_mode.clone(),
                base_active,
                segments,
                catalog,
                revision,
                context,
            ) {
                Ok(hit) => hit,
                Err(message) => {
                    if !matches!(
                        context.recorded_error(message.clone()).kind,
                        ExecutionErrorKind::SourceChanged | ExecutionErrorKind::Execution
                    ) {
                        return segmented_stream_error(&mut delivery, context, message);
                    }
                    match record_unavailable_hit(
                        repo,
                        catalog,
                        locator,
                        message.clone(),
                        &mut result.counters,
                    ) {
                        Ok(true) => {
                            result.count = None;
                            result.counters.total_matches =
                                result.counters.total_matches.saturating_sub(1);
                            continue;
                        }
                        Ok(false) => {
                            return segmented_stream_error(&mut delivery, context, message);
                        }
                        Err(error) => return segmented_stream_error(&mut delivery, context, error),
                    }
                }
            };
            repo.stamp_native_hit_at(&mut hit, revision);
            if query.options.return_mode != ReturnMode::Locator {
                result.counters.files_opened += 1;
                result.counters.bytes_read += hit.byte_end.saturating_sub(hit.byte_start);
                result.counters.record_parses += u64::from(
                    query.options.return_mode == ReturnMode::Logical
                        && hit.record_class == RecordClass::Logical,
                );
            }
            let size = crate::query_execution::streaming::hit_bytes(&hit);
            if !batch.is_empty()
                && (batch.len() >= options.batch_rows
                    || batch_bytes.saturating_add(size) > options.max_event_bytes)
            {
                if let Err(message) = delivery.ordered_batch(&batch, &event_counters) {
                    return segmented_stream_error(&mut delivery, context, message);
                }
                batch.clear();
                batch_bytes = 0;
            }
            batch_bytes = batch_bytes.saturating_add(size);
            batch.push(hit);
            event_counters.total_matches += 1;
            if let Err(message) = delivery.check_buffer(batch_bytes) {
                return segmented_stream_error(&mut delivery, context, message);
            }
        }
        if let Err(message) = delivery.ordered_batch(&batch, &event_counters) {
            return segmented_stream_error(&mut delivery, context, message);
        }
        result.counters.returned_records = delivery.progress.emitted_rows;
    } else {
        stamp_native_rows(repo, revision, &mut result.hits);
        let delivered = match options.mode {
            crate::query_execution::streaming::StreamMode::OrderedRows => {
                delivery.ordered_rows(&result.hits, &result.counters, true)
            }
            crate::query_execution::streaming::StreamMode::PageReplacements => {
                delivery.replacement(&result.hits, &result.counters, 0)
            }
        };
        if let Err(message) = delivered {
            return segmented_stream_error(&mut delivery, context, message);
        }
    }
    if let Err(message) = delivery.check() {
        return segmented_stream_error(&mut delivery, context, message);
    }
    if delivery.progress.sequence == 0
        && let Err(message) = delivery.progress_event(&result.counters)
    {
        return segmented_stream_error(&mut delivery, context, message);
    }
    delivery.observe(&result.counters);
    if let Some((scope, store)) = deferred_cache {
        let key =
            crate::query_results::verified::lifecycle_key(&request, scope.instance_id, revision.0)
                .map_err(|message| StreamError::Execution {
                    kind: ExecutionErrorKind::Execution,
                    message,
                    progress: delivery.progress,
                })?;
        let cached = store
            .lock()
            .map_err(|error| StreamError::Execution {
                kind: ExecutionErrorKind::Execution,
                message: error.to_string(),
                progress: delivery.progress,
            })?
            .get(&key);
        if let Some(cached) = cached {
            let displaced = scope
                .store
                .lock()
                .map_err(|error| StreamError::Execution {
                    kind: ExecutionErrorKind::Execution,
                    message: error.to_string(),
                    progress: delivery.progress,
                })?
                .insert_shared(key, cached);
            drop(displaced);
        }
    }
    Ok(StreamOutcome::Complete(Box::new(StreamSummary {
        count: result.count,
        returned_rows: if all {
            delivery.progress.emitted_rows as usize
        } else {
            result.hits.len()
        },
        final_sequence: delivery.last_page_sequence(),
        counters: result.counters,
        progress: delivery.progress,
    })))
}

fn stamp_native_rows(repo: &Repository, revision: IndexRevision, rows: &mut [SearchHit]) {
    for row in rows {
        repo.stamp_native_hit_at(row, revision);
    }
}

fn collection_domain(
    collection: &CollectionState,
    collection_index: usize,
    active: Option<&[ActiveRecords]>,
) -> RoaringBitmap {
    active.map_or_else(
        || (0..collection.generation.records.len() as u32).collect(),
        |domains| domains[collection_index].bitmap(),
    )
}

pub(crate) fn search_active(
    repo: &Repository,
    query: &Query,
    active: &[ActiveRecords],
) -> Result<SearchResult, String> {
    search_active_inner(repo, query, active, None)
}

fn search_active_inner(
    repo: &Repository,
    query: &Query,
    active: &[ActiveRecords],
    context: Option<&ExecutionContext>,
) -> Result<SearchResult, String> {
    let scoped_context = context
        .cloned()
        .unwrap_or_else(|| {
            ExecutionContext::new(
                crate::query::clock::ClockContext::new(time::OffsetDateTime::now_utc()),
                ExecutionLimits::default(),
                None,
            )
        })
        .with_active(active);
    let mut count_query;
    let query = if query.options.count && query.options.limit == 0 {
        BoundSort::bind(repo, query)?;
        count_query = query.clone();
        count_query.options.sort.clear();
        count_query.options.return_mode = ReturnMode::Locator;
        count_query.options.evidence = false;
        &count_query
    } else {
        query
    };
    search_impl_with_collector(
        repo,
        query,
        None,
        None,
        None,
        None,
        None,
        None,
        (query.options.count && query.options.limit == 0)
            .then_some(ExactCollector { members: None }),
        Some(&scoped_context),
        Some(active),
    )
}

pub(crate) fn search_with_context_active(
    repo: &Repository,
    query: &Query,
    context: &ExecutionContext,
    active: &[ActiveRecords],
) -> Result<SearchResult, crate::query_execution::streaming::ExecutionError> {
    context.check()?;
    search_active_inner(repo, query, active, Some(context))
        .map_err(|message| context.recorded_error(message))
}

pub fn search(repo: &Repository, query: &Query) -> Result<SearchResult, String> {
    if query.options.count && query.options.limit == 0 {
        // Sort syntax still belongs to the request contract, although ordering
        // cannot affect a count. Validate it before omitting ranking work.
        BoundSort::bind(repo, query)?;
        let mut count_query = query.clone();
        count_query.options.sort.clear();
        count_query.options.return_mode = ReturnMode::Locator;
        count_query.options.evidence = false;
        return search_impl_with_collector(
            repo,
            &count_query,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(ExactCollector { members: None }),
            None,
            None,
        );
    }
    if posting_order_eligible(repo, query, None, &repo.segments, &repo.catalog) {
        return search_impl(repo, query, None, None, None, None, None, None);
    }
    let budget = {
        let cache = repo.verified.lock().map_err(|e| e.to_string())?;
        if cache.enabled() { cache.budget() } else { 0 }
    };
    if budget == 0 {
        return search_impl(repo, query, None, None, None, None, None, None);
    }
    let bound_sort = BoundSort::bind(repo, query)?;
    let computed_sort = bound_sort.has_computed;
    let mut membership_query = query.clone();
    if computed_sort {
        membership_query.options.sort.clear();
    }
    let key = crate::query_results::verified::key(&membership_query)?;
    let cached = repo.verified.lock().map_err(|e| e.to_string())?.get(&key);
    if let Some(cached) = cached {
        return if computed_sort {
            materialize_computed_cached(repo, query, &cached, bound_sort, None, None)
        } else {
            materialize_cached(repo, query, &cached)
        };
    }
    if !query.options.count {
        return search_impl(repo, query, None, None, None, Some(bound_sort), None, None);
    }
    let mut members = Vec::new();
    let tracker = CacheBudget::new(budget.saturating_sub(key.len() + 128));
    let result = search_impl(
        repo,
        query,
        None,
        Some(&mut members),
        Some(&tracker),
        Some(bound_sort),
        None,
        None,
    )?;
    if tracker.complete() && Some(members.len()) == result.count {
        sort_cached_members(repo, &membership_query, &mut members);
        let displaced = repo
            .verified
            .lock()
            .map_err(|e| e.to_string())?
            .insert(key, members);
        drop(displaced);
    }
    Ok(result)
}

/// Execute with caller-supplied clock, cancellation and request limits.
/// The ordinary `search` path keeps its existing allocation-free context fast path.
pub(crate) fn search_with_context(
    repo: &Repository,
    query: &Query,
    context: &ExecutionContext,
) -> Result<SearchResult, crate::query_execution::streaming::ExecutionError> {
    context.check()?;
    let result = if query.options.count && query.options.limit == 0 {
        BoundSort::bind(repo, query)
            .map_err(|message| context.error(ExecutionErrorKind::InvalidQuery, message))?;
        let mut count_query = query.clone();
        count_query.options.sort.clear();
        count_query.options.return_mode = ReturnMode::Locator;
        count_query.options.evidence = false;
        search_impl_with_collector(
            repo,
            &count_query,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(ExactCollector { members: None }),
            Some(context),
            None,
        )
    } else {
        search_impl_with_collector(
            repo,
            query,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(context),
            None,
        )
    };
    result.map_err(|message| context.recorded_error(message))
}

/// Execute and publish from the shared scan/ranking path before completion.
pub fn search_stream(
    repo: &Repository,
    query: &Query,
    options: StreamOptions,
    cancellation: &CancellationToken,
    sink: &mut dyn for<'e> FnMut(StreamEvent<'e>) -> Result<StreamControl, String>,
) -> Result<StreamOutcome, StreamError> {
    search_stream_scoped(
        repo,
        query,
        ResultScope::Page {
            offset: query.options.offset,
            limit: query.options.limit,
        },
        options,
        cancellation,
        sink,
    )
}

/// Stream a typed finite-page or all-results request through the shared plan.
pub fn search_stream_scoped(
    repo: &Repository,
    query: &Query,
    scope: ResultScope,
    options: StreamOptions,
    cancellation: &CancellationToken,
    sink: &mut dyn for<'e> FnMut(StreamEvent<'e>) -> Result<StreamControl, String>,
) -> Result<StreamOutcome, StreamError> {
    let context = ExecutionContext::new(
        crate::query::clock::ClockContext::new(time::OffsetDateTime::now_utc()),
        ExecutionLimits::default(),
        Some(cancellation.clone()),
    );
    search_stream_scoped_with_context(repo, query, scope, options, cancellation, &context, sink)
}

pub(crate) fn search_stream_scoped_with_context(
    repo: &Repository,
    query: &Query,
    scope: ResultScope,
    options: StreamOptions,
    cancellation: &CancellationToken,
    context: &ExecutionContext,
    sink: &mut dyn for<'e> FnMut(StreamEvent<'e>) -> Result<StreamControl, String>,
) -> Result<StreamOutcome, StreamError> {
    search_stream_scoped_with_context_active(
        repo,
        query,
        scope,
        options,
        cancellation,
        context,
        sink,
        None,
    )
}

pub(crate) fn search_stream_scoped_with_context_active(
    repo: &Repository,
    query: &Query,
    scope: ResultScope,
    options: StreamOptions,
    cancellation: &CancellationToken,
    context: &ExecutionContext,
    sink: &mut dyn for<'e> FnMut(StreamEvent<'e>) -> Result<StreamControl, String>,
    active: Option<&[ActiveRecords]>,
) -> Result<StreamOutcome, StreamError> {
    options
        .validate()
        .map_err(|message| StreamError::Execution {
            kind: ExecutionErrorKind::InvalidQuery,
            message,
            progress: Default::default(),
        })?;
    if matches!(scope, ResultScope::All { .. }) && options.mode != StreamMode::OrderedRows {
        return Err(StreamError::Execution {
            kind: ExecutionErrorKind::InvalidQuery,
            message: "all-results scope requires ordered rows".into(),
            progress: Default::default(),
        });
    }
    let mut delivery = Delivery::new(options, cancellation, sink);
    if delivery.check().is_err() {
        return Ok(StreamOutcome::Cancelled(delivery.progress));
    }
    let mut execution_query = query.clone();
    if let ResultScope::All { .. } = scope {
        execution_query.options.offset = 0;
        execution_query.options.limit = repo
            .collections
            .iter()
            .enumerate()
            .try_fold(0usize, |total, (index, collection)| {
                let records = active.map_or(collection.generation.records.len(), |domains| {
                    domains[index].len() as usize
                });
                total.checked_add(records)
            })
            .ok_or_else(|| StreamError::Resource {
                message: "all-results record count overflow".into(),
                progress: delivery.progress,
            })?;
        execution_query.options.count = true;
    }
    let execution_query = &execution_query;
    let inferred_order =
        posting_order_eligible(repo, execution_query, None, &repo.segments, &repo.catalog);
    let bound_sort =
        BoundSort::bind(repo, execution_query).map_err(|message| StreamError::Execution {
            kind: crate::query_execution::streaming::ExecutionErrorKind::InvalidQuery,
            message,
            progress: delivery.progress,
        })?;
    let computed_sort = bound_sort.has_computed;
    let mut membership_query = execution_query.clone();
    if computed_sort {
        membership_query.options.sort.clear();
    }
    let budget = repo
        .verified
        .lock()
        .map_err(|error| StreamError::Execution {
            kind: crate::query_execution::streaming::ExecutionErrorKind::Execution,
            message: error.to_string(),
            progress: delivery.progress,
        })?
        .budget();
    let mut members = Vec::new();
    // The ordered cursor can publish its page before COUNT traverses the rest
    // of the domain. Building a membership cache would force the top-k scan.
    let cache_key = if active.is_none()
        && context.limits == ExecutionLimits::default()
        && budget > 0
        && !inferred_order
        && execution_query.options.count
        && matches!(scope, ResultScope::Page { .. })
    {
        Some(
            crate::query_results::verified::key(&membership_query).map_err(|message| {
                StreamError::Execution {
                    kind: crate::query_execution::streaming::ExecutionErrorKind::Execution,
                    message,
                    progress: delivery.progress,
                }
            })?,
        )
    } else {
        None
    };
    let cached = if let Some(key) = &cache_key {
        repo.verified
            .lock()
            .map_err(|error| StreamError::Execution {
                kind: crate::query_execution::streaming::ExecutionErrorKind::Execution,
                message: error.to_string(),
                progress: delivery.progress,
            })?
            .get(key)
    } else {
        None
    };
    let tracker = cache_key
        .as_ref()
        .filter(|_| cached.is_none() && options.mode != StreamMode::OrderedRows)
        .map(|key| {
            CacheBudget::new(
                budget
                    .saturating_sub(key.len() + 128)
                    .min(options.max_buffered_bytes / 4),
            )
        });
    let result = if let Some(cached) = cached {
        if computed_sort {
            materialize_computed_cached(
                repo,
                execution_query,
                &cached,
                bound_sort,
                Some(&mut delivery),
                Some(context),
            )
        } else {
            materialize_cached_stream(repo, execution_query, &cached, &mut delivery, Some(context))
        }
    } else {
        search_impl_with_collector(
            repo,
            execution_query,
            None,
            tracker.as_ref().map(|_| &mut members),
            tracker.as_ref(),
            Some(bound_sort),
            Some(&mut delivery),
            match scope {
                ResultScope::Page { .. } => None,
                ResultScope::All { offset } => Some(offset),
            },
            None,
            Some(context),
            active,
        )
    };
    let result = result.and_then(|mut result| {
        if let ResultScope::All { offset } = scope {
            deliver_ranked_all(repo, query, offset, &mut result, &mut delivery, context)?;
        }
        Ok(result)
    });
    let result = match result {
        Ok(result) => result,
        Err(message) => {
            return match delivery.abort.take() {
                Some(Abort::Cancelled) => Ok(StreamOutcome::Cancelled(delivery.progress)),
                Some(Abort::Sink(message)) => Err(StreamError::Sink {
                    message,
                    progress: delivery.progress,
                }),
                Some(Abort::Resource(message)) => Err(StreamError::Resource {
                    message,
                    progress: delivery.progress,
                }),
                None if cancellation.is_cancelled() => {
                    Ok(StreamOutcome::Cancelled(delivery.progress))
                }
                None => {
                    let error = context.recorded_error(message);
                    match error.kind {
                        ExecutionErrorKind::Cancelled => {
                            Ok(StreamOutcome::Cancelled(delivery.progress))
                        }
                        ExecutionErrorKind::Resource => Err(StreamError::Resource {
                            message: error.message,
                            progress: delivery.progress,
                        }),
                        kind => Err(StreamError::Execution {
                            kind,
                            message: error.message,
                            progress: delivery.progress,
                        }),
                    }
                }
            };
        }
    };
    let final_delivery = delivery
        .check_buffer(
            result
                .hits
                .iter()
                .map(crate::query_execution::streaming::hit_bytes)
                .sum(),
        )
        .and_then(|_| match options.mode {
            StreamMode::OrderedRows => delivery.ordered_rows(&result.hits, &result.counters, true),
            StreamMode::PageReplacements => delivery.replacement(&result.hits, &result.counters, 0),
        });
    if let Err(message) = final_delivery.and_then(|_| delivery.check()) {
        return match delivery.abort.take() {
            Some(Abort::Cancelled) => Ok(StreamOutcome::Cancelled(delivery.progress)),
            Some(Abort::Sink(message)) => Err(StreamError::Sink {
                message,
                progress: delivery.progress,
            }),
            Some(Abort::Resource(message)) => Err(StreamError::Resource {
                message,
                progress: delivery.progress,
            }),
            None => Err(StreamError::Execution {
                kind: crate::query_execution::streaming::ExecutionErrorKind::Execution,
                message,
                progress: delivery.progress,
            }),
        };
    }
    delivery.observe(&result.counters);
    let publish_cache =
        tracker.as_ref().is_some_and(CacheBudget::complete) && Some(members.len()) == result.count;
    if publish_cache {
        sort_cached_members(repo, &membership_query, &mut members);
    }
    // This check is the completion decision. Cancellation observed afterwards
    // cannot undo a successfully delivered page or a committed cache entry.
    if delivery.check().is_err() {
        return Ok(StreamOutcome::Cancelled(delivery.progress));
    }
    if publish_cache {
        if let Some(key) = cache_key {
            let displaced = repo
                .verified
                .lock()
                .map_err(|error| StreamError::Execution {
                    kind: crate::query_execution::streaming::ExecutionErrorKind::Execution,
                    message: error.to_string(),
                    progress: delivery.progress,
                })?
                .insert(key, members);
            drop(displaced);
        }
    }
    Ok(StreamOutcome::Complete(Box::new(StreamSummary {
        count: result.count,
        returned_rows: if matches!(scope, ResultScope::All { .. }) {
            delivery.progress.emitted_rows as usize
        } else {
            result.hits.len()
        },
        final_sequence: delivery.last_page_sequence(),
        counters: result.counters,
        progress: delivery.progress,
    })))
}

fn deliver_ranked_all(
    repo: &Repository,
    query: &Query,
    offset: usize,
    result: &mut SearchResult,
    delivery: &mut Delivery<'_>,
    execution: &ExecutionContext,
) -> Result<(), String> {
    if result.hits.is_empty() {
        return Ok(());
    }
    let mut source_access = SourceAccessSession::new(&repo.files);
    source_access.configure_execution(execution);
    // `batch_rows` is caller-controlled; byte limits govern allocation growth.
    let mut batch = Vec::new();
    for mut hit in std::mem::take(&mut result.hits).into_iter().skip(offset) {
        delivery.check()?;
        let source_bytes =
            usize::try_from(hit.byte_end.saturating_sub(hit.byte_start)).unwrap_or(usize::MAX);
        let current = batch.iter().fold(0usize, |total, prior| {
            total.saturating_add(crate::query_execution::streaming::hit_bytes(prior))
        });
        if !batch.is_empty()
            && (batch.len() >= delivery.options().batch_rows
                || current.saturating_add(source_bytes) >= delivery.options().max_event_bytes
                || current.saturating_add(source_bytes) >= delivery.max_buffered_bytes())
        {
            delivery.ordered_batch(&batch, &result.counters)?;
            batch.clear();
        }
        delivery.check_record_bytes(source_bytes)?;
        if query.options.return_mode != ReturnMode::Locator {
            if query.options.return_mode == ReturnMode::Logical
                && hit.record_class == RecordClass::RawFallback
            {
                hit.payload = Some(Payload::LogicalUnavailable {
                    diagnostic_ref: hit.diagnostic_ref,
                });
            } else {
                let range = ByteRange {
                    start: hit.byte_start,
                    end: hit.byte_end,
                };
                let mut read = source_access.read_bounded(
                    crate::model::FileId(hit.file_id),
                    &[(0, range)],
                    delivery.max_buffered_bytes(),
                )?;
                result.counters.files_opened += read.opened;
                result.counters.bytes_read += read.bytes_read;
                let bytes = read.records.pop().ok_or("source record missing")?.1;
                let collection = if query.options.return_mode == ReturnMode::Logical {
                    repo.collections
                        .iter()
                        .find(|collection| collection.definition.name == hit.collection)
                } else {
                    None
                };
                let (payload, parses) = payload_from_bytes_with_execution(
                    hit.record_class,
                    collection,
                    &query.options.return_mode,
                    bytes,
                    Some(execution),
                )?;
                result.counters.record_parses += parses;
                hit.payload = payload;
            }
        }
        batch.push(hit);
        delivery.check_buffer(batch.iter().fold(0usize, |total, row| {
            total.saturating_add(crate::query_execution::streaming::hit_bytes(row))
        }))?;
        flush_all_batch_if_full(delivery, &mut batch, &result.counters)?;
    }
    delivery.ordered_batch(&batch, &result.counters)?;
    result.counters.returned_records = delivery.progress.emitted_rows;
    result.counters.distinct_files = result
        .counters
        .distinct_files
        .max(source_access.distinct_files());
    Ok(())
}

pub(crate) fn exact_members_with_active(
    repo: &Repository,
    query: &Query,
    collection_index: usize,
    active: Option<&[ActiveRecords]>,
) -> Result<RoaringBitmap, String> {
    BoundSort::bind(repo, query)?;
    let mut scoped = query.clone();
    scoped.collections = vec![repo.collections[collection_index].definition.name.clone()];
    scoped.options.count = true;
    scoped.options.limit = 0;
    scoped.options.offset = 0;
    scoped.options.sort.clear();
    scoped.options.return_mode = ReturnMode::Locator;
    scoped.options.evidence = false;
    let mut members = RoaringBitmap::new();
    let active_context = active.map(|active| {
        ExecutionContext::new(
            crate::query::clock::ClockContext::new(time::OffsetDateTime::now_utc()),
            ExecutionLimits::default(),
            None,
        )
        .with_active(active)
    });
    let result = search_impl_with_collector(
        repo,
        &scoped,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(ExactCollector {
            members: Some(&mut members),
        }),
        active_context.as_ref(),
        active,
    )?;
    if result.count != Some(members.len() as usize) {
        return Err("exact selection is incomplete".into());
    }
    Ok(members)
}

fn check_cached_stream_sort_budget(
    delivery: &mut Delivery<'_>,
    sort: &ResolvedSort<'_>,
    fixed_bytes: u64,
    retained_key_bytes: u64,
) -> Result<usize, String> {
    delivery.check()?;
    let resident = fixed_bytes
        .saturating_add(retained_key_bytes)
        .saturating_add(sort_target_retained_bytes(sort)? as u64);
    let resident = usize::try_from(resident).unwrap_or(usize::MAX);
    delivery.check_buffer(resident)?;
    Ok(delivery.max_buffered_bytes().saturating_sub(resident))
}

/// Re-rank complete cached membership with bounded keys; no computed page is cached.
fn materialize_computed_cached<'a>(
    repo: &'a Repository,
    query: &Query,
    cached: &CachedResult,
    bound_sort: BoundSort<'a>,
    mut delivery: Option<&mut Delivery<'_>>,
    execution: Option<&ExecutionContext>,
) -> Result<SearchResult, String> {
    record_base_cache_hits(repo, cached);
    #[cfg(feature = "detailed-profiling")]
    let preparation_start = DetailedInstant::now();
    let sort = ResolvedSort::from_bound(repo, query, bound_sort);
    if let Some(execution) = execution {
        sort.target_cache
            .lock()
            .map_err(|error| error.to_string())?
            .configure_execution(execution);
    }
    if let Some(delivery) = delivery.as_deref_mut() {
        sort.target_cache
            .lock()
            .map_err(|error| error.to_string())?
            .configure_stream(delivery.max_buffered_bytes(), delivery.token());
    }
    let has_related_sort = sort.bound.terms.iter().flatten().any(|terms| {
        terms.iter().any(|term| {
            matches!(&term.source,
            BoundSortSource::Computed(computed) if computed.related.is_some())
        })
    });
    let cap = query
        .options
        .limit
        .checked_add(query.options.offset)
        .ok_or("page size overflow")?;
    let mut counters = WorkCounters {
        total_matches: cached.members.len() as u64,
        match_count_complete: true,
        verified_result_hit: true,
        ..WorkCounters::default()
    };
    #[cfg(feature = "detailed-profiling")]
    {
        counters.preparation_ns = preparation_start.elapsed().as_nanos() as u64;
    }
    let mut winners = BinaryHeap::new();
    let mut retained_key_bytes = 0u64;
    let reads_needed = cached
        .members
        .iter()
        .filter(|member| {
            sort.bound.needs_source
                && repo.collections[member.collection_index].generation.records
                    [member.slot as usize]
                    .class
                    == RecordClass::Logical
        })
        .count();
    let fixed_bytes = cached_sort_fixed_bytes(cached, reads_needed)?;
    check_computed_sort_budget(fixed_bytes, 0, 0)?;
    let mut reads = Vec::<CachedSortRead>::with_capacity(reads_needed);
    let mut touched_files = HashSet::new();
    let empty = Value::Object(Default::default());
    for (index, member) in cached.members.iter().enumerate() {
        let collection = &repo.collections[member.collection_index];
        let record = &collection.generation.records[member.slot as usize];
        if sort.bound.needs_source && record.class == RecordClass::Logical {
            reads.push(CachedSortRead {
                collection_index: member.collection_index,
                file_id: record.file.0,
                member_index: u32::try_from(index).map_err(|_| "cached membership exceeds u32")?,
                range: record.range,
            });
            continue;
        }
        let ctx = RecordContext {
            value: &empty,
            source_bytes: None,
            record,
            file: &repo.files.files[record.file.0 as usize],
            collection_name: &collection.definition.name,
            repository_root: &repo.files.root,
        };
        let key = rank_key_from_context(&sort, repo, member.collection_index, &ctx, None)?;
        counters.sort_key_evaluations += 1;
        offer_cached_key(
            repo,
            cached,
            index,
            key,
            &mut winners,
            &mut retained_key_bytes,
            &mut counters.peak_retained_sort_key_bytes,
            cap,
        )?;
        check_cached_sort_live(&sort, fixed_bytes, retained_key_bytes, has_related_sort)?;
        if let Some(delivery) = delivery.as_deref_mut() {
            check_cached_stream_sort_budget(delivery, &sort, fixed_bytes, retained_key_bytes)?;
        }
    }
    reads.sort_unstable_by_key(|read| (read.collection_index, read.file_id, read.range.start));
    let mut source_access = SourceAccessSession::new(&repo.files);
    if let Some(execution) = execution {
        source_access.configure_execution(execution);
    }
    let mut batch = Vec::<(u32, ByteRange)>::with_capacity(128);
    let mut start = 0;
    while start < reads.len() {
        if let Some(delivery) = delivery.as_deref_mut() {
            check_cached_stream_sort_budget(delivery, &sort, fixed_bytes, retained_key_bytes)?;
        }
        let collection_index = reads[start].collection_index;
        let file_id = reads[start].file_id;
        touched_files.insert(file_id);
        let collection = &repo.collections[collection_index];
        while start < reads.len()
            && reads[start].collection_index == collection_index
            && reads[start].file_id == file_id
        {
            batch.clear();
            while start < reads.len()
                && batch.len() < 128
                && reads[start].collection_index == collection_index
                && reads[start].file_id == file_id
            {
                batch.push((reads[start].member_index, reads[start].range));
                start += 1;
            }
            let read = if let Some(delivery) = delivery.as_deref_mut() {
                let available = check_cached_stream_sort_budget(
                    delivery,
                    &sort,
                    fixed_bytes,
                    retained_key_bytes,
                )? / 4;
                if available == 0 {
                    return Err("stream resource: no room for computed sort source read".into());
                }
                source_access.read_bounded(crate::model::FileId(file_id), &batch, available)?
            } else {
                source_access.read(crate::model::FileId(file_id), &batch)?
            };
            counters.files_opened += read.opened;
            counters.bytes_read += read.bytes_read;
            for (index, bytes) in read.records {
                let member = &cached.members[index as usize];
                let record = &collection.generation.records[member.slot as usize];
                let extracted = prepare_query_record_with_execution(
                    record.class,
                    &collection.source,
                    &collection.definition.schema,
                    &bytes,
                    &ScanProjection::full(false),
                    false,
                    execution,
                )?;
                counters.record_parses += 1;
                counters.sort_key_evaluations += 1;
                let ctx = RecordContext {
                    value: &extracted.value,
                    source_bytes: Some(&bytes),
                    record,
                    file: &repo.files.files[file_id as usize],
                    collection_name: &collection.definition.name,
                    repository_root: &repo.files.root,
                };
                let key = rank_key_from_context(&sort, repo, collection_index, &ctx, None)?;
                offer_cached_key(
                    repo,
                    cached,
                    index as usize,
                    key,
                    &mut winners,
                    &mut retained_key_bytes,
                    &mut counters.peak_retained_sort_key_bytes,
                    cap,
                )?;
                check_cached_sort_live(&sort, fixed_bytes, retained_key_bytes, has_related_sort)?;
                if let Some(delivery) = delivery.as_deref_mut() {
                    check_cached_stream_sort_budget(
                        delivery,
                        &sort,
                        fixed_bytes,
                        retained_key_bytes,
                    )?;
                }
            }
        }
    }
    let targets = sort
        .target_cache
        .lock()
        .map_err(|error| error.to_string())?;
    counters.sort_target_reads += targets.reads;
    counters.sort_target_reuse += targets.reuse;
    counters.relation_edge_visits += targets.edge_visits;
    counters.files_opened += targets.reads;
    counters.record_parses += targets.reads;
    counters.bytes_read += targets.bytes_read;
    let target_bytes = targets.retained_bytes();
    drop(targets);
    if let Some(delivery) = delivery.as_deref_mut() {
        check_cached_stream_sort_budget(delivery, &sort, fixed_bytes, retained_key_bytes)?;
    }
    let page_members = winners
        .into_sorted_vec()
        .into_iter()
        .skip(query.options.offset)
        .take(query.options.limit)
        .map(|winner| CachedMember {
            collection_index: winner.collection_index,
            segment_index: None,
            slot: winner.slot,
            matched_fields: winner.hit.matched_fields.into(),
        })
        .collect();
    let page_cache = CachedResult {
        members: page_members,
    };
    let payload_bytes = page_cache
        .members
        .iter()
        .try_fold(0u64, |total, member| {
            let record =
                &repo.collections[member.collection_index].generation.records[member.slot as usize];
            let estimate = if query.options.return_mode == ReturnMode::Logical {
                record.range.len().checked_mul(4)?
            } else if query.options.return_mode == ReturnMode::Bytes {
                record.range.len()
            } else {
                0
            };
            total.checked_add(estimate)
        })
        .ok_or("computed sort payload count overflow")?;
    check_computed_sort_budget(
        payload_bytes
            .checked_add(fixed_bytes)
            .ok_or("computed sort memory count overflow")?,
        retained_key_bytes,
        target_bytes,
    )?;
    let mut page_query = query.clone();
    page_query.options.offset = 0;
    page_query.options.limit = page_cache.members.len();
    let mut result = if let Some(delivery) = delivery.as_deref_mut() {
        materialize_cached_stream(repo, &page_query, &page_cache, delivery, execution)?
    } else {
        materialize_cached(repo, &page_query, &page_cache)?
    };
    for member in &page_cache.members {
        let record =
            &repo.collections[member.collection_index].generation.records[member.slot as usize];
        if query.options.return_mode != ReturnMode::Locator
            && !(query.options.return_mode == ReturnMode::Logical
                && record.class == RecordClass::RawFallback)
        {
            touched_files.insert(record.file.0);
        }
    }
    counters.files_opened += result.counters.files_opened;
    counters.bytes_read += result.counters.bytes_read;
    counters.record_parses += result.counters.record_parses;
    counters.distinct_files = touched_files.len() as u64;
    counters.returned_records = result.hits.len() as u64;
    result.count = query.options.count.then_some(cached.members.len());
    result.counters = counters;
    Ok(result)
}

pub fn search_analyze(
    repo: &Repository,
    query: &Query,
) -> Result<(SearchResult, Vec<QueryPlanTrace>), String> {
    search_analyze_with_active(repo, query, None)
}

pub(crate) fn search_analyze_with_active(
    repo: &Repository,
    query: &Query,
    active: Option<&[ActiveRecords]>,
) -> Result<(SearchResult, Vec<QueryPlanTrace>), String> {
    if active.is_none() {
        let bound_sort = BoundSort::bind(repo, query)?;
        let computed_sort = bound_sort.has_computed;
        let mut membership_query = query.clone();
        if computed_sort {
            membership_query.options.sort.clear();
        }
        let key = crate::query_results::verified::key(&membership_query)?;
        let cached = repo
            .verified
            .lock()
            .map_err(|error| error.to_string())?
            .get(&key);
        if let Some(cached) = cached {
            let mut traces = explain_with_active(repo, query, None)?;
            crate::query_execution::request::begin_observation_capture(repo.instance_id);
            let result = if computed_sort {
                materialize_computed_cached(repo, query, &cached, bound_sort, None, None)
            } else {
                materialize_cached(repo, query, &cached)
            };
            let result = match result {
                Ok(result) => result,
                Err(error) => {
                    crate::query_execution::request::cancel_observation_capture(repo.instance_id);
                    return Err(error);
                }
            };
            let observations =
                crate::query_execution::request::finish_observation_capture(repo.instance_id);
            attach_execution_observations(repo, &mut traces, observations);
            return Ok((result, traces));
        }
    }
    let mut traces = Vec::new();
    crate::query_execution::request::begin_observation_capture(repo.instance_id);
    let active_context = active.map(|active| {
        ExecutionContext::new(
            crate::query::clock::ClockContext::new(time::OffsetDateTime::now_utc()),
            ExecutionLimits::default(),
            None,
        )
        .with_active(active)
    });
    let result = match search_impl_with_collector(
        repo,
        query,
        Some(&mut traces),
        None,
        None,
        None,
        None,
        None,
        None,
        active_context.as_ref(),
        active,
    ) {
        Ok(result) => result,
        Err(error) => {
            crate::query_execution::request::cancel_observation_capture(repo.instance_id);
            return Err(error);
        }
    };
    let observations =
        crate::query_execution::request::finish_observation_capture(repo.instance_id);
    attach_execution_observations(repo, &mut traces, observations);
    Ok((result, traces))
}

pub fn explain(repo: &Repository, query: &Query) -> Result<Vec<QueryPlanTrace>, String> {
    explain_with_active(repo, query, None)
}

pub(crate) fn explain_with_active(
    repo: &Repository,
    query: &Query,
    active: Option<&[ActiveRecords]>,
) -> Result<Vec<QueryPlanTrace>, String> {
    let bound_sort = BoundSort::bind(repo, query)?;
    let prepared = crate::query::prepared::prepare(repo, query)?;
    let has_source_dependent = prepared.collections.iter().any(|entry| {
        entry
            .filter
            .as_ref()
            .is_some_and(crate::query_execution::relations::target_cache::has_source_dependent)
    });
    let has_related_fts = prepared.collections.iter().any(|entry| {
        entry
            .fts
            .as_ref()
            .is_some_and(|fts| fts.fields.iter().any(|field| field.related.is_some()))
    });
    let inferred_order = active.is_none()
        && !has_source_dependent
        && !has_related_fts
        && prepared.collections.len() == 1
        && posting_order_eligible(repo, query, Some(&prepared), &repo.segments, &repo.catalog)
        && inferred_order_index(repo, query, &repo.segments)
            .is_some_and(|(index, _, _)| prepared.collections[0].collection_index == index);
    let mut traces = Vec::new();
    let mut ordered_candidate_total = 0u64;
    for entry in prepared.collections {
        let collection_index = entry.collection_index;
        let filter = entry.filter;
        let fts = entry.fts;
        let collection = &repo.collections[collection_index];
        let domain_len = active.map_or(collection.generation.records.len() as u64, |domains| {
            domains[collection_index].len()
        });
        let estimated = filter
            .as_ref()
            .map(|expr| estimate_plan_expr(collection, expr, domain_len))
            .transpose()?;
        let fts_estimate = fts
            .as_ref()
            .and_then(|fts| estimate_fts_candidates(collection, fts));
        let candidates = estimated
            .as_ref()
            .map_or(domain_len, |plan| plan.candidates)
            .min(fts_estimate.map_or(domain_len, |estimate| estimate.0))
            .min(domain_len);
        let related_fts = fts
            .as_ref()
            .is_some_and(|fts| fts.fields.iter().any(|field| field.related.is_some()));
        let filter_exact = estimated.as_ref().is_none_or(|plan| plan.exact)
            && (filter.is_none()
                || collection.logical_records.len() == collection.generation.records.len() as u64);
        ordered_candidate_total = ordered_candidate_total.saturating_add(candidates);
        traces.push(QueryPlanTrace {
            collection: collection.definition.name.clone(),
            generation: collection.generation.generation,
            bigram_alphabet: collection.raw.alphabet,
            folded_representation: crate::index::bigram::FOLDED_REPRESENTATION,
            fts_case_sensitivity: fts.as_ref().map(|fts| fts.case_sensitivity),
            fts_mode: fts.as_ref().map(|fts| fts.mode),
            fts_exact_literal: fts.as_ref().map(|fts| fts.source_literal.clone()),
            fts_prefilter_reason: regex_prefilter_reason(collection, fts.as_ref()),
            fts_covered_target: fts_covered_target(fts.as_ref()),
            fts_estimated_candidates: fts_estimate.map(|estimate| estimate.0.min(domain_len)),
            remaining_verification: fts.is_some() || !filter_exact,
            input_records: domain_len,
            fts_strategy: if fts.as_ref().is_some_and(|fts| fts.fields.is_empty())
                && collection.logical_records.len() != collection.generation.records.len() as u64
            {
                "logical text plus fallback source text"
            } else if related_fts {
                if fts
                    .as_ref()
                    .is_some_and(|fts| fts.fields.iter().all(|field| field.related.is_some()))
                {
                    "relation semijoin strategy undecided (estimate only)"
                } else {
                    "mixed local and relation FTS (estimate only)"
                }
            } else if let Some(fts) = &fts {
                if !fts.fields.is_empty()
                    && fts
                        .fields
                        .iter()
                        .all(|field| matches!(field.path.root, PathRoot::Metadata(_)))
                {
                    "metadata inventory"
                } else if fts_estimate.is_some() {
                    "compatible bigram candidate or restricted scanner"
                } else {
                    "restricted scanner"
                }
            } else {
                "absent"
            }
            .into(),
            normalized_sort: query.options.sort.clone(),
            sort_strategy: String::new(),
            sort_strategy_estimated: true,
            filter_exact,
            terms: estimated
                .as_ref()
                .map_or_else(Vec::new, |plan| plan.choices.clone()),
            planning: estimated.map_or_else(PlanningCounters::default, |plan| plan.planning),
            bound_terms: bound_term_diagnostics(repo, filter.as_ref()),
            candidates,
            candidates_estimated: true,
            actual: None,
            observations: Vec::new(),
        });
    }
    let cap = query
        .options
        .limit
        .checked_add(query.options.offset)
        .ok_or("page size overflow")?;
    let stored_order = !bound_sort.has_computed
        && !query.options.count
        && (query.options.sort.len() <= 1
            || repo
                .collections
                .iter()
                .filter(|collection| {
                    query.collections.is_empty()
                        || query.collections.contains(&collection.definition.name)
                })
                .all(|collection| collection.sorting.composite(&query.options.sort).is_some()))
        && !has_source_dependent
        && !has_related_fts
        && !(cap > 0
            && ordered_candidate_total > 0
            && ordered_candidate_total < (cap as u64).saturating_mul(4));
    for trace in &mut traces {
        trace.sort_strategy = if inferred_order {
            "inferred_posting_order_page"
        } else if stored_order {
            if query.options.sort.len() > 1 {
                "stored_composite_order_page"
            } else {
                "stored_order_page"
            }
        } else if bound_sort.has_computed {
            "computed_top_k_full_key_evaluation"
        } else {
            "bounded_top_k"
        }
        .into();
    }
    Ok(traces)
}

fn sort_target_retained_bytes(sort: &ResolvedSort<'_>) -> Result<usize, String> {
    Ok(usize::try_from(
        sort.target_cache
            .lock()
            .map_err(|error| error.to_string())?
            .retained_bytes(),
    )
    .unwrap_or(usize::MAX))
}

fn scanned_file_bytes(outcome: &ScannedFile) -> usize {
    let matches = outcome
        .matches
        .iter()
        .map(|matched| matched.buffered_bytes)
        .sum::<usize>();
    let cache = outcome
        .cache_members
        .iter()
        .map(|member| {
            std::mem::size_of::<CachedMember>()
                + member
                    .matched_fields
                    .iter()
                    .map(|field| field.len() + std::mem::size_of::<String>())
                    .sum::<usize>()
        })
        .sum::<usize>();
    std::mem::size_of::<ScannedFile>()
        .saturating_add(matches)
        .saturating_add(cache)
}

fn search_impl<'a>(
    repo: &'a Repository,
    query: &Query,
    traces: Option<&mut Vec<QueryPlanTrace>>,
    cache_members: Option<&mut Vec<CachedMember>>,
    cache_budget: Option<&CacheBudget>,
    prebound_sort: Option<BoundSort<'a>>,
    delivery: Option<&mut Delivery<'_>>,
    all_offset: Option<usize>,
) -> Result<SearchResult, String> {
    search_impl_with_collector(
        repo,
        query,
        traces,
        cache_members,
        cache_budget,
        prebound_sort,
        delivery,
        all_offset,
        None,
        None,
        None,
    )
}

fn search_impl_with_collector<'a>(
    repo: &'a Repository,
    query: &Query,
    mut traces: Option<&mut Vec<QueryPlanTrace>>,
    mut cache_members: Option<&mut Vec<CachedMember>>,
    cache_budget: Option<&CacheBudget>,
    prebound_sort: Option<BoundSort<'a>>,
    mut delivery: Option<&mut Delivery<'_>>,
    all_offset: Option<usize>,
    mut exact_collector: Option<ExactCollector<'_>>,
    execution: Option<&ExecutionContext>,
    active: Option<&[ActiveRecords]>,
) -> Result<SearchResult, String> {
    if let Some(execution) = execution {
        execution.check_string()?;
    }
    if let Some(delivery) = delivery.as_deref_mut() {
        delivery.check()?;
    }
    let preparation_start = DetailedInstant::now();
    let sort = match prebound_sort {
        Some(bound) => ResolvedSort::from_bound(repo, query, bound),
        None => ResolvedSort::new(repo, query).map_err(|message| {
            if let Some(context) = execution {
                context.fail(ExecutionErrorKind::InvalidQuery, message)
            } else {
                message
            }
        })?,
    };
    if let Some(execution) = execution {
        sort.target_cache
            .lock()
            .map_err(|error| error.to_string())?
            .configure_execution(execution);
    }
    if let Some(delivery) = delivery.as_deref_mut() {
        sort.target_cache
            .lock()
            .map_err(|error| error.to_string())?
            .configure_stream(delivery.max_buffered_bytes(), delivery.token());
    }
    let prepared = crate::query::prepared::prepare(repo, query)?;
    let mut rejected_order = RejectedOrderPlans {
        entries: Vec::new(),
        planning_ns: 0,
    };
    #[cfg(test)]
    let test_provider_order =
        !crate::query_execution::request::test_access_providers(repo.instance_id).is_empty();
    #[cfg(not(test))]
    let test_provider_order = false;
    if !test_provider_order && cache_members.is_none() && exact_collector.is_none() {
        if let Some(result) = try_inferred_order_page(
            repo,
            query,
            &prepared,
            preparation_start.elapsed().as_nanos() as u64,
            traces.as_deref_mut(),
            delivery.as_deref_mut(),
            all_offset,
            execution,
            active,
        )? {
            return Ok(result);
        }
        if let Some(result) = try_stored_order_page(
            repo,
            query,
            &prepared,
            &sort,
            preparation_start.elapsed().as_nanos() as u64,
            traces.as_deref_mut(),
            delivery.as_deref_mut(),
            all_offset,
            false,
            Some(&mut rejected_order),
            execution,
            active,
        )? {
            return Ok(result);
        }
    }
    let prepared_collection_count = prepared.collections.len();
    let crate::query::prepared::PreparedQuery {
        collections,
        predicates,
    } = prepared;
    let cap = query
        .options
        .limit
        .checked_add(query.options.offset)
        .ok_or("page size overflow")?;
    let mut winners = BinaryHeap::new();
    let mut locator_scan_query = query.clone();
    locator_scan_query.options.return_mode = ReturnMode::Locator;
    let scan_query = if all_offset.is_some() {
        &locator_scan_query
    } else {
        query
    };
    let mut retained_key_bytes = 0u64;
    let mut counters = WorkCounters::default();
    // One collection with locator output scans each grouped physical file at
    // most once. Other routes may revisit files for page payloads or share a
    // physical file across collections, so they need a compact union.
    let mut files_read = (prepared_collection_count > 1
        || query.options.return_mode != ReturnMode::Locator)
        .then(RoaringBitmap::new);
    let mut distinct_files = 0u64;
    #[cfg(feature = "detailed-profiling")]
    {
        counters.preparation_ns = preparation_start.elapsed().as_nanos() as u64;
    }
    #[cfg(feature = "detailed-profiling")]
    {
        counters.planning_ns += rejected_order.planning_ns;
    }
    let mut reused_order = rejected_order.entries.into_iter();
    for entry in collections {
        let collection_index = entry.collection_index;
        let filter = entry.filter;
        let fts = entry.fts;
        if let Some(delivery) = delivery.as_deref_mut() {
            delivery.check()?;
        }

        #[cfg(feature = "detailed-profiling")]
        let planning_start = DetailedInstant::now();
        let diagnostics = traces.is_some();
        let before = traces.as_ref().map(|_| counters.clone());
        let collection = &repo.collections[collection_index];
        let domain = collection_domain(collection, collection_index, active);
        let prepared_text_estimate = entry
            .text_term
            .map(|term| {
                crate::query_access::text::prepared_text_estimate(
                    &predicates,
                    term,
                    collection,
                    repo.instance_id,
                    domain.len(),
                )
            })
            .transpose()?
            .flatten();
        let prepared_filter_estimate = match (filter.as_ref(), entry.filter_shape.as_ref()) {
            (Some(BoundExpr::Term(term)), Some(PreparedExpr::Term(term_id))) => Some(
                crate::query_execution::restrictions::estimate_prepared_filter_term(
                    repo,
                    collection,
                    &predicates,
                    *term_id,
                    term,
                    domain.len(),
                ),
            )
            .transpose()?
            .flatten(),
            _ => None,
        };
        let text_estimate = prepared_text_estimate.or_else(|| {
            fts.as_ref()
                .and_then(|fts| estimate_fts_candidates(collection, fts))
        });
        let source_unit = 30.0 + collection.stats.mean_record_bytes() / 128.0;
        let (plan, mut candidates, text_first) = if let Some((mut plan, candidates)) =
            reused_order.next()
        {
            plan.planning.reused_order_plans += 1;
            (plan, candidates, false)
        } else {
            let mut input = domain.clone();
            let filter_estimate = prepared_filter_estimate
                .map(|(output, work)| (work, output))
                .or(filter
                    .as_ref()
                    .map(|filter| estimate_expr(collection, filter, domain.len()))
                    .transpose()?);
            let text_first =
                if let (Some((text_count, text_work)), Some((filter_work, filter_count))) =
                    (text_estimate, filter_estimate)
                {
                    let prepared_costs =
                        prepared_text_estimate.is_some() && prepared_filter_estimate.is_some();
                    if prepared_costs {
                        text_work < filter_work
                    } else {
                        text_work + text_count as f64 * source_unit * 0.1
                            < filter_work + filter_count as f64 * source_unit * 0.1
                    }
                } else {
                    text_estimate.is_some()
                };
            if text_first {
                if fts.is_some() {
                    if let Some(text) = crate::query_access::text::prepared_text_candidates(
                        &predicates,
                        entry.text_term.expect("prepared FTS term"),
                        collection,
                        repo.instance_id,
                        &input,
                    )? {
                        input &= text;
                    }
                }
            }
            let mut plan = filter
                .as_ref()
                .map(|f| {
                    plan_expr_restricted_with_execution(
                        repo,
                        collection,
                        &repo.segments,
                        f,
                        &domain,
                        &input,
                        query.options.evidence,
                        diagnostics,
                        execution,
                        Some(Arc::clone(&predicates)),
                    )
                })
                .transpose()?
                .unwrap_or(Plan {
                    candidates: None,
                    exact: true,
                    proven_hits: RoaringBitmap::new(),
                    proofs: Vec::new(),
                    choices: Vec::new(),
                    semijoin_work: Default::default(),
                    planning: PlanningCounters::default(),
                });
            let mut candidates = plan.candidates.take().unwrap_or(input);
            if !text_first {
                if let (true, Some((estimated, lookup))) = (fts.is_some(), text_estimate) {
                    if lookup + estimated as f64 * 0.05 < candidates.len() as f64 * source_unit {
                        if let Some(text_candidates) =
                            crate::query_access::text::prepared_text_candidates(
                                &predicates,
                                entry.text_term.expect("prepared FTS term"),
                                collection,
                                repo.instance_id,
                                &candidates,
                            )?
                        {
                            candidates &= text_candidates;
                        }
                    }
                }
            }
            (plan, candidates, text_first)
        };
        candidates &= &domain;
        counters.add_semijoin(plan.semijoin_work)?;
        let metadata_only_fts = fts.as_ref().is_some_and(|fts| {
            !fts.fields.is_empty()
                && fts
                    .fields
                    .iter()
                    .all(|field| matches!(field.path.root, PathRoot::Metadata(_)))
        });
        let relation_fts = fts
            .as_ref()
            .map(|fts| {
                crate::query_access::relations::semijoin::fts_witnesses(
                    repo,
                    &repo.segments,
                    fts,
                    &candidates,
                    execution,
                )
            })
            .transpose()?
            .flatten();
        let relation_fts_strategy = relation_fts.as_ref().map(|proof| {
            if proof.strategies.iter().all(|strategy| {
                *strategy == crate::query_access::relations::semijoin::Strategy::TargetFirst
            }) {
                "relation target-first semijoin"
            } else if proof.strategies.iter().all(|strategy| {
                *strategy == crate::query_access::relations::semijoin::Strategy::SourceRestricted
            }) {
                "relation source-restricted semijoin"
            } else {
                "relation mixed semijoin"
            }
        });
        let preverified_evidence = if let Some(proof) = relation_fts {
            counters.add_semijoin(proof.work)?;
            candidates = proof.evidence.keys().copied().collect();
            Some(proof.evidence)
        } else if metadata_only_fts {
            let matches =
                inventory_fts_evidence(repo, collection, fts.as_ref().unwrap(), &candidates)?;
            candidates = matches.keys().copied().collect();
            Some(matches)
        } else {
            None
        };
        let fused_filter_fts_terms =
            (filter.is_some() && fts.is_some() && !plan.exact && preverified_evidence.is_none())
                .then(|| {
                    entry
                        .filter_shape
                        .as_ref()
                        .map(PreparedExpr::term_ids)
                        .unwrap_or_default()
                        .into_iter()
                        .chain(entry.text_term)
                        .collect::<Vec<_>>()
                });
        #[cfg(feature = "detailed-profiling")]
        {
            counters.planning_ns += planning_start.elapsed().as_nanos() as u64;
        }
        counters.candidates += candidates.len();
        let needs_scan = sort.bound.needs_source
            || !plan.exact
            || (fts.is_some() && preverified_evidence.is_none());
        if let Some(traces) = traces.as_deref_mut() {
            traces.push(QueryPlanTrace {
                collection: collection.definition.name.clone(),
                generation: collection.generation.generation,
                bigram_alphabet: collection.raw.alphabet,
                folded_representation: crate::index::bigram::FOLDED_REPRESENTATION,
                fts_case_sensitivity: fts.as_ref().map(|fts| fts.case_sensitivity),
                fts_mode: fts.as_ref().map(|fts| fts.mode),
                fts_exact_literal: fts.as_ref().map(|fts| fts.source_literal.clone()),
                fts_prefilter_reason: regex_prefilter_reason(collection, fts.as_ref()),
                fts_covered_target: fts_covered_target(fts.as_ref()),
                fts_estimated_candidates: text_estimate.map(|estimate| estimate.0),
                remaining_verification: needs_scan,
                input_records: domain.len(),
                fts_strategy: if fts.as_ref().is_some_and(|fts| fts.fields.is_empty())
                    && collection.logical_records.len()
                        != collection.generation.records.len() as u64
                {
                    "logical text plus fallback source text"
                } else if fts.is_none() {
                    "absent"
                } else if metadata_only_fts {
                    "metadata inventory"
                } else if let Some(strategy) = relation_fts_strategy {
                    strategy
                } else if text_first {
                    "bigram before filter"
                } else if candidates.len() < domain.len() {
                    "filter before text verification"
                } else {
                    "text verification scan"
                }
                .into(),
                sort_strategy_estimated: false,
                normalized_sort: query.options.sort.clone(),
                sort_strategy: if sort.bound.has_computed {
                    "computed_top_k_full_key_evaluation"
                } else {
                    "bounded_top_k"
                }
                .into(),
                filter_exact: plan.exact,
                terms: plan.choices.clone(),
                planning: plan.planning,
                bound_terms: bound_term_diagnostics(repo, filter.as_ref()),
                candidates: candidates.len(),
                candidates_estimated: false,
                actual: None,
                observations: Vec::new(),
            });
        }
        if !needs_scan {
            if let Some(collector) = exact_collector.as_mut() {
                counters.exact_posting_matches += candidates.len();
                collector.add_set(&candidates, &mut counters);
                if let (Some(traces), Some(before)) = (traces.as_deref_mut(), before.as_ref()) {
                    finish_trace(traces, before, &counters);
                }
                continue;
            }
            for (index, slot) in candidates.iter().enumerate() {
                if let Some(delivery) = delivery.as_deref_mut() {
                    delivery.check()?;
                }
                let record = &collection.generation.records[slot as usize];
                let needs_evidence = query.options.evidence
                    && (cache_members.is_some()
                        || would_enter_page(repo, &sort, collection_index, record, &winners, cap));
                let mut evidence = if needs_evidence {
                    filter
                        .as_ref()
                        .map(|expr| proven_evidence(repo, collection, record, expr, &plan.proofs))
                        .transpose()?
                        .unwrap_or_default()
                } else {
                    HashSet::new()
                };
                if needs_evidence {
                    if let Some(fts_evidence) =
                        preverified_evidence.as_ref().and_then(|map| map.get(&slot))
                    {
                        evidence.extend(fts_evidence.iter().cloned());
                    }
                }
                counters.exact_posting_matches += 1;
                offer(
                    repo,
                    query,
                    &sort,
                    collection_index,
                    record,
                    evidence,
                    None,
                    None,
                    &mut winners,
                    &mut retained_key_bytes,
                    cap,
                    &mut counters,
                    cache_members.as_deref_mut(),
                    cache_budget,
                )?;
                if let Some(delivery) = delivery.as_deref_mut()
                    && (index + 1) % delivery.options().checkpoint_files == 0
                {
                    emit_provisional_page(
                        repo,
                        query,
                        &mut winners,
                        &mut counters,
                        &mut files_read,
                        delivery,
                        cache_budget.map_or(0, CacheBudget::used),
                        execution,
                    )?;
                }
            }
            if let Some(delivery) = delivery.as_deref_mut() {
                emit_provisional_page(
                    repo,
                    query,
                    &mut winners,
                    &mut counters,
                    &mut files_read,
                    delivery,
                    cache_budget.map_or(0, CacheBudget::used),
                    execution,
                )?;
            }
            if let (Some(traces), Some(before)) = (traces.as_deref_mut(), before.as_ref()) {
                finish_trace(traces, before, &counters);
            }
            continue;
        }
        if !sort.bound.has_computed
            && fts.is_none()
            && query.options.evidence
            && collection.logical_records.len() != collection.generation.records.len() as u64
        {
            // Fallback truth established from inventory metadata needs no
            // source verification, even when logical candidates still do.
            let fallback_proven = &(&candidates & &plan.proven_hits) - &collection.logical_records;
            for slot in fallback_proven.iter() {
                let record = &collection.generation.records[slot as usize];
                let evidence = filter
                    .as_ref()
                    .map(|expr| proven_evidence(repo, collection, record, expr, &plan.proofs))
                    .transpose()?
                    .unwrap_or_default();
                counters.exact_posting_matches += 1;
                offer(
                    repo,
                    query,
                    &sort,
                    collection_index,
                    record,
                    evidence,
                    None,
                    None,
                    &mut winners,
                    &mut retained_key_bytes,
                    cap,
                    &mut counters,
                    cache_members.as_deref_mut(),
                    cache_budget,
                )?;
            }
            candidates -= &fallback_proven;
        }
        if !sort.bound.has_computed && fts.is_none() && !query.options.evidence {
            let proven = &candidates & &plan.proven_hits;
            if let Some(collector) = exact_collector.as_mut() {
                counters.exact_posting_matches += proven.len();
                collector.add_set(&proven, &mut counters);
            } else {
                for slot in proven.iter() {
                    let record = &collection.generation.records[slot as usize];
                    counters.exact_posting_matches += 1;
                    offer(
                        repo,
                        query,
                        &sort,
                        collection_index,
                        record,
                        HashSet::new(),
                        None,
                        None,
                        &mut winners,
                        &mut retained_key_bytes,
                        cap,
                        &mut counters,
                        cache_members.as_deref_mut(),
                        cache_budget,
                    )?;
                }
            }
            candidates -= &proven;
        }
        let residual = filter.as_ref().map(|expr| {
            prepare_residual(
                expr,
                &plan.proofs,
                collection,
                candidates.len(),
                &predicates,
            )
        });
        let target_values = filter
            .as_ref()
            .map(|expr| {
                if let Some(delivery) = delivery.as_ref() {
                    crate::query_execution::relations::target_cache::prepare_stream(
                        repo,
                        collection_index,
                        expr,
                        &candidates,
                        delivery.token(),
                        delivery.max_buffered_bytes() / 4,
                        execution,
                    )
                } else {
                    crate::query_execution::relations::target_cache::prepare(
                        repo,
                        collection_index,
                        expr,
                        &candidates,
                        execution,
                    )
                }
            })
            .transpose()?
            .flatten();
        if let Some(cache) = &target_values {
            let work = cache.work;
            counters.relation_target_candidates += work.target_candidates;
            counters.relation_target_reads += work.target_reads;
            counters.relation_target_file_opens += work.target_file_opens;
            counters.relation_edge_visits += work.edge_visits;
            counters.files_opened += work.target_file_opens;
            counters.bytes_read += work.bytes_read;
            counters.relation_bytes_read += work.bytes_read;
            counters.record_parses += work.target_reads;
            crate::query_execution::relations::work::check_query_work_budget(&counters)?;
        }
        // Record slots are grouped by source file before opening any source.
        let mut files: BTreeMap<u32, Vec<(u32, ByteRange)>> = BTreeMap::new();
        for slot in candidates.iter() {
            let record = &collection.generation.records[slot as usize];
            files
                .entry(record.file.0)
                .or_default()
                .push((slot, record.range));
        }
        let files: Vec<_> = files.into_iter().collect();
        let mut projection = scan_projection(
            query,
            residual.as_ref(),
            &candidates,
            preverified_evidence.is_some(),
        );
        if let Some(fields) = projection.top_level.as_mut() {
            let mut needs_root = false;
            for term in sort.bound.terms[collection_index]
                .as_ref()
                .into_iter()
                .flatten()
            {
                if let BoundSortSource::Computed(computed) = &term.source
                    && computed.path.field.root == PathRoot::Data
                {
                    if let Some(first) = computed.path.field.segments.first() {
                        fields.insert(first.clone());
                    } else {
                        needs_root = true;
                        break;
                    }
                }
            }
            if needs_root {
                projection.top_level = None;
            }
        }
        prepare_value_text_projection(
            &mut projection,
            preverified_evidence.is_none()
                && (borrowed_value_text(collection, fts.as_ref())
                    || borrowed_relation_source_text(collection, fts.as_ref())),
        );
        let retain_all_evidence = cache_budget.is_some_and(CacheBudget::complete);
        let direct_plan = if preverified_evidence.is_none() {
            DirectSourcePlan::prepare(collection, &sort, fts.as_ref(), residual.as_ref())
        } else {
            None
        };
        let parallel = repo.options.query_workers > 1 && files.len() > 1;
        let has_related_sort = sort.bound.terms.iter().flatten().flatten().any(|term| {
            matches!(&term.source, BoundSortSource::Computed(computed) if computed.related.is_some())
        });
        // Bound in-flight parsed records while workers scan independent files.
        // Applying offers in file order preserves deterministic tie handling.
        let base_chunk_size = if sort.bound.has_computed {
            repo.options.query_workers.clamp(1, 8)
        } else if query.options.return_mode == ReturnMode::Locator || query.options.limit == 0 {
            256
        } else {
            repo.options.query_workers.min(64).max(1)
        };
        let chunk_size = delivery.as_ref().map_or(base_chunk_size, |delivery| {
            base_chunk_size
                .min(delivery.options().checkpoint_files)
                .max(1)
        });
        let mut file_cursor = 0;
        while file_cursor < files.len() {
            let mut group_size = chunk_size.min(files.len() - file_cursor);
            if sort.bound.has_computed && delivery.is_none() {
                let resident = winners
                    .iter()
                    .try_fold(0u64, |used, winner| {
                        used.checked_add(winner.retained_bytes)
                            .and_then(|used| used.checked_add(rank_key_bytes(&winner.key)))
                    })
                    .ok_or("computed sort memory count overflow")?;
                let targets = sort
                    .target_cache
                    .lock()
                    .map_err(|error| error.to_string())?;
                let external = target_values
                    .as_ref()
                    .map_or(0, |cache| cache.retained_bytes);
                let available = computed_sort_limit(execution)
                    .checked_sub(
                        resident
                            .saturating_add(targets.retained_bytes())
                            .saturating_add(external),
                    )
                    .ok_or_else(|| {
                        computed_sort_resource(execution, "computed sort memory budget exceeded")
                    })?;
                let parallel_available = available.saturating_sub(if has_related_sort {
                    targets.reserve_headroom()
                } else {
                    0
                });
                while group_size > 1 {
                    let share = parallel_available / (group_size as u64 * 2);
                    if files[file_cursor..file_cursor + group_size]
                        .iter()
                        .all(|(_, ranges)| ranges.iter().all(|(_, range)| range.len() <= share))
                    {
                        break;
                    }
                    group_size -= 1;
                }
            }
            let chunk = &files[file_cursor..file_cursor + group_size];
            if let Some(delivery) = delivery.as_deref_mut() {
                delivery.check()?;
            }
            let stream_token = delivery.as_ref().map(|delivery| delivery.token().clone());
            let per_file_budget = if let Some(delivery) = delivery.as_deref_mut() {
                let resident = winners
                    .iter()
                    .map(winner_bytes)
                    .sum::<usize>()
                    .saturating_add(sort_target_retained_bytes(&sort)?)
                    .saturating_add(
                        target_values
                            .as_ref()
                            .map_or(0, |cache| cache.estimated_bytes()),
                    )
                    // Reserve the complete potential membership allocation while
                    // workers run; their atomic reservations can grow concurrently.
                    .saturating_add(cache_budget.map_or(0, |budget| budget.initial));
                delivery.check_buffer(resident)?;
                let per_file = delivery.max_buffered_bytes().saturating_sub(resident)
                    / (chunk.len().max(1) * 4);
                if per_file == 0 {
                    return Err(computed_sort_resource(
                        execution,
                        "stream resource: no room for source scan batch",
                    ));
                }
                Some(per_file as u64)
            } else {
                None
            };
            let per_file_budget = if sort.bound.has_computed {
                let resident = winners
                    .iter()
                    .try_fold(0u64, |used, winner| {
                        used.checked_add(winner.retained_bytes)
                            .and_then(|used| used.checked_add(rank_key_bytes(&winner.key)))
                    })
                    .ok_or("computed sort memory count overflow")?;
                let targets = sort
                    .target_cache
                    .lock()
                    .map_err(|error| error.to_string())?;
                let external = target_values
                    .as_ref()
                    .map_or(0, |cache| cache.retained_bytes);
                let used = resident
                    .checked_add(targets.retained_bytes())
                    .and_then(|used| used.checked_add(external))
                    .ok_or("computed sort memory count overflow")?;
                let available = computed_sort_limit(execution)
                    .checked_sub(used)
                    .ok_or_else(|| {
                        computed_sort_resource(execution, "computed sort memory budget exceeded")
                    })?;
                // A worker may hold a source batch while retaining its local keys.
                let share = if chunk.len() == 1 {
                    available
                } else {
                    let parallel_available = available.saturating_sub(if has_related_sort {
                        targets.reserve_headroom()
                    } else {
                        0
                    });
                    parallel_available / (chunk.len() as u64 * 2)
                };
                if share == 0 {
                    return Err(computed_sort_resource(
                        execution,
                        "computed sort memory budget exceeded",
                    ));
                }
                Some(per_file_budget.map_or(share, |stream| stream.min(share)))
            } else {
                per_file_budget
            };
            let payload_cutoff = if query.options.return_mode != ReturnMode::Locator
                && query.options.limit > 0
                && winners.len() == cap
            {
                winners.peek().map(|winner: &Winner| winner.key.clone())
            } else {
                None
            };
            let scanned: Vec<_> = if parallel {
                repo.workers.install(|| {
                    chunk
                        .par_iter()
                        .map(|(file_id, ranges)| {
                            scan_file(
                                repo,
                                scan_query,
                                &sort,
                                collection_index,
                                residual.as_ref(),
                                if preverified_evidence.is_some() {
                                    None
                                } else {
                                    fts.as_ref()
                                },
                                preverified_evidence.as_ref(),
                                direct_plan.as_ref(),
                                target_values.as_ref(),
                                &projection,
                                retain_all_evidence,
                                cache_budget,
                                payload_cutoff.as_ref(),
                                *file_id,
                                ranges,
                                None,
                                stream_token.as_ref(),
                                per_file_budget,
                                exact_collector
                                    .as_ref()
                                    .is_some_and(|collector| collector.members.is_some()),
                                execution,
                            )
                        })
                        .collect()
                })
            } else {
                chunk
                    .iter()
                    .map(|(file_id, ranges)| {
                        scan_file(
                            repo,
                            scan_query,
                            &sort,
                            collection_index,
                            residual.as_ref(),
                            if preverified_evidence.is_some() {
                                None
                            } else {
                                fts.as_ref()
                            },
                            preverified_evidence.as_ref(),
                            direct_plan.as_ref(),
                            target_values.as_ref(),
                            &projection,
                            retain_all_evidence,
                            cache_budget,
                            payload_cutoff.as_ref(),
                            *file_id,
                            ranges,
                            None,
                            stream_token.as_ref(),
                            per_file_budget,
                            exact_collector
                                .as_ref()
                                .is_some_and(|collector| collector.members.is_some()),
                            execution,
                        )
                    })
                    .collect()
            };
            if let Some(delivery) = delivery.as_deref_mut() {
                let pending = scanned
                    .iter()
                    .filter_map(|item| item.as_ref().ok())
                    .map(scanned_file_bytes)
                    .sum::<usize>();
                let retained = winners
                    .iter()
                    .map(winner_bytes)
                    .sum::<usize>()
                    .saturating_add(sort_target_retained_bytes(&sort)?)
                    .saturating_add(
                        target_values
                            .as_ref()
                            .map_or(0, |cache| cache.estimated_bytes()),
                    )
                    .saturating_add(cache_budget.map_or(0, CacheBudget::used))
                    .saturating_add(pending);
                delivery.check_buffer(retained)?;
            }
            let payload_upper = scanned
                .iter()
                .filter_map(|result| result.as_ref().ok())
                .try_fold(0u64, |total, outcome| {
                    total.checked_add(outcome.peak_payload_bytes)
                })
                .and_then(|used| {
                    winners.iter().try_fold(used, |total, winner: &Winner| {
                        total.checked_add(winner.retained_bytes)
                    })
                })
                .ok_or("sort payload memory count overflow")?;
            counters.peak_retained_payload_bytes =
                counters.peak_retained_payload_bytes.max(payload_upper);
            let key_upper = retained_key_bytes
                .checked_add(
                    scanned
                        .iter()
                        .filter_map(|result| result.as_ref().ok())
                        .try_fold(0u64, |total, outcome| {
                            total.checked_add(outcome.peak_sort_key_bytes)
                        })
                        .ok_or("sort key memory count overflow")?,
                )
                .ok_or("sort key memory count overflow")?;
            if key_upper > MAX_SORT_KEY_BYTES {
                return Err(computed_sort_resource(
                    execution,
                    "sort key memory budget exceeded",
                ));
            }
            if sort.bound.has_computed {
                let targets = sort
                    .target_cache
                    .lock()
                    .map_err(|error| error.to_string())?;
                let external_bytes = target_values
                    .as_ref()
                    .map_or(0, |values| values.retained_bytes);
                let target_bytes = targets
                    .retained_bytes()
                    .checked_add(external_bytes)
                    .ok_or("computed sort target byte count overflow")?;
                check_execution_sort_budget(execution, payload_upper, key_upper, target_bytes)?;
            }
            counters.peak_retained_sort_key_bytes =
                counters.peak_retained_sort_key_bytes.max(key_upper);
            for ((file_id, _), outcome) in chunk.iter().zip(scanned) {
                let outcome = outcome?;
                if let Some(terms) = fused_filter_fts_terms.as_deref()
                    && terms.len() > 1
                    && outcome.scanned_records > 0
                {
                    crate::query_execution::request::record_fused_observation_with_residual(
                        repo.instance_id,
                        terms,
                        crate::query_execution::domain::domain_id(
                            repo.instance_id,
                            collection.generation.collection.0,
                            collection.generation.generation,
                            None,
                        ),
                        collection.generation.collection.0,
                        outcome.scanned_records,
                        outcome.match_count,
                        outcome.match_count,
                        outcome.residual_evaluations,
                        "fused_filter_fts_source",
                    );
                }
                if outcome.files_opened > 0 || outcome.bytes_read > 0 {
                    if let Some(seen) = files_read.as_mut() {
                        seen.insert(*file_id);
                    } else {
                        distinct_files += 1;
                    }
                }
                counters.add_scan(ScanWork::from(&outcome), &outcome.relation_work)?;
                if let Some(collector) = exact_collector.as_mut() {
                    counters.total_matches += outcome.match_count;
                    if let Some(members) = collector.members.as_deref_mut() {
                        *members |= &outcome.exact_members;
                    }
                } else {
                    counters.total_matches += outcome.match_count - outcome.matches.len() as u64;
                    if let Some(members) = cache_members.as_deref_mut() {
                        members.extend(outcome.cache_members);
                    }
                    for matched in outcome.matches {
                        let record = &collection.generation.records[matched.slot as usize];
                        offer(
                            repo,
                            query,
                            &sort,
                            collection_index,
                            record,
                            matched.evidence.iter().cloned().collect(),
                            matched.payload,
                            matched.key,
                            &mut winners,
                            &mut retained_key_bytes,
                            cap,
                            &mut counters,
                            None,
                            None,
                        )?;
                    }
                }
            }
            if let Some(delivery) = delivery.as_deref_mut() {
                emit_provisional_page(
                    repo,
                    query,
                    &mut winners,
                    &mut counters,
                    &mut files_read,
                    delivery,
                    target_values
                        .as_ref()
                        .map_or(0, |cache| cache.estimated_bytes())
                        .saturating_add(sort_target_retained_bytes(&sort)?)
                        .saturating_add(cache_budget.map_or(0, CacheBudget::used)),
                    execution,
                )?;
            }
            file_cursor += group_size;
        }
        if let (Some(traces), Some(before)) = (traces.as_deref_mut(), before.as_ref()) {
            finish_trace(traces, before, &counters);
        }
    }
    let sort_targets = sort
        .target_cache
        .lock()
        .map_err(|error| error.to_string())?;
    counters.sort_target_reads += sort_targets.reads;
    counters.sort_target_reuse += sort_targets.reuse;
    counters.relation_edge_visits += sort_targets.edge_visits;
    counters.files_opened += sort_targets.reads;
    counters.record_parses += sort_targets.reads;
    counters.bytes_read += sort_targets.bytes_read;
    drop(sort_targets);
    let count = counters.total_matches as usize;
    counters.match_count_complete = true;
    let ordered = winners.into_sorted_vec();
    let mut page: Vec<_> = ordered
        .into_iter()
        .skip(query.options.offset)
        .take(query.options.limit)
        .collect();
    if sort.bound.has_computed {
        let page_payload = page
            .iter()
            .try_fold(0u64, |total, winner: &Winner| {
                let record = &repo.collections[winner.collection_index].generation.records
                    [winner.slot as usize];
                let estimate = if all_offset.is_some() {
                    0
                } else if query.options.return_mode == ReturnMode::Logical {
                    record.range.len().checked_mul(4)?
                } else if query.options.return_mode == ReturnMode::Bytes {
                    record.range.len()
                } else {
                    0
                };
                total.checked_add(estimate)
            })
            .ok_or("computed sort payload count overflow")?;
        let targets = sort
            .target_cache
            .lock()
            .map_err(|error| error.to_string())?;
        check_execution_sort_budget(
            execution,
            page_payload,
            retained_key_bytes,
            targets.retained_bytes(),
        )?;
    }
    if let Some(delivery) = delivery.as_deref_mut()
        && all_offset.is_none()
    {
        let anticipated = page
            .iter()
            .map(|winner| {
                winner_bytes(winner).saturating_add(
                    if winner.hit.payload.is_none()
                        && query.options.return_mode != ReturnMode::Locator
                    {
                        repo.collections[winner.collection_index].generation.records
                            [winner.slot as usize]
                            .range
                            .len() as usize
                    } else {
                        0
                    },
                )
            })
            .sum();
        delivery.check_buffer(anticipated)?;
    }
    if all_offset.is_none() {
        materialize_winner_page(
            repo,
            query,
            &mut page,
            &mut counters,
            &mut files_read,
            traces.as_deref_mut(),
            delivery.as_ref().map(|delivery| delivery.token()),
            delivery
                .as_ref()
                .map(|delivery| delivery.max_buffered_bytes()),
            execution,
        )?;
    }
    counters.returned_records = page.len() as u64;
    counters.distinct_files = files_read.map_or(distinct_files, |seen| seen.len());
    Ok(SearchResult {
        count: query.options.count.then_some(count),
        hits: page.into_iter().map(|winner| winner.hit).collect(),
        counters,
    })
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
