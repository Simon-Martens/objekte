use super::super::*;
use super::{fixture_schema, with_record};
use crate::query::parse_filter;
#[test]
fn membership_filter_and_unavailable_truth_match_evidence_evaluation() {
    let value = Value::from_json(serde_json::json!({
        "name": "Ada", "values": ["x", "y"], "people": [{"name": "Bob"}]
    }))
    .unwrap();
    with_record(value, |ctx| {
        for filter in [
            "name = 'Ada'",
            "name ~ 'A%a' && values ?= 'x'",
            "name !~ 'A%a' || people.name ?= 'Bob'",
            "!(name = 'Ada' && values ?= 'no')",
            "values ?= 'missing'",
            "@file.id = 0 || name = 'no'",
            "@file.id != 0 && name = 'Ada'",
        ] {
            let bound = bind_filter(&parse_filter(filter).unwrap(), &fixture_schema()).unwrap();
            let unavailable = eval_unavailable_filter(&bound, &ctx, &mut HashSet::new()).unwrap();
            assert_eq!(
                eval_unavailable_filter_membership(&bound, &ctx).unwrap(),
                unavailable,
                "{filter}"
            );
        }
    });
}

#[test]
fn invalid_branches_rejected_before_evaluation() {
    let schema = fixture_schema();
    assert!(
        bind_filter(
            &parse_filter("name = 'x' || absent:missing = 1").unwrap(),
            &schema
        )
        .is_err()
    );
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
            &fixture_schema(),
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
fn boolean_state_predicates_match_source_selection_and_composition() {
    let schema = Schema::default();
    for (value, checks) in [
        (
            serde_json::json!({}),
            vec![
                ("x:? = false", true),
                ("x:exists = false", true),
                ("x:null = false", true),
                ("x:string != true", true),
            ],
        ),
        (
            serde_json::json!({"x":null}),
            vec![
                ("x:? = false", true),
                ("x:null = true", true),
                ("x:exists = true", true),
            ],
        ),
        (
            serde_json::json!({"x":""}),
            vec![
                ("x:? = false", true),
                ("x:string = true", true),
                ("x:null = false", true),
            ],
        ),
        (
            serde_json::json!({"x":" "}),
            vec![("x:? = true", true), ("x:string = true", true)],
        ),
        (
            serde_json::json!({"x":0}),
            vec![("x:? = true", true), ("x:number = true", true)],
        ),
        (
            serde_json::json!({"x":false}),
            vec![("x:? = true", true), ("x:number = false", true)],
        ),
        (
            serde_json::json!({"x":[]}),
            vec![
                ("x:? = false", true),
                ("x:array = true", true),
                ("x:exists = true", true),
            ],
        ),
        (
            serde_json::json!({"x":[null]}),
            vec![
                ("x:? = true", true),
                ("x:array = true", true),
                ("x:null = false", true),
            ],
        ),
        (
            serde_json::json!({"x":{}}),
            vec![("x:? = false", true), ("x:object = true", true)],
        ),
        (
            serde_json::json!({"x":{"a":null}}),
            vec![("x:? = true", true), ("x:object = true", true)],
        ),
    ] {
        with_record(Value::from_json(value).unwrap(), |ctx| {
            for (source, expected) in checks {
                let bound = bind_filter(&parse_filter(source).unwrap(), &schema).unwrap();
                assert_eq!(
                    eval_filter(&bound, &ctx, &mut HashSet::new()).unwrap(),
                    expected,
                    "{source}"
                );
            }
        });
    }
    let value = Value::from_json(serde_json::json!({"items":[null,"", "hello",0,{}]})).unwrap();
    with_record(value, |ctx| {
        for (source, expected) in [
            ("items:array = true", true),
            ("items:string = false", true),
            ("items[*]:array = false", true),
            ("items[*]:null = true", true),
            ("items[*]:string = true", true),
            ("items[*]:number = true", true),
            ("items[*]:object = true", true),
            ("items[*]:? = true", true),
            ("items[*]:? = false", false),
            ("items[*]:null = true && items[*]:string = true", true),
            ("!(items[*]:number = true) || items[*]:object = true", true),
        ] {
            let bound = bind_filter(&parse_filter(source).unwrap(), &schema).unwrap();
            assert_eq!(
                eval_filter(&bound, &ctx, &mut HashSet::new()).unwrap(),
                expected,
                "{source}"
            );
        }
    });
}
