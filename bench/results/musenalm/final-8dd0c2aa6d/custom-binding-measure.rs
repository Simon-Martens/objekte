// Copy this file to core/examples/measure_custom_musenalm.rs, then run it with
// `cargo run --release --example measure_custom_musenalm -- with 1` or `without 1`.
#[path = "support/musenalm.rs"]
mod musenalm;

use std::path::PathBuf;
use std::time::Instant;

use objekte_search_core::repository::{Repository, RepositoryOptions, SnapshotMode};

fn status_bytes(name: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.starts_with(name))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|v| v.parse::<u64>().ok())
        })
        .unwrap_or(0)
        * 1024
}

fn run() -> Result<(), String> {
    let args = std::env::args().collect::<Vec<_>>();
    let variant = args.get(1).ok_or("expected with or without")?;
    if variant != "with" && variant != "without" {
        return Err("expected with or without".into());
    }
    let run = args.get(2).ok_or("expected run number")?;
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../examples/musenalm");
    let mut definitions = musenalm::load_definitions(&root)?;
    if variant == "without" {
        definitions
            .iter_mut()
            .find(|d| d.name == "agents")
            .ok_or("missing agents collection")?
            .schema
            .custom
            .clear();
    }
    let started = Instant::now();
    let repo = Repository::open_with_options(
        root,
        definitions,
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..RepositoryOptions::default()
        },
    )?;
    let wall_ns = started.elapsed().as_nanos() as u64;
    let custom_bytes: usize = repo
        .collections
        .iter()
        .find(|c| c.definition.name == "agents")
        .ok_or("missing agents state")?
        .custom_text
        .values()
        .map(|index| index.estimated_heap_bytes())
        .sum();
    println!(
        "{}",
        serde_json::json!({
            "run": run,
            "variant": variant,
            "build_ns": repo.startup.build_ns,
            "wall_ns": wall_ns,
            "build_parses": repo.build_parses,
            "custom_index_estimated_bytes": custom_bytes,
            "resident_bytes": status_bytes("VmRSS:"),
            "peak_resident_bytes": status_bytes("VmHWM:"),
            "snapshot_mode": "disabled",
            "alphabet": "ascii"
        })
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
