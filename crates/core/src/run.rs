//! Bounded result retention, grouped source access, and payload materialization.

use std::cmp::Ordering;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
use std::time::Instant;

use rayon::prelude::*;
use roaring::RoaringBitmap;
use serde::Serialize;
use smallvec::SmallVec;

use crate::executor::{
    Plan, Proof, TermChoice, estimate_expr, estimate_fts_candidates, fts_candidates,
    plan_expr_restricted,
};
use crate::inventory::{ByteRange, RecordClass, RecordEntry};
use crate::model::{PathRoot, Value};
use crate::query::{CaseSensitivity, FtsMode, Operator, Quantifier, Query};
use crate::record_preparation::{materialize, prepare_query_record};
use crate::reference::{
    BoundExpr, BoundFts, BoundTerm, Derived, ExactInput, RecordContext, eval_bound_fts, eval_term,
    eval_unavailable_filter, same_bound_term,
};
use crate::repository::{Payload, Repository, SearchHit, SearchResult};
use crate::sort_binding::{BoundSort, BoundSortSource, SortTargetCache};
use crate::sorting::{SortColumn, SortKey, SortOptions};
use crate::source::{ExtractedRecord, RecordRangeReader, ScanProjection};
use crate::source_access::SourceAccessSession;
use crate::streaming::{
    Abort, CancellationToken, Delivery, StreamControl, StreamError, StreamEvent, StreamMode,
    StreamOptions, StreamOutcome, StreamSummary,
};
use crate::verified::{CachedMember, CachedResult};
use crate::workload::{ReturnMode, SortField, WorkCounters};

const MAX_SORT_KEY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_COMPUTED_SORT_BYTES: u64 = 128 * 1024 * 1024;

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
    }
}

