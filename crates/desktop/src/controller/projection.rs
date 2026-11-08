//! Bounded display projections; source bytes stay available for complete export.

use std::collections::{BTreeSet, HashSet};

use objekte_search_core::repository::{Payload, SearchHit};
use serde_json::Value;

use crate::backend::{QueryPage, QuerySpec};
use crate::ui::model::{
    DetailOrigin, DetailRow, DetailValueKind, DetailView, MAX_COLUMNS, MAX_DETAIL_ROWS, PageView,
    RelationRow, ResultRow, RowKey,
};

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
        vec!["@id".to_owned()]
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
        if key != "@collection" && key != "@file" && !columns.contains(&key) {
            columns.push(key);
        }
    }
    for metadata in ["@collection", "@file"] {
        if !columns.iter().any(|column| column == metadata) {
            columns.push(metadata.to_owned());
        }
    }
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
                    "@file" => hit.file_path.display().to_string(),
                    field => json
                        .as_ref()
                        .and_then(Value::as_object)
                        .and_then(|object| object.get(field))
                        .map(value_label)
                        .unwrap_or_default(),
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
    let available = columns.clone();
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
        clipped: false,
    }
    .without_hidden(hidden)
    .bounded();
    (view, available)
}


/// Add visible-column filtering without reducing the underlying result set.
trait VisibleColumns {
    fn without_hidden(self, hidden: &[String]) -> Self;
}


impl VisibleColumns for PageView {
    fn without_hidden(mut self, hidden: &[String]) -> Self {
        let indexes: Vec<_> = self
            .columns
            .iter()
            .enumerate()
            .filter_map(|(index, column)| (!hidden.contains(column)).then_some(index))
            .collect();
        self.columns = indexes
            .iter()
            .map(|index| self.columns[*index].clone())
            .collect();
        for row in &mut self.rows {
            row.cells = indexes
                .iter()
                .map(|index| row.cells[*index].clone())
                .collect();
        }
        self
    }
}


/// Present the original source, parsed structure, and locator metadata.
pub fn detail_view(hit: &SearchHit, expanded: &HashSet<String>) -> DetailView {
    let mut detail = DetailView {
        title: format!("{} / {}", hit.collection, hit.record_id),
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
    let expandable = matches!(value, Value::Array(_) | Value::Object(_));
    let is_expanded = depth == 0 || expanded.contains(path);
    rows.push(DetailRow {
        label: path.into(),
        value: value_label(value),
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
