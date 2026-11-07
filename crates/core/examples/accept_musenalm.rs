#[path = "support/musenalm.rs"]
mod musenalm;

use std::collections::BTreeMap;
use std::path::PathBuf;

use objekte_search_core::repository::{Repository, RepositoryOptions, SearchHit};
use objekte_search_core::workload::{
    BenchmarkCase, BigramAlphabet, load_extended_manifest, load_manifest,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("acceptance error: {error}");
        std::process::exit(1);
    }
}

fn hit_difference(actual: &SearchHit, expected: &SearchHit) -> Option<String> {
    macro_rules! field {
        ($name:ident) => {
            if actual.$name != expected.$name {
                return Some(format!(
                    "{}: optimized {:?}, reference {:?}",
                    stringify!($name),
                    actual.$name,
                    expected.$name
                ));
            }
        };
    }
    field!(collection);
    field!(record_id);
    field!(generation);
    field!(file_id);
    field!(file_path);
    field!(file_size);
    field!(file_created_ns);
    field!(byte_start);
    field!(byte_end);
    field!(modified_ns);
    field!(matched_fields);
    field!(file_status);
    field!(record_class);
    field!(diagnostic_ref);
    field!(payload);
    None
}

fn run() -> Result<(), String> {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let mut alphabet = BigramAlphabet::Ascii;
    let mut selected: Option<Vec<String>> = None;
    let mut extended = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--extended" => extended = true,
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
    let definitions = if extended {
        musenalm::load_extended_definitions(&root)?
    } else {
        musenalm::load_definitions(&root)?
    };
    let repo = Repository::open_with_options(
        &root,
        definitions,
        RepositoryOptions {
            bigram_alphabet: alphabet,
            ..RepositoryOptions::default()
        },
    )?;
    let baseline_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("benches/musenalm_queries.json");
    let cases = if extended {
        load_extended_manifest(
            &baseline_path,
            &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("benches/musenalm_queries_extended.json"),
        )?
    } else {
        load_manifest(&baseline_path)?
            .into_iter()
            .map(|request| BenchmarkCase {
                request,
                parameters: BTreeMap::new(),
                clock_unix_seconds: None,
            })
            .collect()
    };
    let mut checked = 0;
    for case in cases {
        if selected.as_ref().is_some_and(|ids| !ids.contains(&case.id)) {
            continue;
        }
        let query = case.query()?;
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
                optimized.count.ok_or("acceptance requires COUNT")?,
                optimized.hits.len(),
                reference.count.ok_or("acceptance requires COUNT")?,
                reference.hits.len()
            ));
        }
        for (index, (a, b)) in optimized.hits.iter().zip(&reference.hits).enumerate() {
            // Full equality catches future SearchHit fields even before this
            // field-level explanation is extended for them.
            if a != b {
                return Err(format!(
                    "{} hit {index} mismatch at {}:{}: {}",
                    case.id,
                    a.collection,
                    a.record_id,
                    hit_difference(a, b).unwrap_or_else(|| format!(
                        "new field differs: optimized {a:?}, reference {b:?}"
                    ))
                ));
            }
        }
        checked += 1;
        println!(
            "{} ok count={} page={}",
            case.id,
            optimized.count.ok_or("acceptance requires COUNT")?,
            optimized.hits.len()
        );
    }
    println!(
        "accepted {checked} cases; alphabet={alphabet:?}; snapshot_restored={}",
        repo.startup().restored
    );
    Ok(())
}
