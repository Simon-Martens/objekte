use super::super::*;
use crate::query::parse_filter;
use crate::schema::CustomBinding;
#[test]
fn optional_ancestor_of_required_temporal_child_stays_optional_during_query_conversion() {
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["details".into()],
            kind: LogicalType::Object,
            required: false,
            nullable: true,
            indexed: false,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    schema
        .register_field(FieldDefinition {
            path: vec!["details".into(), "published".into()],
            kind: LogicalType::Date,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    schema
        .register_field(FieldDefinition {
            path: vec!["details".into(), "history".into(), "at".into()],
            kind: LogicalType::Datetime,
            required: true,
            nullable: false,
            indexed: false,
            multi: true,
            element_nullable: vec![],
        })
        .unwrap();
    schema
        .register_field(FieldDefinition {
            path: vec!["details".into(), "history".into()],
            kind: LogicalType::Array(Box::new(LogicalType::Object)),
            required: false,
            nullable: false,
            indexed: false,
            multi: true,
            element_nullable: vec![false],
        })
        .unwrap();
    for json in [
        serde_json::json!({}),
        serde_json::json!({"details":null}),
        serde_json::json!({"details":{"published":"2026-01-02", "history":[{"at":"2026-01-02T03:04:05Z"}]}}),
    ] {
        let mut built = Value::from_json(json.clone()).unwrap();
        schema.coerce_record(&mut built).unwrap();
        let mut scanned = Value::from_json(json).unwrap();
        schema.convert_indexed_record(&mut scanned, None).unwrap();
        assert_eq!(scanned, built);
    }
    let mut projected = Value::from_json(serde_json::json!({"note":"keep"})).unwrap();
    schema
        .convert_indexed_record(&mut projected, Some(&HashSet::from(["note".into()])))
        .unwrap();
    let mut invalid = Value::from_json(serde_json::json!({"details":{}})).unwrap();
    assert!(
        schema
            .coerce_record(&mut invalid)
            .unwrap_err()
            .contains("details.published")
    );
}

#[test]
fn boolean_state_binding_rejects_invalid_forms_and_preserves_named_conversion() {
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["x".into()],
            kind: LogicalType::String,
            required: false,
            nullable: true,
            indexed: false,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    schema
        .register_custom(
            vec!["x".into()],
            CustomBinding {
                name: "string".into(),
                version: 1,
                transform: Transform::CaseFold,
            },
        )
        .unwrap();
    let converted = bind_filter(&parse_filter("x:string = 'HELLO'").unwrap(), &schema).unwrap();
    let BoundExpr::Term(converted) = converted else {
        panic!("term")
    };
    assert_eq!(converted.derived, Derived::None);
    assert_eq!(converted.binding_name, "string");
    let predicate = bind_filter(&parse_filter("x:string = true").unwrap(), &schema).unwrap();
    let BoundExpr::Term(predicate) = predicate else {
        panic!("term")
    };
    assert_eq!(predicate.derived, Derived::IsString);
    for source in [
        "x:? > true",
        "x:string ~ true",
        "x:number = 1",
        "x:array = null",
        "x:? = 'true'",
        "x:object:lower = true",
        "x:null:object = true",
        "x:exists:array = true",
        "x:array ?= true",
        "x:each:string = true",
        "x:string = $missing",
    ] {
        let result =
            parse_filter(source).and_then(|parsed| bind_filter(&parsed, &schema).map(|_| ()));
        assert!(result.is_err(), "{source}");
    }
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
    let mut query_record = Value::from_json(serde_json::json!({
        "days": ["2026-01-01", null], "tags": ["a"]
    }))
    .unwrap();
    schema
        .convert_indexed_record(&mut query_record, Some(&HashSet::from(["days".into()])))
        .unwrap();
    let Value::Object(query_fields) = query_record else {
        panic!("object");
    };
    assert!(
        matches!(query_fields["days"], Value::Array(ref array) if matches!(array.values[0], Value::Date(_)))
    );
}
