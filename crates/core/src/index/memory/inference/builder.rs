//! Bounded construction-only field observation and posting admission.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use roaring::RoaringBitmap;

use super::InferenceState;
use crate::index::posting::{PostingIndex, PostingStats, ScalarKey, SelectionSummary};
use crate::index::storage::inference::{CompactFieldObservation, InferenceObservationReader};
use crate::index::storage::segment::{SegmentReader, SegmentSlotFact, read_slot_members};
use crate::index::storage::transfer::StorageResult;
use crate::inference::{
    InferenceBuildStats, InferenceConfig, InferenceIndexStatus, InferenceMode,
    InferencePathDiagnostic, InferencePolicy, InferenceReport, InferredFieldReport,
    OccurrenceCountStatus,
};
use crate::inventory::append_shifted_bitmap;
use crate::model::{LogicalType, Number, RecordSlot, Value};
use crate::query_access::membership::SlotSet;
use crate::schema::Schema;

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

/// Compact segment-local observations retained independently from candidate
/// postings. Global admission is derived from these observations, never from
/// the surviving candidate dictionaries.
#[derive(Clone)]
pub(crate) struct InferenceObservation {
    fields: Vec<CompactFieldObservation>,
    empty_ancestors: HashMap<Vec<ObservedStep>, RoaringBitmap>,
    complete: bool,
    omitted_paths: u64,
    estimated_bytes: usize,
}

impl InferenceObservationReader for InferenceObservation {
    fn estimated_bytes(&self) -> usize {
        self.estimated_bytes
    }

    fn complete(&self) -> bool {
        self.complete
    }

    fn omitted_paths(&self) -> u64 {
        self.omitted_paths
    }

    fn visit_paths(
        &self,
        visit: &mut dyn FnMut(&[&[String]]) -> StorageResult<()>,
    ) -> StorageResult<()> {
        for fields in self.fields.chunks(64) {
            let paths = fields
                .iter()
                .map(|field| field.path.as_slice())
                .collect::<smallvec::SmallVec<[_; 64]>>();
            visit(&paths)?;
        }
        Ok(())
    }

    fn field(
        &self,
        path: &[String],
    ) -> StorageResult<Option<std::borrow::Cow<'_, CompactFieldObservation>>> {
        Ok(self
            .fields
            .binary_search_by(|field| field.path.as_slice().cmp(path))
            .ok()
            .map(|index| std::borrow::Cow::Borrowed(&self.fields[index])))
    }

    fn empty_ancestor_members(&self, prefix: &[String]) -> StorageResult<SlotSet<'_>> {
        Ok(self
            .empty_ancestors
            .get(&typed_path(prefix))
            .map_or(SlotSet::Empty, SlotSet::borrowed))
    }
}

struct FieldAdmissionFacts<'a> {
    path: &'a [String],
    complete: bool,
    kinds: &'a [LogicalType],
    scalar_records: u64,
    array_records: u64,
    object_records: u64,
    through_array_records: u64,
    direct_records: u64,
    nonblank_occurrences: u64,
    nonblank_records: u64,
    distinct_nonblank: Option<usize>,
    has_short_values: bool,
    has_lengths: bool,
    length_dropped: bool,
    short_dropped: bool,
    short_seen_records: u64,
    dropped: bool,
    has_values: bool,
    short_summary_truncated: bool,
}

struct FieldAdmissionDecision {
    status: InferenceIndexStatus,
    reason: &'static str,
    scalar_kind: Option<LogicalType>,
    mixed_shape: bool,
    string_admissible: bool,
    has_short_values: bool,
}

fn short_dictionary_covers_required_domain(
    retained_short_postings: bool,
    present: &RoaringBitmap,
    exact_null: &RoaringBitmap,
    short_seen: &RoaringBitmap,
    short_uncovered: &RoaringBitmap,
) -> bool {
    let required = present - exact_null;
    required.is_empty() || (retained_short_postings && short_seen - short_uncovered == required)
}

