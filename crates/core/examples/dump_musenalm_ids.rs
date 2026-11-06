#[path = "support/musenalm.rs"]
mod musenalm;

use std::collections::BTreeSet;
use std::path::PathBuf;

use objekte_search_core::query::Query;
use objekte_search_core::repository::{Repository, RepositoryOptions};
use objekte_search_core::workload::{BigramAlphabet, ReturnMode, load_manifest};

fn run() -> Result<(), String> {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let mut alphabet = BigramAlphabet::Ascii;
    let mut selected = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = PathBuf::from(args.next().ok_or("--root needs a path")?),
            "--cases" => {
                selected = Some(
                    args.next()
                        .ok_or("--cases needs comma-separated IDs")?
                        .split(',')
                        .map(str::to_owned)
                        .collect::<BTreeSet<_>>(),
                )
            }
            "--alphabet" => {
                alphabet = match args.next().ok_or("--alphabet needs a value")?.as_str() {
                    "ascii" => BigramAlphabet::Ascii,
                    "unicode" => BigramAlphabet::Unicode,
                    other => return Err(format!("unknown alphabet {other}")),
                }
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    let selected = selected.ok_or("--cases is required")?;
    let manifest = load_manifest(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("benches/musenalm_queries.json"),
    )?;
    for id in &selected {
        if !manifest.iter().any(|case| &case.id == id) {
            return Err(format!("unknown case {id}"));
        }
    }
    let repo = Repository::open_with_options(
        &root,
        musenalm::load_definitions(&root)?,
        RepositoryOptions {
            bigram_alphabet: alphabet,
            ..RepositoryOptions::default()
        },
    )?;
    let maximum = repo
        .collections()
        .iter()
        .map(|collection| collection.generation.records.len())
        .sum();
    for case in manifest
        .into_iter()
        .filter(|case| selected.contains(&case.id))
    {
        let mut query = Query::from_workload(&case)?;
        query.options.limit = maximum;
        query.options.offset = 0;
        query.options.return_mode = ReturnMode::Locator;
        query.options.evidence = false;
        let result = repo.search(&query)?;
        let total = result.count.ok_or("ID dump requires COUNT")?;
        let ids = result
            .hits
            .into_iter()
            .map(|hit| (hit.collection, hit.record_id))
            .collect::<BTreeSet<_>>();
        if ids.len() != total {
            return Err(format!("{} has duplicate or missing IDs", case.id));
        }
        println!(
            "{}",
            serde_json::json!({"id":case.id,"count":total,"ids":ids})
        );
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("membership dump error: {error}");
        std::process::exit(1);
    }
}
