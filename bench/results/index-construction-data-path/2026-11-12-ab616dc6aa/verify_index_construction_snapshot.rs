// Copied to crates/core/examples/ for the temporary required-load verification.
#[path = "support/musenalm.rs"]
mod musenalm;

use std::collections::BTreeMap;
use std::path::PathBuf;

use objekte_search_core::repository::{
    Repository, RepositoryOptions, SnapshotMode, SnapshotOpenPolicy,
};
use objekte_search_core::workload::{self, BigramAlphabet};
use sha2::{Digest, Sha256};

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().ok_or("root required")?);
    let directory = PathBuf::from(args.next().ok_or("snapshot directory required")?);
    let alphabet = match args.next().as_deref() {
        Some("ascii") => BigramAlphabet::Ascii,
        Some("unicode") => BigramAlphabet::Unicode,
        _ => return Err("alphabet must be ascii or unicode".into()),
    };
    let definitions = musenalm::load_extended_definitions_with_schemas(&root)?.0;
    let repo = Repository::open_with_snapshot_policy(
        &root,
        definitions,
        RepositoryOptions {
            bigram_alphabet: alphabet,
            snapshot_mode: SnapshotMode::Required,
            ..Default::default()
        },
        SnapshotOpenPolicy::new(SnapshotMode::Required, Some(directory)),
    )?;
    if !repo.startup().restored || repo.build_parses() != 0 {
        return Err("required snapshot was not restored without parsing".into());
    }
    let core = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let cases = workload::load_extended_manifest(
        &core.join("benches/musenalm_queries.json"),
        &core.join("benches/musenalm_queries_extended.json"),
    )?;
    let mut digests = BTreeMap::new();
    for case in &cases {
        let result = repo.search(&case.query()?)?;
        let digest = Sha256::digest(format!("{:?}", (result.count, result.hits)).as_bytes());
        digests.insert(case.id.clone(), format!("{digest:x}"));
    }
    println!(
        "{}",
        serde_json::json!({
            "restored": repo.startup().restored,
            "build_parses": repo.build_parses(),
            "alphabet": alphabet,
            "case_count": digests.len(),
            "query_digests": digests,
        })
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("snapshot verification: {error}");
        std::process::exit(1);
    }
}
