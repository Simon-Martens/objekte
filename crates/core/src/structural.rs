//! Collection-wide conservative facts about logical record structure.

use crate::model::Value;

/// Whether a logical value contains any empty array, including an unrelated
/// path. Such records may satisfy a universal comparison without a text witness.
pub(crate) fn contains_empty_array(value: &Value) -> bool {
    match value {
        Value::Array(array) => {
            array.values.is_empty() || array.values.iter().any(contains_empty_array)
        }
        Value::Object(fields) => fields.values().any(contains_empty_array),
        _ => false,
    }
}

#[cfg(test)]
#[path = "structural.test.rs"]
mod tests;
