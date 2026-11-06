//! Bounded result retention, grouped source access, and payload materialization.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet};
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
use crate::reference::{
    BoundExpr, BoundFts, Derived, RecordContext, eval_bound_fts, eval_filter, eval_term,
};
use crate::repository::{Payload, Repository, SearchHit, SearchResult};
use crate::sorting::{SortKey, SortOptions};
use crate::source::{ScanProjection, read_record_ranges, text_segments};
use crate::verified::{CachedMember, CachedResult};
use crate::workload::{ReturnMode, WorkCounters};


/// Comparable sort key with collection and ID tie breakers.
#[derive(Clone)]
struct RankKey {
    parts: Vec<(SortKey, bool, SortOptions)>,
    collection: String,
    id: String,
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
}


/// Verified record and any evidence or retained payload.
struct ScannedMatch {
    slot: u32,
    evidence: HashSet<String>,
    payload: Option<Payload>,
}


/// Per-file scan work and verified matches returned by a worker.
#[derive(Default)]
struct ScannedFile {
    files_opened: u64,
    bytes_read: u64,
    record_parses: u64,
    scanned_records: u64,
    residual_evaluations: u64,
    matches: SmallVec<[ScannedMatch; 1]>,
}


fn scan_projection(query: &Query, filter: Option<&BoundExpr>) -> ScanProjection {
    let collect_text = query.fts.is_some();
    if query.fts.as_ref().is_some_and(|fts| fts.fields.is_empty()) {
        return ScanProjection::full(true);
    }

    fn add_filter_fields(expr: &BoundExpr, fields: &mut HashSet<String>) {
        match expr {
            BoundExpr::Term(term) => {
                if term.path.root == PathRoot::Data {
                    if let Some(first) = term.path.segments.first() {
                        fields.insert(first.clone());
                    }
                }
            }
            BoundExpr::And(parts) | BoundExpr::Or(parts) => {
                for part in parts {
                    add_filter_fields(part, fields);
                }
            }
            BoundExpr::Not(inner) => add_filter_fields(inner, fields),
        }
    }
    let mut fields = HashSet::new();
    if let Some(filter) = filter {
        add_filter_fields(filter, &mut fields);
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

fn eval_filter_without_evidence(expr: &BoundExpr, ctx: &RecordContext<'_>) -> Result<bool, String> {
    Ok(match expr {
        BoundExpr::Term(term) => eval_term(term, ctx)?,
        BoundExpr::And(parts) => {
            let mut result = true;
            for part in parts {
                result &= eval_filter_without_evidence(part, ctx)?;
            }
            result
        }
        BoundExpr::Or(parts) => {
            let mut result = false;
            for part in parts {
                result |= eval_filter_without_evidence(part, ctx)?;
            }
            result
        }
        BoundExpr::Not(inner) => !eval_filter_without_evidence(inner, ctx)?,
    })
}

fn scan_file(
    repo: &Repository,
    query: &Query,
    collection_index: usize,
    filter: Option<&BoundExpr>,
    fts: Option<&BoundFts>,
    projection: &ScanProjection,
    retain_all_evidence: bool,
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
    let full_projection = ScanProjection::full(projection.collect_text);
    for batch in ranges.chunks(128) {
        let (records, bytes_read) = read_record_ranges(&path, revision, batch)?;
        outcome.files_opened += 1;
        outcome.bytes_read += bytes_read;
        for (slot, bytes) in records {
            let record = &collection.generation.records[slot as usize];
            let file = &repo.files.files[file_id as usize];
            let may_reach_page = query.options.limit > 0
                && payload_cutoff.is_none_or(|worst| {
                    if query.options.sort.is_empty() {
                        (&collection.definition.name, &record.id.0) < (&worst.collection, &worst.id)
                    } else {
                        rank_key(repo, query, collection_index, record) < *worst
                    }
                });
            let record_projection =
                if query.options.return_mode == ReturnMode::Logical && may_reach_page {
                    &full_projection
                } else {
                    projection
                };
            let mut extracted = collection
                .source
                .reader()
                .parse_for_query(&bytes, record_projection)?;
            collection.definition.schema.coerce_projected_record(
                &mut extracted.value,
                record_projection.top_level.as_ref(),
            )?;
            if projection.collect_text && collection.definition.schema.has_temporal_fields() {
                extracted.text = text_segments(&extracted.value);
            }
            outcome.record_parses += 1;
            outcome.scanned_records += 1;
            let ctx = RecordContext {
                value: &extracted.value,
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
            let filter_match = filter
                .map(|f| {
                    if query.options.evidence && (retain_all_evidence || may_reach_page) {
                        eval_filter(f, &ctx, &mut evidence)
                    } else {
                        eval_filter_without_evidence(f, &ctx)
                    }
                })
                .transpose()?
                .unwrap_or(true);
            outcome.residual_evaluations += u64::from(filter.is_some());
            if fts_match && filter_match {
                let payload = match (&query.options.return_mode, may_reach_page) {
                    (_, false) | (ReturnMode::Locator, _) => None,
                    (ReturnMode::Bytes, true) => Some(Payload::Bytes(bytes)),
                    (ReturnMode::Logical, true) => Some(Payload::Logical(extracted.value)),
                };
                outcome.matches.push(ScannedMatch {
                    slot,
                    evidence,
                    payload,
                });
            }
        }
    }
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
    repo: &Repository,
    query: &Query,
    collection_index: usize,
    record: &RecordEntry,
) -> RankKey {
    let collection = &repo.collections[collection_index];
    let parts = query
        .options
        .sort
        .iter()
        .map(|field| {
            let column = collection
                .sorting
                .column(&field.field)
                .expect("sort validated");
            let key = collection
                .sorting
                .key(&field.field, record.slot)
                .expect("sort validated")
                .clone();
            (key, field.descending, column.options)
        })
        .collect();
    RankKey {
        parts,
        collection: collection.definition.name.clone(),
        id: record.id.0.clone(),
    }
}

fn offer(
    repo: &Repository,
    query: &Query,
    collection_index: usize,
    record: &RecordEntry,
    evidence: HashSet<String>,
    payload: Option<Payload>,
    winners: &mut BinaryHeap<Winner>,
    cap: usize,
    counters: &mut WorkCounters,
    mut cache_members: Option<&mut Vec<CachedMember>>,
) {
    counters.total_matches += 1;
    let mut matched_fields: Vec<_> = if query.options.evidence {
        evidence.into_iter().collect()
    } else {
        Vec::new()
    };
    matched_fields.sort();
    if let Some(members) = cache_members.as_deref_mut() {
        members.push(CachedMember {
            collection_index,
            slot: record.slot.0,
            matched_fields: matched_fields.clone(),
        });
    }
    if cap == 0 {
        return;
    }
    let key = rank_key(repo, query, collection_index, record);
    if winners.len() == cap && winners.peek().is_some_and(|worst| worst.key <= key) {
        return;
    }
    if winners.len() == cap {
        winners.pop();
    }
    let file = &repo.files.files[record.file.0 as usize];
    let hit = SearchHit {
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
    };
    winners.push(Winner {
        key,
        collection_index,
        slot: record.slot.0,
        hit,
    });
}

pub fn search(repo: &Repository, query: &Query) -> Result<SearchResult, String> {
    let enabled = repo.verified.lock().map_err(|e| e.to_string())?.enabled();
    if !enabled {
        return search_impl(repo, query, None, None);
    }
    let key = crate::verified::key(query);
    let cached = repo.verified.lock().map_err(|e| e.to_string())?.get(&key);
    if let Some(cached) = cached {
        return materialize_cached(repo, query, &cached);
    }
    let mut members = Vec::new();
    let result = search_impl(repo, query, None, Some(&mut members))?;
    debug_assert_eq!(members.len(), result.count);
    sort_cached_members(repo, query, &mut members);
    repo.verified
        .lock()
        .map_err(|e| e.to_string())?
        .insert(key, members);
    Ok(result)
}

fn sort_cached_members(repo: &Repository, query: &Query, members: &mut Vec<CachedMember>) {
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
        let mut keyed: Vec<_> = members
            .drain(..)
            .map(|member| {
                let record = &repo.collections[member.collection_index].generation.records
                    [member.slot as usize];
                (
                    rank_key(repo, query, member.collection_index, record),
                    member,
                )
            })
            .collect();
        keyed.sort_by(|a, b| a.0.cmp(&b.0));
        members.extend(keyed.into_iter().map(|(_, member)| member));
    }
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
        count: cached.members.len(),
        hits,
        counters,
    })
}

