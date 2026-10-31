//! Build index text and scalar keys from one Sonic tree without copying the full JSON tree.

use super::{
    TypedKey, ViewIndex,
    observed::{FieldObservation, ObservedKind},
};
use crate::{
    Error, Result,
    error::io,
    query::Scalar,
    source::{Source, Stamp},
};
use serde_json::{Map, Number, Value};
use sonic_rs::{JsonValueTrait, Value as SonicValue, ValueRef};
use std::{collections::BTreeMap, io::Read};
use unicode_casefold::UnicodeCaseFold;


/// Decoded text, observed paths, and selected keys from one source document.
pub(super) struct Projection {
    pub(super) bytes: Vec<u8>,
    pub(super) typed_values: Vec<(usize, TypedKey)>,
    pub(super) observed: Vec<FieldObservation>,
    pub(super) view_keys: Vec<Scalar>,
}


pub(super) fn projected_values(
    source: &Source,
    id: &str,
    stamp: &Stamp,
    indexed_fields: &[String],
    view: &ViewIndex,
    folded_index: bool,
) -> Result<Option<Projection>> {
    let path = source.path.join(format!("{id}.json"));
    let mut file = source.file(id)?;
    let before = Stamp::from_metadata(&file.metadata().map_err(|e| io(&path, e))?, &path)?;
    if &before != stamp {
        return Err(Error::Unstable(path));
    }
    let mut raw = Vec::new();
    file.read_to_end(&mut raw).map_err(|e| io(&path, e))?;
    let after = Stamp::from_metadata(&file.metadata().map_err(|e| io(&path, e))?, &path)?;
    if after != before {
        return Err(Error::Unstable(path));
    }

    let Ok(parsed) = sonic_rs::from_slice::<SonicValue>(&raw) else {
        return Ok(None);
    };
    let mut values = String::new();
    if append_values(&parsed, &mut values, 0).is_err() {
        return Ok(None);
    }
    if folded_index && !values.is_ascii() {
        let folded = values.case_fold().collect::<String>();
        if folded != values.to_ascii_lowercase() {
            values.push('\u{1f}');
            values.push_str(&folded);
        }
    }

    let observed = observed_fields(&parsed);

    let fields = top_level_fields(&parsed);

    // View and typed indexes need only these top-level scalars. Containers
    // receive an invalid-type placeholder, so nested trees are never copied.
    let mut selected = Map::new();
    for name in view.fields().iter().chain(indexed_fields) {
        if selected.contains_key(name) {
            continue;
        }
        if let Some(value) = fields.get(name.as_str()) {
            let Ok(value) = scalar_projection(value) else {
                return Ok(None);
            };
            selected.insert(name.clone(), value);
        }
    }
    let typed_values = indexed_fields
        .iter()
        .enumerate()
        .filter_map(|(index, name)| {
            selected
                .get(name)
                .and_then(TypedKey::from_value)
                .map(|key| (index, key))
        })
        .collect();
    let view_keys = view.project(id, &Value::Object(selected));
    Ok(Some(Projection {
        bytes: values.into_bytes(),
        typed_values,
        observed,
        view_keys,
    }))
}

fn observed_fields(value: &SonicValue) -> Vec<FieldObservation> {
    fn visit(
        value: &SonicValue,
        path: &mut Vec<String>,
        observations: &mut BTreeMap<Vec<String>, FieldObservation>,
        depth: usize,
    ) {
        if depth >= 128 {
            return;
        }
        match value.as_ref() {
            ValueRef::Object(_) => {
                // Match full JSON parsing when an object repeats a key: the
                // last value at that key is the one a query can reach.
                for (name, child) in top_level_fields(value) {
                    path.push(name.to_owned());
                    let (kind, scalar) = observed_value(child);
                    if let Some(previous) = observations.get_mut(path) {
                        previous.kinds |= kind.bit();
                        previous.occurrences += 1;
                        previous.scalar = None;
                    } else {
                        observations.insert(
                            path.clone(),
                            FieldObservation {
                                path: path.clone(),
                                kinds: kind.bit(),
                                occurrences: 1,
                                scalar,
                            },
                        );
                    }
                    visit(child, path, observations, depth + 1);
                    path.pop();
                }
            }
            ValueRef::Array(items) => {
                // Key paths descend through every array item. Explicit array
                // positions remain source-verified by the query executor.
                for item in items {
                    visit(item, path, observations, depth + 1);
                }
            }
            _ => {}
        }
    }

    let mut observations = BTreeMap::new();
    visit(value, &mut Vec::new(), &mut observations, 0);
    observations.into_values().collect()
}

