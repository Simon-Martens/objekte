//! Exact relation truth, membership and evidence evaluation.

use crate::records::RecordIdSlot;
use std::sync::Arc;

use super::view::{MergedRecordSlot, MergedRelationView, positions_match, read_reference_value};
use crate::index::relation::RelationTarget;
use crate::inventory::RecordClass;
use crate::model::{PathRoot, RecordSlot};
use crate::query::Quantifier;
use crate::query::binding::filter::{BoundExpr, BoundTerm};
use crate::query::binding::fts::{BoundFts, BoundFtsField};
use crate::query::binding::relation::{BoundRelationHop, HopDirection};
use crate::query::semantics::boolean;
use crate::query::semantics::comparison::eval_cross_record_field_term;
use crate::query::semantics::filter::emit_bound_filter_evidence;
use crate::query::semantics::record::RecordContext;
use crate::query_execution::relations::target_cache::TargetValueCache;
use crate::reference::filter::eval_term;
use crate::reference::{
    eval_bound_fts, eval_bound_fts_field, eval_bound_fts_membership,
    fts::eval_borrowed_value_fts_field,
};
use crate::repository::Repository;
use crate::source::access::read_record_range_for_search;
use crate::source::{ScanProjection, TextSegment};

use super::work::{MAX_QUERY_RELATION_BYTES, RelationWork};

fn relation_target_uses_value_text(collection: &crate::repository::state::CollectionState) -> bool {
    collection.required_file_reader().scalar_verification()
        == Some(crate::source::verification::ScalarVerification::ResolvedStrings)
}

fn relation_source_uses_value_text(repo: &Repository, source: &RecordContext<'_>) -> bool {
    source.record.record_class() == RecordClass::Logical
        && repo
            .collections
            .iter()
            .find(|collection| collection.definition.name == source.collection_name)
            .is_some_and(relation_target_uses_value_text)
}

/// Independent logical ID occurrences and presence/null state.
pub(crate) struct LogicalReferences {
    pub(crate) present: bool,
    pub(crate) null: bool,
    pub(crate) values: Vec<(String, Vec<u32>)>,
}

/// An intentionally separate, iterative oracle for declared ID occurrences.
/// It walks logical values directly and never consults build-time edge state.
pub(super) fn logical_references(
    value: &crate::model::Value,
    path: &[String],
) -> Result<LogicalReferences, String> {
    use crate::model::Value;

    let mut found = false;
    let mut null = false;
    let mut references = Vec::new();
    let mut pending = vec![(value, 0usize, Vec::<u32>::new())];
    while let Some((value, offset, positions)) = pending.pop() {
        match value {
            Value::Array(array) => {
                if array.values.is_empty() {
                    found = true;
                }
                for (index, item) in array.values.iter().enumerate().rev() {
                    let mut child_positions = positions.clone();
                    child_positions
                        .push(u32::try_from(index).map_err(|_| "relation array index overflow")?);
                    pending.push((item, offset, child_positions));
                }
            }
            Value::Object(fields) if offset < path.len() => {
                if let Some(next) = fields.get(&path[offset]) {
                    pending.push((next, offset + 1, positions));
                }
            }
            Value::Id(id) if offset == path.len() => {
                found = true;
                references.push((id.clone(), positions));
            }
            Value::Null if offset == path.len() => {
                found = true;
                null = true;
            }
            _ if offset == path.len() => return Err("relation field has non-ID value".into()),
            _ => {}
        }
    }
    Ok(LogicalReferences {
        present: found,
        null,
        values: references,
    })
}

