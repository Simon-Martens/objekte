#[path = "support/musenalm.rs"]
mod musenalm;

use std::path::PathBuf;

use objekte_search_core::query::Query;
use objekte_search_core::repository::{Payload, Repository, RepositoryOptions};
use objekte_search_core::workload::{BigramAlphabet, ReturnMode, WorkloadCase, load_manifest};

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../examples/musenalm");
    let mut case_id = None;
    let mut fts = None;
    let mut filter = None;
    let mut collection = "*".to_string();
    let mut explain = false;
    let mut analyze = false;
    let mut reference = false;
    let mut alphabet = BigramAlphabet::Ascii;
    let mut mode_override = None;
    let mut limit_override = None;
    let mut offset_override = None;
    let mut quiet_hits = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = PathBuf::from(args.next().ok_or("--root needs a path")?),
            "--case" => case_id = Some(args.next().ok_or("--case needs an ID")?),
            "--fts" => fts = Some(args.next().ok_or("--fts needs a literal")?),
            "--filter" => filter = Some(args.next().ok_or("--filter needs an expression")?),
            "--collection" => collection = args.next().ok_or("--collection needs a name")?,
            "--explain" => explain = true,
            "--explain-analyze" => analyze = true,
            "--reference" => reference = true,
            "--quiet-hits" => quiet_hits = true,
            "--return" => {
                mode_override = Some(match args.next().ok_or("--return needs a mode")?.as_str() {
                    "locator" => ReturnMode::Locator,
                    "bytes" => ReturnMode::Bytes,
                    "logical" => ReturnMode::Logical,
                    other => return Err(format!("unknown return mode {other}")),
                })
            }
            "--limit" => {
                limit_override = Some(
                    args.next()
                        .ok_or("--limit needs a number")?
                        .parse::<usize>()
                        .map_err(|e| e.to_string())?,
                )
            }
            "--offset" => {
                offset_override = Some(
                    args.next()
                        .ok_or("--offset needs a number")?
                        .parse::<usize>()
                        .map_err(|e| e.to_string())?,
                )
            }
            "--alphabet" => {
                alphabet = match args
                    .next()
                    .ok_or("--alphabet needs ascii or unicode")?
                    .as_str()
                {
                    "ascii" => BigramAlphabet::Ascii,
                    "unicode" => BigramAlphabet::Unicode,
                    other => return Err(format!("unknown alphabet {other}")),
                }
            }
            _ => return Err(format!("unknown option {arg}")),
        }
    }
    let definitions = musenalm::load_definitions(&root)?;
    let mut case = if let Some(id) = case_id {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("benches/musenalm_queries.json");
        load_manifest(&path)?
            .into_iter()
            .find(|c| c.id == id)
            .ok_or_else(|| format!("unknown case {id}"))?
    } else {
        WorkloadCase {
            id: "adhoc".into(),
            collection,
            fts,
            filter,
            fts_fields: Vec::new(),
            sort: Vec::new(),
            limit: 50,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        }
    };
    if let Some(mode) = mode_override {
        case.return_mode = mode;
    }
    if let Some(limit) = limit_override {
        case.limit = limit;
    }
    if let Some(offset) = offset_override {
        case.offset = offset;
    }
    let query = Query::from_workload(&case)?;
    let selected = if case.collection == "*" {
        definitions
    } else {
        definitions
            .into_iter()
            .filter(|d| d.name == case.collection)
            .collect()
    };
    if selected.is_empty() {
        return Err(format!("unknown collection {}", case.collection));
    }
    let repository = Repository::open_with_options(
        &root,
        selected,
        RepositoryOptions {
            bigram_alphabet: alphabet,
            ..RepositoryOptions::default()
        },
    )?;
    for state in &repository.collections {
        println!(
            "collection={} files={} records={} format={:?} fields={} indexed={} custom={}",
            state.definition.name,
            state.generation.files.len(),
            state.generation.records.len(),
            state.definition.format,
            state.definition.schema.fields.len(),
            state
                .definition
                .schema
                .fields
                .values()
                .filter(|f| f.indexed)
                .count(),
            state.definition.schema.custom.len()
        );
    }
    println!(
        "build_parses={} root={} alphabet={alphabet:?}",
        repository.build_parses,
        repository.files.root.display()
    );
    if explain && !reference {
        println!(
            "{}",
            serde_json::to_string_pretty(&repository.explain(&query)?).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    let result = if reference {
        repository.search_reference(&query)?
    } else if analyze {
        let (result, traces) = repository.explain_analyze(&query)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&traces).map_err(|e| e.to_string())?
        );
        result
    } else {
        repository.search(&query)?
    };
    println!(
        "case={} count={} returned={} scanned={} parsed={} bytes_read={}",
        case.id,
        result.count,
        result.hits.len(),
        result.counters.scanned_records,
        result.counters.record_parses,
        result.counters.bytes_read
    );
    for hit in result.hits.iter().filter(|_| !quiet_hits) {
        println!(
            "{}:{} {} {}..{} {:?}",
            hit.collection,
            hit.record_id,
            hit.file_path.display(),
            hit.byte_start,
            hit.byte_end,
            hit.matched_fields
        );
        match &hit.payload {
            Some(Payload::Bytes(bytes)) => println!(
                "  bytes_hex={}",
                bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
            ),
            Some(Payload::Logical(value)) => println!(
                "  logical={}",
                serde_json::to_string(value).map_err(|e| e.to_string())?
            ),
            None => {}
        }
    }
    Ok(())
}
