//! Bounded result retention, grouped source access, and payload materialization.

use std::cmp::Ordering;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use rayon::prelude::*;
use roaring::RoaringBitmap;
use serde::Serialize;
use smallvec::SmallVec;

use crate::build::{ExtendedBatch, IndexedCollectionSegment};
use crate::execution_options::{BigramAlphabet, ReturnMode, SortField};
use crate::executor::{
    Plan, PlanningCounters, Proof, TermChoice, estimate_expr, estimate_fts_candidates,
    estimate_plan_expr, extended_filter_bounds, fts_candidates, plan_expr_restricted_with_execution, segment_filter_bounds,
    segment_filter_bounds_with_catalog, segment_fts_candidates,
};
use crate::inventory::{ActiveRecords, ByteRange, FileEntry, RecordClass, RecordEntry};
use crate::json_scalar_text::{BorrowedScanQuery, scan_borrowed_segments_worker};
use crate::lifecycle::{IndexRevision, PublishedSegment};
use crate::lifecycle_catalog::FileCatalog;
use crate::materialization::{payload_from_bytes, payload_from_bytes_with_execution};
use crate::measurement::{ScanWork, SourceAvailabilityLog, SourceDiagnostic, WorkCounters};
use crate::model::{FieldPath, LogicalType, PathRoot, RecordId, RecordSlot, Value};
use crate::posting::PostingIndex;
use crate::query::{CaseSensitivity, FtsMode, Operator, Quantifier, Query};
use crate::record_preparation::{
    materialize, prepare_decoded_record_with_borrowed_fts, prepare_query_record_with_execution,
};
use crate::reference::{
    BoundExpr, BoundFts, BoundTerm, Derived, ExactInput, FoldRecordGuard, RecordContext,
    Transform, eval_bound_fts, eval_term, eval_unavailable_filter, same_bound_term,
};
use crate::repository::{CollectionState, Payload, Repository, SearchHit, SearchResult};
use crate::sort_binding::{
    BoundSort, BoundSortSource, BoundSortTerm, MergedSortTargetCache, SortTargetCache,
};
use crate::sorting::{InferredOrderKey, SortColumn, SortKey, SortOptions, simple_composite_part};
use crate::source::{
    ExtractedRecord, RecordRangeReader, ScanProjection, read_record_range_for_search,
};
use crate::source_access::{
    SourceAccessSession, classify_source_read_failure_for_search,
    probe_source_unavailable_for_search,
};
use crate::streaming::{
    Abort, CancellationToken, Delivery, ExecutionContext, ExecutionErrorKind, ExecutionLimits,
    ResultScope, StreamControl, StreamError, StreamEvent, StreamMode, StreamOptions, StreamOutcome,
    StreamSummary,
};
use crate::verified::{CachedMember, CachedResult, VerifiedCache};

const MAX_SORT_KEY_BYTES: u64 = 64 * 1024 * 1024;
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


/// One cached member that needs a source read to compute its sort key.
struct CachedSortRead {
    collection_index: usize,
    file_id: u32,
    member_index: u32,
    range: ByteRange,
}


fn cached_sort_fixed_bytes(cached: &CachedResult, reads: usize) -> Result<u64, String> {
    let mut bytes = cached
        .members
        .capacity()
        .checked_mul(std::mem::size_of::<CachedMember>())
        .and_then(|used| used.checked_add(std::mem::size_of::<CachedResult>()))
        .ok_or("cached sort memory count overflow")?;
    for member in &cached.members {
        bytes = bytes
            .checked_add(
                member
                    .matched_fields
                    .capacity()
                    .checked_mul(std::mem::size_of::<String>())
                    .ok_or("cached sort memory count overflow")?,
            )
            .ok_or("cached sort memory count overflow")?;
        for field in &member.matched_fields {
            bytes = bytes
                .checked_add(field.capacity())
                .ok_or("cached sort memory count overflow")?;
        }
    }
    bytes = bytes
        .checked_add(
            reads
                .checked_mul(std::mem::size_of::<CachedSortRead>())
                .ok_or("cached sort memory count overflow")?,
        )
        .and_then(|used| used.checked_add(128 * std::mem::size_of::<(u32, ByteRange)>()))
        .ok_or("cached sort memory count overflow")?;
    u64::try_from(bytes).map_err(|_| "cached sort memory count overflow".into())
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


/// Comparable sort key with collection and ID tie breakers.
#[derive(Clone)]
struct RankKey {
    parts: Vec<(SortKey, bool, SortOptions)>,
    collection: String,
    id: String,
}


/// Sort paths are parsed and columns resolved once per query.
struct ResolvedSort<'a> {
    columns: Vec<Option<Vec<(&'a SortColumn, bool)>>>,
    bound: BoundSort<'a>,
    target_cache: std::sync::Mutex<SortTargetCache>,
}


impl<'a> ResolvedSort<'a> {
    fn new(repo: &'a Repository, query: &Query) -> Result<Self, String> {
        let bound = BoundSort::bind(repo, query)?;
        Ok(Self::from_bound(repo, query, bound))
    }

    fn from_bound(repo: &'a Repository, query: &Query, bound: BoundSort<'a>) -> Self {
        let columns = repo
            .collections
            .iter()
            .enumerate()
            .map(|(index, collection)| {
                if bound.has_computed {
                    return None;
                }
                if !query.collections.is_empty()
                    && !query.collections.contains(&collection.definition.name)
                {
                    return None;
                }
                Some(
                    bound.terms[index]
                        .as_ref()
                        .expect("selected collection")
                        .iter()
                        .map(|term| {
                            let BoundSortSource::Stored(column) = &term.source else {
                                unreachable!("all terms resident")
                            };
                            (*column, term.descending)
                        })
                        .collect(),
                )
            })
            .collect();
        Self {
            columns,
            bound,
            target_cache: std::sync::Mutex::new(SortTargetCache::default()),
        }
    }

    fn compare_to_rank(
        &self,
        collection_index: usize,
        record: &RecordEntry,
        collection_name: &str,
        worst: &RankKey,
    ) -> Ordering {
        if self.bound.has_computed {
            return Ordering::Less;
        }
        for ((column, descending), (other, _, _)) in self.columns[collection_index]
            .as_ref()
            .expect("selected collection")
            .iter()
            .zip(&worst.parts)
        {
            let key = &column.keys[record.slot.0 as usize];
            let order = key.compare_with(other, *descending, column.options);
            if !order.is_eq() {
                return order;
            }
        }
        (collection_name, record.id.0.as_str()).cmp(&(worst.collection.as_str(), worst.id.as_str()))
    }
}


impl PartialEq for RankKey {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}


impl Eq for RankKey {}


impl PartialOrd for RankKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}


impl Ord for RankKey {
    fn cmp(&self, other: &Self) -> Ordering {
        crate::sort_binding::compare_parts(&self.parts, &other.parts)
            .then_with(|| (&self.collection, &self.id).cmp(&(&other.collection, &other.id)))
    }
}


/// Bounded page candidate and its materialized result.
#[derive(Clone)]
struct Winner {
    key: RankKey,
    collection_index: usize,
    slot: u32,
    hit: SearchHit,
    retained_bytes: u64,
}


/// Verified record and any evidence or retained payload.
struct ScannedMatch {
    slot: u32,
    key: Option<RankKey>,
    evidence: HashSet<String>,
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
    relation_work: crate::relation_query::RelationWork,
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


/// Compact cache membership is budgeted independently of page payloads.
struct CacheBudget {
    initial: usize,
    remaining: AtomicUsize,
    complete: AtomicBool,
}


impl CacheBudget {
    fn new(bytes: usize) -> Self {
        Self {
            initial: bytes,
            remaining: AtomicUsize::new(bytes),
            complete: AtomicBool::new(true),
        }
    }

    fn complete(&self) -> bool {
        self.complete.load(AtomicOrdering::Relaxed)
    }

    fn used(&self) -> usize {
        self.initial
            .saturating_sub(self.remaining.load(AtomicOrdering::Relaxed))
    }

    fn reserve(&self, fields: &[String]) -> bool {
        if !self.complete() {
            return false;
        }
        let bytes = std::mem::size_of::<CachedMember>()
            + fields
                .iter()
                .map(|field| field.len() + std::mem::size_of::<String>())
                .sum::<usize>();
        if self
            .remaining
            .try_update(
                AtomicOrdering::Relaxed,
                AtomicOrdering::Relaxed,
                |remaining| remaining.checked_sub(bytes),
            )
            .is_ok()
        {
            true
        } else {
            self.complete.store(false, AtomicOrdering::Relaxed);
            false
        }
    }
}


fn scan_projection(
    query: &Query,
    residual: Option<&Residual<'_>>,
    candidates: &RoaringBitmap,
    fts_proven: bool,
) -> ScanProjection {
    let collect_text = query.fts.is_some() && !fts_proven;
    fn uses_root(expr: &Residual<'_>) -> bool {
        match expr {
            Residual::Term(term, _) => {
                (term.path.root == PathRoot::Data && term.path.segments.is_empty())
                    || term.rhs.as_ref().is_some_and(|rhs| {
                        rhs.path.root == PathRoot::Data && rhs.path.segments.is_empty()
                    })
            }
            Residual::And(parts) | Residual::Or(parts) => parts.iter().any(uses_root),
            Residual::Not(inner) => uses_root(inner),
        }
    }
    if residual.is_some_and(uses_root)
        || query.fts.as_ref().is_some_and(|fts| {
            fts.fields
                .iter()
                .any(|field| field.path.root == PathRoot::Data && field.path.segments.is_empty())
        })
    {
        let mut projection = ScanProjection::full(collect_text);
        projection.resolved_text_only =
            collect_text && query.fts.as_ref().is_some_and(|fts| !fts.fields.is_empty());
        return projection;
    }
    if !fts_proven && query.fts.as_ref().is_some_and(|fts| fts.fields.is_empty()) {
        return ScanProjection::full(true);
    }

    fn add_filter_fields(
        expr: &Residual<'_>,
        candidates: &RoaringBitmap,
        fields: &mut HashSet<String>,
    ) {
        match expr {
            Residual::Term(term, proof) => {
                let fully_proven = proof.is_some_and(|proof| candidates.is_subset(&proof.input));
                if !fully_proven
                    && term.path.root == PathRoot::Data
                    && !term
                        .matcher
                        .as_ref()
                        .is_some_and(|matcher| matcher.input() == ExactInput::OriginalRecordBytes)
                {
                    if let Some(first) = term.path.segments.first() {
                        fields.insert(first.clone());
                    }
                }
                if let Some(related) = &term.related
                    && let Some(source) = &related.source_operand
                    && source.path.root == PathRoot::Data
                    && let Some(first) = source.path.segments.first()
                {
                    fields.insert(first.clone());
                }
                if !fully_proven
                    && let Some(rhs) = &term.rhs
                    && rhs.path.root == PathRoot::Data
                    && let Some(first) = rhs.path.segments.first()
                {
                    fields.insert(first.clone());
                }
            }
            Residual::And(parts) | Residual::Or(parts) => {
                for part in parts {
                    add_filter_fields(part, candidates, fields);
                }
            }
            Residual::Not(inner) => add_filter_fields(inner, candidates, fields),
        }
    }
    let mut fields = HashSet::new();
    if let Some(residual) = residual {
        add_filter_fields(residual, candidates, &mut fields);
    }
    if let Some(fts) = &query.fts
        && !fts_proven
    {
        for field in &fts.fields {
            if field.path.root == PathRoot::Data {
                if let Some(first) = field.path.segments.first() {
                    fields.insert(first.clone());
                }
            }
        }
    }
    ScanProjection {
        top_level: Some(fields),
        collect_text,
        resolved_text_only: collect_text
            && query.fts.as_ref().is_some_and(|fts| !fts.fields.is_empty()),
    }
}

fn borrowed_value_text(collection: &CollectionState, fts: Option<&BoundFts>) -> bool {
    !collection.source.is_extension()
        && matches!(collection.source.id(), "json" | "jsonl")
        && fts.is_some_and(|fts| fts.fields.iter().all(|field| field.related.is_none()))
}

fn borrowed_relation_source_text(
    collection: &CollectionState,
    fts: Option<&BoundFts>,
) -> bool {
    !collection.source.is_extension()
        && matches!(collection.source.id(), "json" | "jsonl")
        && fts.is_some_and(|fts| {
            fts.fields.iter().any(|field| field.related.is_some())
                && fts
                    .fields
                    .iter()
                    .filter(|field| field.related.is_none())
                    .all(|field| {
                        matches!(field.path.root, PathRoot::Data) && field.query_path.is_none()
                    })
        })
}

fn prepare_value_text_projection(projection: &mut ScanProjection, borrow_text: bool) {
    if borrow_text {
        projection.collect_text = false;
        projection.resolved_text_only = false;
    }
}


enum Residual<'a> {
    Term(&'a BoundTerm, Option<&'a Proof>),
    And(Vec<Residual<'a>>),
    Or(Vec<Residual<'a>>),
    Not(Box<Residual<'a>>),
}


/// Collection-plan eligibility for matching trusted built-in source scalars.
#[derive(Clone, Copy)]
struct DirectSourcePlan<'a> {
    fts: Option<&'a BoundFts>,
    filter: Option<(&'a BoundTerm, Option<&'a Proof>)>,
    decoded_scalars: bool,
}


fn source_fts_eligible(
    collection: &CollectionState,
    fts: &BoundFts,
    decoded_scalars: bool,
) -> bool {
    fts.mode.is_literal()
        && fts.raw_operation.is_none()
        && fts.raw_matcher.is_none()
        && (decoded_scalars || !collection.definition.schema.has_converted_fields())
        && fts.fields.iter().all(|field| {
            matches!(field.path.root, PathRoot::Data)
                && field.related.is_none()
                && field.operation.is_none()
                && field.matcher.is_none()
                && !field
                    .query_path
                    .as_ref()
                    .is_some_and(|path| path.has_explicit_array_step)
        })
}


impl<'a> DirectSourcePlan<'a> {
    fn prepare(
        collection: &CollectionState,
        sort: &ResolvedSort<'_>,
        fts: Option<&'a BoundFts>,
        residual: Option<&'a Residual<'a>>,
    ) -> Option<Self> {
        let filter = match residual {
            None => None,
            Some(Residual::Term(term, proof)) => Some((*term, *proof)),
            Some(_) => return None,
        };
        Self::prepare_filter(collection, sort, fts, filter)
    }

    fn prepare_bound(
        collection: &CollectionState,
        sort: &ResolvedSort<'_>,
        fts: Option<&'a BoundFts>,
        filter: Option<&'a BoundExpr>,
    ) -> Option<Self> {
        let filter = match filter {
            None => None,
            Some(BoundExpr::Term(term)) => Some((term, None)),
            Some(_) => return None,
        };
        Self::prepare_filter(collection, sort, fts, filter)
    }

    fn prepare_filter(
        collection: &CollectionState,
        sort: &ResolvedSort<'_>,
        fts: Option<&'a BoundFts>,
        filter: Option<(&'a BoundTerm, Option<&'a Proof>)>,
    ) -> Option<Self> {
        if sort.bound.has_computed || collection.source.is_extension() {
            return None;
        }
        let decoded_scalars = match collection.source.id() {
            "json_decoded_scalars" => true,
            "json" | "jsonl" => false,
            _ => return None,
        };
        if let Some(fts) = fts {
            if !source_fts_eligible(collection, fts, decoded_scalars) {
                return None;
            }
        }
        let filter = match filter {
            None => None,
            Some((term, proof))
                if matches!(term.path.root, PathRoot::Data)
                    && term.related.is_none()
                    && term.rhs.is_none()
                    && !term.query_path.has_explicit_array_step
                    && (!term.multi || term.source_kind == LogicalType::Unknown)
                    && !term.implicit_array
                    && matches!(term.source_kind, LogicalType::String | LogicalType::Unknown)
                    && matches!(term.operator, Operator::Contains | Operator::NotContains)
                    && term.derived == Derived::None
                    && term.operation.is_none()
                    && term.matcher.is_none()
                    && term.prepared_contains.is_some() =>
            {
                Some((term, proof))
            }
            Some(_) => return None,
        };
        (fts.is_some() || filter.is_some()).then_some(Self {
            fts,
            filter,
            decoded_scalars,
        })
    }
}


fn prepare_residual<'a>(
    expr: &'a BoundExpr,
    proofs: &'a [Proof],
    collection: &crate::repository::CollectionState,
    input_len: u64,
) -> Residual<'a> {
    match expr {
        BoundExpr::Term(term) => Residual::Term(
            term,
            proofs
                .iter()
                .find(|proof| same_bound_term(&proof.term, term)),
        ),
        BoundExpr::And(parts) | BoundExpr::Or(parts) => {
            let is_and = matches!(expr, BoundExpr::And(_));
            let mut ordered = parts
                .iter()
                .enumerate()
                .map(|(index, part)| {
                    let (cost, output) = estimate_expr(collection, part, input_len);
                    let fraction = output as f64 / input_len.max(1) as f64;
                    let probability = if is_and { 1.0 - fraction } else { fraction };
                    let score = cost / probability.max(0.05);
                    (
                        score,
                        index,
                        prepare_residual(part, proofs, collection, input_len),
                    )
                })
                .collect::<Vec<_>>();
            ordered.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
            let children = ordered.into_iter().map(|(_, _, child)| child).collect();
            if is_and {
                Residual::And(children)
            } else {
                Residual::Or(children)
            }
        }
        BoundExpr::Not(inner) => Residual::Not(Box::new(prepare_residual(
            inner, proofs, collection, input_len,
        ))),
    }
}

type FoldKey<'a> = (&'a [String], &'a str, u32);

fn fold_key(term: &BoundTerm) -> Option<FoldKey<'_>> {
    (term.path.root == PathRoot::Data
        && term.transform == Transform::CaseFold
        && term.rhs.is_none()
        && term.related.is_none()
        && term.operation.is_none()
        && term.matcher.is_none()
        && term.derived == Derived::None)
        .then_some((
            &term.path.segments,
            term.binding_name.as_str(),
            term.binding_version,
        ))
}

fn repeated_fold_key<'a>(term: &'a BoundTerm, seen: &mut SmallVec<[FoldKey<'a>; 8]>) -> bool {
    let Some(key) = fold_key(term) else {
        return false;
    };
    if seen.contains(&key) {
        return true;
    }
    seen.push(key);
    false
}

fn shared_fold_residual(residual: Option<&Residual<'_>>) -> bool {
    fn visit<'a>(node: &'a Residual<'a>, seen: &mut SmallVec<[FoldKey<'a>; 8]>) -> bool {
        match node {
            Residual::Term(term, _) => repeated_fold_key(term, seen),
            Residual::And(parts) | Residual::Or(parts) => {
                parts.iter().any(|part| visit(part, seen))
            }
            Residual::Not(inner) => visit(inner, seen),
        }
    }
    let mut seen = SmallVec::new();
    residual.is_some_and(|node| visit(node, &mut seen))
}

fn shared_fold_bound(filter: Option<&BoundExpr>) -> bool {
    fn visit<'a>(node: &'a BoundExpr, seen: &mut SmallVec<[FoldKey<'a>; 8]>) -> bool {
        match node {
            BoundExpr::Term(term) => repeated_fold_key(term, seen),
            BoundExpr::And(parts) | BoundExpr::Or(parts) => {
                parts.iter().any(|part| visit(part, seen))
            }
            BoundExpr::Not(inner) => visit(inner, seen),
        }
    }
    let mut seen = SmallVec::new();
    filter.is_some_and(|node| visit(node, &mut seen))
}


impl Residual<'_> {
    fn unavailable(
        &self,
        ctx: &RecordContext<'_>,
        evidence: &mut HashSet<String>,
        evaluations: &mut u64,
    ) -> Result<Option<bool>, String> {
        match self {
            Self::Term(term, _) => {
                if term.related.is_some()
                    || term.path.root == PathRoot::Data
                    || term.path.root == PathRoot::Backlinks
                    || term
                        .rhs
                        .as_ref()
                        .is_some_and(|rhs| rhs.path.root == PathRoot::Data)
                {
                    return Ok(None);
                }
                *evaluations += 1;
                let matched = eval_term(term, ctx)?;
                if matched && self.can_emit_evidence() {
                    evidence.insert(term.query_path.display.clone());
                    if let Some(rhs) = &term.rhs {
                        evidence.insert(rhs.query_path.display.clone());
                    }
                }
                Ok(Some(matched))
            }
            Self::Not(inner) => Ok(inner
                .unavailable(ctx, &mut HashSet::new(), evaluations)?
                .map(|v| !v)),
            Self::And(parts) => {
                let mut unknown = false;
                let mut branch = HashSet::new();
                for part in parts {
                    match part.unavailable(ctx, &mut branch, evaluations)? {
                        Some(false) => return Ok(Some(false)),
                        None => unknown = true,
                        Some(true) => {}
                    }
                }
                if unknown {
                    Ok(None)
                } else {
                    evidence.extend(branch);
                    Ok(Some(true))
                }
            }
            Self::Or(parts) => {
                let mut unknown = false;
                let mut matched = false;
                for part in parts {
                    let mut branch = HashSet::new();
                    match part.unavailable(ctx, &mut branch, evaluations)? {
                        Some(true) => {
                            matched = true;
                            evidence.extend(branch);
                        }
                        None => unknown = true,
                        Some(false) => {}
                    }
                }
                if matched {
                    Ok(Some(true))
                } else if unknown {
                    Ok(None)
                } else {
                    Ok(Some(false))
                }
            }
        }
    }

    fn proven(proof: Option<&Proof>, slot: u32) -> Option<bool> {
        proof
            .filter(|proof| proof.input.contains(slot))
            .map(|proof| proof.matches.contains(slot))
    }

    fn can_emit_evidence(&self) -> bool {
        match self {
            Self::Term(term, _) => {
                term.derived == Derived::None
                    && !matches!(term.operator, Operator::Ne | Operator::NotContains)
            }
            Self::And(parts) | Self::Or(parts) => parts.iter().any(Self::can_emit_evidence),
            Self::Not(_) => false,
        }
    }

    fn membership(
        &self,
        repo: &Repository,
        slot: u32,
        ctx: &RecordContext<'_>,
        evaluations: &mut u64,
        relation_work: &mut crate::relation_query::RelationWork,
        target_values: Option<&crate::relation_target_cache::TargetValueCache>,
    ) -> Result<Option<bool>, String> {
        Ok(match self {
            Self::Term(term, proof) => match Self::proven(*proof, slot) {
                Some(result) => Some(result),
                None => {
                    *evaluations += 1;
                    if term.related.is_some() {
                        crate::relation_query::eval_related_term(
                            repo,
                            term,
                            ctx,
                            relation_work,
                            target_values,
                        )?
                    } else {
                        Some(eval_term(term, ctx)?)
                    }
                }
            },
            Self::And(parts) => {
                let mut unknown = false;
                for part in parts {
                    match part.membership(
                        repo,
                        slot,
                        ctx,
                        evaluations,
                        relation_work,
                        target_values,
                    )? {
                        Some(false) => return Ok(Some(false)),
                        None => unknown = true,
                        Some(true) => {}
                    }
                }
                if unknown { None } else { Some(true) }
            }
            Self::Or(parts) => {
                let mut unknown = false;
                for part in parts {
                    match part.membership(
                        repo,
                        slot,
                        ctx,
                        evaluations,
                        relation_work,
                        target_values,
                    )? {
                        Some(true) => return Ok(Some(true)),
                        None => unknown = true,
                        Some(false) => {}
                    }
                }
                if unknown { None } else { Some(false) }
            }
            Self::Not(inner) => inner
                .membership(repo, slot, ctx, evaluations, relation_work, target_values)?
                .map(|value| !value),
        })
    }

    fn with_evidence(
        &self,
        repo: &Repository,
        slot: u32,
        ctx: &RecordContext<'_>,
        evidence: &mut HashSet<String>,
        evaluations: &mut u64,
        relation_work: &mut crate::relation_query::RelationWork,
        target_values: Option<&crate::relation_target_cache::TargetValueCache>,
    ) -> Result<Option<bool>, String> {
        Ok(match self {
            Self::Term(term, proof) => {
                let matched = match Self::proven(*proof, slot) {
                    Some(result) => Some(result),
                    None => {
                        *evaluations += 1;
                        if term.related.is_some() {
                            crate::relation_query::eval_related_term(
                                repo,
                                term,
                                ctx,
                                relation_work,
                                target_values,
                            )?
                        } else {
                            Some(eval_term(term, ctx)?)
                        }
                    }
                };
                if matched == Some(true) && self.can_emit_evidence() {
                    evidence.insert(term.query_path.display.clone());
                    if let Some(rhs) = &term.rhs {
                        evidence.insert(rhs.query_path.display.clone());
                    }
                    if let Some(related) = &term.related {
                        evidence.insert(related.evidence_path.clone());
                    }
                }
                matched
            }
            Self::And(parts) => {
                let mut branch = HashSet::new();
                let mut unknown = false;
                for part in parts {
                    match part.with_evidence(
                        repo,
                        slot,
                        ctx,
                        &mut branch,
                        evaluations,
                        relation_work,
                        target_values,
                    )? {
                        Some(false) => return Ok(Some(false)),
                        None => unknown = true,
                        Some(true) => {}
                    }
                }
                if unknown {
                    None
                } else {
                    evidence.extend(branch);
                    Some(true)
                }
            }
            Self::Or(parts) => {
                let mut matched = false;
                let mut unknown = false;
                for part in parts {
                    if matched && !part.can_emit_evidence() {
                        continue;
                    }
                    let mut branch = HashSet::new();
                    match part.with_evidence(
                        repo,
                        slot,
                        ctx,
                        &mut branch,
                        evaluations,
                        relation_work,
                        target_values,
                    )? {
                        Some(true) => {
                            matched = true;
                            evidence.extend(branch);
                        }
                        None => unknown = true,
                        Some(false) => {}
                    }
                }
                if matched {
                    Some(true)
                } else if unknown {
                    None
                } else {
                    Some(false)
                }
            }
            Self::Not(inner) => inner
                .membership(repo, slot, ctx, evaluations, relation_work, target_values)?
                .map(|value| !value),
        })
    }
}


