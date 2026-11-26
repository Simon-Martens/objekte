//! Logical construction observations used to decide collection-wide admission.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use crate::inference::OccurrenceCountStatus;
use crate::model::{LogicalType, Number};
use crate::query_access::membership::SlotSet;

use super::transfer::StorageResult;

pub(crate) type InferenceObservationHandle = Arc<dyn InferenceObservationReader>;

/// Dictionary metadata used by the collection report. A backend can answer
/// this without reading keys or their memberships.
pub(crate) struct InferredPostingDiagnostic {
    pub kind: LogicalType,
    pub multi: bool,
    pub value_keys: usize,
    pub length_keys: usize,
    pub covered_records: u32,
    pub value_complete: bool,
    pub has_structural_facts: bool,
    pub estimated_bytes: usize,
}

pub(crate) trait InferenceObservationReader: Send + Sync {
    fn estimated_bytes(&self) -> usize;
    fn complete(&self) -> bool;
    fn omitted_paths(&self) -> u64;

    fn visit_paths(
        &self,
        visit: &mut dyn FnMut(&[&[String]]) -> StorageResult<()>,
    ) -> StorageResult<()>;

    fn field(&self, path: &[String]) -> StorageResult<Option<Cow<'_, CompactFieldObservation>>>;

    fn empty_ancestor_members(&self, prefix: &[String]) -> StorageResult<SlotSet<'_>>;
}

#[derive(Clone)]
pub(crate) struct CompactFieldObservation {
    pub(crate) path: Vec<String>,
    pub(crate) kinds: Vec<LogicalType>,
    pub(crate) array_nesting: u8,
    pub(crate) fractional_number: bool,
    pub(crate) summary_complete: bool,
    pub(crate) present_records: u64,
    pub(crate) null_occurrence_records: u64,
    pub(crate) exact_null_records: u64,
    pub(crate) empty_array_records: u64,
    pub(crate) empty_selection_records: u64,
    pub(crate) value_occurrences: u64,
    pub(crate) nonblank_occurrences: u64,
    pub(crate) nonblank_records: u64,
    pub(crate) scalar_records: u64,
    pub(crate) array_records: u64,
    pub(crate) object_records: u64,
    pub(crate) through_array_records: u64,
    pub(crate) direct_records: u64,
    pub(crate) short_string_observed_occurrences: u64,
    pub(crate) occurrence_count_status: OccurrenceCountStatus,
    pub(crate) short_string_covered_records: u64,
    pub(crate) short_seen_records: u64,
    pub(crate) short_values_present: bool,
    pub(crate) short_postings_present: bool,
    pub(crate) short_domains_complete: bool,
    pub(crate) short_dropped: bool,
    pub(crate) short_summary_truncated: bool,
    pub(crate) lengths_present: bool,
    pub(crate) length_dropped: bool,
    pub(crate) length_keys: BTreeSet<Number>,
    pub(crate) values_present: bool,
    pub(crate) dropped: bool,
    pub(crate) distinct_nonblank: Option<HashSet<String>>,
    pub(crate) short_distinct: Option<HashSet<String>>,
}
