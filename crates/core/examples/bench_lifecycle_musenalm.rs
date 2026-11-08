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
    for (name, alphabet) in [
        ("ascii-off", BigramAlphabet::Ascii),
        ("unicode-off", BigramAlphabet::Unicode),
    ] {
        let started = Instant::now();
        let index = Index::build(
            root,
            definitions.clone(),
            RepositoryOptions {
                bigram_alphabet: alphabet,
                query_workers: 32,
                ..RepositoryOptions::default()
            },
        )?;
        let build_ns = started.elapsed().as_nanos();
        let view = index.view()?;
        println!(
            "profile={name} build_ms={:.3} files={} records={} rss_kb={} hwm_kb={}",
            build_ns as f64 / 1_000_000.0,
            index.status()?.indexed_files,
            index.status()?.live_records,
            memory_kb("VmRSS:"),
            memory_kb("VmHWM:")
        );
        for case in &cases {
            let query = case.query()?;
            for repetition in 0..5 {
                let started = Instant::now();
                let result = view.search(&query);
                let ns = started.elapsed().as_nanos();
                match result {
                    Ok(result) => {
                        writeln!(
                            writer,
                            "{}",
                            serde_json::json!({
                                "case_id":case.id,"profile":name,"repetition":repetition,
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
                                "case_id":case.id,"profile":name,"repetition":repetition,
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
        "cases={} profiles=2 repetitions=5 failures={} peak_rss_kb={}",
        cases.len(),
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
