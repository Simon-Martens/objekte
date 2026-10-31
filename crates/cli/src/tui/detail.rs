//! Full record fields arranged as compact, synchronized name and value columns.

use super::state::Hit;
use serde_json::Value;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};


/// One structural field or metadata item in the detail panel.
struct DetailRow {
    name: String,
    value: String,
    matched: bool,
}


/// Wrapped detail lines with a matching-field marker for each name line.
pub(super) struct DetailColumns {
    pub(super) names: Vec<String>,
    pub(super) values: Vec<String>,
    pub(super) matched: Vec<bool>,
}


fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        _ => value.to_string(),
    }
}

fn visible(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(text) => !text.trim().is_empty(),
        Value::Array(items) => items.iter().any(visible),
        Value::Object(fields) => fields.values().any(visible),
        Value::Bool(_) | Value::Number(_) => true,
    }
}

fn array_item(
    index: usize,
    value: &Value,
    depth: usize,
    path: &str,
    matched: &[String],
    rows: &mut Vec<DetailRow>,
) {
    if let Value::Object(fields) = value {
        let mut first = true;
        for (name, child) in fields.iter().filter(|(_, child)| visible(child)) {
            // Put the number beside the first visible child. Later children
            // hang under it, so an object never needs a count-only row.
            let label = if first {
                format!("[{index}] {name}")
            } else {
                format!("    {name}")
            };
            walk(
                &label,
                child,
                depth,
                &format!("{path}.{name}"),
                matched,
                rows,
            );
            first = false;
        }
    } else {
        walk(&format!("[{index}]"), value, depth, path, matched, rows);
    }
}

fn walk(
    name: &str,
    value: &Value,
    depth: usize,
    path: &str,
    matched: &[String],
    rows: &mut Vec<DetailRow>,
) {
    if !visible(value) {
        return;
    }
    let label = format!("{}{}", "  ".repeat(depth), name);
    match value {
        Value::Object(fields) => {
            rows.push(DetailRow {
                name: label,
                value: String::new(),
                matched: false,
            });
            for (name, value) in fields {
                walk(
                    name,
                    value,
                    depth + 1,
                    &format!("{path}.{name}"),
                    matched,
                    rows,
                );
            }
        }
        Value::Array(items) => {
            if items.iter().all(|item| item.is_string()) {
                let item_prefix = format!("{path}[");
                let values = items
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|text| !text.trim().is_empty())
                    .collect::<Vec<_>>();
                rows.push(DetailRow {
                    name: label,
                    value: values.join(", "),
                    matched: matched.iter().any(|field| field.starts_with(&item_prefix)),
                });
                return;
            }
            rows.push(DetailRow {
                name: label,
                value: String::new(),
                matched: false,
            });
            for (index, value) in items.iter().enumerate() {
                if visible(value) {
                    array_item(
                        index,
                        value,
                        depth + 1,
                        &format!("{path}[{index}]"),
                        matched,
                        rows,
                    );
                }
            }
        }
        _ => rows.push(DetailRow {
            name: label,
            value: value_text(value),
            matched: matched.iter().any(|field| field == path),
        }),
    }
}

fn size_kb(size_bytes: u64) -> String {
    format!("{:.2} KB", size_bytes as f64 / 1_000.0)
}

