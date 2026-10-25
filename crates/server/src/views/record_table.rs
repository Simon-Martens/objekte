//! The same record rows, sorting and pagination in collection and related tables.
use super::{
    list::highlighted,
    table::{TableColumns, query_url},
    *,
};
use std::collections::HashMap;
use structured_editor_core::{ListPage, ListRequest};

pub struct RecordTable<'a> {
    pub base: &'a str,
    pub params: &'a HashMap<String, String>,
    pub columns: &'a TableColumns,
    pub request: &'a ListRequest,
    pub page: &'a ListPage,
    pub pattern_error: Option<&'a str>,
    pub navigation: &'a str,
    pub placement: Option<(&'a str, &'a str, &'a str, &'a str)>,
}
impl RecordTable<'_> {
    pub fn render(&self, editor: &Editor, name: &str, actor: &Actor) -> Result<String> {
        let c = editor.config().collection(name)?;
        let props = c.editor_schema["properties"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let columns = &self.columns.selected;
        let available = &self.columns.available;
        let request = self.request;
        let page = self.page;
        let params = self.params;
        let base = self.base;
        let navigation = self.navigation;
        let sort = request.sort.as_str();
        let direction = if request.descending { "desc" } else { "asc" };
        let q = &request.query;
        let offset = request.offset;
        let limit = request.limit;
        let pattern_error = self.pattern_error;
        let mut body = String::new();
        let mut page_controls = String::new();
        for (enabled, next_offset, label, icon) in [
            (
                offset > 0,
                offset.saturating_sub(limit),
                "Previous page",
                "ri-arrow-left-s-line",
            ),
            (
                offset.saturating_add(limit) < page.matched,
                offset.saturating_add(limit),
                "Next page",
                "ri-arrow-right-s-line",
            ),
        ] {
            if enabled {
                let mut next = params.clone();
                next.insert("offset".into(), next_offset.to_string());
                page_controls += &format!(
                    "<a class=\"icon-button\" {navigation} data-reference-page href=\"{}\" aria-label=\"{label}\"><i class=\"{icon}\" aria-hidden=\"true\"></i></a>",
                    escape(&query_url(&base, &next))
                );
            } else {
                page_controls += &format!(
                    "<button type=\"button\" class=\"icon-button\" disabled aria-label=\"{label}\"><i class=\"{icon}\" aria-hidden=\"true\"></i></button>"
                );
            }
        }
        body += &format!(
            "<div class=\"result-summary\"><span><strong>{}</strong> {} {}</span><nav class=\"summary-pagination\" aria-label=\"List pagination\">{page_controls}<span class=\"mono\">{}–{}</span></nav></div><div class=\"table-scroll\"><table class=\"data-table\"><thead><tr>",
            page.matched,
            if q.is_empty() && request.filters.values().all(|v| v.is_empty()) {
                if page.matched == 1 {
                    escape(&super::record_kind(editor, name)?.to_lowercase())
                } else {
                    escape(&crate::i18n::text(&c.ui,"label", &c.label).to_lowercase())
                }
            } else {
                if page.matched == 1 {
                    "match"
                } else {
                    "matches"
                }
                .into()
            },
            if page.matched == page.total {
                String::new()
            } else {
                format!(
                    "<span class=\"muted\">of {} {}</span>",
                    page.total,
                    escape(&crate::i18n::text(&c.ui,"label", &c.label).to_lowercase())
                )
            },
            if page.rows.is_empty() { 0 } else { offset + 1 },
            (offset + page.rows.len()).min(page.matched)
        );
        for (key, label) in columns {
            let mut p = params.clone();
            p.insert("sort".into(), key.clone());
            p.insert(
                "direction".into(),
                if sort == key && direction == "asc" {
                    "desc"
                } else {
                    "asc"
                }
                .into(),
            );
            p.remove("offset");
            body += &format!(
                "<th scope=\"col\" aria-sort=\"{}\"><a {navigation} href=\"{}\">{}<i class=\"{}\" aria-hidden=\"true\"></i></a></th>",
                if sort == key {
                    if direction == "desc" {
                        "descending"
                    } else {
                        "ascending"
                    }
                } else {
                    "none"
                },
                escape(&query_url(&base, &p)),
                escape(label),
                if sort == key {
                    if direction == "desc" {
                        "ri-arrow-down-line"
                    } else {
                        "ri-arrow-up-line"
                    }
                } else {
                    "ri-expand-up-down-line"
                }
            );
        }
        body += &format!("<th scope=\"col\"><span class=\"sr-only\">{ui_text_0}</span></th></tr></thead><tbody>", ui_text_0 = crate::views::escape(&crate::i18n::t("Actions")));
        for row in &page.rows {
            let link = record_url(name, &row.id);
            body += "<tr>";
            for (key, label) in columns {
                let text = match key.as_str() {
                    "_id" => Some(&row.id),
                    "_label" => Some(&row.label),
                    _ => row.display.get(key),
                };
                let text = text
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| {
                        highlighted(
                            s,
                            row.highlights.get(key).map(Vec::as_slice).unwrap_or(&[]),
                            true,
                        )
                    })
                    .unwrap_or_else(|| "<span class=\"muted\">—</span>".into());
                let numeric = matches!(
                    props.get(key).and_then(|s| s["type"].as_str()),
                    Some("integer" | "number" | "boolean")
                );
                let primary = key == "_label" || key == &c.display;
                let cell_link = if props
                    .get(key)
                    .is_some_and(|s| s["type"] == "array" && s["items"]["type"] == "object")
                {
                    format!(
                        "{link}?node={}",
                        encode(&structured_editor_core::draft::child_path("", key))
                    )
                } else {
                    link.clone()
                };
                body += &format!(
                    "<td data-label=\"{}\" class=\"{}\"><a data-nav class=\"{}\" href=\"{cell_link}\">{text}</a></td>",
                    escape(label),
                    if key == "_id" {
                        "mono id-cell"
                    } else if numeric {
                        "numeric-cell mono"
                    } else {
                        ""
                    },
                    if primary {
                        "table-record-link"
                    } else {
                        "table-cell-link"
                    }
                );
            }
            body += &format!(
                "<td class=\"row-actions\"><div class=\"row-action-buttons\"><a class=\"icon-button\" data-nav href=\"{link}\" aria-label=\"{ui_text_0}\"><i class=\"ri-arrow-right-up-line\" aria-hidden=\"true\"></i></a>", ui_text_0 = crate::views::escape(&crate::i18n::t("Open record")));
            if actor.editable() && editor.allows(name, "update") {
                body += &format!(
                    "<a class=\"icon-button\" data-nav href=\"{link}/edit\" aria-label=\"{ui_text_0}\"><i class=\"ri-edit-line\" aria-hidden=\"true\"></i></a>", ui_text_0 = crate::views::escape(&crate::i18n::t("Edit")));
            }
            body += &crate::hierarchy::record_actions(
                editor,
                actor,
                name,
                &row.id,
                &row.label,
                self.placement,
            )?;
            body += "</div></td></tr>";
            let extra_matches: Vec<_> = row
                .matches
                .iter()
                .filter(|m| !columns.iter().any(|(key, _)| key == &m.field))
                .collect();
            if !q.is_empty() && !extra_matches.is_empty() {
                body += &format!(
                    "<tr class=\"match-row\"><td colspan=\"{}\"><div class=\"match-excerpts\">",
                    columns.len() + 1
                );
                for m in extra_matches.iter().take(4) {
                    let label = available
                        .iter()
                        .find(|(k, _)| k == &m.field)
                        .map(|(_, l)| l.as_str())
                        .unwrap_or(&m.field);
                    body += &format!(
                        "<p><span class=\"match-field\">{}</span>{}</p>",
                        escape(label),
                        highlighted(&m.text, &m.ranges, true)
                    );
                }
                body += "</div></td></tr>";
            }
        }
        body += "</tbody></table></div>";
        if page.rows.is_empty() && pattern_error.is_none() {
            body += "<div class=\"empty-state\"><i class=\"ri-search-line\" aria-hidden=\"true\"></i><h2>No matching records</h2><p>Try a broader search or clear a filter.</p></div>";
        }
        body += "<footer class=\"list-footer\"><div class=\"actions\">";
        for (enabled, n, label) in [
            (offset > 0, offset.saturating_sub(limit), "Previous"),
            (
                offset.saturating_add(limit) < page.matched,
                offset.saturating_add(limit),
                "Next",
            ),
        ] {
            if enabled {
                let mut p = params.clone();
                p.insert("offset".into(), n.to_string());
                body += &format!(
                    "<a class=\"button\" {navigation} data-reference-page href=\"{}\">{label}</a>",
                    escape(&query_url(&base, &p))
                );
            } else {
                body += &format!("<button type=\"button\" disabled>{label}</button>");
            }
        }
        body += "</div></footer>";
        Ok(body)
    }
}