/// One policy decision for both a single-domain builder and collection-wide
/// observations. Candidate construction supplies the facts; this function is
/// the sole place that interprets semantic thresholds and admission order.
fn decide_field_admission(
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
    segments: &[Arc<crate::index::segment::PublishedSegment>],
    collection_name: &str,
) -> Result<Option<crate::repository::state::GlobalInferenceSummary>, String> {
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

    let mut paths = std::collections::BTreeSet::<Vec<String>>::new();
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
            if let Some(inference) = &segment.index.inference
                && let Some(index) = inference.indexes.get(&path)
            {
                aggregate.indexes.push(index);
                aggregate.index_bytes_estimate = aggregate
                    .index_bytes_estimate
                    .saturating_add(index_byte_estimate(index));
                aggregate.structural_facts &= index.summary.is_some();
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
            kind: aggregate.indexes.first().map(|index| index.kind.clone()),
            multi: aggregate.indexes.first().map(|index| index.multi),
            value_keys: Some(
                aggregate
                    .indexes
                    .iter()
                    .map(|index| index.distinct_values())
                    .sum(),
            ),
            length_keys: Some(
                aggregate
                    .indexes
                    .iter()
                    .map(|index| index.distinct_lengths())
                    .sum(),
            ),
            covered_records: Some(
                aggregate
                    .indexes
                    .first()
                    .map_or(0, |index| index.covered_records),
            ),
            present_records,
            value_dictionary_complete: decision.status == InferenceIndexStatus::Built
                && (aggregate.values_present || aggregate.short_values_present)
                && decision.scalar_kind.is_some()
                && !decision.mixed_shape
                && (decision.string_admissible || aggregate.short_dictionary_complete())
                && !aggregate.indexes.is_empty()
                && aggregate.indexes.iter().all(|index| index.value_complete),
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
            structural_facts: !aggregate.indexes.is_empty() && aggregate.structural_facts,
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
    let _ = collection_name;
    Ok(Some(crate::repository::state::GlobalInferenceSummary {
        report,
        diagnostics,
    }))
}

fn add_count(target: &mut u64, value: u64, message: &'static str) -> Result<(), String> {
    *target = target.checked_add(value).ok_or(message)?;
    Ok(())
}

fn index_byte_estimate(index: &PostingIndex) -> usize {
    index
        .values
        .iter()
        .map(|(key, slots)| {
            128 + match key {
                ScalarKey::String(text) => text.len(),
                _ => 0,
            } + slots.serialized_size()
        })
        .sum::<usize>()
        + index
            .lengths
            .values()
            .map(|slots| 64 + slots.serialized_size())
            .sum::<usize>()
}

fn weakest_occurrence_status(
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
struct AggregatedField<'a> {
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
    indexes: Vec<&'a PostingIndex>,
}

impl<'a> AggregatedField<'a> {
    fn new() -> Self {
        Self {
            complete: true,
            lengths_complete: true,
            values_present: true,
            distinct_nonblank: Some(HashSet::new()),
            short_distinct: Some(HashSet::new()),
            short_domains_complete: true,
            structural_facts: true,
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

/// Observations retained for one path while construction batches are admitted.
#[derive(Clone)]
struct FieldState {
    path: Vec<String>,
    kinds: Vec<LogicalType>,
    summary: SelectionSummary,
    present: RoaringBitmap,
    null_occurrence: RoaringBitmap,
    selected_null: RoaringBitmap,
    exact_null: RoaringBitmap,
    empty_array: RoaringBitmap,
    empty_selection: RoaringBitmap,
    nonblank_records: RoaringBitmap,
    scalar_records: RoaringBitmap,
    array_records: RoaringBitmap,
    object_records: RoaringBitmap,
    through_array_records: RoaringBitmap,
    direct_records: RoaringBitmap,
    array_nesting: u8,
    fractional_number: bool,
    value_occurrences: u64,
    nonblank_occurrences: u64,
    distinct_nonblank: Option<std::collections::HashSet<String>>,
    distinct_nonblank_bytes: usize,
    values: Option<BTreeMap<ScalarKey, RoaringBitmap>>,
    short_values: Option<BTreeMap<String, RoaringBitmap>>,
    no_provisional_postings: bool,
    facts_enabled: bool,
    short_distinct_without_postings: Option<HashSet<String>>,
    short_distinct_without_postings_bytes: usize,
    short_summary_truncated: bool,
    short_seen: RoaringBitmap,
    short_uncovered: RoaringBitmap,
    short_observed_occurrences: u64,
    occurrence_count_status: OccurrenceCountStatus,
    short_estimated_bytes: usize,
    short_dropped: bool,
    lengths: Option<BTreeMap<Number, RoaringBitmap>>,
    empty_string: RoaringBitmap,
    length_dropped: bool,
    dropped: bool,
    #[cfg(test)]
    key_builds: usize,
}

impl FieldState {
    fn new(path: Vec<String>, mode: InferenceMode) -> Self {
        Self {
            path,
            kinds: Vec::new(),
            summary: SelectionSummary::default(),
            present: RoaringBitmap::new(),
            null_occurrence: RoaringBitmap::new(),
            selected_null: RoaringBitmap::new(),
            exact_null: RoaringBitmap::new(),
            empty_array: RoaringBitmap::new(),
            empty_selection: RoaringBitmap::new(),
            nonblank_records: RoaringBitmap::new(),
            scalar_records: RoaringBitmap::new(),
            array_records: RoaringBitmap::new(),
            object_records: RoaringBitmap::new(),
            through_array_records: RoaringBitmap::new(),
            direct_records: RoaringBitmap::new(),
            array_nesting: 0,
            fractional_number: false,
            value_occurrences: 0,
            nonblank_occurrences: 0,
            distinct_nonblank: Some(Default::default()),
            distinct_nonblank_bytes: 0,
            values: (mode == InferenceMode::AutoIndex).then(BTreeMap::new),
            short_values: (mode == InferenceMode::AutoIndex).then(BTreeMap::new),
            no_provisional_postings: false,
            facts_enabled: mode == InferenceMode::AutoIndex,
            short_distinct_without_postings: None,
            short_distinct_without_postings_bytes: 0,
            short_summary_truncated: false,
            short_seen: RoaringBitmap::new(),
            short_uncovered: RoaringBitmap::new(),
            short_observed_occurrences: 0,
            occurrence_count_status: OccurrenceCountStatus::IncompleteObservation,
            short_estimated_bytes: 0,
            short_dropped: false,
            lengths: (mode == InferenceMode::AutoIndex).then(BTreeMap::new),
            empty_string: RoaringBitmap::new(),
            length_dropped: false,
            dropped: false,
            #[cfg(test)]
            key_builds: 0,
        }
    }

    fn without_provisional_postings(path: Vec<String>) -> Self {
        let mut field = Self::new(path, InferenceMode::ReportOnly);
        field.no_provisional_postings = true;
        field.facts_enabled = false;
        field.short_distinct_without_postings = Some(HashSet::new());
        field
    }

    fn kind(&mut self, kind: LogicalType) {
        if !self.kinds.contains(&kind) {
            self.kinds.push(kind);
        }
    }

    fn drop_values(&mut self) {
        self.drop_general_values();
        self.selected_null.clear();
        self.lengths = None;
        self.length_dropped = true;
        self.empty_string.clear();
        self.short_values = None;
        self.short_distinct_without_postings = None;
        self.short_dropped = true;
    }

    fn drop_general_values(&mut self) {
        self.values = None;
        self.dropped = true;
    }

    fn add_scalar(&mut self, slot: RecordSlot, value: &Value, policy: InferencePolicy) -> usize {
        let mut retained = 8;
        let kind = match value {
            Value::Bool(_) => LogicalType::Bool,
            Value::Number(Number::Integer(_)) => LogicalType::Number,
            Value::Number(Number::Float(_)) => {
                self.fractional_number = true;
                LogicalType::Number
            }
            Value::Number(Number::ExactDecimal(_)) => {
                self.value_occurrences = self.value_occurrences.saturating_add(1);
                self.kind(LogicalType::Number);
                self.short_uncovered.insert(slot.0);
                self.drop_general_values();
                return retained;
            }
            Value::String(_) => LogicalType::String,
            Value::Id(_) => LogicalType::Id,
            Value::Date(_) => LogicalType::Date,
            Value::Datetime(_) => LogicalType::Datetime,
            Value::Unixtime(_) => LogicalType::Unixtime,
            Value::Null => {
                self.null_occurrence.insert(slot.0);
                self.short_uncovered.insert(slot.0);
                return retained;
            }
            _ => return retained,
        };
        self.value_occurrences = self.value_occurrences.saturating_add(1);
        self.kind(kind);
        if self.facts_enabled
            && matches!(value, Value::String(text) if text.is_empty())
            && self.empty_string.insert(slot.0)
        {
            retained += 8;
        }
        if let Value::String(text) = value {
            if text.len() <= policy.short_string_max_utf8_bytes {
                self.short_observed_occurrences = self.short_observed_occurrences.saturating_add(1);
                self.short_seen.insert(slot.0);
                if let Some(values) = &mut self.short_values {
                    let extra = if let Some(slots) = values.get_mut(text.as_str()) {
                        usize::from(slots.insert(slot.0)) * 8
                    } else {
                        let mut slots = RoaringBitmap::new();
                        slots.insert(slot.0);
                        values.insert(text.clone(), slots);
                        136usize.saturating_add(text.len())
                    };
                    self.short_estimated_bytes = self.short_estimated_bytes.saturating_add(extra);
                    retained = retained.saturating_add(extra);
                    if self.short_estimated_bytes > policy.max_short_string_index_bytes_per_path {
                        self.short_values = None;
                        self.short_dropped = true;
                    }
                }
                if let Some(distinct) = &mut self.short_distinct_without_postings
                    && !distinct.contains(text.as_str())
                {
                    if distinct.len() >= policy.max_distinct_nonblank_strings
                        || self
                            .short_distinct_without_postings_bytes
                            .saturating_add(text.len())
                            > policy.max_distinct_string_bytes
                    {
                        self.short_distinct_without_postings = None;
                        self.short_distinct_without_postings_bytes = 0;
                        self.short_summary_truncated = true;
                    } else {
                        distinct.insert(text.clone());
                        self.short_distinct_without_postings_bytes = self
                            .short_distinct_without_postings_bytes
                            .saturating_add(text.len());
                        retained = retained.saturating_add(48 + text.len());
                    }
                }
            } else {
                self.short_uncovered.insert(slot.0);
            }
        } else {
            self.short_uncovered.insert(slot.0);
        }
        if let Value::String(value) = value
            && !value.trim().is_empty()
        {
            self.nonblank_occurrences = self.nonblank_occurrences.saturating_add(1);
            self.nonblank_records.insert(slot.0);
            if let Some(distinct) = &mut self.distinct_nonblank {
                let known = distinct.contains(value);
                if value.len() > policy.max_string_bytes
                    || !known
                        && (distinct.len() >= policy.max_distinct_nonblank_strings
                            || self.distinct_nonblank_bytes.saturating_add(value.len())
                                > policy.max_distinct_string_bytes)
                {
                    self.distinct_nonblank = None;
                    self.drop_general_values();
                } else if !known {
                    distinct.insert(value.clone());
                    self.distinct_nonblank_bytes =
                        self.distinct_nonblank_bytes.saturating_add(value.len());
                    retained += value.len() + 48;
                }
            }
        }
        // Short strings use their own ordered map, with borrowed repeated-key
        // lookup. The low-cardinality provisional map only needs longer strings.
        if matches!(value, Value::String(text) if text.len() <= policy.short_string_max_utf8_bytes)
        {
            return retained;
        }
        // A dropped provisional dictionary must not keep cloning later values.
        // Check the string bound on the borrowed value before making an owned key.
        if self.values.is_none() {
            return retained;
        }
        match value {
            Value::String(text) if text.len() > policy.max_string_bytes => {
                self.drop_general_values();
                return retained;
            }
            Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
            // These kinds cannot be admitted as inferred typed postings.
            _ => return retained,
        }

        #[cfg(test)]
        {
            self.key_builds += 1;
        }
        let key = ScalarKey::from_value(value).expect("eligible inferred scalar");
        let values = self
            .values
            .as_mut()
            .expect("checked provisional dictionary");
        if values.len() >= policy.max_distinct_values_per_path && !values.contains_key(&key) {
            self.drop_general_values();
        } else {
            if !values.contains_key(&key) {
                retained += 128
                    + match &key {
                        ScalarKey::String(text) => text.len(),
                        _ => 0,
                    };
            }
            values.entry(key).or_default().insert(slot.0);
        }
        retained
    }

    fn merge_shifted(
        &mut self,
        other: Self,
        base: u32,
        policy: InferencePolicy,
    ) -> Result<(), String> {
        let occurrence_count_status =
            weakest_occurrence_status(self.occurrence_count_status, other.occurrence_count_status);
        for kind in other.kinds {
            self.kind(kind);
        }
        for (target, source) in [
            (&mut self.present, &other.present),
            (&mut self.null_occurrence, &other.null_occurrence),
            (&mut self.selected_null, &other.selected_null),
            (&mut self.exact_null, &other.exact_null),
            (&mut self.empty_array, &other.empty_array),
            (&mut self.empty_selection, &other.empty_selection),
            (&mut self.nonblank_records, &other.nonblank_records),
            (&mut self.scalar_records, &other.scalar_records),
            (&mut self.array_records, &other.array_records),
            (&mut self.object_records, &other.object_records),
            (
                &mut self.through_array_records,
                &other.through_array_records,
            ),
            (&mut self.direct_records, &other.direct_records),
            (&mut self.short_seen, &other.short_seen),
            (&mut self.short_uncovered, &other.short_uncovered),
            (&mut self.empty_string, &other.empty_string),
        ] {
            append_shifted_bitmap(target, source, base)?;
        }
        self.array_nesting = self.array_nesting.max(other.array_nesting);
        self.summary.merge_shifted(other.summary);
        self.fractional_number |= other.fractional_number;
        self.value_occurrences = self
            .value_occurrences
            .checked_add(other.value_occurrences)
            .ok_or("inference occurrence overflow")?;
        self.nonblank_occurrences = self
            .nonblank_occurrences
            .checked_add(other.nonblank_occurrences)
            .ok_or("inference occurrence overflow")?;
        self.short_estimated_bytes = self
            .short_estimated_bytes
            .saturating_add(other.short_estimated_bytes);
        self.short_observed_occurrences = self
            .short_observed_occurrences
            .checked_add(other.short_observed_occurrences)
            .ok_or("inference occurrence overflow")?;
        if self.short_dropped
            || other.short_dropped
            || self.short_estimated_bytes > policy.max_short_string_index_bytes_per_path
        {
            self.short_values = None;
            self.short_dropped = true;
        } else if let (Some(existing), Some(incoming)) =
            (&mut self.short_values, other.short_values)
        {
            for (value, bitmap) in incoming {
                append_shifted_bitmap(existing.entry(value).or_default(), &bitmap, base)?;
            }
        }
        self.short_summary_truncated |= other.short_summary_truncated;
        if self.short_summary_truncated {
            self.short_distinct_without_postings = None;
            self.short_distinct_without_postings_bytes = 0;
        } else if let (Some(existing), Some(incoming)) = (
            &mut self.short_distinct_without_postings,
            other.short_distinct_without_postings,
        ) {
            for value in incoming {
                if existing.contains(value.as_str()) {
                    continue;
                }
                if existing.len() >= policy.max_distinct_nonblank_strings
                    || self
                        .short_distinct_without_postings_bytes
                        .saturating_add(value.len())
                        > policy.max_distinct_string_bytes
                {
                    self.short_distinct_without_postings = None;
                    self.short_distinct_without_postings_bytes = 0;
                    self.short_summary_truncated = true;
                    break;
                }
                self.short_distinct_without_postings_bytes = self
                    .short_distinct_without_postings_bytes
                    .saturating_add(value.len());
                existing.insert(value);
            }
        }
        match (&mut self.distinct_nonblank, other.distinct_nonblank) {
            (Some(existing), Some(incoming)) => {
                for value in incoming {
                    let bytes = value.len();
                    if existing.insert(value) {
                        self.distinct_nonblank_bytes =
                            self.distinct_nonblank_bytes.saturating_add(bytes);
                    }
                    if existing.len() > policy.max_distinct_nonblank_strings
                        || self.distinct_nonblank_bytes > policy.max_distinct_string_bytes
                    {
                        self.distinct_nonblank = None;
                        break;
                    }
                }
            }
            _ => self.distinct_nonblank = None,
        }
        if self.dropped || other.dropped || self.distinct_nonblank.is_none() {
            self.drop_general_values();
        } else if let (Some(existing), Some(incoming)) = (&mut self.values, other.values) {
            for (key, bitmap) in incoming {
                if existing.len() >= policy.max_distinct_values_per_path
                    && !existing.contains_key(&key)
                {
                    self.drop_general_values();
                    break;
                }
                append_shifted_bitmap(existing.entry(key).or_default(), &bitmap, base)?;
            }
        }
        let mut length_overflow = false;
        if let (Some(existing), Some(incoming)) = (&mut self.lengths, other.lengths) {
            for (key, bitmap) in incoming {
                if existing.len() >= policy.max_distinct_values_per_path
                    && !existing.contains_key(&key)
                {
                    length_overflow = true;
                    break;
                }
                append_shifted_bitmap(existing.entry(key).or_default(), &bitmap, base)?;
            }
        }
        self.length_dropped |= other.length_dropped || length_overflow;
        if self.length_dropped {
            self.lengths = None;
        }
        self.occurrence_count_status = occurrence_count_status;
        Ok(())
    }
}

fn compact_field_observation(
    field: &FieldState,
    occurrence_count_status: OccurrenceCountStatus,
) -> CompactFieldObservation {
    let present_records = field
        .present
        .len()
        .saturating_add(field.empty_selection.difference_len(&field.present));
    let short_postings_present = field
        .short_values
        .as_ref()
        .is_some_and(|values| !values.is_empty());
    let mut short_distinct = field
        .short_values
        .as_ref()
        .map(|values| values.keys().cloned().collect::<HashSet<_>>())
        .unwrap_or_default();
    if let Some(existing) = &field.short_distinct_without_postings {
        short_distinct.extend(existing.iter().cloned());
    }
    let short_values_present = short_postings_present || !short_distinct.is_empty();
    let short_domains_complete = short_dictionary_covers_required_domain(
        short_postings_present,
        &field.present,
        &field.exact_null,
        &field.short_seen,
        &field.short_uncovered,
    );
    let summary_complete = !field.short_summary_truncated
        && (short_values_present || field.distinct_nonblank.is_some());
    CompactFieldObservation {
        path: field.path.clone(),
        kinds: field.kinds.clone(),
        array_nesting: field.array_nesting,
        fractional_number: field.fractional_number,
        summary_complete,
        present_records,
        null_occurrence_records: field.null_occurrence.len(),
        exact_null_records: field.exact_null.len(),
        empty_array_records: field.empty_array.len(),
        empty_selection_records: field.empty_selection.len(),
        value_occurrences: field.value_occurrences,
        nonblank_occurrences: field.nonblank_occurrences,
        nonblank_records: field.nonblank_records.len(),
        scalar_records: field.scalar_records.len(),
        array_records: field.array_records.len(),
        object_records: field.object_records.len(),
        through_array_records: field.through_array_records.len(),
        direct_records: field.direct_records.len(),
        short_string_observed_occurrences: field.short_observed_occurrences,
        occurrence_count_status,
        short_string_covered_records: field.short_seen.difference_len(&field.short_uncovered),
        short_seen_records: field.short_seen.len(),
        short_values_present,
        short_postings_present,
        short_domains_complete,
        short_dropped: field.short_dropped,
        short_summary_truncated: field.short_summary_truncated,
        lengths_present: field.lengths.is_some(),
        length_dropped: field.length_dropped,
        length_keys: field
            .lengths
            .as_ref()
            .map_or_else(BTreeSet::new, |lengths| lengths.keys().cloned().collect()),
        values_present: field.values.is_some(),
        dropped: field.dropped,
        distinct_nonblank: field.distinct_nonblank.clone(),
        short_distinct: (!short_distinct.is_empty()).then_some(short_distinct),
    }
}

fn compact_observation_bytes(
    fields: &[CompactFieldObservation],
    empty_ancestors: &HashMap<Vec<ObservedStep>, RoaringBitmap>,
) -> usize {
    64usize
        .saturating_add(fields.len().saturating_mul(256))
        .saturating_add(
            fields
                .iter()
                .map(|field| {
                    field
                        .path
                        .iter()
                        .map(|segment| segment.len().saturating_add(std::mem::size_of::<String>()))
                        .sum::<usize>()
                        .saturating_add(field.kinds.len().saturating_mul(16))
                        .saturating_add(field.length_keys.len().saturating_mul(48))
                        .saturating_add(field.distinct_nonblank.as_ref().map_or(0, |values| {
                            values
                                .iter()
                                .map(|value| value.len().saturating_add(48))
                                .sum()
                        }))
                        .saturating_add(field.short_distinct.as_ref().map_or(0, |values| {
                            values
                                .iter()
                                .map(|value| value.len().saturating_add(48))
                                .sum()
                        }))
                })
                .sum::<usize>(),
        )
        .saturating_add(
            empty_ancestors
                .iter()
                .map(|(path, bitmap)| {
                    64usize
                        .saturating_add(path.len().saturating_mul(16))
                        .saturating_add(bitmap.serialized_size())
                })
                .sum::<usize>(),
        )
}

/// One batch or coordinator's bounded construction-only observation state.
#[derive(Clone)]
pub(crate) struct InferenceBuilder {
    config: InferenceConfig,
    field_names: HashSet<Arc<str>>,
    paths: HashMap<Vec<ObservedStep>, usize>,
    fields: Vec<FieldState>,
    empty_ancestors: HashMap<Vec<ObservedStep>, RoaringBitmap>,
    selected: HashMap<usize, (u32, bool)>,
    visit_path: Vec<ObservedStep>,
    eligible: u32,
    omitted_paths: u64,
    complete: bool,
    estimated_bytes: usize,
    observed_records: u64,
    merged_batches: u64,
    #[cfg(feature = "detailed-profiling")]
    merge_ns: u64,
    unique_path_bytes: usize,
    repeated_batch_path_bytes: usize,
    batch: bool,
    batch_limit: usize,
    coordinator_limit: usize,
}

/// Reused traversal state for one already parsed logical record.
struct VisitState {
    slot: RecordSlot,
    path: Vec<ObservedStep>,
    contributions: usize,
}

impl InferenceBuilder {
    pub(crate) fn new(config: InferenceConfig, batch: bool) -> Option<Self> {
        Self::with_job_count(config, batch, 1)
    }

    pub(crate) fn with_job_count(
        config: InferenceConfig,
        batch: bool,
        _max_jobs: usize,
    ) -> Option<Self> {
        // A batch's drop decision depends on its contents, not worker count.
        // The scheduler limits simultaneous batches to the repository slack.
        let batch_limit = config.policy.max_batch_bytes;
        (config.mode != InferenceMode::Disabled).then(|| Self {
            config,
            field_names: HashSet::new(),
            paths: HashMap::new(),
            fields: Vec::new(),
            empty_ancestors: HashMap::new(),
            selected: HashMap::new(),
            visit_path: Vec::new(),
            eligible: 0,
            omitted_paths: 0,
            complete: true,
            estimated_bytes: 0,
            observed_records: 0,
            merged_batches: 0,
            #[cfg(feature = "detailed-profiling")]
            merge_ns: 0,
            unique_path_bytes: 0,
            repeated_batch_path_bytes: 0,
            batch,
            batch_limit,
            coordinator_limit: config.policy.max_total_bytes,
        })
    }

    pub(crate) fn for_collection(config: InferenceConfig, collections: usize) -> Option<Self> {
        let mut builder = Self::new(config, false)?;
        // All collection builders coexist until finalization. Give each the
        // same deterministic share of the coordinator's retained allowance.
        builder.coordinator_limit = config.policy.max_total_bytes / collections.max(1);
        Some(builder)
    }

    fn limit(&self) -> usize {
        if self.batch {
            self.batch_limit
        } else {
            self.coordinator_limit
        }
    }

    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "tested accounting snapshot for inference construction"
        )
    )]
    pub(crate) fn build_stats(&self) -> InferenceBuildStats {
        InferenceBuildStats {
            observed_records: self.observed_records,
            merged_batches: self.merged_batches,
            merge_ns: {
                #[cfg(feature = "detailed-profiling")]
                {
                    self.merge_ns
                }

                #[cfg(not(feature = "detailed-profiling"))]
                {
                    0
                }
            },
            cumulative_budget_bytes: self.estimated_bytes,
            unique_path_bytes: self.unique_path_bytes,
            repeated_batch_path_bytes: self.repeated_batch_path_bytes,
            retained_manual_summary_bytes: self.fields.iter().fold(0usize, |total, field| {
                total.saturating_add(field.short_distinct_without_postings.as_ref().map_or(
                    0,
                    |distinct| {
                        distinct
                            .len()
                            .saturating_mul(48)
                            .saturating_add(field.short_distinct_without_postings_bytes)
                    },
                ))
            }),
            ..Default::default()
        }
    }

    fn charge(&mut self, bytes: usize) {
        self.estimated_bytes = self.estimated_bytes.saturating_add(bytes);
        if self.complete && self.estimated_bytes > self.limit() {
            self.complete = false;
            for field in &mut self.fields {
                field.drop_values();
            }
        }
    }

    fn intern_name(&mut self, name: &str) -> Arc<str> {
        if let Some(existing) = self.field_names.get(name) {
            return Arc::clone(existing);
        }
        let owned: Arc<str> = Arc::from(name);
        self.field_names.insert(Arc::clone(&owned));
        owned
    }

    fn field(&mut self, path: &[ObservedStep], schema: Option<&Schema>) -> Option<usize> {
        if !self.complete {
            return None;
        }
        if let Some(index) = self.paths.get(path) {
            return Some(*index);
        }
        if path.len() > self.config.policy.max_depth
            || self.fields.len() >= self.config.policy.max_paths
        {
            self.complete = false;
            self.omitted_paths = self.omitted_paths.saturating_add(1);
            for field in &mut self.fields {
                field.drop_values();
            }
            return None;
        }
        let extra = 256
            + path
                .iter()
                .map(|step| match step {
                    ObservedStep::Field(name) => name.len(),
                    ObservedStep::ArrayIndex(_) => 4,
                })
                .sum::<usize>();
        self.charge(extra);
        if !self.complete {
            self.omitted_paths = self.omitted_paths.saturating_add(1);
            return None;
        }
        let fields = path
            .iter()
            .map(|step| match step {
                ObservedStep::Field(name) => Some(name.to_string()),
                ObservedStep::ArrayIndex(_) => None,
            })
            .collect::<Option<Vec<_>>>()?;
        let index = self.fields.len();
        self.unique_path_bytes = self.unique_path_bytes.saturating_add(extra);
        let owned = path.to_vec();
        self.paths.insert(owned, index);
        let redundant_or_prohibited = self.config.mode == InferenceMode::AutoIndex
            && schema.is_some_and(|schema| {
                schema.fields.get(&fields).is_some_and(|field| {
                    field.indexed || matches!(field.kind, LogicalType::Array(_))
                }) || schema.is_auto_index_prohibited(&fields)
            });
        self.fields.push(if redundant_or_prohibited {
            FieldState::without_provisional_postings(fields)
        } else {
            FieldState::new(fields, self.config.mode)
        });
        Some(index)
    }

    fn visit(
        &mut self,
        value: &Value,
        state: &mut VisitState,
        terminal: bool,
        through_array: bool,
        nesting: u8,
        schema: Option<&Schema>,
    ) {
        if !self.complete {
            return;
        }
        state.contributions += 1;
        if state.contributions > self.config.policy.max_record_contributions {
            self.complete = false;
            for field in &mut self.fields {
                field.drop_values();
            }
            return;
        }
        let index = if state.path.is_empty() && matches!(value, Value::Object(_)) {
            None
        } else {
            self.field(&state.path, schema)
        };
        if let Some(index) = index {
            let field = &mut self.fields[index];
            let mut charge = 0;
            if matches!(value, Value::Object(_))
                || matches!(value, Value::Array(array) if array.values.is_empty())
            {
                field.short_uncovered.insert(state.slot.0);
            }
            if matches!(value, Value::Array(_)) {
                field.array_nesting = field.array_nesting.max(nesting.saturating_add(1));
            }
            if terminal {
                field.present.insert(state.slot.0);
                field.summary.observe(value);
                if self
                    .selected
                    .get(&index)
                    .is_some_and(|(count, _)| *count > 0)
                {
                    field.summary.note_multiple();
                }
                charge += 16;
                if field.facts_enabled {
                    if matches!(value, Value::Null) && field.selected_null.insert(state.slot.0) {
                        charge += 8;
                    }
                }
                field.array_nesting = field.array_nesting.max(nesting);
                let selection = self.selected.entry(index).or_insert((0, false));
                selection.0 = selection.0.saturating_add(1);
                selection.1 = selection.0 == 1 && matches!(value, Value::Null);
                if through_array {
                    field.through_array_records.insert(state.slot.0);
                } else {
                    field.direct_records.insert(state.slot.0);
                }
                match value {
                    Value::Array(array) => {
                        field.array_records.insert(state.slot.0);
                        if array.values.is_empty() {
                            field.empty_array.insert(state.slot.0);
                            field.empty_selection.insert(state.slot.0);
                        }
                        let key = Number::integer(array.values.len() as i128);
                        if field.lengths.as_ref().is_some_and(|lengths| {
                            !lengths.contains_key(&key)
                                && lengths.len() >= self.config.policy.max_distinct_values_per_path
                        }) {
                            field.lengths = None;
                            field.length_dropped = true;
                        } else if let Some(lengths) = &mut field.lengths {
                            if !lengths.contains_key(&key) {
                                charge += 128;
                            }
                            if lengths.entry(key).or_default().insert(state.slot.0) {
                                charge += 8;
                            }
                        }
                    }
                    Value::Object(_) => {
                        field.object_records.insert(state.slot.0);
                        field.short_uncovered.insert(state.slot.0);
                    }
                    _ => {
                        field.scalar_records.insert(state.slot.0);
                    }
                }
            }
            if !matches!(value, Value::Object(_) | Value::Array(_)) {
                charge += field.add_scalar(state.slot, value, self.config.policy);
            }
            self.charge(charge);
        }
        if !self.complete {
            return;
        }
        match value {
            Value::Object(map) => {
                for (name, child) in map {
                    state.path.push(ObservedStep::Field(self.intern_name(name)));
                    self.visit(child, state, true, through_array, nesting, schema);
                    state.path.pop();
                    if !self.complete {
                        break;
                    }
                }
            }
            Value::Array(array) => {
                if array.values.is_empty() {
                    if let Some(slots) = self.empty_ancestors.get_mut(state.path.as_slice()) {
                        slots.insert(state.slot.0);
                    } else {
                        let mut slots = RoaringBitmap::new();
                        slots.insert(state.slot.0);
                        self.empty_ancestors.insert(state.path.clone(), slots);
                    }
                    self.charge(16);
                }
                for child in &array.values {
                    self.visit(child, state, false, true, nesting.saturating_add(1), schema);
                    if !self.complete {
                        break;
                    }
                }
            }
            _ => {}
        }
    }

    #[cfg(test)]
    pub(crate) fn observe_record(&mut self, slot: RecordSlot, value: &Value) -> Result<(), String> {
        self.observe_record_with_schema(slot, value, None)
    }

    pub(crate) fn observe_record_with_schema(
        &mut self,
        slot: RecordSlot,
        value: &Value,
        schema: Option<&Schema>,
    ) -> Result<(), String> {
        self.eligible = self
            .eligible
            .checked_add(1)
            .ok_or("inference coverage overflow")?;
        if !self.complete {
            return Ok(());
        }
        self.observed_records = self.observed_records.saturating_add(1);
        self.selected.clear();
        let mut state = VisitState {
            slot,
            path: std::mem::take(&mut self.visit_path),
            contributions: 0,
        };
        state.path.clear();
        self.visit(value, &mut state, true, false, 0, schema);
        self.visit_path = state.path;
        for (index, (count, sole_null)) in self.selected.drain() {
            if count == 1 && sole_null {
                self.fields[index].exact_null.insert(slot.0);
            }
        }
        Ok(())
    }

    pub(crate) fn merge_shifted(&mut self, other: Self, base: u32) -> Result<(), String> {
        #[cfg(feature = "detailed-profiling")]
        let started = crate::measurement::timer::DetailedInstant::now();
        if self.config != other.config || self.batch || !other.batch {
            return Err("inference batch policy mismatch".into());
        }
        self.merged_batches = self.merged_batches.saturating_add(1);
        self.observed_records = self.observed_records.saturating_add(other.observed_records);
        self.repeated_batch_path_bytes = self
            .repeated_batch_path_bytes
            .saturating_add(other.unique_path_bytes);
        self.eligible = self
            .eligible
            .checked_add(other.eligible)
            .ok_or("inference coverage overflow")?;
        self.omitted_paths = self.omitted_paths.saturating_add(other.omitted_paths);
        // Decline before merging a whole batch, while it still occupies the
        // scheduler's separate in-flight allowance.
        self.charge(other.estimated_bytes);
        if self.complete && !other.complete {
            self.complete = false;
            for field in &mut self.fields {
                field.drop_values();
            }
        }
        if !self.complete {
            #[cfg(feature = "detailed-profiling")]
            {
                self.merge_ns = self
                    .merge_ns
                    .saturating_add(started.elapsed().as_nanos() as u64);
            }
            return Ok(());
        }
        for (path, bitmap) in other.empty_ancestors {
            append_shifted_bitmap(self.empty_ancestors.entry(path).or_default(), &bitmap, base)?;
        }
        for incoming in other.fields {
            let typed = typed_path(&incoming.path);
            if let Some(index) = self.paths.get(&typed).copied() {
                self.fields[index].merge_shifted(incoming, base, self.config.policy)?;
            } else if let Some(index) = self.field(&typed, None) {
                let mut new = if incoming.no_provisional_postings {
                    FieldState::without_provisional_postings(incoming.path.clone())
                } else {
                    FieldState::new(incoming.path.clone(), self.config.mode)
                };
                new.merge_shifted(incoming, base, self.config.policy)?;
                self.fields[index] = new;
            }
        }

        #[cfg(feature = "detailed-profiling")]
        {
            self.merge_ns = self
                .merge_ns
                .saturating_add(started.elapsed().as_nanos() as u64);
        }
        Ok(())
    }

    pub(crate) fn finish(
        self,
        logical: &RoaringBitmap,
        record_count: usize,
        schema: &Schema,
    ) -> Result<InferenceState, String> {
        self.finish_with_observation(logical, record_count, schema, false)
            .map(|(state, _)| state)
    }

    fn finish_with_observation(
        mut self,
        logical: &RoaringBitmap,
        record_count: usize,
        schema: &Schema,
        capture_observation: bool,
    ) -> Result<(InferenceState, Option<InferenceObservation>), String> {
        if self.eligible as u64 != logical.len() {
            return Err("inference logical coverage mismatch".into());
        }
        let mut report_fields = Vec::with_capacity(self.fields.len());
        let mut indexes = HashMap::new();
        let mut short_string_coverage = HashMap::new();
        self.fields.sort_by(|a, b| a.path.cmp(&b.path));
        for field in &mut self.fields {
            for depth in 0..field.path.len() {
                if let Some(empty) = self.empty_ancestors.get(&typed_path(&field.path[..depth])) {
                    field.empty_selection |= empty;
                    if !empty.is_empty() {
                        field.summary.note_empty_traversal();
                    }
                }
            }
        }
        let mut compact_fields = if capture_observation {
            Vec::with_capacity(self.fields.len())
        } else {
            Vec::new()
        };
        for mut field in self.fields {
            if !field.present.is_subset(logical)
                || !field.exact_null.is_subset(&field.present)
                || !field.selected_null.is_subset(&field.present)
            {
                return Err("inference path membership outside logical domain".into());
            }
            field.kinds.sort_by_key(|kind| format!("{kind:?}"));
            if capture_observation {
                compact_fields.push(compact_field_observation(
                    &field,
                    if self.complete {
                        OccurrenceCountStatus::Exact
                    } else {
                        OccurrenceCountStatus::IncompleteObservation
                    },
                ));
            }
            let decision = decide_field_admission(
                self.config,
                schema,
                FieldAdmissionFacts {
                    path: &field.path,
                    complete: self.complete,
                    kinds: &field.kinds,
                    scalar_records: field.scalar_records.len(),
                    array_records: field.array_records.len(),
                    object_records: field.object_records.len(),
                    through_array_records: field.through_array_records.len(),
                    direct_records: field.direct_records.len(),
                    nonblank_occurrences: field.nonblank_occurrences,
                    nonblank_records: field.nonblank_records.len(),
                    distinct_nonblank: field
                        .distinct_nonblank
                        .as_ref()
                        .map(std::collections::HashSet::len),
                    has_short_values: field
                        .short_values
                        .as_ref()
                        .is_some_and(|values| !values.is_empty()),
                    has_lengths: !field.array_records.is_empty() && field.lengths.is_some(),
                    length_dropped: field.length_dropped,
                    short_dropped: field.short_dropped,
                    short_seen_records: field.short_seen.len(),
                    dropped: field.dropped,
                    has_values: field.values.is_some(),
                    short_summary_truncated: field.short_summary_truncated,
                },
            );
            let status = decision.status;
            let reason = decision.reason;
            let scalar_kind = decision.scalar_kind;
            let mixed_shape = decision.mixed_shape;
            let string_admissible = decision.string_admissible;
            let has_short_values = decision.has_short_values;
            let short_covered = &field.short_seen - &field.short_uncovered;
            let short_covered_count = if has_short_values {
                short_covered.len()
            } else {
                0
            };
            let short_distinct_count = field.short_values.as_ref().map_or(0, BTreeMap::len).max(
                field
                    .short_distinct_without_postings
                    .as_ref()
                    .map_or(0, HashSet::len),
            );
            let mut index_bytes = 0usize;
            let published_short_covered = if status == InferenceIndexStatus::Built {
                short_covered_count
            } else {
                0
            };
            if status == InferenceIndexStatus::Built {
                let all_short = short_dictionary_covers_required_domain(
                    has_short_values,
                    &field.present,
                    &field.exact_null,
                    &field.short_seen,
                    &field.short_uncovered,
                );
                let value_complete = !mixed_shape
                    && scalar_kind.is_some()
                    && ((!field.dropped
                        && field.values.is_some()
                        && (!has_short_values || string_admissible))
                        || all_short);
                let kind = if has_short_values {
                    LogicalType::String
                } else if !field.array_records.is_empty() && !value_complete {
                    LogicalType::Array(Box::new(LogicalType::Unknown))
                } else {
                    scalar_kind.clone().unwrap_or(LogicalType::Unknown)
                };
                let length_complete = !field.array_records.is_empty() && field.lengths.is_some();
                let empty_string_complete = kind == LogicalType::String
                    && field.array_records.is_empty()
                    && field.through_array_records.is_empty()
                    && !mixed_shape;
                let mut values = if has_short_values {
                    field
                        .short_values
                        .take()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|(key, slots)| (ScalarKey::String(key), slots))
                        .collect::<BTreeMap<_, _>>()
                } else {
                    field.values.take().unwrap_or_default()
                };
                if has_short_values && string_admissible && scalar_kind == Some(LogicalType::String)
                {
                    for (key, slots) in field.values.take().unwrap_or_default() {
                        *values.entry(key).or_default() |= slots;
                    }
                }
                if empty_string_complete && !values.contains_key(&ScalarKey::String(String::new()))
                {
                    if !field.empty_string.is_empty() {
                        values.insert(ScalarKey::String(String::new()), field.empty_string.clone());
                    }
                }
                let legacy_present_equivalent = field.empty_selection.is_subset(&field.present);
                let legacy_null_equivalent =
                    field.selected_null.difference_len(&field.exact_null) == 0;
                if !field.selected_null.is_empty() {
                    values.insert(ScalarKey::Null, std::mem::take(&mut field.selected_null));
                }
                let mut index = PostingIndex {
                    path: field.path.clone(),
                    kind,
                    multi: !field.array_records.is_empty()
                        || !field.through_array_records.is_empty(),
                    presence: field.present.clone(),
                    summary: Some(field.summary),
                    values,
                    lengths: field.lengths.take().unwrap_or_default(),
                    values_indexed: true,
                    value_complete,
                    length_complete,
                    empty_string_complete,
                    covered_records: self.eligible,
                    stats: PostingStats {
                        legacy_present_equivalent,
                        legacy_null_equivalent,
                        ..Default::default()
                    },
                };
                index.finish_stats();
                index.validate_eligible(record_count, logical.len() as usize)?;
                index.validate_domain(logical)?;
                index_bytes = index
                    .values
                    .iter()
                    .map(|(key, slots)| {
                        128 + match key {
                            ScalarKey::String(text) => text.len(),
                            _ => 0,
                        } + slots.serialized_size()
                    })
                    .sum::<usize>()
                    + index
                        .lengths
                        .values()
                        .map(|slots| 64 + slots.serialized_size())
                        .sum::<usize>();
                if has_short_values {
                    short_string_coverage.insert(field.path.clone(), short_covered);
                }
                indexes.insert(field.path.clone(), index);
            }
            let summary_bytes_estimate = 256
                + field.path.iter().map(String::len).sum::<usize>()
                + field.present.serialized_size()
                + field.null_occurrence.serialized_size()
                + field.empty_array.serialized_size();
            let retained_bytes_estimate = summary_bytes_estimate.saturating_add(index_bytes);
            let integer_only = field
                .kinds
                .contains(&LogicalType::Number)
                .then_some(!field.fractional_number);
            report_fields.push(InferredFieldReport {
                path: field.path,
                observed_kinds: field.kinds,
                scalar_records: field.scalar_records.len(),
                array_records: field.array_records.len(),
                object_records: field.object_records.len(),
                array_nesting: field.array_nesting,
                integer_only,
                eligible_records: logical.len(),
                unavailable_records: record_count as u64 - logical.len(),
                present_records: field
                    .present
                    .len()
                    .saturating_add(field.empty_selection.difference_len(&field.present)),
                missing_records: logical.len().saturating_sub(
                    field
                        .present
                        .len()
                        .saturating_add(field.empty_selection.difference_len(&field.present)),
                ),
                null_occurrence_records: field.null_occurrence.len(),
                exact_null_records: field.exact_null.len(),
                empty_array_records: field.empty_array.len(),
                empty_selection_records: field.empty_selection.len(),
                value_occurrences: field.value_occurrences,
                nonblank_occurrences: field.nonblank_occurrences,
                nonblank_records: field.nonblank_records.len(),
                distinct_nonblank_strings: field
                    .distinct_nonblank
                    .as_ref()
                    .map(std::collections::HashSet::len),
                distinct_nonblank_strings_truncated: field.distinct_nonblank.is_none(),
                short_string_observed_occurrences: field.short_observed_occurrences,
                occurrence_count_status: if self.complete {
                    OccurrenceCountStatus::Exact
                } else {
                    OccurrenceCountStatus::IncompleteObservation
                },
                short_string_distinct_values: short_distinct_count,
                short_string_distinct_values_truncated: field.short_summary_truncated,
                short_string_covered_records: published_short_covered,
                short_string_unresolved_records: logical.len() - published_short_covered,
                index_status: status,
                reason: reason.into(),
                complete: self.complete
                    && !field.short_summary_truncated
                    && (has_short_values
                        || field
                            .short_distinct_without_postings
                            .as_ref()
                            .is_some_and(|values| !values.is_empty())
                        || field.distinct_nonblank.is_some()),
                summary_bytes_estimate,
                index_bytes_estimate: index_bytes,
                retained_bytes_estimate,
            });
        }
        let report = InferenceReport {
            mode: self.config.mode,
            policy: self.config.policy,
            eligible_records: logical.len(),
            unavailable_records: record_count as u64 - logical.len(),
            omitted_paths: self.omitted_paths,
            complete: self.complete,
            observed_paths: report_fields.len(),
            admitted_paths: indexes.len(),
            dropped_paths: report_fields
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
            summary_bytes_estimate: report_fields.iter().fold(0usize, |total, field| {
                total.saturating_add(field.summary_bytes_estimate)
            }),
            index_bytes_estimate: report_fields.iter().fold(0usize, |total, field| {
                total.saturating_add(field.index_bytes_estimate)
            }),
            retained_bytes_estimate: report_fields.iter().fold(0usize, |total, field| {
                total.saturating_add(field.retained_bytes_estimate)
            }),
            fields: report_fields,
        };
        let state = InferenceState {
            report,
            indexes,
            short_string_coverage,
        };
        let observation = capture_observation.then(|| {
            let estimated_bytes = compact_observation_bytes(&compact_fields, &self.empty_ancestors);
            InferenceObservation {
                fields: compact_fields,
                empty_ancestors: self.empty_ancestors,
                complete: self.complete,
                omitted_paths: self.omitted_paths,
                estimated_bytes,
            }
        });
        Ok((state, observation))
    }

    pub(crate) fn finish_candidates_with_observation(
        mut self,
        logical: &RoaringBitmap,
        record_count: usize,
        schema: &Schema,
        capture_observation: bool,
    ) -> Result<(InferenceState, Option<InferenceObservation>), String> {
        let published_config = self.config;
        self.config = candidate_config(self.config);
        let (mut state, observation) =
            self.finish_with_observation(logical, record_count, schema, capture_observation)?;
        state.report.policy = published_config.policy;
        Ok((state, observation))
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
