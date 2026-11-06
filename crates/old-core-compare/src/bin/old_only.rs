#[path = "../cases.rs"]
mod cases;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Instant;

use objekte_core::{
    BigramEncoding, CacheMode, Direction, JsonPathStep, JsonQuery, JsonQueryBackend, Options,
    Repository, Sort,
};
use objekte_search_core::workload::load_manifest;

fn main() {
    if let Err(error) = run() {
        eprintln!("old-only benchmark: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let manifest = load_manifest(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../core/benches/musenalm_queries.json"),
    )?;
    let options = Options {
        cache: CacheMode::Memory,
        bigram_encoding: BigramEncoding::Ascii,
        total_limit: 200_000,
        query_cache_bytes: 0,
        ..Options::default()
    };
    let mut repo =
        Repository::load_repository(&root, options).map_err(|error| error.to_string())?;
    if repo.model().collections.len() != 7
        || repo
            .model()
            .collections
            .values()
            .filter(|collection| collection.schema_path.is_some())
            .count()
            != 7
    {
        return Err("archived core did not load seven configured schemas".into());
    }
    let paths: BTreeMap<_, BTreeSet<_>> = repo
        .model()
        .collections
        .values()
        .map(|collection| {
            let indexed = collection
                .indexes
                .iter()
                .map(|index| index.name.clone())
                .chain(collection.advanced_index_paths.iter().map(|path| {
                    path.iter()
                        .filter_map(|step| match step {
                            JsonPathStep::Key(key) => Some(key.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(".")
                }))
                .collect();
            (collection.name.clone(), indexed)
        })
        .collect();
    repo.warm_all_bigram_indexes()
        .map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::json!({"type":"setup","core":"old","schemas":7,"indexed_paths":paths,"query_cache_bytes":0,"cache_mode":"memory","alphabet":"ascii","return_mode":"logical","page_limit":50,"sort":"record ID ascending"})
    );
    for case in manifest
        .into_iter()
        .filter(|case| cases::CASES.contains(&case.id.as_str()))
    {
        let request = JsonQuery {
            filter: case
                .filter
                .clone()
                .ok_or("missing filter")?
                .replace('"', "'"),
            collections: Some(vec![case.collection.clone()]),
            count: true,
            limit: 50,
            sort: vec![Sort {
                field: "@id".into(),
                direction: Direction::Asc,
            }],
            backend: JsonQueryBackend::Bigram,
            ..JsonQuery::default()
        };
        let first = repo
            .query_json(&request)
            .map_err(|error| error.to_string())?;
        let count = first
            .total
            .ok_or("archived query did not return an exact count")?;
        let ids: Vec<_> = first
            .records
            .iter()
            .map(|record| record.id.clone())
            .collect();
        let mut times = Vec::new();
        for _ in 0..5 {
            let started = Instant::now();
            let result = repo
                .query_json(&request)
                .map_err(|error| error.to_string())?;
            times.push(started.elapsed().as_nanos() as u64);
            if result.total != Some(count)
                || result
                    .records
                    .iter()
                    .map(|record| &record.id)
                    .collect::<Vec<_>>()
                    != ids.iter().collect::<Vec<_>>()
            {
                return Err(format!("{} changed during old-only timing", case.id));
            }
        }
        times.sort_unstable();
        println!(
            "{}",
            serde_json::json!({"type":"case","core":"old","case":case.id,"collection":case.collection,"count":count,"ids":ids,"samples_ns":times,"median_ns":times[2],"source_reads":first.metrics.source_reads,"parses":first.metrics.parses})
        );
    }
    Ok(())
}
