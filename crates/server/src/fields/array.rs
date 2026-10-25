use super::*;
use structured_editor_core::draft::InputKind;
pub struct Array;
crate::templates::template!(Input, "fields/array.html");
#[derive(serde::Serialize)]
struct Input {
    items: Vec<Item>,
    add: String,
    base: String,
    id: String,
    order: String,
    reorder_label: String,
    empty_message: String,
}
#[derive(serde::Serialize)]
struct Item {
    id: String,
    handle: String,
    linked: bool,
    delete_url: String,
    delete_label: String,
    html: String,
    remove: String,
}
/// Only homogeneous scalar choices use a multi-select. Explicit raw editors and
/// tuple/complex arrays retain their normal controls.
pub fn choices_schema(field: &DraftField) -> Option<&serde_json::Value> {
    if field.kind != InputKind::Array {
        return None;
    }
    let schema = field
        .items
        .first()
        .map(|f| &f.schema)
        .unwrap_or(&field.schema["items"]);
    if matches!(schema["x-editor"].as_str(), Some("json" | "hidden"))
        || schema["x-presentation"]["editor"] == "raw"
        || schema["readOnly"] == true
        || schema["x-visible"] == false
    {
        return None;
    }
    let relation = schema
        .pointer("/x-relation/collection")
        .and_then(serde_json::Value::as_str)
        .is_some();
    let choices = schema["enum"]
        .as_array()
        .is_some_and(|v| v.iter().all(|v| !v.is_object() && !v.is_array()));
    (relation || choices).then_some(schema)
}

fn handle(base: &str, item: &DraftField, index: usize, total: usize) -> String {
    format!(
        "<button type=\"button\" class=\"icon-button array-handle\" data-array-handle data-up=\"{}\" data-down=\"{}\" aria-label=\"{}\" data-tooltip=\"{ui_text_0}\"><i class=\"ri-draggable\" aria-hidden=\"true\"></i></button>",
        if index > 0 {
            format!("{base}/up?item={}", item.id)
        } else {
            String::new()
        },
        if index + 1 < total {
            format!("{base}/down?item={}", item.id)
        } else {
            String::new()
        },
        crate::views::escape(&crate::i18n::message("Reorder item {number}", &[("number", &(index+1).to_string())])), ui_text_0 = crate::views::escape(&crate::i18n::t("Drag to reorder · ↑ / ↓")))
}

