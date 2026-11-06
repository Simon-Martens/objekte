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