fn scan_file(
    repo: &Repository,
    query: &Query,
    sort: &ResolvedSort<'_>,
    collection_index: usize,
    residual: Option<&Residual<'_>>,
    fts: Option<&BoundFts>,
    fts_proof: Option<&HashMap<u32, HashSet<String>>>,
    direct_plan: Option<&DirectSourcePlan<'_>>,
    target_values: Option<&crate::relation_target_cache::TargetValueCache>,
    projection: &ScanProjection,
    retain_all_evidence: bool,
    cache_budget: Option<&CacheBudget>,
    payload_cutoff: Option<&RankKey>,
    file_id: u32,
    ranges: &[(u32, ByteRange)],
    mut source_access: Option<&mut SourceAccessSession<'_>>,
    cancellation: Option<&CancellationToken>,
    max_retained_bytes: Option<u64>,
    collect_exact_members: bool,
    execution: Option<&ExecutionContext>,
) -> Result<ScannedFile, String> {
    if let Some(execution) = execution {
        execution.check_string()?;
    }
    let collection = &repo.collections[collection_index];
    let path = repo
        .files
        .native_path(crate::model::FileId(file_id))
        .ok_or("missing source path")?;
    let revision = repo.files.files[file_id as usize].revision;
    let mut outcome = ScannedFile::default();
    outcome.relation_work.attach(execution);
    let cap = query
        .options
        .limit
        .checked_add(query.options.offset)
        .ok_or("page size overflow")?;
    let keep_cap = if query.options.limit == 0 { 0 } else { cap };
    let mut winners = BinaryHeap::<FileWinner>::new();
    let mut retained_bytes = 0u64;
    let mut retained_key_bytes = 0u64;
    let mut buffered_bytes = 0usize;
    let mut reader = if source_access.is_some() {
        None
    } else {
        Some(RecordRangeReader::open_for_search(
            &path, revision, execution,
        )?)
    };
    outcome.files_opened = u64::from(reader.is_some());
    // These built-in readers search the converted logical strings. Keep the
    // value, but avoid producing a second owned copy of every string and path.
    let borrowed_value_text = borrowed_value_text(collection, fts);
    let share_fold = shared_fold_residual(residual);
    let full_projection = ScanProjection::full(projection.collect_text);
    let mut cursor = 0;
    while cursor < ranges.len() {
        if let Some(execution) = execution {
            execution.check_string()?;
            if ranges[cursor].1.len() > execution.limits.source_record_bytes {
                return Err(execution.fail(
                    ExecutionErrorKind::Resource,
                    "source record exceeds read allowance",
                ));
            }
        }
        if cancellation.is_some_and(CancellationToken::is_cancelled) {
            return Err("stream cancelled".into());
        }
        let mut end = cursor + 1;
        let mut batch_bytes = ranges[cursor].1.len();
        let batch_limit = max_retained_bytes.unwrap_or(1024 * 1024).min(1024 * 1024);
        while end < ranges.len()
            && end - cursor < 128
            && batch_bytes.saturating_add(ranges[end].1.len()) <= batch_limit
        {
            batch_bytes += ranges[end].1.len();
            end += 1;
        }
        let batch = &ranges[cursor..end];
        if let Some(execution) = execution
            && batch
                .iter()
                .any(|(_, range)| range.len() > execution.limits.source_record_bytes)
        {
            return Err(execution.fail(
                ExecutionErrorKind::Resource,
                "source record exceeds read allowance",
            ));
        }
        if max_retained_bytes
            .is_some_and(|limit| batch.iter().any(|(_, range)| range.len() > limit))
        {
            return Err(computed_sort_resource(
                execution,
                "stream resource: record exceeds buffered-byte limit",
            ));
        }
        cursor = end;
        let (records, bytes_read) = if let Some(session) = source_access.as_deref_mut() {
            let read = if let Some(limit) = max_retained_bytes {
                session.read_bounded(
                    crate::model::FileId(file_id),
                    batch,
                    usize::try_from(limit).unwrap_or(usize::MAX),
                )?
            } else {
                session.read(crate::model::FileId(file_id), batch)?
            };
            outcome.files_opened += read.opened;
            (read.records, read.bytes_read)
        } else {
            let reader = reader.as_mut().ok_or("missing source reader")?;
            if let Some(limit) = max_retained_bytes {
                reader.read_bounded(batch, usize::try_from(limit).unwrap_or(usize::MAX))?
            } else {
                reader.read(batch)?
            }
        };
        outcome.bytes_read += bytes_read;
        for (slot, bytes) in records {
            let _fold_record = FoldRecordGuard::new_with_sharing(share_fold);
            if let Some(execution) = execution {
                execution.check_string()?;
            }
            if cancellation.is_some_and(CancellationToken::is_cancelled) {
                return Err("stream cancelled".into());
            }
            let record = &collection.generation.records[slot as usize];
            let file = &repo.files.files[file_id as usize];
            let may_reach_page = !sort.bound.has_computed
                && keep_cap > 0
                && (winners.len() < keep_cap
                    || winners.peek().is_some_and(|worst| {
                        sort.compare_to_rank(
                            collection_index,
                            record,
                            &collection.definition.name,
                            &worst.key,
                        )
                        .is_lt()
                    }))
                && payload_cutoff.is_none_or(|worst| {
                    sort.compare_to_rank(
                        collection_index,
                        record,
                        &collection.definition.name,
                        worst,
                    )
                    .is_lt()
                });
            let logical_projection =
                query.options.return_mode == ReturnMode::Logical && may_reach_page;
            let direct = direct_plan.filter(|_| {
                record.class == RecordClass::Logical
                    && query.options.return_mode != ReturnMode::Logical
            });
            let record_projection = if logical_projection {
                &full_projection
            } else {
                projection
            };
            let decode = direct.is_none()
                && record.class == RecordClass::Logical
                && (logical_projection
                    || record_projection.collect_text
                    || record_projection
                        .top_level
                        .as_ref()
                        .is_none_or(|fields| !fields.is_empty()));
            let combined_fts = decode
                .then_some(())
                .filter(|_| collection.source.id() == "json_decoded_scalars")
                .and_then(|_| fts.filter(|fts| source_fts_eligible(collection, fts, true)));
            let (extracted, combined_result) = if let Some(combined_fts) = combined_fts {
                let (extracted, result) = prepare_decoded_record_with_borrowed_fts(
                    &collection.definition.schema,
                    &bytes,
                    record_projection,
                    combined_fts,
                    query.options.evidence,
                    execution,
                )?;
                outcome.record_parses += 1;
                (extracted, Some(result))
            } else if decode {
                let extracted = prepare_query_record_with_execution(
                    record.class,
                    &collection.source,
                    &collection.definition.schema,
                    &bytes,
                    record_projection,
                    logical_projection,
                    execution,
                )?;
                outcome.record_parses += 1;
                (extracted, None)
            } else {
                (
                    ExtractedRecord {
                        value: Value::Object(Default::default()),
                        text: Vec::new(),
                    },
                    None,
                )
            };
            outcome.scanned_records += 1;
            let ctx = RecordContext {
                value: &extracted.value,
                source_bytes: Some(&bytes),
                record,
                file,
                collection_name: &collection.definition.name,
                repository_root: &repo.files.root,
            };
            let mut evidence = HashSet::new();
            let direct_result = if let Some(direct) = direct {
                outcome.record_parses += 1;
                let filter = direct.filter.and_then(|(term, proof)| {
                    Residual::proven(proof, slot).is_none().then_some(term)
                });
                Some(scan_borrowed_segments_worker(
                    &bytes,
                    &BorrowedScanQuery {
                        fts: direct.fts,
                        filter,
                        decoded_scalars: direct.decoded_scalars,
                        retain_evidence: query.options.evidence,
                    },
                )?)
            } else {
                None
            };
            let fts_match = if let Some(result) = &direct_result {
                if query.options.evidence {
                    evidence.extend(result.evidence.iter().cloned());
                }
                result.fts_match
            } else if let Some(result) = &combined_result {
                if query.options.evidence {
                    evidence.extend(result.evidence.iter().cloned());
                }
                result.fts_match
            } else if let Some(proof) = fts_proof {
                if let Some(fields) = proof.get(&slot) {
                    if query.options.evidence {
                        evidence.extend(fields.iter().cloned());
                    }
                    true
                } else {
                    false
                }
            } else if record.class == RecordClass::RawFallback {
                if let Some(fts) = fts {
                    if fts.fields.is_empty() {
                        outcome.raw_text_verifications += 1;
                        let matched = crate::reference::raw_fallback_contains(
                            fts,
                            &String::from_utf8_lossy(&bytes),
                        );
                        if matched && query.options.evidence {
                            evidence.insert("@record.raw_text".into());
                        }
                        matched
                    } else {
                        if query.options.evidence {
                            crate::relation_query::eval_fts_with_relations(
                                repo,
                                fts,
                                &[],
                                &ctx,
                                &mut evidence,
                                crate::relation_query::EvaluationMode::Indexed,
                                &mut outcome.relation_work,
                            )?
                        } else {
                            crate::relation_query::eval_fts_with_relations_membership(
                                repo,
                                fts,
                                &[],
                                &ctx,
                                crate::relation_query::EvaluationMode::Indexed,
                                &mut outcome.relation_work,
                            )?
                        }
                    }
                } else {
                    true
                }
            } else if borrowed_value_text && fts.is_some() {
                crate::reference::eval_borrowed_value_fts(
                    fts.expect("checked above"),
                    &ctx,
                    query.options.evidence.then_some(&mut evidence),
                )?
            } else {
                fts.map(|fts| {
                    if query.options.evidence {
                        crate::relation_query::eval_fts_with_relations(
                            repo,
                            fts,
                            &extracted.text,
                            &ctx,
                            &mut evidence,
                            crate::relation_query::EvaluationMode::Indexed,
                            &mut outcome.relation_work,
                        )
                    } else {
                        crate::relation_query::eval_fts_with_relations_membership(
                            repo,
                            fts,
                            &extracted.text,
                            &ctx,
                            crate::relation_query::EvaluationMode::Indexed,
                            &mut outcome.relation_work,
                        )
                    }
                })
                .transpose()?
                .unwrap_or(true)
            };
            let filter_match = if fts_match && let Some(result) = &direct_result {
                let direct = direct.expect("direct result has a plan");
                let matched = if let Some((_, proof)) = direct.filter {
                    if let Some(proven) = Residual::proven(proof, slot) {
                        proven
                    } else {
                        outcome.residual_evaluations += 1;
                        result.filter_match
                    }
                } else {
                    true
                };
                if matched
                    && query.options.evidence
                    && ((retain_all_evidence && cache_budget.is_some_and(CacheBudget::complete))
                        || may_reach_page
                        || sort.bound.has_computed)
                    && let Some((term, _)) = direct.filter
                    && term.operator == Operator::Contains
                {
                    evidence.insert(term.query_path.display.clone());
                }
                matched
            } else if fts_match && record.class == RecordClass::RawFallback {
                residual
                    .map(|f| f.unavailable(&ctx, &mut evidence, &mut outcome.residual_evaluations))
                    .transpose()?
                    .flatten()
                    == Some(true)
                    || residual.is_none()
            } else if fts_match {
                residual
                    .map(|f| {
                        if query.options.evidence
                            && ((retain_all_evidence
                                && cache_budget.is_some_and(CacheBudget::complete))
                                || may_reach_page
                                || sort.bound.has_computed)
                        {
                            f.with_evidence(
                                repo,
                                slot,
                                &ctx,
                                &mut evidence,
                                &mut outcome.residual_evaluations,
                                &mut outcome.relation_work,
                                target_values,
                            )
                        } else {
                            f.membership(
                                repo,
                                slot,
                                &ctx,
                                &mut outcome.residual_evaluations,
                                &mut outcome.relation_work,
                                target_values,
                            )
                        }
                    })
                    .transpose()?
                    .unwrap_or(Some(true))
                    == Some(true)
            } else {
                false
            };
            if fts_match && filter_match {
                outcome.match_count += 1;
                if collect_exact_members {
                    outcome.exact_members.insert(slot);
                }
                if let Some(budget) = cache_budget {
                    let mut matched_fields = if query.options.evidence {
                        evidence.iter().cloned().collect::<Vec<_>>()
                    } else {
                        Vec::new()
                    };
                    matched_fields.sort();
                    if budget.reserve(&matched_fields) {
                        outcome.cache_members.push(CachedMember {
                            collection_index,
                            segment_index: None,
                            slot,
                            matched_fields,
                        });
                    }
                }
                let computed_key = if sort.bound.has_computed {
                    outcome.sort_key_evaluations += 1;
                    Some(rank_key_from_context(
                        sort,
                        repo,
                        collection_index,
                        &ctx,
                        target_values,
                    )?)
                } else {
                    None
                };
                let enters_page = if let Some(key) = &computed_key {
                    keep_cap > 0
                        && (winners.len() < keep_cap
                            || winners.peek().is_some_and(|worst| key < &worst.key))
                        && payload_cutoff.is_none_or(|worst| key < worst)
                } else {
                    may_reach_page
                };
                if enters_page {
                    let retained = if sort.bound.has_computed {
                        0
                    } else {
                        match query.options.return_mode {
                            ReturnMode::Locator => 0,
                            _ => bytes.len() as u64,
                        }
                    };
                    let replaced = if winners.len() == keep_cap {
                        winners
                            .peek()
                            .map_or(0, |worst| worst.matched.retained_bytes)
                    } else {
                        0
                    };
                    if max_retained_bytes.is_some_and(|limit| {
                        retained_bytes
                            .saturating_sub(replaced)
                            .saturating_add(retained)
                            > limit
                    }) {
                        return Err(computed_sort_resource(
                            execution,
                            "stream resource: file winners exceed buffered-byte limit",
                        ));
                    }
                    let payload = if sort.bound.has_computed {
                        None
                    } else if record.class == RecordClass::RawFallback
                        && query.options.return_mode == ReturnMode::Logical
                    {
                        Some(Payload::LogicalUnavailable {
                            diagnostic_ref: record.diagnostic_ref,
                        })
                    } else {
                        let payload_record = if direct.is_some()
                            && query.options.return_mode == ReturnMode::Logical
                        {
                            prepare_query_record_with_execution(
                                record.class,
                                &collection.source,
                                &collection.definition.schema,
                                &bytes,
                                &ScanProjection::full(false),
                                true,
                                execution,
                            )?
                        } else {
                            extracted
                        };
                        materialize(&query.options.return_mode, bytes, Some(payload_record))?
                    };
                    let key = computed_key
                        .unwrap_or_else(|| rank_key(sort, repo, collection_index, record));
                    let key_bytes = if sort.bound.has_computed || max_retained_bytes.is_some() {
                        rank_key_bytes(&key)
                    } else {
                        0
                    };
                    let measured = if sort.bound.has_computed || max_retained_bytes.is_some() {
                        std::mem::size_of::<FileWinner>()
                            .saturating_add(key_bytes as usize)
                            .saturating_add(
                                evidence.iter().map(|field| field.len() + 64).sum::<usize>(),
                            )
                            .saturating_add(
                                payload.as_ref().map_or(0, crate::streaming::payload_bytes),
                            )
                    } else {
                        0
                    };
                    let replaced_buffered = if (sort.bound.has_computed
                        || max_retained_bytes.is_some())
                        && winners.len() == keep_cap
                    {
                        winners
                            .peek()
                            .map_or(0, |worst| worst.matched.buffered_bytes)
                    } else {
                        0
                    };
                    if max_retained_bytes.is_some_and(|limit| {
                        buffered_bytes
                            .saturating_sub(replaced_buffered)
                            .saturating_add(measured)
                            > limit as usize
                    }) {
                        return Err(if sort.bound.has_computed && cancellation.is_none() {
                            computed_sort_resource(
                                execution,
                                "computed sort memory budget exceeded",
                            )
                        } else {
                            computed_sort_resource(
                                execution,
                                "stream resource: file winners exceed buffered-byte limit",
                            )
                        });
                    }
                    if winners.len() == keep_cap {
                        let removed = winners.pop().unwrap();
                        retained_bytes -= removed.matched.retained_bytes;
                        if sort.bound.has_computed {
                            retained_key_bytes -= rank_key_bytes(&removed.key);
                        }
                        buffered_bytes -= removed.matched.buffered_bytes;
                    }
                    retained_bytes += retained;
                    buffered_bytes += measured;
                    outcome.peak_payload_bytes = outcome.peak_payload_bytes.max(retained_bytes);
                    if sort.bound.has_computed {
                        retained_key_bytes = retained_key_bytes
                            .checked_add(key_bytes)
                            .ok_or("sort key byte count overflow")?;
                        if retained_key_bytes > MAX_SORT_KEY_BYTES {
                            return Err(computed_sort_resource(
                                execution,
                                "sort key memory budget exceeded",
                            ));
                        }
                        check_execution_sort_budget(
                            execution,
                            retained_bytes,
                            retained_key_bytes,
                            0,
                        )?;
                        outcome.peak_sort_key_bytes =
                            outcome.peak_sort_key_bytes.max(retained_key_bytes);
                    }
                    winners.push(FileWinner {
                        key,
                        matched: ScannedMatch {
                            slot,
                            key: None,
                            evidence,
                            payload,
                            retained_bytes: retained,
                            buffered_bytes: measured,
                        },
                    });
                }
            }
        }
    }
    outcome.matches = winners
        .into_sorted_vec()
        .into_iter()
        .map(|mut winner| {
            winner.matched.key = Some(winner.key);
            winner.matched
        })
        .collect();
    Ok(outcome)
}


/// One resolved relation hop shown only in diagnostic EXPLAIN output.
#[derive(Clone, Debug, Serialize)]
pub struct RelationHopDiagnostic {
    pub relation: crate::relation::RelationId,
    pub direction: String,
    pub from_collection: String,
    pub to_collection: String,
    pub selectors: Vec<Option<u32>>,
}


/// Exact predicate semantics shown outside the primary timed search path.
#[derive(Clone, Debug, Serialize)]
pub struct BoundTermDiagnostic {
    pub field: String,
    pub operator: Operator,
    pub quantifier: Quantifier,
    pub bound_value: Option<Value>,
    pub rhs_field: Option<String>,
    pub resolved_period: Option<(Value, Value)>,
    pub relation_hops: Vec<RelationHopDiagnostic>,
}


fn bound_term_diagnostics(
    repo: &Repository,
    filter: Option<&BoundExpr>,
) -> Vec<BoundTermDiagnostic> {
    fn visit(repo: &Repository, expr: &BoundExpr, output: &mut Vec<BoundTermDiagnostic>) {
        match expr {
            BoundExpr::Term(term) => {
                let related = term.related.as_deref();
                let exact = related.map_or(term, |path| path.terminal.as_ref());
                let rhs_field = related
                    .and_then(|path| path.source_operand.as_ref())
                    .map(|field| field.query_path.display.clone())
                    .or_else(|| {
                        term.rhs
                            .as_ref()
                            .map(|field| field.query_path.display.clone())
                    });
                output.push(BoundTermDiagnostic {
                    field: term.query_path.display.clone(),
                    operator: term.operator,
                    quantifier: term.quantifier,
                    bound_value: rhs_field.is_none().then(|| exact.literal.clone()),
                    rhs_field,
                    resolved_period: exact
                        .period
                        .as_ref()
                        .map(|period| (period.start.clone(), period.end.clone())),
                    relation_hops: related.map_or_else(Vec::new, |path| {
                        path.hops
                            .iter()
                            .map(|hop| RelationHopDiagnostic {
                                relation: hop.relation,
                                direction: format!("{:?}", hop.direction),
                                from_collection: repo.collections[hop.from.0 as usize]
                                    .definition
                                    .name
                                    .clone(),
                                to_collection: repo.collections[hop.to.0 as usize]
                                    .definition
                                    .name
                                    .clone(),
                                selectors: hop.selectors.clone(),
                            })
                            .collect()
                    }),
                });
            }
            BoundExpr::And(parts) | BoundExpr::Or(parts) => {
                for part in parts {
                    visit(repo, part, output);
                }
            }
            BoundExpr::Not(inner) => visit(repo, inner, output),
        }
    }
    let mut terms = Vec::new();
    if let Some(filter) = filter {
        visit(repo, filter, &mut terms);
    }
    terms
}


/// Explain output for a collection plan and its actual work.
#[derive(Clone, Debug, Serialize)]
pub struct QueryPlanTrace {
    pub collection: String,
    pub generation: u64,
    pub bigram_alphabet: BigramAlphabet,
    pub folded_representation: &'static str,
    pub fts_case_sensitivity: Option<CaseSensitivity>,
    pub fts_mode: Option<FtsMode>,
    pub fts_exact_literal: Option<String>,
    pub fts_prefilter_reason: Option<String>,
    pub fts_covered_target: String,
    pub fts_estimated_candidates: Option<u64>,
    pub remaining_verification: bool,
    pub input_records: u64,
    pub fts_strategy: String,
    pub normalized_sort: Vec<SortField>,
    pub sort_strategy: String,
    pub sort_strategy_estimated: bool,
    pub filter_exact: bool,
    pub terms: Vec<TermChoice>,
    pub planning: PlanningCounters,
    pub bound_terms: Vec<BoundTermDiagnostic>,
    pub candidates: u64,
    /// True when `candidates` and the selected paths are estimates only.
    pub candidates_estimated: bool,
    pub actual: Option<PlanActual>,
}


fn fts_covered_target(fts: Option<&BoundFts>) -> String {
    match fts {
        None => "none".into(),
        Some(fts) if fts.fields.is_empty() => {
            "raw collection text and malformed fallback text".into()
        }
        Some(fts) => fts
            .fields
            .iter()
            .map(|field| field.evidence_path.clone())
            .collect::<Vec<_>>()
            .join(", "),
    }
}

fn regex_prefilter_reason(
    collection: &crate::repository::CollectionState,
    fts: Option<&BoundFts>,
) -> Option<String> {
    let fts = fts?;
    if !matches!(fts.mode, FtsMode::Regex(_)) {
        return None;
    }
    if fts
        .fallback_regex
        .as_ref()
        .and_then(|regex| regex.condition())
        .is_none()
    {
        return Some("regex has no bounded necessary literal; scanner fallback".into());
    }
    if estimate_fts_candidates(collection, fts).is_none() {
        Some("regex has no usable grams or compatible index coverage; scanner fallback".into())
    } else {
        Some("regex uses necessary folded grams; exact verification remains".into())
    }
}


/// Observed source and predicate work for one plan.
#[derive(Clone, Debug, Serialize)]
pub struct PlanActual {
    pub files_opened: u64,
    pub bytes_read: u64,
    pub record_parses: u64,
    pub raw_text_verifications: u64,
    pub residual_evaluations: u64,
    pub index_proven_matches: u64,
    pub relation_target_candidates: u64,
    pub relation_target_matches: u64,
    pub relation_target_reads: u64,
    pub relation_target_file_opens: u64,
    pub relation_edge_visits: u64,
    pub relation_backlink_probe_visits: u64,
    pub relation_bytes_read: u64,
    pub relation_reverse_postings_visited: u64,
    pub relation_verified_targets_reused: u64,
}


impl Default for PlanActual {
    fn default() -> Self {
        Self {
            files_opened: 0,
            bytes_read: 0,
            record_parses: 0,
            raw_text_verifications: 0,
            residual_evaluations: 0,
            index_proven_matches: 0,
            relation_target_candidates: 0,
            relation_target_matches: 0,
            relation_target_reads: 0,
            relation_target_file_opens: 0,
            relation_edge_visits: 0,
            relation_backlink_probe_visits: 0,
            relation_bytes_read: 0,
            relation_reverse_postings_visited: 0,
            relation_verified_targets_reused: 0,
        }
    }
}


fn finish_trace(traces: &mut [QueryPlanTrace], before: &WorkCounters, after: &WorkCounters) {
    if let Some(trace) = traces.last_mut() {
        trace.actual = Some(PlanActual {
            files_opened: after.files_opened - before.files_opened,
            bytes_read: after.bytes_read - before.bytes_read,
            record_parses: after.record_parses - before.record_parses,
            raw_text_verifications: after.raw_text_verifications - before.raw_text_verifications,
            residual_evaluations: after.residual_evaluations - before.residual_evaluations,
            index_proven_matches: after.exact_posting_matches - before.exact_posting_matches,
            relation_target_candidates: after.relation_target_candidates
                - before.relation_target_candidates,
            relation_target_matches: after.relation_target_matches - before.relation_target_matches,
            relation_target_reads: after.relation_target_reads - before.relation_target_reads,
            relation_target_file_opens: after.relation_target_file_opens
                - before.relation_target_file_opens,
            relation_edge_visits: after.relation_edge_visits - before.relation_edge_visits,
            relation_backlink_probe_visits: after.relation_backlink_probe_visits
                - before.relation_backlink_probe_visits,
            relation_bytes_read: after.relation_bytes_read - before.relation_bytes_read,
            relation_reverse_postings_visited: after.relation_reverse_postings_visited
                - before.relation_reverse_postings_visited,
            relation_verified_targets_reused: after.relation_verified_targets_reused
                - before.relation_verified_targets_reused,
        });
    }
}

fn proof_evidence(expr: &BoundExpr, slot: u32, proofs: &[Proof]) -> Option<HashSet<String>> {
    match expr {
        BoundExpr::Term(term) => {
            let proof = proofs
                .iter()
                .find(|proof| same_bound_term(&proof.term, term) && proof.input.contains(slot))?;
            if !proof.matches.contains(slot) {
                return None;
            }
            let mut evidence = HashSet::new();
            if term.derived == Derived::None
                && !matches!(term.operator, Operator::Ne | Operator::NotContains)
            {
                evidence.insert(term.query_path.display.clone());
                if let Some(rhs) = &term.rhs {
                    evidence.insert(rhs.query_path.display.clone());
                }
                if let Some(related) = &term.related {
                    evidence.insert(related.evidence_path.clone());
                }
            }
            Some(evidence)
        }
        BoundExpr::And(parts) => {
            let mut evidence = HashSet::new();
            for part in parts {
                evidence.extend(proof_evidence(part, slot, proofs)?);
            }
            Some(evidence)
        }
        BoundExpr::Or(parts) => {
            let mut evidence = HashSet::new();
            let mut matched = false;
            for part in parts {
                if let Some(branch) = proof_evidence(part, slot, proofs) {
                    matched = true;
                    evidence.extend(branch);
                }
            }
            matched.then_some(evidence)
        }
        BoundExpr::Not(inner) => proof_evidence(inner, slot, proofs)
            .is_none()
            .then_some(HashSet::new()),
    }
}

/// Fallback proofs establish truth from inventory metadata. Reconstruct their
/// evidence with the same three-valued evaluator as reference execution: an
/// unavailable logical predicate stays unavailable even under negation.
fn proven_evidence(
    repo: &Repository,
    collection: &crate::repository::CollectionState,
    record: &RecordEntry,
    expr: &BoundExpr,
    proofs: &[Proof],
) -> Result<HashSet<String>, String> {
    if record.class != RecordClass::RawFallback {
        return Ok(proof_evidence(expr, record.slot.0, proofs).unwrap_or_default());
    }
    let empty = Value::Object(Default::default());
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
    if eval_unavailable_filter(expr, &ctx, &mut evidence)? != Some(true) {
        return Err("index-proven fallback has non-true filter truth".into());
    }
    Ok(evidence)
}

fn inventory_fts_evidence(
    repo: &Repository,
    collection: &crate::repository::CollectionState,
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


impl PartialEq for Winner {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}


impl Eq for Winner {}


impl PartialOrd for Winner {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}


impl Ord for Winner {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key.cmp(&other.key)
    }
}


fn rank_key(
    sort: &ResolvedSort<'_>,
    repo: &Repository,
    collection_index: usize,
    record: &RecordEntry,
) -> RankKey {
    let collection = &repo.collections[collection_index];
    let parts = sort.columns[collection_index]
        .as_ref()
        .expect("selected collection")
        .iter()
        .map(|(column, descending)| {
            let key = column.keys[record.slot.0 as usize].clone();
            (key, *descending, column.options)
        })
        .collect();
    RankKey {
        parts,
        collection: collection.definition.name.clone(),
        id: record.id.0.clone(),
    }
}

