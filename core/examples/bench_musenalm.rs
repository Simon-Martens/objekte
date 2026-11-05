#[path = "support/musenalm.rs"]
mod musenalm;

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use objekte_search_core::query::Query;
use objekte_search_core::repository::{Repository, RepositoryOptions};
use objekte_search_core::workload::{
    BigramAlphabet, Profile, TimingSample, WorkCounters, load_manifest,
};
use sha2::{Digest, Sha256};

fn main() {
    if let Err(error) = run() {
        eprintln!("benchmark error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let arguments: Vec<String> = std::env::args().collect();
    let mut args = arguments.iter().skip(1);
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../examples/musenalm");
    let mut result_dir = None;
    let mut case_ids = None;
    let mut profile_names = vec![
        "ascii-off".to_string(),
        "ascii-warm".to_string(),
        "unicode-off".to_string(),
        "unicode-warm".to_string(),
    ];
    let mut repetitions = 5usize;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = PathBuf::from(args.next().ok_or("--root needs a path")?),
            "--result-dir" => {
                result_dir = Some(PathBuf::from(
                    args.next().ok_or("--result-dir needs a path")?,
                ))
            }
            "--cases" => {
                case_ids = Some(
                    args.next()
                        .ok_or("--cases needs IDs")?
                        .split(',')
                        .map(str::to_owned)
                        .collect::<Vec<_>>(),
                )
            }
            "--profiles" => {
                profile_names = args
                    .next()
                    .ok_or("--profiles needs names")?
                    .split(',')
                    .map(str::to_owned)
                    .collect()
            }
            "--repetitions" => {
                repetitions = args
                    .next()
                    .ok_or("--repetitions needs a number")?
                    .parse()
                    .map_err(|e| format!("invalid repetitions: {e}"))?
            }
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    if repetitions == 0 {
        return Err("repetitions must be positive".into());
    }
    let mut profiles = Vec::new();
    for name in &profile_names {
        let profile = match name.as_str() {
            "ascii-off" => Profile {
                alphabet: BigramAlphabet::Ascii,
                verified_cache_bytes: 0,
                warm: false,
            },
            "ascii-warm" => Profile {
                alphabet: BigramAlphabet::Ascii,
                verified_cache_bytes: 64 * 1024 * 1024,
                warm: true,
            },
            "unicode-off" => Profile {
                alphabet: BigramAlphabet::Unicode,
                verified_cache_bytes: 0,
                warm: false,
            },
            "unicode-warm" => Profile {
                alphabet: BigramAlphabet::Unicode,
                verified_cache_bytes: 64 * 1024 * 1024,
                warm: true,
            },
            _ => return Err(format!("unknown profile {name}")),
        };
        if profile.warm {
            return Err(
                "verified-result caching is pending; select off profiles for this baseline".into(),
            );
        }
        profiles.push((name.clone(), profile));
    }
    let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/musenalm_queries.json");
    let all_cases = load_manifest(&manifest_path)?;
    let cases: Vec<_> = if let Some(ids) = case_ids {
        for id in &ids {
            if !all_cases.iter().any(|case| &case.id == id) {
                return Err(format!("unknown case {id}"));
            }
        }
        all_cases
            .into_iter()
            .filter(|case| ids.contains(&case.id))
            .collect()
    } else {
        all_cases
    };
    let definitions = musenalm::load_definitions(&root)?;
    let rev = command_output("git", &["rev-parse", "HEAD"])?;
    let compiler = command_output("rustc", &["--version"])?;
    let run_id = format!(
        "{}-{}",
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_secs(),
        &rev[..10]
    );
    let result_dir =
        result_dir.unwrap_or_else(|| PathBuf::from("bench/results/musenalm").join(&run_id));
    fs::create_dir_all(&result_dir).map_err(|e| e.to_string())?;
    fs::write(
        result_dir.join("queries.json"),
        serde_json::to_vec_pretty(&cases).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let schema_hash = hash_paths(&schema_paths(&root)?)?;
    let workload_hash = hash_paths(&[manifest_path])?;
    let mut samples = BufWriter::new(
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(result_dir.join("measurements.jsonl"))
            .map_err(|e| e.to_string())?,
    );
    let mut plan_output = BufWriter::new(
        File::create(result_dir.join("query-plans.jsonl")).map_err(|e| e.to_string())?,
    );
    let mut rows = Vec::new();
    let mut build_results = Vec::new();
    let mut corpus_hash = String::new();
    let mut registrations = Vec::new();
    let mut expected: HashMap<String, (usize, Vec<String>)> = HashMap::new();
    let mut failures = 0usize;
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let selected_profiles: Vec<_> = profiles
            .iter()
            .filter(|(_, p)| p.alphabet == alphabet)
            .collect();
        if selected_profiles.is_empty() {
            continue;
        }
        let build_start = Instant::now();
        let repo = Repository::open_with_options(
            &root,
            definitions.clone(),
            RepositoryOptions {
                bigram_alphabet: alphabet,
                ..RepositoryOptions::default()
            },
        )?;
        let build_ns = build_start.elapsed().as_nanos() as u64;
        if corpus_hash.is_empty() {
            corpus_hash = fingerprint_inventory(&repo);
        }
        let memory_bytes = resident_bytes();
        let estimated_index_bytes: usize = repo
            .collections
            .iter()
            .map(|c| {
                c.raw.estimated_heap_bytes()
                    + c.custom_text
                        .values()
                        .map(|i| i.estimated_heap_bytes())
                        .sum::<usize>()
                    + c.postings
                        .fields
                        .values()
                        .map(|p| {
                            p.values
                                .values()
                                .map(|s| s.serialized_size() as usize)
                                .sum::<usize>()
                                + p.lengths
                                    .values()
                                    .map(|s| s.serialized_size() as usize)
                                    .sum::<usize>()
                        })
                        .sum::<usize>()
                    + c.sorting
                        .columns
                        .values()
                        .map(|s| {
                            s.keys
                                .iter()
                                .map(|k| match k {
                                    objekte_search_core::sorting::SortKey::String(v) => v.len(),
                                    _ => 24,
                                })
                                .sum::<usize>()
                        })
                        .sum::<usize>()
            })
            .sum();
        build_results.push(serde_json::json!({"alphabet":alphabet,"build_ns":build_ns,"build_parses":repo.build_parses,"resident_bytes":memory_bytes,"estimated_index_bytes":estimated_index_bytes,"restore_status":"pending"}));
        if registrations.is_empty() {
            registrations = repo.collections.iter().map(|c| serde_json::json!({"name":c.definition.name,"source_path":c.definition.source_path,"format":c.definition.format,"identity":format!("{:?}",c.definition.identity),"files":c.generation.files.len(),"records":c.generation.records.len(),"type_hints":c.definition.schema.fields.len(),"indexed_paths":c.postings.fields.keys().map(|p|p.join(".")).collect::<Vec<_>>(),"sort_fields":c.definition.sort_fields.iter().map(|p|p.display()).collect::<Vec<_>>(),"custom_bindings":c.definition.schema.custom.keys().map(|(p,n)|format!("{}:{n}",p.join("."))).collect::<Vec<_>>() })).collect();
        }
        for (profile_name, profile) in selected_profiles {
            for case in &cases {
                let query = match Query::from_workload(case) {
                    Ok(q) => q,
                    Err(error) => {
                        failures += 1;
                        for repetition in 0..repetitions {
                            write_sample(
                                &mut samples,
                                TimingSample {
                                    case_id: case.id.clone(),
                                    profile: *profile,
                                    repetition,
                                    status: "failed".into(),
                                    error: Some(error.clone()),
                                    total_ns: None,
                                    counters: None,
                                },
                            )?;
                        }
                        rows.push((
                            case.id.clone(),
                            profile_name.clone(),
                            Vec::<u64>::new(),
                            None::<WorkCounters>,
                            Some(error),
                        ));
                        continue;
                    }
                };
                let mut times = Vec::new();
                let mut last = None;
                let mut error = None;
                for repetition in 0..repetitions {
                    let start = Instant::now();
                    match repo.search(&query) {
                        Ok(result) => {
                            let ns = start.elapsed().as_nanos() as u64;
                            let identity = (
                                result.count,
                                result
                                    .hits
                                    .iter()
                                    .map(|h| format!("{}:{}", h.collection, h.record_id))
                                    .collect::<Vec<_>>(),
                            );
                            if let Some(previous) = expected.get(&case.id) {
                                if previous != &identity {
                                    error = Some("results differ across profiles".to_string());
                                }
                            } else {
                                expected.insert(case.id.clone(), identity);
                            }
                            let counters = result.counters;
                            write_sample(
                                &mut samples,
                                TimingSample {
                                    case_id: case.id.clone(),
                                    profile: *profile,
                                    repetition,
                                    status: if error.is_some() { "failed" } else { "ok" }.into(),
                                    error: error.clone(),
                                    total_ns: Some(ns),
                                    counters: Some(counters.clone()),
                                },
                            )?;
                            times.push(ns);
                            last = Some(counters);
                        }
                        Err(message) => {
                            write_sample(
                                &mut samples,
                                TimingSample {
                                    case_id: case.id.clone(),
                                    profile: *profile,
                                    repetition,
                                    status: "failed".into(),
                                    error: Some(message.clone()),
                                    total_ns: None,
                                    counters: None,
                                },
                            )?;
                            error = Some(message);
                        }
                    }
                }
                if error.is_some() {
                    failures += 1;
                }
                rows.push((case.id.clone(), profile_name.clone(), times, last, error));
                let (_, plans) = repo.explain_analyze(&query)?;
                serde_json::to_writer(&mut plan_output, &serde_json::json!({
                    "case_id": case.id, "profile": profile_name, "diagnostic_timing": "outside_primary_samples",
                    "plans": plans
                })).map_err(|e| e.to_string())?;
                plan_output.write_all(b"\n").map_err(|e| e.to_string())?;
            }
        }
    }
    samples.flush().map_err(|e| e.to_string())?;
    plan_output.flush().map_err(|e| e.to_string())?;
    let run_manifest = serde_json::json!({"run_id":run_id,"code_revision":rev,"compiler":compiler,"command":arguments,"root":root,"schema_fingerprint":schema_hash,"workload_fingerprint":workload_hash,"corpus_fingerprint":corpus_hash,"repetitions":repetitions,"profiles":profiles,"registrations":registrations,"failures":failures,"source_cache_bytes":0,"decoded_cache_bytes":0});
    fs::write(
        result_dir.join("run-manifest.json"),
        serde_json::to_vec_pretty(&run_manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::write(
        result_dir.join("index-results.json"),
        serde_json::to_vec_pretty(&build_results).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let mut csv = BufWriter::new(
        File::create(result_dir.join("query-results.csv")).map_err(|e| e.to_string())?,
    );
    writeln!(csv,"case_id,profile,status,min_ns,median_ns,max_ns,samples,total_matches,returned_records,candidates,files_opened,bytes_read,record_parses").map_err(|e|e.to_string())?;
    let mut report = String::from("# Musenalm query checkpoint\n\n");
    report.push_str(&format!(
        "Code revision: `{rev}`. Repetitions: {repetitions}. Failures: {failures}.\n\n"
    ));
    report.push_str("| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Status |\n|---|---|---:|---:|---:|---:|---:|---:|---|\n");
    let mut by_case: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (id, profile, times, counters, error) in &rows {
        let mut sorted = times.clone();
        sorted.sort_unstable();
        let (min, med, max) = if sorted.is_empty() {
            (None, None, None)
        } else {
            (
                sorted.first().copied(),
                sorted.get(sorted.len() / 2).copied(),
                sorted.last().copied(),
            )
        };
        let value = |n: Option<u64>| {
            n.map(|v| format!("{:.3}", v as f64 / 1000.0))
                .unwrap_or_else(|| "failed".into())
        };
        let status = error.as_deref().unwrap_or("ok");
        report.push_str(&format!(
            "| {id} | {profile} | {} | {} | {} | {} | {} | {} | {status} |\n",
            value(min),
            value(med),
            value(max),
            counters.as_ref().map(|c| c.total_matches).unwrap_or(0),
            counters.as_ref().map(|c| c.candidates).unwrap_or(0),
            counters.as_ref().map(|c| c.record_parses).unwrap_or(0)
        ));
        writeln!(
            csv,
            "{id},{profile},{},{},{},{},{},{},{},{},{},{},{}",
            if error.is_some() { "failed" } else { "ok" },
            min.map(|v| v.to_string()).unwrap_or_default(),
            med.map(|v| v.to_string()).unwrap_or_default(),
            max.map(|v| v.to_string()).unwrap_or_default(),
            times.len(),
            counters.as_ref().map(|c| c.total_matches).unwrap_or(0),
            counters.as_ref().map(|c| c.returned_records).unwrap_or(0),
            counters.as_ref().map(|c| c.candidates).unwrap_or(0),
            counters.as_ref().map(|c| c.files_opened).unwrap_or(0),
            counters.as_ref().map(|c| c.bytes_read).unwrap_or(0),
            counters.as_ref().map(|c| c.record_parses).unwrap_or(0)
        )
        .map_err(|e| e.to_string())?;
        by_case.entry(id.clone()).or_default().push(format!(
            "{profile}: {} / {} / {} µs ({status})",
            value(min),
            value(med),
            value(max)
        ));
    }
    csv.flush().map_err(|e| e.to_string())?;
    report.push_str("\n## Index construction\n\n");
    for result in &build_results {
        report.push_str(&format!(
            "- {}: {:.3} s; parses {}; resident {} MiB; estimated index {} MiB. Restore pending.\n",
            result["alphabet"],
            result["build_ns"].as_u64().unwrap_or(0) as f64 / 1e9,
            result["build_parses"],
            result["resident_bytes"].as_u64().unwrap_or(0) / 1024 / 1024,
            result["estimated_index_bytes"].as_u64().unwrap_or(0) / 1024 / 1024
        ));
    }
    report.push_str("\nTiming includes COUNT and requested page/payload. Query plans and restore measurements are pending.\n");
    fs::write(result_dir.join("REPORT.md"), report).map_err(|e| e.to_string())?;
    for case in &cases {
        println!(
            "{} collection={} FTS={:?} FILTER={:?} sort={:?} limit={} offset={} mode={:?} count={} evidence={} | {}",
            case.id,
            case.collection,
            case.fts,
            case.filter,
            case.sort,
            case.limit,
            case.offset,
            case.return_mode,
            case.count,
            case.evidence,
            by_case
                .get(&case.id)
                .map(|v| v.join(" | "))
                .unwrap_or_default()
        );
    }
    for result in &build_results {
        println!(
            "build {}: {:.3} s; parses={}; resident_bytes={}; estimated_index_bytes={}",
            result["alphabet"],
            result["build_ns"].as_u64().unwrap_or(0) as f64 / 1e9,
            result["build_parses"],
            result["resident_bytes"],
            result["estimated_index_bytes"]
        );
    }
    println!("reports: {}", result_dir.display());
    println!(
        "reproduce: cargo run --release --example bench_musenalm -- {}",
        arguments
            .iter()
            .skip(1)
            .map(|s| format!("{:?}", s))
            .collect::<Vec<_>>()
            .join(" ")
    );
    if failures != 0 {
        return Err(format!("{failures} benchmark case/profile failures"));
    }
    Ok(())
}

fn write_sample(writer: &mut BufWriter<File>, sample: TimingSample) -> Result<(), String> {
    serde_json::to_writer(&mut *writer, &sample).map_err(|e| e.to_string())?;
    writer.write_all(b"\n").map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())
}
fn command_output(command: &str, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new(command)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!("{command} failed"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().into())
}
fn schema_paths(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = fs::read_dir(root.join(".objekte/collections"))
        .map_err(|e| e.to_string())?
        .map(|e| e.map(|e| e.path()).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    Ok(paths)
}
fn hash_paths(paths: &[PathBuf]) -> Result<String, String> {
    let mut hash = Sha256::new();
    for path in paths {
        hash.update(path.to_string_lossy().as_bytes());
        hash.update(fs::read(path).map_err(|e| e.to_string())?);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn fingerprint_inventory(repo: &Repository) -> String {
    let mut hash = Sha256::new();
    for file in &repo.files.files {
        hash.update(file.relative_path.to_string_lossy().as_bytes());
        hash.update(file.revision.modified_ns.to_le_bytes());
        hash.update(file.revision.size.to_le_bytes());
    }
    format!("{:x}", hash.finalize())
}
fn resident_bytes() -> u64 {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.starts_with("VmRSS:"))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|number| number.parse::<u64>().ok())
        })
        .unwrap_or(0)
        * 1024
}
