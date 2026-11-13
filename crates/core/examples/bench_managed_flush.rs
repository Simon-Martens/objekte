//! One fresh incremental Musenalm build followed by one explicit managed flush.
//! The timer excludes construction and includes immutable writes and manifest
//! publication. Use a new output base for every fresh-process trial.

#[path = "support/musenalm.rs"]
mod musenalm;

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Instant;

use objekte_search_core::index_storage::{
    IndexResidency, IndexStorageLocation, IndexStoragePolicy,
};
use objekte_search_core::lifecycle::Index;
use objekte_search_core::repository::{RepositoryOptions, SnapshotMode};
use objekte_search_core::source::SourceRegistry;
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
    std::fs::create_dir_all(&output).map_err(|error| error.to_string())?;
    let definitions = musenalm::load_extended_definitions(&root)?;
    let options = RepositoryOptions {
        bigram_alphabet: alphabet,
        snapshot_mode: SnapshotMode::Disabled,
        build_workers: Some(NonZeroUsize::new(32).unwrap()),
        inference: Default::default(),
        ..Default::default()
    };
    let sources = SourceRegistry::builtin();
    let prepared = IndexStoragePolicy {
        location: IndexStorageLocation::CustomDirectory(output),
        residency: IndexResidency::OnDemand,
    }
    .prepare(&root, &definitions, options, &sources)?;
    let index =
        Index::start_incremental_build_with_sources(&root, prepared.definitions, options, sources)
            .map_err(|error| error.to_string())?;
    let index = index.wait_complete().map_err(|error| error.to_string())?;
    index
        .attach_storage(prepared.resolved)
        .map_err(|error| error.to_string())?;
    let started = Instant::now();
    let revision = index.flush_storage().map_err(|error| error.to_string())?;
    let flush_ns = started.elapsed().as_nanos() as u64;
    let unchanged_started = Instant::now();
    let unchanged_revision = index.flush_storage().map_err(|error| error.to_string())?;
    let unchanged_flush_ns = unchanged_started.elapsed().as_nanos() as u64;
    if unchanged_revision != revision {
        return Err("unchanged flush changed the durable revision".into());
    }
    let status = index.storage_status().map_err(|error| error.to_string())?;
    println!(
        "{}",
        json!({
            "root": root.canonicalize().map_err(|error| error.to_string())?,
            "alphabet": alphabet,
            "flush_ns": flush_ns,
            "unchanged_flush_ns": unchanged_flush_ns,
            "revision": revision.0,
            "durable_generation": status.durable_generation,
            "managed_directory": status.managed_directory,
            "source_reads": index.status().map_err(|error| error.to_string())?.build_source_reads,
        })
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("managed flush benchmark: {error}");
        std::process::exit(1);
    }
}
