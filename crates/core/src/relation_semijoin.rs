//! Bounded exact positive-witness semijoins for forward relation predicates.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use roaring::RoaringBitmap;

use crate::executor::{
    AccessPathEstimate, AccessPathKind, Plan, Proof, estimate_expr, estimate_fts_candidates,
};
use crate::inventory::RecordClass;
use crate::lifecycle::PublishedSegment;
use crate::model::{FileId, RecordSlot};
use crate::native_segment::{SegmentRecordAddress, SegmentRecordSet};
use crate::query::Quantifier;
use crate::reference::{
    BoundExpr, BoundFts, BoundTerm, RecordContext, eval_bound_fts_membership, eval_term,
    same_bound_term,
};
use crate::relation::{RelationCardinality, RelationTarget};
use crate::relation_query::{BoundRelationHop, HopDirection};
use crate::repository::Repository;
use crate::source::{ScanProjection, TextSegment};
use crate::source_access::SourceAccessSession;
use crate::streaming::{ExecutionContext, ExecutionErrorKind};

// These are query-local limits. Hitting one is an explicit error, never a
// silently truncated candidate set or count.
const MAX_INTERMEDIATE_SLOTS: u64 = 5_000_000;
const MAX_VISITED_EDGES: u64 = 32_000_000;
const MAX_TARGET_RECORD_BYTES: u64 = 64 * 1024 * 1024;
const TARGET_BATCH_BYTES: u64 = 1024 * 1024;

/// Physical strategy chosen for one positive relation term.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Strategy {
    TargetFirst,
    SourceRestricted,
}

/// Exact source membership and selected-field evidence for relation FTS.
pub(crate) struct FtsWitnesses {
    pub evidence: HashMap<u32, HashSet<String>>,
    pub qualified_evidence: HashMap<SegmentRecordAddress, HashSet<String>>,
    pub work: SemijoinWork,
    pub strategies: Vec<Strategy>,
}

/// Query-local work performed while constructing exact relation witnesses.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SemijoinWork {
    pub target_candidates: u64,
    pub target_matches: u64,
    pub target_reads: u64,
    pub target_file_opens: u64,
    pub target_bytes_read: u64,
    pub edge_visits: u64,
    pub reverse_postings_visited: u64,
    pub reverse_members_visited: u64,
    pub verified_targets_reused: u64,
}

impl SemijoinWork {
    pub fn add(&mut self, other: Self) {
        self.target_candidates += other.target_candidates;
        self.target_matches += other.target_matches;
        self.target_reads += other.target_reads;
        self.target_file_opens += other.target_file_opens;
        self.target_bytes_read += other.target_bytes_read;
        self.edge_visits += other.edge_visits;
        self.reverse_postings_visited += other.reverse_postings_visited;
        self.reverse_members_visited += other.reverse_members_visited;
        self.verified_targets_reused += other.verified_targets_reused;
    }
}

fn ensure_slots(count: u64, execution: Option<&ExecutionContext>) -> Result<(), String> {
    if let Some(execution) = execution {
        return execution.check_slots(count);
    }
    if count > MAX_INTERMEDIATE_SLOTS {
        Err(format!(
            "relation semijoin intermediate exceeds {MAX_INTERMEDIATE_SLOTS} slots"
        ))
    } else {
        Ok(())
    }
}

fn visit_edge(work: &mut SemijoinWork, execution: Option<&ExecutionContext>) -> Result<(), String> {
    if let Some(execution) = execution {
        execution.charge_edges(1)?;
    }
    work.edge_visits = work
        .edge_visits
        .checked_add(1)
        .ok_or("relation edge count overflow")?;
    if work.edge_visits > MAX_VISITED_EDGES {
        return Err(format!(
            "relation semijoin exceeds {MAX_VISITED_EDGES} edge visits"
        ));
    }
    Ok(())
}

fn positive_terms<'a>(expr: &'a BoundExpr, output: &mut Vec<&'a BoundTerm>) {
    match expr {
        BoundExpr::Term(term) => output.push(term),
        BoundExpr::And(parts) => {
            for part in parts {
                positive_terms(part, output);
            }
        }
        BoundExpr::Or(_) | BoundExpr::Not(_) => {}
    }
}

