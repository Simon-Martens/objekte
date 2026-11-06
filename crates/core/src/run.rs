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
use crate::inventory::{ByteRange, RecordEntry};
use crate::model::{PathRoot, Value};
use crate::query::{Operator, Query};
use crate::record_preparation::{materialize, prepare_query_record};
use crate::reference::{
    BoundExpr, BoundFts, BoundTerm, Derived, ExactInput, RecordContext, eval_bound_fts, eval_term,
};
use crate::repository::{Payload, Repository, SearchHit, SearchResult};
use crate::sorting::{SortColumn, SortKey, SortOptions};
use crate::source::{
    ExtractedRecord, RecordRangeReader, ScanProjection,
    read_record_range, read_record_ranges,
};
use crate::verified::{CachedMember, CachedResult};
use crate::workload::{ReturnMode, WorkCounters};


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
}


impl<'a> ResolvedSort<'a> {
    fn new(repo: &'a Repository, query: &Query) -> Self {
        let columns = repo
            .collections
            .iter()
            .map(|collection| {
                if !query.collections.is_empty()
                    && !query.collections.contains(&collection.definition.name)
                {
                    return None;
                }
                Some(
                    query
                        .options
                        .sort
                        .iter()
                        .map(|field| {
                            (
                                collection
                                    .sorting
                                    .column(&field.field)
                                    .expect("sort validated"),
                                field.descending,
                            )
                        })
                        .collect(),
                )
            })
            .collect();
        Self { columns }
    }

    fn compare_to_rank(
        &self,
        collection_index: usize,
        record: &RecordEntry,
        collection_name: &str,
        worst: &RankKey,
    ) -> Ordering {
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
        for ((a, descending, options), (b, _, _)) in self.parts.iter().zip(&other.parts) {
            let order = a.compare_with(b, *descending, *options);
            if !order.is_eq() {
                return order;
            }
        }
        (&self.collection, &self.id).cmp(&(&other.collection, &other.id))
    }
}


/// Bounded page candidate and its materialized result.
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
    evidence: HashSet<String>,
    payload: Option<Payload>,
    retained_bytes: u64,
}


/// Per-file scan work and verified matches returned by a worker.
#[derive(Default)]
struct ScannedFile {
    files_opened: u64,
    bytes_read: u64,
    record_parses: u64,
    scanned_records: u64,
    residual_evaluations: u64,
    match_count: u64,
    peak_payload_bytes: u64,
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
    remaining: AtomicUsize,
    complete: AtomicBool,
}


impl CacheBudget {
    fn new(bytes: usize) -> Self {
        Self {
            remaining: AtomicUsize::new(bytes),
            complete: AtomicBool::new(true),
        }
    }

