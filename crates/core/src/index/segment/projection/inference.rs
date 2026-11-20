use std::collections::HashMap;

use roaring::RoaringBitmap;

use super::super::IndexedCollectionSegment;
use super::DomainProjection;
use super::postings::{project_posting, scatter_posting_partitions};
use super::routing::{
    PartitionRouting, SlotMap, project_bitmap_in_range, scatter_bitmap_partitions,
};
use crate::inference::{InferenceIndexStatus, InferenceState, OccurrenceCountStatus};

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
                if !scattered.is_empty() {
                    part.short_string_coverage.insert(path.clone(), scattered);
                }
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
        .filter_map(|(path, bitmap)| {
            project_bitmap_in_range(bitmap, slots, bounds)
                .map(|bitmap| (!bitmap.is_empty()).then_some((path.clone(), bitmap)))
                .transpose()
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
    let source_domain_complete = state.report.complete;
    for field in &mut state.report.fields {
        field.eligible_records = eligible;
        field.unavailable_records = unavailable;
        field.distinct_nonblank_strings = None;
        let old_present_records = field.present_records;
        let old_scalar_records = field.scalar_records;
        let old_array_records = field.array_records;
        let old_object_records = field.object_records;
        let old_empty_array_records = field.empty_array_records;
        let old_empty_selection_records = field.empty_selection_records;
        let old_null_occurrence_records = field.null_occurrence_records;
        let old_exact_null_records = field.exact_null_records;
        let old_nonblank_records = field.nonblank_records;
        let old_value_occurrences = field.value_occurrences;
        let old_nonblank_occurrences = field.nonblank_occurrences;
        let old_short_string_observed_occurrences = field.short_string_observed_occurrences;
        let incoming_occurrence_status = field.occurrence_count_status;
        if let Some(index) = state.indexes.get(&field.path) {
            field.index_status = InferenceIndexStatus::Built;
            let terminal = index.summary;
            let terminal_records = index.presence.len();
            let nulls = terminal.map(|_| {
                index
                    .values
                    .get(&crate::index::posting::ScalarKey::Null)
                    .map_or(0, RoaringBitmap::len)
            });
            let terminal_empty = index.length_complete.then(|| {
                index
                    .lengths
                    .get(&crate::model::Number::integer(0))
                    .map_or(0, RoaringBitmap::len)
            });
            let exact_scalar = terminal.is_some_and(|summary| {
                !summary.may_match_array()
                    && !summary.may_select_multiple()
                    && !summary.may_have_empty_traversal()
            }) && index.value_complete
                && index.kind != crate::model::LogicalType::String;
            if exact_scalar {
                let nulls = nulls.unwrap_or(0);
                field.present_records = terminal_records;
                field.null_occurrence_records = nulls;
                field.exact_null_records = nulls;
                field.empty_array_records = 0;
                field.empty_selection_records = 0;
                field.scalar_records = terminal_records;
                field.array_records = 0;
                field.object_records = 0;
                field.integer_only = if index.kind == crate::model::LogicalType::Number {
                    let fractional = index.values.keys().any(|key| {
                        matches!(
                            key,
                            crate::index::posting::ScalarKey::Number(crate::model::Number::Float(
                                _
                            ))
                        )
                    });
                    if fractional {
                        Some(false)
                    } else if terminal_records > nulls {
                        Some(true)
                    } else {
                        None
                    }
                } else {
                    None
                };
                field.value_occurrences = terminal_records.saturating_sub(nulls);
                field.nonblank_occurrences = 0;
                field.nonblank_records = 0;
                field.short_string_observed_occurrences = 0;
                field.occurrence_count_status = OccurrenceCountStatus::Exact;
            } else {
                field.complete = false;
                let inherited = |old: u64, bound: u64| {
                    if source_domain_complete {
                        old.min(bound)
                    } else {
                        bound
                    }
                };
                let present = if source_domain_complete
                    && terminal.is_some_and(|summary| !summary.may_have_empty_traversal())
                {
                    terminal_records
                } else {
                    inherited(old_present_records, eligible).max(terminal_records)
                };
                let array_records = inherited(old_array_records, present);
                let empty_array_records = terminal_empty
                    .filter(|_| source_domain_complete)
                    .unwrap_or_else(|| {
                        if source_domain_complete {
                            old_empty_array_records.min(array_records)
                        } else {
                            array_records
                        }
                    });
                let empty_selection_records = if source_domain_complete
                    && terminal.is_some_and(|summary| !summary.may_have_empty_traversal())
                {
                    empty_array_records
                } else {
                    inherited(old_empty_selection_records, present)
                };
                let null_occurrence_records = if source_domain_complete
                    && terminal.is_some_and(|summary| !summary.may_match_array())
                {
                    nulls.unwrap_or(0)
                } else {
                    inherited(old_null_occurrence_records, present)
                };
                let exact_null_records = if source_domain_complete {
                    match (terminal, nulls) {
                        (Some(summary), Some(nulls)) if !summary.may_select_multiple() => nulls,
                        (Some(_), Some(nulls)) => old_exact_null_records
                            .min(nulls)
                            .min(null_occurrence_records),
                        _ => old_exact_null_records.min(null_occurrence_records),
                    }
                } else {
                    nulls.unwrap_or(present).min(null_occurrence_records)
                };
                field.present_records = present;
                field.scalar_records = inherited(old_scalar_records, present);
                field.array_records = array_records;
                field.object_records = inherited(old_object_records, present);
                field.empty_array_records = empty_array_records;
                field.empty_selection_records = empty_selection_records;
                field.null_occurrence_records = null_occurrence_records;
                field.exact_null_records = exact_null_records;
                field.nonblank_records = inherited(old_nonblank_records, present);
                if present == 0 {
                    field.value_occurrences = 0;
                    field.nonblank_occurrences = 0;
                    field.short_string_observed_occurrences = 0;
                } else {
                    field.value_occurrences = old_value_occurrences;
                    field.nonblank_occurrences = old_nonblank_occurrences;
                    field.short_string_observed_occurrences = old_short_string_observed_occurrences;
                }
                field.integer_only = None;
                field.occurrence_count_status = if eligible == 0 || present == 0 {
                    OccurrenceCountStatus::Exact
                } else {
                    match incoming_occurrence_status {
                        OccurrenceCountStatus::Exact | OccurrenceCountStatus::UpperBound => {
                            OccurrenceCountStatus::UpperBound
                        }
                        OccurrenceCountStatus::IncompleteObservation => {
                            OccurrenceCountStatus::IncompleteObservation
                        }
                    }
                };
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
        } else {
            field.index_status = InferenceIndexStatus::Incomplete;
            field.complete = false;
            field.reason = "retired records removed from incomplete inference summary".into();
            let inherited = |old: u64, bound: u64| {
                if source_domain_complete {
                    old.min(bound)
                } else {
                    bound
                }
            };
            let present = inherited(old_present_records, eligible);
            let array_records = inherited(old_array_records, present);
            let empty_array_records = if source_domain_complete {
                old_empty_array_records.min(array_records)
            } else {
                array_records
            };
            field.present_records = present;
            field.scalar_records = inherited(old_scalar_records, present);
            field.array_records = array_records;
            field.object_records = inherited(old_object_records, present);
            field.empty_array_records = empty_array_records;
            field.empty_selection_records = inherited(old_empty_selection_records, present);
            field.null_occurrence_records = inherited(old_null_occurrence_records, present);
            field.exact_null_records =
                inherited(old_exact_null_records, field.null_occurrence_records);
            field.nonblank_records = inherited(old_nonblank_records, present);
            if present == 0 {
                field.value_occurrences = 0;
                field.nonblank_occurrences = 0;
                field.short_string_observed_occurrences = 0;
            } else {
                field.value_occurrences = old_value_occurrences;
                field.nonblank_occurrences = old_nonblank_occurrences;
                field.short_string_observed_occurrences = old_short_string_observed_occurrences;
            }
            field.short_string_covered_records = 0;
            field.short_string_distinct_values = 0;
            field.integer_only = None;
            field.index_bytes_estimate = 0;
            field.occurrence_count_status = if eligible == 0 || present == 0 {
                OccurrenceCountStatus::Exact
            } else {
                match incoming_occurrence_status {
                    OccurrenceCountStatus::Exact | OccurrenceCountStatus::UpperBound => {
                        OccurrenceCountStatus::UpperBound
                    }
                    OccurrenceCountStatus::IncompleteObservation => {
                        OccurrenceCountStatus::IncompleteObservation
                    }
                }
            };
        }
        field.missing_records = eligible - field.present_records;
        field.short_string_unresolved_records = eligible - field.short_string_covered_records;
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
