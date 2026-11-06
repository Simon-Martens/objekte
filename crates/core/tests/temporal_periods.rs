use std::collections::HashMap;

use objekte_search_core::model::LogicalType;
use objekte_search_core::query::{Query, QueryOptions, parse_filter};
use objekte_search_core::reference::{FieldDefinition, Schema};
use objekte_search_core::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::workload::ReturnMode;

fn query(filter: &str) -> Query {
    Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(parse_filter(filter).unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 20,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    }
}

#[test]
fn typed_periods_preserve_half_open_scalar_and_array_answers() {
    let root = std::env::temp_dir().join(format!(
        "objekte-temporal-periods-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, body) in [
        (
            "A",
            r#"{"id":"A","issued":"2024-02-29","created":"2026-11-06T13:30:59Z","epoch":-1,"dates":["2024-03-01","2024-01-01"]}"#,
        ),
        (
            "B",
            r#"{"id":"B","issued":"2024-03-01","created":"2026-11-06T13:31:00Z","epoch":0,"dates":["2024-02-29","2024-03-01"]}"#,
        ),
        (
            "C",
            r#"{"id":"C","issued":"2024-02-01","created":"2026-11-06T13:30:00Z","epoch":1,"dates":[]}"#,
        ),
    ] {
        std::fs::write(root.join(format!("data/{name}.json")), body).unwrap();
    }
    let mut schema = Schema::default();
    for (name, kind, multi) in [
        ("id", LogicalType::String, false),
        ("issued", LogicalType::Date, false),
        ("created", LogicalType::Datetime, false),
        ("epoch", LogicalType::Unixtime, false),
        (
            "dates",
            LogicalType::Array(Box::new(LogicalType::Date)),
            true,
        ),
    ] {
        schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required: true,
                nullable: false,
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
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    for (filter, expected) in [
        ("issued = 2024-02", vec!["A", "C"]),
        ("issued != 2024-02", vec!["B"]),
        ("issued < 2024-02", vec![]),
        ("issued <= 2024-02", vec!["A", "C"]),
        ("issued > 2024-02", vec!["B"]),
        ("issued >= 2024-02", vec!["A", "B", "C"]),
        ("created = 2026-11-06T14:30+01:00", vec!["A", "C"]),
        ("created >= 2026-11-06T14:30:15.250+01:00", vec!["A", "B"]),
        ("epoch = 1969-12-31T23:59:59Z", vec!["A"]),
        ("epoch = 1969-12-31T23:59:59.5Z", vec![]),
        ("dates ?= 2024-02", vec!["B"]),
        ("dates = 2024-02", vec!["C"]),
        ("dates ?!= 2024-02", vec!["A", "B"]),
    ] {
        let request = query(filter);
        let actual = repo.search(&request).unwrap();
        let reference = repo.search_reference(&request).unwrap();
        let ids = actual
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, expected, "{filter}");
        assert_eq!(actual.count, reference.count, "{filter}");
        assert_eq!(actual.hits, reference.hits, "{filter}");
        if filter.starts_with("issued") && filter != "issued < 2024-02" {
            assert_eq!(actual.counters.files_opened, 0, "{filter}");
        }
    }
    assert!(repo.search(&query("issued = 2026-11-06T14:30Z")).is_err());
    assert!(repo.search(&query("epoch = 2024-02")).is_ok());
    assert!(repo.search(&query("issued ~ 2024-02")).is_err());
    assert!(parse_filter("issued = 2024-02-30").is_err());
    assert!(matches!(
        parse_filter("issued = 2024").unwrap(),
        objekte_search_core::query::FilterExpr::Term(_)
    ));
    drop(repo);
    std::fs::remove_dir_all(root).unwrap();
}
