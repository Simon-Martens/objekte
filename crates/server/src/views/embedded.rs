//! Tables for objects stored inside a record. Row URLs always use the original
//! array index, even when the displayed rows are filtered, sorted or paged.
use super::{
    encode, escape, record_url, search,
    table::{self, TableColumns},
};
use serde_json::{Value, json};
use std::collections::HashMap;
use structured_editor_core::{
    Editor, SearchMatcher,
    draft::{DraftField, InputKind, child_path},
};

pub fn is_table(field: &DraftField) -> bool {
    field.kind == InputKind::Array
        && field.schema["x-presentation"]["table"] != false
        && (field.schema["items"]["type"] == "object"
            || !field.items.is_empty() && field.items.iter().all(|f| f.kind == InputKind::Object))
}

pub fn table_anchor(path: &str) -> String {
    let mut id = String::from("array-");
    for byte in path.bytes() {
        use std::fmt::Write;
        let _ = write!(&mut id, "{byte:02x}");
    }
    id
}

/// Return original item indices so a filtered edit page keeps stable row IDs.
pub fn matching_edit_rows(
    editor: &Editor,
    field: &DraftField,
    search: &crate::sessions::TableSearch,
) -> Result<Vec<usize>, String> {
    let matcher = SearchMatcher::new(
        &search.query,
        structured_editor_core::SearchOptions {
            regex: search.regex,
            case_sensitive: search.case_sensitive,
        },
    )?;
    if !matcher.is_active() {
        return Ok((0..field.items.len()).collect());
    }
    let mut cells = Cells::new(editor);
    Ok(field.items.iter().enumerate().filter_map(|(index, item)| {
        let value = crate::tree::snapshot(item);
        item.schema["properties"].as_object().is_some_and(|props| {
            props.iter()
                .filter(|(_, schema)| schema["x-visible"] != false && schema["x-editor"] != "hidden")
                .any(|(key, schema)| matcher.is_match(&cells.text(schema, &value[key])))
        }).then_some(index)
    }).collect())
}

/// A table can set x-presentation.table.icon in its field UI options.
/// Otherwise a single direct relation in its row adopts that collection icon.
pub fn table_icon(editor: &Editor, field: &DraftField) -> String {
    let explicit = field.schema.pointer("/x-presentation/table/icon").and_then(Value::as_str);
    if explicit.is_some() {
        return super::icon_class(explicit, "ri-table-line");
    }
    let row = field.items.first().map(|item| &item.schema).unwrap_or(&field.schema["items"]);
    let targets: std::collections::BTreeSet<_> = row["properties"].as_object()
        .into_iter().flat_map(|properties| properties.values())
        .filter_map(|schema| schema.pointer("/x-relation/collection").and_then(Value::as_str))
        .collect();
    if targets.len() == 1 {
        let target = targets.iter().next().unwrap();
        let icon = editor.config().collection(target).ok()
            .and_then(|collection| collection.ui["icon"].as_str())
            .or_else(|| editor.config().media_collections.get(*target).and_then(|collection| collection.ui["icon"].as_str()))
            .or_else(|| editor.config().ui["collections"][*target]["icon"].as_str());
        return super::icon_class(icon, "ri-table-line");
    }
    "ri-table-line".into()
}

