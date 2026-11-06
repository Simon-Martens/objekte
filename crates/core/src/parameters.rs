//! Checked CLI/JSON transport for typed execution parameters.

use crate::model::{LogicalType, Number, Value};

/// Decode one parameter value. A tagged object disambiguates temporal types;
/// ordinary JSON scalars keep their JSON logical type.
pub fn parse_parameter_json(input: &str) -> Result<Value, String> {
    let json: serde_json::Value = serde_json::from_str(input).map_err(|error| error.to_string())?;
    let serde_json::Value::Object(object) = &json else {
        return Value::from_json(json);
    };
    let (Some(kind), Some(value)) = (object.get("type"), object.get("value")) else {
        return Value::from_json(json);
    };
    if object.len() != 2 {
        return Err("typed parameter accepts only type and value".into());
    }
    let kind = kind.as_str().ok_or("parameter type must be a string")?;
    Ok(match kind {
        "string" => Value::String(value.as_str().ok_or("string parameter needs text")?.into()),
        "number" => {
            let number = value
                .as_number()
                .ok_or("number parameter needs a JSON number")?;
            Value::Number(Number::from_json(number)?)
        }
        "bool" => Value::Bool(
            value
                .as_bool()
                .ok_or("bool parameter needs true or false")?,
        ),
        "null" => {
            if !value.is_null() {
                return Err("null parameter needs null".into());
            }
            Value::Null
        }
        "date" => Value::parse_typed_string(
            value.as_str().ok_or("date parameter needs text")?,
            &LogicalType::Date,
        )?,
        "datetime" => Value::parse_typed_string(
            value.as_str().ok_or("datetime parameter needs text")?,
            &LogicalType::Datetime,
        )?,
        "unixtime" => Value::Unixtime(
            value
                .as_i64()
                .ok_or("unixtime parameter needs an i64 number")?,
        ),
        _ => return Err(format!("unknown parameter type {kind}")),
    })
}

#[cfg(test)]
#[path = "parameters.test.rs"]
mod tests;
