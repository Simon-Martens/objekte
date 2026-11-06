#[path = "support/musenalm.rs"]
mod musenalm;

use std::path::PathBuf;
use std::time::Instant;

use objekte_search_core::repository::{Repository, RepositoryOptions, SnapshotMode};
use objekte_search_core::workload::BigramAlphabet;

fn memory_bytes(name: &str) -> Option<u64> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix(name)?
                .trim()
                .strip_suffix(" kB")?
                .trim()
                .parse::<u64>()
                .ok()
                .map(|value| value * 1024)
        })
}

fn main() {
    if let Err(error) = run() {
        eprintln!("build benchmark error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let mut alphabet = BigramAlphabet::Ascii;
    let mut snapshot_mode = SnapshotMode::Rebuild;
    let mut schema_manifest_path = None;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--root" => root = PathBuf::from(arguments.next().ok_or("--root needs a path")?),
            "--schema-manifest-path" => {
                schema_manifest_path = Some(PathBuf::from(
                    arguments.next().ok_or("--schema-manifest-path needs a path")?,
                ));
            }
            "--alphabet" => {
                alphabet = match arguments.next().ok_or("--alphabet needs a value")?.as_str() {
                    "ascii" => BigramAlphabet::Ascii,
                    "unicode" => BigramAlphabet::Unicode,
                    other => return Err(format!("unknown alphabet {other}")),
                }
            }
            "--snapshot-mode" => {
                snapshot_mode = match arguments
                    .next()
                    .ok_or("--snapshot-mode needs a value")?
                    .as_str()
                {
                    "rebuild" => SnapshotMode::Rebuild,
                    "disabled" => SnapshotMode::Disabled,
                    other => return Err(format!("unknown snapshot mode {other}")),
                }
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let (definitions, loaded_schemas) = musenalm::load_definitions_with_schemas(&root)?;
    let schema_manifest = musenalm::schema_manifest(&loaded_schemas)?;
    if let Some(path) = schema_manifest_path {
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&schema_manifest).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
    }
    let started = Instant::now();
    let repo = Repository::open_with_options(
        &root,
        definitions,
        RepositoryOptions {
            bigram_alphabet: alphabet,
            snapshot_mode,
            ..RepositoryOptions::default()
        },
    )?;
    let wall_ns = started.elapsed().as_nanos();
    let snapshot_bytes = if snapshot_mode == SnapshotMode::Rebuild {
        Some(
            std::fs::metadata(objekte_search_core::storage::snapshot_path(&root, alphabet))
                .map_err(|error| error.to_string())?
                .len(),
        )
    } else {
        None
    };
    let (inventory_heap_bytes, diagnostic_heap_bytes) = repo.files().estimated_heap_bytes();
    let registrations = repo.collections().iter().map(|c| serde_json::json!({"name":c.definition.name,"source_path":c.definition.source_path,"format":c.definition.format,"identity":format!("{:?}",c.definition.identity),"files":c.generation.files.len(),"records":c.generation.records.len(),"type_hints":c.definition.schema.fields().len(),"indexed_paths":c.postings.fields.keys().map(|p|p.join(".")).collect::<Vec<_>>(),"sort_fields":c.definition.sort_fields.iter().map(|p|p.display()).collect::<Vec<_>>(),"custom_bindings":c.definition.schema.custom_bindings().keys().map(|(p,n)|format!("{}:{n}",p.join("."))).collect::<Vec<_>>() })).collect::<Vec<_>>();
    let (index_stores, distinct_gram_keys, bigram_heap_bytes) =
        repo.collections()
            .iter()
            .fold((0usize, 0usize, 0usize), |total, collection| {
                std::iter::once(&collection.raw)
                    .chain(std::iter::once(&collection.fallback_text))
                    .chain(collection.custom_text.values())
                    .fold(total, |(stores, keys, bytes), index| {
                        (
                            stores + 1,
                            keys + index.normal.len() + index.skip.len(),
                            bytes + index.estimated_heap_bytes(),
                        )
                    })
            });
    println!(
        "{}",
        serde_json::json!({
            "alphabet": alphabet,
            "snapshot_mode": if snapshot_mode == SnapshotMode::Rebuild { "rebuild" } else { "disabled" },
            "wall_ns": wall_ns,
            "build_ns": repo.startup().build_ns,
            "discovery_ns": repo.startup().build_discovery_ns,
            "extraction_index_ns": repo.startup().build_extraction_index_ns,
            "finalization_ns": repo.startup().build_finalization_ns,
            "snapshot_write_ns": repo.startup().snapshot_write_ns,
            "build_parses": repo.build_parses(),
            "failed_parses": repo.build_failed_parses(),
            "inventory_heap_bytes": inventory_heap_bytes,
            "diagnostic_heap_bytes": diagnostic_heap_bytes,
            "index_stores": index_stores,
            "distinct_gram_keys": distinct_gram_keys,
            "bigram_heap_bytes": bigram_heap_bytes,
            "records": repo.collections().iter().map(|collection| collection.generation.records.len()).sum::<usize>(),
            "resident_bytes": memory_bytes("VmRSS:"),
            "peak_resident_bytes": memory_bytes("VmHWM:"),
            "snapshot_bytes": snapshot_bytes,
            "schema_manifest_digest": schema_manifest["sha256"],
            "schema_collections": loaded_schemas.iter().map(|schema| &schema.collection).collect::<Vec<_>>(),
            "registrations": registrations,
        })
    );
    Ok(())
}