pub fn search_analyze(
    repo: &Repository,
    query: &Query,
) -> Result<(SearchResult, Vec<QueryPlanTrace>), String> {
    let mut traces = Vec::new();
    let result = search_impl(repo, query, Some(&mut traces), None)?;
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
            .map(|f| plan_expr_restricted(repo, collection, f, &domain, &domain))
            .transpose()?
            .unwrap_or(Plan {
                candidates: None,
                exact: true,
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
) -> Result<SearchResult, String> {
    let preparation_start = Instant::now();
    let (prepared, plan_hit) = crate::prepared::prepare(repo, query)?;
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
            .map(|f| plan_expr_restricted(repo, collection, f, &domain, &input))
            .transpose()?
            .unwrap_or(Plan {
                candidates: None,
                exact: true,
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
                    collection_index,
                    record,
                    evidence,
                    None,
                    &mut winners,
                    cap,
                    &mut counters,
                    cache_members.as_deref_mut(),
                );
            }
            if let (Some(traces), Some(before)) = (traces.as_deref_mut(), before.as_ref()) {
                finish_trace(traces, before, &counters);
            }
            continue;
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
        let projection = scan_projection(query, filter.as_ref());
        let retain_all_evidence = cache_members.is_some();
        let parallel = repo.options.query_workers > 1 && files.len() >= 128;
        // Bound in-flight parsed records while workers scan independent files.
        // Applying offers in file order preserves deterministic tie handling.
        for chunk in files.chunks(256) {
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
                                collection_index,
                                filter.as_ref(),
                                fts.as_ref(),
                                &projection,
                                retain_all_evidence,
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
                            collection_index,
                            filter.as_ref(),
                            fts.as_ref(),
                            &projection,
                            retain_all_evidence,
                            payload_cutoff.as_ref(),
                            *file_id,
                            ranges,
                        )
                    })
                    .collect()
            };
            for outcome in scanned {
                let outcome = outcome?;
                counters.files_opened += outcome.files_opened;
                counters.bytes_read += outcome.bytes_read;
                counters.record_parses += outcome.record_parses;
                counters.scanned_records += outcome.scanned_records;
                counters.residual_evaluations += outcome.residual_evaluations;
                for matched in outcome.matches {
                    let record = &collection.generation.records[matched.slot as usize];
                    offer(
                        repo,
                        query,
                        collection_index,
                        record,
                        matched.evidence,
                        matched.payload,
                        &mut winners,
                        cap,
                        &mut counters,
                        cache_members.as_deref_mut(),
                    );
                }
            }
        }
        if let (Some(traces), Some(before)) = (traces.as_deref_mut(), before.as_ref()) {
            finish_trace(traces, before, &counters);
        }
    }
    let count = counters.total_matches as usize;
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
        count,
        hits: page.into_iter().map(|winner| winner.hit).collect(),
        counters,
    })
}

#[cfg(test)]
#[path = "run.test.rs"]
mod tests;
