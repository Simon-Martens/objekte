//! Measure one fresh JSONL construction with fixed explicit field policy.

use std::path::PathBuf;
use std::time::Instant;

use objekte_search_core::inference::{InferenceConfig, InferenceMode};
use objekte_search_core::model::LogicalType;
use objekte_search_core::reference::FieldDefinition;
use objekte_search_core::repository::{
    CollectionDefinition, Repository, RepositoryOptions, SnapshotMode,
};
use objekte_search_core::source::SourceFormat;
use serde_json::json;

fn resident_kib(label: &str) -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix(label)?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    })
}

fn run() -> Result<(), String> {
    let mut root = None;
    let mut mode = InferenceMode::Disabled;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = Some(PathBuf::from(args.next().ok_or("--root needs a path")?)),
            "--inference" => {
                mode = match args.next().ok_or("--inference needs a mode")?.as_str() {
                    "disabled" => InferenceMode::Disabled,
                    "report" => InferenceMode::ReportOnly,
                    "auto" => InferenceMode::AutoIndex,
                    other => return Err(format!("unknown inference mode {other}")),
                }
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let root = root.ok_or("--root is required")?;
    let mut definition = CollectionDefinition::folder("records", "data", SourceFormat::JsonLines);
    definition.schema.register_field(FieldDefinition {
        path: vec!["manual".into()],
        kind: LogicalType::String,
        required: true,
        nullable: false,
        indexed: true,
        multi: false,
        element_nullable: Vec::new(),
    })?;
    definition
        .schema
        .prohibit_auto_index(vec!["blocked".into()])?;
    let started = Instant::now();
    let repo = Repository::open_with_options(
        &root,
        vec![definition],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            inference: InferenceConfig {
                mode,
                ..Default::default()
            },
            ..Default::default()
        },
    )?;
    let open_ns = started.elapsed().as_nanos() as u64;
    let collection = &repo.collections()[0];
    let report = repo.inference_report("records");
    let indexes = collection.inference.as_ref().map(|state| {
        let mut summaries = state
            .indexes
            .iter()
            .map(|(path, index)| {
                json!({
                    "path": path,
                    "keys": index.values.len(),
                    "memberships": index.values.values().map(|slots| slots.len()).sum::<u64>(),
                    "covered_records": index.covered_records,
                })
            })
            .collect::<Vec<_>>();
        summaries.sort_by(|a, b| a["path"].to_string().cmp(&b["path"].to_string()));
        summaries
    });
    println!(
        "{}",
        json!({
            "mode": mode,
            "root": root.canonicalize().map_err(|error| error.to_string())?,
            "files": repo.files().files.len(),
            "records": collection.generation.records.len(),
            "build_parses": repo.build_parses(),
            "open_ns": open_ns,
            "build_ns": repo.startup().build_ns,
            "discovery_ns": repo.startup().build_discovery_ns,
            "extraction_index_ns": repo.startup().build_extraction_index_ns,
            "finalization_ns": repo.startup().build_finalization_ns,
            "workers": repo.startup().build_workers,
            "peak_active_jobs": repo.startup().build_peak_active_jobs,
            "receive_wait_ns": repo.startup().build_receive_wait_ns,
            "admission_ns": repo.startup().build_admission_ns,
            "resident_kib": resident_kib("VmRSS:"),
            "peak_resident_kib": resident_kib("VmHWM:"),
            "inference_report": report,
            "indexes": indexes,
        })
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("inference fixture benchmark: {error}");
        std::process::exit(1);
    }
}