fn all_terms_proven(expr: &BoundExpr, proofs: &[Proof], survivors: &RoaringBitmap) -> bool {
    match expr {
        BoundExpr::Term(term) => proofs.iter().any(|proof| {
            same_bound_term(&proof.term, term)
                && survivors.is_subset(&proof.input)
                && survivors.is_subset(&proof.matches)
        }),
        BoundExpr::And(parts) => parts
            .iter()
            .all(|part| all_terms_proven(part, proofs, survivors)),
        BoundExpr::Or(_) | BoundExpr::Not(_) => false,
    }
}

fn eligible<'a>(repo: &Repository, term: &'a BoundTerm) -> Option<&'a [BoundRelationHop]> {
    let related = term.related.as_ref()?;
    if related.source_operand.is_some()
        || related.hops.is_empty()
        || related.hops.iter().any(|hop| {
            hop.direction != HopDirection::Forward || hop.selectors.iter().any(Option::is_some)
        })
    {
        return None;
    }
    if term.quantifier == Quantifier::Default {
        for hop in &related.hops {
            let index = relation(repo, hop).ok()?;
            if index.declaration.cardinality != RelationCardinality::One
                || !hop.selectors.is_empty()
            {
                return None;
            }
            let source = &repo.collections[hop.from.0 as usize];
            for depth in 1..index.declaration.path.len() {
                let ancestor = source
                    .definition
                    .schema
                    .fields()
                    .get(&index.declaration.path[..depth])?;
                if matches!(ancestor.kind, crate::model::LogicalType::Array(_)) {
                    return None;
                }
            }
        }
    } else if term.quantifier != Quantifier::Any {
        return None;
    }
    Some(&related.hops)
}

fn relation<'a>(
    repo: &'a Repository,
    hop: &BoundRelationHop,
) -> Result<&'a crate::relation::RelationIndex, String> {
    repo.collections
        .get(hop.relation.source.0 as usize)
        .and_then(|collection| collection.relations.get(hop.relation.ordinal as usize))
        .filter(|relation| relation.target_collection == hop.to)
        .ok_or_else(|| "relation semijoin declaration changed".into())
}

fn segment_target_candidates(
    repo: &Repository,
    target_index: usize,
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    expr: &BoundExpr,
    restricted: Option<&RoaringBitmap>,
    work: &mut SemijoinWork,
    execution: Option<&ExecutionContext>,
) -> Result<RoaringBitmap, String> {
    let collection = &repo.collections[target_index];
    let mut may = RoaringBitmap::new();
    for published in segments[target_index].iter() {
        let mut input = RoaringBitmap::new();
        for (slot, record) in published.index.records.iter().enumerate() {
            if published.active.contains(record.slot)
                && restricted.is_none_or(|restricted| restricted.contains(record.slot.0))
            {
                input.insert(slot as u32);
            }
        }
        if input.is_empty() {
            continue;
        }
        let domain = published.active.bitmap();
        let bounds = crate::executor::segment_filter_bounds_with_catalog(
            &published.index,
            &published.active,
            expr,
            collection,
            Some(&repo.catalog),
        )?;
        advance_slots(work, execution, input.len())?;
        for slot in (&bounds.may & domain & &input).iter() {
            let record = published
                .index
                .record(RecordSlot(slot))
                .ok_or("relation target candidate record missing")?;
            may.insert(
                collection
                    .id_lookup
                    .get(&record.id)
                    .ok_or("relation target candidate has no collection route")?
                    .0,
            );
        }
    }
    ensure_slots(may.len(), execution)?;
    work.target_candidates = work
        .target_candidates
        .checked_add(may.len())
        .ok_or("relation target candidate count overflow")?;
    Ok(may)
}