fn rows(hit: &Hit) -> Vec<DetailRow> {
    let record = hit.record();
    let mut rows = vec![
        DetailRow {
            name: "collection".into(),
            value: record.collection.clone(),
            matched: false,
        },
        DetailRow {
            name: "id".into(),
            value: record.id.clone(),
            matched: false,
        },
        DetailRow {
            name: "path".into(),
            value: record.path.clone(),
            matched: false,
        },
        DetailRow {
            name: "status".into(),
            value: format!("{:?}", record.state.status),
            matched: false,
        },
        DetailRow {
            name: "size".into(),
            value: size_kb(record.size_bytes),
            matched: false,
        },
        DetailRow {
            name: "modified_unix_nanos".into(),
            value: record.modified_unix_nanos.clone(),
            matched: false,
        },
        DetailRow {
            name: "hash".into(),
            value: record.hash.clone(),
            matched: false,
        },
    ];
    if !hit.matched_paths().is_empty() {
        rows.push(DetailRow {
            name: "matched paths".into(),
            value: hit.matched_paths().join(", "),
            matched: false,
        });
    }
    rows.retain(|row| !row.value.trim().is_empty());
    let mut entry = Vec::new();
    if let Some(raw) = &record.raw {
        let raw = String::from_utf8_lossy(raw);
        if !raw.trim().is_empty() {
            entry.push(DetailRow {
                name: "$raw".into(),
                value: raw.into_owned(),
                matched: hit.matched_paths().iter().any(|field| field == "$raw"),
            });
        }
    } else if let Value::Object(fields) = &record.data {
        for (name, value) in fields {
            walk(name, value, 0, name, hit.matched_paths(), &mut entry);
        }
    } else {
        walk(
            "$value",
            &record.data,
            0,
            "$value",
            hit.matched_paths(),
            &mut entry,
        );
    }
    if !entry.is_empty() {
        rows.push(DetailRow {
            name: String::new(),
            value: String::new(),
            matched: false,
        });
        rows.extend(entry);
    }
    rows
}

fn visible_text(text: &str) -> String {
    let mut visible = String::new();
    for ch in text.chars() {
        if ch == '\n' {
            visible.push(ch);
        } else if ch.is_control() {
            // Show control characters as escapes so terminal rendering cannot
            // silently change a field's content.
            visible.extend(ch.escape_default());
        } else {
            visible.push(ch);
        }
    }
    visible
}

/// Wrap labels without dropping characters, including source newlines.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = vec![String::new()];
    let mut used = 0;
    for ch in visible_text(text).chars() {
        if ch == '\n' {
            lines.push(String::new());
            used = 0;
            continue;
        }
        let cells = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + cells > width && used > 0 {
            lines.push(String::new());
            used = 0;
        }
        lines.last_mut().expect("one line").push(ch);
        used += cells;
    }
    lines
}

/// Prefer a whitespace boundary, but split a word if it cannot fit.
fn wrap_value(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut used = 0;
    for ch in visible_text(text).chars() {
        if ch == '\n' {
            lines.push(std::mem::take(&mut current));
            used = 0;
            continue;
        }
        let cells = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + cells > width && used > 0 {
            if let Some(end) = current
                .char_indices()
                .rev()
                .find(|(_, candidate)| candidate.is_whitespace())
                .map(|(index, candidate)| index + candidate.len_utf8())
            {
                let tail = current.split_off(end);
                lines.push(std::mem::take(&mut current));
                current = tail;
                used = UnicodeWidthStr::width(current.as_str());
                if used + cells > width && used > 0 {
                    lines.push(std::mem::take(&mut current));
                    used = 0;
                }
            } else {
                lines.push(std::mem::take(&mut current));
                used = 0;
            }
        }
        current.push(ch);
        used += cells;
    }
    lines.push(current);
    lines
}

pub(super) fn columns(hit: &Hit, names_width: usize, values_width: usize) -> DetailColumns {
    let mut names = Vec::new();
    let mut values = Vec::new();
    let mut matched = Vec::new();
    for row in rows(hit) {
        let left = wrap(&row.name, names_width);
        let right = wrap_value(&row.value, values_width.min(80));
        let height = left.len().max(right.len());
        for line in 0..height {
            names.push(left.get(line).cloned().unwrap_or_default());
            values.push(right.get(line).cloned().unwrap_or_default());
            matched.push(row.matched && line < left.len());
        }
    }
    DetailColumns {
        names,
        values,
        matched,
    }
}

#[cfg(test)]
#[path = "detail.test.rs"]
mod tests;
