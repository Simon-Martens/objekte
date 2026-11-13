//! Shared source semantics for Boolean predicates over selected logical values.
//!
//! A path with multiple values is aggregated existentially before its Boolean
//! comparison is applied. This module does not retain any index payload.

use crate::model::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StatePredicate {
    NonEmpty,
    String,
    Number,
    Array,
    Null,
    Object,
    Exists,
}

pub(crate) fn selected_matches(selected: Option<&[&Value]>, predicate: StatePredicate) -> bool {
    selected
        .into_iter()
        .flatten()
        .any(|value| value_matches(value, predicate))
}

pub(crate) fn value_matches(value: &Value, predicate: StatePredicate) -> bool {
    match predicate {
        StatePredicate::Exists => true,
        StatePredicate::NonEmpty => match value {
            Value::Null => false,
            Value::String(text) | Value::Id(text) => !text.is_empty(),
            Value::Array(array) => !array.values.is_empty(),
            Value::Object(map) => !map.is_empty(),
            _ => true,
        },
        StatePredicate::String => matches!(value, Value::String(_) | Value::Id(_)),
        StatePredicate::Number => matches!(value, Value::Number(_)),
        StatePredicate::Array => matches!(value, Value::Array(_)),
        StatePredicate::Null => matches!(value, Value::Null),
        StatePredicate::Object => matches!(value, Value::Object(_)),
    }
}

#[cfg(test)]
#[path = "field_state.test.rs"]
mod tests;
