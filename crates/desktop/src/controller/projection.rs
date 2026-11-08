//! Bounded display projections; source bytes stay available for complete export.

use std::collections::{BTreeSet, HashSet};

use objekte_search_core::inventory::{FileStatus, RecordClass};
use objekte_search_core::model::PathRoot;
use objekte_search_core::query::{FilterExpr, parse_structured_filter};
use objekte_search_core::query_path::{PathStep, QueryPath};
use objekte_search_core::repository::{Payload, SearchHit};
use serde_json::Value;

use crate::backend::{OpenedSession, QueryPage, QuerySpec};
use crate::ui::model::{
    DetailOrigin, DetailRow, DetailValueKind, DetailView, MAX_COLUMNS, MAX_DETAIL_ROWS, PageView,
    RelationRow, ResultRow, RowKey, SearchDraft,
};

use super::filter_validation::field_choices;

/// Build a stable leading schema from the effective predicate and declared fields.
pub fn initial_columns(session: &OpenedSession, draft: &SearchDraft) -> Vec<String> {
    let mut columns = vec!["@id".to_owned(), "@status".to_owned()];
    if !draft.full_text.is_empty() {
        columns.push("@match".to_owned());
    }
    if draft.filter_enabled {
        if let Ok(parsed) = parse_structured_filter(&draft.structured) {
            if let Some(filter) = &parsed.filter {
                append_predicate_fields(filter, &mut columns);
            }
        }
    }
    for field in field_choices(session, draft.collection.as_deref(), &[]) {
        if !columns.contains(&field.path) {
            columns.push(field.path);
        }
    }
    columns
}

fn append_predicate_fields(expression: &FilterExpr, columns: &mut Vec<String>) {
    match expression {
        FilterExpr::Term(term) => {
            let path = term.path.display.clone();
            if !columns.contains(&path) {
                columns.push(path);
            }
        }
        FilterExpr::And(parts) | FilterExpr::Or(parts) => {
            for part in parts {
                append_predicate_fields(part, columns);
            }
        }
        FilterExpr::Not(part) => append_predicate_fields(part, columns),
    }
}

/// Present a bounded query page while keeping its full backend hits separately.
pub fn page_view(page: &QueryPage, spec: &QuerySpec, hidden: &[String]) -> PageView {
    page_view_with_available(page, spec, hidden).0
}

/// Return both the visible grid and every currently discoverable column.
pub fn page_view_with_available(
    page: &QueryPage,
    spec: &QuerySpec,
    hidden: &[String],
) -> (PageView, Vec<String>) {
    page_view_with_prior(page, spec, hidden, &[])
}

/// Preserve the established order when a later delivery page reveals fields.
pub fn page_view_with_prior(
    page: &QueryPage,
    spec: &QuerySpec,
    hidden: &[String],
    prior: &[String],
) -> (PageView, Vec<String>) {
    let mut columns = if prior.is_empty() {
        let mut leading = vec!["@id".to_owned(), "@status".to_owned()];
        if !spec.full_text.is_empty() {
            leading.push("@match".to_owned());
        }
        leading
    } else {
        prior.to_vec()
    };
    let mut discovered = BTreeSet::new();
    for hit in &page.hits {
        if let Some(Value::Object(object)) = payload_json(hit) {
            discovered.extend(object.keys().cloned());
        }
    }
    for key in discovered {
        if key != "@collection" && key != "@file" && !columns.contains(&key) && columns.len() < 254
        {
            columns.push(key);
        }
    }
    for metadata in ["@collection", "@file"] {
        if !columns.iter().any(|column| column == metadata) {
            columns.push(metadata.to_owned());
        }
    }
    let available = columns.clone();
    columns.retain(|column| !hidden.contains(column));
    let clipped = columns.len() > MAX_COLUMNS;
    columns.truncate(MAX_COLUMNS);
    let rows = page
        .hits
        .iter()
        .enumerate()
        .map(|(index, hit)| {
            let json = payload_json(hit);
            let cells = columns
                .iter()
                .map(|column| match column.as_str() {
                    "@collection" => hit.collection.clone(),
                    "@id" => hit.record_id.clone(),
                    "@status" => status_label(hit),
                    "@match" => match_preview(hit, json.as_ref(), spec),
                    "@file" => hit.file_path.display().to_string(),
                    field => project_field(json.as_ref(), field),
                })
                .collect();
            ResultRow {
                key: RowKey {
                    token: (page.page_start + index) as u64,
                    collection: hit.collection.clone(),
                    record_id: hit.record_id.clone(),
                    file: hit.file_path.display().to_string(),
                    byte_start: hit.byte_start,
                },
                cells,
                matched_fields: hit.matched_fields.clone(),
                preview: json.as_ref().map(value_label),
            }
        })
        .collect();
    let view = PageView {
        columns,
        rows,
        offset: page.page_start,
        requested_limit: spec.limit,
        exact_total: page.count,
        count_pending: spec.count && page.count.is_none(),
        has_previous: page.page_start > 0,
        has_next: page.next_page_start.is_some(),
        selected: None,
        clipped,
    }
    .bounded();
    (view, available)
}

