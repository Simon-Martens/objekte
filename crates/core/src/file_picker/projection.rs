//! Build index text and scalar keys from one Sonic tree without copying the full JSON tree.

use super::{
    TypedKey, ViewIndex,
    text::{TextContent, project_text},
};
use crate::{
    Error, Result,
    error::io,
    query::Scalar,
    source::{Source, Stamp},
    text::TextRules,
};
use serde_json::{Map, Number, Value};
use sonic_rs::{JsonValueTrait, Value as SonicValue, ValueRef};
use std::{collections::BTreeMap, io::Read};
use unicode_casefold::UnicodeCaseFold;


/// Decoded text and top-level keys extracted from one source document.
pub(super) struct Projection {
    pub(super) bytes: Vec<u8>,
    pub(super) text: Option<TextContent>,
    pub(super) array_fields: Vec<String>,
    pub(super) present_fields: Vec<usize>,
    pub(super) typed_values: Vec<(usize, TypedKey)>,
    pub(super) view_keys: Vec<Scalar>,
}


/// Optional projections produced during a source-file index scan.
pub(super) struct ProjectionOptions<'a> {
    pub(super) folded_index: bool,
    pub(super) text_rules: Option<&'a TextRules>,
}


pub(super) fn projected_values(
    source: &Source,
    id: &str,
    stamp: &Stamp,
    indexed_fields: &[String],
    presence_fields: &[String],
    view: &ViewIndex,
    folded_index: bool,
) -> Result<Option<Projection>> {
    projected_values_with_text(
        source,
        id,
        stamp,
        indexed_fields,
        presence_fields,
        view,
        ProjectionOptions {
            folded_index,
            text_rules: None,
        },
    )
}

/// Reuse the same source read and Sonic tree when full-text postings are active.
pub(super) fn projected_values_with_text(
    source: &Source,
    id: &str,
    stamp: &Stamp,
    indexed_fields: &[String],
    presence_fields: &[String],
    view: &ViewIndex,
    options: ProjectionOptions<'_>,
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
    if after != before || raw.len() as u64 != after.size {
        return Err(Error::Unstable(path));
    }

    let Ok(parsed) = sonic_rs::from_slice::<SonicValue>(&raw) else {
        return Ok(None);
    };
    let mut values = String::new();
    if append_values(&parsed, &mut values, 0).is_err() {
        return Ok(None);
    }
    if options.folded_index && !values.is_ascii() {
        let folded = values.case_fold().collect::<String>();
        if folded != values.to_ascii_lowercase() {
            values.push('\u{1f}');
            values.push_str(&folded);
        }
    }

    let fields = top_level_fields(&parsed);
    let array_fields = fields
        .iter()
        .filter(|(_, value)| value.is_array())
        .map(|(name, _)| (*name).to_owned())
        .collect();
    let present_fields = fields
        .keys()
        .filter_map(|name| {
            presence_fields
                .binary_search_by(|field| field.as_str().cmp(name))
                .ok()
        })
        .collect();

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
            // Absence is a distinct posting from an explicit JSON null.
            // Containers remain unposted because they are not scalar values.
            match selected.get(name) {
                Some(value) => TypedKey::from_value(value),
                None => Some(TypedKey::Missing),
            }
            .map(|key| (index, key))
        })
        .collect();
    let view_keys = view.project(id, &Value::Object(selected));
    Ok(Some(Projection {
        bytes: values.into_bytes(),
        text: options.text_rules.map(|rules| project_text(&parsed, rules)),
        array_fields,
        present_fields,
        typed_values,
        view_keys,
    }))
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
