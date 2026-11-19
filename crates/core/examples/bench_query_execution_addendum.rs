#[path = "support/musenalm.rs"]
mod musenalm;

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use objekte_search_core::index::bigram::BigramAlphabet;
use objekte_search_core::lifecycle::Index;
use objekte_search_core::model::{FieldPath, LogicalType};
use objekte_search_core::query::options::{ReturnMode, SortField};
use objekte_search_core::query::{
    CaseSensitivity, FtsField, FtsMode, FtsQuery, Query, QueryOptions, parse_filter,
};
use objekte_search_core::query_execution::streaming::{
    CancellationToken, StreamControl, StreamEvent, StreamOptions, StreamOutcome,
};
use objekte_search_core::repository::configuration::{
    CollectionDefinition, IdentityPolicy, RepositoryOptions,
};
use objekte_search_core::schema::relations::{RelationCardinality, RelationDefinition};
use objekte_search_core::schema::{FieldDefinition, Schema};
use objekte_search_core::source::SourceFormat;

fn field(schema: &mut Schema, name: &str, kind: LogicalType) {
    schema
        .register_field(FieldDefinition {
            path: vec![name.into()],
            kind,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
}

fn definition(name: &str, schema: Schema) -> CollectionDefinition {
    CollectionDefinition {
        name: name.into(),
        source_path: name.into(),
        format: SourceFormat::Json,
        identity: IdentityPolicy::Intrinsic(vec!["id".into()]),
        discovery: Default::default(),
        schema,
        sort_fields: Vec::new(),
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    }
}

fn q141() -> Query {
    Query {
        collections: vec!["entries".into()],
        fts: Some(FtsQuery {
            pattern_parameter: None,
            mode: FtsMode::Literal,
            case_sensitivity: CaseSensitivity::Sensitive,
            literal: "Almanach".into(),
            fields: Vec::new(),
        }),
        filter: Some(parse_filter("year >= 1800 && year < 1850").unwrap()),
        options: QueryOptions {
            sort: vec![SortField::simple("year", false)],
            limit: 50,
            offset: 0,
            count: false,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    }
}

fn ordered_timing(index: &Index, count: bool) -> Result<serde_json::Value, String> {
    let view = index.view().map_err(|error| error.to_string())?;
    let mut query = q141();
    query.options.count = count;
    view.search(&query)?;
    let started = Instant::now();
    let mut first_ns = None;
    let mut first_observed_matches = None;
    let mut returned = 0usize;
    let mut digest = DefaultHasher::new();
    let outcome = view
        .search_stream(
            &query,
            StreamOptions {
                batch_rows: 10,
                ..Default::default()
            },
            &CancellationToken::new(),
            &mut |event| {
                if let StreamEvent::OrderedRows { rows, progress, .. } = event {
                    if !rows.is_empty() {
                        first_ns.get_or_insert_with(|| started.elapsed().as_nanos() as u64);
                        first_observed_matches.get_or_insert(progress.observed_matches);
                    }
                    returned += rows.len();
                    for row in rows {
                        row.record_id.hash(&mut digest);
                        row.matched_fields.hash(&mut digest);
                    }
                }
                Ok(StreamControl::Continue)
            },
        )
        .map_err(|error| error.to_string())?;
    let completion_ns = started.elapsed().as_nanos() as u64;
    let StreamOutcome::Complete(summary) = outcome else {
        return Err("ordered stream did not complete".into());
    };
    Ok(serde_json::json!({
        "case": if count { "q141-count-on" } else { "q141-count-off" },
        "first_rows_ns": first_ns,
        "first_observed_matches": first_observed_matches,
        "completion_ns": completion_ns,
        "returned_rows": returned,
        "digest": format!("{:016x}", digest.finish()),
        "count": summary.count,
        "counters": summary.counters,
    }))
}

fn relation_case() -> Result<serde_json::Value, String> {
    let root = std::env::temp_dir().join(format!(
        "objekte-addendum-relation-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    for name in ["sources", "targets"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }
    for (name, id, text) in [
        ("author", "AUTHOR", "Needle"),
        ("editor", "EDITOR", "Needle"),
    ] {
        std::fs::write(
            root.join(format!("targets/{name}.json")),
            format!(r#"{{"id":"{id}","name":"{text}"}}"#),
        )
        .unwrap();
    }
    for (name, id, author, editor) in [
        ("a", "A", "AUTHOR", "EDITOR"),
        ("b", "B", "AUTHOR", "EDITOR"),
        ("c", "C", "AUTHOR", "EDITOR"),
    ] {
        std::fs::write(
            root.join(format!("sources/{name}.json")),
            format!(
                r#"{{"id":"{id}","author":"{author}","editor":"{editor}","padding":"{}"}}"#,
                "x".repeat(512)
            ),
        )
        .unwrap();
    }
    let mut targets = Schema::default();
    field(&mut targets, "id", LogicalType::Id);
    field(&mut targets, "name", LogicalType::String);
    let mut sources = Schema::default();
    field(&mut sources, "id", LogicalType::Id);
    field(&mut sources, "author", LogicalType::Id);
    field(&mut sources, "editor", LogicalType::Id);
    sources
        .register_field(FieldDefinition {
            path: vec!["padding".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: false,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    for (path, alias) in [("author", "authored"), ("editor", "edited")] {
        sources
            .register_relation(RelationDefinition {
                path: vec![path.into()],
                target_collection: "targets".into(),
                cardinality: RelationCardinality::One,
                reverse_alias: Some(alias.into()),
            })
            .unwrap();
    }
    let index = Index::start_build(
        &root,
        vec![
            definition("sources", sources),
            definition("targets", targets),
        ],
        RepositoryOptions {
            verified_cache_bytes: 0,
            ..Default::default()
        },
        objekte_search_core::source::registry::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .map_err(|error| error.to_string())?;
    let view = index.view().map_err(|error| error.to_string())?;
    let query = Query {
        collections: vec!["sources".into()],
        fts: Some(FtsQuery {
            pattern_parameter: None,
            mode: FtsMode::Literal,
            case_sensitivity: CaseSensitivity::Sensitive,
            literal: "Needle".into(),
            fields: ["author.name", "editor.name"]
                .into_iter()
                .map(|path| FtsField {
                    path: FieldPath::parse(path).unwrap(),
                    binding: None,
                    query_path: None,
                })
                .collect(),
        }),
        filter: None,
        options: QueryOptions {
            sort: Vec::new(),
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    };
    let expected = view.search_reference(&query)?;
    let mut samples = Vec::new();
    let mut actual = None;
    for _ in 0..11 {
        let started = Instant::now();
        let result = view.search(&query)?;
        samples.push(started.elapsed().as_nanos() as u64);
        actual = Some(result);
    }
    let actual = actual.ok_or("missing relation result")?;
    if actual.hits != expected.hits || actual.count != expected.count {
        return Err("relation case differs from reference".into());
    }
    samples.sort_unstable();
    drop(view);
    index.shutdown().map_err(|error| error.to_string())?;
    std::fs::remove_dir_all(root).unwrap();
    Ok(serde_json::json!({
        "case": "multi-field-relation-fts",
        "median_ns": samples[samples.len() / 2],
        "count": actual.count,
        "counters": actual.counters,
    }))
}

fn residual_order_cases() -> Result<Vec<serde_json::Value>, String> {
    let root = std::env::temp_dir().join(format!(
        "objekte-addendum-residual-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let mut source = String::new();
    for score in 0..1000 {
        source.push_str(&format!(
            r#"{{"body":"{}","score":{score}}}"#,
            if score < 500 { "match" } else { "miss" }
        ));
        source.push('\n');
    }
    std::fs::write(root.join("data/all.jsonl"), source).unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["body".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: false,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    schema
        .register_field(FieldDefinition {
            path: vec!["score".into()],
            kind: LogicalType::Number,
            required: true,
            nullable: false,
            indexed: false,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    let index = Index::start_build(
        &root,
        vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::JsonLines,
            identity: IdentityPolicy::Generated,
            discovery: Default::default(),
            schema,
            sort_fields: vec![FieldPath::parse("score").unwrap()],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }],
        RepositoryOptions {
            verified_cache_bytes: 0,
            ..Default::default()
        },
        objekte_search_core::source::registry::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .map_err(|error| error.to_string())?;
    let view = index.view().map_err(|error| error.to_string())?;
    let mut results = Vec::new();
    for count in [false, true] {
        let query = Query {
            collections: vec!["data".into()],
            fts: None,
            filter: Some(parse_filter("body != 'miss'").unwrap()),
            options: QueryOptions {
                sort: vec![SortField::simple("score", false)],
                limit: 1,
                offset: 0,
                count,
                return_mode: ReturnMode::Bytes,
                evidence: true,
            },
        };
        view.search(&query)?;
        let started = Instant::now();
        let mut first_ns = None;
        let mut first_observed_matches = None;
        let outcome = view
            .search_stream(
                &query,
                StreamOptions::default(),
                &CancellationToken::new(),
                &mut |event| {
                    if let StreamEvent::OrderedRows { rows, progress, .. } = event {
                        if !rows.is_empty() {
                            first_ns.get_or_insert_with(|| started.elapsed().as_nanos() as u64);
                            first_observed_matches.get_or_insert(progress.observed_matches);
                        }
                    }
                    Ok(StreamControl::Continue)
                },
            )
            .map_err(|error| error.to_string())?;
        let completion_ns = started.elapsed().as_nanos() as u64;
        let StreamOutcome::Complete(summary) = outcome else {
            return Err("residual stream did not complete".into());
        };
        results.push(serde_json::json!({
            "case": if count { "stored-residual-count-on" } else { "stored-residual-count-off" },
            "first_rows_ns": first_ns,
            "first_observed_matches": first_observed_matches,
            "completion_ns": completion_ns,
            "count": summary.count,
            "counters": summary.counters,
        }));
    }
    drop(view);
    index.shutdown().map_err(|error| error.to_string())?;
    std::fs::remove_dir_all(root).unwrap();
    Ok(results)
}

fn main() -> Result<(), String> {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm")
        });
    let (definitions, _) = musenalm::load_extended_definitions_with_schemas(&root)?;
    let index = Index::start_build(
        &root,
        definitions,
        RepositoryOptions {
            bigram_alphabet: BigramAlphabet::Unicode,
            query_workers: 4,
            ..Default::default()
        },
        objekte_search_core::source::registry::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .map_err(|error| error.to_string())?;
    println!("{}", ordered_timing(&index, false)?);
    println!("{}", ordered_timing(&index, true)?);
    index.shutdown().map_err(|error| error.to_string())?;
    println!("{}", relation_case()?);
    for result in residual_order_cases()? {
        println!("{result}");
    }
    Ok(())
}
