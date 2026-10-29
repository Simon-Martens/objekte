//! Warm, in-process full-query comparison on the Musenalm collection.
use objekte_core::{CacheMode, JsonQuery, JsonQueryBackend, Metrics, Options, Repository};
use serde::Deserialize;
use std::time::Instant;


/// One filter from the shared warm-query workload.
#[derive(Deserialize)]
struct Case {
    name: String,
    filter: String,
}


/// One timed result and its complete result signature.
#[derive(Clone)]
struct Sample {
    elapsed_ms: f64,
    metrics: Metrics,
    signature: (Option<usize>, Vec<String>),
}


fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "examples/musenalm".into());
    let data_dir = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "target/segment-accept-final".into());
    let mut cases: Vec<Case> =
        serde_json::from_str(include_str!("../../bench/workloads/musenalm-bigram.json"))?;
    if std::env::args().nth(3).as_deref() == Some("--reverse") {
        cases.reverse();
    }
    let mut segment_options = Options::new(&data_dir);
    segment_options.cache = CacheMode::Segments;
    let mut segments = Repository::load_repository(&root, segment_options)?;
    let mut bigram = Repository::load_repository(&root, Options::new(&data_dir))?;
    for case in cases {
        let request = |backend| JsonQuery {
            filter: case.filter.clone(),
            collections: Some(vec!["contents".into()]),
            limit: 3,
            count: true,
            backend,
            ..Default::default()
        };
        let segment_query = request(JsonQueryBackend::Segments);
        let bigram_query = request(JsonQueryBackend::Bigram);
        segments.query_json(&segment_query)?;
        let first = bigram.query_json(&bigram_query)?;
        if first.metrics.bigram_open_nanos > 0 {
            eprintln!(
                "initial in-memory index build: {:.2}ms",
                first.metrics.bigram_open_nanos as f64 / 1e6
            );
        }
        let mut samples: [Vec<Sample>; 2] = [Vec::new(), Vec::new()];
        for round in 0..5 {
            let order = if round % 2 == 0 { [0, 1] } else { [1, 0] };
            for backend in order {
                let (repo, query) = if backend == 0 {
                    (&mut segments, &segment_query)
                } else {
                    (&mut bigram, &bigram_query)
                };
                let started = Instant::now();
                let result = repo.query_json(query)?;
                samples[backend].push(Sample {
                    elapsed_ms: started.elapsed().as_secs_f64() * 1_000.0,
                    metrics: result.metrics,
                    signature: (
                        result.total,
                        result.records.into_iter().map(|r| r.id).collect(),
                    ),
                });
            }
        }
        for backend in &samples {
            for sample in backend {
                assert_eq!(sample.signature, samples[0][0].signature, "{}", case.name);
            }
        }
        for backend in &mut samples {
            backend.sort_by(|a, b| a.elapsed_ms.total_cmp(&b.elapsed_ms));
        }
        let s = &samples[0][2];
        let b = &samples[1][2];
        println!(
            "{}",
            serde_json::json!({
                "case": case.name,
                "filter": case.filter,
                "matches": s.signature.0,
                "segments_ms": s.elapsed_ms,
                "bigram_ms": b.elapsed_ms,
                "bigram_index_candidates": b.metrics.grep_scans,
                "bigram_source_candidates": b.metrics.grep_candidates,
                "bigram_candidate_ms": b.metrics.bigram_candidates_nanos as f64 / 1e6,
                "bigram_scan_ms": b.metrics.grep_nanos as f64 / 1e6,
                "bigram_materialize_ms": b.metrics.bigram_materialize_nanos as f64 / 1e6,
                "bigram_open_cpu_ms": b.metrics.bigram_file_open_nanos as f64 / 1e6,
                "bigram_read_cpu_ms": b.metrics.bigram_read_nanos as f64 / 1e6,
                "bigram_regex_cpu_ms": b.metrics.bigram_regex_nanos as f64 / 1e6,
                "bigram_parse_cpu_ms": b.metrics.parse_nanos as f64 / 1e6,
                "bigram_filter_cpu_ms": b.metrics.query_nanos as f64 / 1e6,
                "segment_text_candidates": s.metrics.grep_candidates,
                "segment_index_ms": s.metrics.segment_index_nanos as f64 / 1e6,
                "segment_text_scan_ms": s.metrics.grep_nanos as f64 / 1e6,
                "segment_verify_ms": s.metrics.segment_verify_nanos as f64 / 1e6,
                "segment_materialize_ms": s.metrics.segment_materialize_nanos as f64 / 1e6,
            })
        );
    }
    Ok(())
}
