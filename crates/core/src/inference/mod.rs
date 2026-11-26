//! Opt-in observations and complete inferred posting accelerators.

pub(crate) mod admission;

use serde::{Deserialize, Serialize};

use crate::model::LogicalType;

pub const INFERENCE_POLICY_VERSION: u32 = 5;

/// Explicit construction choice. Disabled is the schema-free default.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum InferenceMode {
    #[default]
    Disabled,
    ReportOnly,
    AutoIndex,
}

/// Bounded, versioned admission thresholds for one repository opening.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InferencePolicy {
    pub version: u32,
    pub max_paths: usize,
    pub max_depth: usize,
    pub max_record_contributions: usize,
    pub max_distinct_values_per_path: usize,
    pub max_string_bytes: usize,
    /// Inclusive decoded UTF-8 byte limit for automatic ordered string keys.
    pub short_string_max_utf8_bytes: usize,
    /// Retained per-path short-string key and membership estimate.
    pub max_short_string_index_bytes_per_path: usize,
    pub max_distinct_string_bytes: usize,
    pub max_batch_bytes: usize,
    /// Coordinator retained budget; the repository cap also covers live batches.
    pub max_total_bytes: usize,
    pub max_repository_bytes: usize,
    pub min_nonblank_occurrences: u64,
    pub min_nonblank_records: u64,
    pub max_distinct_nonblank_strings: usize,
    pub minimum_repeated_percent: u8,
}

impl Default for InferencePolicy {
    fn default() -> Self {
        Self {
            version: INFERENCE_POLICY_VERSION,
            max_paths: 4096,
            max_depth: 16,
            max_record_contributions: 4096,
            max_distinct_values_per_path: 4096,
            max_string_bytes: 64,
            short_string_max_utf8_bytes: 64,
            max_short_string_index_bytes_per_path: 24 * 1024 * 1024,
            max_distinct_string_bytes: 8 * 1024,
            // Dense 512-file batches and large collections can exceed the
            // smaller observation budgets before useful paths are admitted.
            // These are bounded estimates, not process-memory limits.
            max_batch_bytes: 16 * 1024 * 1024,
            max_total_bytes: 512 * 1024 * 1024,
            max_repository_bytes: 768 * 1024 * 1024,
            min_nonblank_occurrences: 20,
            min_nonblank_records: 10,
            max_distinct_nonblank_strings: 64,
            minimum_repeated_percent: 80,
        }
    }
}

/// Mode and admission policy frozen for one immutable repository generation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct InferenceConfig {
    pub mode: InferenceMode,
    pub policy: InferencePolicy,
}

impl InferenceConfig {
    pub fn validate(self) -> Result<(), String> {
        let policy = self.policy;
        if policy.version != INFERENCE_POLICY_VERSION
            || policy.max_paths == 0
            || policy.max_depth == 0
            || policy.max_record_contributions == 0
            || policy.max_distinct_values_per_path == 0
            || policy.max_string_bytes == 0
            || policy.short_string_max_utf8_bytes == 0
            || policy.max_short_string_index_bytes_per_path == 0
            || policy.max_distinct_string_bytes == 0
            || policy.max_batch_bytes == 0
            || policy.max_total_bytes < policy.max_batch_bytes
            || policy.max_repository_bytes < policy.max_total_bytes
            || (self.mode != InferenceMode::Disabled
                && policy.max_repository_bytes - policy.max_total_bytes < policy.max_batch_bytes)
            || policy.minimum_repeated_percent > 100
        {
            return Err("invalid inference policy".into());
        }
        Ok(())
    }

    /// Reserve room for every admitted worker batch in the repository-wide
    /// inference estimate. The per-batch admission threshold stays fixed.
    pub(crate) fn max_build_jobs(self, requested: usize) -> usize {
        if self.mode == InferenceMode::Disabled {
            return requested;
        }
        let slack = self.policy.max_repository_bytes - self.policy.max_total_bytes;
        requested.min(slack / self.policy.max_batch_bytes).max(1)
    }
}

/// Why an observed field did or did not receive a typed accelerator.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum InferenceIndexStatus {
    Built,
    NotRequested,
    Redundant,
    ExplicitOptOut,
    Incompatible,
    Incomplete,
    BudgetExceeded,
    BelowThreshold,
}

/// Complete or explicitly incomplete statistics for one logical field path.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum OccurrenceCountStatus {
    /// All three occurrence totals are exact for the reported domain.
    Exact,
    /// Each occurrence total is at least the true survivor total.
    UpperBound,
    /// Observed or inherited tallies without a guaranteed survivor bound.
    IncompleteObservation,
}

impl Default for OccurrenceCountStatus {
    fn default() -> Self {
        Self::IncompleteObservation
    }
}

