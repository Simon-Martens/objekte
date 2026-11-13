#[path = "support/musenalm.rs"]
mod musenalm;

use objekte_search_core::repository::{Repository, RepositoryOptions, SnapshotMode};
use objekte_search_core::workload::{BigramAlphabet, load_extended_manifest};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

fn main() -> Result<(), String> {
    let mut arguments = std::env::args().skip(1);
    let root = PathBuf::from(arguments.next().ok_or("missing corpus root")?);
    let mut selected = arguments.collect::<Vec<_>>();
    if selected.is_empty() {
        selected = ["Q115", "Q116", "Q117", "Q118", "Q120"]
            .into_iter()
            .map(str::to_owned)
            .collect();
    }
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("benches");
    let cases = load_extended_manifest(
        &base.join("musenalm_queries.json"),
        &base.join("musenalm_queries_extended.json"),
    )?;
    let definitions = musenalm::load_extended_definitions(&root)?;
    let repo = Repository::open_with_options(
        &root,
        definitions,
        RepositoryOptions {
            bigram_alphabet: BigramAlphabet::Unicode,
            snapshot_mode: SnapshotMode::Disabled,
            verified_cache_bytes: 0,
            query_workers: 4,
            ..Default::default()
        },
    )?;
    for id in selected {
        let case = cases.iter().find(|case| case.id == id).ok_or_else(|| id.clone())?;
        let query = case.query()?;
        let actual = repo.search(&query)?;
        let reference = repo.search_reference(&query)?;
        if actual.count != reference.count || actual.hits != reference.hits {
            return Err(format!("{id}: optimized result differs from reference"));
        }
        let mut hash = Sha256::new();
        for hit in &actual.hits {
            hash.update(hit.collection.as_bytes());
            hash.update([0]);
            hash.update(hit.record_id.as_bytes());
            hash.update([0]);
        }
        println!(
            "{id},count={},page_sha256={:x},opens={},bytes={},parses={},scans={}",
            actual.count.unwrap_or(0),
            hash.finalize(),
            actual.counters.files_opened,
            actual.counters.bytes_read,
            actual.counters.record_parses,
            actual.counters.scanned_records
        );
    }
    Ok(())
}
