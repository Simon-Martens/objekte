#[path = "../cases.rs"]
mod cases;
#[path = "../../../core/examples/support/musenalm.rs"]
mod musenalm;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Instant;

use objekte_search_core::query::Query;
use objekte_search_core::repository::{Repository, RepositoryOptions};
use objekte_search_core::workload::{BigramAlphabet, ReturnMode, load_manifest};

fn main() {
    if let Err(error) = run() {
        eprintln!("new-only benchmark: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let manifest = load_manifest(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../core/benches/musenalm_queries.json"),
    )?;
    let repo = Repository::open_with_options(
        &root,
        musenalm::load_definitions(&root)?,
        RepositoryOptions {
            bigram_alphabet: BigramAlphabet::Ascii,
            verified_cache_bytes: 0,
            ..RepositoryOptions::default()
        },
    )?;
    if repo.collections.len() != 7 {
        return Err("new core did not load seven configured schemas".into());
    }
    let paths: BTreeMap<_, BTreeSet<_>> = repo
        .collections
        .iter()
        .map(|collection| {
            (
                collection.definition.name.clone(),
                collection
                    .postings
                    .fields
                    .keys()
                    .map(|path| path.join("."))
                    .collect(),
            )
        })
        .collect();
    println!(
        "{}",
        serde_json::json!({"type":"setup","core":"new","schemas":7,"indexed_paths":paths,"verified_cache_bytes":0,"alphabet":"ascii","return_mode":"logical","page_limit":50,"sort":"record ID ascending","query_workers":repo.options.query_workers})
    );
    for case in manifest
        .into_iter()
        .filter(|case| cases::CASES.contains(&case.id.as_str()))
    {
        let mut query = Query::from_workload(&case)?;
        query.options.return_mode = ReturnMode::Logical;
        query.options.evidence = false;
        let first = repo.search(&query)?;
        let ids: Vec<_> = first.hits.iter().map(|hit| hit.record_id.clone()).collect();
        let mut times = Vec::new();
        for _ in 0..5 {
            let started = Instant::now();
            let result = repo.search(&query)?;
            times.push(started.elapsed().as_nanos() as u64);
            if result.count != first.count
                || result
                    .hits
                    .iter()
                    .map(|hit| &hit.record_id)
                    .collect::<Vec<_>>()
                    != ids.iter().collect::<Vec<_>>()
            {
                return Err(format!("{} changed during new-only timing", case.id));
            }
        }
        times.sort_unstable();
        println!(
            "{}",
            serde_json::json!({"type":"case","core":"new","case":case.id,"collection":case.collection,"count":first.count,"ids":ids,"samples_ns":times,"median_ns":times[2],"files_opened":first.counters.files_opened,"parses":first.counters.record_parses})
        );
    }
    Ok(())
}
