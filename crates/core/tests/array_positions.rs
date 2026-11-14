use std::collections::HashMap;

use objekte_search_core::model::{LogicalType, Value};
use objekte_search_core::query::{
    Literal, Operator, Query, QueryOptions, parse_filter, parse_structured_filter,
};
use objekte_search_core::reference::{
    BindingOperation, FieldDefinition, RegisteredBinding, Schema,
};
use objekte_search_core::repository::{
    CollectionDefinition, IdentityPolicy, Payload, Repository, RepositoryOptions,
};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::workload::ReturnMode;


#[derive(Debug)]
struct LiteralOnlyOperation;


impl BindingOperation for LiteralOnlyOperation {
    fn accepts(&self, _: &LogicalType, _: Operator, _: bool) -> bool {
        true
    }

    fn prepare_literal(&self, _: &Literal, _: &LogicalType) -> Result<Value, String> {
        Ok(Value::Null)
    }

    fn map_value(&self, value: &Value) -> Option<Value> {
        Some(value.clone())
    }

    fn map_text(&self, text: &str) -> String {
        text.into()
    }
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
            r#"{"items":[{"price":5,"maximum_price":3},{"price":1,"maximum_price":10}],"matrix":[[1,2],[3,4]],"literal.key":["value","other"],"0":"digit-key","scalar":"x","limit":3,"minimum_price":2,"declared_size":0,"title":"Goethe","alternate_title":"goethe","pattern":"%ethe"}"#,
        ),
        (
            "b",
            r#"{"items":[{"price":2,"maximum_price":3}],"matrix":[[5,6]],"literal.key":["other"],"scalar":"x","limit":3,"minimum_price":4,"declared_size":0,"title":"Müller","alternate_title":"MÜLLER","pattern":"Mül%"}"#,
        ),
        (
            "c",
            r#"{"items":[],"matrix":[],"literal.key":[],"scalar":"x","limit":3,"minimum_price":3,"declared_size":0,"title":"X","alternate_title":"Y","pattern":"Y"}"#,
        ),
    ] {
        let mut record: serde_json::Value = serde_json::from_str(body).unwrap();
        if let Some(items) = record["items"].as_array_mut() {
            for item in items {
                item["tags"] = serde_json::json!([1, 2]);
                item["other_tags"] = serde_json::json!([1, 2]);
            }
        }
        record["id"] = id.into();
        std::fs::write(root.join(format!("data/{id}.json")), record.to_string()).unwrap();
    }
    let mut schema = Schema::default();
    for (path, kind, multi) in [
        (vec!["id"], LogicalType::String, false),
        (vec!["scalar"], LogicalType::String, false),
        (vec!["limit"], LogicalType::Number, false),
        (vec!["minimum_price"], LogicalType::Number, false),
        (vec!["declared_size"], LogicalType::Number, false),
        (vec!["title"], LogicalType::String, false),
        (vec!["alternate_title"], LogicalType::String, false),
        (vec!["pattern"], LogicalType::String, false),
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
        ("items[0].price > items[0].maximum_price", vec!["a"]),
        ("items[*].price <= items[*].maximum_price", vec!["b", "c"]),
        ("items[*].price ?<= items[*].maximum_price", vec!["a", "b"]),
        (
            "items[*].tags[0] = items[*].other_tags[0]",
            vec!["a", "b", "c"],
        ),
        ("items[*].price <= limit", vec!["b", "c"]),
        ("limit >= items[*].price", vec!["b", "c"]),
        ("limit > minimum_price", vec!["a"]),
        ("matrix[0]:length <= limit", vec!["a", "b"]),
        ("title:lower = alternate_title:lower", vec!["a", "b"]),
        ("title = alternate_title", vec![]),
        ("title ~ pattern", vec!["a", "b"]),
        ("@file.size > declared_size", vec!["a", "b", "c"]),
        ("@file.size >= @record.byte_length", vec!["a", "b", "c"]),
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
                if filter == "['literal.key'][0] = 'value'" {
                    objekte_search_core::executor::AccessPathKind::RawBigram
                } else {
                    objekte_search_core::executor::AccessPathKind::Scanner
                },
                "{filter}"
            );
        }
    }
    assert!(
        repo.search(&query("scalar[0] = 'x'"))
            .unwrap_err()
            .contains("array selector on scalar")
    );
    assert!(parse_structured_filter("SORT BY items[0].price").is_ok());
    let mut positioned_sort = query("items[0].price:exists = true");
    positioned_sort.filter = None;
    positioned_sort.options.sort = parse_structured_filter("SORT BY items[0].price")
        .unwrap()
        .sort
        .unwrap();
    let optimized = repo.search(&positioned_sort).unwrap();
    let reference = repo.search_reference(&positioned_sort).unwrap();
    assert_eq!(optimized.hits, reference.hits);
    assert!(
        repo.search(&query("items[*].price <= matrix[*][0]"))
            .unwrap_err()
            .contains("independent multivalued")
    );
    assert!(
        repo.search(&query("items[*].tags = items[*].other_tags"))
            .unwrap_err()
            .contains("ambiguous nested multivalued")
    );
    assert!(
        repo.search(&query("id > limit"))
            .unwrap_err()
            .contains("incompatible field operand types")
    );
    assert!(
        repo.search(&query("items[*].price:null = items[*].maximum_price:null"))
            .unwrap_err()
            .contains("exists/null modifiers cannot compare")
    );
    let field_query = query("items[*].price ?<= items[*].maximum_price");
    assert_eq!(
        repo.explain(&field_query).unwrap()[0].terms[0]
            .selected
            .kind,
        objekte_search_core::executor::AccessPathKind::Scanner
    );
    assert_eq!(
        repo.search(&field_query).unwrap().hits[0].matched_fields,
        ["items[*].maximum_price", "items[*].price"]
    );
    let mut metadata_query = query("@file.size >= @record.byte_length");
    metadata_query.options.return_mode = ReturnMode::Locator;
    assert_eq!(
        repo.search(&metadata_query).unwrap().counters.files_opened,
        0
    );
    let _ = Value::Null;
    drop(repo);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn field_comparisons_preserve_null_and_unavailable_fallback_truth() {
    let root = std::env::temp_dir().join(format!(
        "objekte-field-comparison-fallback-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/mixed.jsonl"),
        b"{\"left\":null,\"right\":null,\"name\":null,\"alias\":null,\"shape\":1,\"other\":null}\n{\"left\":1,\"right\":null,\"name\":\"X\",\"alias\":null,\"shape\":[1,2],\"other\":2}\n{\"left\":null,\"right\":1,\"name\":null,\"alias\":\"X\",\"shape\":\"bad\",\"other\":null}\n{\"left\":\n",
    )
    .unwrap();
    let mut schema = Schema::default();
    for name in ["left", "right", "other"] {
        schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind: LogicalType::Number,
                required: false,
                nullable: true,
                indexed: true,
                multi: false,
                element_nullable: vec![],
            })
            .unwrap();
    }
    for name in ["name", "alias"] {
        schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind: LogicalType::String,
                required: false,
                nullable: true,
                indexed: true,
                multi: false,
                element_nullable: vec![],
            })
            .unwrap();
    }
    schema
        .register_operation(
            vec!["left".into()],
            RegisteredBinding {
                name: "literal_only".into(),
                version: 1,
                operation: std::sync::Arc::new(LiteralOnlyOperation),
            },
        )
        .unwrap();
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
            sort_overrides: HashMap::new(),
        }],
        RepositoryOptions {
            ..Default::default()
        },
    )
    .unwrap();
    for (filter, count) in [
        ("left = right", 1),
        ("left != right", 2),
        ("right != left", 2),
        ("left != null", 1),
        ("right != null", 1),
        ("name:lower = alias:lower", 1),
        ("name:lower != alias:lower", 2),
        ("name:lower = null", 2),
        ("shape:length = other", 1),
        ("shape:length != other", 0),
        ("shape:length = 0", 0),
        ("!(left = right)", 2),
        ("@record.class = 1 && !(left = right)", 0),
        ("@file.size >= @record.byte_length", 4),
    ] {
        let request = query(filter);
        let actual = repo.search(&request).unwrap();
        let reference = repo.search_reference(&request).unwrap();
        assert_eq!(actual.count, Some(count), "{filter}");
        assert_eq!(actual.hits, reference.hits, "{filter}");
    }
    let length_match = repo.search(&query("shape:length = other")).unwrap();
    assert!(matches!(
        length_match.hits[0].payload,
        Some(Payload::Logical(Value::Object(ref fields)))
            if matches!(fields.get("shape"), Some(Value::Array(_)))
    ));
    let unequal = repo.search(&query("left != right")).unwrap();
    assert!(unequal.hits.iter().any(|hit| matches!(
        hit.payload,
        Some(Payload::Logical(Value::Object(ref fields)))
            if matches!((fields.get("left"), fields.get("right")),
                (Some(Value::Null), Some(Value::Number(_))))
    )));
    assert!(
        repo.search(&query("left:literal_only = right"))
            .unwrap_err()
            .contains("does not support dynamic field comparison")
    );
    drop(repo);
    std::fs::remove_dir_all(root).unwrap();
}
