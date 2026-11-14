//! Opt-in observations and complete inferred posting accelerators.

use std::collections::HashMap;
use std::sync::Arc;

use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};

use crate::model::LogicalType;
use crate::posting::PostingIndex;
use crate::reference::Schema;

pub const INFERENCE_POLICY_VERSION: u32 = 5;

/// Typed intern key; automatic observation visits field steps only, while
/// positional query steps remain a separate path kind and are never inferred.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ObservedStep {
    Field(Arc<str>),
    // Automatic inference observes named fields only. Keep the typed key so a
    // future positional observer cannot conflate index 0 with object key "0".
    #[allow(dead_code)]
    ArrayIndex(u32),
}

fn typed_path(path: &[String]) -> Vec<ObservedStep> {
    path.iter()
        .map(|name| ObservedStep::Field(Arc::from(name.as_str())))
        .collect()
}

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
    pub short_string_observed_occurrences: u64,
    pub short_string_distinct_values: usize,
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
    pub short_string_covered_records: u64,
    pub short_string_unresolved_records: u64,
}

/// Published optional indexes and their descriptive report.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InferenceState {
    pub report: InferenceReport,
    pub indexes: HashMap<Vec<String>, PostingIndex>,
    pub empty_selections: HashMap<Vec<String>, RoaringBitmap>,
    /// Records whose entire selected value stream is represented by the
    /// corresponding short-string posting dictionary.
    pub short_string_coverage: HashMap<Vec<String>, RoaringBitmap>,
}

impl InferenceState {
    pub(crate) fn path_diagnostics(&self) -> Vec<InferencePathDiagnostic> {
        let mut fields = self.report.fields.iter().collect::<Vec<_>>();
        fields.sort_unstable_by(|left, right| left.path.cmp(&right.path));
        fields
            .into_iter()
            .map(|field| {
                let posting = self.indexes.get(&field.path);
                InferencePathDiagnostic {
                    path: field.path.clone(),
                    index_status: field.index_status,
                    reason: field.reason.clone(),
                    observed_kinds: field.observed_kinds.clone(),
                    kind: posting.map(|index| index.kind.clone()),
                    multi: posting.map(|index| index.multi),
                    value_keys: posting.map(PostingIndex::distinct_values),
                    length_keys: posting.map(PostingIndex::distinct_lengths),
                    covered_records: posting.map(|index| index.covered_records),
                    present_records: field.present_records,
                    short_string_covered_records: field.short_string_covered_records,
                    short_string_unresolved_records: field.short_string_unresolved_records,
                }
            })
            .collect()
    }

