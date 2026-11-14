//! Focused mixed JSONL posting benchmark; accepts an existing fixture root.

use std::path::PathBuf;
use std::time::Instant;

use objekte_search_core::model::LogicalType;
use objekte_search_core::query::{Query, QueryOptions, parse_filter};
use objekte_search_core::reference::{FieldDefinition, Schema};
use objekte_search_core::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions,
};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::workload::ReturnMode;

fn main() {
    if let Err(error) = run() {
        eprintln!("mixed planner benchmark: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().ok_or("fixture root required")?);
    let repetitions: usize = args
        .next()
        .unwrap_or_else(|| "11".into())
        .parse()
        .map_err(|e| format!("invalid repetitions: {e}"))?;
    if repetitions == 0 || args.next().is_some() {
        return Err("expected root and a positive repetition count".into());
    }
    let mut schema = Schema::default();
    schema.register_field(FieldDefinition {
        path: vec!["title".into()],
        kind: LogicalType::String,
        required: true,
        nullable: false,
        indexed: true,
        multi: false,
        element_nullable: Vec::new(),
    })?;
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::JsonLines,
            identity: IdentityPolicy::Generated,
            discovery: Default::default(),
            schema,
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }],
        RepositoryOptions {
            verified_cache_bytes: 0,
            query_workers: 32,
            ..RepositoryOptions::default()
        },
    )?;
    let query = Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(parse_filter("title = 'needle'")?),
        options: QueryOptions {
            sort: vec![],
            limit: 1,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    };
    let warmup = repo.search(&query)?;
    if warmup.count != Some(1) {
        return Err("fixture must have exactly one target".into());
    }
    for repetition in 0..repetitions {
        let started = Instant::now();
        let result = repo.search(&query)?;
        let elapsed_ns = started.elapsed().as_nanos() as u64;
        if result.count != Some(1) || result.hits.len() != 1 {
            return Err("benchmark answer changed".into());
        }
        println!(
            "{}",
            serde_json::json!({"repetition":repetition,"elapsed_ns":elapsed_ns,"counters":result.counters})
        );
    }
    Ok(())
}