/// Complete or explicitly incomplete statistics for one logical field path.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InferredFieldReport {
    pub path: Vec<String>,
    pub observed_kinds: Vec<LogicalType>,
    pub scalar_records: u64,
    pub array_records: u64,
    pub object_records: u64,
    pub array_nesting: u8,
    pub integer_only: Option<bool>,
    pub eligible_records: u64,
    pub unavailable_records: u64,
    pub present_records: u64,
    pub missing_records: u64,
    pub null_occurrence_records: u64,
    pub exact_null_records: u64,
    pub empty_array_records: u64,
    pub empty_selection_records: u64,
    pub value_occurrences: u64,
    pub nonblank_occurrences: u64,
    pub nonblank_records: u64,
    pub distinct_nonblank_strings: Option<usize>,
    #[serde(default)]
    pub distinct_nonblank_strings_truncated: bool,
    pub short_string_observed_occurrences: u64,
    /// Joint contract for `value_occurrences`, `nonblank_occurrences`, and
    /// `short_string_observed_occurrences`.
    #[serde(default)]
    pub occurrence_count_status: OccurrenceCountStatus,
    pub short_string_distinct_values: usize,
    #[serde(default)]
    pub short_string_distinct_values_truncated: bool,
    pub short_string_covered_records: u64,
    pub short_string_unresolved_records: u64,
    pub index_status: InferenceIndexStatus,
    pub reason: String,
    pub complete: bool,
    pub summary_bytes_estimate: usize,
    pub index_bytes_estimate: usize,
    pub retained_bytes_estimate: usize,
}

/// Per-collection inference report for one source generation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InferenceReport {
    pub mode: InferenceMode,
    pub policy: InferencePolicy,
    pub eligible_records: u64,
    pub unavailable_records: u64,
    pub omitted_paths: u64,
    pub complete: bool,
    pub observed_paths: usize,
    pub admitted_paths: usize,
    pub dropped_paths: usize,
    pub summary_bytes_estimate: usize,
    pub index_bytes_estimate: usize,
    pub retained_bytes_estimate: usize,
    pub fields: Vec<InferredFieldReport>,
}

/// Construction-only inference work and retained-state estimates.
/// Budget charges are policy estimates, not allocator measurements or RSS.
#[derive(Clone, Copy, Debug, Default)]
pub struct InferenceBuildStats {
    pub observed_records: u64,
    pub merged_batches: u64,
    pub merge_ns: u64,
    pub finish_ns: u64,
    pub cumulative_budget_bytes: usize,
    pub unique_path_bytes: usize,
    pub repeated_batch_path_bytes: usize,
    pub retained_manual_summary_bytes: usize,
}

impl InferenceBuildStats {
    pub(crate) fn absorb(&mut self, other: Self) {
        self.observed_records = self.observed_records.saturating_add(other.observed_records);
        self.merged_batches = self.merged_batches.saturating_add(other.merged_batches);
        self.merge_ns = self.merge_ns.saturating_add(other.merge_ns);
        self.finish_ns = self.finish_ns.saturating_add(other.finish_ns);
        self.cumulative_budget_bytes = self
            .cumulative_budget_bytes
            .saturating_add(other.cumulative_budget_bytes);
        self.unique_path_bytes = self
            .unique_path_bytes
            .saturating_add(other.unique_path_bytes);
        self.repeated_batch_path_bytes = self
            .repeated_batch_path_bytes
            .saturating_add(other.repeated_batch_path_bytes);
        self.retained_manual_summary_bytes = self
            .retained_manual_summary_bytes
            .saturating_add(other.retained_manual_summary_bytes);
    }
}

/// One observed path and its ordered posting admission outcome.
///
/// A built index owns B-tree value and length dictionaries, even when either
/// dictionary has no keys. Multivalued and partially covered strings can
/// still require source verification for particular queries.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InferencePathDiagnostic {
    pub path: Vec<String>,
    pub index_status: InferenceIndexStatus,
    pub reason: String,
    pub observed_kinds: Vec<LogicalType>,
    pub kind: Option<LogicalType>,
    pub multi: Option<bool>,
    pub value_keys: Option<usize>,
    pub length_keys: Option<usize>,
    pub covered_records: Option<u32>,
    pub present_records: u64,
    #[serde(default)]
    pub value_dictionary_complete: bool,
    #[serde(default)]
    pub short_string_candidates: bool,
    #[serde(default)]
    pub empty_string_exact: bool,
    #[serde(default)]
    pub array_length_complete: bool,
    #[serde(default)]
    /// Compatibility name for the retired structural posting payload. It now
    /// reports whether the index has complete terminal-selection coverage.
    pub structural_facts: bool,
    pub short_string_covered_records: u64,
    pub short_string_unresolved_records: u64,
}

#[cfg(test)]
mod tests;