fn rank_key_from_context(
    sort: &ResolvedSort<'_>,
    repo: &Repository,
    collection_index: usize,
    ctx: &RecordContext<'_>,
    target_values: Option<&crate::relation_target_cache::TargetValueCache>,
) -> Result<RankKey, String> {
    Ok(RankKey {
        parts: sort.bound.keys(
            repo,
            collection_index,
            ctx,
            &sort.target_cache,
            target_values,
        )?,
        collection: repo.collections[collection_index].definition.name.clone(),
        id: ctx.record.id.0.clone(),
    })
}

fn rank_key_without_source(
    sort: &ResolvedSort<'_>,
    repo: &Repository,
    collection_index: usize,
    record: &RecordEntry,
) -> Result<RankKey, String> {
    let empty = Value::Object(Default::default());
    let collection = &repo.collections[collection_index];
    let ctx = RecordContext {
        value: &empty,
        source_bytes: None,
        record,
        file: &repo.files.files[record.file.0 as usize],
        collection_name: &collection.definition.name,
        repository_root: &repo.files.root,
    };
    rank_key_from_context(sort, repo, collection_index, &ctx, None)
}

fn rank_key_bytes(key: &RankKey) -> u64 {
    let bytes = std::mem::size_of::<RankKey>()
        .saturating_add(key.parts.capacity().saturating_mul(std::mem::size_of::<(
            SortKey,
            bool,
            SortOptions,
        )>()))
        .saturating_add(key.collection.capacity())
        .saturating_add(key.id.capacity())
        .saturating_add(key.parts.iter().fold(0usize, |total, (part, _, _)| {
            total.saturating_add(crate::sort_binding::key_bytes(part))
        }));
    u64::try_from(bytes).unwrap_or(u64::MAX)
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
        record_id: record.id.0.clone(),
        generation: repo.collections[collection_index].generation.generation,
        file_id: file.id.0,
        file_path: file.relative_path.clone(),
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


pub(crate) fn segment_record_ref<'a>(
    segment: &'a IndexedCollectionSegment,
    active: &ActiveRecords,
    catalog: &'a FileCatalog,
    slot: RecordSlot,
) -> Result<Option<SegmentRecordRef<'a>>, String> {
    if active.slot_count() as usize != segment.records.len() {
        return Err("segment active domain length mismatch".into());
    }
    if !active.contains(slot) {
        return Ok(None);
    }
    let record = segment.record(slot).ok_or("segment record slot missing")?;
    let segment_file = &segment
        .file(record.file)
        .ok_or("segment record file missing")?
        .entry;
    let file = catalog
        .entry(record.file)
        .ok_or("active segment record file is retired")?;
    // The catalog owns the path in this publication. A compatible move may
    // route an unchanged segment to a new path without rebuilding postings.
    if file.revision != segment_file.revision || !file.collections.contains(&segment.collection) {
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
    let Some(slot) = segment.id_lookup.get(&RecordId(record_id.to_owned())) else {
        return Ok(None);
    };
    let Some(resolved) = segment_record_ref(segment, active, catalog, *slot)? else {
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
        record_id: record.id.0.clone(),
        generation: revision.0,
        file_id: file.id.0,
        file_path: file.relative_path.clone(),
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
    let id = RecordId(record_id.to_owned());
    let base = collection
        .id_lookup
        .get(&id)
        .copied()
        .filter(|slot| base_active[collection_index].contains(*slot));
    let mut selected_segment = None;
    for (index, segment) in segments[collection_index].iter().enumerate() {
        if let Some(slot) = segment.index.id_lookup.get(&id)
            && segment.active.contains(*slot)
        {
            if base.is_some() || selected_segment.is_some() {
                return Err("duplicate active direct-lookup ID".into());
            }
            selected_segment = Some((index, *slot));
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
                record_id: record.id.0.clone(),
                generation: revision.0,
                file_id: file.id.0,
                file_path: file.relative_path.clone(),
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
            hit.generation = revision.0;
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
                    file_path: file.relative_path.clone(),
                    kind,
                    message,
                }))
            } else {
                Err(message)
            }
        }
    }
}


struct MergedPageHit {
    key: RankKey,
    hit: SearchHit,
    segment: Option<(usize, usize)>,
    retained_bytes: usize,
}


/// Optional complete ordering captured during an already exhaustive lifecycle
/// query. Once its budget is exhausted, query execution continues without a
/// cache entry; a prefix is never published as complete membership.
struct LifecycleCacheBuilder {
    ranked: Option<Vec<(RankKey, CachedMember)>>,
    bytes: usize,
    budget: usize,
}


/// One position in a stored ordering. Only the leading entry of each
/// partition is retained in the heap, so traversal memory scales with the
/// number of partitions and the requested output batch.
struct LifecycleOrderCursor {
    key: RankKey,
    bytes: usize,
    collection_index: usize,
    segment_index: usize,
    position: usize,
    slot: RecordSlot,
}


impl PartialEq for LifecycleOrderCursor {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}


impl Eq for LifecycleOrderCursor {}


impl PartialOrd for LifecycleOrderCursor {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}


impl Ord for LifecycleOrderCursor {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key.cmp(&other.key)
    }
}


impl LifecycleCacheBuilder {
    fn new(budget: usize) -> Self {
        Self {
            ranked: Some(Vec::new()),
            bytes: 0,
            budget,
        }
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


impl PartialEq for MergedPageHit {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}


impl Eq for MergedPageHit {}


impl PartialOrd for MergedPageHit {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}


impl Ord for MergedPageHit {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key.cmp(&other.key)
    }
}


fn offer_merged(
    winners: &mut BinaryHeap<MergedPageHit>,
    cap: usize,
    mut candidate: MergedPageHit,
    budget: Option<(usize, &ExecutionContext)>,
    used_bytes: &mut usize,
) -> Result<(), String> {
    if cap == 0 {
        return Ok(());
    }
    let replace = if winners.len() < cap {
        None
    } else if let Some(worst) = winners.peek()
        && candidate.key < worst.key
    {
        Some(worst.retained_bytes)
    } else {
        return Ok(());
    };
    if let Some((limit, context)) = budget {
        candidate.retained_bytes = std::mem::size_of::<MergedPageHit>()
            .saturating_add(usize::try_from(rank_key_bytes(&candidate.key)).unwrap_or(usize::MAX))
            .saturating_add(crate::streaming::hit_bytes(&candidate.hit));
        let next = used_bytes
            .checked_sub(replace.unwrap_or(0))
            .and_then(|bytes| bytes.checked_add(candidate.retained_bytes))
            .ok_or_else(|| {
                context.fail(
                    ExecutionErrorKind::Resource,
                    "ordered result memory overflow",
                )
            })?;
        if next > limit {
            return Err(context.fail(
                ExecutionErrorKind::Resource,
                "ordered result exceeds stream memory budget",
            ));
        }
        *used_bytes = next;
    }
    if replace.is_none() {
        winners.push(candidate);
    } else {
        winners.pop();
        winners.push(candidate);
    }
    Ok(())
}

fn segment_rank_key(
    repo: &Repository,
    sort: &ResolvedSort<'_>,
    collection_index: usize,
    segment: &IndexedCollectionSegment,
    record: &RecordEntry,
    ctx: Option<&RecordContext<'_>>,
) -> Result<RankKey, String> {
    let terms = sort.bound.terms[collection_index]
        .as_ref()
        .ok_or("segment sort collection missing")?;
    let mut parts = Vec::with_capacity(terms.len());
    for term in terms {
        let key = match &term.source {
            BoundSortSource::Stored(base_column) => {
                let column = segment.sorting.column(&base_column.path.display())?;
                column
                    .keys
                    .get(record.slot.0 as usize)
                    .ok_or("segment sort slot missing")?
                    .clone()
            }
            BoundSortSource::Computed(computed) => {
                computed.key_local(ctx.ok_or("computed segment sort needs record context")?)?
            }
        };
        parts.push((key, term.descending, term.options));
    }
    Ok(RankKey {
        parts,
        collection: repo.collections[collection_index].definition.name.clone(),
        id: record.id.0.clone(),
    })
}

fn base_rank_key_merged(
    repo: &Repository,
    sort: &ResolvedSort<'_>,
    collection_index: usize,
    record: &RecordEntry,
    catalog: &FileCatalog,
    context: Option<&ExecutionContext>,
) -> Result<RankKey, String> {
    if !sort.bound.has_computed {
        return Ok(rank_key(sort, repo, collection_index, record));
    }
    let collection = &repo.collections[collection_index];
    let file = catalog.entry(record.file).ok_or("base sort file retired")?;
    let bytes = if sort.bound.needs_source {
        if let Some(context) = context {
            context.check_string()?;
            if record.range.len() > context.limits.source_record_bytes {
                return Err(context.fail(
                    ExecutionErrorKind::Resource,
                    "base sort record exceeds source read allowance",
                ));
            }
            context.charge_reads(1)?;
            context.charge_bytes(record.range.len())?;
        }
        let path = repo.files.root.join(&file.relative_path);
        Some(if let Some(context) = context {
            read_record_range_for_search(&path, file.revision, record.range, Some(context))?
        } else {
            read_record_range_for_search(&path, file.revision, record.range, None)?
        })
    } else {
        None
    };
    let extracted = if let Some(bytes) = bytes.as_ref() {
        prepare_query_record_with_execution(
            record.class,
            &collection.source,
            &collection.definition.schema,
            bytes,
            &ScanProjection::full(false),
            false,
            context,
        )?
    } else {
        ExtractedRecord {
            value: Value::Object(Default::default()),
            text: Vec::new(),
        }
    };
    let ctx = RecordContext {
        value: &extracted.value,
        source_bytes: bytes.as_deref(),
        record,
        file,
        collection_name: &collection.definition.name,
        repository_root: &repo.files.root,
    };
    rank_key_from_context(sort, repo, collection_index, &ctx, None)
}

fn relation_rank_key_merged(
    repo: &Repository,
    sort: &ResolvedSort<'_>,
    view: &crate::relation_query::MergedRelationView<'_>,
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    address: crate::relation_query::MergedRecordSlot,
    ctx: &RecordContext<'_>,
    cache: &mut MergedSortTargetCache,
) -> Result<RankKey, String> {
    let collection_index = address.collection.0 as usize;
    let terms = sort.bound.terms[collection_index]
        .as_ref()
        .ok_or("merged sort collection missing")?;
    let mut parts = Vec::with_capacity(terms.len());
    for term in terms {
        let key = match &term.source {
            BoundSortSource::Stored(column) => {
                if let Some(segment_index) = address.segment {
                    let segment = &segments[collection_index][segment_index].index;
                    segment
                        .sorting
                        .column(&column.path.display())?
                        .keys
                        .get(address.slot.0 as usize)
                        .ok_or("merged segment sort slot missing")?
                        .clone()
                } else {
                    column
                        .keys
                        .get(address.slot.0 as usize)
                        .ok_or("merged base sort slot missing")?
                        .clone()
                }
            }
            BoundSortSource::Computed(computed) => {
                computed.key_merged(view, address, ctx, cache)?
            }
        };
        parts.push((key, term.descending, term.options));
    }
    Ok(RankKey {
        parts,
        collection: repo.collections[collection_index].definition.name.clone(),
        id: ctx.record.id.0.clone(),
    })
}

fn segment_locator_hit(
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
        record_id: record.id.0.clone(),
        generation: revision.0,
        file_id: file.id.0,
        file_path: file.relative_path.clone(),
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

fn record_unavailable_hit(
    repo: &Repository,
    catalog: &FileCatalog,
    hit: &SearchHit,
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
        file_path: file.relative_path.clone(),
        kind,
        message,
    });
    Ok(true)
}

fn bound_expr_has_relations(expr: &BoundExpr) -> bool {
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
    search_segmented_lifecycle(repo, query, base_active, segments, catalog, revision, &[], None)
}

