#[path = "support/musenalm.rs"]
mod musenalm;

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use objekte_search_core::lifecycle::Index;
use objekte_search_core::repository::RepositoryOptions;
use objekte_search_core::storage::{CachePolicy, StorageOpenOptions, StorageWriteOptions, audit};
use objekte_search_core::workload::{BenchmarkCase, BigramAlphabet, load_manifest};
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

fn memory_bytes(field: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with(field))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|value| value.parse::<u64>().ok())
        })
        .unwrap_or(0)
        .saturating_mul(1024)
}

fn write_tsv(path: &std::path::Path, header: &[&str], rows: &[Vec<String>]) -> std::io::Result<()> {
    let mut writer = BufWriter::new(File::create(path)?);
    writeln!(writer, "{}", header.join("\t"))?;
    for row in rows {
        writeln!(writer, "{}", row.join("\t"))?;
    }
    writer.flush()
}

fn object_size_rows(storage: &std::path::Path) -> std::io::Result<Vec<Vec<String>>> {
    let mut totals = std::collections::BTreeMap::<String, (u64, u64)>::new();
    for entry in std::fs::read_dir(storage.join("objects"))? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let category = if name.ends_with(".meta") {
            "metadata"
        } else if name.ends_with(".pack") {
            "payload_pack"
        } else {
            "other"
        };
        let size = entry.metadata()?.len();
        let total = totals.entry(category.into()).or_default();
        total.0 = total.0.saturating_add(1);
        total.1 = total.1.saturating_add(size);
    }
    Ok(totals
        .into_iter()
        .map(|(category, (count, bytes))| {
            vec![category, count.to_string(), bytes.to_string(), "0".into()]
        })
        .collect())
}

fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let mut storage_root = std::env::temp_dir().join("objekte-storage-benchmark");
    let mut cases = vec!["Q001".to_string(), "Q119".to_string()];
    let mut all_cases = false;
    let mut repetitions = 5usize;
    let mut workers = 32usize;
    let mut alphabet = BigramAlphabet::Ascii;
    let mut output: Option<PathBuf> = None;
    let mut open_samples = 1usize;
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
            "--all-cases" => all_cases = true,
            "--alphabet" => {
                alphabet = match args
                    .next()
                    .ok_or("--alphabet needs ascii or unicode")?
                    .as_str()
                {
                    "ascii" => BigramAlphabet::Ascii,
                    "unicode" => BigramAlphabet::Unicode,
                    other => return Err(format!("unknown alphabet {other}").into()),
                }
            }
            "--out" => {
                output = Some(PathBuf::from(args.next().ok_or("--out needs a path")?));
            }
            "--open-samples" => {
                open_samples = args
                    .next()
                    .ok_or("--open-samples needs a number")?
                    .parse()
                    .map_err(|error| format!("invalid open samples: {error}"))?
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
    if repetitions == 0 || workers == 0 || open_samples == 0 {
        return Err("repetitions, workers and open samples must be positive".into());
    }
    let (definitions, _) = musenalm::load_definitions_with_schemas(&root)?;
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("benches/musenalm_queries.json");
    let mut selected = load_manifest(&manifest)?
        .into_iter()
        .map(|request| BenchmarkCase {
            request,
            parameters: std::collections::BTreeMap::new(),
            clock_unix_seconds: None,
        })
        .collect::<Vec<_>>();
    if !all_cases {
        selected.retain(|case| cases.contains(&case.id));
    }
    if selected.len() != cases.len() && !all_cases {
        return Err("one or more requested cases are not in the canonical suite".into());
    }
    let options = RepositoryOptions {
        query_workers: workers,
        verified_cache_bytes: 0,
        bigram_alphabet: alphabet,
        ..RepositoryOptions::default()
    };
    let mut construction_rows = Vec::<Vec<String>>::new();
    let mut save_rows = Vec::<Vec<String>>::new();
    let mut open_rows = Vec::<Vec<String>>::new();
    let mut query_rows = Vec::<Vec<String>>::new();
    let mut io_rows = Vec::<Vec<String>>::new();
    let mut memory_rows = Vec::<Vec<String>>::new();
    let alphabet_name = match alphabet {
        BigramAlphabet::Ascii => "ascii",
        BigramAlphabet::Unicode => "unicode",
    };
    if let Some(output) = &output {
        std::fs::create_dir_all(output)?;
    }
    let rss_before = memory_bytes("VmRSS:");
    let build_started = Instant::now();
    let index = Index::build(&root, definitions.clone(), options)?;
    let build_ns = build_started.elapsed().as_nanos() as u64;
    let mut native = Vec::with_capacity(selected.len());
    for case in &selected {
        let started = Instant::now();
        let result = index.view()?.search(&case.query()?)?;
        let elapsed = started.elapsed().as_nanos() as u64;
        let digest = digest_result(&result);
        native.push((case.id.clone(), digest.clone(), result.count));
        query_rows.push(vec![
            alphabet_name.into(),
            "native".into(),
            case.id.clone(),
            "0".into(),
            elapsed.to_string(),
            result
                .count
                .map_or_else(String::new, |value| value.to_string()),
            result.hits.len().to_string(),
            digest,
            "0".into(),
            "0".into(),
        ]);
    }
    construction_rows.push(vec![
        alphabet_name.into(),
        workers.to_string(),
        "index_build".into(),
        build_ns.to_string(),
        memory_bytes("VmRSS:").to_string(),
        memory_bytes("VmHWM:").to_string(),
        rss_before.to_string(),
    ]);
    memory_rows.push(vec![
        alphabet_name.into(),
        "native_build".into(),
        memory_bytes("VmRSS:").to_string(),
        memory_bytes("VmHWM:").to_string(),
    ]);
    let archive_started = Instant::now();
    let archive_report = index.archive()?;
    let in_memory_archive_ns = archive_started.elapsed().as_nanos() as u64;
    for case in &selected {
        let started = Instant::now();
        let result = index.view()?.search(&case.query()?)?;
        let elapsed = started.elapsed().as_nanos() as u64;
        let digest = digest_result(&result);
        query_rows.push(vec![
            alphabet_name.into(),
            "resident_archived".into(),
            case.id.clone(),
            "0".into(),
            elapsed.to_string(),
            result
                .count
                .map_or_else(String::new, |value| value.to_string()),
            result.hits.len().to_string(),
            digest,
            "0".into(),
            "0".into(),
        ]);
    }
    memory_rows.push(vec![
        alphabet_name.into(),
        "after_memory_archive".into(),
        memory_bytes("VmRSS:").to_string(),
        memory_bytes("VmHWM:").to_string(),
    ]);

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
    save_rows.push(vec![
        alphabet_name.into(),
        "first_save".into(),
        save_ns.to_string(),
        save.bytes_serialized.to_string(),
        save.bytes_written.to_string(),
        save.objects_new.to_string(),
        save.objects_reused.to_string(),
        save.sync_calls.to_string(),
        memory_bytes("VmRSS:").to_string(),
        memory_bytes("VmHWM:").to_string(),
    ]);
    memory_rows.push(vec![
        alphabet_name.into(),
        "after_first_save".into(),
        memory_bytes("VmRSS:").to_string(),
        memory_bytes("VmHWM:").to_string(),
    ]);
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
    open_rows.push(vec![
        alphabet_name.into(),
        "0".into(),
        open_ns.to_string(),
        cold_status.index_reads.to_string(),
        cold_status.index_bytes.to_string(),
        cold_status.eager_file_rows.to_string(),
        cold_status.eager_record_rows.to_string(),
        cold_status.eager_index_descriptors.to_string(),
        cold_status.eager_leaf_rows.to_string(),
        memory_bytes("VmRSS:").to_string(),
        memory_bytes("VmHWM:").to_string(),
    ]);
    for sample in 1..open_samples {
        let started = Instant::now();
        let opened = Index::open_storage(
            &root,
            definitions.clone(),
            options,
            &storage,
            StorageOpenOptions::default(),
        )?;
        let elapsed = started.elapsed().as_nanos() as u64;
        let status = opened.storage_status()?;
        open_rows.push(vec![
            alphabet_name.into(),
            sample.to_string(),
            elapsed.to_string(),
            status.index_reads.to_string(),
            status.index_bytes.to_string(),
            status.eager_file_rows.to_string(),
            status.eager_record_rows.to_string(),
            status.eager_index_descriptors.to_string(),
            status.eager_leaf_rows.to_string(),
            memory_bytes("VmRSS:").to_string(),
            memory_bytes("VmHWM:").to_string(),
        ]);
        opened.shutdown()?;
    }
    memory_rows.push(vec![
        alphabet_name.into(),
        "after_eager_open".into(),
        memory_bytes("VmRSS:").to_string(),
        memory_bytes("VmHWM:").to_string(),
    ]);
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
        let first_status = reopened.storage_status()?;
        query_rows.push(vec![
            alphabet_name.into(),
            "reopened_session_first".into(),
            case.id.clone(),
            "0".into(),
            first_ns.to_string(),
            first
                .count
                .map_or_else(String::new, |value| value.to_string()),
            first.hits.len().to_string(),
            first_digest.clone(),
            first_status.index_reads.to_string(),
            first_status.index_bytes.to_string(),
        ]);
        let mut warm_ns = Vec::with_capacity(repetitions);
        for repetition in 0..repetitions {
            let started = Instant::now();
            let result = reopened.view()?.search(&case.query()?)?;
            let elapsed = started.elapsed().as_nanos() as u64;
            warm_ns.push(elapsed);
            let digest = digest_result(&result);
            if digest != first_digest || result.count != first.count {
                return Err(format!("warm result mismatch for {}", case.id).into());
            }
            let status = reopened.storage_status()?;
            query_rows.push(vec![
                alphabet_name.into(),
                "reopened_warm".into(),
                case.id.clone(),
                repetition.to_string(),
                elapsed.to_string(),
                result
                    .count
                    .map_or_else(String::new, |value| value.to_string()),
                result.hits.len().to_string(),
                digest,
                status.index_reads.to_string(),
                status.index_bytes.to_string(),
            ]);
            io_rows.push(vec![
                alphabet_name.into(),
                case.id.clone(),
                "reopened_warm".into(),
                status.index_reads.to_string(),
                status.index_bytes.to_string(),
                status.cache.hits.to_string(),
                status.cache.misses.to_string(),
                status.cache.joins.to_string(),
                status.cache.evictions.to_string(),
                status.cache.resident_bytes.to_string(),
                status.cache.pinned_bytes.to_string(),
            ]);
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
    let mut cache_sequence_rows = Vec::<Vec<String>>::new();
    for budget_mib in [64u64, 256] {
        let cached = Index::open_storage(
            &root,
            definitions.clone(),
            options,
            &storage,
            StorageOpenOptions {
                cache_policy: CachePolicy::Budget {
                    resident_bytes: std::num::NonZeroU64::new(budget_mib * 1024 * 1024)
                        .expect("positive cache budget"),
                },
                ..StorageOpenOptions::default()
            },
        )?;
        for (step, case) in selected.iter().take(8).enumerate() {
            let started = Instant::now();
            let result = cached.view()?.search(&case.query()?)?;
            let elapsed = started.elapsed().as_nanos() as u64;
            let status = cached.storage_status()?;
            cache_sequence_rows.push(vec![
                alphabet_name.into(),
                budget_mib.to_string(),
                (step + 1).to_string(),
                case.id.clone(),
                elapsed.to_string(),
                result
                    .count
                    .map_or_else(String::new, |value| value.to_string()),
                status.index_reads.to_string(),
                status.index_bytes.to_string(),
                status.cache.hits.to_string(),
                status.cache.misses.to_string(),
                status.cache.evictions.to_string(),
                status.cache.resident_bytes.to_string(),
                status.cache.pinned_bytes.to_string(),
            ]);
        }
        cached.shutdown()?;
    }
    let audit_started = Instant::now();
    let audit_report = audit(&storage, Default::default())?;
    let audit_ns = audit_started.elapsed().as_nanos() as u64;
    reopened.shutdown()?;
    drop(reopened);

    if let Some(output) = &output {
        write_tsv(
            &output.join("construction-samples.tsv"),
            &[
                "alphabet",
                "workers",
                "entry_point",
                "elapsed_ns",
                "rss_bytes",
                "peak_rss_bytes",
                "rss_before_bytes",
            ],
            &construction_rows,
        )?;
        write_tsv(
            &output.join("save-samples.tsv"),
            &[
                "alphabet",
                "variant",
                "elapsed_ns",
                "bytes_serialized",
                "bytes_written",
                "objects_new",
                "objects_reused",
                "sync_calls",
                "rss_bytes",
                "peak_rss_bytes",
            ],
            &save_rows,
        )?;
        write_tsv(
            &output.join("open-samples.tsv"),
            &[
                "alphabet",
                "sample",
                "elapsed_ns",
                "index_reads",
                "index_bytes",
                "file_rows",
                "record_rows",
                "index_descriptors",
                "leaf_rows",
                "rss_bytes",
                "peak_rss_bytes",
            ],
            &open_rows,
        )?;
        write_tsv(
            &output.join("query-samples.tsv"),
            &[
                "alphabet",
                "state",
                "case_id",
                "repetition",
                "elapsed_ns",
                "count",
                "returned",
                "result_digest",
                "index_reads",
                "index_bytes",
            ],
            &query_rows,
        )?;
        write_tsv(
            &output.join("cache-sequence.tsv"),
            &[
                "alphabet",
                "budget_mib",
                "step",
                "case_id",
                "elapsed_ns",
                "count",
                "index_reads",
                "index_bytes",
                "cache_hits",
                "cache_misses",
                "cache_evictions",
                "resident_bytes",
                "pinned_bytes",
            ],
            &cache_sequence_rows,
        )?;
        write_tsv(
            &output.join("storage-io-counters.tsv"),
            &[
                "alphabet",
                "case_id",
                "state",
                "index_reads",
                "index_bytes",
                "cache_hits",
                "cache_misses",
                "cache_joins",
                "cache_evictions",
                "resident_bytes",
                "pinned_bytes",
            ],
            &io_rows,
        )?;
        write_tsv(
            &output.join("memory-summary.tsv"),
            &["alphabet", "checkpoint", "rss_bytes", "peak_rss_bytes"],
            &memory_rows,
        )?;
        write_tsv(
            &output.join("object-size-summary.tsv"),
            &["category", "objects", "bytes", "fragmented_bytes"],
            &object_size_rows(&storage)?,
        )?;
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "root": root,
            "storage": storage,
            "workers": workers,
            "repetitions": repetitions,
            "alphabet": alphabet_name,
            "build_ns": build_ns,
            "in_memory_archive_ns": in_memory_archive_ns,
            "in_memory_archive": archive_report,
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