#[cfg(test)]
static BORROWED_RELATION_TARGETS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
pub(crate) fn borrowed_relation_targets() -> usize {
    BORROWED_RELATION_TARGETS.load(std::sync::atomic::Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EvaluationMode {
    Indexed,
    Reference,
}

/// Immutable traversal choices shared by every hop of one predicate.
#[derive(Clone, Copy)]
struct TraverseConfig<'a> {
    repo: &'a Repository,
    hops: &'a [BoundRelationHop],
    quantifier: Quantifier,
    collect_text: bool,
    mode: EvaluationMode,
    target_values: Option<&'a TargetValueCache>,
}

/// Bind a Boolean expression with collection-specific relation declarations.
pub(crate) fn eval_filter_truth(
    repo: &Repository,
    expr: &BoundExpr,
    ctx: &RecordContext<'_>,
    evidence: &mut std::collections::HashSet<String>,
    mode: EvaluationMode,
    work: &mut RelationWork,
) -> Result<Option<bool>, String> {
    boolean::eval_with_evidence(
        expr,
        evidence,
        &mut |node, fields| match node {
            BoundExpr::Term(term) => {
                let result = if term.related.is_some() {
                    eval_related_term_mode(repo, term, ctx, mode, work, None)?
                } else {
                    Some(eval_term(term, ctx)?)
                };
                emit_bound_filter_evidence(term, result, fields);
                Ok(result)
            }
            BoundExpr::And(_) | BoundExpr::Or(_) | BoundExpr::Not(_) => {
                unreachable!("Boolean leaf evaluator received a branch node")
            }
        },
        &mut |_| true,
        false,
    )
}

/// Three-valued membership stops at decisive branches without evidence sets.
pub(crate) fn eval_filter_truth_membership(
    repo: &Repository,
    expr: &BoundExpr,
    ctx: &RecordContext<'_>,
    mode: EvaluationMode,
    work: &mut RelationWork,
) -> Result<Option<bool>, String> {
    boolean::eval_membership(expr, &mut |node| match node {
        BoundExpr::Term(term) => {
            if term.related.is_some() {
                eval_related_term_mode(repo, term, ctx, mode, work, None)
            } else {
                eval_term(term, ctx).map(Some)
            }
        }
        BoundExpr::And(_) | BoundExpr::Or(_) | BoundExpr::Not(_) => {
            unreachable!("Boolean leaf evaluator received a branch node")
        }
    })
}

/// Evaluate selected local and relation FTS fields with source identity intact.
pub(crate) fn eval_fts_with_relations(
    repo: &Repository,
    fts: &BoundFts,
    segments: &[TextSegment],
    ctx: &RecordContext<'_>,
    evidence: &mut std::collections::HashSet<String>,
    mode: EvaluationMode,
    work: &mut RelationWork,
) -> Result<bool, String> {
    eval_fts_with_relations_inner(repo, fts, segments, ctx, Some(evidence), mode, work)
}

/// Membership-only route avoids constructing evidence and stops at one match.
pub(crate) fn eval_fts_with_relations_membership(
    repo: &Repository,
    fts: &BoundFts,
    segments: &[TextSegment],
    ctx: &RecordContext<'_>,
    mode: EvaluationMode,
    work: &mut RelationWork,
) -> Result<bool, String> {
    eval_fts_with_relations_inner(repo, fts, segments, ctx, None, mode, work)
}

fn eval_fts_with_relations_inner(
    repo: &Repository,
    fts: &BoundFts,
    segments: &[TextSegment],
    ctx: &RecordContext<'_>,
    evidence: Option<&mut std::collections::HashSet<String>>,
    mode: EvaluationMode,
    work: &mut RelationWork,
) -> Result<bool, String> {
    if fts.fields.iter().all(|field| field.related.is_none()) {
        return if let Some(evidence) = evidence {
            eval_bound_fts(fts, segments, ctx, evidence)
        } else {
            eval_bound_fts_membership(fts, segments, ctx)
        };
    }
    let borrowed_local_text = fts.fields.iter().any(|field| field.related.is_none())
        && relation_source_uses_value_text(repo, ctx);
    eval_fts_fields(
        fts,
        segments,
        ctx,
        borrowed_local_text,
        evidence,
        &mut |field| {
            let hops = field
                .related
                .as_ref()
                .expect("related FTS visitor received a local field");
            let target_collection =
                &repo.collections[hops.last().ok_or("empty FTS relation path")?.to.0 as usize];
            let borrowed_text = relation_target_uses_value_text(target_collection);
            let source_index = repo
                .collections
                .iter()
                .position(|collection| collection.definition.name == ctx.collection_name)
                .ok_or("relation FTS source collection is unknown")?;
            let mut truth = Some(false);
            let config = TraverseConfig {
                repo,
                hops,
                quantifier: Quantifier::Any,
                collect_text: !borrowed_text,
                mode,
                target_values: None,
            };
            visit_hop(
                &config,
                0,
                source_index,
                ctx.record.record_slot(),
                &mut truth,
                work,
                &[],
                &mut |target, target_segments, _| {
                    eval_related_fts_terminal(field, fts, borrowed_text, target, target_segments)
                },
            )?;
            Ok(truth == Some(true))
        },
    )
}

fn eval_fts_fields(
    fts: &BoundFts,
    segments: &[TextSegment],
    source: &RecordContext<'_>,
    borrowed_local_text: bool,
    mut evidence: Option<&mut std::collections::HashSet<String>>,
    visit_related: &mut impl FnMut(&BoundFtsField) -> Result<bool, String>,
) -> Result<bool, String> {
    let mut matched = false;
    for field in &fts.fields {
        let field_matched = if field.related.is_some() {
            visit_related(field)?
        } else if borrowed_local_text
            && matches!(field.path.root, PathRoot::Data)
            && field.query_path.is_none()
        {
            eval_borrowed_value_fts_field(
                field,
                fts.case_sensitivity,
                source,
                evidence.as_deref_mut(),
            )?
        } else {
            eval_bound_fts_field(
                field,
                fts.case_sensitivity,
                segments,
                source,
                evidence.as_deref_mut(),
            )?
        };
        if field_matched {
            matched = true;
            if let Some(evidence) = evidence.as_deref_mut() {
                if field.related.is_some() {
                    evidence.insert(field.evidence_path.clone());
                }
            } else {
                return Ok(true);
            }
        }
    }
    Ok(matched)
}

fn eval_related_fts_terminal(
    field: &BoundFtsField,
    fts: &BoundFts,
    borrowed_text: bool,
    target: &RecordContext<'_>,
    target_segments: &[TextSegment],
) -> Result<bool, String> {
    if borrowed_text {
        #[cfg(test)]
        BORROWED_RELATION_TARGETS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        debug_assert!(target_segments.is_empty());
        eval_borrowed_value_fts_field(field, fts.case_sensitivity, target, None)
    } else {
        eval_bound_fts_field(field, fts.case_sensitivity, target_segments, target, None)
    }
}

/// Exact three-valued result for a relation term. Unknown means a dangling or
/// logically unavailable target, so Boolean negation must keep it unknown.
pub(crate) fn eval_related_term(
    repo: &Repository,
    term: &BoundTerm,
    source: &RecordContext<'_>,
    work: &mut RelationWork,
    target_values: Option<&TargetValueCache>,
) -> Result<Option<bool>, String> {
    eval_related_term_mode(
        repo,
        term,
        source,
        EvaluationMode::Indexed,
        work,
        target_values,
    )
}

fn eval_related_term_mode(
    repo: &Repository,
    term: &BoundTerm,
    source: &RecordContext<'_>,
    mode: EvaluationMode,
    work: &mut RelationWork,
    target_values: Option<&TargetValueCache>,
) -> Result<Option<bool>, String> {
    let related = term.related.as_ref().ok_or("relation term is not bound")?;
    let start = repo
        .collections
        .iter()
        .position(|collection| collection.definition.name == source.collection_name)
        .ok_or("relation source collection is unknown")?;
    let mut truth = if term.quantifier == Quantifier::Any {
        Some(false)
    } else {
        Some(true)
    };
    let config = TraverseConfig {
        repo,
        hops: &related.hops,
        quantifier: term.quantifier,
        collect_text: false,
        mode,
        target_values: target_values.filter(|_| {
            related.source_operand.is_some() && related.terminal.path.root == PathRoot::Data
        }),
    };
    visit_hop(
        &config,
        0,
        start,
        source.record.record_slot(),
        &mut truth,
        work,
        &[],
        &mut |target, _, source_positions| {
            if let Some(source_operand) = &related.source_operand {
                eval_cross_record_field_term(
                    &related.terminal,
                    source_operand,
                    target,
                    source,
                    related.target_is_lhs,
                    source_positions,
                    &related.hops[0].selectors,
                )
            } else {
                eval_term(&related.terminal, target)
            }
        },
    )?;
    Ok(truth)
}

fn merge_truth(current: &mut Option<bool>, next: Option<bool>, quantifier: Quantifier) {
    *current = if quantifier == Quantifier::Any {
        match (*current, next) {
            (Some(true), _) | (_, Some(true)) => Some(true),
            (None, _) | (_, None) => None,
            _ => Some(false),
        }
    } else {
        match (*current, next) {
            (Some(false), _) | (_, Some(false)) => Some(false),
            (None, _) | (_, None) => None,
            _ => Some(true),
        }
    };
}

fn decisive(truth: Option<bool>, quantifier: Quantifier) -> bool {
    truth == Some(quantifier == Quantifier::Any)
}

fn visit_merged_hop(
    view: &MergedRelationView<'_>,
    hops: &[BoundRelationHop],
    depth: usize,
    address: MergedRecordSlot,
    quantifier: Quantifier,
    collect_text: bool,
    cache_values: bool,
    truth: &mut Option<bool>,
    work: &mut RelationWork,
    reference: bool,
    source_positions: &[u32],
    terminal: &mut impl FnMut(&RecordContext<'_>, &[TextSegment], &[u32]) -> Result<bool, String>,
) -> Result<(), String> {
    if decisive(*truth, quantifier) {
        return Ok(());
    }
    let Some(current) = view.record(address)? else {
        merge_truth(truth, None, quantifier);
        return Ok(());
    };
    if depth == hops.len() {
        if current.record.class == RecordClass::RawFallback {
            merge_truth(truth, None, quantifier);
            return Ok(());
        }
        if current.record.range.len() > MAX_QUERY_RELATION_BYTES {
            return Err(format!(
                "relation target record exceeds {MAX_QUERY_RELATION_BYTES} source bytes"
            ));
        }
        // Distinct targets can load in parallel; repeated visits to one
        // target synchronize through the same shard and decode it once.
        let mut target_cache = view.lock_target_value_cache(address, cache_values);
        if let Some(value) = target_cache
            .as_ref()
            .and_then(|cache| cache.0.get(&address).cloned())
        {
            work.target_cache_hits += 1;
            drop(target_cache);
            let ctx = RecordContext {
                value: &value,
                source_bytes: None,
                record: &current.record,
                range: (current.record).record_range(),
                file: Some(current.file),
                collection_name: &current.collection.definition.name,
                repository_root: Some(view.repository_root()),
            };
            merge_truth(
                truth,
                Some(terminal(&ctx, &[], source_positions)?),
                quantifier,
            );
            return Ok(());
        }
        work.before_read(current.record.range.len(), true)?;
        let path = view.repository_root().join(&current.file.relative_path);
        if let Some(execution) = work.execution() {
            execution.check_string()?;
        }
        let bytes = match read_record_range_for_search(
            &path,
            current.file.revision,
            current.record.range,
            work.execution(),
        ) {
            Ok(bytes) => bytes,
            Err(message) => {
                view.record_source_failure(current.file, current.record.range, message);
                merge_truth(truth, None, quantifier);
                return Ok(());
            }
        };
        work.target_read(bytes.len() as u64)?;
        let extracted = crate::source::preparation::prepare_query_record_with_execution(
            current.record.class,
            &current.collection.required_file_reader(),
            &current.collection.definition.schema,
            &bytes,
            &ScanProjection::full(collect_text),
            false,
            work.execution(),
        )?;
        if cache_values {
            let value = Arc::new(extracted.value);
            if let Some(cache) = target_cache.as_mut()
                && bytes.len() <= 64 * 1024
                && cache.1.saturating_add(bytes.len()) <= 512 * 1024
            {
                cache.1 += bytes.len();
                cache.0.insert(address, value.clone());
            }
            drop(target_cache);
            let ctx = RecordContext {
                value: &value,
                source_bytes: None,
                record: &current.record,
                range: (current.record).record_range(),
                file: Some(current.file),
                collection_name: &current.collection.definition.name,
                repository_root: Some(view.repository_root()),
            };
            merge_truth(
                truth,
                Some(terminal(&ctx, &[], source_positions)?),
                quantifier,
            );
        } else {
            let ctx = RecordContext {
                value: &extracted.value,
                source_bytes: Some(&bytes),
                record: &current.record,
                range: (current.record).record_range(),
                file: Some(current.file),
                collection_name: &current.collection.definition.name,
                repository_root: Some(view.repository_root()),
            };
            merge_truth(
                truth,
                Some(terminal(&ctx, &extracted.text, source_positions)?),
                quantifier,
            );
        }
        return Ok(());
    }
    let hop = &hops[depth];
    if hop.from != address.collection {
        return Err("merged relation hop collection mismatch".into());
    }
    let relation_index = hop.relation.ordinal as usize;
    match hop.direction {
        HopDirection::Forward => {
            if reference {
                let Some(references) =
                    view.reference_forward_edges(address, hop.relation, Some(&mut *work))?
                else {
                    merge_truth(truth, None, quantifier);
                    return Ok(());
                };
                let mut selected = 0usize;
                for (target_id, positions) in references.values {
                    if !positions_match(&hop.selectors, &positions) {
                        continue;
                    }
                    work.edge()?;
                    selected += 1;
                    if let Some(target) = view.reference_resolve_id(hop.to, &target_id)? {
                        visit_merged_hop(
                            view,
                            hops,
                            depth + 1,
                            target,
                            quantifier,
                            collect_text,
                            cache_values,
                            truth,
                            work,
                            true,
                            if depth == 0 {
                                &positions
                            } else {
                                source_positions
                            },
                            terminal,
                        )?;
                    } else {
                        merge_truth(truth, None, quantifier);
                    }
                    if decisive(*truth, quantifier) {
                        break;
                    }
                }
                if selected == 0
                    && (!references.present
                        || references.null
                        || hop.selectors.iter().any(Option::is_some))
                {
                    merge_truth(truth, Some(false), quantifier);
                }
                return Ok(());
            }
            let (references, present, null, _, unavailable) =
                view.forward_edges(address, relation_index)?;
            if unavailable {
                merge_truth(truth, None, quantifier);
                return Ok(());
            }
            let mut selected = 0usize;
            for (target_id, positions) in references {
                if !positions_match(&hop.selectors, positions) {
                    continue;
                }
                work.edge()?;
                selected += 1;
                if let Some(target) = view.resolve_id(hop.to, target_id)? {
                    visit_merged_hop(
                        view,
                        hops,
                        depth + 1,
                        target,
                        quantifier,
                        collect_text,
                        cache_values,
                        truth,
                        work,
                        reference,
                        if depth == 0 {
                            positions
                        } else {
                            source_positions
                        },
                        terminal,
                    )?;
                } else {
                    merge_truth(truth, None, quantifier);
                }
                if decisive(*truth, quantifier) {
                    break;
                }
            }
            if selected == 0 && (!present || null || hop.selectors.iter().any(Option::is_some)) {
                merge_truth(truth, Some(false), quantifier);
            }
        }
        HopDirection::Backlink => {
            let target_id = current.record.id.0.as_ref();
            let sources = if reference {
                view.reference_backlink_sources(hop.relation, target_id, Some(&mut *work))?
            } else {
                view.backlink_sources(hop.relation, target_id)?
            };
            let mut selected = 0usize;
            for (position, source) in sources.into_iter().enumerate() {
                work.backlink_probe()?;
                work.edge()?;
                if hop.selectors.first().is_some_and(|selector| {
                    selector.is_some_and(|index| index as usize != position)
                }) {
                    continue;
                }
                selected += 1;
                visit_merged_hop(
                    view,
                    hops,
                    depth + 1,
                    source,
                    quantifier,
                    collect_text,
                    cache_values,
                    truth,
                    work,
                    reference,
                    source_positions,
                    terminal,
                )?;
                if decisive(*truth, quantifier) {
                    break;
                }
            }
            if selected == 0 && hop.selectors.iter().any(Option::is_some) {
                merge_truth(truth, Some(false), quantifier);
            }
        }
    }
    Ok(())
}

fn eval_related_term_merged(
    view: &MergedRelationView<'_>,
    term: &BoundTerm,
    source_slot: MergedRecordSlot,
    source: &RecordContext<'_>,
    work: &mut RelationWork,
    reference: bool,
) -> Result<Option<bool>, String> {
    let related = term
        .related
        .as_ref()
        .ok_or("merged relation term is not bound")?;
    let mut truth = Some(term.quantifier != Quantifier::Any);
    visit_merged_hop(
        view,
        &related.hops,
        0,
        source_slot,
        term.quantifier,
        false,
        related.source_operand.is_some() && related.terminal.path.root == PathRoot::Data,
        &mut truth,
        work,
        reference,
        &[],
        &mut |target, _, positions| {
            if let Some(source_operand) = &related.source_operand {
                eval_cross_record_field_term(
                    &related.terminal,
                    source_operand,
                    target,
                    source,
                    related.target_is_lhs,
                    positions,
                    &related.hops[0].selectors,
                )
            } else {
                eval_term(&related.terminal, target)
            }
        },
    )?;
    Ok(truth)
}

pub(crate) fn eval_filter_truth_merged(
    view: &MergedRelationView<'_>,
    expr: &BoundExpr,
    source_slot: MergedRecordSlot,
    source: &RecordContext<'_>,
    evidence: &mut std::collections::HashSet<String>,
    work: &mut RelationWork,
    reference: bool,
) -> Result<Option<bool>, String> {
    boolean::eval_with_evidence(
        expr,
        evidence,
        &mut |node, fields| match node {
            BoundExpr::Term(term) => {
                let result = if term.related.is_some() {
                    eval_related_term_merged(view, term, source_slot, source, work, reference)?
                } else {
                    Some(eval_term(term, source)?)
                };
                emit_bound_filter_evidence(term, result, fields);
                Ok(result)
            }
            BoundExpr::And(_) | BoundExpr::Or(_) | BoundExpr::Not(_) => {
                unreachable!("Boolean leaf evaluator received a branch node")
            }
        },
        &mut |_| true,
        false,
    )
}

pub(crate) fn eval_fts_with_relations_merged(
    view: &MergedRelationView<'_>,
    fts: &BoundFts,
    segments: &[TextSegment],
    source_slot: MergedRecordSlot,
    source: &RecordContext<'_>,
    evidence: &mut std::collections::HashSet<String>,
    work: &mut RelationWork,
    reference: bool,
) -> Result<bool, String> {
    if fts.fields.iter().all(|field| field.related.is_none()) {
        return eval_bound_fts(fts, segments, source, evidence);
    }
    let borrowed_local_text = fts.fields.iter().any(|field| field.related.is_none())
        && relation_source_uses_value_text(view.repository(), source);
    eval_fts_fields(
        fts,
        segments,
        source,
        borrowed_local_text,
        Some(evidence),
        &mut |field| {
            let hops = field
                .related
                .as_ref()
                .expect("related FTS visitor received a local field");
            let target_collection = &view.repository().collections
                [hops.last().ok_or("empty FTS relation path")?.to.0 as usize];
            let borrowed_text = relation_target_uses_value_text(target_collection);
            let mut truth = Some(false);
            visit_merged_hop(
                view,
                hops,
                0,
                source_slot,
                Quantifier::Any,
                !borrowed_text,
                false,
                &mut truth,
                work,
                reference,
                &[],
                &mut |target, target_segments, _| {
                    eval_related_fts_terminal(field, fts, borrowed_text, target, target_segments)
                },
            )?;
            Ok(truth == Some(true))
        },
    )
}

fn visit_hop(
    config: &TraverseConfig<'_>,
    depth: usize,
    collection_index: usize,
    source_slot: RecordSlot,
    truth: &mut Option<bool>,
    work: &mut RelationWork,
    source_positions: &[u32],
    terminal: &mut impl FnMut(&RecordContext<'_>, &[TextSegment], &[u32]) -> Result<bool, String>,
) -> Result<(), String> {
    let TraverseConfig {
        repo,
        hops,
        quantifier,
        collect_text,
        mode,
        target_values,
    } = *config;
    if decisive(*truth, quantifier) {
        return Ok(());
    }
    if !work.is_active(
        repo.collections[collection_index].generation.collection,
        source_slot,
    ) {
        merge_truth(truth, None, quantifier);
        return Ok(());
    }
    if depth == hops.len() {
        let collection = &repo.collections[collection_index];
        let record = &collection.generation.records[source_slot.0 as usize];
        if record.class == RecordClass::RawFallback {
            merge_truth(truth, None, quantifier);
            return Ok(());
        }
        if let Some(value) =
            target_values.and_then(|cache| cache.get(collection.generation.collection, source_slot))
        {
            work.target_cache_hits += 1;
            let file = &repo.files.files[record.file.0 as usize];
            let ctx = RecordContext {
                value,
                source_bytes: None,
                record,
                range: (record).record_range(),
                file: Some(file),
                collection_name: &collection.definition.name,
                repository_root: Some(repo.files.required_root()),
            };
            merge_truth(
                truth,
                Some(terminal(&ctx, &[], source_positions)?),
                quantifier,
            );
            return Ok(());
        }
        let file = &repo.files.files[record.file.0 as usize];
        let path = repo
            .files
            .native_path(record.file)
            .ok_or("missing target source path")?;
        if record.range.len() > MAX_QUERY_RELATION_BYTES {
            return Err(format!(
                "relation target record exceeds {MAX_QUERY_RELATION_BYTES} source bytes"
            ));
        }
        work.before_read(record.range.len(), true)?;
        let bytes =
            read_record_range_for_search(&path, record.revision, record.range, work.execution())?;
        work.target_read(bytes.len() as u64)?;
        let extracted = crate::source::preparation::prepare_query_record_with_execution(
            record.class,
            &collection.required_file_reader(),
            &collection.definition.schema,
            &bytes,
            &ScanProjection::full(collect_text),
            false,
            work.execution(),
        )?;
        let ctx = RecordContext {
            value: &extracted.value,
            source_bytes: Some(&bytes),
            record,
            range: (record).record_range(),
            file: Some(file),
            collection_name: &collection.definition.name,
            repository_root: Some(repo.files.required_root()),
        };
        merge_truth(
            truth,
            Some(terminal(&ctx, &extracted.text, source_positions)?),
            quantifier,
        );
        return Ok(());
    }
    let hop = &hops[depth];
    if hop.from.0 as usize != collection_index {
        return Err("relation hop collection mismatch".into());
    }
    let source_collection = &repo.collections[hop.relation.source.0 as usize];
    let relation = source_collection
        .relations
        .get(hop.relation.ordinal as usize)
        .ok_or("relation hop index is missing")?;
    if mode == EvaluationMode::Reference {
        match hop.direction {
            HopDirection::Forward => {
                let Some(value) = read_reference_value(repo, collection_index, source_slot, work)?
                else {
                    merge_truth(truth, None, quantifier);
                    return Ok(());
                };
                let references = logical_references(&value, &relation.declaration.path)?;
                let present = references.present;
                let null = references.null;
                let target_collection = &repo.collections[hop.to.0 as usize];
                let targets = &target_collection.generation.records;
                let mut selected = 0usize;
                for (id, positions) in references.values {
                    if !positions_match(&hop.selectors, &positions) {
                        continue;
                    }
                    work.edge()?;
                    selected += 1;
                    let target_slot = if work.has_active_domains() {
                        target_collection
                            .id_lookup
                            .get(id.as_str())
                            .copied()
                            .filter(|slot| work.is_active(hop.to, *slot))
                    } else {
                        targets
                            .iter()
                            .find(|record| record.id.0.as_ref() == id.as_str())
                            .map(|record| record.slot)
                    };
                    if let Some(target_slot) = target_slot {
                        visit_hop(
                            config,
                            depth + 1,
                            hop.to.0 as usize,
                            target_slot,
                            truth,
                            work,
                            if depth == 0 {
                                &positions
                            } else {
                                source_positions
                            },
                            terminal,
                        )?;
                    } else {
                        merge_truth(truth, None, quantifier);
                    }
                    if decisive(*truth, quantifier) {
                        break;
                    }
                }
                if selected == 0 && (!present || null || hop.selectors.iter().any(Option::is_some))
                {
                    merge_truth(truth, Some(false), quantifier);
                }
            }
            HopDirection::Backlink => {
                let target =
                    &repo.collections[collection_index].generation.records[source_slot.0 as usize];
                let mut seen = 0usize;
                let mut selected = 0usize;
                for candidate in &source_collection.identity_order {
                    if !work.is_active(hop.to, *candidate) {
                        continue;
                    }
                    work.backlink_probe()?;
                    let Some(value) =
                        read_reference_value(repo, hop.to.0 as usize, *candidate, work)?
                    else {
                        continue;
                    };
                    let references = logical_references(&value, &relation.declaration.path)?;
                    if !references
                        .values
                        .iter()
                        .any(|(id, _)| id.as_str() == target.id.0.as_ref())
                    {
                        continue;
                    }
                    work.edge()?;
                    if hop
                        .selectors
                        .first()
                        .is_none_or(|selector| selector.is_none_or(|index| index as usize == seen))
                    {
                        selected += 1;
                        visit_hop(
                            config,
                            depth + 1,
                            hop.to.0 as usize,
                            *candidate,
                            truth,
                            work,
                            source_positions,
                            terminal,
                        )?;
                    }
                    seen += 1;
                    if decisive(*truth, quantifier) {
                        break;
                    }
                }
                if selected == 0 && hop.selectors.iter().any(Option::is_some) {
                    merge_truth(truth, Some(false), quantifier);
                }
            }
        }
        return Ok(());
    }
    match hop.direction {
        HopDirection::Forward => {
            if relation.unavailable().contains(source_slot.0) {
                merge_truth(truth, None, quantifier);
                return Ok(());
            }
            let edges = relation
                .edges(source_slot)
                .ok_or("relation source slot is missing")?;
            let mut selected = 0;
            for edge in edges {
                if !positions_match(&hop.selectors, &edge.positions) {
                    continue;
                }
                work.edge()?;
                selected += 1;
                let target = if work.has_active_domains() {
                    repo.collections[hop.to.0 as usize]
                        .id_lookup
                        .get(edge.target_id())
                        .copied()
                        .filter(|slot| work.is_active(hop.to, *slot))
                } else if let RelationTarget::Resolved(target) = edge.target {
                    Some(target)
                } else {
                    None
                };
                match target {
                    Some(target) => visit_hop(
                        config,
                        depth + 1,
                        hop.to.0 as usize,
                        target,
                        truth,
                        work,
                        if depth == 0 {
                            &edge.positions
                        } else {
                            source_positions
                        },
                        terminal,
                    )?,
                    None => merge_truth(truth, None, quantifier),
                }
                if decisive(*truth, quantifier) {
                    break;
                }
            }
            if selected == 0
                && (!relation.present().contains(source_slot.0)
                    || relation.nulls().contains(source_slot.0)
                    || hop.selectors.iter().any(Option::is_some))
            {
                merge_truth(truth, Some(false), quantifier);
            }
        }
        HopDirection::Backlink => {
            let target =
                &repo.collections[collection_index].generation.records[source_slot.0 as usize];
            let sources = repo.relation_reverse_sources(relation, &target.id.0);
            let mut seen = 0usize;
            let mut selected = 0usize;
            if let Some(sources) = sources {
                if hop.selectors.iter().all(Option::is_none) {
                    // Without a numeric selector, Boolean truth is independent
                    // of identity-order rank. Visit only actual reverse hits.
                    for source in sources.iter() {
                        if !work.is_active(hop.to, RecordSlot(source)) {
                            continue;
                        }
                        work.backlink_probe()?;
                        work.edge()?;
                        selected += 1;
                        visit_hop(
                            config,
                            depth + 1,
                            hop.to.0 as usize,
                            RecordSlot(source),
                            truth,
                            work,
                            source_positions,
                            terminal,
                        )?;
                        if decisive(*truth, quantifier) {
                            break;
                        }
                    }
                } else {
                    // A positional backlink means rank among sources in
                    // identity order, so retain that ordering here.
                    for source in &source_collection.identity_order {
                        if !work.is_active(hop.to, *source) {
                            continue;
                        }
                        work.backlink_probe()?;
                        if !sources.contains(source.0) {
                            continue;
                        }
                        work.edge()?;
                        if hop.selectors.first().is_none_or(|selector| {
                            selector.is_none_or(|index| index as usize == seen)
                        }) {
                            selected += 1;
                            visit_hop(
                                config,
                                depth + 1,
                                hop.to.0 as usize,
                                *source,
                                truth,
                                work,
                                source_positions,
                                terminal,
                            )?;
                        }
                        seen += 1;
                        if decisive(*truth, quantifier) {
                            break;
                        }
                    }
                }
            }
            if selected == 0 && hop.selectors.iter().any(Option::is_some) {
                merge_truth(truth, Some(false), quantifier);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