fn status_label(hit: &SearchHit) -> String {
    if hit.record_class == RecordClass::RawFallback {
        format!("{} · raw fallback", hit.file_status.label())
    } else {
        hit.file_status.label().to_owned()
    }
}

fn project_field(json: Option<&Value>, path: &str) -> String {
    let Some(path) = QueryPath::parse(path).ok() else {
        return "Unavailable".into();
    };
    if path.field.root != PathRoot::Data {
        return "Unavailable".into();
    }
    let Some(mut value) = json else {
        return "Unavailable".into();
    };
    for step in &path.steps {
        value = match step {
            PathStep::Field(name) => match value.get(name) {
                Some(value) => value,
                None => return "—".into(),
            },
            PathStep::ArrayIndex(index) => match value.get(*index as usize) {
                Some(value) => value,
                None => return "—".into(),
            },
            PathStep::EachElement => return "Unavailable".into(),
        };
    }
    match value {
        Value::Null => "null".into(),
        Value::String(text) if text.is_empty() => "\"\"".into(),
        Value::Array(values) => format!("{} items", values.len()),
        Value::Object(object) => format!("{} fields", object.len()),
        _ => value_label(value),
    }
}

fn match_preview(hit: &SearchHit, json: Option<&Value>, spec: &QuerySpec) -> String {
    if hit.matched_fields.is_empty() {
        return "Source: match".into();
    }
    let mut lines = Vec::new();
    for field in hit.matched_fields.iter().take(3) {
        let excerpt = if field.starts_with('@') {
            "matched".to_owned()
        } else {
            let value = project_field(json, field);
            if value == "Unavailable" || value == "—" {
                "matched; inspect Source".to_owned()
            } else if !spec.regex && spec.case_sensitive {
                match value.find(&spec.full_text) {
                    Some(start) => {
                        let before = value[..start]
                            .char_indices()
                            .rev()
                            .nth(30)
                            .map_or(0, |(at, _)| at);
                        let after = value[start..]
                            .char_indices()
                            .nth(100)
                            .map_or(value.len(), |(at, _)| start + at);
                        value[before..after].to_owned()
                    }
                    None => "matched; inspect record".to_owned(),
                }
            } else {
                "matched; inspect record".to_owned()
            }
        };
        lines.push(format!("{field}: {excerpt}"));
    }
    if hit.matched_fields.len() > 3 {
        lines.push(format!("+{} more", hit.matched_fields.len() - 3));
    }
    lines.join(" · ")
}

