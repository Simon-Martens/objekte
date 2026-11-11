//! Bounded display projections; source bytes stay available for complete export.

use std::collections::{BTreeSet, HashSet};

use objekte_search_core::fts_preview::PreviewMatcher;
use objekte_search_core::inventory::{FileStatus, RecordClass};
use objekte_search_core::model::PathRoot;
use objekte_search_core::query::{
    CaseSensitivity, FilterExpr, FtsMode, FtsQuery, RegexOptions, parse_structured_filter,
};
use objekte_search_core::query_path::{PathStep, QueryPath};
use objekte_search_core::repository::{Payload, SearchHit};
use serde_json::Value;

use crate::backend::{OpenedSession, QueryPage, QuerySpec};
use crate::ui::model::{
    DetailOrigin, DetailRow, DetailValueKind, DetailView, MAX_COLUMNS, MAX_DETAIL_ROWS, PageView,
    RelationRow, ResultRow, RowKey, SearchDraft,
};
use crate::ui::preferences::nested_column;

use super::filter_validation::field_choices;

/// Build a stable leading schema from the effective predicate and declared fields.
pub fn initial_columns(session: &OpenedSession, draft: &SearchDraft) -> Vec<String> {
    let mut columns = vec!["@status".to_owned(), "@id".to_owned()];
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
    let (view, available, _) = page_view_with_prior(page, spec, hidden, &[], &[]);
    (view, available)
}

