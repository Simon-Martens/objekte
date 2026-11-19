use super::contains_empty_array;
use crate::model::{ArrayValue, Value};

#[test]
fn empty_arrays_are_detected_at_any_depth_without_conflating_null_or_missing() {
    let empty = Value::Array(ArrayValue::new(vec![], None, false).unwrap());
    let nested = Value::Array(ArrayValue::new(vec![empty], None, false).unwrap());
    assert!(contains_empty_array(&nested));
    assert!(!contains_empty_array(&Value::Null));
    assert!(!contains_empty_array(&Value::String(String::new())));
}
