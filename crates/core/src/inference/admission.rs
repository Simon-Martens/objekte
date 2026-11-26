//! Shared inference admission and collection-wide observation aggregation.

use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use roaring::RoaringBitmap;

use crate::index::storage::inference::{CompactFieldObservation, InferredPostingDiagnostic};
use crate::index::storage::publication::PublishedSegment;
use crate::index::storage::segment::{SegmentReader, SegmentSlotFact, read_slot_members};
use crate::index::storage::transfer::StorageResult;
use crate::model::{LogicalType, Number};
use crate::repository::state::GlobalInferenceSummary;
use crate::schema::Schema;

use super::{
    InferenceConfig, InferenceIndexStatus, InferenceMode, InferencePathDiagnostic, InferencePolicy,
    InferenceReport, InferredFieldReport, OccurrenceCountStatus,
};

pub(crate) struct FieldAdmissionFacts<'a> {
    pub(crate) path: &'a [String],
    pub(crate) complete: bool,
    pub(crate) kinds: &'a [LogicalType],
    pub(crate) scalar_records: u64,
    pub(crate) array_records: u64,
    pub(crate) object_records: u64,
    pub(crate) through_array_records: u64,
    pub(crate) direct_records: u64,
    pub(crate) nonblank_occurrences: u64,
    pub(crate) nonblank_records: u64,
    pub(crate) distinct_nonblank: Option<usize>,
    pub(crate) has_short_values: bool,
    pub(crate) has_lengths: bool,
    pub(crate) length_dropped: bool,
    pub(crate) short_dropped: bool,
    pub(crate) short_seen_records: u64,
    pub(crate) dropped: bool,
    pub(crate) has_values: bool,
    pub(crate) short_summary_truncated: bool,
}

pub(crate) struct FieldAdmissionDecision {
    pub(crate) status: InferenceIndexStatus,
    pub(crate) reason: &'static str,
    pub(crate) scalar_kind: Option<LogicalType>,
    pub(crate) mixed_shape: bool,
    pub(crate) string_admissible: bool,
    pub(crate) has_short_values: bool,
}

/// One policy decision for both a single-domain builder and collection-wide
/// observations. Candidate construction supplies the facts; this function is
/// the sole place that interprets semantic thresholds and admission order.
pub(crate) fn decide_field_admission(
    config: InferenceConfig,
    schema: &Schema,
    facts: FieldAdmissionFacts<'_>,
) -> FieldAdmissionDecision {
    let declared = schema.fields.get(facts.path);
    let scalar_kind = (facts.kinds.len() == 1).then(|| facts.kinds[0].clone());
    let mixed_shape = (facts.scalar_records != 0 && facts.array_records != 0)
        || (facts.direct_records != 0 && facts.through_array_records != 0)
        || facts.object_records != 0;
    let string_admissible = scalar_kind.as_ref() != Some(&LogicalType::String)
        || facts.distinct_nonblank.is_some_and(|distinct| {
            facts.nonblank_occurrences >= config.policy.min_nonblank_occurrences
                && facts.nonblank_records >= config.policy.min_nonblank_records
                && distinct <= config.policy.max_distinct_nonblank_strings
                && distinct <= (facts.nonblank_occurrences / 10).max(4) as usize
                && facts
                    .nonblank_occurrences
                    .saturating_sub(distinct as u64)
                    .saturating_mul(100)
                    >= facts.nonblank_occurrences * config.policy.minimum_repeated_percent as u64
        });
    let (status, reason) = if !facts.complete {
        (
            InferenceIndexStatus::Incomplete,
            "observation budget exceeded",
        )
    } else if config.mode == InferenceMode::ReportOnly {
        (InferenceIndexStatus::NotRequested, "report-only mode")
    } else if declared
        .is_some_and(|field| field.indexed || matches!(field.kind, LogicalType::Array(_)))
    {
        (
            InferenceIndexStatus::Redundant,
            "declared posting already exists",
        )
    } else if schema.is_auto_index_prohibited(facts.path) {
        (
            InferenceIndexStatus::ExplicitOptOut,
            "declared field disabled indexing",
        )
    } else if facts.has_short_values {
        (
            InferenceIndexStatus::Built,
            "ordered short-string postings with residual coverage",
        )
    } else if facts.array_records != 0 && facts.has_lengths {
        (
            InferenceIndexStatus::Built,
            "complete array-length postings",
        )
    } else if scalar_kind == Some(LogicalType::String) && !mixed_shape {
        (
            InferenceIndexStatus::Built,
            "complete exact empty-string posting",
        )
    } else if facts.length_dropped {
        (
            InferenceIndexStatus::BudgetExceeded,
            "array-length postings exceeded the per-path key budget",
        )
    } else if facts.short_dropped && facts.short_seen_records != 0 {
        (
            InferenceIndexStatus::BudgetExceeded,
            "short-string index exceeded its per-path memory budget",
        )
    } else if facts.dropped || !facts.has_values {
        (
            InferenceIndexStatus::BudgetExceeded,
            "provisional values exceeded a bound",
        )
    } else if mixed_shape || scalar_kind.is_none() {
        (
            InferenceIndexStatus::Incompatible,
            "mixed or unknown observed shape/kind",
        )
    } else if !matches!(
        scalar_kind,
        Some(LogicalType::Bool | LogicalType::Number | LogicalType::String)
    ) {
        (
            InferenceIndexStatus::Incompatible,
            "kind is not an inferred posting target",
        )
    } else if !string_admissible {
        (
            InferenceIndexStatus::BelowThreshold,
            "string repetition threshold not met",
        )
    } else {
        (
            InferenceIndexStatus::Built,
            "complete compatible logical coverage",
        )
    };
    let reason = if facts.short_summary_truncated {
        match status {
            InferenceIndexStatus::Redundant => {
                "declared posting already exists; short-string distinct summary exceeded a bound"
            }
            InferenceIndexStatus::ExplicitOptOut => {
                "declared field disabled indexing; short-string distinct summary exceeded a bound"
            }
            _ => reason,
        }
    } else {
        reason
    };
    FieldAdmissionDecision {
        status,
        reason,
        scalar_kind,
        mixed_shape,
        string_admissible,
        has_short_values: facts.has_short_values,
    }
}

