//! Structured record rows shared by the preview and full detail view.

use std::collections::BTreeSet;

use objekte_search_core::model::{Number, Value};
use objekte_search_core::repository::{Payload, SearchHit};

const MAX_ROWS: usize = 10_000;
const MAX_DEPTH: usize = 40;


/// One visible field, metadata item, or section heading.
#[derive(Clone, Debug)]
pub(super) struct RecordRow {
    pub(super) label: String,
    pub(super) value: String,
    pub(super) depth: usize,
    pub(super) matched: bool,
    pub(super) branch: Option<(String, bool)>,
}


/// Cursor and fold state for a structured record.
#[derive(Default)]
pub(super) struct RecordState {
    pub(super) cursor: usize,
    pub(super) scroll: usize,
    pub(super) raw: bool,
    collapsed_objects: BTreeSet<String>,
    expanded_arrays: BTreeSet<String>,
}


impl RecordState {
    pub(super) fn reset(&mut self) {
        self.cursor = 0;
        self.scroll = 0;
        self.raw = false;
    }

    pub(super) fn toggle(&mut self, path: &str, array: bool) {
        let set = if array {
            &mut self.expanded_arrays
        } else {
            &mut self.collapsed_objects
        };
        if !set.insert(path.to_owned()) {
            set.remove(path);
        }
    }

    pub(super) fn is_folded(&self, path: &str, array: bool) -> bool {
        if array {
            !self.expanded_arrays.contains(path)
        } else {
            self.collapsed_objects.contains(path)
        }
    }

    pub(super) fn rows(&self, hit: &SearchHit, diagnostic: Option<&str>) -> Vec<RecordRow> {
        let mut rows = Vec::new();
        plain(&mut rows, "File", hit.file_path.display().to_string());
        plain(
            &mut rows,
            "Bytes",
            format!("{}..{}", hit.byte_start, hit.byte_end),
        );
        plain(
            &mut rows,
            "Status",
            format!("{} ({})", hit.file_status.label(), hit.file_status.code()),
        );
        plain(
            &mut rows,
            "Record class",
            hit.record_class.code().to_string(),
        );
        if let Some(diagnostic) = diagnostic {
            plain(&mut rows, "Diagnostic", diagnostic.to_owned());
        }
        if !hit.matched_fields.is_empty() {
            plain(&mut rows, "Matched", hit.matched_fields.join(", "));
        }
        plain(&mut rows, "Fields", String::new());
        if let Some(value) = json_value(hit) {
            match &value {
                serde_json::Value::Object(fields) => {
                    for (name, value) in fields {
                        walk(
                            self,
                            &mut rows,
                            name,
                            value,
                            name,
                            name,
                            0,
                            &hit.matched_fields,
                        );
                    }
                }
                _ => walk(
                    self,
                    &mut rows,
                    "$value",
                    &value,
                    "$value",
                    "$value",
                    0,
                    &hit.matched_fields,
                ),
            }
        } else {
            plain(
                &mut rows,
                "Source",
                "No structured JSON; press r for raw bytes".into(),
            );
        }
        rows
    }
}


fn plain(rows: &mut Vec<RecordRow>, label: &str, value: String) {
    rows.push(RecordRow {
        label: label.into(),
        value,
        depth: 0,
        matched: false,
        branch: None,
    });
}

fn walk(
    state: &RecordState,
    rows: &mut Vec<RecordRow>,
    label: &str,
    value: &serde_json::Value,
    pointer: &str,
    matched_path: &str,
    depth: usize,
    matched_fields: &[String],
) {
    if rows.len() >= MAX_ROWS {
        return;
    }
    let (branch, summary, array) = match value {
        serde_json::Value::Object(fields) => (true, format!("{{{} fields}}", fields.len()), false),
        serde_json::Value::Array(values) => (true, format!("[{} items]", values.len()), true),
        serde_json::Value::String(text) => (false, text.clone(), false),
        serde_json::Value::Null => (false, "null".into(), false),
        other => (false, other.to_string(), false),
    };
    rows.push(RecordRow {
        label: label.into(),
        value: summary,
        depth,
        matched: matched_fields.iter().any(|path| {
            path == matched_path
                || path.starts_with(&format!("{matched_path}."))
                || path.starts_with(&format!("{matched_path}["))
        }),
        branch: branch.then(|| (pointer.to_owned(), array)),
    });
    if !branch || state.is_folded(pointer, array) || depth >= MAX_DEPTH {
        return;
    }
    match value {
        serde_json::Value::Object(fields) => {
            for (name, child) in fields {
                let pointer = format!("{pointer}/{}", name.replace('~', "~0").replace('/', "~1"));
                let matched_path = format!("{matched_path}.{name}");
                walk(
                    state,
                    rows,
                    name,
                    child,
                    &pointer,
                    &matched_path,
                    depth + 1,
                    matched_fields,
                );
            }
        }
        serde_json::Value::Array(values) => {
            for (index, child) in values.iter().enumerate() {
                let name = format!("[{index}]");
                let pointer = format!("{pointer}/{index}");
                let matched_path = format!("{matched_path}[{index}]");
                walk(
                    state,
                    rows,
                    &name,
                    child,
                    &pointer,
                    &matched_path,
                    depth + 1,
                    matched_fields,
                );
            }
        }
        _ => {}
    }
}

