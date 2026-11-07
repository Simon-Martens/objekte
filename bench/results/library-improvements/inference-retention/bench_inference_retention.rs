//! Focused, fixed-corpus inference construction probe.

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Instant;

use objekte_search_core::inference::{InferenceConfig, InferenceMode};
use objekte_search_core::model::LogicalType;
use objekte_search_core::reference::FieldDefinition;
use objekte_search_core::repository::{
    CollectionDefinition, Repository, RepositoryOptions, SnapshotMode,
};
use objekte_search_core::source::SourceFormat;
use sha2::{Digest, Sha256};

const FILES: usize = 1024;
const RECORDS_PER_FILE: usize = 32;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/inference-retention-fixture")
}

fn peak_rss_kib() -> Option<u64> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

fn prepare() -> Result<(), String> {
    let root = root();
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).map_err(|error| error.to_string())?;
    for file in 0..FILES {
        let mut contents = String::new();
        for record in 0..RECORDS_PER_FILE {
            let id = file * RECORDS_PER_FILE + record;
            contents.push_str(&format!(
                "{{\"manual\":\"m{id:05}\",\"blocked\":\"b{id:05}\",\"kind\":\"{}\"}}\n",
                id % 4
            ));
        }
        std::fs::write(root.join(format!("data/{file:04}.jsonl")), contents)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn measure() -> Result<(), String> {
    let root = root();
    let mut definition = CollectionDefinition::folder("items", "data", SourceFormat::JsonLines);
    definition.schema.register_field(FieldDefinition {
        path: vec!["manual".into()],
        kind: LogicalType::String,
        required: false,
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
            build_workers: NonZeroUsize::new(4),
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            ..Default::default()
        },
    )?;
    let wall_ms = started.elapsed().as_secs_f64() * 1_000.0;
    let collection = &repo.collections()[0];
    let inference = collection.inference.as_ref().ok_or("missing inference")?;
    let mut hash = Sha256::new();
    let mut keys = inference.indexes.keys().collect::<Vec<_>>();
    keys.sort();
    for key in keys {
        hash.update(bincode::serialize(key).map_err(|error| error.to_string())?);
        hash.update(
            bincode::serialize(&inference.indexes[key]).map_err(|error| error.to_string())?,
        );
    }
    let selected = inference
        .report
        .fields
        .iter()
        .map(|field| {
            serde_json::json!({
                "path": field.path,
                "status": format!("{:?}", field.index_status),
                "complete": field.complete,
                "short_distinct": field.short_string_distinct_values,
                "present": field.present_records,
                "reason": field.reason,
            })
        })
        .collect::<Vec<_>>();
    println!(
        "{}",
        serde_json::json!({
            "wall_ms": wall_ms,
            "build_ms": repo.startup().build_ns as f64 / 1_000_000.0,
            "discovery_ms": repo.startup().build_discovery_ns as f64 / 1_000_000.0,
            "extraction_index_ms": repo.startup().build_extraction_index_ns as f64 / 1_000_000.0,
            "finalization_ms": repo.startup().build_finalization_ns as f64 / 1_000_000.0,
            "admission_ms": repo.startup().build_admission_ns as f64 / 1_000_000.0,
            "inference_merge_ms": repo.startup().inference_build.merge_ns as f64 / 1_000_000.0,
            "inference_finish_ms": repo.startup().inference_build.finish_ns as f64 / 1_000_000.0,
            "inference_observed_records": repo.startup().inference_build.observed_records,
            "inference_merged_batches": repo.startup().inference_build.merged_batches,
            "inference_cumulative_budget_bytes": repo.startup().inference_build.cumulative_budget_bytes,
            "inference_unique_path_bytes": repo.startup().inference_build.unique_path_bytes,
            "inference_repeated_batch_path_bytes": repo.startup().inference_build.repeated_batch_path_bytes,
            "inference_retained_manual_summary_bytes": repo.startup().inference_build.retained_manual_summary_bytes,
            "peak_rss_kib": peak_rss_kib(),
            "files": repo.files().files.len(),
            "records": collection.generation.records.len(),
            "parses": repo.build_parses(),
            "failed_parses": repo.build_failed_parses(),
            "inference_complete": inference.report.complete,
            "admitted_paths": inference.report.admitted_paths,
            "dropped_paths": inference.report.dropped_paths,
            "inferred_index_sha256": format!("{:x}", hash.finalize()),
            "fields": selected,
        })
    );
    Ok(())
}

fn main() {
    let result = match std::env::args().nth(1).as_deref() {
        Some("prepare") => prepare(),
        Some("measure") => measure(),
        _ => Err("expected prepare or measure".into()),
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
