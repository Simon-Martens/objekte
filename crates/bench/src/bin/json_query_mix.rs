//! Measure a diverse set of full-scope JSON queries with one warm repository.

use objekte_core::{JsonQuery, JsonQueryBackend, Options, Repository};
use serde::Deserialize;
use serde_json::json;
use std::{env, fs, path::PathBuf, time::Instant};


/// One unscoped filter and its label in the benchmark workload.
#[derive(Deserialize)]
struct Case {
    id: usize,
    kind: String,
    filter: String,
}


/// One query execution, including wall time and source work.
struct Sample {
    ms: f64,
    total: usize,
    scans: u64,
    reads: u64,
    parses: u64,
    source_check_ms: f64,
}


fn arg(name: &str, default: &str) -> Result<String, Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    while let Some(key) = args.next() {
        if key == name {
            return Ok(args.next().ok_or(format!("{name} needs a value"))?);
        }
    }
    Ok(default.into())
}

fn flag(name: &str) -> bool {
    env::args().any(|arg| arg == name)
}

fn run(repo: &mut Repository, case: &Case) -> Result<Sample, Box<dyn std::error::Error>> {
    let request = JsonQuery {
        filter: case.filter.clone(),
        collections: None,
        count: true,
        limit: 1,
        backend: JsonQueryBackend::Bigram,
        ..Default::default()
    };
    let started = Instant::now();
    let result = repo.query_json(&request)?;
    Ok(Sample {
        ms: started.elapsed().as_secs_f64() * 1000.0,
        total: result.total.ok_or("missing exact count")?,
        scans: result.metrics.grep_scans,
        reads: result.metrics.source_reads,
        parses: result.metrics.parses,
        source_check_ms: result.metrics.source_check_nanos as f64 / 1_000_000.0,
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(arg("--repo", "examples/musenalm")?);
    let workload = PathBuf::from(arg(
        "--workload",
        "crates/bench/workloads/musenalm-mix-50.json",
    )?);
    let runs: usize = arg("--runs", "3")?.parse()?;
    if runs == 0 {
        return Err("--runs must be positive".into());
    }
    let cases: Vec<Case> = serde_json::from_slice(&fs::read(workload)?)?;
    if cases.len() != 50 || cases.iter().enumerate().any(|(i, c)| c.id != i + 1) {
        return Err("workload must have 50 sequentially numbered cases".into());
    }
    let no_schema = flag("--no-schema");
    let mut repo = if no_schema {
        Repository::builder(&root)
            .without_default_schemas()
            .open()?
    } else {
        Repository::load_repository(&root, Options::new())?
    };
    let collection_count = repo.model().collections.len();
    let warm_started = Instant::now();
    repo.warm_all_bigram_indexes()?;
    let warm_ms = warm_started.elapsed().as_secs_f64() * 1000.0;
    eprintln!("warm index: {warm_ms:.1} ms across {collection_count} collections");

    // Check every filter once before timing and bring source pages into the same
    // warm state. This also catches unstable totals before reporting a median.
    let expected = cases
        .iter()
        .map(|case| {
            run(&mut repo, case)
                .map(|sample| sample.total)
                .map_err(|error| format!("case {} ({}): {error}", case.id, case.filter))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut samples: Vec<Vec<Sample>> = (0..cases.len()).map(|_| Vec::new()).collect();
    for round in 0..runs {
        // Reverse every other round so a consistently hot or cold position does
        // not decide the ranking of cases with similar runtimes.
        let order: Vec<usize> = if round % 2 == 0 {
            (0..cases.len()).collect()
        } else {
            (0..cases.len()).rev().collect()
        };
        for index in order {
            let sample = run(&mut repo, &cases[index])?;
            if sample.total != expected[index] {
                return Err(format!("case {} changed its count", cases[index].id).into());
            }
            samples[index].push(sample);
        }
        eprintln!("completed round {}/{runs}", round + 1);
    }
    println!(
        "{}",
        json!({"type": "setup", "cases": cases.len(), "runs": runs,
            "collections": collection_count, "index_warm_ms": warm_ms,
            "count": true, "page_limit": 1, "backend": "bigram",
            "schema_mode": if no_schema { "none" } else { "configured" }})
    );
    for (case, mut case_samples) in cases.iter().zip(samples) {
        case_samples.sort_by(|a, b| a.ms.total_cmp(&b.ms));
        let median = &case_samples[runs / 2];
        println!(
            "{}",
            json!({"type": "case", "id": case.id, "kind": case.kind,
                "filter": case.filter, "matches": median.total,
                "median_ms": median.ms, "min_ms": case_samples[0].ms,
                "max_ms": case_samples[runs - 1].ms,
                "scans": median.scans, "source_reads": median.reads,
                "parses": median.parses,
                "source_check_ms": median.source_check_ms})
        );
    }
    Ok(())
}
