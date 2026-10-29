//! Warm, complete filter queries over FFF's original-file index versus segments.
use fff_search::{
    Casing, ContentCacheBudget, FFFMode, FFFQuery, FilePicker, FilePickerOptions, FuzzyQuery,
    GrepMode, GrepSearchOptions, SharedFilePicker, SharedFrecency,
};
use objekte_core::{CacheMode, JsonFilter, JsonQuery, JsonQueryBackend, Options, Repository};
use rayon::prelude::*;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    env,
    fs::{self, File},
    io::Read,
    path::PathBuf,
    time::{Duration, Instant},
};

type BenchResult<T> = Result<T, Box<dyn std::error::Error>>;


/// One filter and its conservative source candidate pattern.
#[derive(Clone, Deserialize)]
struct Case {
    name: String,
    filter: String,
    /// A conservative raw-file candidate prefilter. None means every indexed file.
    pattern: Option<String>,
    /// `literal` or `regex`.
    mode: Option<String>,
    /// LIKE matching folds ASCII case; candidate search must be at least as broad.
    insensitive: Option<bool>,
}


/// Parsed workload case reused across benchmark repetitions.
struct CompiledCase {
    case: Case,
    filter: JsonFilter,
    safe_pattern: Option<String>,
}


/// One timed result with complete IDs for parity checks.
#[derive(Default)]
struct Sample {
    total_ms: f64,
    search_ms: f64,
    verify_ms: f64,
    candidates: usize,
    count: usize,
    ids: Vec<String>,
}


