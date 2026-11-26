//! Bounded exact positive-witness semijoins for forward relation predicates.

use crate::index::storage::segment::{
    SegmentReader, SegmentSlotFact, read_slot_members, visit_record_headers,
};
use crate::records::RecordIdSlot;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use roaring::RoaringBitmap;

use crate::index::storage::publication::PublishedSegment;
use crate::index::storage::segment::SegmentRecordAddress;
use crate::inventory::RecordClass;
use crate::model::{CollectionId, FileId, RecordSlot};
use crate::query::Quantifier;
use crate::query::binding::fts::BoundFts;
use crate::query::binding::relation::{BoundRelationHop, HopDirection};
use crate::query::semantics::record::RecordContext;
use crate::query::{
    binding::filter::BoundExpr, binding::filter::BoundTerm, binding::filter::same_bound_term,
};
use crate::query_access::contract::{AccessExecution, AccessRequest, InputEstimate};
use crate::query_access::membership::SegmentRecordSet;
use crate::query_access::membership::SlotInput;
use crate::query_access::registry::ProviderCatalog;
use crate::query_access::relations::NativeRelationProvider;
use crate::query_execution::relations::view::{MergedRelationView, positions_match};
use crate::query_execution::restrictions::{
    AccessPathEstimate, AccessPathKind, Plan, Proof, estimate_expr, estimate_fts_candidates,
};
use crate::query_execution::streaming::{ExecutionContext, ExecutionErrorKind};
use crate::query_plan::selection::choose_offer;
use crate::reference::filter::eval_term;
use crate::reference::fts::eval_bound_fts_membership;
use crate::repository::Repository;
use crate::schema::relations::RelationCardinality;
use crate::source::access::SourceAccessSession;
use crate::source::{ScanProjection, TextSegment};

// These are query-local limits. Hitting one is an explicit error, never a
// silently truncated candidate set or count.
const MAX_INTERMEDIATE_SLOTS: u64 = 5_000_000;
const MAX_VISITED_EDGES: u64 = 32_000_000;
const MAX_TARGET_RECORD_BYTES: u64 = 64 * 1024 * 1024;
const TARGET_BATCH_BYTES: u64 = 1024 * 1024;

fn target_batch_bounds<T>(
    ranges: &[T],
    start: usize,
    execution: Option<&ExecutionContext>,
    byte_range: impl Fn(&T) -> crate::inventory::ByteRange,
) -> Result<(usize, u64), String> {
    if start >= ranges.len() {
        return Ok((start, 0));
    }
    let validate = |bytes: u64| {
        if let Some(execution) = execution
            && bytes > execution.limits.target_record_bytes
        {
            return Err(execution.fail(
                ExecutionErrorKind::Resource,
                "relation target record exceeds source read allowance",
            ));
        }
        if bytes > MAX_TARGET_RECORD_BYTES {
            return Err(format!(
                "relation target record exceeds {MAX_TARGET_RECORD_BYTES} bytes"
            ));
        }
        Ok(())
    };
    let first_bytes = byte_range(&ranges[start]).len();
    validate(first_bytes)?;
    let mut end = start + 1;
    let mut batch_bytes = first_bytes;
    while end < ranges.len() && end - start < 128 {
        let next_bytes = byte_range(&ranges[end]).len();
        if batch_bytes.saturating_add(next_bytes) > TARGET_BATCH_BYTES {
            break;
        }
        validate(next_bytes)?;
        batch_bytes += next_bytes;
        end += 1;
    }
    Ok((end, batch_bytes))
}

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
) -> Result<crate::index::storage::relation::DeclaredRelation<'a>, String> {
    let binding = repo.declared_relation(hop.relation)?;
    if binding.target_collection != hop.to {
        return Err("relation semijoin declaration changed".into());
    }
    Ok(binding)
}

