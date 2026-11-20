use std::collections::HashMap;

use roaring::RoaringBitmap;

use super::super::IndexedCollectionSegment;
use super::DomainProjection;
use super::postings::{project_posting, scatter_posting_partitions};
use super::routing::{
    PartitionRouting, SlotMap, project_bitmap_in_range, scatter_bitmap_partitions,
};
use crate::inference::{InferenceIndexStatus, InferenceState};

impl IndexedCollectionSegment {
    pub(crate) fn project_inference_for_partitions(
        &self,
        routing: &PartitionRouting,
        domains: &[DomainProjection],
    ) -> Result<Vec<Option<InferenceState>>, String> {
        if domains.len() != routing.selected.len() {
            return Err("partition inference domain count mismatch".into());
        }
        let Some(source) = &self.inference else {
            return Ok((0..routing.selected.len()).map(|_| None).collect());
        };
        let mut parts = (0..routing.selected.len())
            .map(|_| InferenceState {
                report: source.report.clone(),
                indexes: HashMap::with_capacity(source.indexes.len()),
                short_string_coverage: HashMap::with_capacity(source.short_string_coverage.len()),
            })
            .collect::<Vec<_>>();
        for (path, index) in &source.indexes {
            for (part, scattered) in parts
                .iter_mut()
                .zip(scatter_posting_partitions(index, routing)?)
            {
                part.indexes.insert(path.clone(), scattered);
            }
        }
        for (path, bitmap) in &source.short_string_coverage {
            for (part, scattered) in parts
                .iter_mut()
                .zip(scatter_bitmap_partitions(bitmap, routing)?)
            {
                part.short_string_coverage.insert(path.clone(), scattered);
            }
        }
        for (number, part) in parts.iter_mut().enumerate() {
            finish_projected_inference(
                part,
                domains[number].logical.len(),
                routing.selected[number].len(),
            )?;
        }
        Ok(parts.into_iter().map(Some).collect())
    }
}

pub(crate) fn project_inference(
    source: &Option<InferenceState>,
    slots: &SlotMap<'_>,
    logical: &RoaringBitmap,
    new_count: usize,
    bounds: Option<(u32, u32)>,
) -> Result<Option<InferenceState>, String> {
    let Some(source) = source else {
        return Ok(None);
    };
    let mut state = InferenceState {
        report: source.report.clone(),
        indexes: HashMap::new(),
        short_string_coverage: HashMap::new(),
    };
    let eligible = u32::try_from(logical.len()).map_err(|_| "inference slot overflow")?;
    state.indexes = source
        .indexes
        .iter()
        .map(|(path, index)| {
            project_posting(index, slots, eligible, bounds).map(|index| (path.clone(), index))
        })
        .collect::<Result<HashMap<_, _>, _>>()?;
    state.short_string_coverage = source
        .short_string_coverage
        .iter()
        .map(|(path, bitmap)| {
            project_bitmap_in_range(bitmap, slots, bounds).map(|bitmap| (path.clone(), bitmap))
        })
        .collect::<Result<HashMap<_, _>, _>>()?;
    finish_projected_inference(&mut state, logical.len(), new_count)?;
    Ok(Some(state))
}

fn finish_projected_inference(
    state: &mut InferenceState,
    eligible: u64,
    new_count: usize,
) -> Result<(), String> {
    let unavailable = (new_count as u64)
        .checked_sub(eligible)
        .ok_or("inference eligible domain exceeds record count")?;
    state.report.eligible_records = eligible;
    state.report.unavailable_records = unavailable;
    for field in &mut state.report.fields {
        field.eligible_records = eligible;
        field.unavailable_records = unavailable;
        field.distinct_nonblank_strings = None;
        if let Some(index) = state.indexes.get(&field.path) {
            field.index_status = InferenceIndexStatus::Built;
            let summary = index.summary;
            field.complete = summary.is_some();
            if summary.is_some_and(|summary| summary.may_have_empty_traversal()) {
                field.present_records = field.present_records.min(eligible);
                field.empty_selection_records = field.empty_selection_records.min(eligible);
                field.complete = false;
            } else {
                field.present_records = index.presence.len();
                field.empty_selection_records = 0;
            }
            let nulls = index
                .values
                .get(&crate::index::posting::ScalarKey::Null)
                .map_or(0, RoaringBitmap::len);
            field.null_occurrence_records = nulls;
            field.exact_null_records =
                if summary.is_some_and(|summary| !summary.may_select_multiple()) {
                    nulls
                } else {
                    field.exact_null_records.min(nulls)
                };
            if summary.is_some_and(|summary| summary.may_select_multiple()) {
                field.complete = false;
            }
            field.short_string_covered_records = state
                .short_string_coverage
                .get(&field.path)
                .map_or(0, RoaringBitmap::len);
            field.short_string_distinct_values = if field.short_string_covered_records > 0 {
                index.distinct_values()
            } else {
                0
            };
            field.scalar_records = if index.multi {
                0
            } else {
                field.present_records
            };
            field.array_records = if index.multi {
                field.present_records
            } else {
                0
            };
        } else {
            field.index_status = InferenceIndexStatus::Incomplete;
            field.complete = false;
            field.reason = "retired records removed from incomplete inference summary".into();
            field.present_records = 0;
            field.null_occurrence_records = 0;
            field.exact_null_records = 0;
            field.empty_selection_records = 0;
            field.short_string_covered_records = 0;
            field.short_string_distinct_values = 0;
            field.scalar_records = 0;
            field.array_records = 0;
            field.index_bytes_estimate = 0;
        }
        field.missing_records = eligible - field.present_records;
        field.short_string_unresolved_records = eligible - field.short_string_covered_records;
        field.object_records = 0;
        field.value_occurrences = 0;
        field.nonblank_occurrences = 0;
        field.nonblank_records = 0;
        field.short_string_observed_occurrences = 0;
        field.retained_bytes_estimate = field
            .summary_bytes_estimate
            .saturating_add(field.index_bytes_estimate);
    }
    state.report.admitted_paths = state.indexes.len();
    state.report.dropped_paths = state
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
        .count();
    state.report.summary_bytes_estimate = state.report.fields.iter().fold(0usize, |sum, field| {
        sum.saturating_add(field.summary_bytes_estimate)
    });
    state.report.index_bytes_estimate = state.report.fields.iter().fold(0usize, |sum, field| {
        sum.saturating_add(field.index_bytes_estimate)
    });
    state.report.retained_bytes_estimate = state
        .report
        .summary_bytes_estimate
        .saturating_add(state.report.index_bytes_estimate);
    Ok(())
}
