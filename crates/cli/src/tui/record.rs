//! Structured record rows shared by the preview and full detail view.

use std::collections::BTreeSet;

use objekte_search_core::diagnostics::signed_milliseconds_3;
use objekte_search_core::model::{Number, Value};
use objekte_search_core::query_path::{PathStep, QueryPath};
use objekte_search_core::repository::{Payload, SearchHit};

use super::input::Input;

const MAX_ROWS: usize = 10_000;
const MAX_DEPTH: usize = 40;

#[cfg(test)]
thread_local! { static JSON_DECODE_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

#[cfg(test)]
pub(super) fn take_decode_count() -> usize {
    JSON_DECODE_COUNT.with(|count| count.replace(0))
}


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
    /// Record row under each rendered line of the full detail view.
    pub(super) visible_rows: Vec<usize>,
    pub(super) raw: bool,
    pub(super) find: String,
    pub(super) find_input: Option<Input>,
    collapsed_objects: BTreeSet<String>,
    expanded_arrays: BTreeSet<String>,
    parsed: Option<serde_json::Value>,
    parsed_id: Option<(String, String)>,
}


impl RecordState {
    pub(super) fn reset(&mut self) {
        self.cursor = 0;
        self.scroll = 0;
        self.visible_rows.clear();
        self.raw = false;
        self.find.clear();
        self.find_input = None;
        self.collapsed_objects.clear();
        self.expanded_arrays.clear();
        self.parsed = None;
        self.parsed_id = None;
    }

