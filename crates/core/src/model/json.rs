//! Conversion from parsed JSON values and numeric literals into logical values.

use super::decimal::ExactDecimal;
use super::{ArrayValue, Number, Value};


impl Number {
    pub fn from_json(value: &serde_json::Number) -> Result<Self, String> {
        if let Some(n) = value.as_i64() {
            return Ok(Self::Integer(n as i128));
        }
        if let Some(n) = value.as_u64() {
            return Ok(Self::Integer(n as i128));
        }
        let text = value.to_string();
        if !text.contains(['.', 'e', 'E']) {
            return Ok(match text.parse::<i128>() {
                Ok(integer) => Self::Integer(integer),
                Err(_) => Self::ExactDecimal(ExactDecimal::parse(&text)?),
            });
        }
        let exact = ExactDecimal::parse(&text)?;
        if let Some(float) = value
            .as_f64()
            .filter(|float| float.is_finite() && *float != 0.0)
        {
            Self::float(float)
        } else if exact.digits == "0" {
            Ok(Self::Integer(0))
        } else {
            Ok(Self::ExactDecimal(exact))
        }
    }

    pub fn parse_json_literal(text: &str) -> Result<Self, String> {
        let parsed = serde_json::from_str::<serde_json::Number>(text)
            .map_err(|error| format!("invalid numeric literal: {error}"))?;
        Self::from_json(&parsed)
    }
}


impl Value {
    pub fn from_json(value: serde_json::Value) -> Result<Self, String> {
        Ok(match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::String(s) => Self::String(s),
            serde_json::Value::Number(n) => Self::Number(Number::from_json(&n)?),
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
