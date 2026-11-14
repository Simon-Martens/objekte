//! Runtime work accounting shared by query execution and diagnostics.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;


/// Why a pinned source range could not be read during a live query.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceUnavailableKind {
    Missing,
    RevisionMismatch,
    UnusableRange,
    Unreadable,
}


/// A bounded example of source unavailability. `source_unavailable_events`
/// counts every occurrence even after the diagnostic limit is reached.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceDiagnostic {
    pub file_id: u32,
    pub file_path: PathBuf,
    pub kind: SourceUnavailableKind,
    pub message: String,
}


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
    /// Number of source-read failures excluded from this result.
    #[serde(default)]
    pub source_unavailable_events: u64,
    /// First 32 source-read failures, in execution order.
    #[serde(default)]
    pub source_diagnostics: Vec<SourceDiagnostic>,
    pub returned_records: u64,
    /// Selected lifecycle segment payload ranges opened after page selection.
    /// Residual verification remains accounted for by the existing counters.
    #[serde(default)]
    pub selected_payload_reads: u64,
    #[serde(default)]
    pub selected_payload_bytes: u64,
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
    /// Slot memberships read from those reverse posting lists.
    #[serde(default)]
    pub relation_reverse_members_visited: u64,
    /// Target values reused by reverse mapping or source-dependent comparisons.
    #[serde(default)]
    pub relation_verified_targets_reused: u64,
    #[cfg(feature = "detailed-profiling")]
    #[serde(default)]
    pub preparation_ns: u64,
    #[cfg(feature = "detailed-profiling")]
    #[serde(default)]
    pub planning_ns: u64,
    pub verified_result_hit: bool,
}


/// Counts produced by scanning one primary-source file. Relation reads are
/// charged separately so the relation budget is checked at the same point as
/// in the query executor.
#[derive(Clone, Copy, Default)]
pub(crate) struct ScanWork {
    pub files_opened: u64,
    pub bytes_read: u64,
    pub record_parses: u64,
    pub raw_text_verifications: u64,
    pub scanned_records: u64,
    pub sort_key_evaluations: u64,
    pub residual_evaluations: u64,
}


impl WorkCounters {
    pub(crate) fn record_source_unavailable(&mut self, mut diagnostic: SourceDiagnostic) {
        self.source_unavailable_events = self.source_unavailable_events.saturating_add(1);
        self.match_count_complete = false;
        if self.source_diagnostics.len() < 32 {
            if diagnostic.message.len() > 256 {
                let end = (0..=256)
                    .rev()
                    .find(|end| diagnostic.message.is_char_boundary(*end))
                    .unwrap_or(0);
                diagnostic.message.truncate(end);
            }
            self.source_diagnostics.push(diagnostic);
        }
    }

    pub(crate) fn add_scan(
        &mut self,
        scan: ScanWork,
        relation: &crate::relation_query::RelationWork,
    ) -> Result<(), String> {
        self.files_opened += scan.files_opened;
        self.bytes_read += scan.bytes_read;
        self.record_parses += scan.record_parses;
        self.raw_text_verifications += scan.raw_text_verifications;
        self.scanned_records += scan.scanned_records;
        self.sort_key_evaluations += scan.sort_key_evaluations;
        self.residual_evaluations += scan.residual_evaluations;
        self.relation_source_reads += relation.source_reads;
        self.relation_target_reads += relation.target_reads;
        self.relation_verified_targets_reused += relation.target_cache_hits;
        self.relation_target_file_opens += relation.target_reads;
        self.relation_edge_visits += relation.edge_visits;
        self.relation_backlink_probe_visits += relation.backlink_probe_visits;
        self.relation_bytes_read += relation.bytes_read;
        crate::relation_query::check_query_work_budget(self)?;
        self.files_opened += relation.source_reads + relation.target_reads;
        self.bytes_read += relation.bytes_read;
        self.record_parses += relation.source_reads + relation.target_reads;
        Ok(())
    }

    pub(crate) fn add_semijoin(
        &mut self,
        work: crate::relation_semijoin::SemijoinWork,
    ) -> Result<(), String> {
        self.relation_target_candidates += work.target_candidates;
        self.relation_target_matches += work.target_matches;
        self.relation_target_reads += work.target_reads;
        self.relation_target_file_opens += work.target_file_opens;
        self.relation_edge_visits += work.edge_visits;
        self.relation_reverse_postings_visited += work.reverse_postings_visited;
        self.relation_reverse_members_visited += work.reverse_members_visited;
        self.relation_verified_targets_reused += work.verified_targets_reused;
        self.relation_bytes_read += work.target_bytes_read;
        self.files_opened += work.target_file_opens;
        self.bytes_read += work.target_bytes_read;
        self.record_parses += work.target_reads;
        crate::relation_query::check_query_work_budget(self)
    }
}


/// Query-local collector; it counts every unavailable read while retaining a
/// fixed number of representative diagnostics.
#[derive(Default)]
pub(crate) struct SourceAvailabilityLog {
    events: u64,
    diagnostics: Vec<SourceDiagnostic>,
}


impl SourceAvailabilityLog {
    pub(crate) fn events(&self) -> u64 {
        self.events
    }

    pub(crate) fn record(&mut self, diagnostic: SourceDiagnostic) {
        self.events = self.events.saturating_add(1);
        if self.diagnostics.len() < 32 {
            self.diagnostics.push(diagnostic);
        }
    }

    pub(crate) fn apply(self, counters: &mut WorkCounters) {
        let shown = self.diagnostics.len() as u64;
        for diagnostic in self.diagnostics {
            counters.record_source_unavailable(diagnostic);
        }
        counters.source_unavailable_events = counters
            .source_unavailable_events
            .saturating_add(self.events.saturating_sub(shown));
    }
}


#[cfg(test)]
#[test]
fn source_diagnostics_remain_bounded_after_many_failures() {
    let mut counters = WorkCounters {
        match_count_complete: true,
        ..Default::default()
    };
    for file_id in 0..40 {
        counters.record_source_unavailable(SourceDiagnostic {
            file_id,
            file_path: PathBuf::from(format!("file-{file_id}")),
            kind: SourceUnavailableKind::Missing,
            message: "é".repeat(200),
        });
    }
    assert_eq!(counters.source_unavailable_events, 40);
    assert!(!counters.match_count_complete);
    assert_eq!(counters.source_diagnostics.len(), 32);
    assert!(
        counters
            .source_diagnostics
            .iter()
            .all(|entry| entry.message.len() <= 256)
    );
}

#[cfg(test)]
#[test]
fn old_prepared_hit_counter_is_ignored_when_reading_archived_json() {
    let mut archived = serde_json::to_value(WorkCounters::default()).unwrap();
    let retired_key = ["prepared", "plan", "hit"].join("_");
    archived[retired_key.as_str()] = serde_json::Value::Bool(true);
    let counters: WorkCounters = serde_json::from_value(archived).unwrap();
    assert!(serde_json::to_value(counters)
        .unwrap()
        .get(&retired_key)
        .is_none());
}