pub struct Cells<'a> {
    editor: &'a Editor,
    labels: HashMap<(String, String), String>,
}
impl<'a> Cells<'a> {
    pub fn new(editor: &'a Editor) -> Self {
        Self {
            editor,
            labels: HashMap::new(),
        }
    }
    fn relation(&mut self, schema: &Value, value: &Value) -> Option<(String, String)> {
        let target = schema.pointer("/x-relation/collection")?.as_str()?;
        let id = value.as_str()?;
        if id.is_empty() {
            return None;
        }
        let label = self
            .labels
            .entry((target.into(), id.into()))
            .or_insert_with(|| {
                self.editor
                    .get(target, id)
                    .ok()
                    .and_then(|doc| self.editor.label(target, &doc.value, id, "reference").ok())
                    .unwrap_or_else(|| id.into())
            });
        Some((record_url(target, id), label.clone()))
    }
    pub fn text(&mut self, schema: &Value, value: &Value) -> String {
        if let Some((_, label)) = self.relation(schema, value) {
            return label;
        }
        if let Some(label) = super::preview::option_label(schema, value) {
            return label.into();
        }
        match value {
            Value::Null => String::new(),
            Value::String(s) => {
                if is_markdown(schema) {
                    structured_editor_core::plain_markdown(s)
                } else {
                    s.clone()
                }
            }
            Value::Bool(b) => if *b { "Yes" } else { "No" }.into(),
            Value::Number(n) => n.to_string(),
            Value::Array(items) => items
                .iter()
                .map(|v| self.text(&schema["items"], v))
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(", "),
            Value::Object(obj) => {
                for key in display_fields(schema) {
                    if let Some(v) = obj.get(&key) {
                        let text = self.text(&schema["properties"][&key], v);
                        if !text.is_empty() {
                            return text;
                        }
                    }
                }
                obj.iter()
                    .filter(|(key, _)| {
                        let s = &schema["properties"][*key];
                        s["readOnly"] != true
                            && s["x-visible"] != false
                            && s["x-editor"] != "hidden"
                    })
                    .map(|(key, v)| self.text(&schema["properties"][key], v))
                    .filter(|s| !s.is_empty())
                    .take(3)
                    .collect::<Vec<_>>()
                    .join(" · ")
            }
        }
    }
    pub fn html(&mut self, schema: &Value, value: &Value) -> String {
        self.html_with_search(schema, value, None)
    }
    fn html_with_search(
        &mut self,
        schema: &Value,
        value: &Value,
        matcher: Option<&SearchMatcher>,
    ) -> String {
        let highlighted = |text: &str| match matcher {
            Some(matcher) => super::list::highlighted(text, &matcher.ranges(text), false),
            None => escape(text),
        };
        if let Some((url, label)) = self.relation(schema, value) {
            return format!(
                "<a data-nav href=\"{}\">{}</a>",
                escape(&url),
                highlighted(&label)
            );
        }
        if let Some(items) = value.as_array() {
            return items
                .iter()
                .map(|v| self.html_with_search(&schema["items"], v, matcher))
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("<span class=\"cell-separator\">, </span>");
        }
        if let Some(text) = value.as_str()
            && is_markdown(schema)
        {
            if matcher.is_some_and(SearchMatcher::is_active) {
                return highlighted(&structured_editor_core::plain_markdown(text));
            }
            return super::markdown(text);
        }
        if let Some(obj) = value.as_object() {
            for key in display_fields(schema) {
                if let Some(v) = obj.get(&key)
                    && !self.text(&schema["properties"][&key], v).is_empty()
                {
                    return self.html_with_search(&schema["properties"][&key], v, matcher);
                }
            }
        }
        highlighted(&self.text(schema, value))
    }
}

/// Label candidates use presentation hints and schema order, never domain field names.
pub fn display_fields(schema: &Value) -> Vec<String> {
    let Some(properties) = schema["properties"].as_object() else {
        return Vec::new();
    };
    let order = schema
        .pointer("/x-presentation/fieldOrder")
        .or_else(|| schema.pointer("/x-collection/fieldOrder"))
        .and_then(Value::as_array);
    let mut keys: Vec<_> = properties
        .iter()
        .filter(|(_, s)| {
            s["readOnly"] != true && s["x-visible"] != false && s["x-editor"] != "hidden"
        })
        .map(|(k, _)| k.clone())
        .collect();
    keys.sort_by_key(|key| {
        (
            properties[key]["x-presentation"]["primary"] != true,
            order
                .and_then(|order| order.iter().position(|v| v.as_str() == Some(key)))
                .unwrap_or(usize::MAX),
            matches!(properties[key]["type"].as_str(), Some("array" | "object")),
        )
    });
    keys
}

fn is_markdown(schema: &Value) -> bool {
    ["x-content", "x-editor"]
        .iter()
        .any(|key| matches!(schema[key].as_str(), Some("markdown" | "inline-markdown")))
}

