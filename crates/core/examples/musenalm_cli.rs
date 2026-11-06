#[path = "support/musenalm.rs"]
mod musenalm;

use std::collections::HashMap;
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
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
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
    let mut parameters = HashMap::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!(
                    "Usage: musenalm_cli [--root PATH] [--collection NAME] [--fts TEXT] [--filter FILTER] [--param NAME=JSON] [--limit N] [--offset N] [--return locator|bytes|logical] [--explain] [--explain-analyze]"
                );
                println!(
                    "FILTER accepts a final SORT BY field [ASC|DESC], ... clause or SORT BY alone. Sort fields must have registered scalar sort columns in every selected collection."
                );
                println!(
                    "Parameters are typed JSON values. Use a type/value object for date, datetime or unixtime; shell-quote JSON strings."
                );
                return Ok(());
            }
            "--root" => root = PathBuf::from(args.next().ok_or("--root needs a path")?),
            "--case" => case_id = Some(args.next().ok_or("--case needs an ID")?),
            "--fts" => fts = Some(args.next().ok_or("--fts needs a literal")?),
            "--filter" => filter = Some(args.next().ok_or("--filter needs an expression")?),
            "--param" => {
                let assignment = args.next().ok_or("--param needs NAME=JSON")?;
                let (name, json) = assignment
                    .split_once('=')
                    .ok_or("--param needs NAME=JSON")?;
                if name.is_empty() || parameters.contains_key(name) {
                    return Err(format!("empty or duplicate parameter name: {name}"));
                }
                parameters.insert(
                    name.to_owned(),
                    objekte_search_core::parameters::parse_parameter_json(json)?,
                );
            }
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
            fts_case_sensitivity: objekte_search_core::query::CaseSensitivity::Sensitive,
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
    let query = Query::from_workload(&case)?.with_parameters(&parameters)?;
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
    for state in repository.collections() {
        println!(
            "collection={} files={} records={} format={:?} fields={} indexed={} custom={}",
            state.definition.name,
            state.generation.files.len(),
            state.generation.records.len(),
            state.definition.format,
            state.definition.schema.fields().len(),
            state
                .definition
                .schema
                .fields()
                .values()
                .filter(|f| f.indexed)
                .count(),
            state.definition.schema.custom_bindings().len()
        );
    }
    println!(
        "build_parses={} build_failed_parses={} root={} alphabet={alphabet:?}",
        repository.build_parses(),
        repository.build_failed_parses(),
        repository.files().root.display()
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
        result.count.ok_or("CLI request requires COUNT")?,
        result.hits.len(),
        result.counters.scanned_records,
        result.counters.record_parses,
        result.counters.bytes_read
    );
    for hit in result.hits.iter().filter(|_| !quiet_hits) {
        println!(
            "{}:{} generation={} file_id={} {} {}..{} size={} created_ns={:?} modified_ns={} status={}({}) class={} diagnostic_ref={} {:?}",
            hit.collection,
            hit.record_id,
            hit.generation,
            hit.file_id,
            hit.file_path.display(),
            hit.byte_start,
            hit.byte_end,
            hit.file_size,
            hit.file_created_ns,
            hit.modified_ns,
            hit.file_status.label(),
            hit.file_status.code(),
            hit.record_class.code(),
            hit.diagnostic_ref.0,
            hit.matched_fields
        );
        if let Some(issue) = repository.files().diagnostic(
            objekte_search_core::model::FileId(hit.file_id),
            hit.diagnostic_ref,
        ) {
            println!(
                "  diagnostic={} {:?}: {}",
                issue.code, issue.category, issue.message
            );
        }
        match &hit.payload {
            Some(Payload::Bytes(bytes)) => println!(
                "  bytes_hex={}",
                bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
            ),
            Some(Payload::Logical(value)) => println!(
                "  logical={}",
                serde_json::to_string(value).map_err(|e| e.to_string())?
            ),
            Some(Payload::LogicalUnavailable { .. }) => println!("  logical=unavailable"),
            None => {}
        }
    }
    Ok(())
}
