use super::*;
use crate::model::{ArrayValue, Number, Value};
use std::collections::BTreeMap;

#[test]
fn build_rejects_non_finite_numbers_before_indexing_nested_values() {
    for invalid in [f64::NAN, f64::NEG_INFINITY, f64::INFINITY] {
        let mut record = ExtractedRecord {
            value: Value::Object(BTreeMap::from([(
                "items".into(),
                Value::Array(ArrayValue::from_unrestricted(vec![Value::Object(
                    BTreeMap::from([("score".into(), Value::Number(Number::Float(invalid)))]),
                )])),
            )])),
            text: Vec::new(),
        };
        let error = finish_build_record(&Schema::default(), &mut record).unwrap_err();
        assert!(error.contains("items: [0]: score: non-finite number"));
    }

    let mut valid = ExtractedRecord {
        value: Value::Object(BTreeMap::from([(
            "score".into(),
            Value::Number(Number::Float(-0.0)),
        )])),
        text: Vec::new(),
    };
    finish_build_record(&Schema::default(), &mut valid).unwrap();
}
