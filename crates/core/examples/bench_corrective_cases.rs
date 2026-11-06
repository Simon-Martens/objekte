//! Focused measurements for corrective range, page, and JSONL scan paths.

use std::fs;
use std::time::Instant;

use objekte_search_core::model::{FieldPath, LogicalType};
use objekte_search_core::query::{FtsQuery, Query, QueryOptions, parse_filter};
use objekte_search_core::reference::{FieldDefinition, Schema};
use objekte_search_core::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::workload::{ReturnMode, SortField};

fn main() {
    if let Err(error) = run() {
        eprintln!("corrective benchmark error: {error}");
        std::process::exit(1);
    }
}

fn definition(
    name: &str,
    format: SourceFormat,
    schema: Schema,
    sort: bool,
) -> CollectionDefinition {
    CollectionDefinition {
        name: name.into(),
        source_path: name.into(),
        format,
        identity: IdentityPolicy::Generated,
        discovery: Default::default(),
        schema,
        sort_fields: if sort {
            vec![FieldPath::parse("num").unwrap()]
        } else {
            vec![]
        },
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    }
}

fn measure(repo: &Repository, name: &str, query: &Query, repetitions: usize) -> Result<(), String> {
    let expected = repo.search_reference(query)?;
    let mut samples = Vec::new();
    let mut counters = None;
    for _ in 0..repetitions {
        let started = Instant::now();
        let result = repo.search(query)?;
        samples.push(started.elapsed().as_nanos() as u64);
        if result.count != expected.count || result.hits != expected.hits {
            return Err(format!(
                "{name}: indexed result differs from independent scanner"
            ));
        }
        counters = Some(result.counters);
    }
    samples.sort_unstable();
    println!(
        "{}",
        serde_json::json!({
            "case": name,
            "repetitions": repetitions,
            "median_ns": samples[samples.len() / 2],
            "min_ns": samples[0],
            "max_ns": samples[samples.len() - 1],
            "samples_ns": samples,
            "count": expected.count,
            "page_size": expected.hits.len(),
            "counters": counters.unwrap(),
            "peak_resident_bytes": memory_bytes("VmHWM:"),
        })
    );
    Ok(())
}

fn run() -> Result<(), String> {
    let root =
        std::env::temp_dir().join(format!("objekte-corrective-bench-{}", std::process::id()));
    fs::create_dir_all(root.join("range")).map_err(|e| e.to_string())?;
    fs::create_dir_all(root.join("left")).map_err(|e| e.to_string())?;
    fs::create_dir_all(root.join("right")).map_err(|e| e.to_string())?;
    let mut numeric = String::new();
    for num in 0..10_000 {
        numeric.push_str(&format!(r#"{{"num":{num},"kind":"yes"}}"#));
        numeric.push('\n');
    }
    fs::write(root.join("range/data.jsonl"), numeric).map_err(|e| e.to_string())?;
    let padding = "x".repeat(64);
    for name in ["left", "right"] {
        let mut body = String::new();
        for num in 0..2_000 {
            let rare = if num % 500 == 0 { " rare-token" } else { "" };
            body.push_str(&format!(
                r#"{{"body":"needle{rare}","padding":"{padding}"}}"#
            ));
            body.push('\n');
        }
        fs::write(root.join(format!("{name}/data.jsonl")), body).map_err(|e| e.to_string())?;
    }
    let mut schema = Schema::default();
    for (path, kind) in [("num", LogicalType::Number), ("kind", LogicalType::String)] {
        schema.register_field(FieldDefinition {
            path: vec![path.into()],
            kind,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })?;
    }
    let repo = Repository::open_with_options(
        &root,
        vec![
            definition("range", SourceFormat::JsonLines, schema, true),
            definition("left", SourceFormat::JsonLines, Schema::default(), false),
            definition("right", SourceFormat::JsonLines, Schema::default(), false),
        ],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            query_workers: 4,
            ..Default::default()
        },
    )?;
    repo.configure_verified_cache(0)?;
    let mut range = Query {
        collections: vec!["range".into()],
        fts: None,
        filter: Some(parse_filter("num >= 9990 && num <= 9998")?),
        options: QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    measure(&repo, "high-cardinality-range", &range, 31)?;
    range.filter = Some(parse_filter("kind = 'yes'")?);
    range.options.sort = vec![SortField {
        field: "num".into(),
        descending: false,
    }];
    range.options.limit = 5;
    range.options.offset = 7;
    range.options.count = false;
    measure(&repo, "page-only-stored-order", &range, 31)?;
    range.options.count = true;
    measure(&repo, "counted-stored-order", &range, 31)?;
    let mut scan = Query {
        collections: vec!["left".into(), "right".into()],
        fts: Some(FtsQuery {
            literal: "needle".into(),
            fields: vec![],
        }),
        filter: None,
        options: QueryOptions {
            sort: vec![],
            limit: 5,
            offset: 7,
            count: true,
            return_mode: ReturnMode::Logical,
            evidence: false,
        },
    };
    measure(&repo, "jsonl-dense-logical", &scan, 11)?;
    scan.options.offset = 1_000;
    measure(&repo, "jsonl-deep-offset", &scan, 11)?;
    scan.options.offset = 0;
    scan.options.return_mode = ReturnMode::Locator;
    measure(&repo, "jsonl-locator", &scan, 11)?;
    scan.options.return_mode = ReturnMode::Logical;
    scan.options.limit = 0;
    measure(&repo, "jsonl-count-only", &scan, 11)?;
    scan.options.limit = 5;
    scan.fts.as_mut().unwrap().literal = "rare-token".into();
    measure(&repo, "jsonl-sparse-logical", &scan, 11)?;
    fs::remove_dir_all(&root).map_err(|e| e.to_string())?;
    Ok(())
}

fn memory_bytes(name: &str) -> Option<u64> {
    fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix(name)?
                .trim()
                .strip_suffix(" kB")?
                .trim()
                .parse::<u64>()
                .ok()
                .map(|value| value * 1_024)
        })
}