/// Segment-local builders retain bounded candidates. Collection-wide policy is
/// applied once after all observations are known, so a per-segment occurrence
/// threshold must not discard a field that qualifies globally.
pub(crate) fn candidate_config(mut config: InferenceConfig) -> InferenceConfig {
    config.policy.min_nonblank_occurrences = 1;
    config.policy.min_nonblank_records = 1;
    config.policy.minimum_repeated_percent = 0;
    config
}

pub(crate) fn aggregate_segment_inference(
    config: InferenceConfig,
    schema: &Schema,
    segments: &[Arc<PublishedSegment>],
) -> Result<Option<GlobalInferenceSummary>, String> {
    if config.mode == InferenceMode::Disabled {
        return Ok(None);
    }
    let total_records = segments
        .iter()
        .map(|segment| segment.descriptor().slots)
        .try_fold(0usize, |total, count| total.checked_add(count))
        .ok_or("inference collection record count overflow")?;
    let counts = segments
        .iter()
        .map(|segment| {
            read_slot_members(segment, SegmentSlotFact::Logical)
                .map(|members| members.view().len() as u64)
        })
        .collect::<StorageResult<Vec<_>>>()?;
    let logical_records = counts
        .iter()
        .copied()
        .try_fold(0u64, |total, count| total.checked_add(count))
        .ok_or("inference collection logical count overflow")?;
    let unavailable_records = u64::try_from(total_records)
        .map_err(|_| "inference collection record count exceeds u64")?
        .checked_sub(logical_records)
        .ok_or("inference unavailable count underflow")?;
    let omitted_paths = segments
        .iter()
        .filter_map(|segment| segment.observation.as_deref())
        .map(|observation| observation.omitted_paths())
        .try_fold(0u64, |total, count| total.checked_add(count))
        .ok_or("inference omitted path count overflow")?;

    let mut paths = BTreeSet::<Vec<String>>::new();
    let mut complete = true;
    for segment in segments {
        let Some(observation) = segment.observation.as_deref() else {
            complete = false;
            continue;
        };
        complete &= observation.complete();
        observation.visit_paths(&mut |batch| {
            paths.extend(batch.iter().map(|path| path.to_vec()));
            Ok(())
        })?;
    }

    let mut fields = Vec::with_capacity(paths.len());
    let mut diagnostics = Vec::with_capacity(paths.len());
    for path in paths {
        let mut aggregate = AggregatedField::new();
        for (segment, &eligible) in segments.iter().zip(&counts) {
            add_count(
                &mut aggregate.eligible_records,
                eligible,
                "inference eligible record count overflow",
            )?;
            let Some(observation) = segment.observation.as_deref() else {
                aggregate.complete = false;
                continue;
            };
            aggregate.complete &= observation.complete();
            if let Some(field) = observation.field(&path)? {
                aggregate.add_observation(&field, config.policy)?;
            } else {
                let mut empty = RoaringBitmap::new();
                for depth in 0..path.len() {
                    let stored = observation.empty_ancestor_members(&path[..depth])?;
                    let slots = stored
                        .explicit_view()
                        .ok_or("missing explicit ancestor membership")?;
                    slots.union_into_bitmap(&mut empty);
                }
                let empty = if empty.is_empty() {
                    0
                } else {
                    let logical = read_slot_members(segment, SegmentSlotFact::Logical)?;
                    logical.view().intersection_len(
                        crate::query_access::membership::MembershipView::Borrowed(&empty),
                    )
                };
                add_count(
                    &mut aggregate.present_records,
                    empty,
                    "inference present record count overflow",
                )?;
                add_count(
                    &mut aggregate.empty_selection_records,
                    empty,
                    "inference empty selection count overflow",
                )?;
                aggregate.short_domains_complete &= empty == 0;
            }
            if let Some(index) = segment.inferred_posting_diagnostic(&path)? {
                aggregate.index_bytes_estimate = aggregate
                    .index_bytes_estimate
                    .saturating_add(index.estimated_bytes);
                aggregate.structural_facts &= index.has_structural_facts;
                aggregate.value_keys += index.value_keys;
                aggregate.length_keys_count += index.length_keys;
                aggregate.value_dictionaries_complete &= index.value_complete;
                aggregate.first_index.get_or_insert(index);
            }
        }

        let distinct_strings = aggregate.distinct_nonblank.as_ref().map(HashSet::len);
        let length_key_overflow = aggregate.length_keys_overflow
            || aggregate.length_keys.len() > config.policy.max_distinct_values_per_path;
        let decision = decide_field_admission(
            config,
            schema,
            FieldAdmissionFacts {
                path: &path,
                complete,
                kinds: &aggregate.kinds,
                scalar_records: aggregate.scalar_records,
                array_records: aggregate.array_records,
                object_records: aggregate.object_records,
                through_array_records: aggregate.through_array_records,
                direct_records: aggregate.direct_records,
                nonblank_occurrences: aggregate.nonblank_occurrences,
                nonblank_records: aggregate.nonblank_records,
                distinct_nonblank: distinct_strings,
                has_short_values: aggregate.short_values_present,
                has_lengths: aggregate.array_records != 0
                    && aggregate.lengths_complete
                    && !length_key_overflow,
                length_dropped: aggregate.length_dropped || length_key_overflow,
                short_dropped: aggregate.short_dropped,
                short_seen_records: aggregate.short_seen_records,
                dropped: aggregate.dropped,
                has_values: aggregate.values_present,
                short_summary_truncated: aggregate.short_summary_truncated,
            },
        );
        let short_string_covered_records = if decision.status == InferenceIndexStatus::Built {
            aggregate.short_string_covered_records
        } else {
            0
        };
        let short_string_unresolved_records = aggregate
            .eligible_records
            .checked_sub(short_string_covered_records)
            .ok_or("inference short-string coverage underflow")?;
        let present_records = aggregate.present_records.min(aggregate.eligible_records);
        let missing_records = aggregate
            .eligible_records
            .checked_sub(present_records)
            .ok_or("inference missing record count underflow")?;
        let summary_bytes_estimate = 256usize.saturating_add(
            path.iter().map(String::len).sum::<usize>().saturating_add(
                usize::try_from(present_records)
                    .unwrap_or(usize::MAX)
                    .saturating_div(8),
            ),
        );
        let integer_only = aggregate
            .kinds
            .contains(&LogicalType::Number)
            .then_some(!aggregate.fractional_number);
        let short_string_distinct_values = aggregate.short_string_distinct_values();
        diagnostics.push(InferencePathDiagnostic {
            path: path.clone(),
            index_status: decision.status,
            reason: decision.reason.into(),
            observed_kinds: aggregate.kinds.clone(),
            kind: aggregate
                .first_index
                .as_ref()
                .map(|index| index.kind.clone()),
            multi: aggregate.first_index.as_ref().map(|index| index.multi),
            value_keys: Some(aggregate.value_keys),
            length_keys: Some(aggregate.length_keys_count),
            covered_records: Some(
                aggregate
                    .first_index
                    .as_ref()
                    .map_or(0, |index| index.covered_records),
            ),
            present_records,
            value_dictionary_complete: decision.status == InferenceIndexStatus::Built
                && (aggregate.values_present || aggregate.short_values_present)
                && decision.scalar_kind.is_some()
                && !decision.mixed_shape
                && (decision.string_admissible || aggregate.short_dictionary_complete())
                && aggregate.first_index.is_some()
                && aggregate.value_dictionaries_complete,
            short_string_candidates: decision.status == InferenceIndexStatus::Built
                && aggregate.short_postings_present
                && short_string_covered_records != 0,
            empty_string_exact: aggregate.empty_string_exact
                && decision.scalar_kind == Some(LogicalType::String)
                && aggregate.array_records == 0
                && aggregate.through_array_records == 0
                && !decision.mixed_shape,
            array_length_complete: aggregate.array_records != 0
                && aggregate.lengths_complete
                && !length_key_overflow
                && aggregate.array_length_complete,
            structural_facts: aggregate.first_index.is_some() && aggregate.structural_facts,
            short_string_covered_records,
            short_string_unresolved_records,
        });
        fields.push(InferredFieldReport {
            path,
            observed_kinds: aggregate.kinds,
            scalar_records: aggregate.scalar_records,
            array_records: aggregate.array_records,
            object_records: aggregate.object_records,
            array_nesting: aggregate.array_nesting,
            integer_only,
            eligible_records: aggregate.eligible_records,
            unavailable_records,
            present_records,
            missing_records,
            null_occurrence_records: aggregate.null_occurrence_records,
            exact_null_records: aggregate.exact_null_records,
            empty_array_records: aggregate.empty_array_records,
            empty_selection_records: aggregate.empty_selection_records,
            value_occurrences: aggregate.value_occurrences,
            nonblank_occurrences: aggregate.nonblank_occurrences,
            nonblank_records: aggregate.nonblank_records,
            distinct_nonblank_strings: (decision.scalar_kind == Some(LogicalType::String))
                .then_some(distinct_strings)
                .flatten(),
            distinct_nonblank_strings_truncated: aggregate.distinct_nonblank_truncated,
            short_string_observed_occurrences: aggregate.short_string_observed_occurrences,
            occurrence_count_status: if complete {
                aggregate.occurrence_count_status
            } else {
                OccurrenceCountStatus::IncompleteObservation
            },
            short_string_distinct_values,
            short_string_distinct_values_truncated: aggregate.short_distinct_truncated,
            short_string_covered_records,
            short_string_unresolved_records,
            index_status: decision.status,
            reason: decision.reason.into(),
            complete: complete && aggregate.complete,
            summary_bytes_estimate,
            index_bytes_estimate: aggregate.index_bytes_estimate,
            retained_bytes_estimate: summary_bytes_estimate
                .saturating_add(aggregate.index_bytes_estimate),
        });
    }
    fields.sort_by(|left, right| left.path.cmp(&right.path));
    diagnostics.sort_by(|left, right| left.path.cmp(&right.path));
    let report = InferenceReport {
        mode: config.mode,
        policy: config.policy,
        eligible_records: logical_records,
        unavailable_records,
        omitted_paths,
        complete,
        observed_paths: fields.len(),
        admitted_paths: fields
            .iter()
            .filter(|field| field.index_status == InferenceIndexStatus::Built)
            .count(),
        dropped_paths: fields
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
            .count(),
        summary_bytes_estimate: fields
            .iter()
            .map(|field| field.summary_bytes_estimate)
            .sum(),
        index_bytes_estimate: fields.iter().map(|field| field.index_bytes_estimate).sum(),
        retained_bytes_estimate: fields
            .iter()
            .map(|field| field.retained_bytes_estimate)
            .sum(),
        fields,
    };
    Ok(Some(GlobalInferenceSummary {
        report,
        diagnostics,
    }))
}