impl Control for Array {
    fn accepts(&self, field: &DraftField) -> bool {
        field.kind == InputKind::Array
    }
    fn render(
        &self,
        field: &DraftField,
        path: &str,
        context: &Context<'_>,
        registry: &Registry,
    ) -> Result<String> {
        let base = format!(
            "/edit-sessions/{}/fields/{}/actions",
            context.session, field.id
        );
        if let Some(schema) = choices_schema(field) {
            use crate::views::escape;
            let mut cells = crate::views::embedded::Cells::new(context.editor);
            let mut html = format!(
                "<div class=\"array-choices\" data-select-anchor data-local-choices=\"{}\" data-array=\"{base}\" aria-label=\"{}\" data-remove-label=\"{}\" data-reorder-label=\"{}\" data-max-items=\"{}\" data-limit-message=\"{}\">",
                field.id, escape(&crate::tree::label(field)),
                escape(&crate::i18n::t("Remove {name}")),
                escape(&crate::i18n::t("Reorder item {number}")),
                field.schema["maxItems"].as_u64().map(|n| n.to_string()).unwrap_or_default(),
                escape(&crate::i18n::t("The maximum number of selections has been reached"))
            );
            for (index, item) in field.items.iter().enumerate() {
                let value = crate::tree::snapshot(item);
                let label = cells.text(schema, &value);
                let label = if label.is_empty() {
                    "Empty value"
                } else {
                    &label
                };
                html += &format!(
                    "<div class=\"array-choice\" data-array-item=\"{}\" data-choice-value=\"{}\">{}<span class=\"array-choice-label\">{}</span><button class=\"icon-button small\" data-remove-choice formaction=\"{base}/remove?item={}\" aria-label=\"{}\" data-tooltip=\"{ui_text_0}\"><i class=\"ri-close-line\" aria-hidden=\"true\"></i></button>",
                    item.id,
                    escape(&value.to_string()),
                    handle(&base, item, index, field.items.len()),
                    escape(label),
                    item.id,
                    escape(&crate::i18n::message("Remove {name}", &[("name", &label)])), ui_text_0 = crate::views::escape(&crate::i18n::t("Remove from selection")));
                for issue in context
                    .issues
                    .iter()
                    .filter(|issue| issue.path == format!("{path}/{index}"))
                {
                    html += &format!("<p class=\"field-errors\">{}</p>", escape(&issue.message));
                }
                html += "</div>";
            }
            html += &format!(
                "<input class=\"choice-input\" id=\"input-{id}\" data-choice-input data-search-url=\"/edit-sessions/{session}/fields/{id}/choices\" role=\"combobox\" aria-autocomplete=\"list\" aria-expanded=\"false\" aria-controls=\"suggestions-{id}\" autocomplete=\"off\" placeholder=\"{placeholder}\" data-search-error=\"{error}\" data-no-matches=\"{no_matches}\"><button class=\"select-toggle\" aria-haspopup=\"dialog\" data-reference-picker formaction=\"{base}/multiple\"><i class=\"ri-arrow-down-s-line\" aria-hidden=\"true\"></i><span>{browse}</span></button></div>",
                id=field.id, session=context.session,
                placeholder=escape(&crate::i18n::t("Type to search…")),
                error=escape(&crate::i18n::t("Search could not be loaded. Try again.")),
                no_matches=escape(&crate::i18n::t("No matches.")),
                browse=escape(&crate::i18n::t("Browse"))
            );
            return Ok(html);
        }
        if crate::views::embedded::is_table(field) {
            use crate::views::{embedded, escape};
            let size = embedded::edit_page_size(context.editor, field);
            let total = field.items.len();
            let search = context.table_search.get(&field.id).cloned().unwrap_or_default();
            let matched = embedded::matching_edit_rows(context.editor, field, &search);
            let error = matched.as_ref().err().cloned();
            let rows = matched.unwrap_or_else(|_| (0..total).collect());
            let result_count = rows.len();
            let offset = context.table_offsets.get(&field.id).copied()
                .map(|offset| offset / size * size).unwrap_or(0)
                .min(result_count.saturating_sub(1) / size * size);
            let end = (offset + size).min(result_count);
            let columns = embedded::columns(field, &Default::default(), "");
            let mut cells = embedded::Cells::new(context.editor);
            let count = crate::i18n::message("{start}–{end} of {total}", &[("start", &if result_count == 0 { "0".into() } else { (offset+1).to_string() }), ("end", &end.to_string()), ("total", &result_count.to_string())]);
            let next = rows.get(end).and_then(|index| field.items.get(*index)).map(|item| item.id.as_str()).unwrap_or("");
            let mut html = format!(
                "<div class=\"embedded-editor surface\" id=\"{}\" data-array=\"{base}\" data-structure-array=\"{}\" data-array-order=\"{}\" data-array-offset=\"{offset}\" data-array-next=\"{next}\" data-array-filtered=\"{}\" data-array-results=\"{result_count}\" data-reorder-label=\"{}\" data-empty-message=\"{}\"><div class=\"embedded-edit-toolbar child-heading\"><h2><i class=\"{}\" aria-hidden=\"true\"></i>{} <span class=\"badge\">{total}</span></h2><button class=\"small\" formaction=\"{base}/add\"><i class=\"ri-add-line\" aria-hidden=\"true\"></i>{}</button></div>",
                embedded::table_anchor(path),
                field.id,
                escape(&serde_json::to_string(&field.items.iter().map(|item| &item.id).collect::<Vec<_>>()).unwrap()),
                !search.query.is_empty(),
                escape(&crate::i18n::t("Reorder item {number}")),
                escape(&crate::i18n::t("No items")),
                embedded::table_icon(context.editor, field),
                escape(&crate::tree::label(field)), escape(&crate::i18n::t("Add"))
            );
            if total == 0 {
                return Ok(html + "</div>");
            }
            html += &format!(
                "<div class=\"list-toolbar embedded-edit-search\"><div class=\"search-form\"><div class=\"search-input has-options\"><i class=\"ri-search-line\" aria-hidden=\"true\"></i><label class=\"sr-only\" for=\"table-query-{}\">{}</label><input id=\"table-query-{}\" type=\"search\" name=\"table-query.{}\" value=\"{}\" placeholder=\"{}\" aria-invalid=\"{}\"><span class=\"search-options\"><label class=\"search-toggle\" title=\"{}\"><input type=\"checkbox\" name=\"table-case.{}\" value=\"1\" {}><span>Aa</span></label><label class=\"search-toggle\" title=\"{}\"><input type=\"checkbox\" name=\"table-regex.{}\" value=\"1\" {}><span>.*</span></label></span></div><button class=\"small\" formaction=\"{base}/table-search\">{}</button>{}</div></div>",
                field.id, escape(&crate::i18n::t("Search")), field.id, field.id,
                escape(&search.query), escape(&crate::i18n::t("Search this table…")),
                error.is_some(), escape(&crate::i18n::t("Match case")), field.id,
                if search.case_sensitive { "checked" } else { "" },
                escape(&crate::i18n::t("Use a regular expression")), field.id,
                if search.regex { "checked" } else { "" }, escape(&crate::i18n::t("Search")),
                if search.query.is_empty() { String::new() } else { format!("<button class=\"small quiet\" formaction=\"{base}/table-search-clear\">{}</button>", escape(&crate::i18n::t("Show All"))) }
            );
            if let Some(error) = &error {
                html += &format!("<p class=\"search-pattern-error\" role=\"status\">{}</p>", escape(error));
            }
            if !search.query.is_empty() && error.is_none() {
                html += &format!("<p class=\"embedded-status\">{}</p>", escape(&crate::i18n::message("{shown} of {total} rows", &[("shown", &result_count.to_string()), ("total", &total.to_string())])));
            }
            html += &format!(
                "<div class=\"table-scroll\"><table class=\"data-table embedded-table\" aria-label=\"{}\"><thead><tr><th class=\"handle-cell\"><span class=\"sr-only\">{ui_text_0}</span></th><th class=\"position-cell\">#</th>",
                escape(&crate::tree::label(field)), ui_text_0 = crate::views::escape(&crate::i18n::t("Reorder")));
            for (_, label) in &columns.selected {
                html += &format!("<th>{}</th>", escape(label));
            }
            html += &format!("<th class=\"row-actions\">{ui_text_0}</th></tr></thead><tbody>", ui_text_0 = crate::views::escape(&crate::i18n::t("Actions")));
            for &index in rows.iter().skip(offset).take(size) {
                let item = &field.items[index];
                let value = crate::tree::snapshot(item);
                html += &format!(
                    "<tr data-array-item=\"{}\"><td class=\"handle-cell\">{}</td><td class=\"position-cell\" data-label=\"Position\">{}</td>",
                    item.id,
                    handle(&base, item, index, total),
                    index + 1
                );
                for (key, label) in &columns.selected {
                    let cell = cells.html(&item.schema["properties"][key], &value[key]);
                    html += &format!(
                        "<td data-label=\"{}\"><div class=\"embedded-cell\">{}</div></td>",
                        escape(label),
                        if cell.is_empty() {
                            "<span class=\"muted\">—</span>"
                        } else {
                            &cell
                        }
                    );
                }
                html += &format!(
                    "<td class=\"row-actions\"><div class=\"row-action-buttons\"><button class=\"icon-button small\" formaction=\"/edit-sessions/{}/fields/{}/actions/browse\" aria-label=\"{}\" data-tooltip=\"{ui_text_0}\"><i class=\"ri-edit-line\" aria-hidden=\"true\"></i></button>",
                    context.session,
                    item.id,
                    escape(&crate::i18n::message("Edit row {number}", &[("number", &(index+1).to_string())])), ui_text_0 = crate::views::escape(&crate::i18n::t("Edit")));
                html += &format!(
                    "<button class=\"icon-button small\" data-remove-array-item formaction=\"{base}/remove?item={}\" aria-label=\"{}\" data-tooltip=\"{ui_text_0}\"><i class=\"ri-delete-bin-line\" aria-hidden=\"true\"></i></button>",
                    item.id,
                    escape(&crate::i18n::message("Delete row {number}", &[("number", &(index+1).to_string())])), ui_text_0 = crate::views::escape(&crate::i18n::t("Delete item and its nested contents when saved")));
                html += "</div></td></tr>";
            }
            if rows.is_empty() {
                html += &format!("<tr><td class=\"embedded-empty\" colspan=\"{}\">{}</td></tr>", columns.selected.len()+3, escape(&crate::i18n::t("No rows match this search.")));
            }
            html += "</tbody></table></div>";
            if result_count > size {
                html += &format!("<nav class=\"table-pagination\" aria-label=\"{ui_text_0}\">", ui_text_0 = crate::views::escape(&crate::i18n::t("Table pages")));
                html += &format!(
                        "<button class=\"small\" formaction=\"{base}/table-page?item={}\" {}>{ui_text_0}</button><span data-table-range=\"{}\">{count}</span><button class=\"small\" formaction=\"{base}/table-page?item={end}\" {}>{ui_text_1}</button>",
                        offset.saturating_sub(size),
                        if offset == 0 { "disabled" } else { "" },
                        escape(&crate::i18n::t("{start}–{end} of {total}")),
                        if end == result_count { "disabled" } else { "" }, ui_text_0 = crate::views::escape(&crate::i18n::t("Previous")), ui_text_1 = crate::views::escape(&crate::i18n::t("Next")));
                html += "</nav>";
            }
            html += "</div>";
            return Ok(html);
        }
        let items = field
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let target = item
                    .schema
                    .pointer("/x-relation/collection")
                    .and_then(serde_json::Value::as_str);
                let linked = target.is_some();
                let target_id = if item.kind == InputKind::Text {
                    item.raw.clone()
                } else {
                    serde_json::from_str::<serde_json::Value>(&item.raw)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .unwrap_or_default()
                };
                let deletable = target.filter(|target| {
                    linked
                        && context.editor.allows(target, "read")
                        && context.editor.allows(target, "delete")
                        && context.editor.get(target, &target_id).is_ok()
                });
                Ok(Item {
                    id: item.id.clone(),
                    handle: handle(&base, item, index, field.items.len()),
                    linked,
                    delete_url: deletable
                        .map(|target| {
                            format!("{}/delete", crate::views::record_url(target, &target_id))
                        })
                        .unwrap_or_default(),
                    delete_label: deletable
                        .map(|target| crate::views::record_kind(context.editor, target))
                        .transpose()?
                        .map(|kind| format!("Delete {kind}…"))
                        .unwrap_or_default(),
                    html: registry.render(item, &format!("{path}/{index}"), context, true)?,
                    remove: format!("{base}/remove?item={}", item.id),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Input {
            items,
            add: format!("{base}/add"),
            base,
            id: field.id.clone(),
            order: serde_json::to_string(&field.items.iter().map(|item| &item.id).collect::<Vec<_>>()).unwrap(),
            reorder_label: crate::i18n::t("Reorder item {number}"),
            empty_message: crate::i18n::t("No items"),
        }
        .render()?)
    }
}