/// Preserve the established order when a later delivery page reveals fields.
pub fn page_view_with_prior(
    page: &QueryPage,
    spec: &QuerySpec,
    hidden: &[String],
    shown_nested: &[String],
    prior: &[String],
) -> (PageView, Vec<String>, Vec<String>) {
    let mut columns = if prior.is_empty() {
        let mut leading = vec!["@status".to_owned(), "@id".to_owned()];
        if !spec.full_text.is_empty() {
            leading.push("@match".to_owned());
        }
        leading
    } else {
        prior.to_vec()
    };
    let mut discovered = BTreeSet::new();
    for hit in &page.hits {
        if let Some(value) = payload_json(hit) {
            discover_columns(&value, &mut Vec::new(), &mut discovered);
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
    let effective_hidden = available
        .iter()
        .filter(|column| {
            hidden.contains(*column)
                || (nested_column(column) && !shown_nested.contains(*column))
        })
        .cloned()
        .collect::<Vec<_>>();
    columns.retain(|column| !effective_hidden.contains(column));
    let clipped = columns.len() > MAX_COLUMNS;
    columns.truncate(MAX_COLUMNS);
    let matcher = (!spec.full_text.trim().is_empty())
        .then(|| FtsQuery {
            case_sensitivity: if spec.case_sensitive {
                CaseSensitivity::Sensitive
            } else {
                CaseSensitivity::Insensitive
            },
            mode: if spec.regex {
                FtsMode::Regex(RegexOptions::default())
            } else {
                FtsMode::Literal
            },
            literal: spec.full_text.trim().to_owned(),
            pattern_parameter: None,
            fields: Vec::new(),
        })
        .and_then(|query| PreviewMatcher::new(&query).ok());
    let rows = page
        .hits
        .iter()
        .enumerate()
        .map(|(index, hit)| {
            let json = payload_json(hit);
            let (match_text, match_highlights) =
                match_preview(hit, json.as_ref(), matcher.as_ref());
            let cells = columns
                .iter()
                .map(|column| match column.as_str() {
                    "@collection" => hit.collection.clone(),
                    "@id" => hit.record_id.clone(),
                    "@status" => status_label(hit),
                    "@match" => match_text.clone(),
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
                match_highlights,
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
        incomplete: page.incomplete(),
        count_pending: spec.count && page.count.is_none(),
        has_previous: page.page_start > 0,
        has_next: page.next_page_start.is_some(),
        selected: None,
        clipped,
    }
    .bounded();
    (view, available, effective_hidden)
}

/// Include nested object paths in the chooser while the default visibility
/// policy keeps them out of the table. Discovery is bounded per page.
fn discover_columns(value: &Value, segments: &mut Vec<String>, found: &mut BTreeSet<String>) {
    if segments.len() >= 4 || found.len() >= 254 {
        return;
    }
    if let Value::Array(items) = value {
        for item in items.iter().take(8) {
            discover_columns(item, segments, found);
        }
        return;
    }
    let Value::Object(object) = value else { return; };
    for (name, child) in object {
        if segments.is_empty() && matches!(name.as_str(), "@collection" | "@file") {
            continue;
        }
        segments.push(name.clone());
        found.insert(
            objekte_search_core::model::FieldPath {
                root: PathRoot::Data,
                segments: segments.clone(),
            }
            .display(),
        );
        discover_columns(child, segments, found);
        segments.pop();
        if found.len() >= 254 {
            break;
        }
    }
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
    let Some(value) = json else {
        return "Unavailable".into();
    };
    let mut values = Vec::new();
    path_values(value, &path.steps, &mut values);
    if values.is_empty() { return "—".into(); }
    values.into_iter().take(16).map(project_value).collect::<Vec<_>>().join("; ")
}

fn path_values<'a>(value: &'a Value, steps: &[PathStep], found: &mut Vec<&'a Value>) {
    if found.len() >= 16 { return; }
    let Some((step, rest)) = steps.split_first() else {
        found.push(value);
        return;
    };
    match step {
        PathStep::Field(name) => {
            if let Value::Array(items) = value {
                for item in items { path_values(item, steps, found); }
            } else if let Some(child) = value.get(name) {
                path_values(child, rest, found);
            }
        }
        PathStep::ArrayIndex(index) => {
            if let Some(child) = value.get(*index as usize) { path_values(child, rest, found); }
        }
        PathStep::EachElement => {
            if let Value::Array(items) = value {
                for item in items { path_values(item, rest, found); }
            }
        }
    }
}

fn project_value(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::String(text) if text.is_empty() => "-".into(),
        Value::Array(values) => format!("{} items", values.len()),
        Value::Object(object) => format!("{} fields", object.len()),
        _ => value_label(value),
    }
}

fn match_preview(
    hit: &SearchHit,
    json: Option<&Value>,
    matcher: Option<&PreviewMatcher>,
) -> (String, Vec<(std::ops::Range<usize>, bool)>) {
    let mut text = String::new();
    let mut highlights = Vec::new();
    for field in &hit.matched_fields {
        let found = matcher.and_then(|matcher| {
            if field == "@root" {
                json.and_then(|value| matching_excerpt(value, matcher))
            } else {
                matched_excerpt(json, field, matcher)
            }
        });
        if !text.is_empty() {
            text.push('\n');
        }
        let label_start = text.len();
        text.push_str(field);
        highlights.push((label_start..text.len(), false));
        if let Some((excerpt, span)) = found {
            text.push_str(": ");
            let excerpt_start = text.len();
            text.push_str(&excerpt);
            highlights.push((excerpt_start + span.start..excerpt_start + span.end, true));
        }
    }
    if text.is_empty() {
        text.push_str("Match field unavailable");
    }
    (text, highlights)
}

fn matched_excerpt(json: Option<&Value>, field: &str, matcher: &PreviewMatcher) -> Option<(String, std::ops::Range<usize>)> {
    let path = QueryPath::parse(field).ok()?;
    if path.field.root != PathRoot::Data { return None; }
    matching_excerpt_at(json?, &path.steps, matcher)
}

fn matching_excerpt_at(value: &Value, steps: &[PathStep], matcher: &PreviewMatcher) -> Option<(String, std::ops::Range<usize>)> {
    let Some((step, rest)) = steps.split_first() else { return matching_excerpt(value, matcher); };
    match step {
        PathStep::Field(name) => {
            if let Value::Array(items) = value {
                items.iter().find_map(|item| matching_excerpt_at(item, steps, matcher))
            } else {
                matching_excerpt_at(value.get(name)?, rest, matcher)
            }
        }
        PathStep::ArrayIndex(index) => matching_excerpt_at(value.get(*index as usize)?, rest, matcher),
        PathStep::EachElement => match value {
            Value::Array(items) => items.iter().find_map(|item| matching_excerpt_at(item, rest, matcher)),
            _ => None,
        }
    }
}

fn matching_excerpt(value: &Value, matcher: &PreviewMatcher) -> Option<(String, std::ops::Range<usize>)> {
    match value {
        Value::Object(fields) => fields.values().find_map(|value| matching_excerpt(value, matcher)),
        Value::Array(items) => items.iter().find_map(|value| matching_excerpt(value, matcher)),
        _ => {
            let value = value.as_str().map(str::to_owned).unwrap_or_else(|| value_label(value));
            let max = value.floor_char_boundary(value.len().min(64 * 1024));
            let visible = &value[..max];
            let span = matcher.find(visible)?;
            let before = visible[..span.start].char_indices().rev().nth(12).map_or(0, |(at, _)| at);
            let after = visible[span.end..].char_indices().nth(60).map_or(visible.len(), |(at, _)| span.end + at);
            let mut excerpt = String::new();
            if before > 0 { excerpt.push('…'); }
            let prefix_bytes = excerpt.len();
            excerpt.push_str(&visible[before..after]);
            if after < visible.len() { excerpt.push('…'); }
            Some((excerpt, prefix_bytes + span.start - before..prefix_bytes + span.end - before))
        }
    }
}

/// Present the original source, parsed structure, and locator metadata.
pub fn detail_view(hit: &SearchHit, expanded: &HashSet<String>) -> DetailView {
    detail_view_with_query(hit, expanded, None)
}

pub fn detail_view_with_query(
    hit: &SearchHit,
    expanded: &HashSet<String>,
    spec: Option<&QuerySpec>,
) -> DetailView {
    let matcher = spec.and_then(|spec| {
        (!spec.full_text.trim().is_empty())
            .then(|| FtsQuery {
                case_sensitivity: if spec.case_sensitive {
                    CaseSensitivity::Sensitive
                } else {
                    CaseSensitivity::Insensitive
                },
                mode: if spec.regex {
                    FtsMode::Regex(RegexOptions::default())
                } else {
                    FtsMode::Literal
                },
                literal: spec.full_text.trim().to_owned(),
                pattern_parameter: None,
                fields: Vec::new(),
            })
            .and_then(|query| PreviewMatcher::new(&query).ok())
    });
    let mut filter_fields = Vec::new();
    if let Some(spec) = spec
        && let Ok(parsed) = parse_structured_filter(&spec.filter)
        && let Some(filter) = &parsed.filter
    {
        append_predicate_fields(filter, &mut filter_fields);
    }
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
            &filter_fields,
            matcher.as_ref(),
        ),
        None => detail.structured.push(DetailRow {
            label: "Payload".into(),
            value: "Logical data unavailable; inspect original source and diagnostics".into(),
            depth: 0,
            matched: false,
            filter_participant: false,
            fts_matched: false,
            value_highlights: Vec::new(),
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
            filter_participant: false,
            fts_matched: false,
            value_highlights: Vec::new(),
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
    filter_fields: &[String],
    matcher: Option<&PreviewMatcher>,
) {
    if rows.len() >= MAX_DETAIL_ROWS {
        return;
    }
    let Some((display, expandable)) = field_display(value) else {
        return;
    };
    let is_expanded = expandable && ((depth == 0 && value.is_object()) || expanded.contains(path));
    let display_path = path.strip_prefix("$.").unwrap_or(path);
    let participates = filter_fields
        .iter()
        .any(|field| field_at_or_below(field, display_path));
    let has_evidence = matched_fields
        .iter()
        .any(|field| field_at_or_below(field, display_path));
    let value_highlights = if has_evidence && !expandable {
        matcher
            .and_then(|matcher| matcher.find(&display))
            .into_iter()
            .collect()
    } else {
        Vec::new()
    };
    let fts_matched = !value_highlights.is_empty()
        || (expandable && !participates && matcher.is_some() && has_evidence);
    rows.push(DetailRow {
        label: path.into(),
        value: display,
        depth,
        matched: participates || has_evidence,
        filter_participant: participates,
        fts_matched,
        value_highlights,
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
                    filter_fields,
                    matcher,
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
                    filter_fields,
                    matcher,
                );
            }
        }
        _ => {}
    }
}

fn field_at_or_below(field: &str, path: &str) -> bool {
    field == path
        || field.starts_with(&format!("{path}."))
        || field.starts_with(&format!("{path}["))
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
    crate::backend::record_json::json_value(hit)
}

fn source_text(hit: &SearchHit) -> String {
    crate::backend::record_json::raw_text(hit)
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
