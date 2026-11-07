//! One fresh in-memory Musenalm construction followed by one explicit save.
//! Run the binary repeatedly in alternating old/new process blocks.

#[path = "support/musenalm.rs"]
mod musenalm;

use std::path::PathBuf;
use std::time::Instant;

use objekte_search_core::repository::{Repository, RepositoryOptions, SnapshotMode};
use objekte_search_core::workload::BigramAlphabet;
use serde_json::json;

fn run() -> Result<(), String> {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let mut output = None;
    let mut alphabet = BigramAlphabet::Ascii;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = PathBuf::from(args.next().ok_or("--root needs a path")?),
            "--output" => output = Some(PathBuf::from(args.next().ok_or("--output needs a path")?)),
            "--alphabet" => {
                alphabet = match args.next().ok_or("--alphabet needs a value")?.as_str() {
                    "ascii" => BigramAlphabet::Ascii,
                    "unicode" => BigramAlphabet::Unicode,
                    other => return Err(format!("unknown alphabet {other}")),
                }
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let output = output.ok_or("--output is required")?;
    let definitions = musenalm::load_definitions(&root)?;
    let definition_summary = definitions.iter().map(|definition| json!({
        "name": definition.name,
        "source_path": definition.source_path,
        "indexed_fields": definition.schema.fields().values().filter(|field| field.indexed).count(),
    })).collect::<Vec<_>>();
    let started = Instant::now();
    let repo = Repository::open_with_options(
        &root,
        definitions,
        RepositoryOptions {
            bigram_alphabet: alphabet,
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    )?;
    let open_ns = started.elapsed().as_nanos() as u64;
    let save = repo.save_snapshot_with_stats(&output)?;
    let bytes = std::fs::metadata(&output)
        .map_err(|error| error.to_string())?
        .len();
    let sections = save
        .sections
        .iter()
        .map(|section| {
            json!({
                "id": section.id,
                "serialize_ns": section.serialize_ns,
                "checksum_ns": section.checksum_ns,
                "write_ns": section.write_ns,
                "payload_bytes": section.payload_bytes,
            })
        })
        .collect::<Vec<_>>();
    println!(
        "{}",
        json!({
            "root": root.canonicalize().map_err(|error| error.to_string())?,
            "output": output,
            "alphabet": alphabet,
            "definitions": definition_summary,
            "files": repo.files().files.len(),
            "records": repo.collections().iter().map(|collection| collection.generation.records.len()).sum::<usize>(),
            "build_parses": repo.build_parses(),
            "open_ns": open_ns,
            "build_ns": repo.startup().build_ns,
            "save_total_ns": save.total_ns,
            "save_directory_ns": save.directory_ns,
            "save_header_ns": save.header_ns,
            "save_inventory_ns": save.inventory_ns,
            "save_write_ns": save.write_ns,
            "save_sync_ns": save.sync_ns,
            "save_rename_ns": save.rename_ns,
            "save_bytes": bytes,
            "sections": sections,
        })
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("snapshot benchmark: {error}");
        std::process::exit(1);
    }
}
