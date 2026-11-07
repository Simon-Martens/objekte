use super::*;

#[test]
fn precise_numbers_and_temporal_distinctions() {
    let a = Number::integer(9_007_199_254_740_993);
    let b = Number::integer(9_007_199_254_740_992);
    assert!(a > b);
    assert!(a > Number::float(9_007_199_254_740_992.0).unwrap());
    assert_eq!(Number::float(1.0).unwrap(), Number::integer(1));
    assert!(Number::float(f64::NAN).is_err());
    let large: serde_json::Value = serde_json::from_str("1208925819614629174706177").unwrap();
    assert_eq!(
        Value::from_json(large).unwrap(),
        Value::Number(Number::integer(1208925819614629174706177))
    );
    assert!(matches!(
        Value::parse_typed_string("2026-01-01", &LogicalType::Date).unwrap(),
        Value::Date(_)
    ));
    assert!(matches!(
        Value::parse_typed_string("2026-01-01T00:00:00Z", &LogicalType::Datetime).unwrap(),
        Value::Datetime(_)
    ));
    assert!(Value::parse_typed_string("2026-01-01T00:00:00", &LogicalType::Datetime).is_err());
    assert_ne!(
        Value::Unixtime(0).logical_type(),
        Value::Number(Number::integer(0)).logical_type()
    );
}

#[test]
fn arrays_paths_and_id_namespace() {
    assert_eq!(FieldPath::parse("@root").unwrap().display(), "@root");
    assert_eq!(
        FieldPath::parse("@root.title").unwrap(),
        FieldPath::parse("title").unwrap()
    );
    assert_eq!(
        FieldPath::parse("@root[\"a.b\"]").unwrap().segments,
        ["a.b"]
    );
    assert!(
        ArrayValue::new(
            vec![Value::String("x".into()), Value::Bool(true)],
            None,
            false
        )
        .is_err()
    );
    assert!(ArrayValue::new(vec![Value::Null], Some(LogicalType::String), false).is_err());
    assert_eq!(
        ArrayValue::new(vec![], None, false).unwrap().element_type,
        None
    );
    assert_eq!(
        FieldPath::parse("data.gnd[\"@context\"]").unwrap().segments,
        ["data", "gnd", "@context"]
    );
    assert_eq!(
        FieldPath::parse("[\"@file\"].size").unwrap().root,
        PathRoot::Data
    );
    for path in [
        "[\"@file\"].size",
        "data.gnd[\"@context\"]",
        "[\"a.b\"]",
        "[\"q\\\"uote\"]",
    ] {
        let parsed = FieldPath::parse(path).unwrap();
        assert_eq!(FieldPath::parse(&parsed.display()).unwrap(), parsed);
    }
    assert_eq!(
        FieldPath::parse("@file.size").unwrap().root,
        PathRoot::Metadata(MetadataField::FileSize)
    );
    assert!(FieldPath::parse("@file.unknown").is_err());
    let backlink = FieldPath::parse("@backlinks.mentions.title").unwrap();
    assert_eq!(backlink.root, PathRoot::Backlinks);
    assert_eq!(backlink.display(), "@backlinks.mentions.title");
    assert_eq!(
        FieldPath::parse("['@backlinks'].mentions").unwrap().root,
        PathRoot::Data
    );
    let mut ids = IdentitySet::default();
    ids.insert(RecordId::intrinsic("a").unwrap()).unwrap();
    assert!(RecordId::intrinsic("~fallback:forged").is_err());
    assert!(ids.insert(RecordId::intrinsic("a").unwrap()).is_err());
    assert_ne!(
        RecordId::generated(FileId(1), 2, 3),
        RecordId::intrinsic("a").unwrap()
    );
}

#[test]
fn unhinted_arrays_keep_actual_or_unresolved_element_types() {
    let array = Value::from_json(serde_json::json!([null, 1, 2])).unwrap();
    assert_eq!(
        array.logical_type(),
        LogicalType::Array(Box::new(LogicalType::Number))
    );
    let empty = Value::from_json(serde_json::json!([])).unwrap();
    assert_eq!(
        empty.logical_type(),
        LogicalType::Array(Box::new(LogicalType::Unknown))
    );
    let nested = Value::from_json(serde_json::json!([[], [1, 2]])).unwrap();
    assert_eq!(
        nested.logical_type(),
        LogicalType::Array(Box::new(LogicalType::Array(Box::new(LogicalType::Number))))
    );
    let objects = Value::from_json(serde_json::json!([{"a": 1}, {"b": "x"}])).unwrap();
    assert_eq!(
        objects.logical_type(),
        LogicalType::Array(Box::new(LogicalType::Object))
    );
}

#[test]
fn parsed_json_arrays_preserve_mixed_members_and_sticky_conflicts() {
    let parsed = Value::from_json(serde_json::json!(["a", 1, null, {}, [], true])).unwrap();
    let Value::Array(array) = parsed else {
        panic!("array root")
    };
    assert_eq!(array.values.len(), 6);
    assert!(array.mixed_elements);
    assert_eq!(array.element_type, None);
    assert!(array.nullable_elements);

    let nested = Value::from_json(serde_json::json!([["a", 1], [2, 3]])).unwrap();
    let Value::Array(array) = nested else {
        panic!("array root")
    };
    assert!(array.mixed_elements);
    assert_eq!(array.element_type, None);

    let all_null = Value::from_json(serde_json::json!([null, null])).unwrap();
    let Value::Array(array) = all_null else {
        panic!("array root")
    };
    assert!(!array.mixed_elements);
    assert_eq!(array.element_type, None);
    assert!(array.nullable_elements);
}
