use std::collections::HashMap;
use std::path::PathBuf;

use objekte_search_core::model::{LogicalType, Number, Value};
use objekte_search_core::query::{Query, QueryOptions, parse_filter};
use objekte_search_core::reference::{FieldDefinition, Schema};
use objekte_search_core::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::workload::ReturnMode;

fn fixture() -> (PathBuf, Repository) {
    let root = std::env::temp_dir().join(format!(
        "objekte-parameters-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (id, title, year, published, issued, created, epoch, optional) in [
        (
            "a",
            "a && $other 'quoted'",
            2020,
            true,
            "2026-11-06",
            "2026-11-06T14:30:00Z",
            -1,
            Some("x"),
        ),
        (
            "b",
            "plain",
            2021,
            false,
            "2026-11-07",
            "2026-11-07T14:30:00Z",
            1,
            None,
        ),
    ] {
        let mut record = serde_json::json!({
            "id": id,
            "title": title,
            "year": year,
            "published": published,
            "issued": issued,
            "created": created,
            "epoch": epoch,
            "tags": ["x", "x"],
        });
        if let Some(value) = optional {
            record["optional"] = serde_json::Value::String(value.into());
        } else {
            record["optional"] = serde_json::Value::Null;
        }
        std::fs::write(root.join(format!("data/{id}.json")), record.to_string()).unwrap();
    }
    let mut schema = Schema::default();
    for (name, kind, nullable, multi) in [
        ("id", LogicalType::String, false, false),
        ("title", LogicalType::String, false, false),
        ("year", LogicalType::Number, false, false),
        ("published", LogicalType::Bool, false, false),
        ("issued", LogicalType::Date, false, false),
        ("created", LogicalType::Datetime, false, false),
        ("epoch", LogicalType::Unixtime, false, false),
        ("optional", LogicalType::String, true, false),
        (
            "tags",
            LogicalType::Array(Box::new(LogicalType::String)),
            false,
            true,
        ),
    ] {
        schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required: true,
                nullable,
                indexed: true,
                multi,
                element_nullable: vec![],
            })
            .unwrap();
    }
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::Json,
            identity: IdentityPolicy::Intrinsic(vec!["id".into()]),
            discovery: Default::default(),
            schema,
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: HashMap::new(),
        }],
        RepositoryOptions {
            verified_cache_bytes: 1024 * 1024,
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    (root, repo)
}

fn query(filter: &str) -> Query {
    Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(parse_filter(filter).unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Logical,
            evidence: true,
        },
    }
}

fn ids(repo: &Repository, template: &Query, name: &str, value: Value) -> Vec<String> {
    let params = HashMap::from([(name.into(), value)]);
    let result = repo.search_with_parameters(template, &params).unwrap();
    let reference = repo
        .search_reference_with_parameters(template, &params)
        .unwrap();
    assert_eq!(result.count, reference.count);
    assert_eq!(result.hits, reference.hits);
    result.hits.into_iter().map(|hit| hit.record_id).collect()
}

#[test]
fn typed_parameters_bind_without_interpolation_and_keep_cache_identity() {
    let (root, repo) = fixture();
    let title = query("title = $value");
    assert_eq!(
        ids(
            &repo,
            &title,
            "value",
            Value::String("a && $other 'quoted'".into())
        ),
        ["a"]
    );
    assert_eq!(
        ids(&repo, &title, "value", Value::String("plain".into())),
        ["b"]
    );
    assert_eq!(
        ids(
            &repo,
            &query("title:lower = $value"),
            "value",
            Value::String("PLAIN".into())
        ),
        ["b"]
    );

    let year = query("year >= $minimum");
    assert_eq!(
        ids(
            &repo,
            &year,
            "minimum",
            Value::Number(Number::integer(2021))
        ),
        ["b"]
    );
    assert_eq!(
        ids(
            &repo,
            &year,
            "minimum",
            Value::Number(Number::integer(2020))
        ),
        ["a", "b"]
    );
    let params = HashMap::from([("minimum".into(), Value::Number(Number::integer(2021)))]);
    let cached = repo.search_with_parameters(&year, &params).unwrap();
    assert!(cached.counters.verified_result_hit);
    assert_eq!(cached.hits[0].record_id, "b");
    assert_eq!(cached.count, Some(1));

    assert_eq!(
        ids(
            &repo,
            &query("published = $flag"),
            "flag",
            Value::Bool(false)
        ),
        ["b"]
    );
    assert_eq!(
        ids(&repo, &query("optional = $value"), "value", Value::Null),
        ["b"]
    );
    let date = objekte_search_core::parameters::parse_parameter_json(
        r#"{"type":"date","value":"2026-11-06"}"#,
    )
    .unwrap();
    assert_eq!(ids(&repo, &query("issued = $start"), "start", date), ["a"]);
    let instant = objekte_search_core::parameters::parse_parameter_json(
        r#"{"type":"datetime","value":"2026-11-07T14:30:00Z"}"#,
    )
    .unwrap();
    assert_eq!(
        ids(&repo, &query("created >= $start"), "start", instant),
        ["b"]
    );
    assert_eq!(
        ids(
            &repo,
            &query("epoch >= $start"),
            "start",
            Value::Unixtime(0)
        ),
        ["b"]
    );
    drop(repo);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn missing_conflicting_and_array_parameters_fail_during_preparation() {
    let (root, repo) = fixture();
    let missing = query("year = $minimum");
    assert!(
        repo.search(&missing)
            .unwrap_err()
            .contains("requires execution bindings")
    );
    assert!(
        repo.search_with_parameters(&missing, &HashMap::new())
            .unwrap_err()
            .contains("missing parameter")
    );
    let conflict = query("year = $shared && title = $shared");
    let params = HashMap::from([("shared".into(), Value::Number(Number::integer(2020)))]);
    assert!(
        repo.search_with_parameters(&conflict, &params)
            .unwrap_err()
            .contains("incompatible uses")
    );
    let array = objekte_search_core::parameters::parse_parameter_json(r#"["x","x"]"#).unwrap();
    let params = HashMap::from([("array".into(), array)]);
    assert!(
        repo.search_with_parameters(&query("tags = $array"), &params)
            .unwrap_err()
            .contains("incompatible")
    );
    let extras = HashMap::from([
        ("minimum".into(), Value::Number(Number::integer(2020))),
        ("unused".into(), Value::String("has no effect".into())),
    ]);
    assert_eq!(
        repo.search_with_parameters(&missing, &extras)
            .unwrap()
            .count,
        Some(1)
    );
    let mut without_extra = extras.clone();
    without_extra.remove("unused");
    assert_eq!(
        missing.with_parameters(&extras).unwrap().filter,
        missing.with_parameters(&without_extra).unwrap().filter
    );
    assert!(
        repo.search_with_parameters(&missing, &without_extra)
            .unwrap()
            .counters
            .verified_result_hit
    );
    assert!(parse_filter("$field = 1").is_err());
    assert!(parse_filter("year:$binding = 1").is_err());
    drop(repo);
    std::fs::remove_dir_all(root).unwrap();
}
