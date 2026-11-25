//! Compatibility conversion used by the existing typed-record test fixtures.

use super::{ArrayValue, Number, Value};

impl Value {
    pub fn from_json(value: serde_json::Value) -> Result<Self, String> {
        Ok(match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::String(s) => Self::String(s),
            serde_json::Value::Number(n) => {
                Self::Number(Number::parse_decimal_literal(n.as_str())?)
            }
            serde_json::Value::Bool(b) => Self::Bool(b),
            serde_json::Value::Array(a) => Self::Array(ArrayValue::from_unrestricted(
                a.into_iter()
                    .map(Self::from_json)
                    .collect::<Result<_, _>>()?,
            )),
            serde_json::Value::Object(o) => Self::Object(
                o.into_iter()
                    .map(|(k, v)| {
                        let value = Self::from_json(v).map_err(|error| format!("{k}: {error}"))?;
                        Ok((k, value))
                    })
                    .collect::<Result<_, String>>()?,
            ),
        })
    }
}