/// Present the original source, parsed structure, and locator metadata.
pub fn detail_view(hit: &SearchHit, expanded: &HashSet<String>) -> DetailView {
    let mut detail = DetailView {
        title: format!("{} / {}", hit.collection, hit.record_id),
        validity: Some(
            hit.file_status == FileStatus::Valid && hit.record_class == RecordClass::Logical,
        ),
        raw_source: source_text(hit),
        source_label: match hit.payload {
            Some(Payload::Bytes(_)) => "Original source".into(),
            Some(Payload::Logical(_)) => {
                "Logical representation; original source unavailable".into()
            }
            Some(Payload::LogicalUnavailable { .. }) | None => "Original source unavailable".into(),
        },
        ..DetailView::default()
    };
    match payload_json(hit) {
        Some(json) => append_value(
            &mut detail.structured,
            "$",
            &json,
            0,
            expanded,
            &hit.matched_fields,
        ),
        None => detail.structured.push(DetailRow {
            label: "Payload".into(),
            value: "Logical data unavailable; inspect original source and diagnostics".into(),
            depth: 0,
            matched: false,
            expandable: false,
            expanded: false,
            origin: DetailOrigin::Diagnostic,
            value_kind: DetailValueKind::LogicalUnavailable,
        }),
    }
    let metadata = [
        ("Collection", hit.collection.clone()),
        ("Record ID", hit.record_id.clone()),
        ("File", hit.file_path.display().to_string()),
        (
            "Byte range",
            format!("{}..{}", hit.byte_start, hit.byte_end),
        ),
        ("File status", format!("{:?}", hit.file_status)),
        ("Record class", format!("{:?}", hit.record_class)),
        ("Diagnostic", format!("{:?}", hit.diagnostic_ref)),
    ];
    for (label, value) in metadata {
        detail.structured.push(DetailRow {
            label: label.into(),
            value,
            depth: 0,
            matched: false,
            expandable: false,
            expanded: false,
            origin: DetailOrigin::Metadata,
            value_kind: DetailValueKind::Present,
        });
    }
    detail.relations = Vec::<RelationRow>::new();
    detail.bounded()
}

fn append_value(
    rows: &mut Vec<DetailRow>,
    path: &str,
    value: &Value,
    depth: usize,
    expanded: &HashSet<String>,
    matched_fields: &[String],
) {
    if rows.len() >= MAX_DETAIL_ROWS {
        return;
    }
    let Some((display, expandable)) = field_display(value) else {
        return;
    };
    let is_expanded = expandable && ((depth == 0 && value.is_object()) || expanded.contains(path));
    rows.push(DetailRow {
        label: path.into(),
        value: display,
        depth,
        matched: matched_fields.iter().any(|field| {
            let path = path.strip_prefix("$.").unwrap_or(path);
            field == path
                || field.starts_with(&format!("{path}."))
                || field.starts_with(&format!("{path}["))
        }),
        expandable,
        expanded: is_expanded,
        origin: DetailOrigin::Structured,
        value_kind: if value.is_null() {
            DetailValueKind::JsonNull
        } else {
            DetailValueKind::Present
        },
    });
    if !is_expanded || depth >= 20 {
        return;
    }
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                append_value(
                    rows,
                    &format!("{path}.{key}"),
                    child,
                    depth + 1,
                    expanded,
                    matched_fields,
                );
            }
        }
        Value::Array(values) => {
            for (index, child) in values.iter().enumerate() {
                append_value(
                    rows,
                    &format!("{path}[{index}]"),
                    child,
                    depth + 1,
                    expanded,
                    matched_fields,
                );
            }
        }
        _ => {}
    }
}

/// Keep arrays with a small scalar shape inline; large or nested containers
/// stay collapsed until requested so opening a record does not flatten them.
fn field_display(value: &Value) -> Option<(String, bool)> {
    match value {
        Value::Null => None,
        Value::String(text) if text.trim().is_empty() => None,
        Value::Array(values) if values.is_empty() => None,
        Value::Array(values)
            if values.len() <= 64
                && values
                    .iter()
                    .all(|value| !value.is_array() && !value.is_object()) =>
        {
            let visible = values
                .iter()
                .filter_map(|value| match value {
                    Value::Null => None,
                    Value::String(text) if text.trim().is_empty() => None,
                    Value::String(text) => Some(text.clone()),
                    _ => Some(value.to_string()),
                })
                .collect::<Vec<_>>();
            (!visible.is_empty()).then(|| (visible.join("; "), false))
        }
        Value::Array(values) => Some((format!("{} items", values.len()), true)),
        Value::Object(object) if object.is_empty() => None,
        Value::Object(object) => Some((format!("{} fields", object.len()), true)),
        _ => Some((value_label(value), false)),
    }
}

fn payload_json(hit: &SearchHit) -> Option<Value> {
    objekte_tui::record_json_value(hit)
}

fn source_text(hit: &SearchHit) -> String {
    objekte_tui::record_raw_text(hit)
}

fn value_label(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        _ => value.to_string(),
    }
}

#[cfg(test)]
#[path = "projection.test.rs"]
mod tests;
