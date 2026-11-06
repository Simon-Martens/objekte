#[path = "../../../core/examples/support/musenalm.rs"]
mod musenalm;

use std::path::PathBuf;
use std::time::Instant;

use objekte_core::{
    BigramEncoding, CacheMode, Direction, JsonQuery, JsonQueryBackend, Options,
    Repository as OldRepository, Sort,
};
use objekte_search_core::query::Query;
use objekte_search_core::repository::{Repository as NewRepository, RepositoryOptions};
use objekte_search_core::workload::{BigramAlphabet, ReturnMode, load_manifest};

const CASES: &[&str] = &[
    "Q031", "Q032", "Q037", "Q038", "Q042", "Q043", "Q044", "Q045", "Q046", "Q049", "Q050", "Q052",
    "Q053", "Q055", "Q057", "Q058", "Q059", "Q060", "Q061", "Q062", "Q065", "Q071", "Q074", "Q081",
    "Q084", "Q088", "Q093", "Q096", "Q101", "Q103", "Q113", "Q117", "Q120", "Q121",
];

fn main() {
    if let Err(error) = run() {
        eprintln!("comparison error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let mut repetitions = 5usize;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = PathBuf::from(args.next().ok_or("--root needs path")?),
            "--repetitions" => {
                repetitions = args
                    .next()
                    .ok_or("--repetitions needs integer")?
                    .parse()
                    .map_err(|error| format!("invalid repetitions: {error}"))?
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if repetitions < 3 {
        return Err("at least three repetitions are needed".into());
    }
    let manifest = load_manifest(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../core/benches/musenalm_queries.json"),
    )?;
    let old_options = Options {
        cache: CacheMode::Memory,
        bigram_encoding: BigramEncoding::Ascii,
        total_limit: 200_000,
        query_cache_bytes: 0,
        ..Options::default()
    };
    let old_started = Instant::now();
    let mut old =
        OldRepository::load_repository(&root, old_options).map_err(|error| error.to_string())?;
    let old_collection_count = old.model().collections.len();
    if old_collection_count != 7 {
        return Err(format!(
            "old core found {old_collection_count} collections, expected 7"
        ));
    }
    let old_schema_count = old
        .model()
        .collections
        .values()
        .filter(|collection| collection.schema_path.is_some())
        .count();
    if old_schema_count != 7 {
        return Err(format!(
            "old core loaded {old_schema_count} schemas, expected 7"
        ));
    }
    let old_index_paths: usize = old
        .model()
        .collections
        .values()
        .map(|collection| collection.indexes.len() + collection.advanced_index_paths.len())
        .sum();
    old.warm_all_bigram_indexes()
        .map_err(|error| error.to_string())?;
    let old_setup_ns = old_started.elapsed().as_nanos();
    let new_started = Instant::now();
    let new = NewRepository::open_with_options(
        &root,
        musenalm::load_definitions(&root)?,
        RepositoryOptions {
            bigram_alphabet: BigramAlphabet::Ascii,
            verified_cache_bytes: 0,
            ..RepositoryOptions::default()
        },
    )?;
    let new_collection_count = new.collections.len();
    if new_collection_count != 7 {
        return Err(format!(
            "new core found {new_collection_count} collections, expected 7"
        ));
    }
    let new_index_paths: usize = new
        .collections
        .iter()
        .map(|collection| collection.postings.fields.len())
        .sum();
    let new_setup_ns = new_started.elapsed().as_nanos();
    println!(
        "{}",
        serde_json::json!({
            "type":"setup", "old_setup_ns":old_setup_ns, "new_setup_ns":new_setup_ns,
            "old_schema_mode":"configured", "new_schema_mode":"configured",
            "old_collections":old_collection_count, "new_collections":new_collection_count,
        "old_schemas":old_schema_count,"old_index_paths":old_index_paths,"new_index_paths":new_index_paths,
            "old_query_cache_bytes":0, "new_verified_cache_bytes":0,
            "old_cache_mode":"memory", "new_return_mode":"logical", "page_limit":50,
            "new_query_workers":new.options.query_workers,
            "sort":"record ID ascending", "repetitions":repetitions,
        })
    );
    for case in manifest
        .into_iter()
        .filter(|case| CASES.contains(&case.id.as_str()))
    {
        let mut new_query = Query::from_workload(&case)?;
        new_query.options.return_mode = ReturnMode::Logical;
        new_query.options.evidence = false;
        let old_query = JsonQuery {
            filter: case
                .filter
                .clone()
                .ok_or("comparison filter missing")?
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
        let old_first = match old.query_json(&old_query) {
            Ok(result) => result,
            Err(error) => {
                println!(
                    "{}",
                    serde_json::json!({"type":"skip","case":case.id,"reason":format!("old query error: {error}")})
                );
                continue;
            }
        };
        let new_first = new.search(&new_query)?;
        let old_ids: Vec<_> = old_first
            .records
            .iter()
            .map(|record| record.id.as_str())
            .collect();
        let new_ids: Vec<_> = new_first
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect();
        if old_first.total != Some(new_first.count) || old_ids != new_ids {
            println!(
                "{}",
                serde_json::json!({
                    "type":"skip", "case":case.id, "reason":"count or ordered page differs",
                    "old_count":old_first.total,"new_count":new_first.count,
                    "old_ids":old_ids,"new_ids":new_ids,
                })
            );
            continue;
        }
        let mut old_ns = Vec::with_capacity(repetitions);
        let mut new_ns = Vec::with_capacity(repetitions);
        for repetition in 0..repetitions {
            if repetition % 2 == 0 {
                let started = Instant::now();
                let result = old
                    .query_json(&old_query)
                    .map_err(|error| error.to_string())?;
                old_ns.push(started.elapsed().as_nanos() as u64);
                if result.total != old_first.total {
                    return Err(format!("{} old count changed", case.id));
                }
                if result
                    .records
                    .iter()
                    .map(|record| record.id.as_str())
                    .collect::<Vec<_>>()
                    != old_ids
                {
                    return Err(format!("{} old page changed", case.id));
                }
                let started = Instant::now();
                let result = new.search(&new_query)?;
                new_ns.push(started.elapsed().as_nanos() as u64);
                if result.count != new_first.count {
                    return Err(format!("{} new count changed", case.id));
                }
                if result
                    .hits
                    .iter()
                    .map(|hit| hit.record_id.as_str())
                    .collect::<Vec<_>>()
                    != new_ids
                {
                    return Err(format!("{} new page changed", case.id));
                }
            } else {
                let started = Instant::now();
                let result = new.search(&new_query)?;
                new_ns.push(started.elapsed().as_nanos() as u64);
                if result.count != new_first.count {
                    return Err(format!("{} new count changed", case.id));
                }
                if result
                    .hits
                    .iter()
                    .map(|hit| hit.record_id.as_str())
                    .collect::<Vec<_>>()
                    != new_ids
                {
                    return Err(format!("{} new page changed", case.id));
                }
                let started = Instant::now();
                let result = old
                    .query_json(&old_query)
                    .map_err(|error| error.to_string())?;
                old_ns.push(started.elapsed().as_nanos() as u64);
                if result.total != old_first.total {
                    return Err(format!("{} old count changed", case.id));
                }
                if result
                    .records
                    .iter()
                    .map(|record| record.id.as_str())
                    .collect::<Vec<_>>()
                    != old_ids
                {
                    return Err(format!("{} old page changed", case.id));
                }
            }
        }
        old_ns.sort_unstable();
        new_ns.sort_unstable();
        println!(
            "{}",
            serde_json::json!({
                "type":"case", "case":case.id,"collection":case.collection,"filter":case.filter,
                "matches":new_first.count,"old_ns":old_ns,"new_ns":new_ns,
                "old_median_ns":old_ns[repetitions/2],"new_median_ns":new_ns[repetitions/2],
                "old_source_reads":old_first.metrics.source_reads,
                "old_parses":old_first.metrics.parses,
                "new_files_opened":new_first.counters.files_opened,
                "new_parses":new_first.counters.record_parses,
            })
        );
    }
    Ok(())
}