fn add_count(target: &mut u64, value: u64, message: &'static str) -> Result<(), String> {
    *target = target.checked_add(value).ok_or(message)?;
    Ok(())
}

pub(crate) fn weakest_occurrence_status(
    left: OccurrenceCountStatus,
    right: OccurrenceCountStatus,
) -> OccurrenceCountStatus {
    match (left, right) {
        (OccurrenceCountStatus::IncompleteObservation, _)
        | (_, OccurrenceCountStatus::IncompleteObservation) => {
            OccurrenceCountStatus::IncompleteObservation
        }
        (OccurrenceCountStatus::UpperBound, _) | (_, OccurrenceCountStatus::UpperBound) => {
            OccurrenceCountStatus::UpperBound
        }
        _ => OccurrenceCountStatus::Exact,
    }
}

#[derive(Default)]
struct AggregatedField {
    eligible_records: u64,
    kinds: Vec<LogicalType>,
    array_nesting: u8,
    fractional_number: bool,
    complete: bool,
    present_records: u64,
    null_occurrence_records: u64,
    exact_null_records: u64,
    empty_array_records: u64,
    empty_selection_records: u64,
    value_occurrences: u64,
    nonblank_occurrences: u64,
    nonblank_records: u64,
    scalar_records: u64,
    array_records: u64,
    object_records: u64,
    through_array_records: u64,
    direct_records: u64,
    short_string_observed_occurrences: u64,
    occurrence_count_status: OccurrenceCountStatus,
    short_string_covered_records: u64,
    short_seen_records: u64,
    short_values_present: bool,
    short_postings_present: bool,
    short_domains_complete: bool,
    short_dropped: bool,
    short_summary_truncated: bool,
    lengths_complete: bool,
    length_dropped: bool,
    length_keys: BTreeSet<Number>,
    length_keys_overflow: bool,
    values_present: bool,
    dropped: bool,
    index_bytes_estimate: usize,
    distinct_nonblank: Option<HashSet<String>>,
    distinct_nonblank_truncated: bool,
    short_distinct: Option<HashSet<String>>,
    short_distinct_lower_bound: usize,
    short_distinct_truncated: bool,
    structural_facts: bool,
    empty_string_exact: bool,
    array_length_complete: bool,
    first_index: Option<InferredPostingDiagnostic>,
    value_keys: usize,
    length_keys_count: usize,
    value_dictionaries_complete: bool,
}

