//! Acceptance smoke test for opening Musenalm through the redb index store.

use std::path::PathBuf;

use objekte_search_core::lifecycle::Index;
use objekte_search_core::lifecycle_store::{StoreOpenOutcome, StoreOptions};
use objekte_search_core::query::{Query, QueryOptions, parse_filter};
use objekte_search_core::repository::RepositoryOptions;
use objekte_search_core::source::SourceRegistry;
use objekte_search_core::workload::{BigramAlphabet, ReturnMode};

fn query(collection: &str, filter: &str) -> Query {
    Query {
        collections: vec![collection.into()],
        fts: None,
        filter: Some(parse_filter(filter).unwrap()),
        options: QueryOptions {
            sort: Vec::new(),
            limit: 5,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    }
}

fn run_queries(index: &Index) -> Vec<(Option<usize>, Vec<String>)> {
    [
        query("contents", "language ?= \"ger\""),
        query("contents", "numbering >= 0"),
        query("entries", "year >= 1800 && year < 1850"),
        query("agents", "corporate_body = false"),
        query("items", "media ?= \"Original\""),
        query("series", "entries:length >= 3"),
    ]
    .iter()
    .map(|query| {
        let result = index.view().unwrap().search(query).unwrap();
        (
            result.count,
            result.hits.iter().map(|hit| hit.record_id.clone()).collect(),
        )
    })
    .collect()
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut alphabet = BigramAlphabet::Ascii;
    let mut store_path = None;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--alphabet" => {
                alphabet = match args.next().as_deref() {
                    Some("ascii") => BigramAlphabet::Ascii,
                    Some("unicode") => BigramAlphabet::Unicode,
                    other => return Err(format!("unknown alphabet {other:?}")),
                }
            }
            "--store" => {
                store_path = Some(PathBuf::from(
                    args.next().ok_or("--store needs a path")?,
                ))
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let store_path = store_path.ok_or("--store PATH is required")?;
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let configuration =
        objekte_search_core::repository_configuration::load_repository_configuration(&root)
            .map_err(|error| error.to_string())?;
    let options = RepositoryOptions {
        bigram_alphabet: alphabet,
        ..Default::default()
    };
    let store = StoreOptions {
        path: Some(store_path.clone()),
        durable: true,
    };
    let (first, outcome) = Index::open_persistent(
        &root,
        configuration.definitions.clone(),
        options,
        SourceRegistry::builtin(),
        store.clone(),
    )
    .map_err(|error| error.to_string())?;
    if !matches!(outcome, StoreOpenOutcome::Rebuilt(_)) {
        return Err(format!("first open was {outcome:?}"));
    }
    let expected = run_queries(&first);
    drop(first);
    let (second, outcome) = Index::open_persistent(
        &root,
        configuration.definitions,
        options,
        SourceRegistry::builtin(),
        store,
    )
    .map_err(|error| error.to_string())?;
    if outcome != StoreOpenOutcome::Loaded {
        return Err(format!("second open was {outcome:?}"));
    }
    let actual = run_queries(&second);
    if actual != expected {
        return Err("store results differ from the first open".into());
    }
    println!(
        "accepted {} store queries; alphabet={alphabet:?}",
        expected.len()
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("store acceptance error: {error}");
        std::process::exit(1);
    }
}