fn segment_fts_target_candidates(
    repo: &Repository,
    target_index: usize,
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    fts: &BoundFts,
    restricted: Option<&RoaringBitmap>,
    work: &mut SemijoinWork,
    execution: Option<&ExecutionContext>,
) -> Result<RoaringBitmap, String> {
    let collection = &repo.collections[target_index];
    let mut may = RoaringBitmap::new();
    for published in segments[target_index].iter() {
        let mut input = published.active.bitmap();
        if let Some(restricted) = restricted {
            let mut mapped = RoaringBitmap::new();
            for (slot, record) in published.index.records.iter().enumerate() {
                if collection
                    .id_lookup
                    .get(&record.id)
                    .is_some_and(|global| restricted.contains(global.0))
                {
                    mapped.insert(slot as u32);
                }
            }
            input &= mapped;
        }
        if input.is_empty() {
            continue;
        }
        advance_slots(work, execution, input.len())?;
        let candidates =
            crate::executor::segment_fts_candidates(&published.index, collection, fts)?
                .unwrap_or_else(|| published.active.bitmap());
        for slot in (&candidates & &input).iter() {
            let record = published
                .index
                .record(RecordSlot(slot))
                .ok_or("relation FTS target candidate record missing")?;
            may.insert(
                collection
                    .id_lookup
                    .get(&record.id)
                    .ok_or("relation FTS target candidate has no collection route")?
                    .0,
            );
        }
    }
    ensure_slots(may.len(), execution)?;
    work.target_candidates = work
        .target_candidates
        .checked_add(may.len())
        .ok_or("relation target candidate count overflow")?;
    Ok(may)
}

fn advance_slots(
    _work: &mut SemijoinWork,
    execution: Option<&ExecutionContext>,
    count: u64,
) -> Result<(), String> {
    ensure_slots(count, execution)
}

fn segment_expr_estimate(
    repo: &Repository,
    target_index: usize,
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    expr: &BoundExpr,
) -> Result<Option<(f64, u64)>, String> {
    if segments[target_index].is_empty() {
        return Ok(None);
    }
    let collection = &repo.collections[target_index];
    let mut candidates = 0u64;
    for published in segments[target_index].iter() {
        let bounds = crate::executor::segment_filter_bounds_with_catalog(
            &published.index,
            &published.active,
            expr,
            collection,
            Some(&repo.catalog),
        )?;
        candidates = candidates.saturating_add(bounds.may.len());
    }
    Ok(Some((candidates as f64 * 0.02, candidates)))
}

fn segment_fts_estimate(
    repo: &Repository,
    target_index: usize,
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    fts: &BoundFts,
) -> Result<Option<(u64, f64)>, String> {
    if segments[target_index].is_empty() {
        return Ok(None);
    }
    let collection = &repo.collections[target_index];
    let mut candidates = 0u64;
    for published in segments[target_index].iter() {
        let may = crate::executor::segment_fts_candidates(&published.index, collection, fts)?
            .unwrap_or_else(|| published.active.bitmap().clone());
        candidates = candidates.saturating_add((&may & published.active.bitmap()).len());
    }
    Ok(Some((candidates, candidates as f64 * 0.02)))
}

fn estimate(
    repo: &Repository,
    term: &BoundTerm,
    input_len: u64,
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
) -> Result<(Strategy, AccessPathEstimate, Vec<AccessPathEstimate>), String> {
    let related = term.related.as_ref().ok_or("missing relation term")?;
    let terminal_index = related.hops.last().ok_or("missing hop")?.to.0 as usize;
    let target = repo
        .collections
        .get(terminal_index)
        .ok_or("missing target collection")?;
    let terminal = BoundExpr::Term((*related.terminal).clone());
    let (lookup, estimated_targets) =
        if let Some(estimate) = segment_expr_estimate(repo, terminal_index, segments, &terminal)? {
            estimate
        } else {
            estimate_expr(target, &terminal, target.logical_records.len())?
        };
    let source_count = repo.collections[related.hops[0].from.0 as usize]
        .logical_records
        .len()
        .max(1) as f64;
    let average_fanout = related.hops.iter().try_fold(1.0_f64, |fanout, hop| {
        let index = relation(repo, hop)?;
        let source = &repo.collections[hop.from.0 as usize];
        Ok::<_, String>(
            (fanout * index.edge_count() as f64 / source.logical_records.len().max(1) as f64)
                .max(1.0),
        )
    })?;
    let target_count = target.logical_records.len().max(1) as f64;
    let reachable = (input_len as f64 * average_fanout).min(target_count);
    let target_unit = 30.0 + target.stats.mean_record_bytes() / 128.0;
    let reverse_degree = source_count * average_fanout / target_count;
    let target_work = lookup
        + estimated_targets as f64 * (target_unit + reverse_degree * 0.25)
        + related.hops.len() as f64 * estimated_targets as f64 * 0.5;
    let source_work = input_len as f64 * average_fanout
        + reachable * target_unit
        + reachable * reverse_degree * 0.25;
    let estimated_output =
        ((input_len as f64 * estimated_targets as f64 / target_count).ceil() as u64).min(input_len);
    let make = |kind, work| AccessPathEstimate {
        kind,
        exact: true,
        coverage: input_len.min(u32::MAX as u64) as u32,
        estimated_output,
        estimated_work: work,
        estimated_files: (estimated_targets as f64 / target.stats.mean_records_per_file().max(1.0))
            .min(target.stats.file_count as f64),
        estimated_bytes: estimated_targets as f64 * target.stats.mean_record_bytes(),
        posting: None,
    };
    let target_first = make(AccessPathKind::RelationTargetFirst, target_work);
    let source_restricted = make(AccessPathKind::RelationSourceRestricted, source_work);
    if target_work <= source_work
        && input_len > target_count as u64
        && reachable > estimated_targets as f64
    {
        Ok((Strategy::TargetFirst, target_first, vec![source_restricted]))
    } else {
        Ok((
            Strategy::SourceRestricted,
            source_restricted,
            vec![target_first],
        ))
    }
}

