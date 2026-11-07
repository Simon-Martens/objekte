//! Fixed request inputs and measurement envelopes. No execution is performed here.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Deref;
use std::path::Path;

use crate::model::Value;


#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReturnMode {
    Locator,
    Bytes,
    Logical,
}


/// Field and direction requested by a workload case.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SortField {
    pub field: String,
    pub descending: bool,
    /// Typed expression for reductions and positional or transformed fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression: Option<SortExpression>,
}


/// Reduction within one source record's reached sort values.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortReducer {
    Min,
    Max,
    First,
    Last,
}


/// Programmatic sort expression, independent of its display spelling.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct SortExpression {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reducer: Option<SortReducer>,
}


impl SortField {
    pub fn simple(field: impl Into<String>, descending: bool) -> Self {
        Self {
            field: field.into(),
            descending,
            expression: None,
        }
    }

    pub fn computed(expression: SortExpression, descending: bool) -> Self {
        Self {
            field: expression.path.clone(),
            descending,
            expression: Some(expression),
        }
    }
}


/// Fixed benchmark query and expected execution options.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkloadCase {
    pub id: String,
    pub collection: String,
    pub fts: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "crate::query::CaseSensitivity::is_sensitive"
    )]
    pub fts_case_sensitivity: crate::query::CaseSensitivity,
    #[serde(default, skip_serializing_if = "crate::query::FtsMode::is_literal")]
    pub fts_mode: crate::query::FtsMode,
    pub filter: Option<String>,
    #[serde(default)]
    pub fts_fields: Vec<String>,
    #[serde(default)]
    pub sort: Vec<SortField>,
    pub limit: usize,
    pub offset: usize,
    pub count: bool,
    pub return_mode: ReturnMode,
    pub evidence: bool,
}


/// A benchmark request with separately typed data parameters. The historical
/// 150 cases remain ordinary `WorkloadCase`s with no parameter field.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchmarkCase {
    #[serde(flatten)]
    pub request: WorkloadCase,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parameters: BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock_unix_seconds: Option<i64>,
}


impl Deref for BenchmarkCase {
    type Target = WorkloadCase;

    fn deref(&self) -> &Self::Target {
        &self.request
    }
}


impl BenchmarkCase {
    /// Bind values once, before the timed search call.
    pub fn query(&self) -> Result<crate::query::Query, String> {
        let mut values = HashMap::<String, Value>::new();
        for (name, encoded) in &self.parameters {
            values.insert(
                name.clone(),
                crate::parameters::parse_parameter_json(&encoded.to_string())?,
            );
        }
        let query = crate::query::Query::from_workload(&self.request)?.with_parameters(&values)?;
        if let Some(seconds) = self.clock_unix_seconds {
            query.with_clock(
                time::OffsetDateTime::from_unix_timestamp(seconds)
                    .map_err(|error| error.to_string())?,
            )
        } else {
            Ok(query)
        }
    }
}


#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BigramAlphabet {
    Ascii,
    Unicode,
}


/// Alphabet and cache settings for one benchmark profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub alphabet: BigramAlphabet,
    pub verified_cache_bytes: usize,
    pub warm: bool,
}


pub const PROFILES: [Profile; 4] = [
    Profile {
        alphabet: BigramAlphabet::Ascii,
        verified_cache_bytes: 0,
        warm: false,
    },
    Profile {
        alphabet: BigramAlphabet::Ascii,
        verified_cache_bytes: 64 * 1024 * 1024,
        warm: true,
    },
    Profile {
        alphabet: BigramAlphabet::Unicode,
        verified_cache_bytes: 0,
        warm: false,
    },
    Profile {
        alphabet: BigramAlphabet::Unicode,
        verified_cache_bytes: 64 * 1024 * 1024,
        warm: true,
    },
];


