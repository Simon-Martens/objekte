#[path = "support/musenalm.rs"]
mod musenalm;

// Readiness-specific Musenalm correctness and timing measurements. A pinned
// lifecycle view is queried after the selected construction milestone.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use objekte_search_core::inventory::SourceRevision;
use objekte_search_core::lifecycle::Index;
use objekte_search_core::repository::{RepositoryOptions, SearchHit, SnapshotMode};
use objekte_search_core::workload::{
    BigramAlphabet, Profile, TimingSample, WorkCounters, load_extended_manifest,
};
use sha2::{Digest, Sha256};

fn main() {
    if let Err(error) = run() {
        eprintln!("staged benchmark error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let arguments = std::env::args().collect::<Vec<_>>();
    let mut args = arguments.iter().skip(1);
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let mut result_dir = None;
    let mut mode = "complete".to_string();
    let mut repetitions = 3usize;
    let mut case_ids: Option<Vec<String>> = None;
    let mut profile_names = vec!["ascii-off", "ascii-warm", "unicode-off", "unicode-warm"];
    let mut query_workers = RepositoryOptions::default().query_workers;
    let mut prepared_cache_bytes = RepositoryOptions::default().prepared_cache_bytes;
    let mut warm_cache_bytes = 64 * 1024 * 1024usize;
    let mut inference = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = PathBuf::from(args.next().ok_or("--root needs a path")?),
            "--result-dir" => {
                result_dir = Some(PathBuf::from(
                    args.next().ok_or("--result-dir needs a path")?,
                ))
            }
            "--mode" => mode = args.next().ok_or("--mode needs a value")?.clone(),
            "--repetitions" => {
                repetitions = args
                    .next()
                    .ok_or("--repetitions needs a value")?
                    .parse()
                    .map_err(|error| format!("invalid repetitions: {error}"))?
            }
            "--cases" => {
                case_ids = Some(
                    args.next()
                        .ok_or("--cases needs IDs")?
                        .split(',')
                        .map(str::to_owned)
                        .collect(),
                )
            }
            "--profiles" => {
                profile_names = args
                    .next()
                    .ok_or("--profiles needs names")?
                    .split(',')
                    .collect()
            }
            "--query-workers" => {
                query_workers = args
                    .next()
                    .ok_or("--query-workers needs a number")?
                    .parse()
                    .map_err(|error| format!("invalid query workers: {error}"))?
            }
            "--prepared-cache-bytes" => {
                prepared_cache_bytes = args
                    .next()
                    .ok_or("--prepared-cache-bytes needs a number")?
                    .parse()
                    .map_err(|error| format!("invalid prepared cache size: {error}"))?
            }
            "--verified-cache-bytes" => {
                warm_cache_bytes = args
                    .next()
                    .ok_or("--verified-cache-bytes needs a number")?
                    .parse()
                    .map_err(|error| format!("invalid verified cache size: {error}"))?
            }
            "--inference" => inference = true,
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    if !matches!(
        mode.as_str(),
        "base" | "partial" | "complete" | "foreground"
    ) || repetitions == 0
    {
        return Err(
            "mode must be base, partial, complete, or foreground; repetitions must be positive"
                .into(),
        );
    }
    root = root.canonicalize().map_err(|error| error.to_string())?;
    let profiles = profile_names
        .into_iter()
        .map(|name| {
            let (alphabet, warm) = match name {
                "ascii-off" => (BigramAlphabet::Ascii, false),
                "ascii-warm" => (BigramAlphabet::Ascii, true),
                "unicode-off" => (BigramAlphabet::Unicode, false),
                "unicode-warm" => (BigramAlphabet::Unicode, true),
                _ => return Err(format!("unknown profile {name}")),
            };
            Ok((
                name.to_owned(),
                Profile {
                    alphabet,
                    verified_cache_bytes: if warm { warm_cache_bytes } else { 0 },
                    warm,
                },
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let manifest_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/musenalm_queries_extended.json");
    let baseline_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/musenalm_queries.json");
    let cases = load_extended_manifest(&baseline_path, &manifest_path)?
        .into_iter()
        .filter(|case| case_ids.as_ref().is_none_or(|ids| ids.contains(&case.id)))
        .collect::<Vec<_>>();
    if cases.is_empty()
        || case_ids
            .as_ref()
            .is_some_and(|ids| ids.len() != cases.len())
    {
        return Err("selected benchmark cases are missing".into());
    }
    let (definitions, schemas) = musenalm::load_extended_definitions_with_schemas(&root)?;
    let schema_manifest = musenalm::schema_manifest(&schemas)?;
    let rev = output("git", &["rev-parse", "HEAD"])?;
    let compiler = output("rustc", &["--version"])?;
    let result_dir = result_dir.ok_or("--result-dir is required")?;
    fs::create_dir_all(&result_dir).map_err(|error| error.to_string())?;
    write_json(&result_dir.join("schema-manifest.json"), &schema_manifest)?;
    write_json(&result_dir.join("queries.json"), &cases)?;
    let mut samples = BufWriter::new(
        File::create(result_dir.join("measurements.jsonl")).map_err(|error| error.to_string())?,
    );
    let mut csv = BufWriter::new(
        File::create(result_dir.join("query-results.csv")).map_err(|error| error.to_string())?,
    );
    let mut foreground_status = (mode == "foreground")
        .then(|| File::create(result_dir.join("foreground-status.jsonl")))
        .transpose()
        .map_err(|error| error.to_string())?
        .map(BufWriter::new);
    writeln!(csv, "case_id,profile,status,min_ns,median_ns,max_ns,samples,total_matches,returned_records,candidates,files_opened,bytes_read,record_parses,exact_posting_matches,scanned_records,residual_evaluations,prepared_plan_hits,verified_result_hits,preparation_median_ns,planning_median_ns")
        .map_err(|error| error.to_string())?;
    let mut expected: HashMap<String, (usize, Vec<SearchHit>)> = HashMap::new();
    let mut build_results = Vec::new();
    let mut failures = 0usize;
    let mut corpus_fingerprint = String::new();
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        if !profiles
            .iter()
            .any(|(_, profile)| profile.alphabet == alphabet)
        {
            continue;
        }
        let started = Instant::now();
        let mut options = RepositoryOptions {
            bigram_alphabet: alphabet,
            snapshot_mode: SnapshotMode::Disabled,
            query_workers,
            prepared_cache_bytes,
            ..RepositoryOptions::default()
        };
        if inference {
            options.inference.mode = objekte_search_core::inference::InferenceMode::AutoIndex;
        }
        let task = Index::start_staged_build(&root, definitions.clone(), options)
            .map_err(|error| error.to_string())?;
        if mode == "base" || mode == "partial" {
            task.pause_extended().map_err(|error| error.to_string())?;
        }
        let index = task
            .wait_search_ready()
            .map_err(|error| error.to_string())?;
        let search_ready_ns = started.elapsed().as_nanos() as u64;
        if corpus_fingerprint.is_empty() {
            corpus_fingerprint = fingerprint_catalogue(
                &root,
                &task.catalogue().map_err(|error| error.to_string())?,
            )?;
        }
        if mode == "complete" {
            index.wait_extended().map_err(|error| error.to_string())?;
        } else if mode == "partial" {
            task.resume_extended().map_err(|error| error.to_string())?;
            let deadline = Instant::now() + Duration::from_secs(60);
            let initial = index
                .view()
                .map_err(|error| error.to_string())?
                .capability_revision();
            while index
                .view()
                .map_err(|error| error.to_string())?
                .capability_revision()
                == initial
            {
                if Instant::now() >= deadline {
                    return Err("no partial batch was published within 60 seconds".into());
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        if mode != "foreground" {
            index.quiesce().map_err(|error| error.to_string())?;
        }
        let view = index.view().map_err(|error| error.to_string())?;
        let status = index.extended_status().map_err(|error| error.to_string())?;
        build_results.push(serde_json::json!({
            "alphabet": alphabet,
            "mode": mode,
            "inference": inference,
            "search_ready_ns": search_ready_ns,
            "measurement_start_ns": started.elapsed().as_nanos() as u64,
            "quiesced_ns": if mode == "foreground" { None } else { Some(started.elapsed().as_nanos() as u64) },
            "data_revision": view.revision().0,
            "capability_revision": view.capability_revision().0,
            "extended_phase": format!("{:?}", status.phase),
            "completed_files": status.completed_files,
            "total_files": status.total_files,
            "build_parses": index.build_parses().map_err(|error| error.to_string())?,
        }));
        for (profile_name, profile) in &profiles {
            if profile.alphabet != alphabet {
                continue;
            }
            index
                .configure_verified_cache(profile.verified_cache_bytes)
                .map_err(|error| error.to_string())?;
            for case in &cases {
                let query = case.query()?;
                if profile.warm {
                    view.search(&query)?;
                }
                let mut times = Vec::new();
                let mut counters = Vec::new();
                let mut error = None;
                for repetition in 0..repetitions {
                    let before = if mode == "foreground" {
                        Some(index.extended_status().map_err(|error| error.to_string())?)
                    } else {
                        None
                    };
                    let instant = Instant::now();
                    let searched = view.search(&query);
                    let ns = instant.elapsed().as_nanos() as u64;
                    if let (Some(before), Some(writer)) = (before, foreground_status.as_mut()) {
                        let after = index.extended_status().map_err(|error| error.to_string())?;
                        serde_json::to_writer(&mut *writer, &serde_json::json!({
                            "case_id": case.id, "profile": profile_name,
                            "repetition": repetition, "before_phase": format!("{:?}", before.phase),
                            "after_phase": format!("{:?}", after.phase),
                            "before_files": before.completed_files, "after_files": after.completed_files,
                            "total_files": after.total_files, "view_capability_revision": view.capability_revision().0,
                        })).map_err(|error| error.to_string())?;
                        writer.write_all(b"\n").map_err(|error| error.to_string())?;
                    }
                    match searched {
                        Ok(result) => {
                            let identity =
                                (result.count.ok_or("benchmark requires COUNT")?, result.hits);
                            if let Some(previous) = expected.get(&case.id) {
                                if previous != &identity {
                                    error =
                                        Some("results differ across readiness/profile".to_string());
                                }
                            } else {
                                expected.insert(case.id.clone(), identity);
                            }
                            write_sample(
                                &mut samples,
                                TimingSample {
                                    case_id: case.id.clone(),
                                    profile: *profile,
                                    repetition,
                                    status: if error.is_some() { "failed" } else { "ok" }.into(),
                                    error: error.clone(),
                                    total_ns: Some(ns),
                                    counters: Some(result.counters.clone()),
                                },
                            )?;
                            times.push(ns);
                            counters.push(result.counters);
                        }
                        Err(message) => {
                            error = Some(message.clone());
                            write_sample(
                                &mut samples,
                                TimingSample {
                                    case_id: case.id.clone(),
                                    profile: *profile,
                                    repetition,
                                    status: "failed".into(),
                                    error: Some(message),
                                    total_ns: None,
                                    counters: None,
                                },
                            )?;
                        }
                    }
                }
                if error.is_some() {
                    failures += 1;
                }
                times.sort_unstable();
                let timing = |index: usize| times.get(index).copied().unwrap_or(0);
                writeln!(
                    csv,
                    "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
                    case.id,
                    profile_name,
                    if error.is_some() { "failed" } else { "ok" },
                    timing(0),
                    timing(times.len() / 2),
                    timing(times.len().saturating_sub(1)),
                    times.len(),
                    median(&counters, |c| c.total_matches),
                    median(&counters, |c| c.returned_records),
                    median(&counters, |c| c.candidates),
                    median(&counters, |c| c.files_opened),
                    median(&counters, |c| c.bytes_read),
                    median(&counters, |c| c.record_parses),
                    median(&counters, |c| c.exact_posting_matches),
                    median(&counters, |c| c.scanned_records),
                    median(&counters, |c| c.residual_evaluations),
                    counters.iter().filter(|c| c.prepared_plan_hit).count(),
                    counters.iter().filter(|c| c.verified_result_hit).count(),
                    median(&counters, |c| c.preparation_ns),
                    median(&counters, |c| c.planning_ns),
                )
                .map_err(|error| error.to_string())?;
            }
        }
        if mode == "foreground" {
            index.quiesce().map_err(|error| error.to_string())?;
            if let Some(result) = build_results.last_mut() {
                result["quiesced_ns"] = serde_json::json!(started.elapsed().as_nanos() as u64);
                result["final_extended_phase"] = serde_json::json!(format!(
                    "{:?}",
                    index
                        .extended_status()
                        .map_err(|error| error.to_string())?
                        .phase
                ));
            }
        }
        index.shutdown().map_err(|error| error.to_string())?;
    }
    samples.flush().map_err(|error| error.to_string())?;
    csv.flush().map_err(|error| error.to_string())?;
    if let Some(writer) = foreground_status.as_mut() {
        writer.flush().map_err(|error| error.to_string())?;
    }
    let digests = expected
        .into_iter()
        .map(|(case_id, identity)| {
            (
                case_id,
                format!("{:x}", Sha256::digest(format!("{identity:?}").as_bytes())),
            )
        })
        .collect::<BTreeMap<_, _>>();
    write_json(&result_dir.join("result-digests.json"), &digests)?;
    write_json(&result_dir.join("index-results.json"), &build_results)?;
    write_json(
        &result_dir.join("run-manifest.json"),
        &serde_json::json!({
            "code_revision": rev, "compiler": compiler, "command": arguments,
            "root": root, "extended": true, "readiness": mode, "inference": inference,
            "schema_fingerprint": hash_paths(&schema_paths(&root)?)?,
            "schema_manifest_digest": schema_manifest["sha256"],
            "workload_fingerprint": hash_paths(&[manifest_path])?,
            "corpus_fingerprint": corpus_fingerprint, "repetitions": repetitions,
            "profiles": profiles, "failures": failures, "source_cache_bytes": 0,
            "decoded_cache_bytes": 0, "prepared_cache_bytes": prepared_cache_bytes,
            "query_workers": query_workers,
        }),
    )?;
    if failures != 0 {
        return Err(format!("{failures} case/profile groups failed"));
    }
    Ok(())
}

fn median(counters: &[WorkCounters], f: impl Fn(&WorkCounters) -> u64) -> u64 {
    let mut values = counters.iter().map(f).collect::<Vec<_>>();
    values.sort_unstable();
    values.get(values.len() / 2).copied().unwrap_or(0)
}

fn write_sample(writer: &mut BufWriter<File>, sample: TimingSample) -> Result<(), String> {
    serde_json::to_writer(&mut *writer, &sample).map_err(|error| error.to_string())?;
    writer.write_all(b"\n").map_err(|error| error.to_string())
}

fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<(), String> {
    fs::write(
        path,
        serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

fn output(program: &str, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!("{program} failed"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().into())
}

fn schema_paths(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = fs::read_dir(root.join(".objekte/collections"))
        .map_err(|error| error.to_string())?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    Ok(paths)
}

fn hash_paths(paths: &[PathBuf]) -> Result<String, String> {
    let mut digest = Sha256::new();
    for path in paths {
        digest.update(path.to_string_lossy().as_bytes());
        digest.update(fs::read(path).map_err(|error| error.to_string())?);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn fingerprint_catalogue(
    root: &Path,
    files: &[objekte_search_core::lifecycle::StagedFile],
) -> Result<String, String> {
    let mut digest = Sha256::new();
    for file in files {
        let path = root.join(&file.relative_path);
        let revision =
            SourceRevision::from_metadata(&fs::metadata(path).map_err(|error| error.to_string())?)?;
        digest.update(file.relative_path.to_string_lossy().as_bytes());
        digest.update(revision.modified_ns.to_le_bytes());
        digest.update(revision.size.to_le_bytes());
    }
    Ok(format!("{:x}", digest.finalize()))
}