enum Residual<'a> {
    Term(&'a BoundTerm, Option<&'a Proof>),
    And(Vec<Residual<'a>>),
    Or(Vec<Residual<'a>>),
    Not(Box<Residual<'a>>),
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
) -> Result<ScannedFile, String> {
    let collection = &repo.collections[collection_index];
    let path = repo
        .files
        .native_path(crate::model::FileId(file_id))
        .ok_or("missing source path")?;
    let revision = repo.files.files[file_id as usize].revision;
    let mut outcome = ScannedFile::default();
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
        Some(RecordRangeReader::open(&path, revision)?)
    };
    outcome.files_opened = u64::from(reader.is_some());
    let full_projection = ScanProjection::full(projection.collect_text);
    let mut cursor = 0;
    while cursor < ranges.len() {
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
        if max_retained_bytes
            .is_some_and(|limit| batch.iter().any(|(_, range)| range.len() > limit))
        {
            return Err("stream resource: record exceeds buffered-byte limit".into());
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
            let logical_projection = query.options.return_mode == ReturnMode::Logical
                && (may_reach_page || sort.bound.has_computed);
            let record_projection = if logical_projection {
                &full_projection
            } else {
                projection
            };
            let decode = record.class == RecordClass::Logical
                && (logical_projection
                    || record_projection.collect_text
                    || record_projection
                        .top_level
                        .as_ref()
                        .is_none_or(|fields| !fields.is_empty()));
            let extracted = if decode {
                let extracted = prepare_query_record(
                    &collection.source,
                    &collection.definition.schema,
                    &bytes,
                    record_projection,
                    logical_projection,
                )?;
                outcome.record_parses += 1;
                extracted
            } else {
                ExtractedRecord {
                    value: Value::Object(Default::default()),
                    text: Vec::new(),
                }
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
            let fts_match = if let Some(proof) = fts_proof {
                if let Some(fields) = proof.get(&slot) {
                    evidence.extend(fields.iter().cloned());
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
                        if matched {
                            evidence.insert("@record.raw_text".into());
                        }
                        matched
                    } else {
                        crate::relation_query::eval_fts_with_relations(
                            repo,
                            fts,
                            &[],
                            &ctx,
                            &mut evidence,
                            crate::relation_query::EvaluationMode::Indexed,
                            &mut outcome.relation_work,
                        )?
                    }
                } else {
                    true
                }
            } else {
                fts.map(|fts| {
                    crate::relation_query::eval_fts_with_relations(
                        repo,
                        fts,
                        &extracted.text,
                        &ctx,
                        &mut evidence,
                        crate::relation_query::EvaluationMode::Indexed,
                        &mut outcome.relation_work,
                    )
                })
                .transpose()?
                .unwrap_or(true)
            };
            let filter_match = if fts_match && record.class == RecordClass::RawFallback {
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
                    let retained = match query.options.return_mode {
                        ReturnMode::Locator => 0,
                        ReturnMode::Logical if sort.bound.has_computed => (bytes.len() as u64)
                            .checked_mul(4)
                            .ok_or("sort payload byte estimate overflow")?,
                        _ => bytes.len() as u64,
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
                        return Err(
                            "stream resource: file winners exceed buffered-byte limit".into()
                        );
                    }
                    let payload = if record.class == RecordClass::RawFallback
                        && query.options.return_mode == ReturnMode::Logical
                    {
                        Some(Payload::LogicalUnavailable {
                            diagnostic_ref: record.diagnostic_ref,
                        })
                    } else {
                        materialize(&query.options.return_mode, bytes, Some(extracted))?
                    };
                    let key = computed_key
                        .unwrap_or_else(|| rank_key(sort, repo, collection_index, record));
                    let measured = std::mem::size_of::<FileWinner>()
                        .saturating_add(rank_key_bytes(&key) as usize)
                        .saturating_add(
                            evidence.iter().map(|field| field.len() + 64).sum::<usize>(),
                        )
                        .saturating_add(
                            payload.as_ref().map_or(0, crate::streaming::payload_bytes),
                        );
                    let replaced_buffered = if winners.len() == keep_cap {
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
                        return Err(
                            "stream resource: file winners exceed buffered-byte limit".into()
                        );
                    }
                    if winners.len() == keep_cap {
                        let removed = winners.pop().unwrap();
                        retained_bytes -= removed.matched.retained_bytes;
                        retained_key_bytes -= rank_key_bytes(&removed.key);
                        buffered_bytes -= removed.matched.buffered_bytes;
                    }
                    retained_bytes += retained;
                    buffered_bytes += measured;
                    outcome.peak_payload_bytes = outcome.peak_payload_bytes.max(retained_bytes);
                    retained_key_bytes = retained_key_bytes
                        .checked_add(rank_key_bytes(&key))
                        .ok_or("sort key byte count overflow")?;
                    if retained_key_bytes > MAX_SORT_KEY_BYTES {
                        return Err("sort key memory budget exceeded".into());
                    }
                    if sort.bound.has_computed {
                        check_computed_sort_budget(retained_bytes, retained_key_bytes, 0)?;
                    }
                    outcome.peak_sort_key_bytes =
                        outcome.peak_sort_key_bytes.max(retained_key_bytes);
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
    pub bigram_alphabet: crate::workload::BigramAlphabet,
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
    pub filter_exact: bool,
    pub terms: Vec<TermChoice>,
    pub bound_terms: Vec<BoundTermDiagnostic>,
    pub candidates: u64,
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

fn record_semijoin_work(
    work: crate::relation_semijoin::SemijoinWork,
    counters: &mut WorkCounters,
) -> Result<(), String> {
    counters.relation_target_candidates += work.target_candidates;
    counters.relation_target_matches += work.target_matches;
    counters.relation_target_reads += work.target_reads;
    counters.relation_target_file_opens += work.target_file_opens;
    counters.relation_edge_visits += work.edge_visits;
    counters.relation_reverse_postings_visited += work.reverse_postings_visited;
    counters.relation_verified_targets_reused += work.verified_targets_reused;
    counters.relation_bytes_read += work.target_bytes_read;
    counters.files_opened += work.target_file_opens;
    counters.bytes_read += work.target_bytes_read;
    counters.record_parses += work.target_reads;
    crate::relation_query::check_query_work_budget(counters)
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
    (std::mem::size_of::<RankKey>()
        + key.parts.capacity() * std::mem::size_of::<(SortKey, bool, SortOptions)>()
        + key.collection.capacity()
        + key.id.capacity()
        + key
            .parts
            .iter()
            .map(|(part, _, _)| crate::sort_binding::key_bytes(part))
            .sum::<usize>()) as u64
}

fn make_hit(
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

pub fn search(repo: &Repository, query: &Query) -> Result<SearchResult, String> {
    let budget = {
        let cache = repo.verified.lock().map_err(|e| e.to_string())?;
        if cache.enabled() { cache.budget() } else { 0 }
    };
    if budget == 0 {
        return search_impl(repo, query, None, None, None, None, None);
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
            materialize_computed_cached(repo, query, &cached, bound_sort, None)
        } else {
            materialize_cached(repo, query, &cached)
        };
    }
    if !query.options.count {
        return search_impl(repo, query, None, None, None, Some(bound_sort), None);
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
    )?;
    if tracker.complete() && Some(members.len()) == result.count {
        sort_cached_members(repo, &membership_query, &mut members);
        repo.verified
            .lock()
            .map_err(|e| e.to_string())?
            .insert(key, members);
    }
    Ok(result)
}

/// Execute and publish from the shared scan/ranking path before completion.
pub fn search_stream(
    repo: &Repository,
    query: &Query,
    options: StreamOptions,
    cancellation: &CancellationToken,
    sink: &mut dyn for<'e> FnMut(StreamEvent<'e>) -> Result<StreamControl, String>,
) -> Result<StreamOutcome, StreamError> {
    options
        .validate()
        .map_err(|message| StreamError::Execution {
            message,
            progress: Default::default(),
        })?;
    let mut delivery = Delivery::new(options, cancellation, sink);
    if delivery.check().is_err() {
        return Ok(StreamOutcome::Cancelled(delivery.progress));
    }
    let bound_sort = BoundSort::bind(repo, query).map_err(|message| StreamError::Execution {
        message,
        progress: delivery.progress,
    })?;
    let computed_sort = bound_sort.has_computed;
    let mut membership_query = query.clone();
    if computed_sort {
        membership_query.options.sort.clear();
    }
    let budget = repo
        .verified
        .lock()
        .map_err(|error| StreamError::Execution {
            message: error.to_string(),
            progress: delivery.progress,
        })?
        .budget();
    let mut members = Vec::new();
    // The ordered cursor can publish its page before COUNT traverses the rest
    // of the domain. Building a membership cache would force the top-k scan.
    let cache_key = if budget > 0 && query.options.count {
        Some(
            crate::verified::key(&membership_query).map_err(|message| StreamError::Execution {
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
            materialize_computed_cached(repo, query, &cached, bound_sort, Some(&mut delivery))
        } else {
            materialize_cached_stream(repo, query, &cached, &mut delivery)
        }
    } else {
        search_impl(
            repo,
            query,
            None,
            tracker.as_ref().map(|_| &mut members),
            tracker.as_ref(),
            Some(bound_sort),
            Some(&mut delivery),
        )
    };
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
                None if message.starts_with("stream resource:") => Err(StreamError::Resource {
                    message,
                    progress: delivery.progress,
                }),
                None => Err(StreamError::Execution {
                    message,
                    progress: delivery.progress,
                }),
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
                message,
                progress: delivery.progress,
            }),
        };
    }
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
            repo.verified
                .lock()
                .map_err(|error| StreamError::Execution {
                    message: error.to_string(),
                    progress: delivery.progress,
                })?
                .insert(key, members);
        }
    }
    Ok(StreamOutcome::Complete(Box::new(StreamSummary {
        count: result.count,
        returned_rows: result.hits.len(),
        final_sequence: delivery.last_page_sequence(),
        counters: result.counters,
        progress: delivery.progress,
    })))
}