impl AggregatedField {
    fn new() -> Self {
        Self {
            complete: true,
            lengths_complete: true,
            values_present: true,
            distinct_nonblank: Some(HashSet::new()),
            short_distinct: Some(HashSet::new()),
            short_domains_complete: true,
            structural_facts: true,
            value_dictionaries_complete: true,
            empty_string_exact: true,
            array_length_complete: true,
            occurrence_count_status: OccurrenceCountStatus::Exact,
            ..Default::default()
        }
    }

    fn add_observation(
        &mut self,
        field: &CompactFieldObservation,
        policy: InferencePolicy,
    ) -> Result<(), String> {
        self.occurrence_count_status =
            weakest_occurrence_status(self.occurrence_count_status, field.occurrence_count_status);
        for kind in &field.kinds {
            if !self.kinds.contains(kind) {
                self.kinds.push(kind.clone());
            }
        }
        self.array_nesting = self.array_nesting.max(field.array_nesting);
        self.fractional_number |= field.fractional_number;
        self.dropped |= field.dropped;
        self.short_dropped |= field.short_dropped;
        self.length_dropped |= field.length_dropped;
        if !self.length_keys_overflow {
            for key in &field.length_keys {
                if self.length_keys.contains(key) {
                    continue;
                }
                if self.length_keys.len() >= policy.max_distinct_values_per_path {
                    self.length_keys_overflow = true;
                    break;
                }
                self.length_keys.insert(key.clone());
            }
        }
        self.short_summary_truncated |= field.short_summary_truncated;
        self.values_present &= field.values_present;
        if field.array_records != 0 {
            self.lengths_complete &= field.lengths_present;
        }
        if field.array_records != 0 && !field.lengths_present {
            self.array_length_complete = false;
        }
        if field.array_records != 0 || field.through_array_records != 0 || field.object_records != 0
        {
            self.empty_string_exact = false;
        }
        let has_short_values = field.short_values_present;
        self.short_values_present |= has_short_values;
        self.short_postings_present |= field.short_postings_present;
        self.short_domains_complete &= field.short_domains_complete;
        add_count(
            &mut self.present_records,
            field.present_records,
            "inference present record count overflow",
        )?;
        add_count(
            &mut self.null_occurrence_records,
            field.null_occurrence_records,
            "inference null count overflow",
        )?;
        add_count(
            &mut self.exact_null_records,
            field.exact_null_records,
            "inference exact-null count overflow",
        )?;
        add_count(
            &mut self.empty_array_records,
            field.empty_array_records,
            "inference empty-array count overflow",
        )?;
        add_count(
            &mut self.empty_selection_records,
            field.empty_selection_records,
            "inference empty-selection count overflow",
        )?;
        add_count(
            &mut self.value_occurrences,
            field.value_occurrences,
            "inference value occurrence overflow",
        )?;
        add_count(
            &mut self.nonblank_occurrences,
            field.nonblank_occurrences,
            "inference nonblank occurrence overflow",
        )?;
        add_count(
            &mut self.nonblank_records,
            field.nonblank_records,
            "inference nonblank record count overflow",
        )?;
        add_count(
            &mut self.scalar_records,
            field.scalar_records,
            "inference scalar record count overflow",
        )?;
        add_count(
            &mut self.array_records,
            field.array_records,
            "inference array record count overflow",
        )?;
        add_count(
            &mut self.object_records,
            field.object_records,
            "inference object record count overflow",
        )?;
        add_count(
            &mut self.through_array_records,
            field.through_array_records,
            "inference through-array record count overflow",
        )?;
        add_count(
            &mut self.direct_records,
            field.direct_records,
            "inference direct record count overflow",
        )?;
        add_count(
            &mut self.short_string_observed_occurrences,
            field.short_string_observed_occurrences,
            "inference short-string occurrence overflow",
        )?;
        add_count(
            &mut self.short_string_covered_records,
            field.short_string_covered_records,
            "inference short-string coverage overflow",
        )?;
        add_count(
            &mut self.short_seen_records,
            field.short_seen_records,
            "inference short-string record count overflow",
        )?;
        if let Some(values) = &field.short_distinct {
            self.merge_short_distinct(values.iter().cloned(), policy);
        }
        if let Some(values) = &field.distinct_nonblank {
            self.merge_distinct_nonblank(values.iter().cloned(), policy);
        } else {
            self.distinct_nonblank = None;
            self.distinct_nonblank_truncated = true;
        }
        let has_exact_field_summary = field.summary_complete;
        self.complete &= has_exact_field_summary;
        Ok(())
    }

    fn merge_distinct_nonblank(
        &mut self,
        values: impl IntoIterator<Item = String>,
        policy: InferencePolicy,
    ) {
        let Some(existing) = &mut self.distinct_nonblank else {
            return;
        };
        for value in values {
            if existing.insert(value) && existing.len() > policy.max_distinct_nonblank_strings {
                self.distinct_nonblank_truncated = true;
                self.distinct_nonblank = None;
                return;
            }
        }
    }

    fn merge_short_distinct(
        &mut self,
        values: impl IntoIterator<Item = String>,
        policy: InferencePolicy,
    ) {
        let Some(existing) = &mut self.short_distinct else {
            return;
        };
        for value in values {
            if existing.insert(value) && existing.len() > policy.max_distinct_nonblank_strings {
                self.short_distinct_lower_bound = existing.len();
                self.short_distinct_truncated = true;
                self.short_distinct = None;
                return;
            }
        }
        self.short_distinct_lower_bound = existing.len();
    }

    fn short_string_distinct_values(&self) -> usize {
        self.short_distinct
            .as_ref()
            .map_or(self.short_distinct_lower_bound, HashSet::len)
    }

    fn short_dictionary_complete(&self) -> bool {
        self.short_postings_present && self.short_domains_complete
    }
}
