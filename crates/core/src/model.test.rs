use super::*;
use std::collections::HashSet;
use std::hash::DefaultHasher;

fn number_hash(number: &Number) -> u64 {
    let mut hasher = DefaultHasher::new();
    number.hash(&mut hasher);
    hasher.finish()
}

#[test]
fn direct_float_variants_keep_total_numeric_laws() {
    let negative_nan = Number::Float(f64::from_bits(0xfff8_0000_0000_0001));
    let positive_nan = Number::Float(f64::from_bits(0x7ff8_0000_0000_0001));
    let ordered = [
        negative_nan,
        Number::Float(f64::NEG_INFINITY),
        Number::ExactDecimal(ExactDecimal::parse("-1e1000").unwrap()),
        Number::integer(i128::MIN),
        Number::Float(-0.0),
        Number::integer(0),
        Number::Float(0.0),
        Number::integer(i128::MAX),
        Number::ExactDecimal(ExactDecimal::parse("1e1000").unwrap()),
        Number::Float(f64::INFINITY),
        positive_nan,
    ];
    for (left_index, left) in ordered.iter().enumerate() {
        assert_eq!(left, left);
        for (right_index, right) in ordered.iter().enumerate() {
            assert_eq!(left.cmp(right), right.cmp(left).reverse());
            if left_index < right_index {
                assert!(left <= right, "{left:?} should not exceed {right:?}");
            }
            if left == right {
                assert_eq!(number_hash(left), number_hash(right));
            }
        }
    }
    for left in &ordered {
        for middle in &ordered {
            for right in &ordered {
                if left <= middle && middle <= right {
                    assert!(left <= right);
                }
            }
        }
    }
    assert_ne!(Number::Float(f64::NAN), Number::integer(0));
    assert_eq!(
        Number::ExactDecimal(ExactDecimal::parse("0.5").unwrap()),
        Number::Float(0.5)
    );
    assert_eq!(
        number_hash(&Number::ExactDecimal(ExactDecimal::parse("0.5").unwrap())),
        number_hash(&Number::Float(0.5))
    );
}

#[test]
fn number_deserialization_rejects_non_finite_floats() {
    for float in [f64::NAN, f64::NEG_INFINITY, f64::INFINITY] {
        let bytes = bincode::serialize(&Number::Float(float)).unwrap();
        assert!(bincode::deserialize::<Number>(&bytes).is_err());
    }
    let bytes = bincode::serialize(&Number::Float(-0.0)).unwrap();
    assert_eq!(
        bincode::deserialize::<Number>(&bytes).unwrap(),
        Number::integer(0)
    );
}

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
fn exceptional_json_numbers_remain_numeric_and_ordered() {
    let huge = Number::parse_json_literal("1e1000000").unwrap();
    let tiny = Number::parse_json_literal("1e-1000000").unwrap();
    let boundary = Number::parse_json_literal("170141183460469231731687303715884105728").unwrap();
    assert!(matches!(huge, Number::ExactDecimal(_)));
    assert!(matches!(tiny, Number::ExactDecimal(_)));
    assert!(matches!(boundary, Number::ExactDecimal(_)));
    assert!(huge > boundary);
    assert!(boundary > Number::integer(i128::MAX));
    assert!(tiny > Number::integer(0));
    assert!(tiny < Number::float(0.1).unwrap());
    assert_eq!(
        Number::parse_json_literal("-0").unwrap(),
        Number::integer(0)
    );
    assert_eq!(Number::Float(-0.0), Number::Float(0.0));
    assert_eq!(Number::Float(-0.0), Number::integer(0));
    assert_eq!(
        ExactDecimal::parse(&i128::MIN.to_string())
            .unwrap()
            .as_i128(),
        Some(i128::MIN)
    );
    assert_eq!(
        Number::ExactDecimal(ExactDecimal::parse(&i128::MIN.to_string()).unwrap()),
        Number::integer(i128::MIN)
    );
    let mut equivalent = HashSet::new();
    equivalent.insert(Number::integer(i128::MIN));
    assert!(equivalent.contains(&Number::ExactDecimal(
        ExactDecimal::parse(&i128::MIN.to_string()).unwrap()
    )));
    equivalent.insert(Number::Float(-0.0));
    assert!(equivalent.contains(&Number::Float(0.0)));
    let mut keys = HashSet::new();
    keys.insert(boundary.clone());
    assert!(keys.contains(&Number::float(2f64.powi(127)).unwrap()));
    assert_eq!(
        Number::parse_json_literal("1.0").unwrap(),
        Number::integer(1)
    );
    assert!(
        Number::parse_json_literal("1e99999999999999999999")
            .unwrap_err()
            .contains("resource limit")
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
    assert!(FieldPath::parse("@root[0]").unwrap().segments.is_empty());
    assert_eq!(
        FieldPath::parse("@root[*].title").unwrap().segments,
        ["title"]
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
fn record_ids_keep_textual_identity_and_serialize_as_strings() {
    let padded = RecordId::intrinsic("001").unwrap();
    let plain = RecordId::intrinsic("1").unwrap();
    assert_ne!(padded, plain);
    assert!(padded < plain);
    assert!(std::sync::Arc::ptr_eq(&padded.0, &padded.clone().0));
    assert_eq!(serde_json::to_string(&padded).unwrap(), "\"001\"");
    assert_eq!(serde_json::from_str::<RecordId>("\"001\"").unwrap(), padded);
}

#[test]
fn schema_field_paths_reject_trailing_separators_and_render_query_syntax_safely() {
    for input in [
        "title.",
        "@root.title.",
        "@backlinks.mentions.",
        "items[*].",
    ] {
        assert!(FieldPath::parse(input).is_err(), "{input}");
    }
    for (input, canonical) in [
        ("['a:b']", "[\"a:b\"]"),
        ("['a b']", "[\"a b\"]"),
        ("['a>=b']", "[\"a>=b\"]"),
        ("['a,b']", "[\"a,b\"]"),
        ("['a\\'b']", "[\"a'b\"]"),
        ("[\"a]b\"]", "[\"a]b\"]"),
        ("['@file'].size", "[\"@file\"].size"),
        ("['']", "[\"\"]"),
    ] {
        let parsed = FieldPath::parse(input).unwrap();
        assert_eq!(parsed.display(), canonical, "{input}");
        assert_eq!(FieldPath::parse(canonical).unwrap(), parsed);
        assert_eq!(
            crate::query_path::QueryPath::parse(canonical)
                .unwrap()
                .field,
            parsed
        );
    }
    assert_eq!(FieldPath::parse("@root").unwrap().display(), "@root");
    assert_eq!(FieldPath::parse("['0']").unwrap().segments, ["0"]);
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