pub(crate) fn search_segmented_lifecycle(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
    revision: IndexRevision,
    batches: &[Arc<Vec<Arc<ExtendedBatch>>>],
    cache: Option<LifecycleCacheScope<'_>>,
) -> Result<SearchResult, String> {
    match search_segmented_impl(
        repo,
        query,
        base_active,
        segments,
        catalog,
        revision,
        batches,
        None,
        cache,
        None,
    ) {
        Err(_error) if has_unavailable_active_source(repo, base_active, segments, catalog) => {
            search_segmented_reference(repo, query, base_active, segments, catalog, revision)
        }
        result => result,
    }
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
) -> Result<SearchResult, crate::streaming::ExecutionError> {
    search_segmented_with_context_lifecycle(
        repo,
        query,
        base_active,
        segments,
        catalog,
        revision,
        context,
        &[],
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
    batches: &[Arc<Vec<Arc<ExtendedBatch>>>],
    cache: Option<LifecycleCacheScope<'_>>,
) -> Result<SearchResult, crate::streaming::ExecutionError> {
    search_segmented_with_context_lifecycle_budget(
        repo,
        query,
        base_active,
        segments,
        catalog,
        revision,
        context,
        batches,
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
    batches: &[Arc<Vec<Arc<ExtendedBatch>>>],
    cache: Option<LifecycleCacheScope<'_>>,
    retained_budget: Option<usize>,
) -> Result<SearchResult, crate::streaming::ExecutionError> {
    context.check()?;
    let result = search_segmented_impl(
        repo,
        query,
        base_active,
        segments,
        catalog,
        revision,
        batches,
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
                let (prepared, _) = crate::prepared::prepare(repo, query)
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
    search_active_available_lifecycle(repo, query, base_active, catalog, revision, &[], None)
}

pub(crate) fn search_active_available_lifecycle(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    catalog: &FileCatalog,
    revision: IndexRevision,
    batches: &[Arc<Vec<Arc<ExtendedBatch>>>],
    _cache: Option<LifecycleCacheScope<'_>>,
) -> Result<SearchResult, String> {
    let empty = vec![Arc::new(Vec::new()); repo.collections.len()];
    match search_active_cached(repo, query, base_active, catalog, revision, batches, None, _cache) {
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
    batches: &[Arc<Vec<Arc<ExtendedBatch>>>],
    _cache: Option<LifecycleCacheScope<'_>>,
) -> Result<SearchResult, crate::streaming::ExecutionError> {
    let empty = vec![Arc::new(Vec::new()); repo.collections.len()];
    let optimized = context.check().and_then(|()| {
        search_active_cached(repo, query, base_active, catalog, revision, batches, Some(context), _cache)
            .map_err(|message| context.recorded_error(message))
    });
    match optimized {
        Err(error)
            if matches!(
                error.kind,
                ExecutionErrorKind::SourceChanged | ExecutionErrorKind::Execution
            ) && has_unavailable_active_source(repo, base_active, &empty, catalog) =>
        {
            context.clear_recovered_source_changed();
            let sort = ResolvedSort::new(repo, query)
                .map_err(|message| context.recorded_error(message))?;
            let (prepared, _) = crate::prepared::prepare(repo, query)
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
            )
            .map_err(|message| context.recorded_error(message))
        }
        result => result,
    }
}

/// Reuse complete membership and order across capability-only publications.
/// The key uses the semantic data revision, so pass-2 acceleration cannot
/// invalidate a verified result. Retain a result only after an exhaustive
/// count and after every member fit within the configured cache budget.
#[allow(clippy::too_many_arguments)]
fn search_active_cached(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    catalog: &FileCatalog,
    revision: IndexRevision,
    batches: &[Arc<Vec<Arc<ExtendedBatch>>>],
    context: Option<&ExecutionContext>,
    cache: Option<LifecycleCacheScope<'_>>,
) -> Result<SearchResult, String> {
    let mut cache_key = None;
    if query.options.count
        && query.options.limit > 0
        && context.is_none_or(|context| context.limits == ExecutionLimits::default())
        && let Some(scope) = cache
    {
        let key = crate::verified::lifecycle_key(query, scope.instance_id, revision.0)?;
        let mut store = scope.store.lock().map_err(|error| error.to_string())?;
        if store.enabled() {
            if let Some(cached) = store.get(&key) {
                drop(store);
                let empty = vec![Arc::new(Vec::new()); repo.collections.len()];
                return materialize_lifecycle_cached(
                    repo, query, &cached, base_active, &empty, catalog, revision, context,
                );
            }
            let budget = store.budget().saturating_sub(key.len().saturating_add(128));
            cache_key = Some((scope, key, budget));
        }
    }
    let mut members = Vec::new();
    let tracker = cache_key.as_ref().and_then(|(_, _, budget)| {
        (*budget > 0).then(|| CacheBudget::new(*budget))
    });
    let result = search_active_inner(
        repo,
        query,
        base_active,
        context,
        Some(batches),
        tracker.as_ref().map(|_| &mut members),
        tracker.as_ref(),
    )?;
    if result.counters.match_count_complete
        && result.count == Some(members.len())
        && tracker.as_ref().is_some_and(CacheBudget::complete)
        && let Some((scope, key, _)) = cache_key
    {
        if let Some(context) = context {
            context.check_string()?;
        }
        sort_cached_members(repo, query, &mut members);
        let displaced = scope
            .store
            .lock()
            .map_err(|error| error.to_string())?
            .insert(key, members);
        drop(displaced);
    }
    Ok(result)
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
            for file in &segment.index.files {
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
                                let range = segment.index.records[slot.0 as usize].range;
                                range.start > range.end || range.end > current.revision.size
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

/// Reconstruct the same positive-term evidence as an index proof for a
/// conjunction. Every candidate must be proven by each term; other expression
/// shapes retain source verification because they may expose different
/// evidence per record even when their combined membership is exact.
fn segment_exact_evidence_fields(
    segment: &IndexedCollectionSegment,
    active: &ActiveRecords,
    collection: &CollectionState,
    expr: &BoundExpr,
    candidates: &RoaringBitmap,
    catalog: &FileCatalog,
) -> Result<Option<Vec<String>>, String> {
    match expr {
        BoundExpr::Term(term) => {
            if term.related.is_some() || term.rhs.is_some() {
                return Ok(None);
            }
            let bounds = segment_filter_bounds_with_catalog(
                segment,
                active,
                expr,
                collection,
                Some(catalog),
            )?;
            if !candidates.is_subset(&bounds.must) {
                return Ok(None);
            }
            Ok(Some(
                if term.derived == Derived::None
                    && !matches!(term.operator, Operator::Ne | Operator::NotContains)
                {
                    vec![term.query_path.display.clone()]
                } else {
                    Vec::new()
                },
            ))
        }
        BoundExpr::And(parts) => {
            let mut fields = Vec::new();
            for part in parts {
                let Some(child) = segment_exact_evidence_fields(
                    segment, active, collection, part, candidates, catalog,
                )?
                else {
                    return Ok(None);
                };
                fields.extend(child);
            }
            fields.sort();
            fields.dedup();
            Ok(Some(fields))
        }
        _ => Ok(None),
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
    matches: u64,
    winners: Vec<MergedPageHit>,
}


/// Verify one local segment candidate with the narrow source or value consumer.
/// Relation and opaque binding requests stay on the established evaluator.
#[allow(clippy::too_many_arguments)]
fn verify_local_segment_record(
    repo: &Repository,
    collection: &CollectionState,
    record: &RecordEntry,
    file: &FileEntry,
    bytes: &[u8],
    filter: Option<&BoundExpr>,
    fts: Option<&BoundFts>,
    direct_plan: Option<&DirectSourcePlan<'_>>,
    retain_evidence: bool,
    execution: Option<&ExecutionContext>,
) -> Result<(ExtractedRecord, bool, HashSet<String>, bool), String> {
    let _fold_record = FoldRecordGuard::new_with_sharing(shared_fold_bound(filter));
    if record.class == RecordClass::Logical
        && let Some(direct) = direct_plan
    {
        let result = scan_borrowed_segments_worker(
            bytes,
            &BorrowedScanQuery {
                fts: direct.fts,
                filter: direct.filter.map(|(term, _)| term),
                decoded_scalars: direct.decoded_scalars,
                retain_evidence,
            },
        )?;
        let matched = result.filter_match && result.fts_match;
        let mut evidence = if matched && retain_evidence {
            result.evidence
        } else {
            HashSet::new()
        };
        if matched
            && retain_evidence
            && let Some((term, _)) = direct.filter
            && term.operator == Operator::Contains
        {
            evidence.insert(term.query_path.display.clone());
        }
        return Ok((
            ExtractedRecord {
                value: Value::Object(Default::default()),
                text: Vec::new(),
            },
            matched,
            evidence,
            false,
        ));
    }
    let borrowed_value_text = borrowed_value_text(collection, fts);
    let projection = ScanProjection::full(fts.is_some() && !borrowed_value_text);
    let combined_fts = fts.filter(|fts| {
        collection.source.id() == "json_decoded_scalars"
            && source_fts_eligible(collection, fts, true)
    });
    let (extracted, combined_result) = if record.class == RecordClass::Logical
        && let Some(combined_fts) = combined_fts
    {
        let (extracted, result) = prepare_decoded_record_with_borrowed_fts(
            &collection.definition.schema,
            bytes,
            &projection,
            combined_fts,
            retain_evidence,
            execution,
        )?;
        (extracted, Some(result))
    } else if record.class == RecordClass::Logical {
        (
            prepare_query_record_with_execution(
                record.class,
                &collection.source,
                &collection.definition.schema,
                bytes,
                &projection,
                false,
                execution,
            )?,
            None,
        )
    } else {
        (
            ExtractedRecord {
                value: Value::Object(Default::default()),
                text: Vec::new(),
            },
            None,
        )
    };
    let ctx = RecordContext {
        value: &extracted.value,
        source_bytes: Some(bytes),
        record,
        file,
        collection_name: &collection.definition.name,
        repository_root: &repo.files.root,
    };
    let mut evidence = HashSet::new();
    let filter_matches = if let Some(filter) = filter {
        if record.class == RecordClass::RawFallback {
            eval_unavailable_filter(filter, &ctx, &mut evidence)? == Some(true)
        } else {
            let mut work = crate::relation_query::RelationWork::default();
            crate::relation_query::eval_filter_truth(
                repo,
                filter,
                &ctx,
                &mut evidence,
                crate::relation_query::EvaluationMode::Indexed,
                &mut work,
            )? == Some(true)
        }
    } else {
        true
    };
    if !filter_matches {
        return Ok((extracted, false, evidence, false));
    }
    let raw_checked =
        record.class == RecordClass::RawFallback && fts.is_some_and(|fts| fts.fields.is_empty());
    let fts_matches = if let Some(fts) = fts {
        if record.class == RecordClass::RawFallback && fts.fields.is_empty() {
            let matched =
                crate::reference::raw_fallback_contains(fts, &String::from_utf8_lossy(bytes));
            if matched && retain_evidence {
                evidence.insert("@record.raw_text".into());
            }
            matched
        } else if let Some(result) = &combined_result {
            if retain_evidence {
                evidence.extend(result.evidence.iter().cloned());
            }
            result.fts_match
        } else if borrowed_value_text && record.class == RecordClass::Logical {
            crate::reference::eval_borrowed_value_fts(
                fts,
                &ctx,
                retain_evidence.then_some(&mut evidence),
            )?
        } else {
            eval_bound_fts(fts, &extracted.text, &ctx, &mut evidence)?
        }
    } else {
        true
    };
    Ok((extracted, fts_matches, evidence, raw_checked))
}

fn scan_segment_file_parallel(
    repo: &Repository,
    query: &Query,
    sort: &ResolvedSort<'_>,
    collection_index: usize,
    segment_index: usize,
    published: &PublishedSegment,
    catalog: &FileCatalog,
    revision: IndexRevision,
    filter: Option<&BoundExpr>,
    fts: Option<&BoundFts>,
    direct_plan: Option<&DirectSourcePlan<'_>>,
    file_id: u32,
    ranges: &[(u32, ByteRange)],
    cap: usize,
) -> Result<SegmentFileScan, String> {
    let collection = &repo.collections[collection_index];
    let segment = &published.index;
    let file = catalog
        .entry(crate::model::FileId(file_id))
        .ok_or("active segment candidate file is retired")?;
    let path = repo.files.root.join(&file.relative_path);
    let mut reader = RecordRangeReader::open_for_search(&path, file.revision, None)?;
    let mut result = SegmentFileScan {
        bytes_read: 0,
        scanned_records: 0,
        record_parses: 0,
        residual_evaluations: 0,
        raw_text_verifications: 0,
        matches: 0,
        winners: Vec::new(),
    };
    let mut local_winners = BinaryHeap::new();
    let mut local_winner_bytes = 0;
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
        result.bytes_read += bytes_read;
        for (slot, bytes) in read {
            let resolved =
                segment_record_ref(segment, &published.active, catalog, RecordSlot(slot))?
                    .ok_or("active segment scan slot missing")?;
            let record = resolved.record;
            result.record_parses += u64::from(record.class == RecordClass::Logical);
            result.residual_evaluations += u64::from(filter.is_some());
            let (extracted, matched, evidence, raw_checked) = verify_local_segment_record(
                repo,
                collection,
                record,
                resolved.file,
                &bytes,
                filter,
                fts,
                direct_plan,
                query.options.evidence,
                None,
            )?;
            result.raw_text_verifications += u64::from(raw_checked);
            result.scanned_records += 1;
            if !matched {
                continue;
            }
            let ctx = RecordContext {
                value: &extracted.value,
                source_bytes: Some(&bytes),
                record,
                file: resolved.file,
                collection_name: &collection.definition.name,
                repository_root: &repo.files.root,
            };
            result.matches += 1;
            let key = segment_rank_key(repo, sort, collection_index, segment, record, Some(&ctx))?;
            let hit = segment_locator_hit(
                collection,
                resolved,
                revision,
                if query.options.evidence {
                    evidence.into_iter().collect()
                } else {
                    Vec::new()
                },
            );
            offer_merged(
                &mut local_winners,
                cap,
                MergedPageHit {
                    key,
                    hit,
                    segment: Some((collection_index, segment_index)),
                    retained_bytes: 0,
                },
                None,
                &mut local_winner_bytes,
            )?;
        }
    }
    result.winners = local_winners.into_sorted_vec();
    Ok(result)
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
            let Some(next) = retained_bytes.checked_add(crate::streaming::hit_bytes(&hit)) else {
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
    prepared: &[(usize, Option<BoundExpr>, Option<BoundFts>)],
    counters: &WorkCounters,
) -> Result<Option<SearchResult>, String> {
    if query.options.return_mode != ReturnMode::Locator
        || query.options.sort.is_empty()
        || sort.bound.has_computed
        || prepared.len() != 1
        || base_active.iter().any(|active| active.len() != 0)
    {
        return Ok(None);
    }
    let (collection_index, filter, fts) = &prepared[0];
    if fts.is_some() {
        return Ok(None);
    }
    let collection = &repo.collections[*collection_index];
    if segments[*collection_index].len() > 4096 {
        return Ok(None);
    }
    let mut selected = Vec::with_capacity(segments[*collection_index].len());
    let mut counters = counters.clone();
    for published in segments[*collection_index].iter() {
        if lifecycle_segment_order(&published.index, sort, query, *collection_index).is_none() {
            return Ok(None);
        }
        let bounds = filter
            .as_ref()
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
        let candidates = bounds
            .as_ref()
            .map_or_else(|| published.active.bitmap(), |bounds| bounds.may.clone());
        if bounds
            .as_ref()
            .is_some_and(|bounds| bounds.may != bounds.must)
        {
            return Ok(None);
        }
        let evidence = if query.options.evidence {
            if matches!(filter, Some(BoundExpr::And(_))) && candidates.len() <= 4 {
                return Ok(None);
            }
            if let Some(filter) = filter {
                let Some(fields) = segment_exact_evidence_fields(
                    &published.index,
                    &published.active,
                    collection,
                    filter,
                    &candidates,
                    catalog,
                )?
                else {
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
        *collection_index,
        &selected,
        counters,
    )
}

fn search_segmented_impl(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
    revision: IndexRevision,
    batches: &[Arc<Vec<Arc<ExtendedBatch>>>],
    context: Option<&ExecutionContext>,
    cache: Option<LifecycleCacheScope<'_>>,
    retained_budget: Option<usize>,
) -> Result<SearchResult, String> {
    if let Some(context) = context {
        context.check_string()?;
    }
    if segments.len() != repo.collections.len() || base_active.len() != repo.collections.len() {
        return Err("segment query collection count mismatch".into());
    }
    let cache_key = if retained_budget.is_none()
        && let Some(scope) = cache
        && context.is_none_or(|context| context.limits == ExecutionLimits::default())
    {
        let key = crate::verified::lifecycle_key(query, scope.instance_id, revision.0)?;
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
    let sort = ResolvedSort::new(repo, query)?;
    let related_sort = sort.bound.terms.iter().flatten().flatten().any(|term| {
        matches!(&term.source, BoundSortSource::Computed(computed) if computed.related.is_some())
    });
    let (prepared, _) = crate::prepared::prepare(repo, query)?;
    let has_relations = prepared.collections.iter().any(|(_, filter, fts)| {
        filter.as_ref().is_some_and(bound_expr_has_relations)
            || fts
                .as_ref()
                .is_some_and(|fts| fts.fields.iter().any(|field| field.related.is_some()))
    });
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
    let mut base_query = query.clone();
    base_query.options.offset = 0;
    base_query.options.limit = cap;
    // A page request must not exhaust the base merely to calculate a total
    // the caller did not request. The top `cap` base rows are sufficient for
    // the top `cap` rows of the merged base-and-segment result.
    base_query.options.count = query.options.count;
    let base = search_active_extended(repo, &base_query, base_active, batches, context)?;
    let mut total = base
        .count
        .map_or(base.counters.total_matches, |count| count as u64);
    let base_count_complete =
        base_active.iter().all(|active| active.len() == 0) || base.counters.match_count_complete;
    let mut counters = base.counters;
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
            .get(&RecordId(hit.record_id.clone()))
            .ok_or("base hit ID missing")?;
        let record = collection
            .generation
            .records
            .get(slot.0 as usize)
            .ok_or("base hit slot missing")?;
        let key = base_rank_key_merged(repo, &sort, collection_index, record, catalog, context)?;
        hit.generation = revision.0;
        offer_merged(
            &mut winners,
            cap,
            MergedPageHit {
                key,
                hit,
                segment: None,
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
    for (collection_index, filter, fts) in prepared.collections {
        let collection = &repo.collections[collection_index];
        let direct_plan =
            DirectSourcePlan::prepare_bound(collection, &sort, fts.as_ref(), filter.as_ref());
        for (segment_index, published) in segments[collection_index].iter().enumerate() {
            if let Some(context) = context {
                context.check_string()?;
            }
            if published.index.collection != collection.generation.collection {
                return Err("segment collection mismatch".into());
            }
            let segment = &published.index;
            let bounds = filter
                .as_ref()
                .map(|filter| {
                    segment_filter_bounds_with_catalog(
                        segment,
                        &published.active,
                        filter,
                        collection,
                        Some(catalog),
                    )
                })
                .transpose()?;
            let mut candidates = bounds
                .as_ref()
                .map_or_else(|| published.active.bitmap(), |bounds| bounds.may.clone());
            if let Some(fts_may) = fts
                .as_ref()
                .and_then(|fts| segment_fts_candidates(segment, collection, fts))
            {
                candidates &= fts_may;
            }
            counters.candidates += candidates.len();
            if candidates.is_empty() {
                continue;
            }
            let exact_evidence = if query.options.evidence
                && fts.is_none()
                && !(matches!(filter, Some(BoundExpr::And(_))) && candidates.len() <= 4)
            {
                if let Some(filter) = filter.as_ref() {
                    segment_exact_evidence_fields(
                        segment,
                        &published.active,
                        collection,
                        filter,
                        &candidates,
                        catalog,
                    )?
                } else {
                    Some(Vec::new())
                }
            } else {
                None
            };
            let index_exact = (!query.options.evidence || exact_evidence.is_some())
                && !sort.bound.has_computed
                && fts.is_none()
                && bounds
                    .as_ref()
                    .is_none_or(|bounds| bounds.may == bounds.must);
            if index_exact {
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
                            exact_evidence.clone().unwrap_or_default(),
                        );
                        offer_merged(
                            &mut winners,
                            cap,
                            MergedPageHit {
                                key,
                                hit,
                                segment: Some((collection_index, segment_index)),
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
                    let resolved =
                        segment_record_ref(segment, &published.active, catalog, RecordSlot(slot))?
                            .ok_or("active segment candidate missing")?;
                    total += 1;
                    counters.exact_posting_matches += 1;
                    let key = segment_rank_key(
                        repo,
                        &sort,
                        collection_index,
                        segment,
                        resolved.record,
                        None,
                    )?;
                    let matched_fields = exact_evidence.clone().unwrap_or_default();
                    let hit =
                        segment_locator_hit(collection, resolved, revision, matched_fields.clone());
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
                            retained_bytes: 0,
                        },
                        winner_budget,
                        &mut winner_bytes,
                    )?;
                }
                continue;
            }
            let mut files: BTreeMap<u32, Vec<(u32, ByteRange)>> = BTreeMap::new();
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
                files
                    .entry(record.file.0)
                    .or_default()
                    .push((slot, record.range));
            }
            // Local residual verification is independent by source file. Limit the
            // parallel route to small ordered pages and bounded source rows;
            // all other plans keep the existing serial, cache-aware path.
            let small_single_record_files = files
                .values()
                .all(|ranges| ranges.len() == 1 && ranges[0].1.len() <= 8 * 1024);
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
                && query
                    .filter
                    .as_ref()
                    .is_none_or(|filter| format!("{filter:?}").len() <= 1024)
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
                let jobs = files.into_iter().collect::<Vec<_>>();
                // Small single-record files retain at most one compact hit per
                // job. Dispatch them in larger bounded batches to avoid one
                // pool installation for every worker-sized group.
                let batch_size = if small_single_record_files {
                    256
                } else {
                    repo.options.query_workers.clamp(1, 32)
                };
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
                                    segment_index,
                                    published,
                                    catalog,
                                    revision,
                                    filter.as_ref(),
                                    fts.as_ref(),
                                    direct_plan.as_ref(),
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
                        delta_opened.insert(*file_id);
                        counters.files_opened += 1;
                        counters.bytes_read += scan.bytes_read;
                        counters.scanned_records += scan.scanned_records;
                        counters.record_parses += scan.record_parses;
                        counters.residual_evaluations += scan.residual_evaluations;
                        counters.raw_text_verifications += scan.raw_text_verifications;
                        total += scan.matches;
                        for winner in scan.winners {
                            offer_merged(
                                &mut winners,
                                cap,
                                winner,
                                winner_budget,
                                &mut winner_bytes,
                            )?;
                        }
                    }
                }
                continue;
            }
            for (file_id, ranges) in files {
                if let Some(context) = context {
                    context.check_string()?;
                    context.charge_reads(1)?;
                }
                let file = catalog
                    .entry(crate::model::FileId(file_id))
                    .ok_or("active segment candidate file is retired")?;
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
                        let resolved = segment_record_ref(
                            segment,
                            &published.active,
                            catalog,
                            RecordSlot(slot),
                        )?
                        .ok_or("active segment scan slot missing")?;
                        let record = resolved.record;
                        counters.record_parses += u64::from(record.class == RecordClass::Logical);
                        counters.residual_evaluations += u64::from(filter.is_some());
                        let (extracted, matched, evidence, raw_checked) =
                            verify_local_segment_record(
                                repo,
                                collection,
                                record,
                                resolved.file,
                                &bytes,
                                filter.as_ref(),
                                fts.as_ref(),
                                direct_plan.as_ref(),
                                query.options.evidence,
                                context,
                            )?;
                        counters.raw_text_verifications += u64::from(raw_checked);
                        counters.scanned_records += 1;
                        if !matched {
                            continue;
                        }
                        let ctx = RecordContext {
                            value: &extracted.value,
                            source_bytes: Some(&bytes),
                            record,
                            file: resolved.file,
                            collection_name: &collection.definition.name,
                            repository_root: &repo.files.root,
                        };
                        total += 1;
                        let key = segment_rank_key(
                            repo,
                            &sort,
                            collection_index,
                            segment,
                            record,
                            Some(&ctx),
                        )?;
                        let hit = segment_locator_hit(
                            collection,
                            resolved,
                            revision,
                            if query.options.evidence {
                                evidence.into_iter().collect()
                            } else {
                                Vec::new()
                            },
                        );
                        if let Some(builder) = cache_builder.as_mut() {
                            builder.push(
                                &key,
                                CachedMember {
                                    collection_index,
                                    segment_index: Some(segment_index),
                                    slot,
                                    matched_fields: hit.matched_fields.clone(),
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
                                retained_bytes: 0,
                            },
                            winner_budget,
                            &mut winner_bytes,
                        )?;
                    }
                }
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
        for mut winner in page {
            if let Some((collection_index, segment_index)) = winner.segment {
                if let Some(context) = context {
                    context.check_string()?;
                    context.charge_reads(1)?;
                    context
                        .charge_bytes(winner.hit.byte_end.saturating_sub(winner.hit.byte_start))?;
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
                            &winner.hit,
                            message.clone(),
                            &mut counters,
                        )? {
                            continue;
                        }
                        return Err(message);
                    }
                };
                materialized.matched_fields = winner.hit.matched_fields.clone();
                winner.hit = materialized;
            }
            retained.push(winner);
        }
        page = retained;
    }
    counters.total_matches = total;
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
fn materialize_lifecycle_cached(
    repo: &Repository,
    query: &Query,
    cached: &CachedResult,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
    revision: IndexRevision,
    context: Option<&ExecutionContext>,
) -> Result<SearchResult, String> {
    let mut counters = WorkCounters {
        total_matches: cached.members.len() as u64,
        match_count_complete: true,
        verified_result_hit: true,
        ..WorkCounters::default()
    };
    let mut hits = Vec::new();
    let mut files = HashSet::new();
    for member in cached
        .members
        .iter()
        .skip(query.options.offset)
        .take(query.options.limit)
    {
        if let Some(context) = context {
            context.check_string()?;
        }
        let collection = repo
            .collections
            .get(member.collection_index)
            .ok_or("cached lifecycle collection missing")?;
        let (record, file, payload) = if let Some(segment_index) = member.segment_index {
            let segment = segments
                .get(member.collection_index)
                .and_then(|group| group.get(segment_index))
                .ok_or("cached lifecycle segment missing")?;
            let resolved = segment_record_ref(
                &segment.index,
                &segment.active,
                catalog,
                RecordSlot(member.slot),
            )?
            .ok_or("cached lifecycle segment slot retired")?;
            charge_lifecycle_cached_payload(context, query, resolved.record)?;
            let payload = if query.options.return_mode == ReturnMode::Locator {
                Ok(None)
            } else {
                get_segment_record_by_id(
                    repo,
                    member.collection_index,
                    &segment.index,
                    &segment.active,
                    catalog,
                    revision,
                    &resolved.record.id.0,
                    query.options.return_mode.clone(),
                )
                .and_then(|hit| hit.ok_or_else(|| "cached lifecycle record missing".to_owned()))
                .map(|hit| hit.payload)
            };
            (resolved.record, resolved.file, payload)
        } else {
            let record = collection
                .generation
                .records
                .get(member.slot as usize)
                .ok_or("cached lifecycle base slot missing")?;
            if !base_active[member.collection_index].contains(RecordSlot(member.slot)) {
                return Err("cached lifecycle base slot retired".into());
            }
            let file = catalog
                .entry(record.file)
                .ok_or("cached lifecycle base file missing")?;
            charge_lifecycle_cached_payload(context, query, record)?;
            let payload = if query.options.return_mode == ReturnMode::Locator {
                Ok(None)
            } else {
                let path = repo.files.root.join(&file.relative_path);
                read_record_range_for_search(&path, file.revision, record.range, None)
                    .and_then(|bytes| {
                        payload_from_bytes(
                            record.class,
                            Some(collection),
                            &query.options.return_mode,
                            bytes,
                        )
                    })
                    .map(|(payload, _)| payload)
            };
            (record, file, payload)
        };
        let mut hit = segment_locator_hit(
            collection,
            SegmentRecordRef { record, file },
            revision,
            member.matched_fields.clone(),
        );
        if query.options.return_mode != ReturnMode::Locator
            && !(query.options.return_mode == ReturnMode::Logical
                && record.class == RecordClass::RawFallback)
        {
            counters.files_opened += 1;
            counters.bytes_read += record.range.len();
            counters.record_parses += u64::from(query.options.return_mode == ReturnMode::Logical);
            files.insert(file.id.0);
        }
        match payload {
            Ok(payload) => hit.payload = payload,
            Err(message) => {
                if record_unavailable_hit(repo, catalog, &hit, message.clone(), &mut counters)? {
                    continue;
                }
                return Err(message);
            }
        }
        hits.push(hit);
    }
    counters.distinct_files = files.len() as u64;
    counters.returned_records = hits.len() as u64;
    counters.match_count_complete = counters.source_unavailable_events == 0;
    Ok(SearchResult {
        count: (query.options.count && counters.match_count_complete)
            .then_some(cached.members.len()),
        hits,
        counters,
    })
}

fn charge_lifecycle_cached_payload(
    context: Option<&ExecutionContext>,
    query: &Query,
    record: &RecordEntry,
) -> Result<(), String> {
    if query.options.return_mode == ReturnMode::Locator
        || (query.options.return_mode == ReturnMode::Logical
            && record.class == RecordClass::RawFallback)
    {
        return Ok(());
    }
    if let Some(context) = context {
        if record.range.len() > context.limits.source_record_bytes {
            return Err(context.fail(
                ExecutionErrorKind::Resource,
                "cached lifecycle record exceeds source read allowance",
            ));
        }
        context.charge_reads(1)?;
        context.charge_bytes(record.range.len())?;
    }
    Ok(())
}


struct RelationTargetPrune {
    source_path: Vec<String>,
    target_ids: HashSet<String>,
}


fn eligible_one_hop_relation_term<'a>(
    repo: &Repository,
    term: &'a BoundTerm,
) -> Option<(&'a crate::relation_query::BoundRelationHop, &'a BoundTerm)> {
    let related = term.related.as_ref()?;
    let [hop] = related.hops.as_slice() else {
        return None;
    };
    if related.source_operand.is_some()
        || hop.direction != crate::relation_query::HopDirection::Forward
        || hop.selectors.iter().any(Option::is_some)
        || !matches!(term.quantifier, Quantifier::Default | Quantifier::Any)
    {
        return None;
    }
    let source = repo.collections.get(hop.from.0 as usize)?;
    let relation = source.relations.get(hop.relation.ordinal as usize)?;
    if term.quantifier == Quantifier::Default {
        if relation.declaration.cardinality != crate::relation::RelationCardinality::One {
            return None;
        }
        for depth in 1..relation.declaration.path.len() {
            let ancestor = source
                .definition
                .schema
                .fields()
                .get(&relation.declaration.path[..depth])?;
            if matches!(ancestor.kind, crate::model::LogicalType::Array(_)) {
                return None;
            }
        }
    }
    Some((hop, &related.terminal))
}

fn one_hop_relation_term<'a>(
    repo: &Repository,
    expr: &'a BoundExpr,
) -> Option<(&'a crate::relation_query::BoundRelationHop, &'a BoundTerm)> {
    match expr {
        BoundExpr::Term(term) => eligible_one_hop_relation_term(repo, term),
        BoundExpr::And(parts) => parts
            .iter()
            .find_map(|part| one_hop_relation_term(repo, part)),
        BoundExpr::Or(_) | BoundExpr::Not(_) => None,
    }
}

fn collect_relation_target_ids(
    repo: &Repository,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    hop: &crate::relation_query::BoundRelationHop,
    mut target_candidates: impl FnMut(
        &IndexedCollectionSegment,
        &ActiveRecords,
        &CollectionState,
    ) -> Result<Option<RoaringBitmap>, String>,
) -> Result<Option<RelationTargetPrune>, String> {
    let target_index = hop.to.0 as usize;
    if base_active[target_index].len() != 0 {
        return Ok(None);
    }
    let target = &repo.collections[target_index];
    let mut selections = Vec::with_capacity(segments[target_index].len());
    let mut candidate_count = 0u64;
    let mut active_count = 0u64;
    for published in segments[target_index].iter() {
        let Some(mut may) = target_candidates(&published.index, &published.active, target)? else {
            return Ok(None);
        };
        may &= published.active.bitmap();
        candidate_count += may.len();
        active_count += published.active.len();
        selections.push(may);
    }
    // A dense target set would spend memory without eliminating enough source
    // reads. Keep this a query-local bounded optimization.
    if candidate_count > 100_000 || candidate_count * 4 > active_count * 3 {
        return Ok(None);
    }
    let mut target_ids = HashSet::new();
    for (published, may) in segments[target_index].iter().zip(selections) {
        for slot in may.iter() {
            let record = published
                .index
                .record(RecordSlot(slot))
                .ok_or("relation target candidate missing")?;
            target_ids.insert(record.id.0.clone());
        }
    }
    let relation = repo
        .collections
        .get(hop.from.0 as usize)
        .and_then(|collection| collection.relations.get(hop.relation.ordinal as usize))
        .ok_or("relation source declaration missing")?;
    Ok(Some(RelationTargetPrune {
        source_path: relation.declaration.path.clone(),
        target_ids,
    }))
}

fn relation_target_prune(
    repo: &Repository,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    filter: Option<&BoundExpr>,
    fts: Option<&BoundFts>,
) -> Result<Option<RelationTargetPrune>, String> {
    if let Some((hop, terminal)) = filter.and_then(|expr| one_hop_relation_term(repo, expr)) {
        return collect_relation_target_ids(
            repo,
            base_active,
            segments,
            hop,
            |segment, active, collection| {
                segment_filter_bounds(
                    segment,
                    active,
                    &BoundExpr::Term(terminal.clone()),
                    collection,
                )
                .map(|bounds| Some(bounds.may))
            },
        );
    }
    if let Some(fts) = fts
        && let [field] = fts.fields.as_slice()
        && let Some(hops) = field.related.as_ref()
        && let [hop] = hops.as_slice()
        && hop.direction == crate::relation_query::HopDirection::Forward
        && !hop.selectors.iter().any(Option::is_some)
        && field.query_path.is_none()
    {
        let mut local = fts.clone();
        local.fields[0].related = None;
        return collect_relation_target_ids(
            repo,
            base_active,
            segments,
            hop,
            |segment, _active, collection| Ok(segment_fts_candidates(segment, collection, &local)),
        );
    }
    Ok(None)
}

fn relation_source_candidates(
    published: &PublishedSegment,
    plan: &RelationTargetPrune,
) -> Option<RoaringBitmap> {
    let posting = published.index.postings.fields.get(&plan.source_path)?;
    let mut may = &published.active.bitmap() - &published.index.logical_records;
    for (key, slots) in &posting.values {
        if let crate::posting::ScalarKey::Id(id) = key
            && plan.target_ids.contains(id)
        {
            may |= slots;
        }
    }
    may &= published.active.bitmap();
    Some(may)
}


enum DirectRelationPredicate<'a> {
    Term(&'a BoundTerm),
    Fts(BoundFts),
}


struct DirectTargetScan {
    matched_id: Option<String>,
    read_bytes: u64,
    read: bool,
    unavailable: bool,
}


fn scan_direct_target_parallel(
    repo: &Repository,
    target_collection: &CollectionState,
    published: &PublishedSegment,
    catalog: &FileCatalog,
    predicate: &DirectRelationPredicate<'_>,
    must: Option<&RoaringBitmap>,
    slot: u32,
) -> Result<DirectTargetScan, String> {
    let resolved = segment_record_ref(
        &published.index,
        &published.active,
        catalog,
        RecordSlot(slot),
    )?
    .ok_or("direct relation target retired")?;
    if resolved.record.class == RecordClass::RawFallback {
        return Ok(DirectTargetScan {
            matched_id: None,
            read_bytes: 0,
            read: false,
            unavailable: false,
        });
    }
    if must.is_some_and(|must| must.contains(slot)) {
        return Ok(DirectTargetScan {
            matched_id: Some(resolved.record.id.0.clone()),
            read_bytes: 0,
            read: false,
            unavailable: false,
        });
    }
    let path = repo.files.root.join(&resolved.file.relative_path);
    let bytes = match read_record_range_for_search(
        &path,
        resolved.file.revision,
        resolved.record.range,
        None,
    ) {
        Ok(bytes) => bytes,
        Err(_) => {
            return Ok(DirectTargetScan {
                matched_id: None,
                read_bytes: 0,
                read: false,
                unavailable: true,
            });
        }
    };
    let borrow_value_text = !target_collection.source.is_extension()
        && matches!(target_collection.source.id(), "json" | "jsonl")
        && matches!(predicate, DirectRelationPredicate::Fts(_));
    let extracted = prepare_query_record_with_execution(
        resolved.record.class,
        &target_collection.source,
        &target_collection.definition.schema,
        &bytes,
        &ScanProjection::full(
            matches!(predicate, DirectRelationPredicate::Fts(_)) && !borrow_value_text,
        ),
        false,
        None,
    )?;
    let ctx = RecordContext {
        value: &extracted.value,
        source_bytes: Some(&bytes),
        record: resolved.record,
        file: resolved.file,
        collection_name: &target_collection.definition.name,
        repository_root: &repo.files.root,
    };
    let matched = match predicate {
        DirectRelationPredicate::Term(term) => eval_term(term, &ctx)?,
        DirectRelationPredicate::Fts(local) => {
            if borrow_value_text {
                crate::reference::eval_borrowed_value_fts(local, &ctx, None)?
            } else {
                eval_bound_fts(local, &extracted.text, &ctx, &mut HashSet::new())?
            }
        }
    };
    Ok(DirectTargetScan {
        matched_id: matched.then(|| resolved.record.id.0.clone()),
        read_bytes: bytes.len() as u64,
        read: true,
        unavailable: false,
    })
}

/// A single positive forward hop can be answered target first. Verify only
/// possible targets, reverse their IDs through resident source postings, and
/// rank the resulting source locators without parsing every source record.
/// Unsupported shapes and unavailable files use the existing merged scan.
#[allow(clippy::too_many_arguments)]
fn try_direct_relation_page(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
    revision: IndexRevision,
    sort: &ResolvedSort<'_>,
    prepared: &[(usize, Option<BoundExpr>, Option<BoundFts>)],
    context: Option<&ExecutionContext>,
    mut cache_builder: Option<&mut LifecycleCacheBuilder>,
    retained_budget: Option<usize>,
) -> Result<Option<SearchResult>, String> {
    if query.options.return_mode != ReturnMode::Locator || prepared.len() != 1 {
        return Ok(None);
    }
    let (source_index, filter, fts) = &prepared[0];
    let (hop, predicate, evidence, local_filter) = if let (Some(filter), None) = (filter, fts) {
        let (term, local_filter) = match filter {
            BoundExpr::Term(term) => (term, None),
            BoundExpr::And(parts) if parts.len() == 2 => {
                let first = &parts[0];
                let second = &parts[1];
                if let BoundExpr::Term(term) = first
                    && eligible_one_hop_relation_term(repo, term).is_some()
                    && matches!(second, BoundExpr::Term(_))
                {
                    (term, Some(second))
                } else if let BoundExpr::Term(term) = second
                    && eligible_one_hop_relation_term(repo, term).is_some()
                    && matches!(first, BoundExpr::Term(_))
                {
                    (term, Some(first))
                } else {
                    return Ok(None);
                }
            }
            _ => return Ok(None),
        };
        let Some((hop, terminal)) = eligible_one_hop_relation_term(repo, term) else {
            return Ok(None);
        };
        let mut evidence = if query.options.evidence
            && term.derived == Derived::None
            && !matches!(term.operator, Operator::Ne | Operator::NotContains)
        {
            let mut fields = vec![term.query_path.display.clone()];
            fields.push(term.related.as_ref().unwrap().evidence_path.clone());
            fields.sort();
            fields.dedup();
            fields
        } else {
            Vec::new()
        };
        if query.options.evidence
            && let Some(BoundExpr::Term(local)) = local_filter
            && local.derived == Derived::None
            && !matches!(local.operator, Operator::Ne | Operator::NotContains)
        {
            evidence.push(local.query_path.display.clone());
            evidence.sort();
            evidence.dedup();
        }
        (
            hop,
            DirectRelationPredicate::Term(terminal),
            evidence,
            local_filter,
        )
    } else if let (None, Some(fts)) = (filter, fts)
        && let [field] = fts.fields.as_slice()
        && let Some(hops) = field.related.as_ref()
        && let [hop] = hops.as_slice()
        && hop.direction == crate::relation_query::HopDirection::Forward
        && !hop.selectors.iter().any(Option::is_some)
        && field.query_path.is_none()
    {
        let mut local = fts.clone();
        local.fields[0].related = None;
        let evidence = if query.options.evidence {
            vec![field.evidence_path.clone()]
        } else {
            Vec::new()
        };
        (hop, DirectRelationPredicate::Fts(local), evidence, None)
    } else {
        return Ok(None);
    };
    if hop.from.0 as usize != *source_index
        || base_active[*source_index].len() != 0
        || base_active[hop.to.0 as usize].len() != 0
    {
        return Ok(None);
    }
    let target_index = hop.to.0 as usize;
    let target_collection = &repo.collections[target_index];
    let mut local_proofs = Vec::new();
    if let Some(local_filter) = local_filter {
        for published in segments[*source_index].iter() {
            let bounds = segment_filter_bounds_with_catalog(
                &published.index,
                &published.active,
                local_filter,
                &repo.collections[*source_index],
                Some(catalog),
            )?;
            // Data predicates cannot match a raw fallback. Prove only the
            // logical source domain used by the relation reverse posting.
            let logical = &published.active.bitmap() & &published.index.logical_records;
            let may = &bounds.may & &logical;
            let must = &bounds.must & &logical;
            if may != must {
                return Ok(None);
            }
            local_proofs.push(must);
        }
    }
    let source_relation = repo.collections[*source_index]
        .relations
        .get(hop.relation.ordinal as usize)
        .ok_or("direct relation declaration missing")?;
    let direct_sort_column = if sort.bound.has_computed {
        let Some([term]) = sort.bound.terms[*source_index].as_deref() else {
            return Ok(None);
        };
        let BoundSortSource::Computed(computed) = &term.source else {
            return Ok(None);
        };
        let Some(path) = computed.direct_related_target_column(hop) else {
            return Ok(None);
        };
        if source_relation.declaration.cardinality != crate::relation::RelationCardinality::One {
            return Ok(None);
        }
        let field = FieldPath::parse(path)?;
        if !target_collection
            .definition
            .schema
            .fields()
            .get(&field.segments)
            .is_some_and(|field| !field.multi)
        {
            return Ok(None);
        }
        for published in segments[target_index].iter() {
            let Ok(column) = published.index.sorting.column(path) else {
                return Ok(None);
            };
            if column.locale != target_collection.definition.sort_locale
                || column.options != target_collection.definition.sort_options
            {
                return Ok(None);
            }
        }
        Some(path.to_owned())
    } else {
        None
    };
    let source_path = &source_relation.declaration.path;
    let mut target_ids = HashSet::new();
    let mut target_sort_keys = HashMap::new();
    let mut counters = WorkCounters::default();
    let route_budget = retained_budget
        .unwrap_or(64 * 1024 * 1024)
        .min(64 * 1024 * 1024);
    let mut route_bytes = 0usize;
    let mut reserve_route = |additional: usize| -> bool {
        let Some(next) = route_bytes.checked_add(additional) else {
            return false;
        };
        if next > route_budget {
            return false;
        }
        route_bytes = next;
        true
    };
    for published in segments[target_index].iter() {
        let segment = &published.index;
        let (may, must) = match &predicate {
            DirectRelationPredicate::Term(term) => {
                let bounds = segment_filter_bounds_with_catalog(
                    segment,
                    &published.active,
                    &BoundExpr::Term((*term).clone()),
                    target_collection,
                    Some(catalog),
                )?;
                (bounds.may, Some(bounds.must))
            }
            DirectRelationPredicate::Fts(local) => (
                segment_fts_candidates(segment, target_collection, local)
                    .unwrap_or_else(|| published.active.bitmap()),
                None,
            ),
        };
        let may = &may & &published.active.bitmap();
        counters.relation_target_candidates += may.len();
        if counters.relation_target_candidates > 100_000 {
            return Ok(None);
        }
        let parallel_targets = context.is_none()
            && cache_builder.is_none()
            && retained_budget.is_none()
            && direct_sort_column.is_none()
            && (2..=32).contains(&repo.options.query_workers)
            && may.len() > 1
            && may.iter().all(|slot| {
                segment.record(RecordSlot(slot)).is_some_and(|record| {
                    record.range.len() <= 64 * 1024
                        && record.id.0.len() <= 1024
                        && catalog
                            .entry(record.file)
                            .is_some_and(|file| file.relative_path.to_string_lossy().len() <= 4096)
                })
            });
        if parallel_targets {
            let slots = may.iter().collect::<Vec<_>>();
            for chunk in slots.chunks(256) {
                let scans = repo.workers.install(|| {
                    chunk
                        .par_iter()
                        .map(|slot| {
                            scan_direct_target_parallel(
                                repo,
                                target_collection,
                                published,
                                catalog,
                                &predicate,
                                must.as_ref(),
                                *slot,
                            )
                        })
                        .collect::<Vec<_>>()
                });
                for scan in scans {
                    let scan = scan?;
                    if scan.unavailable {
                        return Ok(None);
                    }
                    if scan.read {
                        counters.relation_target_reads += 1;
                        counters.relation_target_file_opens += 1;
                        counters.relation_bytes_read += scan.read_bytes;
                    }
                    if let Some(id) = scan.matched_id {
                        counters.relation_target_matches += 1;
                        if !target_ids.contains(&id) {
                            if !reserve_route(id.len().saturating_add(64)) {
                                return Ok(None);
                            }
                            target_ids.insert(id);
                        }
                    }
                }
            }
            continue;
        }
        for slot in may.iter() {
            if let Some(context) = context {
                context.check_string()?;
            }
            let resolved =
                segment_record_ref(segment, &published.active, catalog, RecordSlot(slot))?
                    .ok_or("direct relation target retired")?;
            if resolved.record.class == RecordClass::RawFallback {
                continue;
            }
            let matched = if must.as_ref().is_some_and(|must| must.contains(slot)) {
                // This is an index-only Locator/count proof. Actual payload or
                // residual reads use the checked pinned source identity.
                true
            } else {
                let path = repo.files.root.join(&resolved.file.relative_path);
                if let Some(context) = context {
                    context.charge_reads(1)?;
                    context.charge_bytes(resolved.record.range.len())?;
                }
                let bytes = match read_record_range_for_search(
                    &path,
                    resolved.file.revision,
                    resolved.record.range,
                    None,
                ) {
                    Ok(bytes) => bytes,
                    Err(_) => return Ok(None),
                };
                counters.relation_target_reads += 1;
                counters.relation_target_file_opens += 1;
                counters.relation_bytes_read += bytes.len() as u64;
                let borrow_value_text = !target_collection.source.is_extension()
                    && matches!(target_collection.source.id(), "json" | "jsonl")
                    && matches!(predicate, DirectRelationPredicate::Fts(_));
                let extracted = prepare_query_record_with_execution(
                    resolved.record.class,
                    &target_collection.source,
                    &target_collection.definition.schema,
                    &bytes,
                    &ScanProjection::full(
                        matches!(predicate, DirectRelationPredicate::Fts(_)) && !borrow_value_text,
                    ),
                    false,
                    context,
                )?;
                let ctx = RecordContext {
                    value: &extracted.value,
                    source_bytes: Some(&bytes),
                    record: resolved.record,
                    file: resolved.file,
                    collection_name: &target_collection.definition.name,
                    repository_root: &repo.files.root,
                };
                match &predicate {
                    DirectRelationPredicate::Term(term) => eval_term(term, &ctx)?,
                    DirectRelationPredicate::Fts(local) => {
                        if borrow_value_text {
                            crate::reference::eval_borrowed_value_fts(local, &ctx, None)?
                        } else {
                            eval_bound_fts(local, &extracted.text, &ctx, &mut HashSet::new())?
                        }
                    }
                }
            };
            if matched {
                counters.relation_target_matches += 1;
                if !target_ids.contains(&resolved.record.id.0) {
                    let sort_key = if let Some(path) = direct_sort_column.as_deref() {
                        Some(segment.sorting.key(path, RecordSlot(slot))?.clone())
                    } else {
                        None
                    };
                    let sort_bytes = sort_key.as_ref().map_or(0, |key| {
                        resolved
                            .record
                            .id
                            .0
                            .len()
                            .saturating_add(crate::sort_binding::key_bytes(key))
                            .saturating_add(64)
                    });
                    if !reserve_route(
                        resolved
                            .record
                            .id
                            .0
                            .len()
                            .saturating_add(64)
                            .saturating_add(sort_bytes),
                    ) {
                        return Ok(None);
                    }
                    if let Some(key) = sort_key {
                        target_sort_keys.insert(resolved.record.id.0.clone(), key);
                    }
                    target_ids.insert(resolved.record.id.0.clone());
                }
            }
        }
    }
    let mut source_candidates = Vec::new();
    for (segment_index, published) in segments[*source_index].iter().enumerate() {
        let Some(posting) = published.index.postings.fields.get(source_path) else {
            return Ok(None);
        };
        let mut may = RoaringBitmap::new();
        for (key, slots) in &posting.values {
            if let crate::posting::ScalarKey::Id(id) = key
                && target_ids.contains(id)
            {
                may |= slots;
                counters.relation_reverse_postings_visited += slots.len();
            }
        }
        may &= published.active.bitmap();
        may &= &published.index.logical_records;
        if local_filter.is_some() {
            may &= &local_proofs[segment_index];
        }
        if !reserve_route(may.serialized_size().saturating_add(64)) {
            return Ok(None);
        }
        source_candidates.push((segment_index, may));
    }
    let cap = query
        .options
        .offset
        .checked_add(query.options.limit)
        .ok_or("direct relation page size overflow")?;
    if context.is_none()
        && cache_builder.is_none()
        && retained_budget.is_none()
        && direct_sort_column.is_none()
        && source_candidates.len() <= 4096
    {
        let selected = source_candidates
            .iter()
            .enumerate()
            .map(|(index, (segment, may))| {
                (index == *segment).then(|| (may.clone(), evidence.clone()))
            })
            .collect::<Option<Vec<_>>>();
        if let Some(selected) = selected
            && let Some(result) = exact_ordered_page(
                repo,
                query,
                sort,
                segments,
                catalog,
                revision,
                *source_index,
                &selected,
                counters.clone(),
            )?
        {
            return Ok(Some(result));
        }
    }
    let mut winners = BinaryHeap::new();
    let mut winner_bytes = 0usize;
    let mut total = 0u64;
    for (segment_index, may) in source_candidates {
        let published = &segments[*source_index][segment_index];
        let stored_order = if cache_builder.is_none()
            && context.is_none()
            && direct_sort_column.is_none()
            && cap < may.len() as usize
        {
            lifecycle_segment_order(&published.index, sort, query, *source_index)
        } else {
            None
        };
        if stored_order.is_some() {
            // A global top `cap` cannot include a row ranked below its own
            // shard's top `cap`. Count the full exact bitmap without building
            // rank keys and hit locators for every matching source.
            total += may.len();
            counters.exact_posting_matches += may.len();
        }
        let slots: Box<dyn Iterator<Item = u32> + '_> = if let Some(order) = stored_order {
            Box::new(
                order
                    .iter()
                    .filter(|slot| may.contains(slot.0))
                    .take(cap)
                    .map(|slot| slot.0),
            )
        } else {
            Box::new(may.iter())
        };
        for slot in slots {
            if let Some(context) = context {
                context.check_string()?;
            }
            let resolved = segment_record_ref(
                &published.index,
                &published.active,
                catalog,
                RecordSlot(slot),
            )?
            .ok_or("direct relation source retired")?;
            let key = if direct_sort_column.is_some() {
                let relation = published
                    .index
                    .relations
                    .get(hop.relation.ordinal as usize)
                    .ok_or("direct relation source contribution missing")?;
                let mut references = relation.references_for_source(RecordSlot(slot));
                let Some((target_id, _)) = references.next() else {
                    return Ok(None);
                };
                if references.next().is_some() {
                    return Ok(None);
                }
                let Some(target_key) = target_sort_keys.get(target_id) else {
                    return Ok(None);
                };
                let term = &sort.bound.terms[*source_index]
                    .as_ref()
                    .ok_or("direct related sort term missing")?[0];
                RankKey {
                    parts: vec![(target_key.clone(), term.descending, term.options)],
                    collection: repo.collections[*source_index].definition.name.clone(),
                    id: resolved.record.id.0.clone(),
                }
            } else {
                segment_rank_key(
                    repo,
                    sort,
                    *source_index,
                    &published.index,
                    resolved.record,
                    None,
                )?
            };
            if stored_order.is_none() {
                total += 1;
                counters.exact_posting_matches += 1;
            }
            let hit = segment_locator_hit(
                &repo.collections[*source_index],
                resolved,
                revision,
                evidence.clone(),
            );
            if let Some(builder) = cache_builder.as_mut() {
                builder.push(
                    &key,
                    CachedMember {
                        collection_index: *source_index,
                        segment_index: Some(segment_index),
                        slot,
                        matched_fields: hit.matched_fields.clone(),
                    },
                );
            }
            offer_merged(
                &mut winners,
                cap,
                MergedPageHit {
                    key,
                    hit,
                    segment: Some((*source_index, segment_index)),
                    retained_bytes: 0,
                },
                retained_budget.zip(context),
                &mut winner_bytes,
            )?;
        }
    }
    let page = winners
        .into_sorted_vec()
        .into_iter()
        .skip(query.options.offset)
        .take(query.options.limit)
        .map(|winner| winner.hit)
        .collect::<Vec<_>>();
    counters.total_matches = total;
    counters.match_count_complete = true;
    counters.returned_records = page.len() as u64;
    Ok(Some(SearchResult {
        count: query.options.count.then_some(total as usize),
        hits: page,
        counters,
    }))
}


struct RelationFileJob {
    segment_index: usize,
    file_id: u32,
    slot: RecordSlot,
    range: ByteRange,
}


struct RelationFileScan {
    counters: WorkCounters,
    primary_opened: bool,
    unavailable: SourceAvailabilityLog,
    relation_unavailable: SourceAvailabilityLog,
    winner: Option<MergedPageHit>,
}


fn bounded_parallel_relation_filter(expr: &BoundExpr) -> bool {
    match expr {
        BoundExpr::Term(term) => {
            term.path.root != PathRoot::Backlinks
                && term
                    .rhs
                    .as_ref()
                    .is_none_or(|rhs| rhs.path.root != PathRoot::Backlinks)
                && term.related.as_ref().is_none_or(|related| {
                    related.hops.len() <= 2
                        && related.hops.iter().all(|hop| {
                            hop.direction == crate::relation_query::HopDirection::Forward
                        })
                })
        }
        BoundExpr::And(parts) | BoundExpr::Or(parts) => {
            parts.iter().all(bounded_parallel_relation_filter)
        }
        BoundExpr::Not(inner) => bounded_parallel_relation_filter(inner),
    }
}

#[allow(clippy::too_many_arguments)]
fn scan_relation_file_parallel(
    repo: &Repository,
    query: &Query,
    sort: &ResolvedSort<'_>,
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    _catalog: &FileCatalog,
    revision: IndexRevision,
    collection_index: usize,
    filter: Option<&BoundExpr>,
    fts: Option<&BoundFts>,
    job: &RelationFileJob,
    view: crate::relation_query::MergedRelationView<'_>,
) -> Result<RelationFileScan, String> {
    let _fold_record = FoldRecordGuard::new_with_sharing(shared_fold_bound(filter));
    let mut scan = RelationFileScan {
        counters: WorkCounters::default(),
        primary_opened: false,
        unavailable: SourceAvailabilityLog::default(),
        relation_unavailable: SourceAvailabilityLog::default(),
        winner: None,
    };
    let address = crate::relation_query::MergedRecordSlot {
        collection: repo.collections[collection_index].generation.collection,
        segment: Some(job.segment_index),
        slot: job.slot,
    };
    let resolved = view
        .record(address)?
        .ok_or("active parallel relation source missing")?;
    let path = repo.files.root.join(&resolved.file.relative_path);
    let mut reader = match RecordRangeReader::open_for_search(&path, resolved.file.revision, None) {
        Ok(reader) => reader,
        Err(message) => {
            scan.unavailable.record(SourceDiagnostic {
                file_id: resolved.file.id.0,
                file_path: resolved.file.relative_path.clone(),
                kind: classify_source_read_failure_for_search(&path, resolved.file.revision, None),
                message,
            });
            return Ok(scan);
        }
    };
    scan.primary_opened = true;
    let (read, _) = match reader.read(&[(job.slot.0, job.range)]) {
        Ok(read) => read,
        Err(message) => {
            scan.unavailable.record(SourceDiagnostic {
                file_id: resolved.file.id.0,
                file_path: resolved.file.relative_path.clone(),
                kind: classify_source_read_failure_for_search(
                    &path,
                    resolved.file.revision,
                    Some(job.range),
                ),
                message,
            });
            return Ok(scan);
        }
    };
    let bytes = read
        .into_iter()
        .next()
        .ok_or("parallel relation row missing")?
        .1;
    scan.counters.candidates += 1;
    let record = resolved.record;
    let borrowed_value_text = borrowed_value_text(resolved.collection, fts);
    let mut projection = ScanProjection::full(
        fts.is_some()
            && !borrowed_value_text
            && !borrowed_relation_source_text(resolved.collection, fts),
    );
    projection.resolved_text_only =
        projection.collect_text && fts.is_some_and(|fts| !fts.fields.is_empty());
    let combined_fts = fts.filter(|fts| {
        resolved.collection.source.id() == "json_decoded_scalars"
            && source_fts_eligible(resolved.collection, fts, true)
    });
    let (extracted, combined_result) = if record.class == RecordClass::Logical
        && let Some(combined_fts) = combined_fts
    {
        let (extracted, result) = prepare_decoded_record_with_borrowed_fts(
            &resolved.collection.definition.schema,
            &bytes,
            &projection,
            combined_fts,
            query.options.evidence,
            None,
        )?;
        (extracted, Some(result))
    } else if record.class == RecordClass::Logical {
        (
            prepare_query_record_with_execution(
                record.class,
                &resolved.collection.source,
                &resolved.collection.definition.schema,
                &bytes,
                &projection,
                false,
                None,
            )?,
            None,
        )
    } else {
        (
            ExtractedRecord {
                value: Value::Object(Default::default()),
                text: Vec::new(),
            },
            None,
        )
    };
    let ctx = RecordContext {
        value: &extracted.value,
        source_bytes: Some(&bytes),
        record,
        file: resolved.file,
        collection_name: &resolved.collection.definition.name,
        repository_root: &repo.files.root,
    };
    let mut evidence = HashSet::new();
    let mut relation_work = crate::relation_query::RelationWork::default();
    let filter_matches = if let Some(filter) = filter {
        if record.class == RecordClass::RawFallback {
            eval_unavailable_filter(filter, &ctx, &mut evidence)? == Some(true)
        } else {
            crate::relation_query::eval_filter_truth_merged(
                &view,
                filter,
                address,
                &ctx,
                &mut evidence,
                &mut relation_work,
            )? == Some(true)
        }
    } else {
        true
    };
    let fts_matches = if filter_matches {
        if let Some(fts) = fts {
            if record.class == RecordClass::RawFallback && fts.fields.is_empty() {
                let matched =
                    crate::reference::raw_fallback_contains(fts, &String::from_utf8_lossy(&bytes));
                if matched && query.options.evidence {
                    evidence.insert("@record.raw_text".into());
                }
                matched
            } else if borrowed_value_text && record.class == RecordClass::Logical {
                crate::reference::eval_borrowed_value_fts(
                    fts,
                    &ctx,
                    query.options.evidence.then_some(&mut evidence),
                )?
            } else if let Some(result) = &combined_result {
                if query.options.evidence {
                    evidence.extend(result.evidence.iter().cloned());
                }
                result.fts_match
            } else {
                crate::relation_query::eval_fts_with_relations_merged(
                    &view,
                    fts,
                    &extracted.text,
                    address,
                    &ctx,
                    &mut evidence,
                    &mut relation_work,
                )?
            }
        } else {
            true
        }
    } else {
        false
    };
    scan.counters.add_scan(
        ScanWork {
            files_opened: 1,
            bytes_read: bytes.len() as u64,
            record_parses: u64::from(record.class == RecordClass::Logical),
            raw_text_verifications: u64::from(
                filter_matches
                    && record.class == RecordClass::RawFallback
                    && fts.is_some_and(|fts| fts.fields.is_empty()),
            ),
            scanned_records: 1,
            residual_evaluations: u64::from(filter.is_some()),
            ..ScanWork::default()
        },
        &relation_work,
    )?;
    if filter_matches && fts_matches {
        let key = segment_rank_key(
            repo,
            sort,
            collection_index,
            &segments[collection_index][job.segment_index].index,
            record,
            Some(&ctx),
        )?;
        let hit = segment_locator_hit(
            resolved.collection,
            SegmentRecordRef {
                record,
                file: resolved.file,
            },
            revision,
            if query.options.evidence {
                evidence.into_iter().collect()
            } else {
                Vec::new()
            },
        );
        scan.winner = Some(MergedPageHit {
            key,
            hit,
            segment: Some((collection_index, job.segment_index)),
            retained_bytes: 0,
        });
    }
    scan.relation_unavailable = view.take_source_errors();
    Ok(scan)
}

fn merge_parallel_relation_work(
    total: &mut WorkCounters,
    part: &WorkCounters,
) -> Result<(), String> {
    total.candidates += part.candidates;
    total.files_opened += part.files_opened;
    total.bytes_read += part.bytes_read;
    total.record_parses += part.record_parses;
    total.raw_text_verifications += part.raw_text_verifications;
    total.scanned_records += part.scanned_records;
    total.residual_evaluations += part.residual_evaluations;
    total.relation_source_reads += part.relation_source_reads;
    total.relation_target_reads += part.relation_target_reads;
    total.relation_target_file_opens += part.relation_target_file_opens;
    total.relation_edge_visits += part.relation_edge_visits;
    total.relation_backlink_probe_visits += part.relation_backlink_probe_visits;
    total.relation_bytes_read += part.relation_bytes_read;
    total.relation_verified_targets_reused += part.relation_verified_targets_reused;
    crate::relation_query::check_query_work_budget(total)
}

#[allow(clippy::too_many_arguments)]
fn try_parallel_relation_page(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
    revision: IndexRevision,
    sort: &ResolvedSort<'_>,
    prepared: &[(usize, Option<BoundExpr>, Option<BoundFts>)],
    prune_segments: bool,
) -> Result<Option<SearchResult>, String> {
    let cap = query
        .options
        .offset
        .checked_add(query.options.limit)
        .ok_or("page size overflow")?;
    if prepared.len() != 1
        || query.options.return_mode != ReturnMode::Locator
        || sort.bound.has_computed
        || cap > 64
        || !(2..=32).contains(&repo.options.query_workers)
        || base_active.iter().any(|active| active.len() != 0)
    {
        return Ok(None);
    }
    let (collection_index, filter, fts) = &prepared[0];
    if repo.collections[*collection_index].definition.name.len() > 1024
        || query
            .filter
            .as_ref()
            .is_some_and(|filter| format!("{filter:?}").len() > 1024)
        || fts.as_ref().is_some_and(|fts| {
            fts.fields
                .iter()
                .try_fold(0usize, |bytes, field| {
                    bytes.checked_add(field.evidence_path.len())
                })
                .is_none_or(|bytes| bytes > 1024)
        })
        || filter
            .as_ref()
            .is_some_and(|filter| !bounded_parallel_relation_filter(filter))
        || fts.as_ref().is_some_and(|fts| {
            fts.fields.iter().any(|field| {
                field.related.as_ref().is_some_and(|hops| {
                    hops.len() > 2
                        || hops.iter().any(|hop| {
                            hop.direction != crate::relation_query::HopDirection::Forward
                        })
                })
            })
        })
    {
        return Ok(None);
    }
    if segments
        .iter()
        .flat_map(|group| group.iter())
        .any(|published| {
            published
                .index
                .records
                .iter()
                .any(|record| record.range.len() > 64 * 1024)
        })
    {
        return Ok(None);
    }
    let collection = &repo.collections[*collection_index];
    let mut jobs = Vec::new();
    let mut relation_prune: Option<Option<RelationTargetPrune>> = None;
    for (segment_index, published) in segments[*collection_index].iter().enumerate() {
        let mut candidates = if prune_segments {
            let filter_may = filter
                .as_ref()
                .map(|filter| {
                    segment_filter_bounds_with_catalog(
                        &published.index,
                        &published.active,
                        filter,
                        collection,
                        Some(catalog),
                    )
                    .map(|bounds| bounds.may)
                })
                .transpose()?;
            let fts_may = fts
                .as_ref()
                .and_then(|fts| segment_fts_candidates(&published.index, collection, fts));
            match (filter_may, fts_may) {
                (Some(filter), Some(fts)) => Some(filter & fts),
                (Some(filter), None) => Some(filter),
                (None, Some(fts)) => Some(fts),
                (None, None) => None,
            }
        } else {
            None
        };
        if prune_segments {
            let local_count = candidates
                .as_ref()
                .map_or_else(|| published.active.len(), RoaringBitmap::len);
            if local_count > 256 && relation_prune.is_none() {
                relation_prune = Some(relation_target_prune(
                    repo,
                    base_active,
                    segments,
                    filter.as_ref(),
                    fts.as_ref(),
                )?);
            }
            if let Some(Some(plan)) = relation_prune.as_ref()
                && let Some(relation_may) = relation_source_candidates(published, plan)
            {
                candidates =
                    Some(candidates.map_or(relation_may.clone(), |may| may & relation_may));
            }
        }
        if candidates.as_ref().is_some_and(RoaringBitmap::is_empty) {
            continue;
        }
        for file in &published.index.files {
            let mut selected = file.records.iter().filter(|slot| {
                published.active.contains(**slot)
                    && candidates.as_ref().is_none_or(|may| may.contains(slot.0))
            });
            let Some(slot) = selected.next().copied() else {
                continue;
            };
            if selected.next().is_some() {
                return Ok(None);
            }
            let record = published
                .index
                .record(slot)
                .ok_or("parallel relation slot missing")?;
            if record.range.len() > 64 * 1024
                || record.id.0.len() > 1024
                || file.entry.relative_path.to_string_lossy().len() > 4096
            {
                return Ok(None);
            }
            jobs.push(RelationFileJob {
                segment_index,
                file_id: file.entry.id.0,
                slot,
                range: record.range,
            });
            if jobs.len() > 50_000 {
                return Ok(None);
            }
        }
    }
    let view =
        crate::relation_query::MergedRelationView::new(repo, base_active, segments, catalog)?;
    let mut counters = WorkCounters::default();
    let mut opened = HashSet::new();
    let mut winners = BinaryHeap::new();
    let mut winner_bytes = 0usize;
    let mut total = 0u64;
    for chunk in jobs.chunks(256) {
        let views = (0..chunk.len())
            .map(|_| view.fork_for_worker())
            .collect::<Vec<_>>();
        let scans = repo.workers.install(|| {
            views
                .into_par_iter()
                .zip(chunk.par_iter())
                .map(|(worker_view, job)| {
                    scan_relation_file_parallel(
                        repo,
                        query,
                        sort,
                        segments,
                        catalog,
                        revision,
                        *collection_index,
                        filter.as_ref(),
                        fts.as_ref(),
                        job,
                        worker_view,
                    )
                })
                .collect::<Vec<_>>()
        });
        for (job, scan) in chunk.iter().zip(scans) {
            let mut scan = scan?;
            if scan.primary_opened {
                opened.insert(job.file_id);
            }
            merge_parallel_relation_work(&mut counters, &scan.counters)?;
            scan.unavailable.apply(&mut counters);
            scan.relation_unavailable.apply(&mut counters);
            if let Some(winner) = scan.winner.take() {
                total += 1;
                offer_merged(&mut winners, cap, winner, None, &mut winner_bytes)?;
            }
        }
    }
    let page = winners
        .into_sorted_vec()
        .into_iter()
        .skip(query.options.offset)
        .take(query.options.limit)
        .map(|winner| winner.hit)
        .collect::<Vec<_>>();
    counters.total_matches = total;
    counters.match_count_complete = counters.source_unavailable_events == 0;
    counters.returned_records = page.len() as u64;
    counters.distinct_files = opened.len() as u64;
    Ok(Some(SearchResult {
        count: (query.options.count && counters.match_count_complete).then_some(total as usize),
        hits: page,
        counters,
    }))
}

fn search_segmented_relations(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
    revision: IndexRevision,
    sort: &ResolvedSort<'_>,
    prepared: &[(usize, Option<BoundExpr>, Option<BoundFts>)],
    context: Option<&ExecutionContext>,
    mut cache_builder: Option<&mut LifecycleCacheBuilder>,
    retained_budget: Option<usize>,
    prune_segments: bool,
) -> Result<SearchResult, String> {
    if prune_segments
        && let Some(result) = try_direct_relation_page(
            repo,
            query,
            base_active,
            segments,
            catalog,
            revision,
            sort,
            prepared,
            context,
            cache_builder.as_deref_mut(),
            retained_budget,
        )?
    {
        return Ok(result);
    }
    if prune_segments
        && context.is_none()
        && cache_builder.is_none()
        && retained_budget.is_none()
        && let Some(result) = try_parallel_relation_page(
            repo,
            query,
            base_active,
            segments,
            catalog,
            revision,
            sort,
            prepared,
            prune_segments,
        )?
    {
        return Ok(result);
    }
    let view =
        crate::relation_query::MergedRelationView::new(repo, base_active, segments, catalog)?;
    let cap = query
        .options
        .offset
        .checked_add(query.options.limit)
        .ok_or("page size overflow")?;
    let mut winners = BinaryHeap::new();
    let mut winner_bytes = 0usize;
    let winner_budget = retained_budget.zip(context);
    let mut counters = WorkCounters::default();
    let mut total = 0u64;
    let mut opened = HashSet::new();
    let mut source_errors = SourceAvailabilityLog::default();
    let mut sort_cache = MergedSortTargetCache::default();
    sort_cache.configure_execution(context);
    for (collection_index, filter, fts) in prepared {
        let collection_index = *collection_index;
        let collection_id = repo.collections[collection_index].generation.collection;
        let collection_plan = &repo.collections[collection_index];
        let direct_plan =
            DirectSourcePlan::prepare_bound(collection_plan, sort, fts.as_ref(), filter.as_ref());
        let borrowed_value_text = borrowed_value_text(collection_plan, fts.as_ref());
        let combined_fts = fts.as_ref().filter(|fts| {
            collection_plan.source.id() == "json_decoded_scalars"
                && source_fts_eligible(collection_plan, fts, true)
        });
        let mut projection = ScanProjection::full(
            fts.is_some()
                && !borrowed_value_text
                && !borrowed_relation_source_text(collection_plan, fts.as_ref()),
        );
        projection.resolved_text_only =
            projection.collect_text && fts.as_ref().is_some_and(|fts| !fts.fields.is_empty());
        let mut process = |address: crate::relation_query::MergedRecordSlot,
                           bytes: Vec<u8>,
                           opened_source: bool|
         -> Result<(), String> {
            let _fold_record = FoldRecordGuard::new_with_sharing(shared_fold_bound(filter.as_ref()));
            if let Some(context) = context {
                context.check_string()?;
                context.charge_bytes(bytes.len() as u64)?;
            }
            let Some(resolved) = view.record(address)? else {
                return Ok(());
            };
            counters.candidates += 1;
            let record = resolved.record;
            let direct_result = if record.class == RecordClass::Logical {
                direct_plan
                    .as_ref()
                    .map(|direct| {
                        scan_borrowed_segments_worker(
                            &bytes,
                            &BorrowedScanQuery {
                                fts: direct.fts,
                                filter: direct.filter.map(|(term, _)| term),
                                decoded_scalars: direct.decoded_scalars,
                                retain_evidence: query.options.evidence,
                            },
                        )
                    })
                    .transpose()?
            } else {
                None
            };
            let (extracted, combined_result) = if record.class == RecordClass::Logical
                && direct_result.is_none()
                && let Some(combined_fts) = combined_fts
            {
                let (extracted, result) = prepare_decoded_record_with_borrowed_fts(
                    &resolved.collection.definition.schema,
                    &bytes,
                    &projection,
                    combined_fts,
                    query.options.evidence,
                    context,
                )?;
                (extracted, Some(result))
            } else if record.class == RecordClass::Logical && direct_result.is_none() {
                (
                    prepare_query_record_with_execution(
                        record.class,
                        &resolved.collection.source,
                        &resolved.collection.definition.schema,
                        &bytes,
                        &projection,
                        false,
                        context,
                    )?,
                    None,
                )
            } else {
                (
                    ExtractedRecord {
                        value: Value::Object(Default::default()),
                        text: Vec::new(),
                    },
                    None,
                )
            };
            let ctx = RecordContext {
                value: &extracted.value,
                source_bytes: Some(&bytes),
                record,
                file: resolved.file,
                collection_name: &resolved.collection.definition.name,
                repository_root: &repo.files.root,
            };
            let mut evidence = HashSet::new();
            let mut relation_work = crate::relation_query::RelationWork::default();
            relation_work.attach(context);
            let filter_matches = if let Some(result) = &direct_result {
                result.filter_match
            } else if let Some(filter) = filter.as_ref() {
                if record.class == RecordClass::RawFallback {
                    eval_unavailable_filter(filter, &ctx, &mut evidence)? == Some(true)
                } else {
                    crate::relation_query::eval_filter_truth_merged(
                        &view,
                        filter,
                        address,
                        &ctx,
                        &mut evidence,
                        &mut relation_work,
                    )? == Some(true)
                }
            } else {
                true
            };
            let fts_matches = if filter_matches {
                if let Some(result) = &direct_result {
                    if query.options.evidence {
                        evidence.extend(result.evidence.iter().cloned());
                        if result.filter_match
                            && result.fts_match
                            && let Some((term, _)) =
                                direct_plan.as_ref().and_then(|plan| plan.filter)
                            && term.operator == Operator::Contains
                        {
                            evidence.insert(term.query_path.display.clone());
                        }
                    }
                    result.fts_match
                } else if let Some(result) = &combined_result {
                    if query.options.evidence {
                        evidence.extend(result.evidence.iter().cloned());
                    }
                    result.fts_match
                } else if let Some(fts) = fts.as_ref() {
                    if record.class == RecordClass::RawFallback && fts.fields.is_empty() {
                        let matched = crate::reference::raw_fallback_contains(
                            fts,
                            &String::from_utf8_lossy(&bytes),
                        );
                        if matched && query.options.evidence {
                            evidence.insert("@record.raw_text".into());
                        }
                        matched
                    } else if borrowed_value_text && record.class == RecordClass::Logical {
                        crate::reference::eval_borrowed_value_fts(
                            fts,
                            &ctx,
                            query.options.evidence.then_some(&mut evidence),
                        )?
                    } else {
                        crate::relation_query::eval_fts_with_relations_merged(
                            &view,
                            fts,
                            &extracted.text,
                            address,
                            &ctx,
                            &mut evidence,
                            &mut relation_work,
                        )?
                    }
                } else {
                    true
                }
            } else {
                false
            };
            counters.add_scan(
                ScanWork {
                    files_opened: u64::from(opened_source),
                    bytes_read: bytes.len() as u64,
                    record_parses: u64::from(record.class == RecordClass::Logical),
                    raw_text_verifications: u64::from(
                        filter_matches
                            && record.class == RecordClass::RawFallback
                            && fts.as_ref().is_some_and(|fts| fts.fields.is_empty()),
                    ),
                    scanned_records: 1,
                    residual_evaluations: u64::from(filter.is_some()),
                    ..ScanWork::default()
                },
                &relation_work,
            )?;
            if !(filter_matches && fts_matches) {
                return Ok(());
            }
            let source_errors_before_sort = view.source_error_events();
            let key = relation_rank_key_merged(
                repo,
                sort,
                &view,
                segments,
                address,
                &ctx,
                &mut sort_cache,
            )?;
            if view.source_error_events() > source_errors_before_sort {
                return Ok(());
            }
            total += 1;
            counters.sort_key_evaluations += u64::from(sort.bound.has_computed);
            let hit = segment_locator_hit(
                resolved.collection,
                SegmentRecordRef {
                    record,
                    file: resolved.file,
                },
                revision,
                if query.options.evidence {
                    evidence.into_iter().collect()
                } else {
                    Vec::new()
                },
            );
            if let Some(builder) = cache_builder.as_mut() {
                builder.push(
                    &key,
                    CachedMember {
                        collection_index,
                        segment_index: address.segment,
                        slot: address.slot.0,
                        matched_fields: hit.matched_fields.clone(),
                    },
                );
            }
            offer_merged(
                &mut winners,
                cap,
                MergedPageHit {
                    key,
                    hit,
                    segment: address
                        .segment
                        .map(|segment_index| (collection_index, segment_index)),
                    retained_bytes: 0,
                },
                winner_budget,
                &mut winner_bytes,
            )?;
            Ok(())
        };
        let collection = &repo.collections[collection_index];
        for file_id in &collection.generation.files {
            if let Some(context) = context {
                context.check_string()?;
            }
            let slots = collection
                .generation
                .file_records
                .records_for(*file_id)
                .ok_or("merged base file ownership missing")?;
            let ranges = slots
                .iter()
                .filter(|slot| base_active[collection_index].contains(**slot))
                .map(|slot| {
                    let record = &collection.generation.records[slot.0 as usize];
                    (slot.0, record.range)
                })
                .collect::<Vec<_>>();
            if let Some(context) = context
                && ranges
                    .iter()
                    .any(|(_, range)| range.len() > context.limits.source_record_bytes)
            {
                return Err(context.fail(
                    ExecutionErrorKind::Resource,
                    "merged base record exceeds source read allowance",
                ));
            }
            if ranges.is_empty() {
                continue;
            }
            let first = crate::relation_query::MergedRecordSlot {
                collection: collection_id,
                segment: None,
                slot: RecordSlot(ranges[0].0),
            };
            let resolved = view
                .record(first)?
                .ok_or("active merged base file missing")?;
            let path = repo.files.root.join(&resolved.file.relative_path);
            let mut reader =
                match RecordRangeReader::open_for_search(&path, resolved.file.revision, context) {
                    Ok(reader) => reader,
                    Err(message) => {
                        source_errors.record(SourceDiagnostic {
                            file_id: resolved.file.id.0,
                            file_path: resolved.file.relative_path.clone(),
                            kind: classify_source_read_failure_for_search(
                                &path,
                                resolved.file.revision,
                                None,
                            ),
                            message,
                        });
                        continue;
                    }
                };
            if let Some(context) = context {
                context.charge_reads(1)?;
            }
            opened.insert(file_id.0);
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
                let read = match reader.read(&ranges[start..cursor]) {
                    Ok((read, _)) => read,
                    Err(message) => {
                        source_errors.record(SourceDiagnostic {
                            file_id: resolved.file.id.0,
                            file_path: resolved.file.relative_path.clone(),
                            kind: classify_source_read_failure_for_search(
                                &path,
                                resolved.file.revision,
                                Some(ranges[start].1),
                            ),
                            message,
                        });
                        break;
                    }
                };
                for (index, (slot, bytes)) in read.into_iter().enumerate() {
                    process(
                        crate::relation_query::MergedRecordSlot {
                            collection: collection_id,
                            segment: None,
                            slot: RecordSlot(slot),
                        },
                        bytes,
                        start == 0 && index == 0,
                    )?;
                }
            }
        }
        let mut relation_prune: Option<Option<RelationTargetPrune>> = None;
        for (segment_index, published) in segments[collection_index].iter().enumerate() {
            let mut candidates = if prune_segments {
                let filter_may = filter
                    .as_ref()
                    .map(|filter| {
                        segment_filter_bounds_with_catalog(
                            &published.index,
                            &published.active,
                            filter,
                            collection,
                            Some(catalog),
                        )
                        .map(|bounds| bounds.may)
                    })
                    .transpose()?;
                let fts_may = fts
                    .as_ref()
                    .and_then(|fts| segment_fts_candidates(&published.index, collection, fts));
                match (filter_may, fts_may) {
                    (Some(filter), Some(fts)) => Some(filter & fts),
                    (Some(filter), None) => Some(filter),
                    (None, Some(fts)) => Some(fts),
                    (None, None) => None,
                }
            } else {
                None
            };
            if prune_segments {
                let local_count = candidates
                    .as_ref()
                    .map_or_else(|| published.active.len(), RoaringBitmap::len);
                if local_count > 256 && relation_prune.is_none() {
                    relation_prune = Some(relation_target_prune(
                        repo,
                        base_active,
                        segments,
                        filter.as_ref(),
                        fts.as_ref(),
                    )?);
                }
                if let Some(Some(plan)) = relation_prune.as_ref()
                    && let Some(relation_may) = relation_source_candidates(published, plan)
                {
                    candidates =
                        Some(candidates.map_or(relation_may.clone(), |may| may & relation_may));
                }
            }
            if candidates.as_ref().is_some_and(RoaringBitmap::is_empty) {
                continue;
            }
            for file in &published.index.files {
                if let Some(context) = context {
                    context.check_string()?;
                }
                let ranges = file
                    .records
                    .iter()
                    .filter(|slot| {
                        published.active.contains(**slot)
                            && candidates.as_ref().is_none_or(|may| may.contains(slot.0))
                    })
                    .map(|slot| {
                        let record = &published.index.records[slot.0 as usize];
                        (slot.0, record.range)
                    })
                    .collect::<Vec<_>>();
                if let Some(context) = context
                    && ranges
                        .iter()
                        .any(|(_, range)| range.len() > context.limits.source_record_bytes)
                {
                    return Err(context.fail(
                        ExecutionErrorKind::Resource,
                        "merged segment record exceeds source read allowance",
                    ));
                }
                if ranges.is_empty() {
                    continue;
                }
                let first = crate::relation_query::MergedRecordSlot {
                    collection: collection_id,
                    segment: Some(segment_index),
                    slot: RecordSlot(ranges[0].0),
                };
                let resolved = view
                    .record(first)?
                    .ok_or("active merged segment file missing")?;
                let path = repo.files.root.join(&resolved.file.relative_path);
                let mut reader = match RecordRangeReader::open_for_search(
                    &path,
                    resolved.file.revision,
                    context,
                ) {
                    Ok(reader) => reader,
                    Err(message) => {
                        source_errors.record(SourceDiagnostic {
                            file_id: resolved.file.id.0,
                            file_path: resolved.file.relative_path.clone(),
                            kind: classify_source_read_failure_for_search(
                                &path,
                                resolved.file.revision,
                                None,
                            ),
                            message,
                        });
                        continue;
                    }
                };
                if let Some(context) = context {
                    context.charge_reads(1)?;
                }
                opened.insert(file.entry.id.0);
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
                    let read = match reader.read(&ranges[start..cursor]) {
                        Ok((read, _)) => read,
                        Err(message) => {
                            source_errors.record(SourceDiagnostic {
                                file_id: resolved.file.id.0,
                                file_path: resolved.file.relative_path.clone(),
                                kind: classify_source_read_failure_for_search(
                                    &path,
                                    resolved.file.revision,
                                    Some(ranges[start].1),
                                ),
                                message,
                            });
                            break;
                        }
                    };
                    for (index, (slot, bytes)) in read.into_iter().enumerate() {
                        process(
                            crate::relation_query::MergedRecordSlot {
                                collection: collection_id,
                                segment: Some(segment_index),
                                slot: RecordSlot(slot),
                            },
                            bytes,
                            start == 0 && index == 0,
                        )?;
                    }
                }
            }
        }
    }
    let mut page = winners
        .into_sorted_vec()
        .into_iter()
        .skip(query.options.offset)
        .take(query.options.limit)
        .collect::<Vec<_>>();
    if query.options.return_mode != ReturnMode::Locator {
        let mut retained = Vec::with_capacity(page.len());
        for mut winner in page {
            if let Some(context) = context {
                context.check_string()?;
                context.charge_reads(1)?;
                context.charge_bytes(winner.hit.byte_end.saturating_sub(winner.hit.byte_start))?;
            }
            let payload_hit = if let Some((collection_index, segment_index)) = winner.segment {
                let published = &segments[collection_index][segment_index];
                get_segment_record_by_id(
                    repo,
                    collection_index,
                    &published.index,
                    &published.active,
                    catalog,
                    revision,
                    &winner.hit.record_id,
                    query.options.return_mode.clone(),
                )
            } else {
                repo.get_record_with_active(
                    &winner.hit.collection,
                    &winner.hit.record_id,
                    query.options.return_mode.clone(),
                    Some(base_active),
                )
            };
            let mut payload_hit = match payload_hit {
                Ok(Some(payload_hit)) => payload_hit,
                Ok(None) => return Err("selected record retired in pinned view".into()),
                Err(message) => {
                    if record_unavailable_hit(
                        repo,
                        catalog,
                        &winner.hit,
                        message.clone(),
                        &mut counters,
                    )? {
                        continue;
                    }
                    return Err(message);
                }
            };
            payload_hit.generation = revision.0;
            payload_hit.matched_fields = winner.hit.matched_fields.clone();
            winner.hit = payload_hit;
            retained.push(winner);
        }
        page = retained;
    }
    counters.total_matches = total;
    source_errors.apply(&mut counters);
    view.take_source_errors().apply(&mut counters);
    counters.match_count_complete = counters.source_unavailable_events == 0;
    counters.returned_records = page.len() as u64;
    counters.distinct_files = opened.len() as u64;
    counters.sort_target_reads += sort_cache.reads;
    counters.sort_target_reuse += sort_cache.reuse;
    counters.relation_edge_visits += sort_cache.edge_visits;
    counters.files_opened += sort_cache.reads;
    counters.record_parses += sort_cache.reads;
    counters.bytes_read += sort_cache.bytes_read;
    Ok(SearchResult {
        count: (query.options.count && counters.match_count_complete).then_some(total as usize),
        hits: page.into_iter().map(|winner| winner.hit).collect(),
        counters,
    })
}

/// Scan the pinned base and segment domains without posting pruning. This is
/// the independent candidate path used to check merged planner results.
pub(crate) fn search_segmented_reference(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
    revision: IndexRevision,
) -> Result<SearchResult, String> {
    if segments.len() != repo.collections.len() || base_active.len() != repo.collections.len() {
        return Err("segment query collection count mismatch".into());
    }
    let sort = ResolvedSort::new(repo, query)?;
    let (prepared, _) = crate::prepared::prepare(repo, query)?;
    search_segmented_relations(
        repo,
        query,
        base_active,
        segments,
        catalog,
        revision,
        &sort,
        &prepared.collections,
        None,
        None,
        None,
        false,
    )
}

pub(crate) fn explain_segmented(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    revision: IndexRevision,
) -> Result<Vec<QueryPlanTrace>, String> {
    if segments.len() != repo.collections.len() || base_active.len() != repo.collections.len() {
        return Err("segment explanation collection count mismatch".into());
    }
    let mut traces = explain_with_active(repo, query, Some(base_active))?;
    for trace in &mut traces {
        let collection_index = repo
            .collections
            .iter()
            .position(|collection| collection.definition.name == trace.collection)
            .ok_or("explained collection missing")?;
        let delta = segments[collection_index]
            .iter()
            .map(|segment| segment.active.len())
            .fold(0u64, u64::saturating_add);
        trace.generation = revision.0;
        trace.input_records = trace.input_records.saturating_add(delta);
        trace.candidates = trace.candidates.saturating_add(delta);
        trace.fts_estimated_candidates = trace
            .fts_estimated_candidates
            .map(|estimate| estimate.saturating_add(delta));
        trace.candidates_estimated = true;
        if delta > 0 {
            trace.sort_strategy = "merged base and indexed segment top-k".into();
            trace.sort_strategy_estimated = true;
        }
        if delta > 0 && (query.filter.is_some() || query.fts.is_some()) {
            trace.filter_exact = false;
            trace.remaining_verification = true;
        }
    }
    Ok(traces)
}

pub(crate) fn search_analyze_segmented(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
    revision: IndexRevision,
) -> Result<(SearchResult, Vec<QueryPlanTrace>), String> {
    let mut traces = explain_segmented(repo, query, base_active, segments, revision)?;
    let result = search_segmented(repo, query, base_active, segments, catalog, revision)?;
    if traces.len() == 1 {
        let trace = &mut traces[0];
        let counters = &result.counters;
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
            relation_verified_targets_reused: counters.relation_verified_targets_reused,
        });
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
    materialized.generation = revision.0;
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
fn read_lifecycle_ordered_record(
    repo: &Repository,
    _catalog: &FileCatalog,
    file: &FileEntry,
    record: &RecordEntry,
    slot: RecordSlot,
    reader: &mut Option<(crate::model::FileId, RecordRangeReader)>,
    max_bytes: usize,
    context: &ExecutionContext,
) -> Result<(Vec<u8>, bool), String> {
    let opened = reader.as_ref().is_none_or(|(id, _)| *id != file.id);
    if opened {
        let path = repo.files.root.join(&file.relative_path);
        *reader = Some((
            file.id,
            RecordRangeReader::open_for_search(&path, file.revision, Some(context))?,
        ));
    }
    let (_, source) = reader.as_mut().ok_or("ordered source reader missing")?;
    let (mut rows, _) = source.read_bounded(&[(slot.0, record.range)], max_bytes)?;
    Ok((rows.pop().ok_or("ordered source row missing")?.1, opened))
}

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
    if sort.bound.has_computed {
        return Ok(None);
    }
    let (prepared, _) = crate::prepared::prepare(repo, request)?;
    if prepared.collections.iter().any(|(_, filter, fts)| {
        filter.as_ref().is_some_and(bound_expr_has_relations)
            || fts
                .as_ref()
                .is_some_and(|fts| fts.fields.iter().any(|field| field.related.is_some()))
    }) {
        return Ok(None);
    }
    for (collection_index, _, _) in &prepared.collections {
        for segment in segments[*collection_index].iter() {
            if lifecycle_segment_order(&segment.index, &sort, request, *collection_index).is_none()
            {
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
    for (collection_index, filter, fts) in prepared.collections {
        for (segment_index, segment) in segments[collection_index].iter().enumerate() {
            let collection = &repo.collections[collection_index];
            let filter_may = filter
                .as_ref()
                .map(|filter| {
                    segment_filter_bounds_with_catalog(
                        &segment.index,
                        &segment.active,
                        filter,
                        collection,
                        Some(catalog),
                    )
                    .map(|bounds| bounds.may)
                })
                .transpose()?;
            let fts_may = fts
                .as_ref()
                .and_then(|fts| segment_fts_candidates(&segment.index, collection, fts));
            candidates[collection_index][segment_index] = match (filter_may, fts_may) {
                (Some(filter), Some(fts)) => Some(filter & fts),
                (Some(filter), None) => Some(filter),
                (None, Some(fts)) => Some(fts),
                (None, None) => None,
            };
        }
        filters[collection_index] = Some((filter, fts));
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
        let must_verify = filter.is_some() || fts.is_some();
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
                    read
                }
                Err(message) => {
                    let locator = segment_locator_hit(collection, resolved, revision, Vec::new());
                    if record_unavailable_hit(
                        repo,
                        catalog,
                        &locator,
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
                request.options.evidence,
                Some(context),
            )?;
            counters.raw_text_verifications += u64::from(raw_checked);
            evidence = found_evidence;
            bytes = Some(read);
            matched
        } else {
            counters.exact_posting_matches += 1;
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
            resolved,
            revision,
            if request.options.evidence {
                evidence.into_iter().collect()
            } else {
                Vec::new()
            },
        );
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
                            if record_unavailable_hit(
                                repo,
                                catalog,
                                &hit,
                                message.clone(),
                                &mut counters,
                            )? {
                                continue;
                            }
                            return Err(message);
                        }
                    };
                    counters.files_opened += u64::from(opened);
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
        let bytes = crate::streaming::hit_bytes(&hit);
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
        if let Some(limit) = limit
            && page.len() == limit
        {
            match delivery.options().mode {
                StreamMode::OrderedRows => delivery.ordered_rows(&page, &counters, true)?,
                StreamMode::PageReplacements => {
                    delivery.replacement(&page, &counters, heap_bytes)?
                }
            }
            stopped_early = true;
            break;
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
            &[],
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
        &[],
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
    batches: &[Arc<Vec<Arc<ExtendedBatch>>>],
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
        && options.mode != crate::streaming::StreamMode::OrderedRows
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
    if let ResultScope::Page { .. } = scope
        && let Some(cache) = cache
        && context.limits == ExecutionLimits::default()
    {
        let key = crate::verified::lifecycle_key(&request, cache.instance_id, revision.0).map_err(
            |message| StreamError::Execution {
                kind: ExecutionErrorKind::Execution,
                message,
                progress: delivery.progress,
            },
        )?;
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
            let result = match result {
                Ok(result) => result,
                Err(message) => return segmented_stream_error(&mut delivery, context, message),
            };
            if let Err(message) = delivery
                .check_buffer(result.hits.iter().map(crate::streaming::hit_bytes).sum())
                .and_then(|_| match options.mode {
                    StreamMode::OrderedRows => {
                        delivery.ordered_rows(&result.hits, &result.counters, true)
                    }
                    StreamMode::PageReplacements => {
                        delivery.replacement(&result.hits, &result.counters, 0)
                    }
                })
            {
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
    let mut result = match search_segmented_with_context_lifecycle_budget(
        repo,
        &request,
        base_active,
        segments,
        catalog,
        revision,
        context,
        batches,
        cache,
        all.then(|| delivery.max_buffered_bytes() / 2),
    ) {
        Ok(result) => result,
        Err(error) => return segmented_stream_error(&mut delivery, context, error.message),
    };
    if let Err(message) = delivery.check_buffer(result.hits.iter().fold(0usize, |bytes, hit| {
        bytes.saturating_add(crate::streaming::hit_bytes(hit))
    })) {
        return segmented_stream_error(&mut delivery, context, message);
    }
    if all && result.counters.match_count_complete {
        result.count = usize::try_from(result.counters.total_matches).ok();
    }
    if all {
        let mut batch = Vec::new();
        let mut batch_bytes = 0usize;
        for hit in std::mem::take(&mut result.hits)
            .into_iter()
            .skip(scope.offset())
        {
            if let Err(message) = delivery.check() {
                return segmented_stream_error(&mut delivery, context, message);
            }
            let locator = hit.clone();
            let hit = match materialize_segmented_stream_hit(
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
                        &locator,
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
            if query.options.return_mode != ReturnMode::Locator {
                result.counters.files_opened += 1;
                result.counters.bytes_read += hit.byte_end.saturating_sub(hit.byte_start);
                result.counters.record_parses += u64::from(
                    query.options.return_mode == ReturnMode::Logical
                        && hit.record_class == RecordClass::Logical,
                );
            }
            let size = crate::streaming::hit_bytes(&hit);
            if !batch.is_empty()
                && (batch.len() >= options.batch_rows
                    || batch_bytes.saturating_add(size) > options.max_event_bytes)
            {
                if let Err(message) = delivery.ordered_batch(&batch, &result.counters) {
                    return segmented_stream_error(&mut delivery, context, message);
                }
                batch.clear();
                batch_bytes = 0;
            }
            batch_bytes = batch_bytes.saturating_add(size);
            batch.push(hit);
            if let Err(message) = delivery.check_buffer(batch_bytes) {
                return segmented_stream_error(&mut delivery, context, message);
            }
        }
        if let Err(message) = delivery.ordered_batch(&batch, &result.counters) {
            return segmented_stream_error(&mut delivery, context, message);
        }
        result.counters.returned_records = delivery.progress.emitted_rows;
    } else {
        let delivered = match options.mode {
            crate::streaming::StreamMode::OrderedRows => {
                delivery.ordered_rows(&result.hits, &result.counters, true)
            }
            crate::streaming::StreamMode::PageReplacements => {
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
    delivery.observe(&result.counters);
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

fn offer(
    repo: &Repository,
    query: &Query,
    sort: &ResolvedSort<'_>,
    collection_index: usize,
    record: &RecordEntry,
    evidence: HashSet<String>,
    payload: Option<Payload>,
    key_override: Option<RankKey>,
    winners: &mut BinaryHeap<Winner>,
    retained_key_bytes: &mut u64,
    cap: usize,
    counters: &mut WorkCounters,
    mut cache_members: Option<&mut Vec<CachedMember>>,
    cache_budget: Option<&CacheBudget>,
) -> Result<(), String> {
    counters.total_matches += 1;
    let mut matched_fields: Vec<_> = if query.options.evidence {
        evidence.into_iter().collect()
    } else {
        Vec::new()
    };
    matched_fields.sort();
    if let (Some(members), Some(budget)) = (cache_members.as_deref_mut(), cache_budget) {
        if budget.reserve(&matched_fields) {
            members.push(CachedMember {
                collection_index,
                segment_index: None,
                slot: record.slot.0,
                matched_fields: matched_fields.clone(),
            });
        }
    }
    let enters = if let Some(key) = &key_override {
        cap > 0 && (winners.len() < cap || winners.peek().is_some_and(|worst| key < &worst.key))
    } else {
        would_enter_page(repo, sort, collection_index, record, winners, cap)
    };
    if !enters {
        return Ok(());
    }
    let key = if let Some(key) = key_override {
        key
    } else if sort.bound.has_computed {
        if sort.bound.needs_source {
            return Err("computed sort key needs source evaluation".into());
        }
        rank_key_without_source(sort, repo, collection_index, record)?
    } else {
        rank_key(sort, repo, collection_index, record)
    };
    if winners.len() == cap {
        let old = winners.pop().unwrap();
        *retained_key_bytes -= rank_key_bytes(&old.key);
    }
    *retained_key_bytes = retained_key_bytes
        .checked_add(rank_key_bytes(&key))
        .ok_or("sort key byte count overflow")?;
    if *retained_key_bytes > MAX_SORT_KEY_BYTES {
        return Err("sort key memory budget exceeded".into());
    }
    counters.peak_retained_sort_key_bytes = counters
        .peak_retained_sort_key_bytes
        .max(*retained_key_bytes);
    let retained_bytes = if payload.is_some() {
        if sort.bound.has_computed && query.options.return_mode == ReturnMode::Logical {
            record
                .range
                .len()
                .checked_mul(4)
                .ok_or("sort payload byte estimate overflow")?
        } else {
            record.range.len()
        }
    } else {
        0
    };
    let hit = make_hit(repo, collection_index, record, matched_fields, payload);
    winners.push(Winner {
        key,
        collection_index,
        slot: record.slot.0,
        hit,
        retained_bytes,
    });
    Ok(())
}

fn would_enter_page(
    repo: &Repository,
    sort: &ResolvedSort<'_>,
    collection_index: usize,
    record: &RecordEntry,
    winners: &BinaryHeap<Winner>,
    cap: usize,
) -> bool {
    cap > 0
        && (winners.len() < cap
            || winners.peek().is_some_and(|worst| {
                sort.compare_to_rank(
                    collection_index,
                    record,
                    &repo.collections[collection_index].definition.name,
                    &worst.key,
                )
                .is_lt()
            }))
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
    search_active_inner(repo, query, active, None, None, None, None)
}

pub(crate) fn search_active_extended(
    repo: &Repository,
    query: &Query,
    active: &[ActiveRecords],
    batches: &[Arc<Vec<Arc<ExtendedBatch>>>],
    context: Option<&ExecutionContext>,
) -> Result<SearchResult, String> {
    search_active_inner(repo, query, active, context, Some(batches), None, None)
}

fn search_active_inner(
    repo: &Repository,
    query: &Query,
    active: &[ActiveRecords],
    context: Option<&ExecutionContext>,
    extended_batches: Option<&[Arc<Vec<Arc<ExtendedBatch>>>]>,
    cache_members: Option<&mut Vec<CachedMember>>,
    cache_budget: Option<&CacheBudget>,
) -> Result<SearchResult, String> {
    let scoped_context = context
        .cloned()
        .unwrap_or_else(|| {
            ExecutionContext::new(
                crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
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
        cache_members,
        cache_budget,
        None,
        None,
        None,
        (query.options.count && query.options.limit == 0)
            .then_some(ExactCollector { members: None }),
        Some(&scoped_context),
        Some(active),
        extended_batches,
    )
}

pub(crate) fn search_with_context_active(
    repo: &Repository,
    query: &Query,
    context: &ExecutionContext,
    active: &[ActiveRecords],
) -> Result<SearchResult, crate::streaming::ExecutionError> {
    context.check()?;
    search_active_inner(repo, query, active, Some(context), None, None, None)
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
            None,
        );
    }
    if posting_order_eligible(repo, query) {
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
    let key = crate::verified::key(&membership_query)?;
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
) -> Result<SearchResult, crate::streaming::ExecutionError> {
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
        crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
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
    let inferred_order = posting_order_eligible(repo, execution_query);
    let bound_sort =
        BoundSort::bind(repo, execution_query).map_err(|message| StreamError::Execution {
            kind: crate::streaming::ExecutionErrorKind::InvalidQuery,
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
            kind: crate::streaming::ExecutionErrorKind::Execution,
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
            crate::verified::key(&membership_query).map_err(|message| StreamError::Execution {
                kind: crate::streaming::ExecutionErrorKind::Execution,
                message,
                progress: delivery.progress,
            })?,
        )
    } else {
        None
    };
    let cached = if let Some(key) = &cache_key {
        repo.verified
            .lock()
            .map_err(|error| StreamError::Execution {
                kind: crate::streaming::ExecutionErrorKind::Execution,
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
            None,
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
        .check_buffer(result.hits.iter().map(crate::streaming::hit_bytes).sum())
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
                kind: crate::streaming::ExecutionErrorKind::Execution,
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
                    kind: crate::streaming::ExecutionErrorKind::Execution,
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
            total.saturating_add(crate::streaming::hit_bytes(prior))
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
            total.saturating_add(crate::streaming::hit_bytes(row))
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
            crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
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
        None,
    )?;
    if result.count != Some(members.len() as usize) {
        return Err("exact selection is incomplete".into());
    }
    Ok(members)
}

fn sort_cached_members(repo: &Repository, query: &Query, members: &mut Vec<CachedMember>) {
    let sort = ResolvedSort::new(repo, query).expect("sort validated");
    if query.options.sort.is_empty() {
        members.sort_by(|a, b| {
            let a_collection = &repo.collections[a.collection_index];
            let b_collection = &repo.collections[b.collection_index];
            (
                &a_collection.definition.name,
                &a_collection.generation.records[a.slot as usize].id.0,
            )
                .cmp(&(
                    &b_collection.definition.name,
                    &b_collection.generation.records[b.slot as usize].id.0,
                ))
        });
    } else {
        members.sort_by(|a, b| {
            let a_collection = &repo.collections[a.collection_index];
            let b_collection = &repo.collections[b.collection_index];
            let a_record = &a_collection.generation.records[a.slot as usize];
            let b_record = &b_collection.generation.records[b.slot as usize];
            for ((a_column, descending), (b_column, _)) in sort.columns[a.collection_index]
                .as_ref()
                .unwrap()
                .iter()
                .zip(sort.columns[b.collection_index].as_ref().unwrap())
            {
                let order = a_column.keys[a.slot as usize].compare_with(
                    &b_column.keys[b.slot as usize],
                    *descending,
                    a_column.options,
                );
                if !order.is_eq() {
                    return order;
                }
            }
            (&a_collection.definition.name, &a_record.id)
                .cmp(&(&b_collection.definition.name, &b_record.id))
        });
    }
}


struct StoredCursor<'a> {
    columns: &'a [(&'a SortColumn, bool)],
    order: &'a [crate::model::RecordSlot],
    record: &'a RecordEntry,
    collection_name: &'a str,
    collection_index: usize,
    plan_index: usize,
    position: usize,
}


impl PartialEq for StoredCursor<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}


impl Eq for StoredCursor<'_> {}


impl PartialOrd for StoredCursor<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}


impl Ord for StoredCursor<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        let mut keys = Ordering::Equal;
        for ((a, descending), (b, _)) in self.columns.iter().zip(other.columns) {
            keys = a.keys[self.record.slot.0 as usize].compare_with(
                &b.keys[other.record.slot.0 as usize],
                *descending,
                a.options,
            );
            if !keys.is_eq() {
                break;
            }
        }
        keys.then_with(|| {
            (self.collection_name, &self.record.id).cmp(&(other.collection_name, &other.record.id))
        })
    }
}


fn stored_cursor<'a>(
    repo: &'a Repository,
    columns: &'a [(&'a SortColumn, bool)],
    order: &'a [crate::model::RecordSlot],
    collection_index: usize,
    plan_index: usize,
    position: usize,
) -> Option<StoredCursor<'a>> {
    let slot = order.get(position)?;
    let collection = &repo.collections[collection_index];
    Some(StoredCursor {
        columns,
        order,
        record: &collection.generation.records[slot.0 as usize],
        collection_name: &collection.definition.name,
        collection_index,
        plan_index,
        position,
    })
}


struct OrderedPlan<'a> {
    collection_index: usize,
    filter: Option<&'a BoundExpr>,
    fts: Option<&'a BoundFts>,
    plan: Plan,
    candidates: RoaringBitmap,
}


