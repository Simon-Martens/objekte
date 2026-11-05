#[path = "support/musenalm.rs"]
mod musenalm;

use std::path::PathBuf;

use objekte_search_core::query::Query;
use objekte_search_core::repository::Repository;
use objekte_search_core::workload::{ReturnMode, WorkloadCase, load_manifest};

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
    let mut reference = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = PathBuf::from(args.next().ok_or("--root needs a path")?),
            "--case" => case_id = Some(args.next().ok_or("--case needs an ID")?),
            "--fts" => fts = Some(args.next().ok_or("--fts needs a literal")?),
            "--filter" => filter = Some(args.next().ok_or("--filter needs an expression")?),
            "--collection" => collection = args.next().ok_or("--collection needs a name")?,
            "--explain" | "--explain-analyze" => explain = true,
            "--reference" => reference = true,
            _ => return Err(format!("unknown option {arg}")),
        }
    }
    let definitions = musenalm::load_definitions(&root)?;
    let case = if let Some(id) = case_id {
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
    let repository = Repository::open(&root, selected)?;
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
        "build_parses={} root={}",
        repository.build_parses,
        repository.files.root.display()
    );
    if explain {
        println!("reference scan: every eligible record; typed predicates bound before execution");
    }
    let result = if reference {
        repository.search_reference(&query)?
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
    for hit in &result.hits {
        println!(
            "{}:{} {} {}..{} {:?}",
            hit.collection,
            hit.record_id,
            hit.file_path.display(),
            hit.byte_start,
            hit.byte_end,
            hit.matched_fields
        );
    }
    Ok(())
}