/// Observed candidate, source, cache, and result work.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WorkCounters {
    pub exact_posting_matches: u64,
    pub candidates: u64,
    pub scanned_records: u64,
    pub total_matches: u64,
    /// Whether `total_matches` covers the entire selected query domain.
    #[serde(default)]
    pub match_count_complete: bool,
    pub returned_records: u64,
    pub files_opened: u64,
    /// Physical primary-source files opened at least once. Relation target
    /// opens are counted in `files_opened` and `relation_target_reads`.
    #[serde(default)]
    pub distinct_files: u64,
    pub bytes_read: u64,
    /// Peak retained page payload bytes, estimated from source record lengths.
    #[serde(default)]
    pub peak_retained_payload_bytes: u64,
    /// Number of matching records whose computed ordering keys were evaluated.
    #[serde(default)]
    pub sort_key_evaluations: u64,
    /// Peak variable-size key bytes retained by the bounded page winners.
    #[serde(default)]
    pub peak_retained_sort_key_bytes: u64,
    /// Related logical target reads and cache reuse for sort expressions.
    #[serde(default)]
    pub sort_target_reads: u64,
    #[serde(default)]
    pub sort_target_reuse: u64,
    pub record_parses: u64,
    /// Raw fallback records checked against the original-text literal.
    #[serde(default)]
    pub raw_text_verifications: u64,
    /// Filter terms actually evaluated against a decoded record (proof hits excluded).
    pub residual_evaluations: u64,
    /// Additional logical source records opened by independent relation scans.
    #[serde(default)]
    pub relation_source_reads: u64,
    /// Target records opened for exact relation predicate or FTS evaluation.
    #[serde(default)]
    pub relation_target_reads: u64,
    /// Physical target-source handles opened for relation verification.
    #[serde(default)]
    pub relation_target_file_opens: u64,
    /// Relation occurrences visited after positional selection.
    #[serde(default)]
    pub relation_edge_visits: u64,
    /// Reverse posting entries or identity-order candidates examined by backlinks.
    #[serde(default)]
    pub relation_backlink_probe_visits: u64,
    /// Bytes read for relation target/source verification and prefetch.
    #[serde(default)]
    pub relation_bytes_read: u64,
    /// Distinct terminal candidates inspected by relation semijoins.
    #[serde(default)]
    pub relation_target_candidates: u64,
    /// Terminal records confirmed true before reverse mapping.
    #[serde(default)]
    pub relation_target_matches: u64,
    /// Typed reverse posting lists visited during relation semijoins.
    #[serde(default)]
    pub relation_reverse_postings_visited: u64,
    /// Target values reused by reverse mapping or source-dependent comparisons.
    #[serde(default)]
    pub relation_verified_targets_reused: u64,
    #[serde(default)]
    pub preparation_ns: u64,
    #[serde(default)]
    pub planning_ns: u64,
    pub prepared_plan_hit: bool,
    pub verified_result_hit: bool,
}


/// One measured query execution and its counters.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TimingSample {
    pub case_id: String,
    pub profile: Profile,
    pub repetition: usize,
    pub status: String,
    pub error: Option<String>,
    pub total_ns: Option<u64>,
    pub counters: Option<WorkCounters>,
}


/// Build and fixture fingerprints for a benchmark run.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunIdentity {
    pub run_id: String,
    pub code_revision: String,
    pub compiler: String,
    pub repository_root: String,
    pub corpus_fingerprint: String,
    pub schema_fingerprint: String,
    pub workload_fingerprint: String,
    pub repetitions: usize,
    pub profiles: Vec<Profile>,
}


pub const REPORT_FILES: [&str; 7] = [
    "run-manifest.json",
    "queries.json",
    "measurements.jsonl",
    "query-results.csv",
    "query-plans.jsonl",
    "index-results.json",
    "REPORT.md",
];

pub fn load_manifest(path: &Path) -> Result<Vec<WorkloadCase>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let cases: Vec<WorkloadCase> = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    validate_manifest(&cases)?;
    Ok(cases)
}

/// Load a superset of the fixed workload without changing its first 150
/// requests or their historical benchmark identity.
pub fn load_extended_manifest(
    baseline_path: &Path,
    extended_path: &Path,
) -> Result<Vec<BenchmarkCase>, String> {
    let baseline = load_manifest(baseline_path)?;
    let bytes = std::fs::read(extended_path)
        .map_err(|error| format!("{}: {error}", extended_path.display()))?;
    let cases: Vec<BenchmarkCase> =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if cases.len() <= baseline.len() {
        return Err("extended workload has no additional cases".into());
    }
    for (original, extended) in baseline.iter().zip(&cases) {
        if original != &extended.request
            || !extended.parameters.is_empty()
            || extended.clock_unix_seconds.is_some()
        {
            return Err(format!(
                "extended workload changed historical case {}",
                original.id
            ));
        }
    }
    for (index, case) in cases.iter().enumerate().skip(baseline.len()) {
        if case.id != format!("Q{:03}", index + 1)
            || case.collection.is_empty()
            || (case.fts.is_none() && case.filter.is_none())
            || case.limit == 0
        {
            return Err(format!("invalid extended workload case at {index}"));
        }
        case.query()
            .map_err(|error| format!("{}: {error}", case.id))?;
    }
    Ok(cases)
}

pub fn validate_manifest(cases: &[WorkloadCase]) -> Result<(), String> {
    if cases.len() != 150 {
        return Err(format!("expected 150 workload cases, got {}", cases.len()));
    }
    let mut seen = HashSet::new();
    for (i, case) in cases.iter().enumerate() {
        let expected = format!("Q{:03}", i + 1);
        if case.id != expected || !seen.insert(&case.id) {
            return Err(format!("expected unique {expected}, got {}", case.id));
        }
        if case.collection.is_empty() || (case.fts.is_none() && case.filter.is_none()) {
            return Err(format!("{} has no collection or predicate", case.id));
        }
        if case.limit == 0 {
            return Err(format!("{} has zero limit", case.id));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "workload.test.rs"]
mod tests;
