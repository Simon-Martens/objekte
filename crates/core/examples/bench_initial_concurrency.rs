//! Fresh-process construction with a fixed foreground query loop or a pinned
//! early view. This complements `bench_index_construction`'s no-query trials.

#[path = "support/musenalm.rs"]
mod musenalm;

use std::fs;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use objekte_search_core::lifecycle::{Index, IndexView, InitialBuildPhase, InitialBuildTask};
use objekte_search_core::lifecycle_caller::IndexedRepository;
use objekte_search_core::query::Query;
use objekte_search_core::repository::{RepositoryOptions, SnapshotMode};
use objekte_search_core::workload::{load_extended_manifest, BigramAlphabet};
use serde_json::json;

fn main() {
    if let Err(error) = run() {
        eprintln!("initial concurrency benchmark: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut root = None;
    let mut output = None;
    let mut mode = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = Some(PathBuf::from(args.next().ok_or("--root needs a path")?)),
            "--output" => output = Some(PathBuf::from(args.next().ok_or("--output needs a path")?)),
            "--mode" => mode = Some(args.next().ok_or("--mode needs a value")?),
            _ => return Err(format!("unknown argument: {arg}")),
        }
    }
    let root = root
        .ok_or("--root is required")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let output = output.ok_or("--output is required")?;
    let mode = mode.ok_or("--mode is required")?;
    if !matches!(mode.as_str(), "idle" | "foreground" | "held-view") {
        return Err("--mode needs idle, foreground or held-view".into());
    }
    let definitions = musenalm::load_extended_definitions_with_schemas(&root)?.0;
    let options = RepositoryOptions {
        bigram_alphabet: BigramAlphabet::Ascii,
        build_workers: Some(NonZeroUsize::new(32).unwrap()),
        snapshot_mode: SnapshotMode::Disabled,
        ..RepositoryOptions::default()
    };
    let started = Instant::now();
    let task =
        Index::start_incremental_build(&root, definitions, options).map_err(|e| e.to_string())?;
    let query_thread = if mode == "foreground" {
        let queries = workload_queries()?;
        let worker = task.clone();
        Some(thread::spawn(move || query_loop(worker, queries)))
    } else {
        None
    };
    let held_view = if mode == "held-view" {
        Some(wait_first_batch(&task)?)
    } else {
        None
    };
    let index = task.wait_complete().map_err(|e| e.to_string())?;
    let build_elapsed_ns = task
        .status()
        .map_err(|e| e.to_string())?
        .completion_elapsed_ns
        .unwrap_or_else(|| started.elapsed().as_nanos() as u64);
    let query = query_thread
        .map(|handle| {
            handle
                .join()
                .map_err(|_| "query worker panicked".to_owned())?
        })
        .transpose()?;
    let status = index.status().map_err(|e| e.to_string())?;
    let startup = IndexedRepository::new(index.clone())
        .map_err(|e| e.to_string())?
        .startup()
        .clone();
    let after_build_rss = resident_bytes();
    let after_build_hwm = peak_resident_bytes();
    let held_revision = held_view
        .as_ref()
        .map(IndexView::revision)
        .map(|revision| revision.0);
    drop(held_view);
    let after_held_release_rss = resident_bytes();
    let result = json!({
        "mode": mode,
        "build_ns": build_elapsed_ns,
        "wall_ns": started.elapsed().as_nanos(),
        "after_build_rss": after_build_rss,
        "after_build_hwm": after_build_hwm,
        "after_held_release_rss": after_held_release_rss,
        "build_parses": status.build_parses,
        "build_workers": startup.build_workers,
        "peak_active_jobs": startup.build_peak_active_jobs,
        "job_execution_ns": startup.build_job_execution_ns,
        "receive_wait_ns": startup.build_receive_wait_ns,
        "admission_ns": startup.build_admission_ns,
        "extraction_index_ns": startup.build_extraction_index_ns,
        "finalization_ns": startup.build_finalization_ns,
        "source_reads": status.build_source_reads,
        "source_bytes_read": status.build_source_bytes_read,
        "files": status.indexed_files,
        "records": status.live_records,
        "held_revision": held_revision,
        "query": query,
        "code_revision": command_output("git", &["rev-parse", "HEAD"]),
        "root": root,
    });
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(
        &output,
        serde_json::to_vec_pretty(&result).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    println!("{}", result);
    Ok(())
}

fn workload_queries() -> Result<Vec<Query>, String> {
    let core = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let cases = load_extended_manifest(
        &core.join("benches/musenalm_queries.json"),
        &core.join("benches/musenalm_queries_extended.json"),
    )?;
    ["Q108", "Q116", "Q120"]
        .iter()
        .map(|id| {
            cases
                .iter()
                .find(|case| case.id == *id)
                .ok_or(format!("missing {id}"))?
                .query()
        })
        .collect()
}

fn wait_first_batch(task: &InitialBuildTask) -> Result<IndexView, String> {
    let mut snapshot = task.snapshot().map_err(|e| e.to_string())?;
    loop {
        if snapshot.status.searchable_batches > 0 {
            return snapshot.view.ok_or("batch has no view".into());
        }
        if matches!(
            snapshot.status.phase,
            InitialBuildPhase::Complete | InitialBuildPhase::Cancelled | InitialBuildPhase::Failed
        ) {
            return Err("construction stopped before first batch".into());
        }
        snapshot = task.wait_next(snapshot.cursor).map_err(|e| e.to_string())?;
    }
}

fn query_loop(task: InitialBuildTask, queries: Vec<Query>) -> Result<serde_json::Value, String> {
    let mut latencies = Vec::new();
    let mut source_opens = 0u64;
    let mut first_result_ns = None;
    let started = Instant::now();
    loop {
        let snapshot = task.snapshot().map_err(|e| e.to_string())?;
        if snapshot.status.construction_complete {
            break;
        }
        if matches!(
            snapshot.status.phase,
            InitialBuildPhase::Cancelled | InitialBuildPhase::Failed
        ) {
            return Err(snapshot
                .status
                .error
                .unwrap_or("construction stopped".into()));
        }
        if snapshot.status.searchable_batches > 0 {
            let view = snapshot.view.ok_or("batch has no view")?;
            for query in &queries {
                let before = Instant::now();
                let result = view.search_available(query)?;
                latencies.push(before.elapsed().as_nanos() as u64);
                source_opens += result.result.counters.files_opened;
                if !result.result.hits.is_empty() && first_result_ns.is_none() {
                    first_result_ns = Some(started.elapsed().as_nanos() as u64);
                }
            }
        }
        // A fixed three-query cycle with a 2 ms pause provides repeatable
        // foreground pressure without turning this into a busy-spin test.
        thread::sleep(Duration::from_millis(2));
    }
    latencies.sort_unstable();
    let sample = |fraction: f64| -> Option<u64> {
        (!latencies.is_empty())
            .then(|| latencies[((latencies.len() - 1) as f64 * fraction) as usize])
    };
    Ok(json!({
        "queries": latencies.len(),
        "median_ns": sample(0.5),
        "p95_ns": sample(0.95),
        "max_ns": latencies.last(),
        "first_nonempty_result_ns_from_worker_start": first_result_ns,
        "source_opens": source_opens,
    }))
}

fn resident_bytes() -> Option<u64> {
    proc_kib("VmRSS:")
}
fn peak_resident_bytes() -> Option<u64> {
    proc_kib("VmHWM:")
}
fn proc_kib(key: &str) -> Option<u64> {
    fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix(key)?
                .split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()
        })
        .map(|kib| kib * 1024)
}
fn command_output(command: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(command)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