pub(crate) fn exact_members(
    repo: &Repository,
    query: &Query,
    collection_index: usize,
) -> Result<RoaringBitmap, String> {
    let mut scoped = query.clone();
    scoped.collections = vec![repo.collections[collection_index].definition.name.clone()];
    scoped.options.count = true;
    scoped.options.limit = 0;
    scoped.options.offset = 0;
    scoped.options.sort.clear();
    scoped.options.return_mode = ReturnMode::Locator;
    scoped.options.evidence = false;
    let mut members = Vec::new();
    let budget = CacheBudget::new(usize::MAX);
    let result = search_impl(
        repo,
        &scoped,
        None,
        Some(&mut members),
        Some(&budget),
        None,
        None,
    )?;
    if !budget.complete()
        || result.count != Some(members.len())
        || members
            .iter()
            .any(|member| member.collection_index != collection_index)
    {
        return Err("exact selection is incomplete".into());
    }
    Ok(members.into_iter().map(|member| member.slot).collect())
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

fn direct_payload(
    repo: &Repository,
    query: &Query,
    collection_index: usize,
    record: &RecordEntry,
    counters: &mut WorkCounters,
    actual: &mut PlanActual,
    source_access: &mut SourceAccessSession<'_>,
    stream_limit: Option<usize>,
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
    let logical = if query.options.return_mode == ReturnMode::Logical {
        counters.record_parses += 1;
        actual.record_parses += 1;
        Some(prepare_query_record(
            &collection.source,
            &collection.definition.schema,
            &bytes,
            &ScanProjection::full(false),
            true,
        )?)
    } else {
        None
    };
    materialize(&query.options.return_mode, bytes, logical)
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
) -> Result<Option<SearchResult>, String> {
    if sort.bound.has_computed || (query.options.count && delivery.is_none()) {
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
    if prepared.collections.iter().any(|(_, _, fts)| {
        fts.as_ref()
            .is_some_and(|fts| fts.fields.iter().any(|field| field.related.is_some()))
    }) {
        return Ok(None);
    }
    if prepared.collections.iter().any(|(_, filter, _)| {
        filter
            .as_ref()
            .is_some_and(crate::relation_target_cache::has_source_dependent)
    }) {
        return Ok(None);
    }
    let cap = query
        .options
        .limit
        .checked_add(query.options.offset)
        .ok_or("page size overflow")?;
    let planning_started = Instant::now();
    let mut plans = Vec::new();
    let mut candidate_total = 0u64;
    for (collection_index, filter, fts) in &prepared.collections {
        let collection = &repo.collections[*collection_index];
        let domain: RoaringBitmap = (0..collection.generation.records.len() as u32).collect();
        let mut plan = filter
            .as_ref()
            .map(|expr| {
                plan_expr_restricted(
                    repo,
                    collection,
                    expr,
                    &domain,
                    &domain,
                    query.options.evidence,
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
            });
        let mut candidates = plan.candidates.take().unwrap_or(domain);
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
    if delivery.is_none()
        && cap > 0
        && candidate_total > 0
        && candidate_total < (cap as u64).saturating_mul(4)
    {
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
            scan_projection(query, residual.as_ref(), &entry.candidates, false)
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
                input_records: collection.generation.records.len() as u64,
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
                filter_exact: entry.plan.exact,
                terms: entry.plan.choices.clone(),
                bound_terms: bound_term_diagnostics(repo, entry.filter),
                candidates: entry.candidates.len(),
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
        record_semijoin_work(work, &mut counters)?;
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
    let mut source_access = SourceAccessSession::new(&repo.files);
    if cap > 0 || (delivery.is_some() && query.options.count) {
        loop {
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
                    if (counters.total_matches as usize) >= query.options.offset
                        && (counters.total_matches as usize) < cap
                    {
                        if let Some(delivery) = delivery.as_deref_mut() {
                            delivery.check_record_bytes(cursor.record.range.len() as usize)?;
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
                        )?;
                    }
                } else {
                    let scan_query = if (counters.total_matches as usize) >= query.options.offset
                        && (counters.total_matches as usize) < cap
                    {
                        query
                    } else {
                        &locator_query
                    };
                    let scanned = scan_file(
                        repo,
                        scan_query,
                        sort,
                        entry.collection_index,
                        residuals[cursor.plan_index].as_ref(),
                        entry.fts,
                        None,
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
                    )?;
                    counters.files_opened += scanned.files_opened;
                    counters.bytes_read += scanned.bytes_read;
                    counters.record_parses += scanned.record_parses;
                    counters.raw_text_verifications += scanned.raw_text_verifications;
                    counters.scanned_records += scanned.scanned_records;
                    counters.residual_evaluations += scanned.residual_evaluations;
                    counters.relation_source_reads += scanned.relation_work.source_reads;
                    counters.relation_target_reads += scanned.relation_work.target_reads;
                    counters.relation_verified_targets_reused +=
                        scanned.relation_work.target_cache_hits;
                    counters.relation_target_file_opens += scanned.relation_work.target_reads;
                    counters.relation_edge_visits += scanned.relation_work.edge_visits;
                    counters.relation_backlink_probe_visits +=
                        scanned.relation_work.backlink_probe_visits;
                    counters.relation_bytes_read += scanned.relation_work.bytes_read;
                    crate::relation_query::check_query_work_budget(&counters)?;
                    counters.files_opened +=
                        scanned.relation_work.source_reads + scanned.relation_work.target_reads;
                    counters.bytes_read += scanned.relation_work.bytes_read;
                    counters.record_parses +=
                        scanned.relation_work.source_reads + scanned.relation_work.target_reads;
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
                    if (counters.total_matches as usize) >= query.options.offset
                        && (counters.total_matches as usize) < cap
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
                        delivery.ordered_rows(
                            &page,
                            &counters,
                            page.len() == query.options.limit,
                        )?;
                        if delivery.options().mode == StreamMode::PageReplacements
                            && page.len() % delivery.options().batch_rows == 0
                        {
                            delivery.replacement(&page, &counters, 0)?;
                        }
                    }
                    if !query.options.count && counters.total_matches as usize >= cap {
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
    counters.returned_records = page.len() as u64;
    counters.distinct_files = source_access.distinct_files();
    counters.match_count_complete = !stopped_early && (cap > 0 || query.options.count);
    if let Some(delivery) = delivery.as_deref_mut() {
        delivery.ordered_rows(&page, &counters, true)?;
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

/// Re-rank complete cached membership with bounded keys; no computed page is cached.
fn materialize_computed_cached<'a>(
    repo: &'a Repository,
    query: &Query,
    cached: &CachedResult,
    bound_sort: BoundSort<'a>,
    mut delivery: Option<&mut Delivery<'_>>,
) -> Result<SearchResult, String> {
    let preparation_start = Instant::now();
    let sort = ResolvedSort::from_bound(repo, query, bound_sort);
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
    }
    reads.sort_unstable_by_key(|read| (read.collection_index, read.file_id, read.range.start));
    let mut source_access = SourceAccessSession::new(&repo.files);
    let mut batch = Vec::<(u32, ByteRange)>::with_capacity(128);
    let mut start = 0;
    while start < reads.len() {
        if let Some(delivery) = delivery.as_deref_mut() {
            delivery.check()?;
            delivery.check_buffer(fixed_bytes.saturating_add(retained_key_bytes) as usize)?;
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
                source_access.read_bounded(
                    crate::model::FileId(file_id),
                    &batch,
                    delivery.max_buffered_bytes(),
                )?
            } else {
                source_access.read(crate::model::FileId(file_id), &batch)?
            };
            counters.files_opened += read.opened;
            counters.bytes_read += read.bytes_read;
            for (index, bytes) in read.records {
                let member = &cached.members[index as usize];
                let record = &collection.generation.records[member.slot as usize];
                let extracted = prepare_query_record(
                    &collection.source,
                    &collection.definition.schema,
                    &bytes,
                    &ScanProjection::full(false),
                    false,
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
    let page_members = winners
        .into_sorted_vec()
        .into_iter()
        .skip(query.options.offset)
        .take(query.options.limit)
        .map(|winner| CachedMember {
            collection_index: winner.collection_index,
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
        materialize_cached_stream(repo, &page_query, &page_cache, delivery)?
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
                let logical = if query.options.return_mode == ReturnMode::Logical {
                    counters.record_parses += 1;
                    Some(prepare_query_record(
                        &collection.source,
                        &collection.definition.schema,
                        &bytes,
                        &ScanProjection::full(false),
                        true,
                    )?)
                } else {
                    None
                };
                hits[index as usize].payload =
                    materialize(&query.options.return_mode, bytes, logical)?;
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
    let mut traces = Vec::new();
    let result = search_impl(repo, query, Some(&mut traces), None, None, None, None)?;
    Ok((result, traces))
}

pub fn explain(repo: &Repository, query: &Query) -> Result<Vec<QueryPlanTrace>, String> {
    let bound_sort = BoundSort::bind(repo, query)?;
    let (prepared, _) = crate::prepared::prepare(repo, query)?;
    let mut relation_work = WorkCounters::default();
    let has_source_dependent = prepared.collections.iter().any(|(_, filter, _)| {
        filter
            .as_ref()
            .is_some_and(crate::relation_target_cache::has_source_dependent)
    });
    let mut traces = Vec::new();
    let mut ordered_candidate_total = 0u64;
    for (collection_index, filter, fts) in prepared.collections {
        let collection = &repo.collections[collection_index];
        let domain: RoaringBitmap = (0..collection.generation.records.len() as u32).collect();
        let plan = filter
            .as_ref()
            .map(|f| {
                plan_expr_restricted(
                    repo,
                    collection,
                    f,
                    &domain,
                    &domain,
                    query.options.evidence,
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
            });
        record_semijoin_work(plan.semijoin_work, &mut relation_work)?;
        let mut ordered_candidates = plan.candidates.clone().unwrap_or_else(|| domain.clone());
        if let Some(text_candidates) = fts.as_ref().and_then(|fts| fts_candidates(collection, fts))
        {
            ordered_candidates &= text_candidates;
        }
        let relation_fts = fts
            .as_ref()
            .map(|fts| crate::relation_semijoin::fts_witnesses(repo, fts, &ordered_candidates))
            .transpose()?
            .flatten();
        if let Some(proof) = &relation_fts {
            record_semijoin_work(proof.work, &mut relation_work)?;
        }
        if let Some(proof) = &relation_fts {
            ordered_candidates = proof.evidence.keys().copied().collect();
        }
        ordered_candidate_total += ordered_candidates.len();
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
            fts_estimated_candidates: fts.as_ref().and_then(|fts| {
                estimate_fts_candidates(collection, fts).map(|estimate| estimate.0)
            }),
            remaining_verification: (fts.is_some() && relation_fts.is_none()) || !plan.exact,
            input_records: domain.len(),
            fts_strategy: if fts.as_ref().is_some_and(|fts| fts.fields.is_empty())
                && collection.logical_records.len() != collection.generation.records.len() as u64
            {
                "logical text plus fallback source text"
            } else if let Some(proof) = &relation_fts {
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
            } else if let Some(fts) = &fts {
                if !fts.fields.is_empty()
                    && fts
                        .fields
                        .iter()
                        .all(|field| matches!(field.path.root, PathRoot::Metadata(_)))
                {
                    "metadata inventory"
                } else if estimate_fts_candidates(collection, fts).is_some() {
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
            filter_exact: plan.exact,
            terms: plan.choices,
            bound_terms: bound_term_diagnostics(repo, filter.as_ref()),
            candidates: ordered_candidates.len(),
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
        && !traces
            .iter()
            .any(|trace| trace.fts_strategy.starts_with("relation "))
        && !(cap > 0
            && ordered_candidate_total > 0
            && ordered_candidate_total < (cap as u64).saturating_mul(4));
    for trace in &mut traces {
        trace.sort_strategy = if stored_order {
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
                    let logical = if query.options.return_mode == ReturnMode::Logical {
                        counters.record_parses += 1;
                        Some(prepare_query_record(
                            &collection.source,
                            &collection.definition.schema,
                            &bytes,
                            &ScanProjection::full(false),
                            true,
                        )?)
                    } else {
                        None
                    };
                    page[index as usize].hit.payload =
                        materialize(&query.options.return_mode, bytes, logical)?;
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
    mut traces: Option<&mut Vec<QueryPlanTrace>>,
    mut cache_members: Option<&mut Vec<CachedMember>>,
    cache_budget: Option<&CacheBudget>,
    prebound_sort: Option<BoundSort<'a>>,
    mut delivery: Option<&mut Delivery<'_>>,
) -> Result<SearchResult, String> {
    if let Some(delivery) = delivery.as_deref_mut() {
        delivery.check()?;
    }
    let preparation_start = Instant::now();
    let sort = match prebound_sort {
        Some(bound) => ResolvedSort::from_bound(repo, query, bound),
        None => ResolvedSort::new(repo, query)?,
    };
    let (prepared, plan_hit) = crate::prepared::prepare(repo, query)?;
    if cache_members.is_none() {
        if let Some(result) = try_stored_order_page(
            repo,
            query,
            &prepared,
            &sort,
            plan_hit,
            preparation_start.elapsed().as_nanos() as u64,
            traces.as_deref_mut(),
            delivery.as_deref_mut(),
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
    for (collection_index, filter, fts) in prepared.collections {
        if let Some(delivery) = delivery.as_deref_mut() {
            delivery.check()?;
        }
        let planning_start = Instant::now();
        let before = traces.as_ref().map(|_| counters.clone());
        let collection = &repo.collections[collection_index];
        let domain: RoaringBitmap = (0..collection.generation.records.len() as u32).collect();
        let mut input = domain.clone();
        let text_estimate = fts
            .as_ref()
            .and_then(|fts| estimate_fts_candidates(collection, fts));
        let filter_estimate = filter
            .as_ref()
            .map(|f| estimate_expr(collection, f, domain.len()));
        let source_unit = 30.0 + collection.stats.mean_record_bytes() / 128.0;
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
        let plan = filter
            .as_ref()
            .map(|f| {
                plan_expr_restricted(repo, collection, f, &domain, &input, query.options.evidence)
            })
            .transpose()?
            .unwrap_or(Plan {
                candidates: None,
                exact: true,
                proven_hits: RoaringBitmap::new(),
                proofs: Vec::new(),
                choices: Vec::new(),
                semijoin_work: Default::default(),
            });
        record_semijoin_work(plan.semijoin_work, &mut counters)?;
        let mut candidates = plan.candidates.unwrap_or(input);
        if !text_first {
            if let (Some(fts), Some((estimated, lookup))) = (&fts, text_estimate) {
                if lookup + estimated as f64 * 0.05 < candidates.len() as f64 * source_unit {
                    if let Some(text_candidates) = fts_candidates(collection, fts) {
                        candidates &= text_candidates;
                    }
                }
            }
        }
        let metadata_only_fts = fts.as_ref().is_some_and(|fts| {
            !fts.fields.is_empty()
                && fts
                    .fields
                    .iter()
                    .all(|field| matches!(field.path.root, PathRoot::Metadata(_)))
        });
        let relation_fts = fts
            .as_ref()
            .map(|fts| crate::relation_semijoin::fts_witnesses(repo, fts, &candidates))
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
            record_semijoin_work(proof.work, &mut counters)?;
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
                normalized_sort: query.options.sort.clone(),
                sort_strategy: if sort.bound.has_computed {
                    "computed_top_k_full_key_evaluation"
                } else {
                    "bounded_top_k"
                }
                .into(),
                filter_exact: plan.exact,
                terms: plan.choices.clone(),
                bound_terms: bound_term_diagnostics(repo, filter.as_ref()),
                candidates: candidates.len(),
                actual: None,
            });
        }
        if !needs_scan {
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
                    )
                } else {
                    crate::relation_target_cache::prepare(repo, collection_index, expr, &candidates)
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
        if sort.bound.has_computed {
            projection = ScanProjection::full(projection.collect_text);
        }
        let retain_all_evidence = cache_budget.is_some_and(CacheBudget::complete);
        let parallel = repo.options.query_workers > 1 && files.len() > 1;
        // Bound in-flight parsed records while workers scan independent files.
        // Applying offers in file order preserves deterministic tie handling.
        let base_chunk_size = if sort.bound.has_computed {
            1
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
        for chunk in files.chunks(chunk_size) {
            if let Some(delivery) = delivery.as_deref_mut() {
                delivery.check()?;
            }
            let stream_token = delivery.as_ref().map(|delivery| delivery.token().clone());
            let per_file_budget = if let Some(delivery) = delivery.as_deref_mut() {
                let resident = winners
                    .iter()
                    .map(winner_bytes)
                    .sum::<usize>()
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
                    return Err("stream resource: no room for source scan batch".into());
                }
                Some(per_file as u64)
            } else {
                None
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
                                query,
                                &sort,
                                collection_index,
                                residual.as_ref(),
                                if preverified_evidence.is_some() {
                                    None
                                } else {
                                    fts.as_ref()
                                },
                                preverified_evidence.as_ref(),
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
                            query,
                            &sort,
                            collection_index,
                            residual.as_ref(),
                            if preverified_evidence.is_some() {
                                None
                            } else {
                                fts.as_ref()
                            },
                            preverified_evidence.as_ref(),
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
                return Err("sort key memory budget exceeded".into());
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
                check_computed_sort_budget(payload_upper, key_upper, target_bytes)?;
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
                counters.files_opened += outcome.files_opened;
                counters.bytes_read += outcome.bytes_read;
                counters.record_parses += outcome.record_parses;
                counters.raw_text_verifications += outcome.raw_text_verifications;
                counters.scanned_records += outcome.scanned_records;
                counters.sort_key_evaluations += outcome.sort_key_evaluations;
                counters.residual_evaluations += outcome.residual_evaluations;
                counters.relation_source_reads += outcome.relation_work.source_reads;
                counters.relation_target_reads += outcome.relation_work.target_reads;
                counters.relation_verified_targets_reused +=
                    outcome.relation_work.target_cache_hits;
                counters.relation_target_file_opens += outcome.relation_work.target_reads;
                counters.relation_edge_visits += outcome.relation_work.edge_visits;
                counters.relation_backlink_probe_visits +=
                    outcome.relation_work.backlink_probe_visits;
                counters.relation_bytes_read += outcome.relation_work.bytes_read;
                crate::relation_query::check_query_work_budget(&counters)?;
                counters.files_opened +=
                    outcome.relation_work.source_reads + outcome.relation_work.target_reads;
                counters.bytes_read += outcome.relation_work.bytes_read;
                counters.record_parses +=
                    outcome.relation_work.source_reads + outcome.relation_work.target_reads;
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
                        .saturating_add(cache_budget.map_or(0, CacheBudget::used)),
                )?;
            }
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
        let targets = sort
            .target_cache
            .lock()
            .map_err(|error| error.to_string())?;
        check_computed_sort_budget(page_payload, retained_key_bytes, targets.retained_bytes())?;
    }
    if let Some(delivery) = delivery.as_deref_mut() {
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
    )?;
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
