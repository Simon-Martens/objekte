#[path = "support/musenalm.rs"]
mod musenalm;

use std::path::PathBuf;

use objekte_search_core::query::Query;
use objekte_search_core::repository::{Repository, RepositoryOptions};
use objekte_search_core::workload::{BigramAlphabet, load_manifest};

fn main() {
    if let Err(error) = run() {
        eprintln!("acceptance error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let mut alphabet = BigramAlphabet::Ascii;
    let mut selected: Option<Vec<String>> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = PathBuf::from(args.next().ok_or("--root needs path")?),
            "--alphabet" => {
                alphabet = match args.next().ok_or("--alphabet needs value")?.as_str() {
                    "ascii" => BigramAlphabet::Ascii,
                    "unicode" => BigramAlphabet::Unicode,
                    _ => return Err("alphabet must be ascii or unicode".into()),
                }
            }
            "--cases" => {
                selected = Some(
                    args.next()
                        .ok_or("--cases needs IDs")?
                        .split(',')
                        .map(str::to_owned)
                        .collect(),
                )
            }
            _ => return Err(format!("unknown option {arg}")),
        }
    }
    let definitions = musenalm::load_definitions(&root)?;
    let repo = Repository::open_with_options(
        &root,
        definitions,
        RepositoryOptions {
            bigram_alphabet: alphabet,
            ..RepositoryOptions::default()
        },
    )?;
    let cases = load_manifest(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("benches/musenalm_queries.json"),
    )?;
    let mut checked = 0;
    for case in cases {
        if selected.as_ref().is_some_and(|ids| !ids.contains(&case.id)) {
            continue;
        }
        let query = Query::from_workload(&case)?;
        let optimized = repo
            .search(&query)
            .map_err(|e| format!("{} optimized: {e}", case.id))?;
        let reference = repo
            .search_reference(&query)
            .map_err(|e| format!("{} reference: {e}", case.id))?;
        if optimized.count != reference.count || optimized.hits.len() != reference.hits.len() {
            return Err(format!(
                "{} count/page mismatch: optimized {}/{}, reference {}/{}",
                case.id,
                optimized.count,
                optimized.hits.len(),
                reference.count,
                reference.hits.len()
            ));
        }
        for (a, b) in optimized.hits.iter().zip(&reference.hits) {
            if a.collection != b.collection
                || a.record_id != b.record_id
                || a.generation != b.generation
                || a.file_id != b.file_id
                || a.file_path != b.file_path
                || a.file_size != b.file_size
                || a.file_created_ns != b.file_created_ns
                || a.byte_start != b.byte_start
                || a.byte_end != b.byte_end
                || a.modified_ns != b.modified_ns
                || a.matched_fields != b.matched_fields
                || a.payload != b.payload
            {
                return Err(format!(
                    "{} hit mismatch at {}:{}; optimized evidence {:?}, reference {:?}",
                    case.id, a.collection, a.record_id, a.matched_fields, b.matched_fields
                ));
            }
        }
        checked += 1;
        println!(
            "{} ok count={} page={}",
            case.id,
            optimized.count,
            optimized.hits.len()
        );
    }
    println!(
        "accepted {checked} cases; alphabet={alphabet:?}; snapshot_restored={}",
        repo.startup.restored
    );
    Ok(())
}
