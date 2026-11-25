//! Measure one fresh JSONL construction with fixed explicit field policy.

use std::path::PathBuf;
use std::time::Instant;

use objekte_search_core::inference::{InferenceConfig, InferenceMode};
use objekte_search_core::lifecycle::session::IndexedRepository;
use objekte_search_core::model::LogicalType;
use objekte_search_core::repository::configuration::{CollectionDefinition, RepositoryOptions};
use objekte_search_core::schema::FieldDefinition;
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
    let mut definition =
        CollectionDefinition::folder("records", "data", objekte_loader_jsonl::JSON_LINES);
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
    let index = objekte_search_core::lifecycle::Index::start_build(
        &root,
        vec![definition],
        RepositoryOptions {
            inference: InferenceConfig {
                mode,
                ..Default::default()
            },
            ..Default::default()
        },
        objekte_cli::loader::installed_sources(),
    )
    .and_then(|task| task.wait_complete())
    .map_err(|error| error.to_string())?;
    let index = IndexedRepository::new(index).map_err(|error| error.to_string())?;
    let open_ns = started.elapsed().as_nanos() as u64;
    let collection = &index.collections()[0];
    let view = index.index().view().map_err(|error| error.to_string())?;
    let report = index.inference_report("records");
    let indexes = collection.base_inferred_postings().map(|postings| {
        let mut summaries = postings
            .map(|index| {
                json!({
                    "path": index.path,
                    "keys": index.keys,
                    "memberships": index.memberships,
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
            "files": view.file_metadata_count(),
            "records": collection.base_record_count(),
            "build_parses": index.build_parses(),
            "open_ns": open_ns,
            "build_ns": index.startup().build_ns,
            "discovery_ns": index.startup().build_discovery_ns,
            "extraction_index_ns": index.startup().build_extraction_index_ns,
            "finalization_ns": index.startup().build_finalization_ns,
            "workers": index.startup().build_workers,
            "peak_active_jobs": index.startup().build_peak_active_jobs,
            "receive_wait_ns": index.startup().build_receive_wait_ns,
            "admission_ns": index.startup().build_admission_ns,
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
