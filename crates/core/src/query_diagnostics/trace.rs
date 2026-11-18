//! Stable query diagnostic DTOs.

use serde::Serialize;

use crate::execution_options::{BigramAlphabet, SortField};
use crate::executor::{PlanningCounters, TermChoice};
use crate::model::Value;
use crate::query::{CaseSensitivity, FtsMode, Operator, Quantifier};

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