pub(crate) fn planning_estimate(
    repo: &Repository,
    term: &BoundTerm,
    input_len: u64,
) -> Result<(Strategy, AccessPathEstimate, Vec<AccessPathEstimate>), String> {
    if eligible(repo, term).is_none() {
        return Err("relation term is not semijoin eligible".into());
    }
    estimate(repo, term, input_len, &repo.segments)
}

pub(crate) fn planning_fts_strategies(
    repo: &Repository,
    fts: &BoundFts,
    input_len: u64,
) -> Result<Option<Vec<Strategy>>, String> {
    if fts.fields.is_empty()
        || fts.fields.iter().any(|field| {
            field.related.as_ref().is_none_or(|hops| {
                hops.is_empty()
                    || hops.iter().any(|hop| {
                        hop.direction != HopDirection::Forward
                            || hop.selectors.iter().any(Option::is_some)
                    })
            })
        })
    {
        return Ok(None);
    }
    let mut strategies = Vec::with_capacity(fts.fields.len());
    for field in &fts.fields {
        let hops = field.related.as_ref().ok_or("relation FTS hop missing")?;
        let target_index = hops.last().ok_or("relation FTS hop missing")?.to.0 as usize;
        let target = &repo.collections[target_index];
        let mut local = fts.clone();
        let mut local_field = field.clone();
        local_field.related = None;
        local.fields = vec![local_field];
        let (estimated_targets, lookup) =
            segment_fts_estimate(repo, target_index, &repo.segments, &local)?.unwrap_or_else(
                || {
                    estimate_fts_candidates(target, &local)
                        .unwrap_or((target.logical_records.len() as u64, 0.0))
                },
            );
        let target_unit = 30.0 + target.stats.mean_record_bytes() / 128.0;
        let fanout = hops.iter().try_fold(1.0_f64, |fanout, hop| {
            let index = relation(repo, hop)?;
            let source = &repo.collections[hop.from.0 as usize];
            Ok::<_, String>(
                (fanout * index.edge_count() as f64 / source.logical_records.len().max(1) as f64)
                    .max(1.0),
            )
        })?;
        let reachable = (input_len as f64 * fanout).min(target.logical_records.len() as f64);
        let reverse_degree = input_len as f64 * fanout / target.logical_records.len().max(1) as f64;
        let source_work =
            input_len as f64 * fanout + reachable * (target_unit + reverse_degree * 0.25);
        let target_work = lookup + estimated_targets as f64 * (target_unit + reverse_degree * 0.25);
        let _ = (target_work, source_work, reachable);
        strategies.push(
            if input_len.saturating_mul(4) >= target.logical_records.len() {
                Strategy::TargetFirst
            } else {
                Strategy::SourceRestricted
            },
        );
    }
    Ok(Some(strategies))
}

