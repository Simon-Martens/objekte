//! Stable query diagnostic DTOs.

use serde::Serialize;

use crate::index::bigram::BigramAlphabet;
use crate::measurement::WorkCounters;
use crate::model::Value;
use crate::query::binding::filter::BoundExpr;
use crate::query::binding::fts::BoundFts;
use crate::query::options::SortField;
use crate::query::{CaseSensitivity, FtsMode, Operator, Quantifier};
use crate::query_execution::restrictions::{PlanningCounters, TermChoice, estimate_fts_candidates};
use crate::repository::Repository;
use crate::repository::state::CollectionState;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationStatus {
    Executed,
    Skipped,
    CacheHit,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ExecutionObservation {
    pub node_id: u32,
    /// Request-local execution phase, such as `search`, `ordered_page` or
    /// `exact_count`.
    pub phase: String,
    pub term_id: u32,
    /// All logical leaves served by this physical node.
    pub term_ids: Vec<u32>,
    pub provider_id: u32,
    pub operation_provider_id: u32,
    pub operation_id: u32,
    /// Concrete access family selected behind the provider handle.
    pub access_kind: Option<String>,
    pub collection: u32,
    pub domain: String,
    pub input_records: u64,
    pub candidate_records: u64,
    pub proven_records: u64,
    pub residual_verifications: u64,
    pub status: ObservationStatus,
}

/// One resolved relation hop shown only in diagnostic EXPLAIN output.
#[derive(Clone, Debug, Serialize)]
pub struct RelationHopDiagnostic {
    pub relation: crate::schema::relations::RelationId,
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
    pub observations: Vec<ExecutionObservation>,
}

/// Observed source and predicate work for one plan.
#[derive(Clone, Debug, Default, Serialize)]
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
    pub relation_reverse_members_visited: u64,
    pub relation_verified_targets_reused: u64,
}

pub(crate) fn bound_term_diagnostics(
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

pub(crate) fn fts_covered_target(fts: Option<&BoundFts>) -> String {
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

pub(crate) fn regex_prefilter_reason(
    collection: &CollectionState,
    fts: Option<&BoundFts>,
) -> Result<Option<String>, String> {
    let Some(fts) = fts.filter(|fts| matches!(fts.mode, FtsMode::Regex(_))) else {
        return Ok(None);
    };
    if fts
        .fallback_regex
        .as_ref()
        .and_then(|regex| regex.condition())
        .is_none()
    {
        return Ok(Some(
            "regex has no bounded necessary literal; scanner fallback".into(),
        ));
    }
    let reason = if estimate_fts_candidates(collection, fts)?.is_none() {
        "regex has no usable grams or compatible index coverage; scanner fallback"
    } else {
        "regex uses necessary folded grams; exact verification remains"
    };
    Ok(Some(reason.into()))
}

pub(crate) fn finish_trace(
    traces: &mut [QueryPlanTrace],
    before: &WorkCounters,
    after: &WorkCounters,
) {
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
            relation_reverse_members_visited: after.relation_reverse_members_visited
                - before.relation_reverse_members_visited,
            relation_verified_targets_reused: after.relation_verified_targets_reused
                - before.relation_verified_targets_reused,
        });
    }
}

pub(crate) fn attach_execution_observations(
    repo: &Repository,
    traces: &mut [QueryPlanTrace],
    observations: Vec<ExecutionObservation>,
) {
    for observation in observations {
        let Some(collection) = repo.collections.get(observation.collection as usize) else {
            continue;
        };
        if let Some(trace) = traces
            .iter_mut()
            .find(|trace| trace.collection == collection.definition.name)
        {
            match observation.access_kind.as_deref() {
                Some("relation_target_first") => {
                    trace.fts_strategy = "relation target-first semijoin".into();
                }
                Some("relation_source_restricted") => {
                    trace.fts_strategy = "relation source-restricted semijoin".into();
                }
                _ => {}
            }
            let mut matched_term = false;
            for choice in &mut trace.terms {
                let Some(term_id) = choice.term_id else {
                    continue;
                };
                if !observation.term_ids.contains(&term_id) {
                    continue;
                }
                matched_term = true;
                choice.actual_input = choice.actual_input.max(observation.input_records);
                choice.actual_output = Some(
                    choice
                        .actual_output
                        .unwrap_or(0)
                        .max(observation.candidate_records),
                );
                choice.estimated_input = None;
                choice.executed |= observation.status == ObservationStatus::Executed;
                if let Some(kind) = observation
                    .access_kind
                    .as_deref()
                    .and_then(crate::query_execution::restrictions::access_path_kind_from_name)
                    && (observation.provider_id == 4
                        || !matches!(
                            choice.selected.kind,
                            crate::query_execution::restrictions::AccessPathKind::RelationTargetFirst
                                | crate::query_execution::restrictions::AccessPathKind::RelationSourceRestricted
                        ))
                    && access_kind_priority(kind) >= access_kind_priority(choice.selected.kind)
                {
                    choice.selected.kind = kind;
                    choice.residual_verification =
                        observation.proven_records != observation.candidate_records;
                }
                if observation.status == ObservationStatus::Skipped {
                    choice.reason = "skipped after an exact empty input".into();
                }
            }
            if !matched_term && trace.terms.len() == 1 && trace.terms[0].term_id.is_some() {
                let choice = &mut trace.terms[0];
                choice.actual_input = choice.actual_input.max(observation.input_records);
                choice.actual_output = Some(
                    choice
                        .actual_output
                        .unwrap_or(0)
                        .max(observation.candidate_records),
                );
                choice.estimated_input = None;
                choice.executed |= observation.status == ObservationStatus::Executed;
                if let Some(kind) = observation
                    .access_kind
                    .as_deref()
                    .and_then(crate::query_execution::restrictions::access_path_kind_from_name)
                    && access_kind_priority(kind) >= access_kind_priority(choice.selected.kind)
                {
                    choice.selected.kind = kind;
                    choice.residual_verification =
                        observation.proven_records != observation.candidate_records;
                }
            }
            trace.observations.push(observation);
        }
    }
}

fn access_kind_priority(kind: crate::query_execution::restrictions::AccessPathKind) -> u8 {
    use crate::query_execution::restrictions::AccessPathKind;
    match kind {
        AccessPathKind::RelationTargetFirst | AccessPathKind::RelationSourceRestricted => 5,
        AccessPathKind::Posting => 4,
        AccessPathKind::SortColumn | AccessPathKind::Metadata | AccessPathKind::FilePathIndex => 3,
        AccessPathKind::CustomBigram | AccessPathKind::RawBigram => 2,
        AccessPathKind::Scanner => 0,
    }
}
