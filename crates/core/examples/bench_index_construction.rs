//! Fresh-process, build-only construction measurement. The measured interval
//! excludes query execution, snapshot writing, and benchmark result output.

#[path = "support/musenalm.rs"]
mod musenalm;

use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use objekte_search_core::inference::InferenceMode;
use objekte_search_core::repository::{
    CollectionDefinition, Repository, RepositoryOptions, SnapshotMode,
};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::workload::BigramAlphabet;
use sha2::{Digest, Sha256};

fn main() {
    if let Err(error) = run() {
        eprintln!("construction benchmark: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let arguments = std::env::args().collect::<Vec<_>>();
    let mut args = arguments.iter().skip(1);
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let mut result_dir = None;
    let mut alphabet = BigramAlphabet::Ascii;
    let mut workers = None;
    let mut plain = false;
    let mut jsonl = false;
    let mut inference = false;
    let mut snapshot = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = PathBuf::from(args.next().ok_or("--root needs a path")?),
            "--result-dir" => {
                result_dir = Some(PathBuf::from(
                    args.next().ok_or("--result-dir needs a path")?,
                ))
            }
            "--alphabet" => {
                alphabet = match args.next().map(String::as_str) {
                    Some("ascii") => BigramAlphabet::Ascii,
                    Some("unicode") => BigramAlphabet::Unicode,
                    _ => return Err("--alphabet needs ascii or unicode".into()),
                }
            }
            "--workers" => {
                workers = Some(
                    NonZeroUsize::new(
                        args.next()
                            .ok_or("--workers needs a value")?
                            .parse::<usize>()
                            .map_err(|e| e.to_string())?,
                    )
                    .ok_or("workers must be positive")?,
                )
            }
            "--plain" => plain = true,
            "--jsonl" => jsonl = true,
            "--inference" => inference = true,
            "--snapshot" => snapshot = true,
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    if plain && jsonl {
        return Err("--plain and --jsonl select different fixtures".into());
    }
    let result_root = result_dir.ok_or("--result-dir is required")?;
    fs::create_dir_all(&result_root).map_err(|e| format!("{}: {e}", result_root.display()))?;
    let run_id = format!(
        "trial-{}-{}",
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos(),
        std::process::id(),
    );
    let result_dir = result_root.join(run_id);
    fs::create_dir(&result_dir).map_err(|e| format!("{}: {e}", result_dir.display()))?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let definitions = if plain || jsonl {
        vec![CollectionDefinition::folder(
            if jsonl { "jsonl" } else { "plain" },
            "data",
            if jsonl {
                SourceFormat::JsonLines
            } else {
                SourceFormat::Json
            },
        )]
    } else {
        musenalm::load_extended_definitions_with_schemas(&root)?.0
    };
    let mut options = RepositoryOptions {
        bigram_alphabet: alphabet,
        snapshot_mode: SnapshotMode::Disabled,
        build_workers: workers,
        ..RepositoryOptions::default()
    };
    if inference {
        options.inference.mode = InferenceMode::AutoIndex;
    }
    let before_rss = resident_bytes();
    let started = Instant::now();
    let repo = Repository::open_with_options(&root, definitions, options)?;
    let open_ns = started.elapsed().as_nanos() as u64;
    let after_build_rss = resident_bytes();
    let after_build_hwm = peak_resident_bytes();
    let startup = repo.startup().clone();
    let corpus_fingerprint = fingerprint_inventory(&repo);
    let files = repo.files().files.len();
    let records = repo
        .collections()
        .iter()
        .map(|c| c.generation.records.len())
        .sum::<usize>();
    let build_parses = repo.build_parses();
    let failed_parses = repo.build_failed_parses();
    let postings = repo
        .collections()
        .iter()
        .map(|c| c.postings.fields.len())
        .sum::<usize>();
    let sort_fields = repo
        .collections()
        .iter()
        .map(|c| c.sorting.columns.len())
        .sum::<usize>();
    let relation_edges = repo
        .collections()
        .iter()
        .flat_map(|c| &c.relations)
        .map(|r| r.edge_count())
        .sum::<usize>();
    let (inventory_heap_bytes, diagnostic_heap_bytes) = repo.files().estimated_heap_bytes();
    let source_bytes = repo
        .files()
        .files
        .iter()
        .map(|f| f.revision.size)
        .sum::<u64>();
    let snapshot_result = if snapshot {
        let path = result_dir.join("index.snapshot");
        let stats = repo.save_snapshot_with_stats(&path)?;
        let bytes = fs::metadata(&path).map_err(|e| e.to_string())?.len();
        Some(
            serde_json::json!({"total_ns":stats.total_ns,"directory_ns":stats.directory_ns,"header_ns":stats.header_ns,"inventory_ns":stats.inventory_ns,"write_ns":stats.write_ns,"sync_ns":stats.sync_ns,"rename_ns":stats.rename_ns,"directory_sync_ns":stats.directory_sync_ns,"bytes":bytes,"rss_after_write":resident_bytes(),"hwm_after_write":peak_resident_bytes()}),
        )
    } else {
        None
    };
    drop(repo);
    let after_release_rss = resident_bytes();
    let rev = command_output("git", &["rev-parse", "HEAD"])?;
    let status = command_output("git", &["status", "--porcelain"])?;
    let manifest = serde_json::json!({
        "code_revision":rev,"dirty_status":status,"command":arguments,"root":root,
        "compiler":command_output("rustc", &["--version"])?,
        "alphabet":alphabet,"requested_workers":workers.map(NonZeroUsize::get),
        "resolved_workers":startup.build_workers,"inference":inference,
        "configuration":if jsonl { "jsonl_fixture" } else if plain { "plain_unconfigured" } else { "musenalm_extended" },
        "snapshot":snapshot,"corpus_fingerprint":corpus_fingerprint,
        "source_bytes":source_bytes,"records":records,"files":files,
    });
    let sample = serde_json::json!({
        "open_ns":open_ns,"build_ns":startup.build_ns,
        "discovery_ns":startup.build_discovery_ns,
        "extraction_index_ns":startup.build_extraction_index_ns,
        "finalization_ns":startup.build_finalization_ns,
        "build_job_execution_ns":startup.build_job_execution_ns,
        "build_receive_wait_ns":startup.build_receive_wait_ns,
        "build_admission_ns":startup.build_admission_ns,
        "peak_active_jobs":startup.build_peak_active_jobs,
        "peak_ready_batches":startup.build_peak_ready_batches,
        "peak_admitted_bytes":startup.peak_build_admitted_bytes,
        "weight_limit_bytes":startup.build_weight_limit_bytes,
        "inference_build":format!("{:?}",startup.inference_build),
        "before_rss":before_rss,"after_build_rss":after_build_rss,"after_build_hwm":after_build_hwm,
        "after_release_rss":after_release_rss,"build_parses":build_parses,"failed_parses":failed_parses,"postings":postings,
        "sort_fields":sort_fields,"relation_edges":relation_edges,
        "inventory_heap_bytes":inventory_heap_bytes,"diagnostic_heap_bytes":diagnostic_heap_bytes,
        "snapshot_result":snapshot_result,
    });
    write_json(&result_dir.join("run-manifest.json"), &manifest)?;
    write_json(&result_dir.join("sample.json"), &sample)?;
    println!(
        "{}",
        serde_json::json!({"build_ns":startup.build_ns,"result_dir":result_dir})
    );
    Ok(())
}

fn write_json(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    fs::write(
        path,
        serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
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

fn fingerprint_inventory(repo: &Repository) -> String {
    let mut hash = Sha256::new();
    for file in &repo.files().files {
        hash.update(file.relative_path.to_string_lossy().as_bytes());
        hash.update(file.revision.modified_ns.to_le_bytes());
        hash.update(file.revision.size.to_le_bytes());
    }
    format!("{:x}", hash.finalize())
}

fn resident_bytes() -> u64 {
    status_bytes("VmRSS:")
}
fn peak_resident_bytes() -> u64 {
    status_bytes("VmHWM:")
}
fn status_bytes(field: &str) -> u64 {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.starts_with(field))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|n| n.parse::<u64>().ok())
        })
        .unwrap_or(0)
        * 1024
}
