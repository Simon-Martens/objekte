//! Warm, in-process full-query profile on the Musenalm collection.
use objekte_core::{JsonQuery, JsonQueryBackend, Metrics, Options, Repository};
use serde::Deserialize;
use std::time::Instant;


/// One named filter from the Musenalm benchmark workload.
#[derive(Deserialize)]
struct Case {
    name: String,
    filter: String,
}


/// Timings and result counts for one query execution.
struct Sample {
    elapsed_ms: f64,
    metrics: Metrics,
    signature: (Option<usize>, Vec<String>),
}


fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "examples/musenalm".into());
    let mut cases: Vec<Case> =
        serde_json::from_str(include_str!("../../bench/workloads/musenalm-bigram.json"))?;
    if std::env::args().nth(2).as_deref() == Some("--reverse") {
        cases.reverse();
    }
    let mut repo = Repository::load_repository(&root, Options::new())?;
    for case in cases {
        let query = JsonQuery {
            filter: case.filter.clone(),
            collections: Some(vec!["contents".into()]),
            limit: 3,
            count: true,
            backend: JsonQueryBackend::Bigram,
            ..Default::default()
        };
        let first = repo.query_json(&query)?;
        if first.metrics.bigram_open_nanos > 0 {
            eprintln!(
                "initial in-memory index build: {:.2}ms",
                first.metrics.bigram_open_nanos as f64 / 1e6
            );
        }
        let mut samples = Vec::new();
        for _ in 0..5 {
            let started = Instant::now();
            let result = repo.query_json(&query)?;
            samples.push(Sample {
                elapsed_ms: started.elapsed().as_secs_f64() * 1_000.0,
                metrics: result.metrics,
                signature: (
                    result.total,
                    result.records.into_iter().map(|r| r.id).collect(),
                ),
            });
        }
        for sample in &samples {
            assert_eq!(sample.signature, samples[0].signature, "{}", case.name);
        }
        samples.sort_by(|a, b| a.elapsed_ms.total_cmp(&b.elapsed_ms));
        let sample = &samples[2];
        println!(
            "{}",
            serde_json::json!({
                "case": case.name,
                "filter": case.filter,
                "matches": sample.signature.0,
                "bigram_ms": sample.elapsed_ms,
                "index_candidates": sample.metrics.grep_scans,
                "source_candidates": sample.metrics.grep_candidates,
                "candidate_ms": sample.metrics.bigram_candidates_nanos as f64 / 1e6,
                "scan_ms": sample.metrics.grep_nanos as f64 / 1e6,
                "materialize_ms": sample.metrics.bigram_materialize_nanos as f64 / 1e6,
            })
        );
    }
    Ok(())
}
