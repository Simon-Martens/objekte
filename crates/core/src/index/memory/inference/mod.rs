//! In-memory observations and inferred posting dictionaries.

use std::collections::HashMap;

use roaring::RoaringBitmap;

use crate::inference::InferenceReport;
#[cfg(test)]
use crate::inference::{
    InferenceConfig, InferenceIndexStatus, InferenceMode, InferencePathDiagnostic,
};
#[cfg(test)]
use crate::model::LogicalType;
#[cfg(test)]
use crate::schema::Schema;

use super::posting::PostingIndex;

mod builder;
#[cfg(test)]
pub(crate) use builder::ObservedStep;
pub(crate) use builder::{InferenceBuilder, InferenceObservation};

/// Published optional indexes and their descriptive report.
#[derive(Clone, Debug)]
pub(crate) struct InferenceState {
    pub report: InferenceReport,
    pub(crate) indexes: HashMap<Vec<String>, PostingIndex>,
    pub(in crate::index::memory) placed: super::posting::placement::PlacedPostings,
    /// Records whose entire selected value stream is represented by the
    /// corresponding short-string posting dictionary.
    pub short_string_coverage: HashMap<Vec<String>, RoaringBitmap>,
}

impl InferenceState {
    pub(in crate::index::memory) fn reader(
        &self,
        path: &[String],
    ) -> Option<&dyn crate::index::storage::posting::PostingReader> {
        if let Some(index) = self.indexes.get(path) {
            return Some(index);
        }
        self.placed
            .get(path, None)
            .map(|placed| placed.reader.as_ref())
    }

    #[cfg(test)]
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
                    value_dictionary_complete: posting.is_some_and(|index| index.value_complete),
                    short_string_candidates: self.short_string_coverage.contains_key(&field.path),
                    empty_string_exact: posting.is_some_and(|index| index.empty_string_complete),
                    array_length_complete: posting.is_some_and(|index| index.length_complete),
                    structural_facts: posting.is_some_and(|index| index.summary.is_some()),
                    short_string_covered_records: field.short_string_covered_records,
                    short_string_unresolved_records: field.short_string_unresolved_records,
                }
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn validate(
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
        if config.mode != InferenceMode::AutoIndex && !self.indexes.is_empty() {
            return Err("snapshot inference mode has unexpected indexes".into());
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
                || field.scalar_records > field.present_records
                || field.array_records > field.present_records
                || field.object_records > field.present_records
                || field.empty_array_records > field.array_records
                || field.empty_selection_records > field.present_records
                || field.nonblank_records > field.present_records
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
            if has_index != (field.index_status == InferenceIndexStatus::Built) {
                return Err("snapshot inferred index/report mismatch".into());
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
            let nulls = index
                .values
                .get(&crate::index::memory::posting::ScalarKey::Null)
                .map_or(0, RoaringBitmap::len);
            let empty_unknown_array = index.kind
                == LogicalType::Array(Box::new(LogicalType::Unknown))
                && !report.complete
                && index
                    .summary
                    .is_some_and(|summary| summary.may_match_array())
                && index.presence.is_empty()
                && index.values.is_empty()
                && index.lengths.is_empty();
            if (!report.observed_kinds.contains(&index.kind)
                && !(index.kind == LogicalType::Array(Box::new(LogicalType::Unknown))
                    && (report.array_records > 0 || empty_unknown_array)))
                || nulls > report.null_occurrence_records
                || index.summary.is_some_and(|summary| {
                    !summary.may_select_multiple() && nulls != report.exact_null_records
                })
            {
                return Err("snapshot inferred posting statistics mismatch".into());
            }
        }
        Ok(())
    }
}