/// Membership already prepared for a stored-order attempt whose selective
/// result is cheaper to rank with the general top-k collector.
struct RejectedOrderPlans {
    entries: Vec<(Plan, RoaringBitmap)>,
    planning_ns: u64,
}


fn direct_payload(
    repo: &Repository,
    query: &Query,
    collection_index: usize,
    record: &RecordEntry,
    counters: &mut WorkCounters,
    actual: &mut PlanActual,
    source_access: &mut SourceAccessSession<'_>,
    stream_limit: Option<usize>,
    execution: Option<&ExecutionContext>,
) -> Result<Option<Payload>, String> {
    if query.options.return_mode == ReturnMode::Locator {
        return Ok(None);
    }
    if query.options.return_mode == ReturnMode::Logical && record.class == RecordClass::RawFallback
    {
        return Ok(Some(Payload::LogicalUnavailable {
            diagnostic_ref: record.diagnostic_ref,
        }));
    }
    let collection = &repo.collections[collection_index];
    let mut read = if let Some(limit) = stream_limit {
        source_access.read_bounded(record.file, &[(0, record.range)], limit)?
    } else {
        source_access.read(record.file, &[(0, record.range)])?
    };
    let bytes = read.records.pop().ok_or("source record missing")?.1;
    counters.files_opened += read.opened;
    counters.bytes_read += read.bytes_read;
    actual.files_opened += read.opened;
    actual.bytes_read += read.bytes_read;
    let (payload, parses) = payload_from_bytes_with_execution(
        record.class,
        Some(collection),
        &query.options.return_mode,
        bytes,
        execution,
    )?;
    counters.record_parses += parses;
    actual.record_parses += parses;
    Ok(payload)
}

