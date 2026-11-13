use super::*;
use crate::model::Number;

#[test]
fn truth_table_covers_logical_values_and_empty_selection() {
    let cases = [
        (Value::Null, [true, true, false, false, false, false, false]),
        (
            Value::String(String::new()),
            [true, false, false, true, false, false, false],
        ),
        (
            Value::String(" ".into()),
            [true, false, true, true, false, false, false],
        ),
        (
            Value::Number(Number::integer(0)),
            [true, false, true, false, true, false, false],
        ),
        (
            Value::Bool(false),
            [true, false, true, false, false, false, false],
        ),
        (
            Value::from_json(serde_json::json!([])).unwrap(),
            [true, false, false, false, false, true, false],
        ),
        (
            Value::from_json(serde_json::json!([null])).unwrap(),
            [true, false, true, false, false, true, false],
        ),
        (
            Value::from_json(serde_json::json!({})).unwrap(),
            [true, false, false, false, false, false, true],
        ),
        (
            Value::from_json(serde_json::json!({"a":null})).unwrap(),
            [true, false, true, false, false, false, true],
        ),
    ];
    let predicates = [
        StatePredicate::Exists,
        StatePredicate::Null,
        StatePredicate::NonEmpty,
        StatePredicate::String,
        StatePredicate::Number,
        StatePredicate::Array,
        StatePredicate::Object,
    ];
    for (value, expected) in cases {
        for (predicate, want) in predicates.into_iter().zip(expected) {
            assert_eq!(
                value_matches(&value, predicate),
                want,
                "{value:?} {predicate:?}"
            );
        }
    }
    for predicate in predicates {
        assert!(!selected_matches(None, predicate));
        assert!(!selected_matches(Some(&[]), predicate));
    }
    let mixed = [Value::Null, Value::String("hello".into())];
    let selected: Vec<&Value> = mixed.iter().collect();
    for predicate in [
        StatePredicate::Exists,
        StatePredicate::Null,
        StatePredicate::NonEmpty,
        StatePredicate::String,
    ] {
        assert!(selected_matches(Some(&selected), predicate));
    }
}