    pub(super) fn load(&mut self, hit: &SearchHit) {
        self.parsed = json_value(hit);
        self.parsed_id = Some((hit.collection.clone(), hit.record_id.clone()));
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
        let cached = self
            .parsed_id
            .as_ref()
            .is_some_and(|(collection, id)| collection == &hit.collection && id == &hit.record_id);
        let uncached = (!cached).then(|| json_value(hit)).flatten();
        let value = if cached {
            self.parsed.as_ref()
        } else {
            uncached.as_ref()
        };
        if let Some(value) = value {
            match value {
                serde_json::Value::Object(fields) => {
                    for (name, value) in fields {
                        walk(
                            self,
                            &mut rows,
                            name,
                            value,
                            (name, name),
                            0,
                            &hit.matched_fields,
                        );
                    }
                }
                _ => walk(
                    self,
                    &mut rows,
                    "$value",
                    value,
                    ("$value", "$value"),
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
        plain(&mut rows, "Metadata", String::new());
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
    location: (&str, &str),
    depth: usize,
    matched_fields: &[String],
) {
    let (pointer, matched_path) = location;
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
                    (&pointer, &matched_path),
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
                    (&pointer, &matched_path),
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
        Some(Payload::Bytes(bytes)) => {
            #[cfg(test)]
            JSON_DECODE_COUNT.with(|count| count.set(count.get() + 1));
            serde_json::from_slice(bytes).ok()
        }
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
        Value::Number(Number::ExactDecimal(number)) => serde_json::from_str(&number.to_string())
            .unwrap_or_else(|_| Json::String(number.to_string())),
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
    let text = match &hit.payload {
        Some(Payload::Bytes(bytes)) => String::from_utf8_lossy(bytes).into_owned(),
        Some(Payload::Logical(_)) => json_value(hit)
            .and_then(|value| serde_json::to_string_pretty(&value).ok())
            .unwrap_or_default(),
        Some(Payload::LogicalUnavailable { .. }) => "Logical value unavailable".into(),
        None => "Record payload not loaded".into(),
    };
    text.chars()
        .map(|ch| {
            if ch.is_control() && ch != '\n' && ch != '\t' {
                '�'
            } else {
                ch
            }
        })
        .collect()
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


/// A bounded, display-only projection prepared when a result page arrives.
#[derive(Clone, Debug, Default)]
pub(super) struct DisplayRow {
    pub fields: Vec<String>,
    pub matched: String,
}


pub(super) fn display_row(
    hit: &SearchHit,
    columns: &[String],
    search: &str,
    case_sensitive: bool,
    regex: bool,
) -> DisplayRow {
    let parsed = json_value(hit);
    let fields = columns
        .iter()
        .map(|column| project(hit, parsed.as_ref(), column))
        .collect();
    let mut matched = String::new();
    for evidence in &hit.matched_fields {
        let candidate = project(hit, parsed.as_ref(), evidence);
        if candidate == "∅" || candidate == "unavailable" {
            continue;
        }
        if search.is_empty() {
            matched = format!("{evidence}: {candidate}");
            break;
        }
        if let Some(snippet) = matching_snippet(&candidate, search, case_sensitive, regex) {
            matched = format!("{evidence}: {snippet}");
            break;
        }
    }
    DisplayRow { fields, matched }
}

fn project(hit: &SearchHit, value: Option<&serde_json::Value>, label: &str) -> String {
    let metadata = match label {
        "@record.id" => Some(hit.record_id.clone()),
        "@record.raw_text" => Some(raw_text(hit)),
        "@record.byte_start" => Some(hit.byte_start.to_string()),
        "@record.byte_end" => Some(hit.byte_end.to_string()),
        "@record.byte_length" => Some((hit.byte_end - hit.byte_start).to_string()),
        "@record.class" => Some(hit.record_class.code().to_string()),
        "@file.id" => Some(hit.file_id.to_string()),
        "@file.path" => Some(hit.file_path.display().to_string()),
        "@file.size" => Some(hit.file_size.to_string()),
        "@file.status" => Some(hit.file_status.code().to_string()),
        "@file.created" => Some(hit.file_created_ns.map_or("null".into(), |value| {
            format!("{} ms", signed_milliseconds_3(value))
        })),
        "@file.modified" => Some(format!("{} ms", signed_milliseconds_3(hit.modified_ns))),
        "@collection.name" => Some(hit.collection.clone()),
        _ => None,
    };
    if let Some(metadata) = metadata {
        return metadata;
    }
    let (reducer, inside) = ["MIN", "MAX", "FIRST", "LAST"]
        .into_iter()
        .find_map(|name| {
            label
                .strip_prefix(&format!("{name}("))
                .and_then(|inner| inner.strip_suffix(')'))
                .map(|inner| (Some(name), inner))
        })
        .unwrap_or((None, label));
    let path_label = inside.split(':').next().unwrap_or(inside);
    let Ok(path) = QueryPath::parse(path_label) else {
        return "∅".into();
    };
    let Some(value) = value else {
        return "unavailable".into();
    };
    let mut found = Vec::new();
    let mut empty_array = false;
    collect_path(value, &path.steps, &mut found, &mut empty_array);
    if found.is_empty() {
        return if empty_array {
            "[]".into()
        } else {
            "∅".into()
        };
    }
    if let Some(reducer) = reducer {
        let selected = match reducer {
            "FIRST" => found.first(),
            "LAST" => found.last(),
            "MIN" => found.iter().min_by_key(|value| display_json(value)),
            "MAX" => found.iter().max_by_key(|value| display_json(value)),
            _ => None,
        };
        return selected.map_or("∅".into(), |value| display_json(value));
    }
    found
        .into_iter()
        .map(display_json)
        .collect::<Vec<_>>()
        .join(", ")
}

fn collect_path<'a>(
    value: &'a serde_json::Value,
    steps: &[PathStep],
    out: &mut Vec<&'a serde_json::Value>,
    empty_array: &mut bool,
) {
    let Some((step, rest)) = steps.split_first() else {
        out.push(value);
        return;
    };
    match step {
        PathStep::Field(name) => match value {
            serde_json::Value::Object(fields) => {
                if let Some(child) = fields.get(name) {
                    collect_path(child, rest, out, empty_array);
                }
            }
            serde_json::Value::Array(items) => {
                if items.is_empty() {
                    *empty_array = true;
                }
                for child in items {
                    collect_path(child, steps, out, empty_array);
                }
            }
            _ => {}
        },
        PathStep::ArrayIndex(index) => {
            if let Some(child) = value
                .as_array()
                .and_then(|items| items.get(*index as usize))
            {
                collect_path(child, rest, out, empty_array);
            }
        }
        PathStep::EachElement => {
            if let Some(items) = value.as_array() {
                if items.is_empty() {
                    *empty_array = true;
                }
                for child in items {
                    collect_path(child, rest, out, empty_array);
                }
            }
        }
    }
}

fn display_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "null".into(),
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn matching_snippet(text: &str, needle: &str, sensitive: bool, regex: bool) -> Option<String> {
    let start = if regex {
        regex::RegexBuilder::new(needle)
            .case_insensitive(!sensitive)
            .build()
            .ok()?
            .find(text)?
            .start()
    } else if sensitive {
        text.find(needle)?
    } else {
        use unicode_casefold::{Locale, UnicodeCaseFold, Variant};
        let mut folded = String::new();
        let mut offsets = Vec::new();
        for (offset, ch) in text.char_indices() {
            for part in ch.case_fold_with(Variant::Full, Locale::NonTurkic) {
                let mut bytes = [0; 4];
                let size = part.encode_utf8(&mut bytes).len();
                folded.push(part);
                offsets.extend(std::iter::repeat_n(offset, size));
            }
        }
        offsets
            .get(
                folded.find(
                    needle
                        .case_fold_with(Variant::Full, Locale::NonTurkic)
                        .collect::<String>()
                        .as_str(),
                )?,
            )
            .copied()?
    };
    let chars_before = text[..start].chars().count();
    let begin = chars_before.saturating_sub(24);
    let snippet = text.chars().skip(begin).take(96).collect::<String>();
    Some(format!(
        "{}{}{}",
        if begin > 0 { "…" } else { "" },
        snippet,
        if text.chars().count() > begin + 96 {
            "…"
        } else {
            ""
        }
    ))
}

#[cfg(test)]
#[path = "record.test.rs"]
mod tests;
