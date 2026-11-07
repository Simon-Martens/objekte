//! Opt-in observations and complete inferred posting accelerators.

use std::collections::HashMap;

use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};

use crate::model::{LogicalType, Value};
use crate::posting::{PostingIndex, ScalarKey};
use crate::query::{Operator, Quantifier};
use crate::reference::{BoundTerm, Derived, Schema, Transform};

pub const INFERENCE_POLICY_VERSION: u32 = 1;


/// Typed intern key; automatic observation visits field steps only, while
/// positional query steps remain a separate path kind and are never inferred.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ObservedStep {
    Field(String),
    // Automatic inference observes named fields only. Keep the typed key so a
    // future positional observer cannot conflate index 0 with object key "0".
    #[allow(dead_code)]
    ArrayIndex(u32),
}


fn typed_path(path: &[String]) -> Vec<ObservedStep> {
    path.iter().cloned().map(ObservedStep::Field).collect()
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
            max_distinct_string_bytes: 8 * 1024,
            max_batch_bytes: 256 * 1024,
            max_total_bytes: 48 * 1024 * 1024,
            max_repository_bytes: 64 * 1024 * 1024,
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
            || policy.max_distinct_string_bytes == 0
            || policy.max_batch_bytes == 0
            || policy.max_total_bytes < policy.max_batch_bytes
            || policy.max_repository_bytes < policy.max_total_bytes
            || policy.minimum_repeated_percent > 100
        {
            return Err("invalid inference policy".into());
        }
        Ok(())
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


/// Published optional indexes and their descriptive report.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InferenceState {
    pub report: InferenceReport,
    pub indexes: HashMap<Vec<String>, PostingIndex>,
    pub empty_selections: HashMap<Vec<String>, RoaringBitmap>,
}


impl InferenceState {
    /// A complete inferred posting may restrict a dynamic term. Ordinary
    /// comparisons retain their residual: the posting records value membership,
    /// not the term's full selector and universal quantifier semantics.
    pub(crate) fn candidates(
        &self,
        term: &BoundTerm,
        domain: &RoaringBitmap,
        input: &RoaringBitmap,
    ) -> Option<(RoaringBitmap, bool)> {
        if self.report.mode != InferenceMode::AutoIndex
            || !self.report.complete
            || term.path.root != crate::model::PathRoot::Data
            || term.kind != LogicalType::Unknown
            || term.related.is_some()
            || term.rhs.is_some()
            || term.query_path.has_position
            || term.operation.is_some()
            || term.matcher.is_some()
            || term.transform != Transform::Identity
            || term.period.is_some()
        {
            return None;
        }
        let index = self.indexes.get(&term.path.segments)?;
        if index.covered_records as u64 != domain.len() {
            return None;
        }
        if matches!(term.derived, Derived::Exists | Derived::Null) {
            return index
                .exact_set_restricted(term, domain, input)
                .map(|set| (set, true));
        }
        if term.derived != Derived::None || matches!(term.literal, Value::Null) {
            return None;
        }
        if !matches!(
            term.operator,
            Operator::Eq | Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le
        ) {
            return None;
        }
        let key = ScalarKey::from_value(&term.literal)?;
        let mut candidate = RoaringBitmap::new();
        for (value, slots) in &index.values {
            if std::mem::discriminant(value) != std::mem::discriminant(&key) {
                continue;
            }
            let ordering = value.cmp(&key);
            let selected = match term.operator {
                Operator::Eq => ordering.is_eq(),
                Operator::Gt => ordering.is_gt(),
                Operator::Ge => ordering.is_ge(),
                Operator::Lt => ordering.is_lt(),
                Operator::Le => ordering.is_le(),
                _ => false,
            };
            if selected {
                candidate |= slots;
            }
        }
        if term.quantifier != Quantifier::Any {
            candidate |= self.empty_selections.get(&term.path.segments)?;
        }
        Some((&candidate & input, false))
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
                || has_index && !field.complete
            {
                return Err("snapshot inferred index/report mismatch".into());
            }
            if let Some(empty) = self.empty_selections.get(&field.path)
                && (!empty.is_subset(logical) || empty.len() != field.empty_selection_records)
            {
                return Err("snapshot inferred empty selection mismatch".into());
            }
        }
        for (path, index) in &self.indexes {
            if path != &index.path || schema.fields.contains_key(path) || !self.report.complete {
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
                || !report.observed_kinds.contains(&index.kind)
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