fn arg(name: &str, default: &str) -> String {
    let mut args = env::args().skip(1);
    while let Some(key) = args.next() {
        if key == name {
            return args
                .next()
                .unwrap_or_else(|| panic!("{name} needs a value"));
        }
    }
    default.into()
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

fn checked_source_bytes(path: &PathBuf) -> Result<Vec<u8>, String> {
    let mut file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let before = file.metadata().map_err(|e| e.to_string())?;
    let mut bytes = Vec::with_capacity(before.len() as usize);
    file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    let after = file.metadata().map_err(|e| e.to_string())?;
    if before.len() != after.len()
        || bytes.len() as u64 != after.len()
        || before.modified().map_err(|e| e.to_string())?
            != after.modified().map_err(|e| e.to_string())?
    {
        return Err(format!("source changed during read: {}", path.display()));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if (
            before.dev(),
            before.ino(),
            before.ctime(),
            before.ctime_nsec(),
        ) != (after.dev(), after.ino(), after.ctime(), after.ctime_nsec())
        {
            return Err(format!("source changed during read: {}", path.display()));
        }
    }
    std::hint::black_box(Sha256::digest(&bytes));
    Ok(bytes)
}

fn segment(repo: &mut Repository, case: &Case) -> BenchResult<Sample> {
    let start = Instant::now();
    let result = repo.query_json(&JsonQuery {
        filter: case.filter.clone(),
        collections: Some(vec!["contents".into()]),
        limit: 3,
        count: true,
        backend: JsonQueryBackend::Segments,
        ..Default::default()
    })?;
    Ok(Sample {
        total_ms: start.elapsed().as_secs_f64() * 1_000.0,
        ids: result.records.into_iter().map(|r| r.id).collect(),
        candidates: result.metrics.grep_candidates as usize,
        count: result.total.unwrap_or(0),
        ..Default::default()
    })
}

fn fff(
    picker: &FilePicker,
    case: &CompiledCase,
    prefilter: &str,
    escaped_paths: &[PathBuf],
) -> BenchResult<Sample> {
    let total_started = Instant::now();
    let search_started = Instant::now();
    let safe = prefilter == "safe";
    let candidate_pattern = if safe {
        case.safe_pattern.as_ref()
    } else {
        case.case.pattern.as_ref()
    };
    let mut candidates = if let Some(pattern) = candidate_pattern {
        let query = FFFQuery {
            raw_query: pattern,
            constraints: Default::default(),
            fuzzy_query: FuzzyQuery::Text(pattern),
            location: None,
        };
        let mode = match if safe {
            "regex"
        } else {
            case.case.mode.as_deref().unwrap_or("literal")
        } {
            "literal" => GrepMode::PlainText,
            "regex" => GrepMode::Regex,
            other => return Err(format!("unknown FFF mode {other}").into()),
        };
        let result = picker.grep(
            &query,
            &GrepSearchOptions {
                mode,
                casing: Some(if !safe && case.case.insensitive.unwrap_or(false) {
                    Casing::Insensitive
                } else {
                    Casing::Sensitive
                }),
                max_matches_per_file: 1,
                page_limit: usize::MAX,
                time_budget_ms: 0,
                ..Default::default()
            },
        );
        if result.next_file_offset != 0 || result.regex_fallback_error.is_some() {
            return Err(format!("FFF search was incomplete: {}", case.case.name).into());
        }
        result
            .files
            .iter()
            .map(|file| file.absolute_path(picker, picker.base_path()))
            .collect::<Vec<_>>()
    } else {
        picker
            .get_files()
            .iter()
            .map(|file| file.absolute_path(picker, picker.base_path()))
            .collect::<Vec<_>>()
    };
    if prefilter == "escape-union" && candidate_pattern.is_some() {
        candidates.extend_from_slice(escaped_paths);
        candidates.sort();
        candidates.dedup();
    }
    let search_ms = search_started.elapsed().as_secs_f64() * 1_000.0;
    let verify_started = Instant::now();
    let filtered = candidates
        .par_iter()
        .map(|path| -> Result<Option<String>, String> {
            let bytes = checked_source_bytes(path)?;
            let value: Value = match sonic_rs::from_slice(&bytes) {
                Ok(value) => value,
                Err(_) => return Ok(None),
            };
            if case.filter.matches(&value).map_err(|e| e.to_string())? {
                let id = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .ok_or_else(|| format!("invalid ID in {}", path.display()))?;
                Ok(Some(id.to_owned()))
            } else {
                Ok(None)
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut ids = filtered.into_iter().flatten().collect::<Vec<_>>();
    ids.sort();
    let count = ids.len();
    ids.truncate(3);
    for id in &ids {
        let path = picker.base_path().join(format!("{id}.json"));
        let bytes = checked_source_bytes(&path)?;
        let value: Value = sonic_rs::from_slice(&bytes)?;
        if !case.filter.matches(&value)? {
            return Err(format!("returned record no longer matches: {id}").into());
        }
    }
    let verify_ms = verify_started.elapsed().as_secs_f64() * 1_000.0;
    Ok(Sample {
        total_ms: total_started.elapsed().as_secs_f64() * 1_000.0,
        search_ms,
        verify_ms,
        candidates: candidates.len(),
        count,
        ids,
    })
}

fn main() -> BenchResult<()> {
    let root = PathBuf::from(arg("--repo", "examples/musenalm"));
    let workload = PathBuf::from(arg(
        "--workload",
        "crates/bench/workloads/musenalm-fff.json",
    ));
    let data_dir = PathBuf::from(arg("--data-dir", "target/fff-query-cache"));
    let runs: usize = arg("--runs", "5").parse()?;
    let prefilter = arg("--prefilter", "safe");
    match prefilter.as_str() {
        "safe" | "manual" | "escape-union" => {}
        _ => return Err("--prefilter must be safe, manual, or escape-union".into()),
    }
    if runs == 0 {
        return Err("--runs must be positive".into());
    }
    let cases: Vec<Case> = serde_json::from_slice(&fs::read(workload)?)?;
    let cases = cases
        .into_iter()
        .map(|case| {
            let filter = JsonFilter::compile(&case.filter, &BTreeMap::new())?;
            let safe_pattern = if case.pattern.is_some() {
                filter.source_candidate_pattern()?
            } else {
                None
            };
            Ok(CompiledCase {
                filter,
                case,
                safe_pattern,
            })
        })
        .collect::<BenchResult<Vec<_>>>()?;
    let mut options = Options::new(data_dir);
    options.cache = CacheMode::Segments;
    let mut repo = Repository::load_repository(&root, options)?;
    let base_path = root.join("data/contents").canonicalize()?;
    let shared = SharedFilePicker::default();
    let start = Instant::now();
    FilePicker::new_with_shared_state(
        shared.clone(),
        SharedFrecency::default(),
        FilePickerOptions {
            base_path: base_path.to_string_lossy().into_owned(),
            mode: FFFMode::Ai,
            enable_content_indexing: true,
            enable_mmap_cache: true,
            cache_budget: Some(ContentCacheBudget::new_for_repo(133_948)),
            watch: false,
            git_recency: fff_search::GitRecencyConfig {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        },
    )?;
    if !shared.wait_for_indexing_complete(Duration::from_secs(240)) {
        return Err("FFF indexing timed out".into());
    }
    let indexing_ms = start.elapsed().as_secs_f64() * 1_000.0;
    let guard = shared.read()?;
    let picker = guard.as_ref().ok_or("FFF picker missing")?;
    if picker.bigram_index().is_none() {
        return Err("FFF content index missing".into());
    }
    if picker.get_files().len() != 133_948 {
        return Err(format!(
            "FFF indexed {} files, expected 133948",
            picker.get_files().len()
        )
        .into());
    }
    let escape_started = Instant::now();
    let escaped_paths = if prefilter == "escape-union" {
        let query = FFFQuery {
            raw_query: "\\",
            constraints: Default::default(),
            fuzzy_query: FuzzyQuery::Text("\\"),
            location: None,
        };
        let result = picker.grep(
            &query,
            &GrepSearchOptions {
                casing: Some(Casing::Sensitive),
                max_matches_per_file: 1,
                page_limit: usize::MAX,
                ..Default::default()
            },
        );
        if result.next_file_offset != 0 {
            return Err("FFF escape-file scan incomplete".into());
        }
        result
            .files
            .iter()
            .map(|file| file.absolute_path(picker, picker.base_path()))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let escape_scan_ms = escape_started.elapsed().as_secs_f64() * 1_000.0;
    println!(
        "{}",
        json!({"type":"setup","fff_indexing_ms":indexing_ms,"fff_files":picker.get_files().len(),"fff_bigram_files":picker.bigram_index().unwrap().file_count(),"escape_files":escaped_paths.len(),"escape_scan_ms":escape_scan_ms,"cases":cases.len(),"runs":runs,"prefilter":prefilter})
    );
    // Warm both backends and assert their complete counts and first page agree.
    for case in &cases {
        let f = fff(picker, case, &prefilter, &escaped_paths)?;
        let s = segment(&mut repo, &case.case)?;
        if f.count != s.count || f.ids != s.ids {
            return Err(format!(
                "result mismatch in {}: FFF count {} first {:?}; segments count {} first {:?}",
                case.case.name, f.count, f.ids, s.count, s.ids
            )
            .into());
        }
    }
    for case in &cases {
        let mut fff_ms = Vec::new();
        let mut segment_ms = Vec::new();
        let mut search_ms = Vec::new();
        let mut verify_ms = Vec::new();
        let mut fff_candidates = 0;
        let mut segment_candidates = 0;
        let mut matches = 0;
        for run in 0..runs {
            let (f, s) = if run % 2 == 0 {
                let f = fff(picker, case, &prefilter, &escaped_paths)?;
                let s = segment(&mut repo, &case.case)?;
                (f, s)
            } else {
                let s = segment(&mut repo, &case.case)?;
                let f = fff(picker, case, &prefilter, &escaped_paths)?;
                (f, s)
            };
            if f.count != s.count || f.ids != s.ids {
                return Err(format!("timed result mismatch in {}", case.case.name).into());
            }
            fff_candidates = f.candidates;
            segment_candidates = s.candidates;
            matches = f.count;
            fff_ms.push(f.total_ms);
            segment_ms.push(s.total_ms);
            search_ms.push(f.search_ms);
            verify_ms.push(f.verify_ms);
        }
        println!(
            "{}",
            json!({"type":"case","name":case.case.name,"filter":case.case.filter,"pattern":if prefilter == "safe" { &case.safe_pattern } else { &case.case.pattern },"mode":if prefilter == "safe" { "regex" } else { case.case.mode.as_deref().unwrap_or("literal") },"matches":matches,"fff_candidates":fff_candidates,"segment_text_candidates":segment_candidates,"fff_median_ms":median(&mut fff_ms),"fff_search_median_ms":median(&mut search_ms),"fff_verify_median_ms":median(&mut verify_ms),"segments_median_ms":median(&mut segment_ms)})
        );
    }
    Ok(())
}