fn observed_value(value: &SonicValue) -> (ObservedKind, Option<TypedKey>) {
    if let Some(number) = value.as_raw_number() {
        return (
            ObservedKind::Number,
            Some(TypedKey::Number(number.as_str().to_owned())),
        );
    }
    match value.as_ref() {
        ValueRef::Null => (ObservedKind::Null, Some(TypedKey::Null)),
        ValueRef::Bool(value) => (ObservedKind::Boolean, Some(TypedKey::Bool(value))),
        ValueRef::String(value) => (
            ObservedKind::String,
            Some(TypedKey::String(value.to_owned())),
        ),
        ValueRef::Array(_) => (ObservedKind::Array, None),
        ValueRef::Object(_) => (ObservedKind::Object, None),
        ValueRef::Number(_) => unreachable!("raw numbers enabled"),
    }
}

fn top_level_fields(value: &SonicValue) -> BTreeMap<&str, &SonicValue> {
    let mut result = BTreeMap::new();
    if let ValueRef::Object(fields) = value.as_ref() {
        for (name, value) in fields {
            result.insert(name, value);
        }
    }
    result
}

fn exact_number(value: &SonicValue) -> std::result::Result<Number, ()> {
    value
        .as_raw_number()
        .ok_or(())?
        .as_str()
        .parse()
        .map_err(|_| ())
}

fn scalar_projection(value: &SonicValue) -> std::result::Result<Value, ()> {
    if value.as_raw_number().is_some() {
        return exact_number(value).map(Value::Number);
    }
    Ok(match value.as_ref() {
        ValueRef::Null => Value::Null,
        ValueRef::Bool(value) => Value::Bool(value),
        ValueRef::String(value) => Value::String(value.to_owned()),
        ValueRef::Number(_) => unreachable!("raw numbers enabled"),
        ValueRef::Array(_) => Value::Array(Vec::new()),
        ValueRef::Object(_) => Value::Object(Map::new()),
    })
}

fn validate_value(value: &SonicValue, depth: usize) -> std::result::Result<(), ()> {
    if value.as_raw_number().is_some() {
        exact_number(value)?;
        return Ok(());
    }
    match value.as_ref() {
        ValueRef::Array(values) => {
            if depth >= 127 {
                return Err(());
            }
            for value in values {
                validate_value(value, depth + 1)?;
            }
        }
        ValueRef::Object(values) => {
            if depth >= 127 {
                return Err(());
            }
            for (_, value) in values {
                validate_value(value, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn append_values(
    value: &SonicValue,
    out: &mut String,
    depth: usize,
) -> std::result::Result<(), ()> {
    if value.as_raw_number().is_some() {
        out.push_str(&exact_number(value)?.to_string());
        out.push('\u{1f}');
        return Ok(());
    }
    match value.as_ref() {
        ValueRef::Null => out.push_str("null\u{1f}"),
        ValueRef::Bool(value) => out.push_str(if value { "true\u{1f}" } else { "false\u{1f}" }),
        ValueRef::String(value) => {
            for ch in value.chars() {
                out.push(if ch == '\n' || ch == '\r' {
                    '\u{1f}'
                } else {
                    ch
                });
            }
            out.push('\u{1f}');
        }
        ValueRef::Number(_) => unreachable!("raw numbers enabled"),
        ValueRef::Array(values) => {
            if depth >= 127 {
                return Err(());
            }
            for value in values {
                append_values(value, out, depth + 1)?;
            }
        }
        ValueRef::Object(values) => {
            if depth >= 127 {
                return Err(());
            }
            let mut last = BTreeMap::new();
            for (name, value) in values {
                // The public JSON representation keeps the last duplicate.
                // Validate discarded subtrees too, even though their text is absent.
                if let Some(previous) = last.insert(name, value) {
                    validate_value(previous, depth + 1)?;
                }
            }
            for value in last.values() {
                append_values(value, out, depth + 1)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "projection.test.rs"]
mod tests;
