use super::*;
use crate::inventory::{ByteRange, SourceRevision};
use crate::model::{FileId, RecordId, RecordSlot};
use crate::query::parse_filter;

fn schema() -> Schema {
    let mut schema = Schema::default();
    for (path, kind, multi) in [
        ("name", LogicalType::String, false),
        (
            "values",
            LogicalType::Array(Box::new(LogicalType::String)),
            true,
        ),
        ("people.name", LogicalType::String, true),
        (
            "people.aliases",
            LogicalType::Array(Box::new(LogicalType::String)),
            true,
        ),
    ] {
        schema
            .register_field(FieldDefinition {
                path: path.split('.').map(str::to_owned).collect(),
                kind,
                required: false,
                nullable: true,
                indexed: true,
                multi,
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    schema
        .register_custom(
            vec!["name".into()],
            CustomBinding {
                name: "folded".into(),
                version: 1,
                transform: Transform::CaseFold,
            },
        )
        .unwrap();
    schema
}

fn with_record<F: FnOnce(RecordContext<'_>)>(value: Value, f: F) {
    let record = RecordEntry {
        slot: RecordSlot(0),
        id: RecordId("x".into()),
        file: FileId(0),
        range: ByteRange { start: 0, end: 4 },
        revision: SourceRevision {
            modified_ns: 0,
            size: 4,
        },
    };
    let file = FileEntry {
        id: FileId(0),
        relative_path: "x.json".into(),
        revision: record.revision,
        created_ns: None,
        collections: vec![],
    };
    f(RecordContext {
        value: &value,
        record: &record,
        file: &file,
        collection_name: "test",
        repository_root: Path::new("/tmp"),
    });
}

#[test]
fn truth_table_arrays_null_and_case_folding() {
    let value = Value::from_json(serde_json::json!({"name":"Straße", "values":["a","b"], "people":[{"name":"Ada","aliases":["A","Ada"]},{"name":"Bob","aliases":["B"]}]})).unwrap();
    with_record(value, |ctx| {
        for (input, expected) in [
            ("values ?!= 'a'", true),
            ("!(values ?= 'a')", false),
            ("values:each = 'a'", false),
            ("values:length = 2", true),
            ("name:lower ~ 'STRASSE'", true),
            ("name:folded ~ 'strasse'", true),
            ("people.name ?= 'Ada'", true),
            ("people.aliases ?= 'Ada'", true),
            ("people.aliases ?= 'missing'", false),
            ("name:null = false", true),
            ("@file.created:null = true", true),
        ] {
            let bound = bind_filter(&parse_filter(input).unwrap(), &schema()).unwrap();
            assert_eq!(
                eval_filter(&bound, &ctx, &mut HashSet::new()).unwrap(),
                expected,
                "{input}"
            );
        }
    });
}

#[test]
fn invalid_branches_rejected_before_evaluation() {
    let schema = schema();
    assert!(bind_filter(&parse_filter("name = 'x' || absent = 1").unwrap(), &schema).is_err());
    assert!(bind_filter(&parse_filter("name:missing = 'x'").unwrap(), &schema).is_err());
    assert!(bind_filter(&parse_filter("name ?= 'x'").unwrap(), &schema).is_err());
}

#[test]
fn evidence_excludes_false_boolean_branches() {
    let value = Value::from_json(
        serde_json::json!({"name":"Ada","values":["yes"],"people":[{"name":"Bob"}]}),
    )
    .unwrap();
    with_record(value, |ctx| {
        let bound = bind_filter(
            &parse_filter("(name = 'Ada' && values ?= 'no') || people.name ?= 'Bob'").unwrap(),
            &schema(),
        )
        .unwrap();
        let mut evidence = HashSet::new();
        assert!(eval_filter(&bound, &ctx, &mut evidence).unwrap());
        assert_eq!(evidence, HashSet::from(["people.name".to_string()]));
    });
}

#[test]
fn explicit_temporal_hints_coerce_values_and_reject_bad_inputs() {
    let mut schema = Schema::default();
    for (name, kind) in [
        ("day", LogicalType::Date),
        ("time", LogicalType::Datetime),
        ("seconds", LogicalType::Unixtime),
        ("days", LogicalType::Array(Box::new(LogicalType::Date))),
    ] {
        schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required: true,
                nullable: false,
                indexed: true,
                multi: name == "days",
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    let mut value = Value::from_json(serde_json::json!({
        "day":"2026-01-02","time":"2026-01-02T03:04:05+02:00",
        "seconds":1767319445,"days":["2026-01-01","2026-01-02"]
    }))
    .unwrap();
    schema.coerce_record(&mut value).unwrap();
    let Value::Object(fields) = value else {
        panic!("object");
    };
    assert!(matches!(fields["day"], Value::Date(_)));
    assert!(matches!(fields["time"], Value::Datetime(_)));
    assert!(matches!(fields["seconds"], Value::Unixtime(_)));
    assert!(
        matches!(fields["days"],Value::Array(ref array) if array.element_type == Some(LogicalType::Date))
    );
    let mut invalid = Value::from_json(serde_json::json!({"day":"not a date"})).unwrap();
    assert!(schema.coerce_record(&mut invalid).is_err());
}

#[test]
fn missing_null_empty_truth_table() {
    for (input, expected) in [
        ("values:exists = true", true),
        ("values:null = false", true),
        ("values:each = 'x'", true),
        ("values ?= 'x'", false),
        ("values:length = 0", true),
        ("name:exists = false", true),
        ("name:null = false", true),
        ("name != 'x'", false),
    ] {
        let value = Value::from_json(serde_json::json!({"values": []})).unwrap();
        with_record(value, |ctx| {
            let bound = bind_filter(&parse_filter(input).unwrap(), &schema()).unwrap();
            assert_eq!(
                eval_filter(&bound, &ctx, &mut HashSet::new()).unwrap(),
                expected,
                "{input}"
            );
        });
    }
    let value = Value::from_json(serde_json::json!({"name":null,"values":[null,"x"]})).unwrap();
    with_record(value, |ctx| {
        for (input, expected) in [
            ("name:exists = true", true),
            ("name:null = true", true),
            ("name = null", true),
            ("name != 'x'", false),
            ("values ?!= 'x'", false),
            ("values ?= 'x'", true),
        ] {
            let bound = bind_filter(&parse_filter(input).unwrap(), &schema()).unwrap();
            assert_eq!(
                eval_filter(&bound, &ctx, &mut HashSet::new()).unwrap(),
                expected,
                "{input}"
            );
        }
    });
}

#[test]
fn declared_types_required_fields_and_array_nullability_are_enforced() {
    let mut schema = Schema::default();
    for (name, kind, required, nullable, element_nullable) in [
        ("count", LogicalType::Number, true, false, vec![]),
        ("optional", LogicalType::String, false, true, vec![]),
        (
            "tags",
            LogicalType::Array(Box::new(LogicalType::String)),
            true,
            false,
            vec![false],
        ),
        (
            "days",
            LogicalType::Array(Box::new(LogicalType::Date)),
            true,
            false,
            vec![true],
        ),
        (
            "times",
            LogicalType::Array(Box::new(LogicalType::Datetime)),
            true,
            false,
            vec![false],
        ),
        (
            "seconds",
            LogicalType::Array(Box::new(LogicalType::Unixtime)),
            true,
            false,
            vec![false],
        ),
        (
            "nested",
            LogicalType::Array(Box::new(LogicalType::Array(Box::new(LogicalType::Number)))),
            true,
            false,
            vec![false, true],
        ),
    ] {
        schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required,
                nullable,
                indexed: name == "count",
                multi: name != "count" && name != "optional",
                element_nullable,
            })
            .unwrap();
    }
    let valid = serde_json::json!({
        "count": 7, "optional": null, "tags": ["a"],
        "days": ["2026-01-01", null],
        "times": ["2026-01-01T12:30:00+02:00"],
        "seconds": [0, 1767319445],
        "nested": [[], [1, null]],
    });
    let mut value = Value::from_json(valid.clone()).unwrap();
    schema.coerce_record(&mut value).unwrap();
    let Value::Object(fields) = value else {
        panic!("object");
    };
    assert!(
        matches!(fields["days"], Value::Array(ref array) if array.element_type == Some(LogicalType::Date) && array.nullable_elements)
    );
    assert!(
        matches!(fields["times"], Value::Array(ref array) if matches!(array.values[0], Value::Datetime(_)))
    );
    assert!(
        matches!(fields["seconds"], Value::Array(ref array) if matches!(array.values[0], Value::Unixtime(0)))
    );
    assert!(
        matches!(fields["nested"], Value::Array(ref array) if array.element_type == Some(LogicalType::Array(Box::new(LogicalType::Number))))
    );
    for (change, expected) in [
        (serde_json::json!({"count": "7"}), "count: expected Number"),
        (
            serde_json::json!({"count": null}),
            "count: null is not allowed",
        ),
        (
            serde_json::json!({"tags": [null]}),
            "tags: null is not allowed",
        ),
        (serde_json::json!({"days": ["not-a-date"]}), "days:"),
        (
            serde_json::json!({"nested": [null]}),
            "nested: null is not allowed",
        ),
    ] {
        let mut json = valid.clone();
        for (key, value) in change.as_object().unwrap() {
            json[key] = value.clone();
        }
        let mut logical = Value::from_json(json).unwrap();
        assert!(
            schema
                .coerce_record(&mut logical)
                .unwrap_err()
                .contains(expected)
        );
    }
    let mut missing = valid.as_object().unwrap().clone();
    missing.remove("count");
    let mut logical = Value::from_json(serde_json::Value::Object(missing)).unwrap();
    assert!(
        schema
            .coerce_record(&mut logical)
            .unwrap_err()
            .contains("count: required field is missing")
    );
    let mut projected = Value::from_json(serde_json::json!({"count": 7})).unwrap();
    schema
        .coerce_projected_record(&mut projected, Some(&HashSet::from(["count".into()])))
        .unwrap();
    assert!(schema.coerce_record(&mut projected).is_err());
}

#[test]
fn numeric_string_is_rejected_before_indexed_or_scanned_inequality() {
    use crate::query::{Query, QueryOptions};
    use crate::repository::{
        CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
    };
    use crate::source::SourceFormat;
    use crate::workload::ReturnMode;

    let root = std::env::temp_dir().join(format!("objekte-type-index-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let definition = |indexed| {
        let mut schema = Schema::default();
        for (name, kind, indexed) in [
            ("count", LogicalType::Number, indexed),
            ("other", LogicalType::String, false),
        ] {
            schema
                .register_field(FieldDefinition {
                    path: vec![name.into()],
                    kind,
                    required: true,
                    nullable: false,
                    indexed,
                    multi: false,
                    element_nullable: vec![],
                })
                .unwrap();
        }
        CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::Json,
            identity: IdentityPolicy::Filename,
            schema,
            sort_fields: vec![],
            sort_locale: "de".into(),
        }
    };
    let options = RepositoryOptions {
        snapshot_mode: SnapshotMode::Disabled,
        ..Default::default()
    };
    std::fs::write(root.join("data/bad.json"), r#"{"count":"7","other":"ok"}"#).unwrap();
    for indexed in [false, true] {
        let error = Repository::open_with_options(&root, vec![definition(indexed)], options)
            .err()
            .unwrap();
        assert!(
            error.contains("bad.json:0..") && error.contains("count: expected Number"),
            "{error}"
        );
    }
    std::fs::remove_file(root.join("data/bad.json")).unwrap();
    std::fs::write(root.join("data/seven.json"), r#"{"count":7,"other":"ok"}"#).unwrap();
    std::fs::write(root.join("data/eight.json"), r#"{"count":8,"other":"ok"}"#).unwrap();
    let query = Query {
        collections: vec![],
        fts: None,
        filter: Some(parse_filter("count != 7").unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    for indexed in [false, true] {
        let repo =
            Repository::open_with_options(&root, vec![definition(indexed)], options).unwrap();
        let optimized = repo.search(&query).unwrap();
        let reference = repo.search_reference(&query).unwrap();
        assert_eq!(optimized.count, 1);
        assert_eq!(optimized.hits[0].record_id, "eight");
        assert_eq!(optimized.hits, reference.hits);
        let mut logical_query = query.clone();
        logical_query.options.return_mode = ReturnMode::Logical;
        let logical = repo.search(&logical_query).unwrap();
        assert!(
            matches!(&logical.hits[0].payload, Some(crate::repository::Payload::Logical(Value::Object(fields))) if fields.get("count") == Some(&Value::Number(Number::integer(8))))
        );
    }
    let persisted = RepositoryOptions {
        snapshot_mode: SnapshotMode::ReadWrite,
        ..Default::default()
    };
    let built = Repository::open_with_options(&root, vec![definition(true)], persisted).unwrap();
    assert!(!built.startup().restored);
    drop(built);
    let restored = Repository::open_with_options(&root, vec![definition(true)], persisted).unwrap();
    assert!(restored.startup().restored);
    assert_eq!(restored.search(&query).unwrap().hits[0].record_id, "eight");
    std::fs::remove_dir_all(root).unwrap();
}
