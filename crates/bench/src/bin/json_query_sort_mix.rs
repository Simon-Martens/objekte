//! Compare the 50 JSON filters with and without varied sort orders.

use objekte_core::{JsonQuery, JsonQueryBackend, Options, Repository};
use serde::Deserialize;
use serde_json::json;
use std::{env, fs, path::PathBuf, time::Instant};

#[derive(Deserialize)]
struct Case {
    id: usize,
    kind: String,
    filter: String,
    collection: String,
    sort: String,
}

struct Sample {
    ms: f64,
    total: usize,
    truncated: bool,
    ids: Vec<String>,
    reads: u64,
    parses: u64,
    indexed: bool,
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

fn run(
    repo: &mut Repository,
    case: &Case,
    scoped: bool,
    sorted: bool,
) -> Result<Sample, Box<dyn std::error::Error>> {
    let filter = if sorted {
        format!("{} SORT BY {}", case.filter, case.sort)
    } else {
        case.filter.clone()
    };
    let request = JsonQuery {
        filter,
        collections: scoped.then(|| vec![case.collection.clone()]),
        count: true,
        limit: 25,
        backend: JsonQueryBackend::Bigram,
        ..Default::default()
    };
    let started = Instant::now();
    let result = repo.query_json(&request)?;
    Ok(Sample {
        ms: started.elapsed().as_secs_f64() * 1000.0,
        total: result.total.ok_or("missing total")?,
        truncated: result.truncated,
        ids: result
            .records
            .iter()
            .map(|record| format!("{}/{}", record.collection, record.id))
            .collect(),
        reads: result.metrics.source_reads,
        parses: result.metrics.parses,
        indexed: result
            .query_plan
            .iter()
            .any(|step| step.contains("ordered sort index")),
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(arg("--repo", "examples/musenalm")?);
    let workload = PathBuf::from(arg(
        "--workload",
        "crates/bench/workloads/musenalm-sort-mix-50.json",
    )?);
    let runs: usize = arg("--runs", "3")?.parse()?;
    let scope = arg("--scope", "original")?;
    if runs == 0 || !matches!(scope.as_str(), "original" | "relevant") {
        return Err("use --runs N (N > 0) and --scope original|relevant".into());
    }
    let scoped = scope == "relevant";
    let cases: Vec<Case> = serde_json::from_slice(&fs::read(&workload)?)?;
    if cases.len() != 50 || cases.iter().enumerate().any(|(i, case)| case.id != i + 1) {
        return Err("workload must have 50 sequentially numbered cases".into());
    }
    let mut repo = Repository::load_repository(&root, Options::new())?;
    let warm = Instant::now();
    repo.warm_all_bigram_indexes()?;
    let warm_ms = warm.elapsed().as_secs_f64() * 1000.0;
    eprintln!("warmed indexes in {warm_ms:.1} ms");

    // A full untimed pass checks totals and pages, and warms source-file pages.
    let expected = cases
        .iter()
        .map(|case| {
            let base = run(&mut repo, case, scoped, false)?;
            let sorted = run(&mut repo, case, scoped, true)?;
            if base.total != sorted.total || base.truncated != sorted.truncated {
                return Err(
                    format!("case {} changed its match count under sorting", case.id).into(),
                );
            }
            Ok((base, sorted))
        })
        .collect::<Result<Vec<(Sample, Sample)>, Box<dyn std::error::Error>>>()?;

    let mut samples: Vec<Vec<(Sample, Sample)>> = (0..50).map(|_| Vec::new()).collect();
    for round in 0..runs {
        let order: Vec<usize> = if round % 2 == 0 {
            (0..50).collect()
        } else {
            (0..50).rev().collect()
        };
        for index in order {
            let case = &cases[index];
            let (base, sorted) = if round % 2 == 0 {
                (
                    run(&mut repo, case, scoped, false)?,
                    run(&mut repo, case, scoped, true)?,
                )
            } else {
                let sorted = run(&mut repo, case, scoped, true)?;
                let base = run(&mut repo, case, scoped, false)?;
                (base, sorted)
            };
            if base.total != expected[index].0.total
                || base.truncated != expected[index].0.truncated
                || base.ids != expected[index].0.ids
                || sorted.total != expected[index].1.total
                || sorted.truncated != expected[index].1.truncated
                || sorted.ids != expected[index].1.ids
            {
                return Err(format!("case {} changed its total or page", case.id).into());
            }
            samples[index].push((base, sorted));
        }
        eprintln!("completed round {}/{runs}", round + 1);
    }
    println!(
        "{}",
        json!({"type":"setup", "cases":50, "runs":runs, "scope":scope,
        "page_limit":25, "total_limit":10_000,
        "index_warm_ms":warm_ms})
    );
    for (case, case_samples) in cases.iter().zip(samples) {
        let mut base = case_samples
            .iter()
            .map(|(base, _)| base)
            .collect::<Vec<_>>();
        let mut sorted = case_samples
            .iter()
            .map(|(_, sorted)| sorted)
            .collect::<Vec<_>>();
        base.sort_by(|a, b| a.ms.total_cmp(&b.ms));
        sorted.sort_by(|a, b| a.ms.total_cmp(&b.ms));
        let b = base[runs / 2];
        let s = sorted[runs / 2];
        println!(
            "{}",
            json!({"type":"case", "id":case.id, "kind":case.kind,
            "filter":case.filter, "collection":case.collection, "sort":case.sort,
            "matches":s.total, "truncated":s.truncated, "sort_index_used":s.indexed,
            "baseline_ms":b.ms, "sorted_ms":s.ms, "baseline_reads":b.reads,
            "sorted_reads":s.reads, "baseline_parses":b.parses, "sorted_parses":s.parses})
        );
    }
    Ok(())
}
