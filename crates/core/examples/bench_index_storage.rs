//! Measure explicit build, save, load, first-query, and repeated-save costs.
//!
//! Synthetic corpus: one JSONL file per collection with bounded records. This
//! keeps the comparison focused on storage rather than source discovery.

use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use objekte_search_core::SaveOptions;
use objekte_search_core::lifecycle::Index;
use objekte_search_core::query::{Query, QueryOptions};
use objekte_search_core::repository::{CollectionDefinition, RepositoryOptions};
use objekte_search_core::source::{SourceFormat, SourceRegistry};
use objekte_search_core::workload::ReturnMode;

fn temporary(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "objekte-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn query(text: &str) -> Query {
    Query {
        collections: vec!["records".into()],
        fts: None,
        filter: Some(
            objekte_search_core::query::parse_filter(&format!("text = \"{text}\"")).unwrap(),
        ),
        options: QueryOptions {
            sort: Vec::new(),
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    }
}

fn main() {
    let records = std::env::var("OBJEKTE_STORAGE_RECORDS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(20_000);
    let root = temporary("storage-bench-source");
    let data = root.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let mut text = String::new();
    for index in 0..records {
        text.push_str(&format!(
            "{{\"id\":\"r{index:06}\",\"text\":\"value {index}\"}}\n"
        ));
    }
    std::fs::write(data.join("records.jsonl"), text).unwrap();
    let definitions = vec![CollectionDefinition::folder(
        "records",
        "data",
        SourceFormat::JsonLines,
    )];
    let options = RepositoryOptions::default();

    let build_started = Instant::now();
    let index = Index::build(&root, definitions.clone(), options).unwrap();
    let build = build_started.elapsed();
    let count = index
        .view()
        .unwrap()
        .search(&query("value 0"))
        .unwrap()
        .count;

    let store = temporary("storage-bench-store");
    let save_started = Instant::now();
    let saved = index.save_to(&store, SaveOptions::default()).unwrap();
    let save = save_started.elapsed();

    let load_started = Instant::now();
    let loaded = Index::load_from(
        &store,
        definitions.clone(),
        options,
        SourceRegistry::builtin(),
    )
    .unwrap();
    let load = load_started.elapsed();
    let first_query_started = Instant::now();
    let loaded_count = loaded
        .view()
        .unwrap()
        .search(&query("value 0"))
        .unwrap()
        .count;
    let first_query = first_query_started.elapsed();

    let noop_started = Instant::now();
    let noop = loaded.save_to(&store, SaveOptions::default()).unwrap();
    let noop_time = noop_started.elapsed();

    println!(
        "{}",
        serde_json::json!({
            "records": records,
            "build_ms": build.as_secs_f64() * 1000.0,
            "save_ms": save.as_secs_f64() * 1000.0,
            "load_ms": load.as_secs_f64() * 1000.0,
            "first_query_ms": first_query.as_secs_f64() * 1000.0,
            "noop_save_ms": noop_time.as_secs_f64() * 1000.0,
            "load_speedup_over_build": build.as_secs_f64() / load.as_secs_f64(),
            "objects_written": saved.objects_written,
            "objects_reused": saved.objects_reused,
            "bytes_written": saved.bytes_written,
            "noop_unchanged": noop.unchanged,
            "noop_bytes_written": noop.bytes_written,
            "count_before": count,
            "count_after": loaded_count,
        })
    );
    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(store);
}
