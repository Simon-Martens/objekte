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
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--root" => root = PathBuf::from(arguments.next().ok_or("--root needs a path")?),
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
    let definitions = musenalm::load_definitions(&root)?;
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
    println!(
        "{}",
        serde_json::json!({
            "alphabet": alphabet,
            "snapshot_mode": if snapshot_mode == SnapshotMode::Rebuild { "rebuild" } else { "disabled" },
            "wall_ns": wall_ns,
            "build_ns": repo.startup().build_ns,
            "snapshot_write_ns": repo.startup().snapshot_write_ns,
            "build_parses": repo.build_parses(),
            "records": repo.collections().iter().map(|collection| collection.generation.records.len()).sum::<usize>(),
            "resident_bytes": memory_bytes("VmRSS:"),
            "peak_resident_bytes": memory_bytes("VmHWM:"),
            "snapshot_bytes": snapshot_bytes,
        })
    );
    Ok(())
}
