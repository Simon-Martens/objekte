//! Full record fields arranged as compact, synchronized name and value columns.

use super::app::Hit;
use serde_json::Value;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};


/// One structural field or metadata item in the record view.
pub(super) struct DetailRow {
    /// Indented label, such as `  [0] name`.
    pub(super) name: String,
    pub(super) value: String,
    pub(super) matched: bool,
    /// JSON Pointer into the record; empty for metadata and separators.
    pub(super) pointer: String,
    /// Filter path with implicit array traversal, such as
    /// `contributors.agents`, or a system field for metadata.
    pub(super) filter_path: String,
    /// The single scalar value of a leaf, for filtering by it.
    pub(super) scalar: Option<Value>,
    /// Inside an array, so a filter path reaches several values.
    pub(super) many: bool,
    /// An object or array whose children follow as deeper rows.
    pub(super) branch: bool,
    /// A branch for an array; arrays start folded in the record view.
    pub(super) array: bool,
    /// Visible members of a branch: array items or object fields.
    pub(super) members: usize,
}


impl DetailRow {
    fn meta(name: &str, value: String, filter_path: &str) -> Self {
        Self {
            name: name.into(),
            scalar: (!filter_path.is_empty()).then(|| Value::String(value.clone())),
            value,
            matched: false,
            pointer: String::new(),
            filter_path: filter_path.into(),
            branch: false,
            array: false,
            members: 0,
            many: false,
        }
    }

    fn separator() -> Self {
        Self::meta("", String::new(), "")
    }

    pub(super) fn is_separator(&self) -> bool {
        self.name.is_empty() && self.value.is_empty()
    }

    /// Whether `ancestor` (a branch pointer) contains this row.
    pub(super) fn is_inside(&self, ancestor: &str) -> bool {
        self.pointer.len() > ancestor.len()
            && self.pointer.starts_with(ancestor)
            && self.pointer.as_bytes()[ancestor.len()] == b'/'
    }
}


/// Wrapped detail lines with a matching-field marker for each name line.
pub(super) struct DetailColumns {
    /// Width of the name column actually used.
    pub(super) names_width: usize,
    pub(super) names: Vec<String>,
    pub(super) values: Vec<String>,
    pub(super) matched: Vec<bool>,
    /// The input row each line belongs to.
    pub(super) row: Vec<usize>,
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

fn identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first.is_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_alphanumeric())
}

/// Where a walk is in the record, in the three path notations.
#[derive(Clone)]
struct Place {
    /// As reported in matched paths: `notes[0].body`.
    matched: String,
    pointer: String,
    filter: String,
    many: bool,
}


impl Place {
    fn root(name: &str) -> Self {
        Self {
            matched: name.into(),
            pointer: format!("/{}", escape_pointer(name)),
            filter: if identifier(name) {
                name.into()
            } else {
                format!("[{}]", serde_json::to_string(name).expect("string"))
            },
            many: false,
        }
    }

    fn key(&self, name: &str) -> Self {
        Self {
            matched: format!("{}.{name}", self.matched),
            pointer: format!("{}/{}", self.pointer, escape_pointer(name)),
            filter: if identifier(name) {
                format!("{}.{name}", self.filter)
            } else {
                format!(
                    "{}[{}]",
                    self.filter,
                    serde_json::to_string(name).expect("string")
                )
            },
            many: self.many,
        }
    }

    /// Arrays are traversed implicitly in filters, so the index is dropped.
    fn index(&self, index: usize) -> Self {
        Self {
            matched: format!("{}[{index}]", self.matched),
            pointer: format!("{}/{index}", self.pointer),
            filter: self.filter.clone(),
            many: true,
        }
    }
}


fn escape_pointer(name: &str) -> String {
    name.replace('~', "~0").replace('/', "~1")
}

fn array_item(
    index: usize,
    value: &Value,
    depth: usize,
    place: &Place,
    matched: &[String],
    rows: &mut Vec<DetailRow>,
) {
    let place = place.index(index);
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
            walk(&label, child, depth, &place.key(name), matched, rows);
            first = false;
        }
    } else {
        walk(&format!("[{index}]"), value, depth, &place, matched, rows);
    }
}

