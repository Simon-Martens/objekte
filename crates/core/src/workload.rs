//! Fixed request inputs and measurement envelopes. No execution is performed here.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;


#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReturnMode {
    Locator,
    Bytes,
    Logical,
}


/// Field and direction requested by a workload case.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SortField {
    pub field: String,
    pub descending: bool,
}


/// Fixed benchmark query and expected execution options.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkloadCase {
    pub id: String,
    pub collection: String,
    pub fts: Option<String>,
    pub filter: Option<String>,
    #[serde(default)]
    pub fts_fields: Vec<String>,
    #[serde(default)]
    pub sort: Vec<SortField>,
    pub limit: usize,
    pub offset: usize,
    pub count: bool,
    pub return_mode: ReturnMode,
    pub evidence: bool,
}


#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BigramAlphabet {
    Ascii,
    Unicode,
}


/// Alphabet and cache settings for one benchmark profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub alphabet: BigramAlphabet,
    pub verified_cache_bytes: usize,
    pub warm: bool,
}


pub const PROFILES: [Profile; 4] = [
    Profile {
        alphabet: BigramAlphabet::Ascii,
        verified_cache_bytes: 0,
        warm: false,
    },
    Profile {
        alphabet: BigramAlphabet::Ascii,
        verified_cache_bytes: 64 * 1024 * 1024,
        warm: true,
    },
    Profile {
        alphabet: BigramAlphabet::Unicode,
        verified_cache_bytes: 0,
        warm: false,
    },
    Profile {
        alphabet: BigramAlphabet::Unicode,
        verified_cache_bytes: 64 * 1024 * 1024,
        warm: true,
    },
];


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
    pub returned_records: u64,
    pub files_opened: u64,
    pub bytes_read: u64,
    /// Peak retained page payload bytes, estimated from source record lengths.
    #[serde(default)]
    pub peak_retained_payload_bytes: u64,
    pub record_parses: u64,
    /// Filter terms actually evaluated against a decoded record (proof hits excluded).
    pub residual_evaluations: u64,
    #[serde(default)]
    pub preparation_ns: u64,
    #[serde(default)]
    pub planning_ns: u64,
    pub prepared_plan_hit: bool,
    pub verified_result_hit: bool,
}


/// One measured query execution and its counters.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TimingSample {
    pub case_id: String,
    pub profile: Profile,
    pub repetition: usize,
    pub status: String,
    pub error: Option<String>,
    pub total_ns: Option<u64>,
    pub counters: Option<WorkCounters>,
}


/// Build and fixture fingerprints for a benchmark run.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunIdentity {
    pub run_id: String,
    pub code_revision: String,
    pub compiler: String,
    pub repository_root: String,
    pub corpus_fingerprint: String,
    pub schema_fingerprint: String,
    pub workload_fingerprint: String,
    pub repetitions: usize,
    pub profiles: Vec<Profile>,
}


pub const REPORT_FILES: [&str; 7] = [
    "run-manifest.json",
    "queries.json",
    "measurements.jsonl",
    "query-results.csv",
    "query-plans.jsonl",
    "index-results.json",
    "REPORT.md",
];

pub fn load_manifest(path: &Path) -> Result<Vec<WorkloadCase>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let cases: Vec<WorkloadCase> = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    validate_manifest(&cases)?;
    Ok(cases)
}

pub fn validate_manifest(cases: &[WorkloadCase]) -> Result<(), String> {
    if cases.len() != 150 {
        return Err(format!("expected 150 workload cases, got {}", cases.len()));
    }
    let mut seen = HashSet::new();
    for (i, case) in cases.iter().enumerate() {
        let expected = format!("Q{:03}", i + 1);
        if case.id != expected || !seen.insert(&case.id) {
            return Err(format!("expected unique {expected}, got {}", case.id));
        }
        if case.collection.is_empty() || (case.fts.is_none() && case.filter.is_none()) {
            return Err(format!("{} has no collection or predicate", case.id));
        }
        if case.limit == 0 {
            return Err(format!("{} has zero limit", case.id));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "workload.test.rs"]
mod tests;
