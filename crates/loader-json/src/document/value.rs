//! Convert a parsed JSON value into the core's typed record model.

use objekte_search_core::model::{ArrayValue, Number, Value};

pub fn number_from_json(number: &serde_json::Number) -> Result<Number, String> {
    if let Some(integer) = number.as_i64() {
        return Ok(Number::integer(i128::from(integer)));
    }
    if let Some(integer) = number.as_u64() {
        return Ok(Number::integer(i128::from(integer)));
    }
    Number::parse_decimal_literal(number.as_str())
}

pub fn value_from_json(value: serde_json::Value) -> Result<Value, String> {
    Ok(match value {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::String(s) => Value::String(s),
        serde_json::Value::Number(number) => Value::Number(number_from_json(&number)?),
        serde_json::Value::Bool(b) => Value::Bool(b),
        serde_json::Value::Array(items) => Value::Array(ArrayValue::from_unrestricted(
            items
                .into_iter()
                .map(value_from_json)
                .collect::<Result<_, _>>()?,
        )),
        serde_json::Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .map(|(name, value)| {
                    let value =
                        value_from_json(value).map_err(|error| format!("{name}: {error}"))?;
                    Ok((name, value))
                })
                .collect::<Result<_, String>>()?,
        ),
    })
}