fn walk(
    name: &str,
    value: &Value,
    depth: usize,
    place: &Place,
    matched: &[String],
    rows: &mut Vec<DetailRow>,
) {
    if !visible(value) {
        return;
    }
    let label = format!("{}{}", "  ".repeat(depth), name);
    let row = |value: String, matched: bool, scalar: Option<Value>, branch: bool| DetailRow {
        name: label.clone(),
        value,
        matched,
        pointer: place.pointer.clone(),
        filter_path: place.filter.clone(),
        scalar,
        branch,
        array: false,
        members: 0,
        many: place.many,
    };
    match value {
        Value::Object(fields) => {
            rows.push(row(String::new(), false, None, true));
            rows.last_mut().expect("pushed").members =
                fields.values().filter(|value| visible(value)).count();
            for (name, value) in fields {
                walk(name, value, depth + 1, &place.key(name), matched, rows);
            }
        }
        Value::Array(items) => {
            if items.iter().all(|item| item.is_string()) {
                let item_prefix = format!("{}[", place.matched);
                let values = items
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|text| !text.trim().is_empty())
                    .collect::<Vec<_>>();
                let single = (values.len() == 1).then(|| Value::String(values[0].into()));
                rows.push(row(
                    values.join(", "),
                    matched.iter().any(|field| field.starts_with(&item_prefix)),
                    single,
                    false,
                ));
                // A string array always reaches several values.
                rows.last_mut().expect("pushed").many = true;
                return;
            }
            rows.push(row(String::new(), false, None, true));
            let branch = rows.last_mut().expect("pushed");
            branch.array = true;
            branch.members = items.iter().filter(|item| visible(item)).count();
            for (index, value) in items.iter().enumerate() {
                if visible(value) {
                    array_item(index, value, depth + 1, place, matched, rows);
                }
            }
        }
        _ => rows.push(row(
            value_text(value),
            matched.contains(&place.matched),
            Some(value.clone()),
            false,
        )),
    }
}

fn size_kb(size_bytes: u64) -> String {
    format!("{:.2} KB", size_bytes as f64 / 1_000.0)
}

/// Every row of a record: metadata first (unless `metadata` is false),
/// then the record's own fields in order.
pub(super) fn rows(hit: &Hit, metadata: bool) -> Vec<DetailRow> {
    let record = hit.record();
    let mut rows = vec![
        DetailRow::meta("collection", record.collection.clone(), "@collection"),
        DetailRow::meta("id", record.id.clone(), "@id"),
        DetailRow::meta("path", record.path.clone(), "@path"),
        DetailRow::meta("status", format!("{:?}", record.state.status), ""),
        DetailRow::meta("size", size_kb(record.size_bytes), ""),
        DetailRow::meta(
            "modified_unix_nanos",
            record.modified_unix_nanos.clone(),
            "",
        ),
        DetailRow::meta("hash", record.hash.clone(), ""),
    ];
    if !hit.matched_paths().is_empty() {
        rows.push(DetailRow::meta(
            "matched paths",
            hit.matched_paths().join(", "),
            "",
        ));
    }
    rows.retain(|row| !row.value.trim().is_empty());
    if !metadata {
        rows.clear();
    }
    let mut entry = Vec::new();
    if let Some(raw) = &record.raw {
        let raw = String::from_utf8_lossy(raw);
        if !raw.trim().is_empty() {
            let mut row = DetailRow::meta("$raw", raw.into_owned(), "");
            row.matched = hit.matched_paths().iter().any(|field| field == "$raw");
            entry.push(row);
        }
    } else if let Value::Object(fields) = &record.data {
        for (name, value) in fields {
            walk(
                name,
                value,
                0,
                &Place::root(name),
                hit.matched_paths(),
                &mut entry,
            );
        }
    } else {
        let place = Place {
            matched: "$value".into(),
            pointer: String::new(),
            filter: "$value".into(),
            many: false,
        };
        walk(
            "$value",
            &record.data,
            0,
            &place,
            hit.matched_paths(),
            &mut entry,
        );
    }
    if !entry.is_empty() && !rows.is_empty() {
        rows.push(DetailRow::separator());
    }
    rows.extend(entry);
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

/// Shorten a label to one line, marking the cut with an ellipsis.
fn truncate(text: &str, width: usize) -> String {
    let text = visible_text(text).replace('\n', " ");
    if UnicodeWidthStr::width(text.as_str()) <= width {
        return text;
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let cells = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + cells + 1 > width {
            break;
        }
        out.push(ch);
        used += cells;
    }
    out.push('…');
    out
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

/// Name and value lines for `rows`. Names take one line each, as wide as
/// the widest name up to `max_names_width`; values wrap in the rest of
/// `width`.
pub(super) fn columns(rows: &[&DetailRow], max_names_width: usize, width: usize) -> DetailColumns {
    let names_width = rows
        .iter()
        .map(|row| UnicodeWidthStr::width(row.name.as_str()))
        .max()
        .unwrap_or(0)
        .clamp(4, max_names_width.max(4));
    let values_width = width.saturating_sub(names_width + 2).clamp(10, 100);
    let mut out = DetailColumns {
        names_width,
        names: Vec::new(),
        values: Vec::new(),
        matched: Vec::new(),
        row: Vec::new(),
    };
    for (index, row) in rows.iter().enumerate() {
        let right = wrap_value(&row.value, values_width);
        for (line, value) in right.into_iter().enumerate() {
            out.names.push(if line == 0 {
                truncate(&row.name, names_width)
            } else {
                String::new()
            });
            out.values.push(value);
            out.matched.push(row.matched && line == 0);
            out.row.push(index);
        }
    }
    out
}

#[cfg(test)]
#[path = "detail.test.rs"]
mod tests;
