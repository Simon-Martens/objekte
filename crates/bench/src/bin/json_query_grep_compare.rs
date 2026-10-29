//! Compare the public JSON query API with a warm segment cache and source grep.
use objekte_core::{CacheMode, JsonQuery, JsonQueryBackend, Options, Repository};
use serde::Deserialize;
use serde_json::json;
use std::{env, fs, path::PathBuf, time::Instant};


/// One collection and filter from the benchmark workload.
#[derive(Deserialize)]
struct Case {
    collection: String,
    filter: String,
}


fn arg(name: &str, default: Option<&str>) -> Result<String, Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    while let Some(key) = args.next() {
        if key == name {
            return Ok(args.next().ok_or(format!("{name} needs a value"))?);
        }
    }
    Ok(default.ok_or(format!("missing {name}"))?.into())
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(arg("--repo", Some("examples/musenalm"))?);
    let workload = PathBuf::from(arg(
        "--workload",
        Some("crates/bench/workloads/musenalm-grep.json"),
    )?);
    let data_dir = PathBuf::from(arg("--data-dir", Some("target/json-query-grep-cache"))?);
    let runs: usize = arg("--runs", Some("3"))?.parse()?;
    let limit: usize = arg("--limit", Some("50"))?.parse()?;
    if runs == 0 {
        return Err("--runs must be positive".into());
    }
    let cases: Vec<Case> = serde_json::from_slice(&fs::read(workload)?)?;
    let mut options = Options::new(data_dir);
    options.cache = CacheMode::Segments;
    let mut repo = Repository::load_repository(root, options)?;
    for case in &cases {
        let request = JsonQuery {
            filter: case.filter.clone(),
            collections: Some(vec![case.collection.clone()]),
            count: true,
            limit,
            backend: JsonQueryBackend::Segments,
            ..Default::default()
        };
        let segment = repo.query_json(&request)?;
        let grep = repo.query_json(&JsonQuery {
            backend: JsonQueryBackend::Grep,
            ..request.clone()
        })?;
        if segment.total != grep.total
            || serde_json::to_value(&segment.records)? != serde_json::to_value(&grep.records)?
        {
            return Err(format!(
                "backend result mismatch: {} / {}",
                case.collection, case.filter
            )
            .into());
        }
    }
    for case in cases {
        let mut segment_ms = Vec::new();
        let mut grep_ms = Vec::new();
        let mut candidates = 0;
        let mut scans = 0;
        let mut matches = 0;
        for i in 0..runs {
            for backend in if i % 2 == 0 {
                [JsonQueryBackend::Segments, JsonQueryBackend::Grep]
            } else {
                [JsonQueryBackend::Grep, JsonQueryBackend::Segments]
            } {
                let request = JsonQuery {
                    filter: case.filter.clone(),
                    collections: Some(vec![case.collection.clone()]),
                    count: true,
                    limit,
                    backend,
                    ..Default::default()
                };
                let t = Instant::now();
                let result = repo.query_json(&request)?;
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                matches = result.total.unwrap_or(0);
                match backend {
                    JsonQueryBackend::Segments => segment_ms.push(ms),
                    JsonQueryBackend::Grep => {
                        grep_ms.push(ms);
                        candidates = result.metrics.grep_candidates;
                        scans = result.metrics.grep_scans;
                    }
                    _ => unreachable!(),
                }
            }
        }
        println!(
            "{}",
            json!({
                "collection": case.collection, "filter": case.filter, "matches": matches,
                "source_files": scans, "grep_candidates": candidates,
                "warm_segments_median_ms": median(&mut segment_ms),
                "grep_median_ms": median(&mut grep_ms),
                "limit": limit,
                "runs": runs
            })
        );
    }
    Ok(())
}