fn check_stream_page_read(
    delivery: &mut Delivery<'_>,
    query: &Query,
    page: &[SearchHit],
    record: &RecordEntry,
) -> Result<(), String> {
    let source_bytes = usize::try_from(record.range.len()).unwrap_or(usize::MAX);
    delivery.check_record_bytes(source_bytes)?;
    let payload_bytes = match query.options.return_mode {
        ReturnMode::Locator => 0,
        ReturnMode::Bytes => source_bytes,
        ReturnMode::Logical => source_bytes.saturating_mul(4),
    };
    let current = page.iter().fold(0usize, |total, hit| {
        total.saturating_add(crate::streaming::hit_bytes(hit))
    });
    delivery.check_buffer(current.saturating_add(payload_bytes))
}

fn prepare_all_batch(
    delivery: &mut Delivery<'_>,
    page: &mut Vec<SearchHit>,
    counters: &WorkCounters,
    record: &RecordEntry,
) -> Result<(), String> {
    let current = page.iter().fold(0usize, |total, hit| {
        total.saturating_add(crate::streaming::hit_bytes(hit))
    });
    let next = usize::try_from(record.range.len()).unwrap_or(usize::MAX);
    if !page.is_empty()
        && (page.len() >= delivery.options().batch_rows
            || current.saturating_add(next) >= delivery.options().max_event_bytes
            || current.saturating_add(next) >= delivery.max_buffered_bytes())
    {
        delivery.ordered_batch(page, counters)?;
        page.clear();
    }
    Ok(())
}