    pub fn validate(
        &self,
        config: InferenceConfig,
        logical: &RoaringBitmap,
        record_count: usize,
        schema: &Schema,
    ) -> Result<(), String> {
        if self.report.mode != config.mode || self.report.policy != config.policy {
            return Err("snapshot inference configuration mismatch".into());
        }
        if self.report.eligible_records != logical.len()
            || self.report.unavailable_records != record_count as u64 - logical.len()
        {
            return Err("snapshot inference domain mismatch".into());
        }
        if self.report.observed_paths != self.report.fields.len()
            || self.report.admitted_paths != self.indexes.len()
            || self.report.dropped_paths
                != self
                    .report
                    .fields
                    .iter()
                    .filter(|field| {
                        matches!(
                            field.index_status,
                            InferenceIndexStatus::Incompatible
                                | InferenceIndexStatus::Incomplete
                                | InferenceIndexStatus::BudgetExceeded
                                | InferenceIndexStatus::BelowThreshold
                        )
                    })
                    .count()
            || self.report.summary_bytes_estimate
                != self.report.fields.iter().fold(0usize, |total, field| {
                    total.saturating_add(field.summary_bytes_estimate)
                })
            || self.report.index_bytes_estimate
                != self.report.fields.iter().fold(0usize, |total, field| {
                    total.saturating_add(field.index_bytes_estimate)
                })
            || self.report.retained_bytes_estimate
                != self
                    .report
                    .summary_bytes_estimate
                    .saturating_add(self.report.index_bytes_estimate)
        {
            return Err("snapshot inference resource summary mismatch".into());
        }
        if config.mode != InferenceMode::AutoIndex
            && (!self.indexes.is_empty() || !self.empty_selections.is_empty())
        {
            return Err("snapshot inference mode has unexpected indexes".into());
        }
        if self.empty_selections.len() != self.indexes.len() {
            return Err("snapshot inferred empty-selection coverage mismatch".into());
        }
        if self
            .short_string_coverage
            .keys()
            .any(|path| !self.indexes.contains_key(path))
        {
            return Err("snapshot inferred short-string coverage without posting".into());
        }
        let mut paths = std::collections::HashSet::new();
        for field in &self.report.fields {
            if !paths.insert(&field.path)
                || field.eligible_records != logical.len()
                || field.unavailable_records != self.report.unavailable_records
                || field.present_records > logical.len()
                || field.missing_records != logical.len() - field.present_records
                || field.exact_null_records > field.null_occurrence_records
                || field.null_occurrence_records > field.present_records
                || field.retained_bytes_estimate
                    != field
                        .summary_bytes_estimate
                        .saturating_add(field.index_bytes_estimate)
            {
                return Err("snapshot inferred field report mismatch".into());
            }
            let has_index = self.indexes.contains_key(&field.path);
            if has_index != (field.index_status == InferenceIndexStatus::Built)
                || has_index != self.empty_selections.contains_key(&field.path)
            {
                return Err("snapshot inferred index/report mismatch".into());
            }
            if let Some(empty) = self.empty_selections.get(&field.path)
                && (!empty.is_subset(logical) || empty.len() != field.empty_selection_records)
            {
                return Err("snapshot inferred empty selection mismatch".into());
            }
            match self.short_string_coverage.get(&field.path) {
                Some(covered) => {
                    if !covered.is_subset(logical)
                        || covered.len() != field.short_string_covered_records
                        || field.short_string_unresolved_records != logical.len() - covered.len()
                        || field.short_string_distinct_values == 0
                    {
                        return Err("snapshot inferred short-string coverage mismatch".into());
                    }
                    let index = &self.indexes[&field.path];
                    let mut represented = RoaringBitmap::new();
                    index.for_each_value(|_, slots| {
                        represented |= slots;
                        Ok(())
                    })?;
                    if !covered.is_subset(&represented) {
                        return Err("snapshot inferred short-string coverage lacks values".into());
                    }
                }
                None if field.short_string_covered_records != 0
                    || field.short_string_unresolved_records != logical.len() =>
                {
                    return Err("snapshot inferred short-string coverage missing".into());
                }
                None => {}
            }
        }
        for (path, index) in &self.indexes {
            if path != &index.path
                || schema.fields.get(path).is_some_and(|field| field.indexed)
                || schema.is_auto_index_prohibited(path)
                || !self.report.complete
            {
                return Err("snapshot inferred index representation mismatch".into());
            }
            index.validate_eligible(record_count, logical.len() as usize)?;
            index.validate_domain(logical)?;
            let report = self
                .report
                .fields
                .iter()
                .find(|field| field.path == *path)
                .ok_or("snapshot inferred field missing")?;
            if index.presence.len() != report.present_records
                || index.nulls.len() != report.exact_null_records
                || (!report.observed_kinds.contains(&index.kind)
                    && !(index.kind == LogicalType::Array(Box::new(LogicalType::Unknown))
                        && report.array_records > 0))
            {
                return Err("snapshot inferred posting statistics mismatch".into());
            }
        }
        Ok(())
    }
}

#[path = "inference_build.rs"]
mod build;
pub(crate) use build::InferenceBuilder;

#[cfg(test)]
#[path = "inference.test.rs"]
mod tests;
