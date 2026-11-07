//! Temporary focused construction measurements for B4 and R5 review.

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Instant;

use objekte_search_core::model::LogicalType;
use objekte_search_core::reference::{FieldDefinition, Schema};
use objekte_search_core::repository::{
    CollectionDefinition, Repository, RepositoryOptions, SnapshotMode,
};
use objekte_search_core::source::SourceFormat;

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

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let scenario = args.next().ok_or("expected sparse, shared, or schema")?;
    let first: usize = args
        .next()
        .ok_or("expected size")?
        .parse()
        .map_err(|error| format!("size: {error}"))?;
    let second: usize = args
        .next()
        .unwrap_or_else(|| "1".into())
        .parse()
        .map_err(|error| format!("files per collection: {error}"))?;

    if scenario == "schema" {
        let mut schema = Schema::default();
        let started = Instant::now();
        for index in 0..first {
            schema.register_field(FieldDefinition {
                path: vec![format!("date_{index}")],
                kind: LogicalType::Date,
                required: false,
                nullable: true,
                indexed: false,
                multi: false,
                element_nullable: Vec::new(),
            })?;
        }
        println!(
            "{}",
            serde_json::json!({
                "scenario": "schema",
                "fields": first,
                "register_ms": started.elapsed().as_secs_f64() * 1_000.0,
                "peak_rss_kib": peak_rss_kib(),
            })
        );
        return Ok(());
    }

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../target/construction-evaluation-{scenario}-{first}-{second}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    let mut definitions = Vec::with_capacity(first);
    let mut input_bytes = 0usize;
    match scenario.as_str() {
        "sparse" => {
            for collection in 0..first {
                let name = format!("collection_{collection:04}");
                let directory = root.join(&name);
                std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
                for file in 0..second {
                    let contents = format!("{{\"value\":{file}}}");
                    input_bytes += contents.len();
                    std::fs::write(directory.join(format!("{file:05}.json")), contents)
                        .map_err(|error| error.to_string())?;
                }
                definitions.push(CollectionDefinition::folder(name.clone(), name, SourceFormat::Json));
            }
        }
        "shared" => {
            let directory = root.join("shared");
            std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
            for file in 0..second {
                let contents = format!("{{\"value\":{file}}}");
                input_bytes += contents.len();
                std::fs::write(directory.join(format!("{file:05}.json")), contents)
                    .map_err(|error| error.to_string())?;
            }
            for collection in 0..first {
                definitions.push(CollectionDefinition::folder(
                    format!("collection_{collection:04}"),
                    "shared",
                    SourceFormat::Json,
                ));
            }
        }
        _ => return Err("expected sparse, shared, or schema".into()),
    }
    let started = Instant::now();
    let repo = Repository::open_with_options(
        &root,
        definitions,
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            build_workers: NonZeroUsize::new(1),
            ..Default::default()
        },
    )?;
    let wall_ms = started.elapsed().as_secs_f64() * 1_000.0;
    let mapping_bytes: usize = repo
        .collections()
        .iter()
        .map(|collection| {
            collection.generation.file_records.offsets.capacity() * 4
                + collection.generation.file_records.slots.capacity() * 4
        })
        .sum();
    let inventory_bytes = repo.files().estimated_heap_bytes();
    let records: usize = repo
        .collections()
        .iter()
        .map(|collection| collection.generation.records.len())
        .sum();
    println!(
        "{}",
        serde_json::json!({
            "scenario": scenario,
            "collections": first,
            "files_per_collection": second,
            "files": repo.files().files.len(),
            "records": records,
            "input_bytes": input_bytes,
            "record_parses": repo.build_parses(),
            "wall_ms": wall_ms,
            "build_ms": repo.startup().build_ns as f64 / 1_000_000.0,
            "discovery_ms": repo.startup().build_discovery_ns as f64 / 1_000_000.0,
            "extraction_ms": repo.startup().build_extraction_index_ns as f64 / 1_000_000.0,
            "finalization_ms": repo.startup().build_finalization_ns as f64 / 1_000_000.0,
            "mapping_bytes": mapping_bytes,
            "inventory_bytes": inventory_bytes.0,
            "diagnostic_bytes": inventory_bytes.1,
            "peak_rss_kib": peak_rss_kib(),
        })
    );
    std::fs::remove_dir_all(root).map_err(|error| error.to_string())?;
    Ok(())
}
