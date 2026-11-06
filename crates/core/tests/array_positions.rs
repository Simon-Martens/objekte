use std::collections::HashMap;

use objekte_search_core::model::{LogicalType, Value};
use objekte_search_core::query::{Query, QueryOptions, parse_filter, parse_structured_filter};
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
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Logical,
            evidence: true,
        },
    }
}

#[test]
fn positional_paths_select_only_the_requested_array_occurrence() {
    let root = std::env::temp_dir().join(format!(
        "objekte-array-positions-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (id, body) in [
        (
            "a",
            r#"{"items":[{"price":5,"maximum_price":3},{"price":1,"maximum_price":10}],"matrix":[[1,2],[3,4]],"literal.key":["value","other"],"0":"digit-key","scalar":"x"}"#,
        ),
        (
            "b",
            r#"{"items":[{"price":2,"maximum_price":3}],"matrix":[[5,6]],"literal.key":["other"],"scalar":"x"}"#,
        ),
        (
            "c",
            r#"{"items":[],"matrix":[],"literal.key":[],"scalar":"x"}"#,
        ),
    ] {
        let mut record: serde_json::Value = serde_json::from_str(body).unwrap();
        record["id"] = id.into();
        std::fs::write(root.join(format!("data/{id}.json")), record.to_string()).unwrap();
    }
    let mut schema = Schema::default();
    for (path, kind, multi) in [
        (vec!["id"], LogicalType::String, false),
        (vec!["scalar"], LogicalType::String, false),
        (
            vec!["items"],
            LogicalType::Array(Box::new(LogicalType::Object)),
            true,
        ),
        (vec!["items", "price"], LogicalType::Number, true),
        (vec!["items", "maximum_price"], LogicalType::Number, true),
        (
            vec!["matrix"],
            LogicalType::Array(Box::new(LogicalType::Array(Box::new(LogicalType::Number)))),
            true,
        ),
        (
            vec!["literal.key"],
            LogicalType::Array(Box::new(LogicalType::String)),
            true,
        ),
    ] {
        schema
            .register_field(FieldDefinition {
                path: path.into_iter().map(str::to_owned).collect(),
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
        ("items[0].price = 5", vec!["a"]),
        ("items[1].price = 1", vec!["a"]),
        ("items[2].price = 1", vec![]),
        ("items[2].price:exists = false", vec!["a", "b", "c"]),
        ("items[2].price:null = true", vec![]),
        ("matrix[1][1] = 4", vec!["a"]),
        ("matrix[0][1] = 6", vec!["b"]),
        ("['literal.key'][0] = 'value'", vec!["a"]),
        ("['0'] = 'digit-key'", vec!["a"]),
        ("items[*].price ?= 1", vec!["a"]),
        ("items[*].price = 1", vec!["c"]),
    ] {
        let query = query(filter);
        let result = repo.search(&query).unwrap();
        let reference = repo.search_reference(&query).unwrap();
        let ids: Vec<_> = result
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect();
        assert_eq!(ids, expected, "{filter}");
        assert_eq!(result.count, reference.count, "{filter}");
        assert_eq!(result.hits, reference.hits, "{filter}");
        if filter.contains("[0]") || filter.contains("[1]") || filter.contains("[2]") {
            assert_eq!(
                repo.explain(&query).unwrap()[0].terms[0].selected.kind,
                objekte_search_core::executor::AccessPathKind::Scanner,
                "{filter}"
            );
        }
    }
    assert!(
        repo.search(&query("scalar[0] = 'x'"))
            .unwrap_err()
            .contains("array selector on scalar")
    );
    assert!(parse_structured_filter("SORT BY items[0].price").is_err());
    let _ = Value::Null;
    drop(repo);
    std::fs::remove_dir_all(root).unwrap();
}