    fn complete(&self) -> bool {
        self.complete.load(AtomicOrdering::Relaxed)
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
) -> ScanProjection {
    let collect_text = query.fts.is_some();
    if query.fts.as_ref().is_some_and(|fts| fts.fields.is_empty()) {
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
                if !fully_proven && term.path.root == PathRoot::Data
                    && !term.matcher.as_ref().is_some_and(|matcher| matcher.input() == ExactInput::OriginalRecordBytes) {
                    if let Some(first) = term.path.segments.first() {
                        fields.insert(first.clone());
                    }
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
    if let Some(fts) = &query.fts {
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
                .find(|proof| proof.term.span == term.span && proof.term.path == term.path),
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
        slot: u32,
        ctx: &RecordContext<'_>,
        evaluations: &mut u64,
    ) -> Result<bool, String> {
        Ok(match self {
            Self::Term(term, proof) => match Self::proven(*proof, slot) {
                Some(result) => result,
                None => {
                    *evaluations += 1;
                    eval_term(term, ctx)?
                }
            },
            Self::And(parts) => {
                let mut matched = true;
                for part in parts {
                    if !part.membership(slot, ctx, evaluations)? {
                        matched = false;
                        break;
                    }
                }
                matched
            }
            Self::Or(parts) => {
                let mut matched = false;
                for part in parts {
                    if part.membership(slot, ctx, evaluations)? {
                        matched = true;
                        break;
                    }
                }
                matched
            }
            Self::Not(inner) => !inner.membership(slot, ctx, evaluations)?,
        })
    }

    fn with_evidence(
        &self,
        slot: u32,
        ctx: &RecordContext<'_>,
        evidence: &mut HashSet<String>,
        evaluations: &mut u64,
    ) -> Result<bool, String> {
        Ok(match self {
            Self::Term(term, proof) => {
                let matched = match Self::proven(*proof, slot) {
                    Some(result) => result,
                    None => {
                        *evaluations += 1;
                        eval_term(term, ctx)?
                    }
                };
                if matched && self.can_emit_evidence() {
                    evidence.insert(term.path.display());
                }
                matched
            }
            Self::And(parts) => {
                let mut branch = HashSet::new();
                for part in parts {
                    if !part.with_evidence(slot, ctx, &mut branch, evaluations)? {
                        return Ok(false);
                    }
                }
                evidence.extend(branch);
                true
            }
            Self::Or(parts) => {
                let mut matched = false;
                for part in parts {
                    if matched && !part.can_emit_evidence() {
                        continue;
                    }
                    let mut branch = HashSet::new();
                    if part.with_evidence(slot, ctx, &mut branch, evaluations)? {
                        matched = true;
                        evidence.extend(branch);
                    }
                }
                matched
            }
            Self::Not(inner) => !inner.membership(slot, ctx, evaluations)?,
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
    projection: &ScanProjection,
    retain_all_evidence: bool,
    cache_budget: Option<&CacheBudget>,
    payload_cutoff: Option<&RankKey>,
    file_id: u32,
    ranges: &[(u32, ByteRange)],
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
    let mut reader = RecordRangeReader::open(&path, revision)?;
    outcome.files_opened = 1;
    let full_projection = ScanProjection::full(projection.collect_text);
    let mut cursor = 0;
    while cursor < ranges.len() {
        let mut end = cursor + 1;
        let mut batch_bytes = ranges[cursor].1.len();
        while end < ranges.len()
            && end - cursor < 128
            && batch_bytes.saturating_add(ranges[end].1.len()) <= 1024 * 1024
        {
            batch_bytes += ranges[end].1.len();
            end += 1;
        }
        let batch = &ranges[cursor..end];
        cursor = end;
        let (records, bytes_read) = reader.read(batch)?;
        outcome.bytes_read += bytes_read;
        for (slot, bytes) in records {
            let record = &collection.generation.records[slot as usize];
            let file = &repo.files.files[file_id as usize];
            let may_reach_page = keep_cap > 0
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
            let record_projection =
                if query.options.return_mode == ReturnMode::Logical && may_reach_page {
                    &full_projection
                } else {
                    projection
                };
            let decode = (query.options.return_mode == ReturnMode::Logical && may_reach_page)
                || record_projection.collect_text
                || record_projection.top_level.as_ref().is_none_or(|fields| !fields.is_empty());
            let extracted = if decode {
                let extracted = prepare_query_record(&collection.source,
                    &collection.definition.schema, &bytes, record_projection,
                    query.options.return_mode == ReturnMode::Logical && may_reach_page)?;
                outcome.record_parses += 1;
                extracted
            } else {
                ExtractedRecord { value: Value::Object(Default::default()), text: Vec::new() }
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
            let fts_match = fts
                .map(|fts| eval_bound_fts(fts, &extracted.text, &ctx, &mut evidence))
                .transpose()?
                .unwrap_or(true);
            let filter_match = if fts_match {
                residual
                    .map(|f| {
                        if query.options.evidence
                            && ((retain_all_evidence
                                && cache_budget.is_some_and(CacheBudget::complete))
                                || may_reach_page)
                        {
                            f.with_evidence(
                                slot,
                                &ctx,
                                &mut evidence,
                                &mut outcome.residual_evaluations,
                            )
                        } else {
                            f.membership(slot, &ctx, &mut outcome.residual_evaluations)
                        }
                    })
                    .transpose()?
                    .unwrap_or(true)
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
                if may_reach_page {
                    let retained = if query.options.return_mode == ReturnMode::Locator {
                        0
                    } else {
                        bytes.len() as u64
                    };
                    let payload = materialize(&query.options.return_mode, bytes, Some(extracted))?;
                    if winners.len() == keep_cap {
                        retained_bytes -= winners.pop().unwrap().matched.retained_bytes;
                    }
                    retained_bytes += retained;
                    outcome.peak_payload_bytes = outcome.peak_payload_bytes.max(retained_bytes);
                    winners.push(FileWinner {
                        key: rank_key(sort, repo, collection_index, record),
                        matched: ScannedMatch {
                            slot,
                            evidence,
                            payload,
                            retained_bytes: retained,
                        },
                    });
                }
            }
        }
    }
    outcome.matches = winners
        .into_sorted_vec()
        .into_iter()
        .map(|winner| winner.matched)
        .collect();
    Ok(outcome)
}


/// Explain output for a collection plan and its actual work.
#[derive(Clone, Debug, Serialize)]
pub struct QueryPlanTrace {
    pub collection: String,
    pub generation: u64,
    pub input_records: u64,
    pub fts_strategy: String,
    pub filter_exact: bool,
    pub terms: Vec<TermChoice>,
    pub candidates: u64,
    pub actual: Option<PlanActual>,
}


/// Observed source and predicate work for one plan.
#[derive(Clone, Debug, Serialize)]
pub struct PlanActual {
    pub files_opened: u64,
    pub bytes_read: u64,
    pub record_parses: u64,
    pub residual_evaluations: u64,
    pub index_proven_matches: u64,
}


impl Default for PlanActual {
    fn default() -> Self {
        Self {
            files_opened: 0,
            bytes_read: 0,
            record_parses: 0,
            residual_evaluations: 0,
            index_proven_matches: 0,
        }
    }
}


fn finish_trace(traces: &mut [QueryPlanTrace], before: &WorkCounters, after: &WorkCounters) {
    if let Some(trace) = traces.last_mut() {
        trace.actual = Some(PlanActual {
            files_opened: after.files_opened - before.files_opened,
            bytes_read: after.bytes_read - before.bytes_read,
            record_parses: after.record_parses - before.record_parses,
            residual_evaluations: after.residual_evaluations - before.residual_evaluations,
            index_proven_matches: after.exact_posting_matches - before.exact_posting_matches,
        });
    }
}

fn proof_evidence(expr: &BoundExpr, slot: u32, proofs: &[Proof]) -> Option<HashSet<String>> {
    match expr {
        BoundExpr::Term(term) => {
            let proof = proofs
                .iter()
                .find(|proof| proof.term.span == term.span && proof.term.path == term.path)?;
            if !proof.matches.contains(slot) {
                return None;
            }
            let mut evidence = HashSet::new();
            if term.derived == Derived::None
                && !matches!(term.operator, Operator::Ne | Operator::NotContains)
            {
                evidence.insert(term.path.display());
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
    winners: &mut BinaryHeap<Winner>,
    cap: usize,
    counters: &mut WorkCounters,
    mut cache_members: Option<&mut Vec<CachedMember>>,
    cache_budget: Option<&CacheBudget>,
) {
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
    if cap == 0 {
        return;
    }
    if winners.len() == cap
        && winners.peek().is_some_and(|worst| {
            !sort
                .compare_to_rank(
                    collection_index,
                    record,
                    &repo.collections[collection_index].definition.name,
                    &worst.key,
                )
                .is_lt()
        })
    {
        return;
    }
    let key = rank_key(sort, repo, collection_index, record);
    if winners.len() == cap {
        winners.pop();
    }
    let retained_bytes = if payload.is_some() {
        record.range.len()
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
}

pub fn search(repo: &Repository, query: &Query) -> Result<SearchResult, String> {
    let budget = {
        let cache = repo.verified.lock().map_err(|e| e.to_string())?;
        if cache.enabled() { cache.budget() } else { 0 }
    };
    if budget == 0 {
        return search_impl(repo, query, None, None, None);
    }
    let key = crate::verified::key(query);
    let cached = repo.verified.lock().map_err(|e| e.to_string())?.get(&key);
    if let Some(cached) = cached {
        return materialize_cached(repo, query, &cached);
    }
    if !query.options.count {
        return search_impl(repo, query, None, None, None);
    }
    let mut members = Vec::new();
    let tracker = CacheBudget::new(budget.saturating_sub(key.len() + 128));
    let result = search_impl(repo, query, None, Some(&mut members), Some(&tracker))?;
    if tracker.complete() && Some(members.len()) == result.count {
        sort_cached_members(repo, query, &mut members);
        repo.verified
            .lock()
            .map_err(|e| e.to_string())?
            .insert(key, members);
    }
    Ok(result)
}

fn sort_cached_members(repo: &Repository, query: &Query, members: &mut Vec<CachedMember>) {
    let sort = ResolvedSort::new(repo, query);
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
    column: Option<&'a SortColumn>,
    order: &'a [crate::model::RecordSlot],
    record: &'a RecordEntry,
    collection_name: &'a str,
    collection_index: usize,
    plan_index: usize,
    position: usize,
    descending: bool,
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
        let keys = match (self.column, other.column) {
            (Some(a), Some(b)) => a.keys[self.record.slot.0 as usize].compare_with(
                &b.keys[other.record.slot.0 as usize],
                self.descending,
                a.options,
            ),
            _ => Ordering::Equal,
        };
        keys.then_with(|| {
            (self.collection_name, &self.record.id).cmp(&(other.collection_name, &other.record.id))
        })
    }
}


fn stored_cursor<'a>(
    repo: &'a Repository,
    column: Option<&'a SortColumn>,
    order: &'a [crate::model::RecordSlot],
    collection_index: usize,
    plan_index: usize,
    position: usize,
    descending: bool,
) -> Option<StoredCursor<'a>> {
    let slot = order.get(position)?;
    let collection = &repo.collections[collection_index];
    Some(StoredCursor {
        column,
        order,
        record: &collection.generation.records[slot.0 as usize],
        collection_name: &collection.definition.name,
        collection_index,
        plan_index,
        position,
        descending,
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
) -> Result<Option<Payload>, String> {
    if query.options.return_mode == ReturnMode::Locator {
        return Ok(None);
    }
    let collection = &repo.collections[collection_index];
    let path = repo
        .files
        .native_path(record.file)
        .ok_or("missing source path")?;
    let bytes = read_record_range(&path, record.revision, record.range)?;
    counters.files_opened += 1;
    counters.bytes_read += bytes.len() as u64;
    actual.files_opened += 1;
    actual.bytes_read += bytes.len() as u64;
    let logical = if query.options.return_mode == ReturnMode::Logical {
        counters.record_parses += 1;
        actual.record_parses += 1;
        Some(prepare_query_record(&collection.source, &collection.definition.schema,
            &bytes, &ScanProjection::full(false), true)?)
    } else { None };
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
) -> Result<Option<SearchResult>, String> {
    if query.options.count || query.options.sort.len() > 1 {
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
    if cap > 0 && candidate_total > 0 && candidate_total < (cap as u64).saturating_mul(4) {
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
        .map(|(entry, residual)| scan_projection(query, residual.as_ref(), &entry.candidates))
        .collect::<Vec<_>>();
    if let Some(traces) = traces.as_deref_mut() {
        for entry in &plans {
            let collection = &repo.collections[entry.collection_index];
            traces.push(QueryPlanTrace {
                collection: collection.definition.name.clone(),
                generation: collection.generation.generation,
                input_records: collection.generation.records.len() as u64,
                fts_strategy: "stored order page traversal".into(),
                filter_exact: entry.plan.exact,
                terms: entry.plan.choices.clone(),
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
    let mut heap = BinaryHeap::new();
    for (plan_index, entry) in plans.iter().enumerate() {
        let (column, order, descending) = if let Some(field) = query.options.sort.first() {
            let column = sort.columns[entry.collection_index]
                .as_ref()
                .expect("selected collection")[0]
                .0;
            let order = if field.descending {
                &column.descending
            } else {
                &column.ascending
            };
            (Some(column), order.as_slice(), field.descending)
        } else {
            (
                None,
                repo.collections[entry.collection_index]
                    .identity_order
                    .as_slice(),
                false,
            )
        };
        if let Some(cursor) = stored_cursor(
            repo,
            column,
            order,
            entry.collection_index,
            plan_index,
            0,
            descending,
        ) {
            heap.push(Reverse(cursor));
        }
    }
    let mut page = Vec::new();
    let mut stopped_early = false;
    let mut locator_query = query.clone();
    locator_query.options.return_mode = ReturnMode::Locator;
    while cap > 0 {
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
                        .and_then(|expr| proof_evidence(expr, slot, &entry.plan.proofs))
                        .unwrap_or_default();
                }
                if counters.total_matches as usize >= query.options.offset {
                    payload = direct_payload(
                        repo,
                        query,
                        entry.collection_index,
                        cursor.record,
                        &mut counters,
                        actual,
                    )?;
                }
            } else {
                let scan_query = if counters.total_matches as usize >= query.options.offset {
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
                    &projections[cursor.plan_index],
                    false,
                    None,
                    None,
                    cursor.record.file.0,
                    &[(slot, cursor.record.range)],
                )?;
                counters.files_opened += scanned.files_opened;
                counters.bytes_read += scanned.bytes_read;
                counters.record_parses += scanned.record_parses;
                counters.scanned_records += scanned.scanned_records;
                counters.residual_evaluations += scanned.residual_evaluations;
                actual.files_opened += scanned.files_opened;
                actual.bytes_read += scanned.bytes_read;
                actual.record_parses += scanned.record_parses;
                actual.residual_evaluations += scanned.residual_evaluations;
                if let Some(found) = scanned.matches.into_iter().next() {
                    matched = true;
                    evidence = found.evidence;
                    payload = found.payload;
                }
            }
            if matched {
                if counters.total_matches as usize >= query.options.offset {
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
                if counters.total_matches as usize >= cap {
                    stopped_early = true;
                    break;
                }
            }
        }
        if let Some(next) = stored_cursor(
            repo,
            cursor.column,
            cursor.order,
            cursor.collection_index,
            cursor.plan_index,
            cursor.position + 1,
            cursor.descending,
        ) {
            heap.push(Reverse(next));
        }
    }
    counters.returned_records = page.len() as u64;
    counters.match_count_complete = !stopped_early && cap > 0;
    if let Some(traces) = traces.as_deref_mut() {
        for (trace, actual) in traces.iter_mut().zip(actuals) {
            trace.actual = Some(actual);
        }
    }
    Ok(Some(SearchResult {
        count: None,
        hits: page,
        counters,
    }))
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
            payload: None,
        });
    }
    for ((collection_index, file_id), ranges) in files {
        let collection = &repo.collections[collection_index];
        let path = repo
            .files
            .native_path(crate::model::FileId(file_id))
            .ok_or("missing source path")?;
        let revision = repo.files.files[file_id as usize].revision;
        for batch in ranges.chunks(128) {
            let (records, bytes_read) = read_record_ranges(&path, revision, batch)?;
            counters.files_opened += 1;
            counters.bytes_read += bytes_read;
            for (index, bytes) in records {
                hits[index as usize].payload = Some(match query.options.return_mode {
                    ReturnMode::Locator => unreachable!(),
                    ReturnMode::Bytes => Payload::Bytes(bytes),
                    ReturnMode::Logical => {
                        counters.record_parses += 1;
                        let mut extracted = collection.source.reader().parse(&bytes)?;
                        collection
                            .definition
                            .schema
                            .coerce_record(&mut extracted.value)?;
                        Payload::Logical(extracted.value)
                    }
                });
            }
        }
    }
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
    let result = search_impl(repo, query, Some(&mut traces), None, None)?;
    Ok((result, traces))
}

pub fn explain(repo: &Repository, query: &Query) -> Result<Vec<QueryPlanTrace>, String> {
    let (prepared, _) = crate::prepared::prepare(repo, query)?;
    let mut traces = Vec::new();
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
            });
        traces.push(QueryPlanTrace {
            collection: collection.definition.name.clone(),
            generation: collection.generation.generation,
            input_records: domain.len(),
            fts_strategy: if let Some(fts) = &fts {
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
            filter_exact: plan.exact,
            terms: plan.choices,
            candidates: plan
                .candidates
                .as_ref()
                .map(RoaringBitmap::len)
                .unwrap_or(domain.len()),
            actual: None,
        });
    }
    Ok(traces)
}

fn search_impl(
    repo: &Repository,
    query: &Query,
    mut traces: Option<&mut Vec<QueryPlanTrace>>,
    mut cache_members: Option<&mut Vec<CachedMember>>,
    cache_budget: Option<&CacheBudget>,
) -> Result<SearchResult, String> {
    let preparation_start = Instant::now();
    let (prepared, plan_hit) = crate::prepared::prepare(repo, query)?;
    let sort = ResolvedSort::new(repo, query);
    if cache_members.is_none() {
        if let Some(result) = try_stored_order_page(
            repo,
            query,
            &prepared,
            &sort,
            plan_hit,
            preparation_start.elapsed().as_nanos() as u64,
            traces.as_deref_mut(),
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
    let mut counters = WorkCounters::default();
    counters.preparation_ns = preparation_start.elapsed().as_nanos() as u64;
    counters.prepared_plan_hit = plan_hit;
    for (collection_index, filter, fts) in prepared.collections {
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
            });
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
        let inventory_evidence = if metadata_only_fts {
            let matches =
                inventory_fts_evidence(repo, collection, fts.as_ref().unwrap(), &candidates)?;
            candidates = matches.keys().copied().collect();
            Some(matches)
        } else {
            None
        };
        counters.planning_ns += planning_start.elapsed().as_nanos() as u64;
        counters.candidates += candidates.len();
        let needs_scan = !plan.exact || (fts.is_some() && !metadata_only_fts);
        if let Some(traces) = traces.as_deref_mut() {
            traces.push(QueryPlanTrace {
                collection: collection.definition.name.clone(),
                generation: collection.generation.generation,
                input_records: domain.len(),
                fts_strategy: if fts.is_none() {
                    "absent"
                } else if metadata_only_fts {
                    "metadata inventory"
                } else if text_first {
                    "bigram before filter"
                } else if candidates.len() < domain.len() {
                    "filter before text verification"
                } else {
                    "text verification scan"
                }
                .into(),
                filter_exact: plan.exact,
                terms: plan.choices.clone(),
                candidates: candidates.len(),
                actual: None,
            });
        }
        if !needs_scan {
            for slot in candidates.iter() {
                let record = &collection.generation.records[slot as usize];
                let mut evidence = if query.options.evidence {
                    filter
                        .as_ref()
                        .and_then(|expr| proof_evidence(expr, slot, &plan.proofs))
                        .unwrap_or_default()
                } else {
                    HashSet::new()
                };
                if query.options.evidence {
                    if let Some(fts_evidence) =
                        inventory_evidence.as_ref().and_then(|map| map.get(&slot))
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
                    &mut winners,
                    cap,
                    &mut counters,
                    cache_members.as_deref_mut(),
                    cache_budget,
                );
            }
            if let (Some(traces), Some(before)) = (traces.as_deref_mut(), before.as_ref()) {
                finish_trace(traces, before, &counters);
            }
            continue;
        }
        if fts.is_none() && !query.options.evidence {
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
                    &mut winners,
                    cap,
                    &mut counters,
                    cache_members.as_deref_mut(),
                    cache_budget,
                );
            }
            candidates -= &proven;
        }
        let residual = filter
            .as_ref()
            .map(|expr| prepare_residual(expr, &plan.proofs, collection, candidates.len()));
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
        let projection = scan_projection(query, residual.as_ref(), &candidates);
        let retain_all_evidence = cache_budget.is_some_and(CacheBudget::complete);
        let parallel = repo.options.query_workers > 1 && files.len() > 1;
        // Bound in-flight parsed records while workers scan independent files.
        // Applying offers in file order preserves deterministic tie handling.
        let chunk_size =
            if query.options.return_mode == ReturnMode::Locator || query.options.limit == 0 {
                256
            } else {
                repo.options.query_workers.min(64).max(1)
            };
        for chunk in files.chunks(chunk_size) {
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
                                fts.as_ref(),
                                &projection,
                                retain_all_evidence,
                                cache_budget,
                                payload_cutoff.as_ref(),
                                *file_id,
                                ranges,
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
                            fts.as_ref(),
                            &projection,
                            retain_all_evidence,
                            cache_budget,
                            payload_cutoff.as_ref(),
                            *file_id,
                            ranges,
                        )
                    })
                    .collect()
            };
            let payload_upper = scanned
                .iter()
                .filter_map(|result| result.as_ref().ok())
                .map(|outcome| outcome.peak_payload_bytes)
                .sum::<u64>()
                + winners
                    .iter()
                    .map(|winner: &Winner| winner.retained_bytes)
                    .sum::<u64>();
            counters.peak_retained_payload_bytes =
                counters.peak_retained_payload_bytes.max(payload_upper);
            for outcome in scanned {
                let outcome = outcome?;
                counters.files_opened += outcome.files_opened;
                counters.bytes_read += outcome.bytes_read;
                counters.record_parses += outcome.record_parses;
                counters.scanned_records += outcome.scanned_records;
                counters.residual_evaluations += outcome.residual_evaluations;
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
                        &mut winners,
                        cap,
                        &mut counters,
                        None,
                        None,
                    );
                }
            }
        }
        if let (Some(traces), Some(before)) = (traces.as_deref_mut(), before.as_ref()) {
            finish_trace(traces, before, &counters);
        }
    }
    let count = counters.total_matches as usize;
    counters.match_count_complete = true;
    let ordered = winners.into_sorted_vec();
    let mut page: Vec<_> = ordered
        .into_iter()
        .skip(query.options.offset)
        .take(query.options.limit)
        .collect();
    // Exact index membership may need a page payload. Read only those ranges.
    if query.options.return_mode != ReturnMode::Locator {
        let mut files: BTreeMap<(usize, u32), Vec<(u32, ByteRange)>> = BTreeMap::new();
        for (index, winner) in page.iter().enumerate() {
            if winner.hit.payload.is_none() {
                let record = &repo.collections[winner.collection_index].generation.records
                    [winner.slot as usize];
                files
                    .entry((winner.collection_index, record.file.0))
                    .or_default()
                    .push((index as u32, record.range));
            }
        }
        for ((collection_index, file_id), ranges) in files {
            let collection = &repo.collections[collection_index];
            let path = repo
                .files
                .native_path(crate::model::FileId(file_id))
                .ok_or("missing source path")?;
            let revision = repo.files.files[file_id as usize].revision;
            for batch in ranges.chunks(128) {
                let (records, bytes_read) = read_record_ranges(&path, revision, batch)?;
                counters.files_opened += 1;
                counters.bytes_read += bytes_read;
                if let Some(traces) = traces.as_deref_mut() {
                    if let Some(actual) = traces
                        .iter_mut()
                        .find(|trace| trace.collection == collection.definition.name)
                        .and_then(|trace| trace.actual.as_mut())
                    {
                        actual.files_opened += 1;
                        actual.bytes_read += bytes_read;
                        if query.options.return_mode == ReturnMode::Logical {
                            actual.record_parses += records.len() as u64;
                        }
                    }
                }
                for (index, bytes) in records {
                    let payload = match query.options.return_mode {
                        ReturnMode::Locator => unreachable!(),
                        ReturnMode::Bytes => Payload::Bytes(bytes),
                        ReturnMode::Logical => {
                            counters.record_parses += 1;
                            let mut extracted = collection.source.reader().parse(&bytes)?;
                            collection
                                .definition
                                .schema
                                .coerce_record(&mut extracted.value)?;
                            Payload::Logical(extracted.value)
                        }
                    };
                    page[index as usize].hit.payload = Some(payload);
                }
            }
        }
    }
    counters.returned_records = page.len() as u64;
    Ok(SearchResult {
        count: query.options.count.then_some(count),
        hits: page.into_iter().map(|winner| winner.hit).collect(),
        counters,
    })
}

#[cfg(test)]
#[path = "run.test.rs"]
mod tests;
