#[path = "support/musenalm.rs"]
mod musenalm;

use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use objekte_search_core::lifecycle::Index;
use objekte_search_core::repository::RepositoryOptions;
use objekte_search_core::storage::{StorageOpenOptions, StorageWriteOptions, audit};
use objekte_search_core::workload::{BenchmarkCase, load_manifest};
use serde_json::json;
use sha2::{Digest, Sha256};

fn main() {
    if let Err(error) = run() {
        eprintln!("storage benchmark error: {error}");
        std::process::exit(1);
    }
}

fn digest_result(result: &objekte_search_core::repository::SearchResult) -> String {
    let bytes = format!("{:?}|{:?}", result.count, result.hits);
    format!("{:x}", Sha256::digest(bytes.as_bytes()))
}

fn median(mut samples: Vec<u64>) -> u64 {
    samples.sort_unstable();
    if samples.is_empty() {
        return 0;
    }
    let middle = samples.len() / 2;
    if samples.len() % 2 == 1 {
        samples[middle]
    } else {
        samples[middle - 1] / 2 + samples[middle] / 2
    }
}

fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let mut storage_root = std::env::temp_dir().join("objekte-storage-benchmark");
    let mut cases = vec!["Q001".to_string(), "Q119".to_string()];
    let mut repetitions = 5usize;
    let mut workers = 32usize;
    let arguments = std::env::args().collect::<Vec<_>>();
    let mut args = arguments.iter().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--root" => root = PathBuf::from(args.next().ok_or("--root needs a path")?),
            "--storage-root" => {
                storage_root = PathBuf::from(args.next().ok_or("--storage-root needs a path")?)
            }
            "--cases" => {
                cases = args
                    .next()
                    .ok_or("--cases needs comma-separated IDs")?
                    .split(',')
                    .map(str::to_owned)
                    .collect()
            }
            "--repetitions" => {
                repetitions = args
                    .next()
                    .ok_or("--repetitions needs a number")?
                    .parse()
                    .map_err(|error| format!("invalid repetitions: {error}"))?
            }
            "--workers" => {
                workers = args
                    .next()
                    .ok_or("--workers needs a number")?
                    .parse()
                    .map_err(|error| format!("invalid workers: {error}"))?
            }
            other => return Err(format!("unknown argument {other}").into()),
        }
    }
    if repetitions == 0 || workers == 0 {
        return Err("repetitions and workers must be positive".into());
    }
    let (definitions, _) = musenalm::load_definitions_with_schemas(&root)?;
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("benches/musenalm_queries.json");
    let selected = load_manifest(&manifest)?
        .into_iter()
        .map(|request| BenchmarkCase {
            request,
            parameters: std::collections::BTreeMap::new(),
            clock_unix_seconds: None,
        })
        .filter(|case| cases.contains(&case.id))
        .collect::<Vec<_>>();
    if selected.len() != cases.len() {
        return Err("one or more requested cases are not in the canonical suite".into());
    }
    let options = RepositoryOptions {
        query_workers: workers,
        verified_cache_bytes: 0,
        ..RepositoryOptions::default()
    };
    let build_started = Instant::now();
    let index = Index::build(&root, definitions.clone(), options)?;
    let native = selected
        .iter()
        .map(|case| {
            let result = index.view()?.search(&case.query()?)?;
            Ok((case.id.clone(), digest_result(&result), result.count))
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error + Send + Sync>>>()?;
    let build_ns = build_started.elapsed().as_nanos() as u64;

    std::fs::create_dir_all(&storage_root)?;
    let storage = storage_root.join(format!(
        "{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_secs(),
        std::process::id()
    ));
    let save_started = Instant::now();
    let save = index.archive_to(&storage, StorageWriteOptions::default())?;
    let save_ns = save_started.elapsed().as_nanos() as u64;
    index.shutdown()?;
    drop(index);

    let open_started = Instant::now();
    let reopened = Index::open_storage(
        &root,
        definitions.clone(),
        options,
        &storage,
        StorageOpenOptions::default(),
    )?;
    let open_ns = open_started.elapsed().as_nanos() as u64;
    let cold_status = reopened.storage_status()?;
    let mut samples = Vec::new();
    for case in &selected {
        let first_started = Instant::now();
        let first = reopened.view()?.search(&case.query()?)?;
        let first_ns = first_started.elapsed().as_nanos() as u64;
        let first_digest = digest_result(&first);
        let expected = native
            .iter()
            .find(|(id, _, _)| id == &case.id)
            .ok_or("native case result missing")?;
        if &first_digest != &expected.1 || first.count != expected.2 {
            return Err(format!("semantic mismatch for {}", case.id).into());
        }
        let mut warm_ns = Vec::with_capacity(repetitions);
        for _ in 0..repetitions {
            let started = Instant::now();
            let result = reopened.view()?.search(&case.query()?)?;
            warm_ns.push(started.elapsed().as_nanos() as u64);
            if digest_result(&result) != first_digest || result.count != first.count {
                return Err(format!("warm result mismatch for {}", case.id).into());
            }
        }
        let status = reopened.storage_status()?;
        samples.push(json!({
            "case": case.id,
            "cold_ns": first_ns,
            "warm_median_ns": median(warm_ns.clone()),
            "warm_min_ns": warm_ns.iter().copied().min().unwrap_or(0),
            "warm_max_ns": warm_ns.iter().copied().max().unwrap_or(0),
            "count": first.count,
            "index_reads": status.index_reads,
            "index_bytes": status.index_bytes,
            "cache": status.cache,
        }));
    }
    let warm_status = reopened.storage_status()?;
    let audit_started = Instant::now();
    let audit_report = audit(&storage, Default::default())?;
    let audit_ns = audit_started.elapsed().as_nanos() as u64;
    reopened.shutdown()?;
    drop(reopened);

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "root": root,
            "storage": storage,
            "workers": workers,
            "repetitions": repetitions,
            "build_ns": build_ns,
            "save_ns": save_ns,
            "open_ns": open_ns,
            "save": save,
            "cold_status": cold_status,
            "warm_status": warm_status,
            "audit_ns": audit_ns,
            "audit": audit_report,
            "queries": samples,
        }))
        .map_err(|error| error.to_string())?
    );
    Ok(())
}
