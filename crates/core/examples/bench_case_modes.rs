#[path = "support/musenalm.rs"]
mod musenalm;

use std::path::PathBuf;
use std::time::Instant;

use objekte_search_core::model::FieldPath;
use objekte_search_core::query::{
    CaseSensitivity, FtsField, FtsQuery, Query, QueryOptions, parse_filter,
};
use objekte_search_core::repository::{Repository, RepositoryOptions, SnapshotMode};
use objekte_search_core::workload::{BigramAlphabet, ReturnMode};


#[derive(Clone, Copy)]
struct Case {
    label: &'static str,
    collection: &'static str,
    literal: Option<&'static str>,
    field: Option<&'static str>,
    filter: Option<&'static str>,
    mode: CaseSensitivity,
}


fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for literal in ["Der", "Liebe", "Goethe", "Müller", "Frühling", "ß", "ss"] {
        for mode in [CaseSensitivity::Sensitive, CaseSensitivity::Insensitive] {
            cases.push(Case {
                label: "raw_fts",
                collection: "*",
                literal: Some(literal),
                field: None,
                filter: None,
                mode,
            });
        }
    }
    for (label, collection, literal, field, filter) in [
        (
            "selected_name",
            "agents",
            Some("Goethe"),
            Some("name"),
            None,
        ),
        (
            "selected_folded",
            "agents",
            Some("GOETHE"),
            Some("name:folded"),
            None,
        ),
        (
            "selected_title",
            "entries",
            Some("FRÜHLING"),
            Some("preferred_title:lower"),
            None,
        ),
        ("filter_name", "agents", None, None, Some("name ~ 'Goethe'")),
        (
            "filter_lower",
            "agents",
            None,
            None,
            Some("name:lower ~ 'goethe'"),
        ),
        (
            "filter_folded",
            "agents",
            None,
            None,
            Some("name:folded ~ 'GOETHE'"),
        ),
        (
            "filter_title_lower",
            "entries",
            None,
            None,
            Some("preferred_title:lower ~ 'FRÜHLING'"),
        ),
        (
            "combined",
            "agents",
            Some("Goethe"),
            None,
            Some("name:lower ~ 'goethe'"),
        ),
        (
            "exact_posting_control",
            "agents",
            None,
            None,
            Some("name = 'Johann Wolfgang Goethe'"),
        ),
    ] {
        for mode in [CaseSensitivity::Sensitive, CaseSensitivity::Insensitive] {
            cases.push(Case {
                label,
                collection,
                literal,
                field,
                filter,
                mode,
            });
        }
    }
    cases
}

fn query(case: Case) -> Result<Query, String> {
    let fields = case
        .field
        .map(|name| {
            let (path, binding) = name
                .rsplit_once(':')
                .map_or((name, None), |(path, binding)| {
                    (path, Some(binding.to_owned()))
                });
            FieldPath::parse(path).map(|path| FtsField {
                query_path: None, path, binding })
        })
        .transpose()?
        .into_iter()
        .collect();
    Ok(Query {
        collections: if case.collection == "*" {
            Vec::new()
        } else {
            vec![case.collection.into()]
        },
        fts: case.literal.map(|literal| FtsQuery {
            pattern_parameter: None,
            mode: objekte_search_core::query::FtsMode::Literal,
            case_sensitivity: case.mode,
            literal: literal.into(),
            fields,
        }),
        filter: case.filter.map(parse_filter).transpose()?,
        options: QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    })
}

fn store_counts(repo: &Repository) -> (usize, usize) {
    repo.collections()
        .iter()
        .fold((0, 0), |(stores, grams), collection| {
            let indexes = std::iter::once(&collection.raw)
                .chain(std::iter::once(&collection.fallback_text))
                .chain(collection.custom_text.values());
            indexes.fold((stores, grams), |(stores, grams), index| {
                (stores + 1, grams + index.normal.len() + index.skip.len())
            })
        })
}

fn main() {
    if let Err(error) = run() {
        eprintln!("case-mode benchmark error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let mut repetitions = 5usize;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = PathBuf::from(args.next().ok_or("--root needs a path")?),
            "--repetitions" => {
                repetitions = args
                    .next()
                    .ok_or("--repetitions needs a number")?
                    .parse()
                    .map_err(|e| format!("invalid repetitions: {e}"))?
            }
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    let revision = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(|e| e.to_string())?;
    let revision = String::from_utf8(revision.stdout)
        .map_err(|e| e.to_string())?
        .trim()
        .to_owned();
    let definitions = musenalm::load_definitions(&root)?;
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let repo = Repository::open_with_options(
            &root,
            definitions.clone(),
            RepositoryOptions {
                bigram_alphabet: alphabet,
                snapshot_mode: SnapshotMode::IfValid,
                query_workers: 32,
                ..Default::default()
            },
        )?;
        let original_stores = store_counts(&repo);
        for cache in ["off", "warm"] {
            repo.configure_verified_cache(if cache == "warm" { 64 << 20 } else { 0 })?;
            for case in cases() {
                let request = query(case)?;
                let (diagnostic, plans) = repo.explain_analyze(&request)?;
                if cache == "warm" {
                    repo.search(&request)?;
                }
                let mut samples = Vec::new();
                let mut observed = None;
                for _ in 0..repetitions {
                    let start = Instant::now();
                    let result = repo.search(&request)?;
                    samples.push(start.elapsed().as_nanos() as u64);
                    if result.count != diagnostic.count || result.hits != diagnostic.hits {
                        return Err(format!(
                            "case-mode result changed: {} {alphabet:?} {cache}",
                            case.label
                        ));
                    }
                    observed = Some(result.counters);
                }
                samples.sort_unstable();
                println!(
                    "{}",
                    serde_json::json!({
                        "revision": revision, "alphabet": alphabet, "cache": cache,
                        "label": case.label, "collection": case.collection, "fts": case.literal,
                        "field": case.field, "filter": case.filter, "case_sensitivity": case.mode,
                        "samples_ns": samples, "median_ns": samples[samples.len()/2],
                        "counters": observed, "diagnostic_counters": diagnostic.counters,
                        "count": diagnostic.count, "returned": diagnostic.hits.len(), "plans": plans,
                        "index_stores": original_stores.0, "distinct_grams": original_stores.1,
                    })
                );
            }
        }
        if store_counts(&repo) != original_stores {
            return Err("case toggle rebuilt an index".into());
        }
    }
    Ok(())
}