fn flush_all_batch_if_full(
    delivery: &mut Delivery<'_>,
    batch: &mut Vec<SearchHit>,
    counters: &WorkCounters,
) -> Result<(), String> {
    let mut bytes = batch.iter().fold(0usize, |total, hit| {
        total.saturating_add(crate::streaming::hit_bytes(hit))
    });
    if bytes > delivery.options().max_event_bytes && batch.len() > 1 {
        let last = batch.pop().ok_or("all-results batch missing final row")?;
        delivery.ordered_batch(batch, counters)?;
        batch.clear();
        bytes = crate::streaming::hit_bytes(&last);
        batch.push(last);
    }
    if batch.len() >= delivery.options().batch_rows || bytes > delivery.options().max_event_bytes {
        delivery.ordered_batch(batch, counters)?;
        batch.clear();
    }
    Ok(())
}

fn inferred_order_index<'a>(
    repo: &'a Repository,
    query: &Query,
) -> Option<(usize, FieldPath, &'a PostingIndex)> {
    if query.options.sort.len() != 1 {
        return None;
    }
    let part = simple_composite_part(&query.options.sort[0])?;
    if part.path.root != PathRoot::Data {
        return None;
    }
    let mut selected = repo
        .collections
        .iter()
        .enumerate()
        .filter(|(_, collection)| {
            query.collections.is_empty() || query.collections.contains(&collection.definition.name)
        });
    let (collection_index, collection) = selected.next()?;
    if selected.next().is_some()
        || collection.sorting.columns.contains_key(&part.path)
        || collection
            .definition
            .schema
            .defaults
            .get(&part.path.segments)
            .is_some_and(|name| name != "default")
    {
        return None;
    }
    let index = crate::index_catalog::IndexCatalog::collection(collection)
        .sortable_scalar(&part.path.segments)?;
    Some((collection_index, part.path, index))
}

/// A posting order is useful for an unrestricted page or a filter whose
/// selected access already proves membership. With a residual FILTER/FTS,
/// filtering first preserves the verified membership cache and avoids an
/// ordered source scan across candidates that will later be rejected.
fn posting_order_eligible(repo: &Repository, query: &Query) -> bool {
    let Some((collection_index, _, _)) = inferred_order_index(repo, query) else {
        return false;
    };
    if query.fts.is_some() {
        return false;
    }
    if query.filter.is_none() {
        return true;
    }
    let Ok((prepared, _)) = crate::prepared::prepare(repo, query) else {
        return false;
    };
    let Some((_, Some(filter), _)) = prepared.collections.iter()
        .find(|(index, _, _)| *index == collection_index)
    else {
        return false;
    };
    let collection = &repo.collections[collection_index];
    estimate_plan_expr(collection, filter, collection.logical_records.len()).exact
}

fn inferred_order_column(
    repo: &Repository,
    query: &Query,
    prepared: &crate::prepared::PreparedQuery,
    execution: Option<&ExecutionContext>,
) -> Result<Option<(usize, Arc<SortColumn>)>, String> {
    let Some((collection_index, path, index)) = inferred_order_index(repo, query) else {
        return Ok(None);
    };
    if prepared.collections.len() != 1 || prepared.collections[0].0 != collection_index {
        return Ok(None);
    }
    let collection = &repo.collections[collection_index];
    let key = InferredOrderKey::new(
        collection_index,
        collection.generation.generation,
        path.clone(),
        &collection.definition.sort_locale,
        collection.definition.sort_options,
        query.options.sort[0].descending,
    );
    let column = cached_inferred_order(&repo.inferred_orders, key, || {
        #[cfg(test)]
        INFERRED_ORDER_BUILDS.with(|count| count.set(count.get() + 1));
        SortColumn::from_inferred_posting(
            index,
            &collection.generation,
            path,
            &collection.definition.sort_locale,
            collection.definition.sort_options,
            query.options.sort[0].descending,
            execution,
        )
    })?;
    if let (Some(column), Some(execution)) = (&column, execution) {
        execution.check_string()?;
        if column.retained_bytes() > execution.limits.inferred_order_bytes {
            return Err(execution.fail(
                ExecutionErrorKind::Resource,
                "inferred order exceeds retained-byte limit",
            ));
        }
    }
    Ok(column.map(|column| (collection_index, column)))
}

fn cached_inferred_order(
    cache: &Mutex<crate::sorting::InferredOrderCache>,
    key: InferredOrderKey,
    build: impl FnOnce() -> Result<Option<SortColumn>, String>,
) -> Result<Option<Arc<SortColumn>>, String> {
    if let Some(column) = cache.lock().map_err(|error| error.to_string())?.get(&key) {
        return Ok(Some(column));
    }
    let Some(column) = build()? else {
        return Ok(None);
    };
    let column = cache
        .lock()
        .map_err(|error| error.to_string())?
        .insert_if_absent(key, Arc::new(column));
    Ok(Some(column))
}