fn reachable_targets(
    repo: &Repository,
    hops: &[BoundRelationHop],
    input: &RoaringBitmap,
    work: &mut SemijoinWork,
    execution: Option<&ExecutionContext>,
) -> Result<RoaringBitmap, String> {
    if let Some(execution) = execution {
        execution.check_slots(input.len())?;
    }
    let mut current = input.clone();
    for hop in hops {
        let index = relation(repo, hop)?;
        let source_active = execution
            .and_then(ExecutionContext::active_records)
            .map(|domains| &domains[hop.from.0 as usize]);
        let target_active = execution
            .and_then(ExecutionContext::active_records)
            .map(|domains| &domains[hop.to.0 as usize]);
        let target_collection = &repo.collections[hop.to.0 as usize];
        let mut next = RoaringBitmap::new();
        for slot in current.iter() {
            if source_active.is_some_and(|active| !active.contains(RecordSlot(slot))) {
                continue;
            }
            if index.unavailable().contains(slot) {
                continue;
            }
            for edge in index
                .edges(RecordSlot(slot))
                .ok_or("relation source slot missing")?
            {
                visit_edge(work, execution)?;
                if let Some(active) = target_active {
                    if let Some(target) = target_collection.id_lookup.get(edge.target_id())
                        && active.contains(*target)
                    {
                        next.insert(target.0);
                    }
                } else if let RelationTarget::Resolved(target) = edge.target {
                    next.insert(target.0);
                }
            }
        }
        ensure_slots(next.len(), execution)?;
        current = next;
    }
    Ok(current)
}

fn verified_targets(
    repo: &Repository,
    target_index: usize,
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    terminal: &BoundTerm,
    restricted: Option<&RoaringBitmap>,
    work: &mut SemijoinWork,
    execution: Option<&ExecutionContext>,
) -> Result<RoaringBitmap, String> {
    let collection = &repo.collections[target_index];
    let active_domain = execution
        .and_then(ExecutionContext::active_records)
        .map(|domains| &collection.logical_records & &domains[target_index].bitmap());
    let domain = active_domain
        .as_ref()
        .unwrap_or(&collection.logical_records);
    if restricted.is_none() {
        ensure_slots(domain.len(), execution)?;
    }
    let input = restricted
        .map(|set| set & domain)
        .unwrap_or_else(|| domain.clone());
    ensure_slots(input.len(), execution)?;
    let expr = BoundExpr::Term(terminal.clone());
    let candidates = segment_target_candidates(
        repo,
        target_index,
        segments,
        &expr,
        Some(&input),
        work,
        execution,
    )?;
    let matched = verify_target_records(
        repo,
        target_index,
        &candidates,
        if terminal.rhs.is_some() {
            None
        } else {
            terminal.path.segments.first().map(String::as_str)
        },
        false,
        work,
        execution,
        &mut |ctx, _| eval_term(terminal, ctx),
    )?;
    work.target_matches += matched.len();
    Ok(matched)
}