fn base_relation_edge_count(repo: &Repository, hop: &BoundRelationHop) -> Result<usize, String> {
    let binding = relation(repo, hop)?;
    Ok(repo.collections[binding.id.source.0 as usize]
        .relation_reader(binding.id.ordinal as usize)?
        .map_or(0, |reader| reader.edge_count()))
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
        let domain = published.active.bitmap();
        let input = restricted.map_or_else(|| domain.clone(), |slots| &domain & slots);
        if input.is_empty() {
            continue;
        }
        let bounds = crate::query_execution::restrictions::segment_filter_bounds_with_catalog(
            published.id,
            published.as_ref(),
            &published.active,
            expr,
            collection,
            Some(&repo.catalog),
        )?;
        advance_slots(work, execution, input.len())?;
        for slot in (bounds.may.as_ref() & domain & &input).iter() {
            let record = published
                .header(RecordSlot(slot))?
                .ok_or("relation target candidate record missing")?;
            may.insert(
                collection
                    .base_slot_for_record(record.id.0.as_ref())?
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
            visit_record_headers(published.as_ref(), |headers| {
                for record in headers {
                    if collection
                        .base_slot_for_record(record.id.0.as_ref())?
                        .is_some_and(|global| restricted.contains(global.0))
                    {
                        mapped.insert(record.slot.0);
                    }
                }
                Ok::<_, String>(())
            })?;
            input &= mapped;
        }
        if input.is_empty() {
            continue;
        }
        advance_slots(work, execution, input.len())?;
        let candidates = crate::query_execution::restrictions::segment_fts_candidates(
            published.id,
            published.as_ref(),
            collection,
            fts,
        )?
        .unwrap_or_else(|| published.active.bitmap());
        for slot in (&candidates & &input).iter() {
            let record = published
                .header(RecordSlot(slot))?
                .ok_or("relation FTS target candidate record missing")?;
            may.insert(
                collection
                    .base_slot_for_record(record.id.0.as_ref())?
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
        let bounds = crate::query_execution::restrictions::segment_filter_bounds_with_catalog(
            published.id,
            published.as_ref(),
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
        let may = crate::query_execution::restrictions::segment_fts_candidates(
            published.id,
            published.as_ref(),
            collection,
            fts,
        )?
        .unwrap_or_else(|| published.active.bitmap().clone());
        candidates = candidates.saturating_add((&may & published.active.bitmap()).len());
    }
    Ok(Some((candidates, candidates as f64 * 0.02)))
}

pub(crate) fn planning_estimate_with_segments(
    repo: &Repository,
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    term: &BoundTerm,
    input_len: u64,
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
            estimate_expr(target, &terminal, target.base_logical_count()?)?
        };
    let source_count = repo.collections[related.hops[0].from.0 as usize]
        .base_logical_count()?
        .max(1) as f64;
    let average_fanout = related.hops.iter().try_fold(1.0_f64, |fanout, hop| {
        let edges = base_relation_edge_count(repo, hop)?;
        let source = &repo.collections[hop.from.0 as usize];
        Ok::<_, String>(
            (fanout * edges as f64 / source.base_logical_count()?.max(1) as f64).max(1.0),
        )
    })?;
    let target_count = target.base_logical_count()?.max(1) as f64;
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
    planning_estimate_with_segments(repo, &repo.segment_groups()?, term, input_len)
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
    let segment_groups = repo.segment_groups()?;
    let mut strategies = Vec::with_capacity(fts.fields.len());
    for field in &fts.fields {
        let hops = field.related.as_ref().ok_or("relation FTS hop missing")?;
        let target_index = hops.last().ok_or("relation FTS hop missing")?.to.0 as usize;
        let target = &repo.collections[target_index];
        let mut local = fts.clone();
        let mut local_field = field.clone();
        local_field.related = None;
        local.fields = vec![local_field];
        let target_count = target.base_logical_count()?;
        let (estimated_targets, lookup) =
            segment_fts_estimate(repo, target_index, &segment_groups, &local)?
                .map_or_else(
                    || estimate_fts_candidates(target, &local),
                    |estimate| Ok(Some(estimate)),
                )?
                .unwrap_or((target_count, 0.0));
        let target_unit = 30.0 + target.stats.mean_record_bytes() / 128.0;
        let fanout = hops.iter().try_fold(1.0_f64, |fanout, hop| {
            let edges = base_relation_edge_count(repo, hop)?;
            let source = &repo.collections[hop.from.0 as usize];
            Ok::<_, String>(
                (fanout * edges as f64 / source.base_logical_count()?.max(1) as f64).max(1.0),
            )
        })?;
        let reachable = (input_len as f64 * fanout).min(target_count as f64);
        let reverse_degree = input_len as f64 * fanout / target_count.max(1) as f64;
        let source_work =
            input_len as f64 * fanout + reachable * (target_unit + reverse_degree * 0.25);
        let target_work = lookup + estimated_targets as f64 * (target_unit + reverse_degree * 0.25);
        let _ = (target_work, source_work, reachable);
        strategies.push(if input_len.saturating_mul(4) >= target_count {
            Strategy::TargetFirst
        } else {
            Strategy::SourceRestricted
        });
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
        if current.is_empty() {
            break;
        }
        let binding = relation(repo, hop)?;
        let index = repo.collections[binding.id.source.0 as usize]
            .relation_reader(binding.id.ordinal as usize)?
            .ok_or("relation semijoin source reader missing")?;
        let source_active = execution
            .and_then(ExecutionContext::active_records)
            .map(|domains| &domains[hop.from.0 as usize]);
        let target_active = execution
            .and_then(ExecutionContext::active_records)
            .map(|domains| &domains[hop.to.0 as usize]);
        let target_collection = &repo.collections[hop.to.0 as usize];
        let mut next = RoaringBitmap::new();
        for slot in current.iter().map(RecordSlot) {
            if source_active.is_some_and(|active| !active.contains(slot)) {
                continue;
            }
            let row = index.forward(slot)?;
            if row.unavailable {
                continue;
            }
            for (id, _) in row.targets {
                visit_edge(work, execution)?;
                if let Some(target) = target_collection.base_slot_for_record(&id)?
                    && target_active.is_none_or(|active| active.contains(target))
                {
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
    let logical =
        collection.base_slot_members(crate::index::storage::segment::SegmentSlotFact::Logical)?;
    let domain = match execution.and_then(ExecutionContext::active_records) {
        Some(domains) => std::borrow::Cow::Owned(
            logical
                .view()
                .intersect_bitmap(&domains[target_index].bitmap()),
        ),
        None => logical.view().bitmap_for_set_algebra(),
    };
    let domain = domain.as_ref();
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
        let record = collection.base_record(crate::model::RecordSlot(slot))?;
        if record.class != RecordClass::RawFallback {
            grouped
                .entry(record.file)
                .or_default()
                .push((slot, record.range));
        }
    }
    let mut access = SourceAccessSession::new(&repo.catalog);
    if let Some(execution) = execution {
        access.configure_target_execution(execution);
    }
    let mut matched = RoaringBitmap::new();
    let mut projection = ScanProjection::full(collect_text);
    if let Some(first) = first_field {
        projection.top_level = Some(HashSet::from([first.to_owned()]));
    }
    access.select_backend(&collection.required_file_reader());
    for (file_id, ranges) in grouped {
        let mut start = 0;
        while start < ranges.len() {
            if let Some(execution) = execution {
                execution.check_string()?;
            }
            let (end, batch_bytes) =
                target_batch_bounds(&ranges, start, execution, |(_, range)| *range)?;
            let chunk = &ranges[start..end];
            start = end;
            if let Some(execution) = execution {
                execution.charge_reads(chunk.len() as u64)?;
                execution.charge_bytes(batch_bytes)?;
            }
            let batch = access.read(file_id, chunk)?;
            work.target_file_opens += batch.opened;
            work.target_bytes_read += batch.bytes_read;
            let file = repo.base_file(file_id)?;
            for (slot, bytes) in batch.records {
                let record = &*collection.base_record(crate::model::RecordSlot(slot))?;
                let prepared = crate::source::preparation::prepare_query_record_with_execution(
                    record.class,
                    &collection.required_file_reader(),
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
                    range: (record).record_range(),
                    file: Some(&file),
                    collection_name: &collection.definition.name,
                    repository_root: Some(repo.required_file_root()),
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

fn qualified_reachable_targets(
    view: &MergedRelationView<'_>,
    hops: &[BoundRelationHop],
    input: &SegmentRecordSet,
    work: &mut SemijoinWork,
    execution: Option<&ExecutionContext>,
) -> Result<SegmentRecordSet, String> {
    ensure_slots(input.len(), execution)?;
    let mut current = input.clone();
    for hop in hops {
        let mut next = SegmentRecordSet::default();
        for address in current.iter() {
            if address.collection != hop.from {
                return Err("relation hop source collection mismatch".into());
            }
            if let Some(execution) = execution {
                execution.check_string()?;
            }
            let view_address = view.view_address(address)?;
            let crate::index::storage::relation::RelationRow {
                targets: references,
                unavailable,
                ..
            } = view.forward_edges(view_address, hop.relation.ordinal as usize)?;
            if unavailable {
                continue;
            }
            for (target_id, positions) in references {
                if !positions_match(&hop.selectors, &positions) {
                    continue;
                }
                visit_edge(work, execution)?;
                if let Some(target) = view.resolve_id(hop.to, &target_id)?
                    && let Some(target) = view.native_address(target)?
                {
                    next.insert(target);
                }
            }
        }
        ensure_slots(next.len(), execution)?;
        if next.is_empty() {
            return Ok(next);
        }
        current = next;
    }
    Ok(current)
}

fn qualified_expression_candidates(
    view: &MergedRelationView<'_>,
    target: CollectionId,
    expr: &BoundExpr,
    restricted: Option<&SegmentRecordSet>,
    work: &mut SemijoinWork,
    execution: Option<&ExecutionContext>,
) -> Result<SegmentRecordSet, String> {
    let repo = view.repository();
    let collection = &repo.collections[target.0 as usize];
    let mut candidates = SegmentRecordSet::default();
    for segment in view.segments(target)? {
        let active = segment.active.bitmap();
        let input = if let Some(restricted) = restricted {
            let Some(local) = restricted.local(target, segment.id) else {
                continue;
            };
            local
        } else {
            &active
        };
        let domain = read_slot_members(segment, SegmentSlotFact::Logical)?
            .view()
            .intersect_bitmap(&active);
        let input = input & &domain;
        if input.is_empty() {
            continue;
        }
        let bounds = crate::query_execution::restrictions::segment_filter_bounds_with_catalog(
            segment.id,
            segment.as_ref(),
            &segment.active,
            expr,
            collection,
            Some(view.catalog()),
        )?;
        advance_slots(work, execution, input.len())?;
        for slot in (bounds.may.as_ref() & &input).iter() {
            candidates.insert(SegmentRecordAddress {
                collection: target,
                segment: segment.id,
                slot: RecordSlot(slot),
            });
        }
    }
    ensure_slots(candidates.len(), execution)?;
    work.target_candidates = work
        .target_candidates
        .checked_add(candidates.len())
        .ok_or("relation target candidate count overflow")?;
    Ok(candidates)
}

fn qualified_fts_candidates(
    view: &MergedRelationView<'_>,
    target: CollectionId,
    fts: &BoundFts,
    restricted: Option<&SegmentRecordSet>,
    work: &mut SemijoinWork,
    execution: Option<&ExecutionContext>,
) -> Result<SegmentRecordSet, String> {
    let repo = view.repository();
    let collection = &repo.collections[target.0 as usize];
    let mut candidates = SegmentRecordSet::default();
    for segment in view.segments(target)? {
        let mut input = segment.active.bitmap();
        if let Some(restricted) = restricted {
            let Some(local) = restricted.local(target, segment.id) else {
                continue;
            };
            input &= local;
        }
        input = read_slot_members(segment, SegmentSlotFact::Logical)?
            .view()
            .intersect_bitmap(&input);
        if input.is_empty() {
            continue;
        }
        advance_slots(work, execution, input.len())?;
        let may = crate::query_execution::restrictions::segment_fts_candidates(
            segment.id,
            segment.as_ref(),
            collection,
            fts,
        )?
        .unwrap_or_else(|| segment.active.bitmap());
        let mut local = SegmentRecordSet::default();
        for slot in (&may & &input).iter() {
            local.insert(SegmentRecordAddress {
                collection: target,
                segment: segment.id,
                slot: RecordSlot(slot),
            });
        }
        candidates.union(&local);
    }
    ensure_slots(candidates.len(), execution)?;
    work.target_candidates = work
        .target_candidates
        .checked_add(candidates.len())
        .ok_or("relation target candidate count overflow")?;
    Ok(candidates)
}

fn verify_qualified_targets(
    view: &MergedRelationView<'_>,
    candidates: &SegmentRecordSet,
    first_field: Option<&str>,
    collect_text: bool,
    work: &mut SemijoinWork,
    execution: Option<&ExecutionContext>,
    evaluate: &mut impl FnMut(&RecordContext<'_>, &[TextSegment]) -> Result<bool, String>,
) -> Result<SegmentRecordSet, String> {
    if let Some(execution) = execution {
        execution.check_slots(candidates.len())?;
    }
    let mut grouped = BTreeMap::<
        (crate::model::CollectionId, FileId),
        Vec<(SegmentRecordAddress, crate::inventory::ByteRange)>,
    >::new();
    for address in candidates.iter() {
        let Some(record) = view.record(view.view_address(address)?)? else {
            continue;
        };
        if record.record.class != RecordClass::RawFallback {
            grouped
                .entry((address.collection, record.record.file))
                .or_default()
                .push((address, record.record.range));
        }
    }
    let mut access = SourceAccessSession::new(view.catalog());
    if let Some(execution) = execution {
        access.configure_target_execution(execution);
    }
    let mut matched = SegmentRecordSet::default();
    let mut projection = ScanProjection::full(collect_text);
    if let Some(first) = first_field {
        projection.top_level = Some(HashSet::from([first.to_owned()]));
    }
    for ((collection, file_id), ranges) in grouped {
        access.select_backend(&view.collection_state(collection)?.required_file_reader());
        let mut start = 0;
        while start < ranges.len() {
            if let Some(execution) = execution {
                execution.check_string()?;
            }
            let (end, batch_bytes) =
                target_batch_bounds(&ranges, start, execution, |(_, range)| *range)?;
            let chunk = ranges[start..end]
                .iter()
                .enumerate()
                .map(|(offset, (_, range))| ((start + offset) as u32, *range))
                .collect::<Vec<_>>();
            start = end;
            if let Some(execution) = execution {
                execution.charge_reads(chunk.len() as u64)?;
                execution.charge_bytes(batch_bytes)?;
            }
            let batch = access.read(file_id, &chunk)?;
            work.target_file_opens += batch.opened;
            work.target_bytes_read += batch.bytes_read;
            for (local_index, bytes) in batch.records {
                let (address, _) = ranges[local_index as usize];
                let Some(resolved) = view.record(view.view_address(address)?)? else {
                    continue;
                };
                let prepared = crate::source::preparation::prepare_query_record_with_execution(
                    resolved.record.class,
                    &resolved.collection.required_file_reader(),
                    &resolved.collection.definition.schema,
                    &bytes,
                    &projection,
                    false,
                    execution,
                )?;
                let ctx = RecordContext {
                    value: &prepared.value,
                    source_bytes: Some(&bytes),
                    record: &resolved.record,
                    range: (resolved.record).record_range(),
                    file: Some(&resolved.file),
                    collection_name: &resolved.collection.definition.name,
                    repository_root: Some(view.repository().required_file_root()),
                };
                work.target_reads += 1;
                if evaluate(&ctx, &prepared.text)? {
                    matched.insert(address);
                }
            }
        }
        let _ = collection;
    }
    Ok(matched)
}

fn qualified_reverse_sources(
    view: &MergedRelationView<'_>,
    hops: &[BoundRelationHop],
    terminals: &SegmentRecordSet,
    input: &SegmentRecordSet,
    work: &mut SemijoinWork,
    execution: Option<&ExecutionContext>,
) -> Result<SegmentRecordSet, String> {
    let mut current = terminals.clone();
    for hop in hops.iter().rev() {
        let mut next = SegmentRecordSet::default();
        for address in current.iter() {
            if address.collection != hop.to {
                return Err("relation reverse-hop collection mismatch".into());
            }
            let Some(record) = view.record(view.view_address(address)?)? else {
                continue;
            };
            let sources = view.backlink_sources(hop.relation, &record.record.id.0)?;
            work.reverse_members_visited = work
                .reverse_members_visited
                .saturating_add(sources.len() as u64);
            for source in sources {
                if let Some(source) = view.native_address(source)? {
                    next.insert(source);
                }
            }
            work.reverse_postings_visited = work.reverse_postings_visited.saturating_add(1);
            ensure_slots(next.len(), execution)?;
        }
        current = next;
    }
    current.intersect(input);
    Ok(current)
}

pub(crate) fn qualified_candidates_with_strategy(
    view: &MergedRelationView<'_>,
    term: &BoundTerm,
    input: &SegmentRecordSet,
    strategy: Strategy,
    execution: Option<&ExecutionContext>,
) -> Result<(SegmentRecordSet, SemijoinWork), String> {
    let repo = view.repository();
    let hops = eligible(repo, term).ok_or("relation term is not semijoin eligible")?;
    ensure_slots(input.len(), execution)?;
    let target = hops.last().ok_or("missing relation hop")?.to;
    let mut work = SemijoinWork::default();
    let restricted = (strategy == Strategy::SourceRestricted)
        .then(|| qualified_reachable_targets(view, hops, input, &mut work, execution))
        .transpose()?;
    let terminal = &term
        .related
        .as_ref()
        .ok_or("missing related term")?
        .terminal;
    let candidates = qualified_expression_candidates(
        view,
        target,
        &BoundExpr::Term((**terminal).clone()),
        restricted.as_ref(),
        &mut work,
        execution,
    )?;
    let matched = verify_qualified_targets(
        view,
        &candidates,
        if terminal.rhs.is_some() {
            None
        } else {
            terminal.path.segments.first().map(String::as_str)
        },
        false,
        &mut work,
        execution,
        &mut |ctx, _| eval_term(terminal, ctx),
    )?;
    work.target_matches = work.target_matches.saturating_add(matched.len());
    let sources = qualified_reverse_sources(view, hops, &matched, input, &mut work, execution)?;
    Ok((sources, work))
}

pub(crate) fn qualified_fts_witnesses(
    view: &MergedRelationView<'_>,
    fts: &BoundFts,
    input: &SegmentRecordSet,
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
    let mut evidence = HashMap::new();
    let mut all_work = SemijoinWork::default();
    let mut strategies = Vec::new();
    for field in &fts.fields {
        let hops = field.related.as_ref().ok_or("relation FTS hop missing")?;
        let target = hops.last().ok_or("relation FTS hop missing")?.to;
        let target_state = &view.repository().collections[target.0 as usize];
        let mut local = fts.clone();
        let mut local_field = field.clone();
        local_field.related = None;
        local.fields = vec![local_field];
        let strategy = if input.len().saturating_mul(4) >= target_state.base_logical_count()? {
            Strategy::TargetFirst
        } else {
            Strategy::SourceRestricted
        };
        let mut work = SemijoinWork::default();
        let restricted = (strategy == Strategy::SourceRestricted)
            .then(|| qualified_reachable_targets(view, hops, input, &mut work, execution))
            .transpose()?;
        let candidates = qualified_fts_candidates(
            view,
            target,
            &local,
            restricted.as_ref(),
            &mut work,
            execution,
        )?;
        let matched = verify_qualified_targets(
            view,
            &candidates,
            field.path.segments.first().map(String::as_str),
            true,
            &mut work,
            execution,
            &mut |ctx, segments| eval_bound_fts_membership(&local, segments, ctx),
        )?;
        work.target_matches = work.target_matches.saturating_add(matched.len());
        let sources = qualified_reverse_sources(view, hops, &matched, input, &mut work, execution)?;
        for address in sources.iter() {
            evidence
                .entry(address)
                .or_insert_with(HashSet::new)
                .insert(field.evidence_path.clone());
        }
        all_work.add(work);
        strategies.push(strategy);
    }
    ensure_slots(evidence.len() as u64, execution)?;
    Ok(Some(FtsWitnesses {
        evidence: HashMap::new(),
        qualified_evidence: evidence,
        work: all_work,
        strategies,
    }))
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
        let binding = relation(repo, hop)?;
        let target = &repo.collections[hop.to.0 as usize];
        let active = execution.and_then(ExecutionContext::active_records);
        let mut next = RoaringBitmap::new();
        for slot in current.iter() {
            if active.is_some_and(|domains| !domains[hop.to.0 as usize].contains(RecordSlot(slot)))
            {
                continue;
            }
            let record = target.base_record(crate::model::RecordSlot(slot))?;
            if let Some(sources) = repo.base_reverse_sources(binding.id, &record.id.0)? {
                let sources = sources
                    .explicit_view()
                    .ok_or("reverse relation membership missing")?;
                if !sources.is_empty() {
                    work.reverse_postings_visited += 1;
                    work.reverse_members_visited += sources.len() as u64;
                    sources.union_into_bitmap(&mut next);
                }
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

pub(crate) fn candidates_with_strategy_with_execution(
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
        let target_count = target.base_logical_count()?;
        let (estimated_targets, lookup) =
            segment_fts_estimate(repo, target_index, segments, &local)?
                .map_or_else(
                    || estimate_fts_candidates(target, &local),
                    |estimate| Ok(Some(estimate)),
                )?
                .unwrap_or((target_count, 0.0));
        let target_unit = 30.0 + target.stats.mean_record_bytes() / 128.0;
        let fanout = hops.iter().try_fold(1.0_f64, |fanout, hop| {
            let edges = base_relation_edge_count(repo, hop)?;
            let source = &repo.collections[hop.from.0 as usize];
            Ok::<_, String>(
                (fanout * edges as f64 / source.base_logical_count()?.max(1) as f64).max(1.0),
            )
        })?;
        let reachable = (input.len() as f64 * fanout).min(target_count as f64);
        let reverse_degree = input.len() as f64 * fanout / target_count.max(1) as f64;
        let source_work =
            input.len() as f64 * fanout + reachable * (target_unit + reverse_degree * 0.25);
        let target_work = lookup + estimated_targets as f64 * (target_unit + reverse_degree * 0.25);
        let _ = (target_work, source_work, reachable);
        let strategy = if input.len().saturating_mul(4) >= target_count {
            Strategy::TargetFirst
        } else {
            Strategy::SourceRestricted
        };
        let mut work = SemijoinWork::default();
        let restricted = if strategy == Strategy::SourceRestricted {
            Some(reachable_targets(repo, hops, input, &mut work, execution)?)
        } else {
            let mut domain = target
                .base_slot_members(crate::index::storage::segment::SegmentSlotFact::Logical)?
                .view()
                .bitmap_for_set_algebra()
                .into_owned();
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
            let record = source_collection.base_record(crate::model::RecordSlot(source_slot))?;
            let mut address = None;
            for published in segments[hops[0].from.0 as usize].iter() {
                if let Some(slot) = published.slot_for_record(record.id.0.as_ref())?
                    && published.active.contains(slot)
                {
                    if address.is_some() {
                        return Err("relation source witness has duplicate active address".into());
                    }
                    address = Some(SegmentRecordAddress {
                        collection: source_collection.collection_id(),
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
            let segment = segments[address.collection.0 as usize]
                .iter()
                .find(|segment| segment.id == address.segment)
                .ok_or("relation witness address no longer resolves")?;
            let record = segment
                .header(address.slot)?
                .ok_or("relation witness address no longer resolves")?;
            let flat = source_collection
                .base_slot_for_record(record.id.0.as_ref())?
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
    term_id: &mut dyn FnMut(&BoundTerm) -> crate::query_predicate::TermId,
    execution: Option<&ExecutionContext>,
) -> Result<(), String> {
    let mut terms = Vec::new();
    positive_terms(expr, &mut terms);
    let Some(source_collection) = terms.iter().find_map(|term| {
        term.related
            .as_ref()
            .and_then(|related| related.hops.first())
            .map(|hop| hop.from)
    }) else {
        return Ok(());
    };
    let provider = NativeRelationProvider::new(repo, segments, source_collection)?;
    let catalog =
        ProviderCatalog::new(
            [&provider as &dyn crate::query_access::contract::IndexAccessProvider],
        );
    let collection = &repo.collections[source_collection.0 as usize];
    let mut current = plan.candidates.take().unwrap_or_else(|| input.clone());
    let mut offers = HashMap::<crate::query_predicate::TermId, _>::new();
    for term in terms {
        if let Some(execution) = execution {
            execution.check_string()?;
        }
        if eligible(repo, term).is_none() || current.is_empty() {
            continue;
        }
        let id = term_id(term);
        if !offers.contains_key(&id) {
            let proposed = catalog
                .propose(
                    id,
                    AccessRequest::Structured(term),
                    InputEstimate {
                        domain: provider.domain(),
                        domain_len: collection.stats.record_count as u64,
                        input_len: current.len() as u64,
                        mean_record_bytes: collection.stats.mean_record_bytes(),
                        mean_records_per_file: collection.stats.mean_records_per_file(),
                        file_count: collection.stats.file_count as u64,
                    },
                )
                .map_err(|error| error.to_string())?;
            offers.insert(id, proposed);
        }
        let selected_offer = choose_offer(
            id,
            offers.get(&id).expect("relation offers disappeared"),
            current.len() as u64,
        )
        .map_err(|error| error.to_string())?
        .cloned()
        .ok_or("relation provider returned no executable offer")?;
        let before = current.clone();
        let actual_input = before.len();
        let restriction = catalog
            .apply(
                id,
                AccessRequest::Structured(term),
                selected_offer.id(),
                SlotInput::unrestricted(provider.domain(), &current),
                &AccessExecution { context: execution },
            )
            .map_err(|error| error.to_string())?;
        current = restriction.possible.into_owned_members();
        let work = provider.take_work();
        plan.semijoin_work.add(work);
        plan.proofs.push(Proof {
            term: term.clone(),
            input: before,
            matches: current.clone(),
        });
        let selected = provider
            .path_for(selected_offer.id())
            .ok_or("relation provider lost its selected path")?;
        let alternatives = provider.alternatives_for(selected_offer.id(), id);
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
mod tests;
