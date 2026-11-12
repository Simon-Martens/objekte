//! Reproducible multi-record and dynamic-field stress sample for staged builds.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use objekte_search_core::inference::{InferenceConfig, InferenceMode};
use objekte_search_core::lifecycle::Index;
use objekte_search_core::query::{Query, QueryOptions, parse_filter};
use objekte_search_core::repository::{CollectionDefinition, RepositoryOptions};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::workload::ReturnMode;
use serde_json::{Map, Value, json};

fn main() {
    if let Err(error) = run() {
        eprintln!("staged shape benchmark: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = std::env::args().collect::<Vec<_>>();
    let output = args
        .windows(2)
        .find(|pair| pair[0] == "--result-dir")
        .map(|pair| PathBuf::from(&pair[1]))
        .ok_or("--result-dir requires a path")?;
    let repetitions = args
        .windows(2)
        .find(|pair| pair[0] == "--repetitions")
        .map(|pair| pair[1].parse::<usize>().map_err(|e| e.to_string()))
        .transpose()?
        .unwrap_or(7);
    if repetitions == 0 {
        return Err("repetitions must be positive".into());
    }
    fs::create_dir_all(&output).map_err(|e| e.to_string())?;
    let root = output.join("fixture");
    generate(&root)?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let definition = CollectionDefinition::folder("shapes", "data", SourceFormat::JsonLines);
    let mut inference = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        ..Default::default()
    };
    inference.policy.min_nonblank_occurrences = 1;
    inference.policy.min_nonblank_records = 1;
    let filters = [
        "nested.value:string = true",
        "nested.value:exists = false",
        "mixed:array = true",
        "mixed:null = true",
        "mixed:? = true",
        "dyn_7:string = true",
        "repeated.value:string = true",
        "repeated.value:null = true",
    ];
    let mut expected = BTreeMap::new();
    let mut measurements = Vec::new();
    for repetition in 0..repetitions {
        for (label, options) in [
            ("base", RepositoryOptions::default()),
            (
                "inferred",
                RepositoryOptions {
                    inference,
                    ..Default::default()
                },
            ),
        ] {
            let started = Instant::now();
            let index = Index::start_staged_build(&root, vec![definition.clone()], options)
                .map_err(|e| e.to_string())?
                .wait_search_ready()
                .map_err(|e| e.to_string())?;
            let ready_ns = started.elapsed().as_nanos() as u64;
            index.wait_extended().map_err(|e| e.to_string())?;
            let complete_ns = started.elapsed().as_nanos() as u64;
            let view = index.view().map_err(|e| e.to_string())?;
            let mut queries = BTreeMap::new();
            for filter in filters {
                let query = Query {
                    collections: vec!["shapes".into()],
                    fts: None,
                    filter: Some(parse_filter(filter)?),
                    options: QueryOptions {
                        sort: vec![],
                        limit: 1100,
                        offset: 0,
                        count: true,
                        return_mode: ReturnMode::Locator,
                        evidence: false,
                    },
                };
                let result = view.search(&query)?;
                let signature = (result.count, result.hits);
                if let Some(previous) = expected.get(filter) {
                    if previous != &signature {
                        return Err(format!("{filter}: source and inferred results differ"));
                    }
                } else {
                    expected.insert(filter, signature);
                }
                queries.insert(filter, result.counters.record_parses);
            }
            let snapshot = output.join(format!("{label}-{repetition}.snapshot"));
            let (_, save) = index
                .flush_cache_copy_with_stats(&snapshot)
                .map_err(|e| e.to_string())?;
            let snapshot_bytes = fs::metadata(&snapshot).map_err(|e| e.to_string())?.len();
            fs::remove_file(&snapshot).map_err(|e| e.to_string())?;
            measurements.push(json!({
                "repetition": repetition,
                "mode": label,
                "search_ready_ns": ready_ns,
                "complete_ns": complete_ns,
                "build_parses": index.build_parses().map_err(|e| e.to_string())?,
                "field_state_memory": view.measure_field_state_memory(),
                "snapshot_bytes": snapshot_bytes,
                "snapshot_save_ns": save.total_ns,
                "query_record_parses": queries,
            }));
            index.shutdown().map_err(|e| e.to_string())?;
        }
    }
    fs::write(
        output.join("measurements.json"),
        serde_json::to_vec_pretty(&json!({
            "fixture": "64 JSONL files x 16 records, 128 rotating dynamic fields",
            "records": 1024,
            "queries": filters,
            "measurements": measurements,
        }))
        .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn generate(root: &Path) -> Result<(), String> {
    let data = root.join("data");
    fs::create_dir_all(&data).map_err(|e| e.to_string())?;
    for file in 0..64usize {
        let mut lines = Vec::new();
        for row in 0..16usize {
            let id = file * 16 + row;
            let mut object = Map::new();
            object.insert("id".into(), json!(id));
            if id % 8 == 0 {
                object.insert("nested".into(), json!({"value": format!("v{}", id % 5)}));
            }
            object.insert(
                "mixed".into(),
                match id % 4 {
                    0 => json!([]),
                    1 => json!([null, "x"]),
                    2 => json!([1, {"k": id}]),
                    _ => json!(["", "y"]),
                },
            );
            object.insert(
                "repeated".into(),
                match id % 3 {
                    0 => json!([{"value": null}, {"value": "x"}]),
                    1 => json!([{"value": 1}, {"value": "y"}]),
                    _ => json!([{"value": false}, {"value": {"k": 1}}]),
                },
            );
            object.insert(format!("dyn_{}", id % 128), json!(format!("v{}", id % 3)));
            lines.push(serde_json::to_string(&Value::Object(object)).map_err(|e| e.to_string())?);
        }
        fs::write(
            data.join(format!("{file:03}.jsonl")),
            lines.join("\n") + "\n",
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}