fn verify_target_records(
    repo: &Repository,
    target_index: usize,
    candidates: &RoaringBitmap,
    first_field: Option<&str>,
    collect_text: bool,
    work: &mut SemijoinWork,
    execution: Option<&ExecutionContext>,
    evaluate: &mut impl FnMut(&RecordContext<'_>, &[TextSegment]) -> Result<bool, String>,
) -> Result<RoaringBitmap, String> {
    if let Some(execution) = execution {
        execution.check_slots(candidates.len())?;
    }
    let collection = &repo.collections[target_index];
    let mut grouped = BTreeMap::<FileId, Vec<(u32, crate::inventory::ByteRange)>>::new();
    for slot in candidates.iter() {
        let record = collection
            .generation
            .records
            .get(slot as usize)
            .ok_or("relation target candidate slot missing")?;
        if record.class != RecordClass::RawFallback {
            grouped
                .entry(record.file)
                .or_default()
                .push((slot, record.range));
        }
    }
    let mut access = SourceAccessSession::new(&repo.files);
    if let Some(execution) = execution {
        access.configure_target_execution(execution);
    }
    let mut matched = RoaringBitmap::new();
    let mut projection = ScanProjection::full(collect_text);
    if let Some(first) = first_field {
        projection.top_level = Some(HashSet::from([first.to_owned()]));
    }
    for (file_id, ranges) in grouped {
        let mut start = 0;
        while start < ranges.len() {
            if let Some(execution) = execution {
                execution.check_string()?;
            }
            let first_bytes = ranges[start].1.len();
            if let Some(execution) = execution
                && first_bytes > execution.limits.target_record_bytes
            {
                return Err(execution.fail(
                    ExecutionErrorKind::Resource,
                    "relation target record exceeds source read allowance",
                ));
            }
            if first_bytes > MAX_TARGET_RECORD_BYTES {
                return Err(format!(
                    "relation target record exceeds {MAX_TARGET_RECORD_BYTES} bytes"
                ));
            }
            let mut end = start + 1;
            let mut batch_bytes = first_bytes;
            while end < ranges.len() && end - start < 128 {
                let next_bytes = ranges[end].1.len();
                if batch_bytes.saturating_add(next_bytes) > TARGET_BATCH_BYTES {
                    break;
                }
                if let Some(execution) = execution
                    && next_bytes > execution.limits.target_record_bytes
                {
                    return Err(execution.fail(
                        ExecutionErrorKind::Resource,
                        "relation target record exceeds source read allowance",
                    ));
                }
                if next_bytes > MAX_TARGET_RECORD_BYTES {
                    return Err(format!(
                        "relation target record exceeds {MAX_TARGET_RECORD_BYTES} bytes"
                    ));
                }
                batch_bytes += next_bytes;
                end += 1;
            }
            let chunk = &ranges[start..end];
            start = end;
            if let Some(execution) = execution {
                execution.charge_reads(chunk.len() as u64)?;
                execution.charge_bytes(batch_bytes)?;
            }
            let batch = access.read(file_id, chunk)?;
            work.target_file_opens += batch.opened;
            work.target_bytes_read += batch.bytes_read;
            for (slot, bytes) in batch.records {
                let record = &collection.generation.records[slot as usize];
                let file = &repo.files.files[file_id.0 as usize];
                let prepared = crate::record_preparation::prepare_query_record_with_execution(
                    record.class,
                    &collection.source,
                    &collection.definition.schema,
                    &bytes,
                    &projection,
                    false,
                    execution,
                )?;
                let ctx = RecordContext {
                    value: &prepared.value,
                    source_bytes: Some(&bytes),
                    record,
                    file,
                    collection_name: &collection.definition.name,
                    repository_root: &repo.files.root,
                };
                work.target_reads += 1;
                if evaluate(&ctx, &prepared.text)? {
                    matched.insert(slot);
                }
            }
        }
    }
    Ok(matched)
}

fn reverse_sources(
    repo: &Repository,
    hops: &[BoundRelationHop],
    terminal: &RoaringBitmap,
    input: &RoaringBitmap,
    work: &mut SemijoinWork,
    execution: Option<&ExecutionContext>,
) -> Result<RoaringBitmap, String> {
    ensure_slots(terminal.len(), execution)?;
    let mut current = terminal.clone();
    for hop in hops.iter().rev() {
        let index = relation(repo, hop)?;
        let target = &repo.collections[hop.to.0 as usize];
        let active = execution.and_then(ExecutionContext::active_records);
        let mut next = RoaringBitmap::new();
        for slot in current.iter() {
            if active.is_some_and(|domains| !domains[hop.to.0 as usize].contains(RecordSlot(slot)))
            {
                continue;
            }
            let record = target
                .generation
                .records
                .get(slot as usize)
                .ok_or("relation reverse target slot missing")?;
            if let Some(sources) = repo.relation_reverse_sources(index, &record.id.0) {
                work.reverse_postings_visited += 1;
                work.reverse_members_visited += sources.len();
                next |= sources;
            }
            ensure_slots(next.len(), execution)?;
        }
        if let Some(domains) = active {
            next &= domains[hop.from.0 as usize].bitmap();
        }
        current = next;
    }
    current &= input;
    Ok(current)
}

#[cfg(test)]
pub(crate) fn candidates_with_strategy(
    repo: &Repository,
    term: &BoundTerm,
    input: &RoaringBitmap,
    strategy: Strategy,
) -> Result<(RoaringBitmap, SemijoinWork), String> {
    candidates_with_strategy_with_execution(repo, &repo.segments, term, input, strategy, None)
}

fn candidates_with_strategy_with_execution(
    repo: &Repository,
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    term: &BoundTerm,
    input: &RoaringBitmap,
    strategy: Strategy,
    execution: Option<&ExecutionContext>,
) -> Result<(RoaringBitmap, SemijoinWork), String> {
    let hops = eligible(repo, term).ok_or("relation term is not semijoin eligible")?;
    ensure_slots(input.len(), execution)?;
    let target_index = hops.last().ok_or("missing relation hop")?.to.0 as usize;
    let mut work = SemijoinWork::default();
    let restricted = if strategy == Strategy::SourceRestricted {
        Some(reachable_targets(repo, hops, input, &mut work, execution)?)
    } else {
        None
    };
    let true_targets = verified_targets(
        repo,
        target_index,
        segments,
        &term
            .related
            .as_ref()
            .ok_or("missing related term")?
            .terminal,
        restricted.as_ref(),
        &mut work,
        execution,
    )?;
    let sources = reverse_sources(repo, hops, &true_targets, input, &mut work, execution)?;
    if work.target_reads > 0 {
        work.verified_targets_reused = sources.len().saturating_sub(work.target_reads);
    }
    Ok((sources, work))
}

/// Resolve selected relation FTS once per distinct target. Mixed local and
/// relation fields retain the established scanner path because their OR
/// evidence cannot be restricted by a relation witness alone.
pub(crate) fn fts_witnesses(
    repo: &Repository,
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    fts: &BoundFts,
    input: &RoaringBitmap,
    execution: Option<&ExecutionContext>,
) -> Result<Option<FtsWitnesses>, String> {
    if fts.fields.is_empty()
        || fts.fields.iter().any(|field| {
            field.related.as_ref().is_none_or(|hops| {
                hops.is_empty()
                    || hops.iter().any(|hop| {
                        hop.direction != HopDirection::Forward
                            || hop.selectors.iter().any(Option::is_some)
                    })
            })
        })
    {
        return Ok(None);
    }
    ensure_slots(input.len(), execution)?;
    let mut evidence = HashMap::<u32, HashSet<String>>::new();
    let mut qualified_evidence = HashMap::<SegmentRecordAddress, HashSet<String>>::new();
    let mut all_work = SemijoinWork::default();
    let mut strategies = Vec::new();
    for field in &fts.fields {
        let hops = field.related.as_ref().ok_or("relation FTS hop missing")?;
        let target_index = hops.last().ok_or("relation FTS hop missing")?.to.0 as usize;
        let target = &repo.collections[target_index];
        let mut local = fts.clone();
        let mut local_field = field.clone();
        local_field.related = None;
        local.fields = vec![local_field];
        let (estimated_targets, lookup) =
            segment_fts_estimate(repo, target_index, segments, &local)?.unwrap_or_else(|| {
                estimate_fts_candidates(target, &local)
                    .unwrap_or((target.logical_records.len() as u64, 0.0))
            });
        let target_unit = 30.0 + target.stats.mean_record_bytes() / 128.0;
        let fanout = hops.iter().try_fold(1.0_f64, |fanout, hop| {
            let index = relation(repo, hop)?;
            let source = &repo.collections[hop.from.0 as usize];
            Ok::<_, String>(
                (fanout * index.edge_count() as f64 / source.logical_records.len().max(1) as f64)
                    .max(1.0),
            )
        })?;
        let reachable = (input.len() as f64 * fanout).min(target.logical_records.len() as f64);
        let reverse_degree =
            input.len() as f64 * fanout / target.logical_records.len().max(1) as f64;
        let source_work =
            input.len() as f64 * fanout + reachable * (target_unit + reverse_degree * 0.25);
        let target_work = lookup + estimated_targets as f64 * (target_unit + reverse_degree * 0.25);
        let _ = (target_work, source_work, reachable);
        let strategy = if input.len().saturating_mul(4) >= target.logical_records.len() {
            Strategy::TargetFirst
        } else {
            Strategy::SourceRestricted
        };
        let mut work = SemijoinWork::default();
        let restricted = if strategy == Strategy::SourceRestricted {
            Some(reachable_targets(repo, hops, input, &mut work, execution)?)
        } else {
            let mut domain = target.logical_records.clone();
            if let Some(active) = execution.and_then(ExecutionContext::active_records) {
                domain &= active[target_index].bitmap();
            }
            ensure_slots(domain.len(), execution)?;
            Some(domain)
        };
        let candidates = segment_fts_target_candidates(
            repo,
            target_index,
            segments,
            &local,
            restricted.as_ref(),
            &mut work,
            execution,
        )?;
        let matched = verify_target_records(
            repo,
            target_index,
            &candidates,
            field.path.segments.first().map(String::as_str),
            true,
            &mut work,
            execution,
            &mut |ctx, segments| eval_bound_fts_membership(&local, segments, ctx),
        )?;
        work.target_matches += matched.len();
        let sources = reverse_sources(repo, hops, &matched, input, &mut work, execution)?;
        if work.target_reads > 0 {
            work.verified_targets_reused = sources.len().saturating_sub(work.target_reads);
        }
        let mut source_witnesses = SegmentRecordSet::default();
        for source_slot in sources.iter() {
            let source_collection = &repo.collections[hops[0].from.0 as usize];
            let record = source_collection
                .generation
                .records
                .get(source_slot as usize)
                .ok_or("relation source witness slot missing")?;
            let mut address = None;
            for published in segments[hops[0].from.0 as usize].iter() {
                if let Some(slot) = published.index.id_lookup.get(&record.id).copied()
                    && published.active.contains(slot)
                {
                    if address.is_some() {
                        return Err("relation source witness has duplicate active address".into());
                    }
                    address = Some(SegmentRecordAddress {
                        collection: source_collection.generation.collection,
                        segment: published.id,
                        slot,
                    });
                }
            }
            let address = address.ok_or("relation source witness has no segment route")?;
            source_witnesses.insert(address);
        }
        ensure_slots(source_witnesses.len(), execution)?;
        for address in source_witnesses.iter() {
            let source_collection = &repo.collections[address.collection.0 as usize];
            let record = segments[address.collection.0 as usize]
                .iter()
                .find(|segment| segment.id == address.segment)
                .and_then(|segment| segment.index.record(address.slot))
                .ok_or("relation witness address no longer resolves")?;
            let flat = source_collection
                .id_lookup
                .get(&record.id)
                .ok_or("relation witness has no flat source route")?;
            evidence
                .entry(flat.0)
                .or_default()
                .insert(field.evidence_path.clone());
            qualified_evidence
                .entry(address)
                .or_default()
                .insert(field.evidence_path.clone());
        }
        ensure_slots(evidence.len() as u64, execution)?;
        all_work.add(work);
        strategies.push(strategy);
    }
    Ok(Some(FtsWitnesses {
        evidence,
        qualified_evidence,
        work: all_work,
        strategies,
    }))
}

/// Restrict only terms whose truth is necessary for the whole expression.
/// Each result is an exact positive witness, but a remaining expression may
/// still require source verification.
pub(crate) fn restrict(
    repo: &Repository,
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    expr: &BoundExpr,
    input: &RoaringBitmap,
    plan: &mut Plan,
    execution: Option<&ExecutionContext>,
) -> Result<(), String> {
    let mut terms = Vec::new();
    positive_terms(expr, &mut terms);
    let mut current = plan.candidates.take().unwrap_or_else(|| input.clone());
    for term in terms {
        if let Some(execution) = execution {
            execution.check_string()?;
        }
        if eligible(repo, term).is_none() || current.is_empty() {
            continue;
        }
        let (strategy, selected, alternatives) = estimate(repo, term, current.len(), segments)?;
        let before = current.clone();
        let actual_input = before.len();
        let (matches, work) = candidates_with_strategy_with_execution(
            repo, segments, term, &current, strategy, execution,
        )?;
        current &= matches;
        plan.semijoin_work.add(work);
        plan.proofs.push(Proof {
            term: term.clone(),
            input: before,
            matches: current.clone(),
        });
        if let Some(choice) = plan
            .choices
            .iter_mut()
            .find(|choice| choice.span == term.span && choice.field == term.query_path.display)
        {
            choice.selected = selected;
            choice.alternatives = alternatives;
            choice.actual_input = actual_input;
            choice.actual_output = Some(current.len());
            choice.reason = format!(
                "exact positive relation witness through {} forward hop(s); {} target candidates, {} target reads, {} reverse postings",
                term.related.as_ref().unwrap().hops.len(),
                work.target_candidates,
                work.target_reads,
                work.reverse_postings_visited,
            );
            choice.residual_verification = false;
        }
    }
    if current.is_empty() || all_terms_proven(expr, &plan.proofs, &current) {
        plan.exact = true;
        plan.proven_hits = current.clone();
    }
    plan.candidates = Some(current);
    Ok(())
}

#[cfg(test)]
#[path = "relation_semijoin.test.rs"]
mod tests;
