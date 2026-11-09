#[path = "support/musenalm.rs"]
mod musenalm;

use std::error::Error;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use objekte_search_core::lifecycle::Index;
use objekte_search_core::repository::RepositoryOptions;
use objekte_search_core::workload::{BigramAlphabet, load_extended_manifest};

fn memory_kb(label: &str) -> u64 {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with(label))
                .map(str::to_owned)
        })
        .and_then(|line| line.split_whitespace().nth(1).and_then(|n| n.parse().ok()))
        .unwrap_or(0)
}

fn run(root: &Path, output: &Path) -> Result<(), Box<dyn Error>> {
    let cache_bytes = std::env::var("LIFECYCLE_CACHE_BYTES")
        .ok()
        .map(|value| value.parse::<usize>())
        .transpose()?
        .unwrap_or(0);
    let count_off = std::env::var_os("LIFECYCLE_COUNT_OFF").is_some();
    let offset = std::env::var("LIFECYCLE_OFFSET")
        .ok()
        .map(|value| value.parse::<usize>())
        .transpose()?;
    let limit = std::env::var("LIFECYCLE_LIMIT")
        .ok()
        .map(|value| value.parse::<usize>())
        .transpose()?;
    let retire_path = std::env::var("LIFECYCLE_RETIRE_PATH").ok();
    let core = Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf();
    let mut cases = load_extended_manifest(
        &core.join("benches/musenalm_queries.json"),
        &core.join("benches/musenalm_queries_extended.json"),
    )?;
    if let Ok(selected) = std::env::var("LIFECYCLE_CASES") {
        let ids: Vec<_> = selected.split(',').collect();
        cases.retain(|case| ids.contains(&case.id.as_str()));
    }
    let (definitions, _) = musenalm::load_extended_definitions_with_schemas(root)?;
    let mut writer = BufWriter::new(File::create(output)?);
    let mut failures = 0;
    for (alphabet_name, alphabet) in [
        ("ascii", BigramAlphabet::Ascii),
        ("unicode", BigramAlphabet::Unicode),
    ] {
        let name = format!(
            "{alphabet_name}-{}",
            if cache_bytes == 0 { "off" } else { "cache" }
        );
        let started = Instant::now();
        let index = Index::build(
            root,
            definitions.clone(),
            RepositoryOptions {
                bigram_alphabet: alphabet,
                query_workers: 32,
                verified_cache_bytes: cache_bytes,
                ..RepositoryOptions::default()
            },
        )?;
        let build_ns = started.elapsed().as_nanos();
        if let Some(path) = retire_path.as_deref() {
            let handle = index
                .view()?
                .file_handle_for_path(Path::new(path))
                .ok_or_else(|| std::io::Error::other(format!("unknown file to retire: {path}")))?;
            index.delete_file(handle)?;
        }
        let view = index.view()?;
        println!(
            "profile={name} cache_bytes={cache_bytes} retired_file={:?} build_ms={:.3} files={} records={} rss_kb={} hwm_kb={}",
            retire_path,
            build_ns as f64 / 1_000_000.0,
            index.status()?.indexed_files,
            index.status()?.live_records,
            memory_kb("VmRSS:"),
            memory_kb("VmHWM:")
        );
        for case in &cases {
            let mut query = case.query()?;
            if count_off {
                query.options.count = false;
            }
            if let Some(offset) = offset {
                query.options.offset = offset;
            }
            if let Some(limit) = limit {
                query.options.limit = limit;
            }
            for repetition in 0..if cache_bytes == 0 { 5 } else { 6 } {
                let started = Instant::now();
                let result = view.search(&query);
                let ns = started.elapsed().as_nanos();
                match result {
                    Ok(result) => {
                        writeln!(
                            writer,
                            "{}",
                            serde_json::json!({
                                "case_id":case.id,"profile":name,"cache_bytes":cache_bytes,"repetition":repetition,
                                "requested_count":query.options.count,"requested_offset":query.options.offset,"requested_limit":query.options.limit,
                                "status":"ok","ns":ns,"count":result.count,
                                "returned":result.hits.len(),"counters":result.counters,
                            })
                        )?;
                    }
                    Err(error) => {
                        failures += 1;
                        writeln!(
                            writer,
                            "{}",
                            serde_json::json!({
                                "case_id":case.id,"profile":name,"cache_bytes":cache_bytes,"repetition":repetition,
                                "requested_count":query.options.count,"requested_offset":query.options.offset,"requested_limit":query.options.limit,
                                "status":"failed","ns":ns,"error":error,
                            })
                        )?;
                    }
                }
            }
        }
        writer.flush()?;
    }
    println!(
        "cases={} profiles=2 repetitions={} failures={} peak_rss_kb={}",
        cases.len(),
        if cache_bytes == 0 { 5 } else { 6 },
        failures,
        memory_kb("VmHWM:")
    );
    if failures > 0 {
        return Err(format!("{failures} lifecycle queries failed").into());
    }
    Ok(())
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: lifecycle-query ROOT OUTPUT");
        std::process::exit(2);
    }
    if let Err(error) = run(&PathBuf::from(&args[1]), &PathBuf::from(&args[2])) {
        eprintln!("lifecycle-query error: {error}");
        std::process::exit(1);
    }
}