pub fn columns(field: &DraftField, params: &HashMap<String, String>, prefix: &str) -> TableColumns {
    let schema = field
        .items
        .first()
        .map(|f| &f.schema)
        .unwrap_or(&field.schema["items"]);
    let defaults: Vec<&str> = schema["properties"]
        .as_object()
        .map(|p| {
            p.iter()
                .filter(|(_, s)| s["readOnly"] != true && s["x-visible"] != false)
                .map(|(k, _)| k.as_str())
                .take(4)
                .collect()
        })
        .unwrap_or_default();
    let mut cols = TableColumns::new(
        schema,
        &json!({"list":field.schema["x-presentation"]["table"]}),
        params,
        prefix,
        &defaults,
    );
    cols.available.retain(|(k, _)| k != "_label" && k != "_id");
    cols.selected.retain(|(k, _)| k != "_label" && k != "_id");
    if cols.selected.is_empty()
        && let Some(first) = cols.available.first()
    {
        cols.selected.push(first.clone());
    }
    cols
}

pub fn render(
    editor: &Editor,
    field: &DraftField,
    value: &Value,
    base: &str,
    path: &str,
    params: &HashMap<String, String>,
    editable: bool,
) -> String {
    let Some(items) = value.as_array() else {
        return String::new();
    };
    let title = crate::tree::label(field);
    let scope = table_anchor(path);
    let scoped_base = format!("{base}#{scope}");
    let scoped_url = |params: &HashMap<String, String>| {
        let mut params = params.clone();
        params.insert("table_anchor".into(), scope.clone());
        format!("{}#{scope}", table::query_url(base, &params))
    };
    let prefix = format!("array.{path}.");
    let key = |suffix: &str| format!("{prefix}{suffix}");
    let cols = columns(field, params, &prefix);
    let schema = field
        .items
        .first()
        .map(|f| &f.schema)
        .unwrap_or(&field.schema["items"]);
    let mut cells = Cells::new(editor);
    let search = params.get(&key("q")).map(String::as_str).unwrap_or("");
    let matcher = SearchMatcher::new(search, search::options(params, &prefix));
    let pattern_error = matcher.as_ref().err().map(String::as_str);
    let matcher = matcher.as_ref().ok();
    let mut rows: Vec<usize> = (0..items.len())
        .filter(|&index| {
            matcher.is_some_and(|matcher| {
                !matcher.is_active()
                    || schema["properties"].as_object().is_some_and(|props| {
                        props
                            .iter()
                            .filter(|(_, s)| s["x-visible"] != false && s["x-editor"] != "hidden")
                            .any(|(k, s)| matcher.is_match(&cells.text(s, &items[index][k])))
                    })
            })
        })
        .collect();
    let sort = params.get(&key("sort")).map(String::as_str).unwrap_or("");
    let desc = params.get(&key("direction")).is_some_and(|s| s == "desc");
    if cols.available.iter().any(|(k, _)| k == sort) {
        rows.sort_by_cached_key(|&index| {
            cells
                .text(&schema["properties"][sort], &items[index][sort])
                .to_lowercase()
        });
        // Numbers must sort by their numeric value, not their displayed text.
        if matches!(
            schema["properties"][sort]["type"].as_str(),
            Some("integer" | "number")
        ) {
            rows.sort_by(|&a, &b| {
                items[a][sort]
                    .as_f64()
                    .unwrap_or(f64::INFINITY)
                    .total_cmp(&items[b][sort].as_f64().unwrap_or(f64::INFINITY))
            });
        }
        if desc {
            rows.reverse();
        }
    }
    let size = edit_page_size(editor, field);
    let offset = params
        .get(&key("offset"))
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0)
        .min(rows.len().saturating_sub(1) / size * size);
    let mut reset = params.clone();
    reset.remove(&key("offset"));
    let mut html = format!(
        "<section class=\"surface child-list embedded-panel\" id=\"{scope}\" aria-label=\"{}\"><div class=\"child-heading\"><div><h2><i class=\"{}\" aria-hidden=\"true\"></i>{}<span class=\"badge mono\">{}</span></h2></div><div class=\"actions\">",
        escape(&title),
        table_icon(editor, field),
        escape(&title),
        items.len()
    );
    if editable {
        html += &format!(
            "<a class=\"icon-button\" href=\"{}/edit#{}\" aria-label=\"Edit {}\" data-tooltip=\"{ui_text_0}\"><i class=\"ri-edit-line\" aria-hidden=\"true\"></i></a>",
            escape(base),
            scope,
            escape(&title), ui_text_0 = crate::views::escape(&crate::i18n::t("Edit")));
    }
    html += &cols.picker_with_submit(&scope, &scoped_base, &reset, &prefix, "", Some(&scope));
    html += "</div></div>";
    {
        let show_all = if search.is_empty() {
            String::new()
        } else {
            let mut clear = params.clone();
            for suffix in ["q", "regex", "case_sensitive", "offset"] {
                clear.remove(&key(suffix));
            }
            format!(
                "<a class=\"button quiet\" href=\"{}\">{ui_text_0}</a>",
                escape(&scoped_url(&clear)), ui_text_0 = crate::views::escape(&crate::i18n::t("Show All")))
        };
        html += &format!(
            "<div class=\"list-toolbar\">{}</div>",
            search::form_with_submit(
                &scoped_base,
                params,
                &prefix,
                &scope,
                &crate::i18n::t("Search this table…"),
                pattern_error,
                &format!("<input type=\"hidden\" name=\"table_anchor\" value=\"{scope}\">{show_all}"),
                ""
            )
        );
    }
    html += &search::validation(&scope, pattern_error);
    if (!search.is_empty() || !sort.is_empty()) && pattern_error.is_none() {
        let mut clear = params.clone();
        for suffix in [
            "q",
            "regex",
            "case_sensitive",
            "offset",
            "sort",
            "direction",
        ] {
            clear.remove(&key(suffix));
        }
        html += &format!(
            "<div class=\"embedded-status\"><span>{}{}</span><a href=\"{}\">{ui_text_0}</a></div>",
            escape(&crate::i18n::message("{shown} of {total} rows", &[("shown", &rows.len().to_string()), ("total", &items.len().to_string())])),
            if sort.is_empty() {
                format!(" · {}", crate::i18n::t("stored order"))
            } else {
                format!(" · {}", crate::i18n::t("sorted view"))
            },
            escape(&scoped_url(&clear)), ui_text_0 = crate::views::escape(&crate::i18n::t("Reset view")));
    }
    html += &format!(
        "<div class=\"table-scroll\"><table class=\"data-table child-table embedded-table\"><caption class=\"sr-only\">{}</caption><thead><tr><th class=\"position-cell\" scope=\"col\">#</th>",
        escape(&title)
    );
    for (name, label) in &cols.selected {
        let mut next = reset.clone();
        next.insert(key("sort"), name.clone());
        next.insert(
            key("direction"),
            if sort == name && !desc { "desc" } else { "asc" }.into(),
        );
        html += &format!(
            "<th scope=\"col\" aria-sort=\"{}\"><a href=\"{}\">{}<i class=\"ri-arrow-up-down-line\" aria-hidden=\"true\"></i></a></th>",
            if sort == name {
                if desc { "descending" } else { "ascending" }
            } else {
                "none"
            },
            escape(&scoped_url(&next)),
            escape(label)
        );
    }
    html += &format!("<th scope=\"col\" class=\"row-actions\"><span class=\"sr-only\">{ui_text_0}</span></th></tr></thead><tbody>", ui_text_0 = crate::views::escape(&crate::i18n::t("Actions")));
    for &index in rows.iter().skip(offset).take(size) {
        let row_path = child_path(path, &index.to_string());
        let mut destination = params.clone();
        destination.insert("node".into(), row_path.clone());
        let url = table::query_url(base, &destination);
        let row_title = cells.text(schema, &items[index]);
        html += &format!(
            "<tr><td class=\"position-cell\" data-label=\"Position\">{}</td>",
            index + 1
        );
        for (column, label) in &cols.selected {
            let text = cells.html_with_search(
                &schema["properties"][column],
                &items[index][column],
                matcher,
            );
            let text = if text.is_empty() {
                "<span class=\"muted\">—</span>".into()
            } else {
                text
            };
            let linkable = |column: &str| {
                let property = &schema["properties"][column];
                property.pointer("/x-relation/collection").is_none()
                    && !matches!(property["type"].as_str(), Some("array" | "object"))
            };
            let primary_column = cols
                .selected
                .iter()
                .find(|(k, _)| {
                    linkable(k) && schema["properties"][k]["x-presentation"]["primary"] == true
                })
                .or_else(|| cols.selected.iter().find(|(k, _)| linkable(k)));
            let primary = primary_column.is_some_and(|(k, _)| k == column);
            html += &format!(
                "<td data-label=\"{}\">{}</td>",
                escape(label),
                if primary {
                    format!(
                        "<a data-nav class=\"record-link table-record-link\" href=\"{}\">{text}</a>",
                        escape(&url)
                    )
                } else {
                    text
                }
            );
        }
        html += &format!(
            "<td class=\"row-actions\"><div class=\"row-action-buttons\"><a class=\"icon-button\" data-nav href=\"{}\" aria-label=\"{}\" data-tooltip=\"{}\"><i class=\"ri-arrow-right-line\" aria-hidden=\"true\"></i></a>",
            escape(&url),
            escape(&crate::i18n::message("Open {name}", &[("name", &row_title)])),
            escape(&crate::i18n::t("Open row"))
        );
        if editable {
            html += &format!(
                "<a class=\"icon-button\" data-nav href=\"{}/edit?node={}\" aria-label=\"{}\" data-tooltip=\"{}\"><i class=\"ri-edit-line\" aria-hidden=\"true\"></i></a>",
                escape(base),
                encode(&row_path),
                escape(&crate::i18n::message("Edit {name}", &[("name", &row_title)])),
                escape(&crate::i18n::t("Edit row"))
            );
        }
        html += "</div></td></tr>";
        if let Some(matcher) = matcher.filter(|m| m.is_active()) {
            let excerpts: String = cols
                .available
                .iter()
                .filter(|(key, _)| !cols.selected.iter().any(|(selected, _)| selected == key))
                .filter_map(|(key, label)| {
                    let text = cells.text(&schema["properties"][key], &items[index][key]);
                    let ranges = matcher.ranges(&text);
                    (!ranges.is_empty()).then(|| {
                        format!(
                            "<p><span class=\"match-field\">{}</span>{}</p>",
                            escape(label),
                            super::list::highlighted(&text, &ranges, true)
                        )
                    })
                })
                .take(4)
                .collect();
            if !excerpts.is_empty() {
                html += &format!(
                    "<tr class=\"match-row\"><td colspan=\"{}\"><div class=\"match-excerpts\">{excerpts}</div></td></tr>",
                    cols.selected.len() + 2
                );
            }
        }
    }
    if rows.is_empty() && pattern_error.is_none() {
        html += &format!(
            "<tr><td colspan=\"{}\" class=\"embedded-empty\">{}</td></tr>",
            cols.selected.len() + 2,
            escape(&crate::i18n::t(if items.is_empty() {
                "No rows yet."
            } else {
                "No matching rows. Try another search."
            }))
        );
    }
    html += "</tbody></table></div>";
    if rows.len() > size {
        html += &format!(
            "<nav class=\"child-footer embedded-pagination\" aria-label=\"{}\"><span>{}</span><div class=\"actions\">",
            escape(&crate::i18n::t("Table pages")),
            escape(&crate::i18n::message("{start}–{end} of {total}", &[("start", &(offset + 1).to_string()), ("end", &(offset + size).min(rows.len()).to_string()), ("total", &rows.len().to_string())]))
        );
        for (label, enabled, start) in [
            ("Previous", offset > 0, offset.saturating_sub(size)),
            ("Next", offset + size < rows.len(), offset + size),
        ] {
            if enabled {
                let mut next = params.clone();
                next.insert(key("offset"), start.to_string());
                html += &format!(
                    "<a class=\"button small\" href=\"{}\">{}</a>",
                    escape(&scoped_url(&next)), escape(&crate::i18n::t(label))
                );
            } else {
                html += &format!("<button class=\"small\" disabled>{}</button>", escape(&crate::i18n::t(label)));
            }
        }
        html += "</div></nav>";
    }
    html + "</section>"
}

/// Use the same configured page length in the edit form as the record table.
pub fn edit_page_size(editor: &Editor, field: &DraftField) -> usize {
    field
        .schema
        .pointer("/x-presentation/table/pageSize")
        .and_then(Value::as_u64)
        .or_else(|| editor.settings().ok()?.value["tablePageSize"].as_u64())
        .unwrap_or(25)
        .clamp(1, 100) as usize
}
