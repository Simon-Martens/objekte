//! Sonic JSON parsing with the existing public/schema value representation.
//! Keep fingerprint serialization separate: its exact bytes are a cache contract.
#[cfg(test)]
use serde::de::{DeserializeSeed, MapAccess, Visitor};
use serde::{
    Deserialize, Deserializer,
    de::{self},
};
use serde_json::{Map, Value};
#[cfg(test)]
use sonic_rs::JsonContainerTrait;
use sonic_rs::{JsonValueTrait, ValueRef};
#[cfg(test)]
use std::{collections::BTreeSet, fmt};
use unicode_casefold::UnicodeCaseFold;


// INFO: Serde-compatible JSON value converted from Sonic's parser.
pub(crate) struct ParsedValue(pub Value);


impl<'de> Deserialize<'de> for ParsedValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = sonic_rs::Value::deserialize(deserializer)?;
        compatible(&value, 0).map(Self).map_err(de::Error::custom)
    }
}


// Sonic's DOM preserves duplicate object members. Insert in source order to retain
// the library's last-member-wins contract. Convert raw numbers without an f64 hop.
fn compatible(value: &sonic_rs::Value, depth: usize) -> Result<Value, String> {
    // as_ref() attempts a finite numeric conversion; inspect raw numbers first
    // so opaque large integers and exponents never become rounded values or null.
    if let Some(number) = value.as_raw_number() {
        return number
            .as_str()
            .parse()
            .map(Value::Number)
            .map_err(|e: serde_json::Error| e.to_string());
    }
    match value.as_ref() {
        ValueRef::Null => Ok(Value::Null),
        ValueRef::Bool(v) => Ok(Value::Bool(v)),
        ValueRef::String(v) => Ok(Value::String(v.to_owned())),
        ValueRef::Number(_) => unreachable!("numbers handled without a floating-point conversion"),
        ValueRef::Array(values) => {
            if depth >= 127 {
                return Err("recursion limit exceeded".into());
            }
            values
                .iter()
                .map(|v| compatible(v, depth + 1))
                .collect::<Result<_, _>>()
                .map(Value::Array)
        }
        ValueRef::Object(values) => {
            if depth >= 127 {
                return Err("recursion limit exceeded".into());
            }
            let mut object = Map::new();
            for (key, value) in values {
                object.insert(key.to_owned(), compatible(value, depth + 1)?);
            }
            Ok(Value::Object(object))
        }
    }
}

pub(crate) fn parse(json: &str) -> Result<Value, sonic_rs::Error> {
    sonic_rs::from_str::<ParsedValue>(json).map(|v| v.0)
}

/// Plain decoded scalar values for conservative text candidate selection.
/// The non-printable separator prevents bigrams from crossing value boundaries.
pub(crate) fn value_text(value: &Value, out: &mut String) {
    match value {
        Value::Array(items) => items.iter().for_each(|item| value_text(item, out)),
        Value::Object(items) => items.values().for_each(|item| value_text(item, out)),
        Value::String(s) => {
            for ch in s.chars() {
                out.push(if ch == '\n' || ch == '\r' {
                    '\u{1f}'
                } else {
                    ch
                });
            }
            out.push('\u{1f}');
        }
        Value::Number(n) => {
            out.push_str(&n.to_string());
            out.push('\u{1f}');
        }
        Value::Bool(v) => {
            out.push_str(if *v { "true" } else { "false" });
            out.push('\u{1f}');
        }
        Value::Null => out.push_str("null\u{1f}"),
    }
}

/// Append original values and any distinct Unicode-folded projection. Keeping
/// both lets byte-oriented indexes narrow sensitive and caseless queries.
pub(crate) fn indexed_value_text(value: &Value, out: &mut String) {
    let start = out.len();
    value_text(value, out);
    let original = &out[start..];
    if original.is_ascii() {
        return;
    }
    let folded = original.case_fold().collect::<String>();
    if folded != original.to_ascii_lowercase() {
        out.push('\u{1f}');
        out.push_str(&folded);
    }
}

/// Validate the entire Sonic DOM with the same depth rules as `parse`, but only
/// convert the field needed by an expression index to the public Value tree.
/// Unlike skip-based projection this also preserves errors in unselected fields.
#[cfg(test)]
pub(crate) fn index_field(json: &str, field: &str) -> Result<Value, String> {
    index_fields(json, |name| name == field)
}

/// Validate every value, but materialize only the requested top-level keys.
#[cfg(test)]
pub(crate) fn index_fields(json: &str, selected: impl Fn(&str) -> bool) -> Result<Value, String> {
    fn depth_check(v: &sonic_rs::Value, depth: usize) -> Result<(), String> {
        if v.is_number() {
            return Ok(());
        }
        match v.as_ref() {
            ValueRef::Array(items) => {
                if depth >= 127 {
                    return Err("recursion limit exceeded".into());
                }
                for v in items {
                    depth_check(v, depth + 1)?;
                }
            }
            ValueRef::Object(items) => {
                if depth >= 127 {
                    return Err("recursion limit exceeded".into());
                }
                for (_, v) in items {
                    depth_check(v, depth + 1)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    let raw: sonic_rs::Value = sonic_rs::from_str(json).map_err(|e| e.to_string())?;
    depth_check(&raw, 0)?;
    if let Some(items) = raw.as_object() {
        let mut out = Map::new();
        for (name, value) in items {
            if selected(name) {
                out.insert(name.to_owned(), compatible(value, 1)?);
            }
        }
        Ok(Value::Object(out))
    } else {
        compatible(&raw, 0)
    }
}

/// Materialize requested top-level fields only. Scan all members, including
/// duplicates, using Sonic's native skip behavior for unselected values.
#[cfg(test)]
pub(crate) fn project(json: &str, fields: &BTreeSet<&str>) -> Result<Value, sonic_rs::Error> {
    // INFO: Deserialization seed selecting only requested fields from a record.
    struct Projection<'a>(&'a BTreeSet<&'a str>);


    impl<'de> DeserializeSeed<'de> for Projection<'_> {
        type Value = Value;
        fn deserialize<D: Deserializer<'de>>(self, de: D) -> Result<Value, D::Error> {
            de.deserialize_map(self)
        }
    }


    impl<'de> Visitor<'de> for Projection<'_> {
        type Value = Value;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a record object")
        }

        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Value, M::Error> {
            let mut object = Map::new();
            while let Some(key) = map.next_key::<String>()? {
                if self.0.contains(key.as_str()) {
                    let ParsedValue(value) = map.next_value()?;
                    object.insert(key, value);
                } else {
                    let _: sonic_rs::LazyValue<'de> = map.next_value()?;
                }
            }
            Ok(Value::Object(object))
        }
    }


    let mut de = sonic_rs::Deserializer::from_str(json).use_rawnumber();
    let value = Projection(fields).deserialize(&mut de)?;
    de.end()?;
    Ok(value)
}

#[cfg(test)]
#[path = "json.test.rs"]
mod tests;
