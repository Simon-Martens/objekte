//! Measure the unindexed source phase used before the desktop index is ready.

#[path = "support/musenalm.rs"]
mod musenalm;

use std::path::PathBuf;
use std::time::Instant;

use objekte_search_core::query::{CaseSensitivity, FtsMode, FtsQuery, Query, QueryOptions};
use objekte_search_core::source_scan::SourceScan;
use objekte_search_core::streaming::CancellationToken;
use objekte_search_core::workload::ReturnMode;

fn main() {
    if let Err(error) = run() {
        eprintln!("source scan benchmark: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm")
        });
    let definitions = musenalm::load_definitions(&root)?;
    let started = Instant::now();
    let scan = SourceScan::open(&root, definitions)?;
    let prepare_ns = started.elapsed().as_nanos();
    let query = Query {
        collections: Vec::new(),
        fts: Some(FtsQuery {
            case_sensitivity: CaseSensitivity::Sensitive,
            mode: FtsMode::Literal,
            literal: "Musenalm".into(),
            pattern_parameter: None,
            fields: Vec::new(),
        }),
        filter: None,
        options: QueryOptions {
            sort: Vec::new(),
            limit: 100,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Bytes,
            evidence: true,
        },
    };
    let started = Instant::now();
    let result = scan.search(&query, &CancellationToken::new())?;
    let search_ns = started.elapsed().as_nanos();
    println!(
        "{}",
        serde_json::json!({
            "root": scan.root(),
            "collections": scan.collections().count(),
            "files": scan.file_paths().count(),
            "prepare_ns": prepare_ns,
            "search_ns": search_ns,
            "count": result.count,
            "rows": result.hits.len(),
            "scanned_records": result.counters.scanned_records,
        })
    );
    Ok(())
}