#[cfg(test)]
thread_local! {
    static INFERRED_ORDER_BUILDS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn inferred_order_builds_for_test() -> u64 {
    INFERRED_ORDER_BUILDS.with(std::cell::Cell::get)
}

#[allow(clippy::too_many_arguments)]
fn try_inferred_order_page(
    repo: &Repository,
    query: &Query,
    prepared: &crate::prepared::PreparedQuery,
    prepared_plan_hit: bool,
    preparation_ns: u64,
    traces: Option<&mut Vec<QueryPlanTrace>>,
    mut delivery: Option<&mut Delivery<'_>>,
    all_offset: Option<usize>,
    execution: Option<&ExecutionContext>,
    active: Option<&[ActiveRecords]>,
) -> Result<Option<SearchResult>, String> {
    if !posting_order_eligible(repo, query) {
        return Ok(None);
    }
    if active.is_some() {
        // A static inferred column may read retired source records while it is built.
        return Ok(None);
    }
    let Some((collection_index, column)) = inferred_order_column(repo, query, prepared, execution)?
    else {
        return Ok(None);
    };
    let terms = (0..repo.collections.len())
        .map(|position| {
            (position == collection_index).then(|| {
                vec![BoundSortTerm {
                    source: BoundSortSource::Stored(column.as_ref()),
                    descending: query.options.sort[0].descending,
                    options: column.options,
                }]
            })
        })
        .collect();
    let sort = ResolvedSort::from_bound(
        repo,
        query,
        BoundSort {
            terms,
            has_computed: false,
            needs_source: false,
        },
    );
    let mut local_traces = Vec::new();
    let result = try_stored_order_page(
        repo,
        query,
        prepared,
        &sort,
        prepared_plan_hit,
        preparation_ns,
        traces.as_ref().map(|_| &mut local_traces),
        delivery.as_deref_mut(),
        all_offset,
        true,
        None,
        execution,
        active,
    )?;
    if result.is_some() {
        if let Some(traces) = traces {
            for trace in &mut local_traces {
                trace.sort_strategy = "inferred_posting_order_page".into();
            }
            traces.extend(local_traces);
        }
    }
    Ok(result)
}

fn try_stored_order_page(
    repo: &Repository,
    query: &Query,
    prepared: &crate::prepared::PreparedQuery,
    sort: &ResolvedSort<'_>,
    prepared_plan_hit: bool,
    preparation_ns: u64,
    mut traces: Option<&mut Vec<QueryPlanTrace>>,
    mut delivery: Option<&mut Delivery<'_>>,
    all_offset: Option<usize>,
    force_order: bool,
    mut rejected: Option<&mut RejectedOrderPlans>,
    execution: Option<&ExecutionContext>,
    active: Option<&[ActiveRecords]>,
) -> Result<Option<SearchResult>, String> {
    if let Some(execution) = execution {
        execution.check_string()?;
    }
    if sort.bound.has_computed || (query.options.count && delivery.is_none() && !force_order) {
        return Ok(None);
    }
    if query.options.sort.len() > 1
        && prepared.collections.iter().any(|(index, _, _)| {
            repo.collections[*index]
                .sorting
                .composite(&query.options.sort)
                .is_none()
        })
    {
        return Ok(None);
    }
    if all_offset.is_none()
        && (prepared.collections.iter().any(|(_, _, fts)| {
            fts.as_ref()
                .is_some_and(|fts| fts.fields.iter().any(|field| field.related.is_some()))
        }) || prepared.collections.iter().any(|(_, filter, _)| {
            filter
                .as_ref()
                .is_some_and(crate::relation_target_cache::has_source_dependent)
        }))
    {
        return Ok(None);
    }
    let cap = query
        .options
        .limit
        .checked_add(query.options.offset)
        .ok_or("page size overflow")?;
    let planning_started = Instant::now();
    let diagnostics = traces.is_some();
    let mut plans = Vec::new();
    let mut candidate_total = 0u64;
    for (collection_index, filter, fts) in &prepared.collections {
        let collection = &repo.collections[*collection_index];
        let domain = collection_domain(collection, *collection_index, active);
        let mut plan = filter
            .as_ref()
            .map(|expr| {
                plan_expr_restricted_with_execution(
                    repo,
                    collection,
                    expr,
                    &domain,
                    &domain,
                    query.options.evidence,
                    diagnostics,
                    execution,
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
        let mut candidates = plan.candidates.take().unwrap_or_else(|| domain.clone());
        candidates &= &domain;
        if let Some(fts_candidates) = fts.as_ref().and_then(|fts| fts_candidates(collection, fts)) {
            candidates &= fts_candidates;
        }
        candidate_total += candidates.len();
        plans.push(OrderedPlan {
            collection_index: *collection_index,
            filter: filter.as_ref(),
            fts: fts.as_ref(),
            plan,
            candidates,
        });
    }
    // For a selective membership set, ranking the matches is cheaper than
    // traversing the full stored list. Zero matches need no traversal.
    if !force_order
        && delivery.is_none()
        && cap > 0
        && candidate_total > 0
        && candidate_total < (cap as u64).saturating_mul(4)
    {
        if let Some(rejected) = rejected.as_deref_mut() {
            rejected.entries = plans
                .into_iter()
                .map(|entry| (entry.plan, entry.candidates))
                .collect();
            rejected.planning_ns = planning_started.elapsed().as_nanos() as u64;
        }
        return Ok(None);
    }
    let residuals = plans
        .iter()
        .map(|entry| {
            entry.filter.map(|expr| {
                prepare_residual(
                    expr,
                    &entry.plan.proofs,
                    &repo.collections[entry.collection_index],
                    entry.candidates.len(),
                )
            })
        })
        .collect::<Vec<_>>();
    let projections = plans
        .iter()
        .zip(&residuals)
        .map(|(entry, residual)| {
            let mut projection =
                scan_projection(query, residual.as_ref(), &entry.candidates, false);
            prepare_value_text_projection(
                &mut projection,
                borrowed_value_text(&repo.collections[entry.collection_index], entry.fts)
                    || borrowed_relation_source_text(
                        &repo.collections[entry.collection_index],
                        entry.fts,
                    ),
            );
            projection
        })
        .collect::<Vec<_>>();
    let direct_plans = plans
        .iter()
        .zip(&residuals)
        .map(|(entry, residual)| {
            DirectSourcePlan::prepare(
                &repo.collections[entry.collection_index],
                sort,
                entry.fts,
                residual.as_ref(),
            )
        })
        .collect::<Vec<_>>();
    if let Some(traces) = traces.as_deref_mut() {
        for entry in &plans {
            let collection = &repo.collections[entry.collection_index];
            traces.push(QueryPlanTrace {
                collection: collection.definition.name.clone(),
                generation: collection.generation.generation,
                bigram_alphabet: collection.raw.alphabet,
                folded_representation: crate::bigram::FOLDED_REPRESENTATION,
                fts_case_sensitivity: entry.fts.map(|fts| fts.case_sensitivity),
                fts_mode: entry.fts.map(|fts| fts.mode),
                fts_exact_literal: entry.fts.map(|fts| fts.source_literal.clone()),
                fts_prefilter_reason: regex_prefilter_reason(collection, entry.fts),
                fts_covered_target: fts_covered_target(entry.fts),
                fts_estimated_candidates: entry.fts.and_then(|fts| {
                    estimate_fts_candidates(collection, fts).map(|estimate| estimate.0)
                }),
                remaining_verification: entry.fts.is_some() || !entry.plan.exact,
                input_records: collection_domain(collection, entry.collection_index, active).len(),
                fts_strategy: if collection.logical_records.len()
                    != collection.generation.records.len() as u64
                    && entry.fts.is_some_and(|fts| fts.fields.is_empty())
                {
                    "stored order: logical text plus fallback source text"
                } else {
                    "stored order page traversal"
                }
                .into(),
                normalized_sort: query.options.sort.clone(),
                sort_strategy: if query.options.sort.len() > 1 {
                    "stored_composite_order_page"
                } else {
                    "stored_order_page"
                }
                .into(),
                sort_strategy_estimated: false,
                filter_exact: entry.plan.exact,
                terms: entry.plan.choices.clone(),
                planning: entry.plan.planning,
                bound_terms: bound_term_diagnostics(repo, entry.filter),
                candidates: entry.candidates.len(),
                candidates_estimated: false,
                actual: None,
            });
        }
    }
    let mut counters = WorkCounters {
        candidates: candidate_total,
        preparation_ns,
        planning_ns: planning_started.elapsed().as_nanos() as u64,
        prepared_plan_hit,
        ..WorkCounters::default()
    };
    let mut actuals = (0..plans.len())
        .map(|_| PlanActual::default())
        .collect::<Vec<_>>();
    for (entry, actual) in plans.iter().zip(&mut actuals) {
        let work = entry.plan.semijoin_work;
        counters.add_semijoin(work)?;
        actual.files_opened += work.target_file_opens;
        actual.bytes_read += work.target_bytes_read;
        actual.record_parses += work.target_reads;
        actual.relation_target_candidates += work.target_candidates;
        actual.relation_target_matches += work.target_matches;
        actual.relation_target_reads += work.target_reads;
        actual.relation_target_file_opens += work.target_file_opens;
        actual.relation_edge_visits += work.edge_visits;
        actual.relation_reverse_postings_visited += work.reverse_postings_visited;
        actual.relation_verified_targets_reused += work.verified_targets_reused;
    }
    let mut heap = BinaryHeap::new();
    for (plan_index, entry) in plans.iter().enumerate() {
        let columns = sort.columns[entry.collection_index]
            .as_ref()
            .expect("selected collection");
        let order = if query.options.sort.len() > 1 {
            &repo.collections[entry.collection_index]
                .sorting
                .composite(&query.options.sort)
                .expect("composite checked")
                .order
        } else if let Some(field) = query.options.sort.first() {
            let column = columns[0].0;
            if field.descending {
                &column.descending
            } else {
                &column.ascending
            }
        } else {
            &repo.collections[entry.collection_index].identity_order
        };
        if let Some(cursor) = stored_cursor(
            repo,
            columns,
            order.as_slice(),
            entry.collection_index,
            plan_index,
            0,
        ) {
            heap.push(Reverse(cursor));
        }
    }
    let mut page = Vec::new();
    let mut stopped_early = false;
    let mut locator_query = query.clone();
    locator_query.options.return_mode = ReturnMode::Locator;
    // A one-record residual check still needs to retain its match even when
    // the requested page has zero rows or this record lies before the offset.
    locator_query.options.limit = 1;
    locator_query.options.offset = 0;
    let mut source_access = SourceAccessSession::new(&repo.files);
    if let Some(execution) = execution {
        source_access.configure_execution(execution);
    }
    if cap > 0 || query.options.count {
        loop {
            if let Some(execution) = execution {
                execution.check_string()?;
            }
            if let Some(delivery) = delivery.as_deref_mut() {
                delivery.check()?;
            }
            let Some(Reverse(cursor)) = heap.pop() else {
                break;
            };
            let entry = &plans[cursor.plan_index];
            let actual = &mut actuals[cursor.plan_index];
            let slot = cursor.record.slot.0;
            if entry.candidates.contains(slot) {
                let direct = (entry.plan.exact && entry.fts.is_none())
                    || (!query.options.evidence
                        && entry.fts.is_none()
                        && entry.plan.proven_hits.contains(slot));
                let mut matched = false;
                let mut evidence = HashSet::new();
                let mut payload = None;
                if direct {
                    matched = true;
                    counters.exact_posting_matches += 1;
                    actual.index_proven_matches += 1;
                    if query.options.evidence {
                        evidence = entry
                            .filter
                            .map(|expr| {
                                proven_evidence(
                                    repo,
                                    &repo.collections[entry.collection_index],
                                    cursor.record,
                                    expr,
                                    &entry.plan.proofs,
                                )
                            })
                            .transpose()?
                            .unwrap_or_default();
                    }
                    let position = counters.total_matches as usize;
                    if position >= all_offset.unwrap_or(query.options.offset)
                        && (all_offset.is_some() || position < cap)
                    {
                        if let Some(delivery) = delivery.as_deref_mut() {
                            if all_offset.is_some() {
                                prepare_all_batch(delivery, &mut page, &counters, cursor.record)?;
                            }
                            check_stream_page_read(delivery, query, &page, cursor.record)?;
                        }
                        payload = direct_payload(
                            repo,
                            query,
                            entry.collection_index,
                            cursor.record,
                            &mut counters,
                            actual,
                            &mut source_access,
                            delivery
                                .as_ref()
                                .map(|delivery| delivery.max_buffered_bytes()),
                            execution,
                        )?;
                    }
                } else {
                    let position = counters.total_matches as usize;
                    let scan_query = if position >= all_offset.unwrap_or(query.options.offset)
                        && (all_offset.is_some() || position < cap)
                    {
                        query
                    } else {
                        &locator_query
                    };
                    if std::ptr::eq(scan_query, query) {
                        if let Some(delivery) = delivery.as_deref_mut() {
                            if all_offset.is_some() {
                                prepare_all_batch(delivery, &mut page, &counters, cursor.record)?;
                            }
                            check_stream_page_read(delivery, query, &page, cursor.record)?;
                        }
                    }
                    let scanned = scan_file(
                        repo,
                        scan_query,
                        sort,
                        entry.collection_index,
                        residuals[cursor.plan_index].as_ref(),
                        entry.fts,
                        None,
                        direct_plans[cursor.plan_index].as_ref(),
                        None,
                        &projections[cursor.plan_index],
                        false,
                        None,
                        None,
                        cursor.record.file.0,
                        &[(slot, cursor.record.range)],
                        Some(&mut source_access),
                        delivery.as_ref().map(|delivery| delivery.token()),
                        delivery
                            .as_ref()
                            .map(|delivery| delivery.max_buffered_bytes() as u64),
                        false,
                        execution,
                    )?;
                    counters.add_scan(
                        ScanWork {
                            sort_key_evaluations: 0,
                            ..ScanWork::from(&scanned)
                        },
                        &scanned.relation_work,
                    )?;
                    actual.files_opened += scanned.files_opened;
                    actual.bytes_read += scanned.bytes_read;
                    actual.record_parses += scanned.record_parses;
                    actual.raw_text_verifications += scanned.raw_text_verifications;
                    actual.residual_evaluations += scanned.residual_evaluations;
                    actual.files_opened +=
                        scanned.relation_work.source_reads + scanned.relation_work.target_reads;
                    actual.bytes_read += scanned.relation_work.bytes_read;
                    actual.record_parses +=
                        scanned.relation_work.source_reads + scanned.relation_work.target_reads;
                    actual.relation_target_reads += scanned.relation_work.target_reads;
                    actual.relation_verified_targets_reused +=
                        scanned.relation_work.target_cache_hits;
                    actual.relation_target_file_opens += scanned.relation_work.target_reads;
                    actual.relation_edge_visits += scanned.relation_work.edge_visits;
                    actual.relation_backlink_probe_visits +=
                        scanned.relation_work.backlink_probe_visits;
                    actual.relation_bytes_read += scanned.relation_work.bytes_read;
                    if let Some(found) = scanned.matches.into_iter().next() {
                        matched = true;
                        evidence = found.evidence;
                        payload = found.payload;
                    }
                }
                if matched {
                    let position = counters.total_matches as usize;
                    if position >= all_offset.unwrap_or(query.options.offset)
                        && (all_offset.is_some() || position < cap)
                    {
                        let matched_fields = if query.options.evidence {
                            evidence.into_iter().collect()
                        } else {
                            Vec::new()
                        };
                        page.push(make_hit(
                            repo,
                            entry.collection_index,
                            cursor.record,
                            matched_fields,
                            payload,
                        ));
                    }
                    counters.total_matches += 1;
                    if let Some(delivery) = delivery.as_deref_mut() {
                        delivery
                            .check_buffer(page.iter().map(crate::streaming::hit_bytes).sum())?;
                        if all_offset.is_some() {
                            flush_all_batch_if_full(delivery, &mut page, &counters)?;
                        } else {
                            delivery.ordered_rows(
                                &page,
                                &counters,
                                page.len() == query.options.limit,
                            )?;
                        }
                        if delivery.options().mode == StreamMode::PageReplacements
                            && page.len() % delivery.options().batch_rows == 0
                        {
                            delivery.replacement(&page, &counters, 0)?;
                        }
                    }
                    if all_offset.is_none()
                        && !query.options.count
                        && counters.total_matches as usize >= cap
                    {
                        stopped_early = true;
                        break;
                    }
                }
            }
            if let Some(next) = stored_cursor(
                repo,
                cursor.columns,
                cursor.order,
                cursor.collection_index,
                cursor.plan_index,
                cursor.position + 1,
            ) {
                heap.push(Reverse(next));
            }
        }
    }
    counters.returned_records = if let Some(delivery) = delivery.as_deref_mut() {
        if all_offset.is_some() {
            delivery.ordered_batch(&page, &counters)?;
            page.clear();
            delivery.progress.emitted_rows
        } else {
            page.len() as u64
        }
    } else {
        page.len() as u64
    };
    counters.distinct_files = source_access.distinct_files();
    counters.match_count_complete = !stopped_early && (cap > 0 || query.options.count);
    if let Some(delivery) = delivery.as_deref_mut() {
        if all_offset.is_none() {
            delivery.ordered_rows(&page, &counters, true)?;
        }
    }
    if let Some(traces) = traces.as_deref_mut() {
        for (trace, actual) in traces.iter_mut().zip(actuals) {
            trace.actual = Some(actual);
        }
    }
    Ok(Some(SearchResult {
        count: query
            .options
            .count
            .then_some(counters.total_matches as usize),
        hits: page,
        counters,
    }))
}

#[allow(clippy::too_many_arguments)]
fn offer_cached_key(
    repo: &Repository,
    cached: &CachedResult,
    member_index: usize,
    key: RankKey,
    winners: &mut BinaryHeap<Winner>,
    retained_key_bytes: &mut u64,
    peak_key_bytes: &mut u64,
    cap: usize,
) -> Result<(), String> {
    if cap == 0 || (winners.len() == cap && winners.peek().is_some_and(|worst| key >= worst.key)) {
        return Ok(());
    }
    if winners.len() == cap {
        let old = winners.pop().unwrap();
        *retained_key_bytes -= rank_key_bytes(&old.key);
    }
    *retained_key_bytes = retained_key_bytes
        .checked_add(rank_key_bytes(&key))
        .ok_or("sort key byte count overflow")?;
    if *retained_key_bytes > MAX_SORT_KEY_BYTES {
        return Err("sort key memory budget exceeded".into());
    }
    *peak_key_bytes = (*peak_key_bytes).max(*retained_key_bytes);
    let member = &cached.members[member_index];
    let record =
        &repo.collections[member.collection_index].generation.records[member.slot as usize];
    winners.push(Winner {
        key,
        collection_index: member.collection_index,
        slot: member.slot,
        hit: make_hit(
            repo,
            member.collection_index,
            record,
            member.matched_fields.clone(),
            None,
        ),
        retained_bytes: 0,
    });
    Ok(())
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
    let preparation_start = Instant::now();
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
    let (_, plan_hit) = crate::prepared::prepare(repo, query)?;
    let cap = query
        .options
        .limit
        .checked_add(query.options.offset)
        .ok_or("page size overflow")?;
    let mut counters = WorkCounters {
        total_matches: cached.members.len() as u64,
        match_count_complete: true,
        verified_result_hit: true,
        prepared_plan_hit: plan_hit,
        ..WorkCounters::default()
    };
    counters.preparation_ns = preparation_start.elapsed().as_nanos() as u64;
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
            matched_fields: winner.hit.matched_fields,
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

fn materialize_cached_stream(
    repo: &Repository,
    query: &Query,
    cached: &CachedResult,
    delivery: &mut Delivery<'_>,
    execution: Option<&ExecutionContext>,
) -> Result<SearchResult, String> {
    delivery.check()?;
    let started = Instant::now();
    let (_, plan_hit) = crate::prepared::prepare(repo, query)?;
    let mut counters = WorkCounters {
        total_matches: cached.members.len() as u64,
        match_count_complete: true,
        verified_result_hit: true,
        prepared_plan_hit: plan_hit,
        ..WorkCounters::default()
    };
    counters.preparation_ns = started.elapsed().as_nanos() as u64;
    let mut page = Vec::new();
    let mut source_access = SourceAccessSession::new(&repo.files);
    if let Some(execution) = execution {
        source_access.configure_execution(execution);
    }
    let mut actual = PlanActual::default();
    for member in cached
        .members
        .iter()
        .skip(query.options.offset)
        .take(query.options.limit)
    {
        delivery.check()?;
        let record =
            &repo.collections[member.collection_index].generation.records[member.slot as usize];
        let existing = page.iter().map(crate::streaming::hit_bytes).sum::<usize>();
        delivery.check_record_bytes(record.range.len() as usize)?;
        delivery.check_buffer(existing.saturating_add(record.range.len() as usize))?;
        let payload = direct_payload(
            repo,
            query,
            member.collection_index,
            record,
            &mut counters,
            &mut actual,
            &mut source_access,
            Some(delivery.max_buffered_bytes()),
            execution,
        )?;
        page.push(make_hit(
            repo,
            member.collection_index,
            record,
            member.matched_fields.clone(),
            payload,
        ));
        delivery.check_buffer(page.iter().map(crate::streaming::hit_bytes).sum())?;
        match delivery.options().mode {
            StreamMode::OrderedRows => delivery.ordered_rows(&page, &counters, false)?,
            StreamMode::PageReplacements if page.len() % delivery.options().batch_rows == 0 => {
                delivery.replacement(&page, &counters, 0)?;
            }
            StreamMode::PageReplacements => {}
        }
    }
    counters.returned_records = page.len() as u64;
    counters.distinct_files = source_access.distinct_files();
    Ok(SearchResult {
        count: Some(cached.members.len()),
        hits: page,
        counters,
    })
}

fn materialize_cached(
    repo: &Repository,
    query: &Query,
    cached: &CachedResult,
) -> Result<SearchResult, String> {
    let preparation_start = Instant::now();
    let (_, plan_hit) = crate::prepared::prepare(repo, query)?;
    let cap = query
        .options
        .limit
        .checked_add(query.options.offset)
        .ok_or("page size overflow")?;
    let mut counters = WorkCounters {
        total_matches: cached.members.len() as u64,
        match_count_complete: true,
        verified_result_hit: true,
        prepared_plan_hit: plan_hit,
        ..WorkCounters::default()
    };
    counters.preparation_ns = preparation_start.elapsed().as_nanos() as u64;
    let mut hits = Vec::new();
    let mut files: BTreeMap<(usize, u32), Vec<(u32, ByteRange)>> = BTreeMap::new();
    for member in cached
        .members
        .iter()
        .skip(query.options.offset)
        .take(query.options.limit.min(cap))
    {
        let collection = &repo.collections[member.collection_index];
        let record = &collection.generation.records[member.slot as usize];
        let file = &repo.files.files[record.file.0 as usize];
        let index = hits.len() as u32;
        if query.options.return_mode == ReturnMode::Logical
            && record.class == RecordClass::RawFallback
        {
            hits.push(make_hit(
                repo,
                member.collection_index,
                record,
                member.matched_fields.clone(),
                Some(Payload::LogicalUnavailable {
                    diagnostic_ref: record.diagnostic_ref,
                }),
            ));
            continue;
        }
        if query.options.return_mode != ReturnMode::Locator {
            files
                .entry((member.collection_index, record.file.0))
                .or_default()
                .push((index, record.range));
        }
        hits.push(SearchHit {
            collection: collection.definition.name.clone(),
            record_id: record.id.0.clone(),
            generation: collection.generation.generation,
            file_id: file.id.0,
            file_path: file.relative_path.clone(),
            file_size: file.revision.size,
            file_created_ns: file.created_ns,
            byte_start: record.range.start,
            byte_end: record.range.end,
            modified_ns: record.revision.modified_ns,
            matched_fields: member.matched_fields.clone(),
            file_status: file.status,
            record_class: record.class,
            diagnostic_ref: record.diagnostic_ref,
            payload: None,
        });
    }
    let mut source_access = SourceAccessSession::new(&repo.files);
    for ((collection_index, file_id), ranges) in files {
        let collection = &repo.collections[collection_index];
        for batch in ranges.chunks(128) {
            let read = source_access.read(crate::model::FileId(file_id), batch)?;
            counters.files_opened += read.opened;
            counters.bytes_read += read.bytes_read;
            for (index, bytes) in read.records {
                let (payload, parses) = payload_from_bytes(
                    hits[index as usize].record_class,
                    Some(collection),
                    &query.options.return_mode,
                    bytes,
                )?;
                counters.record_parses += parses;
                hits[index as usize].payload = payload;
            }
        }
    }
    counters.distinct_files = source_access.distinct_files();
    counters.returned_records = hits.len() as u64;
    Ok(SearchResult {
        count: query.options.count.then_some(cached.members.len()),
        hits,
        counters,
    })
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
    let mut traces = Vec::new();
    let active_context = active.map(|active| {
        ExecutionContext::new(
            crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
            ExecutionLimits::default(),
            None,
        )
        .with_active(active)
    });
    let result = search_impl_with_collector(
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
        None,
    )?;
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
    let (prepared, _) = crate::prepared::prepare(repo, query)?;
    let has_source_dependent = prepared.collections.iter().any(|(_, filter, _)| {
        filter
            .as_ref()
            .is_some_and(crate::relation_target_cache::has_source_dependent)
    });
    let has_related_fts = prepared.collections.iter().any(|(_, _, fts)| {
        fts.as_ref()
            .is_some_and(|fts| fts.fields.iter().any(|field| field.related.is_some()))
    });
    let inferred_order = active.is_none()
        && !has_source_dependent
        && !has_related_fts
        && prepared.collections.len() == 1
        && posting_order_eligible(repo, query)
        && inferred_order_index(repo, query)
            .is_some_and(|(index, _, _)| prepared.collections[0].0 == index);
    let mut traces = Vec::new();
    let mut ordered_candidate_total = 0u64;
    for (collection_index, filter, fts) in prepared.collections {
        let collection = &repo.collections[collection_index];
        let domain_len = active.map_or(collection.generation.records.len() as u64, |domains| {
            domains[collection_index].len()
        });
        let estimated = filter
            .as_ref()
            .map(|expr| estimate_plan_expr(collection, expr, domain_len));
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
            folded_representation: crate::bigram::FOLDED_REPRESENTATION,
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

/// Load only payloads needed by the currently visible requested page.
fn materialize_winner_page(
    repo: &Repository,
    query: &Query,
    page: &mut [Winner],
    counters: &mut WorkCounters,
    files_read: &mut Option<RoaringBitmap>,
    mut traces: Option<&mut Vec<QueryPlanTrace>>,
    cancellation: Option<&CancellationToken>,
    stream_limit: Option<usize>,
    execution: Option<&ExecutionContext>,
) -> Result<(), String> {
    // Exact index membership may need a page payload. Read only those ranges.
    if query.options.return_mode != ReturnMode::Locator {
        let mut files: BTreeMap<(usize, u32), Vec<(u32, ByteRange)>> = BTreeMap::new();
        for (index, winner) in page.iter_mut().enumerate() {
            if winner.hit.payload.is_none() {
                let record = &repo.collections[winner.collection_index].generation.records
                    [winner.slot as usize];
                if query.options.return_mode == ReturnMode::Logical
                    && record.class == RecordClass::RawFallback
                {
                    winner.hit.payload = Some(Payload::LogicalUnavailable {
                        diagnostic_ref: record.diagnostic_ref,
                    });
                    continue;
                }
                files
                    .entry((winner.collection_index, record.file.0))
                    .or_default()
                    .push((index as u32, record.range));
            }
        }
        let mut source_access = SourceAccessSession::new(&repo.files);
        if let Some(execution) = execution {
            source_access.configure_execution(execution);
        }
        for ((collection_index, file_id), ranges) in files {
            if cancellation.is_some_and(CancellationToken::is_cancelled) {
                return Err("stream cancelled".into());
            }
            let collection = &repo.collections[collection_index];
            for batch in ranges.chunks(if stream_limit.is_some() { 1 } else { 128 }) {
                if cancellation.is_some_and(CancellationToken::is_cancelled) {
                    return Err("stream cancelled".into());
                }
                let read = if let Some(limit) = stream_limit {
                    source_access.read_bounded(crate::model::FileId(file_id), batch, limit)?
                } else {
                    source_access.read(crate::model::FileId(file_id), batch)?
                };
                counters.files_opened += read.opened;
                counters.bytes_read += read.bytes_read;
                if let Some(traces) = traces.as_deref_mut() {
                    if let Some(actual) = traces
                        .iter_mut()
                        .find(|trace| trace.collection == collection.definition.name)
                        .and_then(|trace| trace.actual.as_mut())
                    {
                        actual.files_opened += read.opened;
                        actual.bytes_read += read.bytes_read;
                        if query.options.return_mode == ReturnMode::Logical {
                            actual.record_parses += read.records.len() as u64;
                        }
                    }
                }
                for (index, bytes) in read.records {
                    if cancellation.is_some_and(CancellationToken::is_cancelled) {
                        return Err("stream cancelled".into());
                    }
                    let (payload, parses) = payload_from_bytes_with_execution(
                        page[index as usize].hit.record_class,
                        Some(collection),
                        &query.options.return_mode,
                        bytes,
                        execution,
                    )?;
                    counters.record_parses += parses;
                    page[index as usize].hit.payload = payload;
                }
            }
        }
        *files_read
            .as_mut()
            .expect("payload access needs file union") |= source_access.accessed_files();
    }
    Ok(())
}

fn winner_bytes(winner: &Winner) -> usize {
    std::mem::size_of::<Winner>()
        + rank_key_bytes(&winner.key) as usize
        + crate::streaming::hit_bytes(&winner.hit)
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

/// Publish a bounded provisional top-k page after a completed source batch.
fn emit_provisional_page(
    repo: &Repository,
    query: &Query,
    winners: &BinaryHeap<Winner>,
    counters: &mut WorkCounters,
    files_read: &mut Option<RoaringBitmap>,
    delivery: &mut Delivery<'_>,
    extra_retained: usize,
    execution: Option<&ExecutionContext>,
) -> Result<(), String> {
    delivery.check()?;
    let retained = winners.iter().map(winner_bytes).sum::<usize>();
    delivery.check_buffer(retained.saturating_add(extra_retained))?;
    if query.options.limit == 0 {
        return delivery.progress_event(counters);
    }
    if delivery.options().mode != StreamMode::PageReplacements {
        return Ok(());
    }
    let mut ranked = winners.iter().collect::<Vec<_>>();
    ranked.sort_by(|a, b| a.key.cmp(&b.key));
    let visible = ranked
        .into_iter()
        .skip(query.options.offset)
        .take(query.options.limit)
        .collect::<Vec<_>>();
    if visible.is_empty() {
        return Ok(());
    }
    let identities = visible
        .iter()
        .map(|winner| (winner.hit.collection.clone(), winner.hit.record_id.clone()))
        .collect::<Vec<_>>();
    if !delivery.page_changed(&identities) {
        return Ok(());
    }
    let projected_copy = visible.iter().fold(0usize, |total, winner| {
        let missing_body =
            if winner.hit.payload.is_none() && query.options.return_mode != ReturnMode::Locator {
                repo.collections[winner.collection_index].generation.records[winner.slot as usize]
                    .range
                    .len() as usize
            } else {
                0
            };
        total.saturating_add(winner_bytes(winner).saturating_add(missing_body))
    });
    let scratch = winners
        .len()
        .saturating_add(visible.len())
        .saturating_mul(std::mem::size_of::<&Winner>())
        .saturating_add(
            identities
                .iter()
                .map(|(collection, id)| {
                    collection
                        .len()
                        .saturating_add(id.len())
                        .saturating_add(2 * std::mem::size_of::<String>())
                })
                .sum::<usize>(),
        );
    delivery.check_buffer(
        retained
            .saturating_add(extra_retained)
            .saturating_add(projected_copy)
            .saturating_add(scratch),
    )?;
    let mut page = visible.into_iter().cloned().collect::<Vec<Winner>>();
    materialize_winner_page(
        repo,
        query,
        &mut page,
        counters,
        files_read,
        None,
        Some(delivery.token()),
        Some(delivery.max_buffered_bytes()),
        execution,
    )?;
    let hits = page
        .into_iter()
        .map(|winner| winner.hit)
        .collect::<Vec<_>>();
    delivery.replacement(&hits, counters, retained.saturating_add(extra_retained))
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
    extended_batches: Option<&[Arc<Vec<Arc<ExtendedBatch>>>]>,
) -> Result<SearchResult, String> {
    if let Some(execution) = execution {
        execution.check_string()?;
    }
    if let Some(delivery) = delivery.as_deref_mut() {
        delivery.check()?;
    }
    let preparation_start = Instant::now();
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
    let (prepared, plan_hit) = crate::prepared::prepare(repo, query)?;
    let mut rejected_order = RejectedOrderPlans {
        entries: Vec::new(),
        planning_ns: 0,
    };
    if cache_members.is_none() && exact_collector.is_none() {
        if let Some(result) = try_inferred_order_page(
            repo,
            query,
            &prepared,
            plan_hit,
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
            plan_hit,
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
    let mut files_read = (prepared.collections.len() > 1
        || query.options.return_mode != ReturnMode::Locator)
        .then(RoaringBitmap::new);
    let mut distinct_files = 0u64;
    counters.preparation_ns = preparation_start.elapsed().as_nanos() as u64;
    counters.prepared_plan_hit = plan_hit;
    counters.planning_ns += rejected_order.planning_ns;
    let mut reused_order = rejected_order.entries.into_iter();
    for (collection_index, filter, fts) in prepared.collections {
        if let Some(delivery) = delivery.as_deref_mut() {
            delivery.check()?;
        }
        let planning_start = Instant::now();
        let diagnostics = traces.is_some();
        let before = traces.as_ref().map(|_| counters.clone());
        let collection = &repo.collections[collection_index];
        let domain = collection_domain(collection, collection_index, active);
        let text_estimate = fts
            .as_ref()
            .and_then(|fts| estimate_fts_candidates(collection, fts));
        let source_unit = 30.0 + collection.stats.mean_record_bytes() / 128.0;
        let (plan, mut candidates, text_first) = if let Some((mut plan, candidates)) =
            reused_order.next()
        {
            plan.planning.reused_order_plans += 1;
            (plan, candidates, false)
        } else {
            let mut input = domain.clone();
            let filter_estimate = filter
                .as_ref()
                .map(|f| estimate_expr(collection, f, domain.len()));
            let text_first = match (text_estimate, filter_estimate) {
                (Some((text_count, text_lookup)), Some((filter_work, filter_count))) => {
                    text_lookup + text_count as f64 * source_unit * 0.1
                        < filter_work + filter_count as f64 * source_unit * 0.1
                }
                (Some(_), None) => true,
                _ => false,
            };
            if text_first {
                if let Some(fts) = &fts {
                    if let Some(text) = fts_candidates(collection, fts) {
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
                        f,
                        &domain,
                        &input,
                        query.options.evidence,
                        diagnostics,
                        execution,
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
            if let (Some(filter), Some(batches)) = (
                filter.as_ref(),
                extended_batches
                    .and_then(|groups| groups.get(collection_index))
                    .filter(|batches| !batches.is_empty()),
            ) {
                let bounds = extended_filter_bounds(batches, filter, &input);
                let original = plan.candidates.take().unwrap_or_else(|| input.clone());
                plan.candidates = Some(original & &bounds.may);
                // Source evidence still needs the original proof machinery.
                // Without evidence the exact batch hits may skip source reads.
                if !query.options.evidence {
                    plan.proven_hits |= &bounds.must;
                    plan.exact |= bounds.may == bounds.must;
                }
            }
            let mut candidates = plan.candidates.take().unwrap_or(input);
            if !text_first {
                if let (Some(fts), Some((estimated, lookup))) = (&fts, text_estimate) {
                    if lookup + estimated as f64 * 0.05 < candidates.len() as f64 * source_unit {
                        if let Some(text_candidates) = fts_candidates(collection, fts) {
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
            .map(|fts| crate::relation_semijoin::fts_witnesses(repo, fts, &candidates, execution))
            .transpose()?
            .flatten();
        let relation_fts_strategy =
            relation_fts.as_ref().map(|proof| {
                if proof
                    .strategies
                    .iter()
                    .all(|strategy| *strategy == crate::relation_semijoin::Strategy::TargetFirst)
                {
                    "relation target-first semijoin"
                } else if proof.strategies.iter().all(|strategy| {
                    *strategy == crate::relation_semijoin::Strategy::SourceRestricted
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
        counters.planning_ns += planning_start.elapsed().as_nanos() as u64;
        counters.candidates += candidates.len();
        let needs_scan = sort.bound.needs_source
            || !plan.exact
            || (fts.is_some() && preverified_evidence.is_none());
        if let Some(traces) = traces.as_deref_mut() {
            traces.push(QueryPlanTrace {
                collection: collection.definition.name.clone(),
                generation: collection.generation.generation,
                bigram_alphabet: collection.raw.alphabet,
                folded_representation: crate::bigram::FOLDED_REPRESENTATION,
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
                        &winners,
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
                    &winners,
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
        let residual = filter
            .as_ref()
            .map(|expr| prepare_residual(expr, &plan.proofs, collection, candidates.len()));
        let target_values = filter
            .as_ref()
            .map(|expr| {
                if let Some(delivery) = delivery.as_ref() {
                    crate::relation_target_cache::prepare_stream(
                        repo,
                        collection_index,
                        expr,
                        &candidates,
                        delivery.token(),
                        delivery.max_buffered_bytes() / 4,
                        execution,
                    )
                } else {
                    crate::relation_target_cache::prepare(
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
            crate::relation_query::check_query_work_budget(&counters)?;
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
                            matched.evidence,
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
                    &winners,
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
#[path = "run.test.rs"]
mod tests;
