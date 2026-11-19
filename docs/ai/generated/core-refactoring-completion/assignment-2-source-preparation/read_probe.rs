use std::path::PathBuf;

use objekte_search_core::lifecycle::Index;
use objekte_search_core::model::LogicalType;
use objekte_search_core::query::options::ReturnMode;
use objekte_search_core::query::{
    CaseSensitivity, FtsMode, FtsQuery, Query, QueryOptions, parse_filter,
};
use objekte_search_core::repository::configuration::{CollectionDefinition, RepositoryOptions};
use objekte_search_core::schema::FieldDefinition;
use objekte_search_core::source::SourceFormat;
use objekte_search_core::source::registry::SourceRegistry;
use serde_json::json;

fn argument(name: &str) -> Result<String, String> {
    let args = std::env::args().collect::<Vec<_>>();
    args.iter()
        .position(|value| value == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
        .ok_or_else(|| format!("{name} is required"))
}

fn query(filter: Option<&str>, fts: Option<&str>, evidence: bool) -> Query {
    Query {
        collections: vec!["data".into()],
        filter: filter.map(|value| parse_filter(value).unwrap()),
        fts: fts.map(|literal| FtsQuery {
            pattern_parameter: None,
            mode: FtsMode::Literal,
            case_sensitivity: CaseSensitivity::Sensitive,
            literal: literal.into(),
            fields: Vec::new(),
        }),
        options: QueryOptions {
            sort: Vec::new(),
            limit: 20,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Logical,
            evidence,
        },
    }
}

fn run() -> Result<(), String> {
    let root = PathBuf::from(argument("--root")?);
    let tested_sha = argument("--tested-sha")?;
    let mut definition =
        CollectionDefinition::folder("data", "data", SourceFormat::JsonDecodedScalars);
    definition.schema.register_field(FieldDefinition {
        path: vec!["title".into()],
        kind: LogicalType::String,
        required: true,
        nullable: false,
        indexed: true,
        multi: false,
        element_nullable: Vec::new(),
    })?;
    let repository = Index::start_build(
        &root,
        vec![definition],
        RepositoryOptions {
            query_workers: 1,
            verified_cache_bytes: 0,
            ..Default::default()
        },
        SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .map_err(|error| error.to_string())?;
    let view = repository.view().map_err(|error| error.to_string())?;

    let mut exact = query(Some("title = 'hit'"), None, false);
    exact.options.limit = 1;
    let queries = [
        ("exact_page", exact),
        (
            "combined_old",
            query(Some("rating > 0 && n > 0"), Some("old"), true),
        ),
        (
            "combined_number",
            query(Some("rating > 0 && n > 0"), Some("1e0"), true),
        ),
    ];
    let mut results = Vec::new();
    for (name, query) in queries {
        let result = view.search(&query)?;
        results.push(json!({
            "name": name,
            "count": result.count,
            "ids": result.hits.iter().map(|hit| hit.record_id.clone()).collect::<Vec<_>>(),
            "counters": {
                "files_opened": result.counters.files_opened,
                "distinct_files": result.counters.distinct_files,
                "bytes_read": result.counters.bytes_read,
                "record_parses": result.counters.record_parses,
                "candidates": result.counters.candidates,
                "scanned_records": result.counters.scanned_records,
                "residual_evaluations": result.counters.residual_evaluations,
            },
        }));
    }
    println!(
        "{}",
        json!({
            "tested_sha": tested_sha,
            "results": results,
        })
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("assignment-2 read probe: {error}");
        std::process::exit(1);
    }
}