/// Payload as ordinary JSON for field display; non-JSON bytes stay available in raw view.
pub(super) fn json_value(hit: &SearchHit) -> Option<serde_json::Value> {
    match &hit.payload {
        Some(Payload::Bytes(bytes)) => serde_json::from_slice(bytes).ok(),
        Some(Payload::Logical(value)) => Some(logical_json(value)),
        _ => None,
    }
}

fn logical_json(value: &Value) -> serde_json::Value {
    use serde_json::Value as Json;
    match value {
        Value::Null => Json::Null,
        Value::String(text) | Value::Id(text) => Json::String(text.clone()),
        Value::Number(Number::Integer(number)) => serde_json::from_str(&number.to_string())
            .unwrap_or_else(|_| Json::String(number.to_string())),
        Value::Number(Number::Float(number)) => {
            serde_json::from_str(&number.to_string()).unwrap_or(Json::Null)
        }
        Value::Bool(value) => Json::Bool(*value),
        Value::Date(value) => Json::String(value.to_string()),
        Value::Datetime(value) => Json::String(value.to_string()),
        Value::Unixtime(value) => Json::Number((*value).into()),
        Value::Object(fields) => Json::Object(
            fields
                .iter()
                .map(|(name, value)| (name.clone(), logical_json(value)))
                .collect(),
        ),
        Value::Array(array) => Json::Array(array.values.iter().map(logical_json).collect()),
    }
}

pub(super) fn raw_text(hit: &SearchHit) -> String {
    match &hit.payload {
        Some(Payload::Bytes(bytes)) => String::from_utf8_lossy(bytes).into_owned(),
        Some(Payload::Logical(_)) => json_value(hit)
            .and_then(|value| serde_json::to_string_pretty(&value).ok())
            .unwrap_or_default(),
        Some(Payload::LogicalUnavailable { .. }) => "Logical value unavailable".into(),
        None => "Record payload not loaded".into(),
    }
}

/// Values for a configured result column, traversing arrays without losing nested fields.
pub(super) fn column_value(hit: &SearchHit, path: &str) -> String {
    let Ok(path) = objekte_search_core::model::FieldPath::parse(path) else {
        return String::new();
    };
    let Some(value) = json_value(hit) else {
        return String::new();
    };
    fn collect(value: &serde_json::Value, path: &[String], out: &mut Vec<String>) {
        if let Some((first, rest)) = path.split_first() {
            match value {
                serde_json::Value::Object(fields) => {
                    if let Some(child) = fields.get(first) {
                        collect(child, rest, out);
                    }
                }
                serde_json::Value::Array(values) => {
                    for child in values {
                        collect(child, path, out);
                    }
                }
                _ => {}
            }
        } else {
            match value {
                serde_json::Value::String(text) => {
                    out.push(text.split_whitespace().collect::<Vec<_>>().join(" "))
                }
                serde_json::Value::Array(values) => {
                    for child in values {
                        collect(child, path, out);
                    }
                }
                serde_json::Value::Object(fields) => {
                    out.push(format!("{{{} fields}}", fields.len()))
                }
                serde_json::Value::Null => {}
                other => out.push(other.to_string()),
            }
        }
    }
    let mut values = Vec::new();
    collect(&value, &path.segments, &mut values);
    values.join(", ")
}

#[cfg(test)]
#[path = "record.test.rs"]
mod tests;
