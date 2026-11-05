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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SortField {
    pub field: String,
    pub descending: bool,
}

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

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WorkCounters {
    pub exact_posting_matches: u64,
    pub candidates: u64,
    pub scanned_records: u64,
    pub total_matches: u64,
    pub returned_records: u64,
    pub files_opened: u64,
    pub bytes_read: u64,
    pub record_parses: u64,
    pub residual_evaluations: u64,
    pub prepared_plan_hit: bool,
    pub verified_result_hit: bool,
}

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
mod tests {
    use super::*;

    #[test]
    fn fixed_manifest_and_report_round_trip() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/musenalm_queries.json");
        let cases = load_manifest(&path).unwrap();
        assert_eq!(cases[0].fts.as_deref(), Some("Almanach"));
        assert_eq!(
            cases[69].filter.as_deref(),
            Some("name:folded ~ \"GOETHE\"")
        );
        assert_eq!(cases[149].sort[0].field, "preferred_title");
        assert_eq!(
            cases,
            serde_json::from_slice::<Vec<WorkloadCase>>(&serde_json::to_vec(&cases).unwrap())
                .unwrap()
        );
        let sample = TimingSample {
            case_id: "Q001".into(),
            profile: PROFILES[0],
            repetition: 0,
            status: "pending".into(),
            error: None,
            total_ns: None,
            counters: None,
        };
        let serialized = serde_json::to_vec(&sample).unwrap();
        let decoded: TimingSample = serde_json::from_slice(&serialized).unwrap();
        assert_eq!(decoded.case_id, sample.case_id);
        assert_eq!(decoded.total_ns, None);
        assert_eq!(PROFILES.iter().filter(|p| p.warm).count(), 2);
    }
}
