use super::*;
use super::table::{TableColumns, GET_FORM, hidden, query_url};
use std::collections::HashMap;
use structured_editor_core::ListRequest;

/// Only presentation lives here; the library supplies UTF-8 match ranges.
pub fn highlighted(
    text: &str,
    ranges: &[structured_editor_core::MatchRange],
    excerpt: bool,
) -> String {
    let first = ranges.first().map(|r| r.start).unwrap_or(0);
    let start = if excerpt {
        text[..first]
            .char_indices()
            .rev()
            .nth(59)
            .map(|(i, _)| i)
            .unwrap_or(0)
    } else {
        0
    };
    let end = if excerpt {
        text[start..]
            .char_indices()
            .nth(230)
            .map(|(i, _)| start + i)
            .unwrap_or(text.len())
    } else {
        text.len()
    };
    let mut html = if start > 0 {
        "…".to_owned()
    } else {
        String::new()
    };
    let mut cursor = start;
    for range in ranges {
        let a = range.start.max(start);
        let b = range.end.min(end);
        if a >= b {
            continue;
        }
        html += &escape(&text[cursor..a]);
        html += "<mark>";
        html += &escape(&text[a..b]);
        html += "</mark>";
        cursor = b;
    }
    html += &escape(&text[cursor..end]);
    if end < text.len() {
        html += "…";
    }
    html
}
pub fn render(
    editor: &Editor,
    name: &str,
    params: &HashMap<String, String>,
    actor: &Actor,
) -> Result<String> {
    let c = editor.config().collection(name)?;
    let ui = if let (Some(parent), Some(field)) =
        (params.get("parent_collection"), params.get("parent_field"))
    {
        editor.child_list_ui(parent, field)?
    } else {
        c.ui.clone()
    };
    let base = format!("/collections/{}", encode(name));
    let settings = editor.settings().ok().map(|d| d.value).unwrap_or_default();
    let limit = ui["pageSize"]
        .as_u64()
        .or_else(|| settings["pageSize"].as_u64())
        .unwrap_or(50)
        .clamp(1, 200) as usize;
    let offset = params
        .get("offset")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let q = params.get("q").cloned().unwrap_or_default();
    let tree = params.get("tree").cloned().unwrap_or_else(|| "[]".into());
    let category = serde_json::from_str(&tree)
        .map_err(|_| crate::error::WebError::bad("Invalid tree path"))?;
    let default_sort = ui
        .pointer("/list/defaultSort/field")
        .and_then(|v| v.as_str())
        .unwrap_or(
            if ui.pointer("/ordering/by").and_then(|v| v.as_str()) == Some("manual") {
                "_manual"
            } else {
                "_label"
            },
        );
    let sort = params
        .get("sort")
        .map(String::as_str)
        .unwrap_or(default_sort);
    let direction = params.get("direction").map(String::as_str).unwrap_or(
        ui.pointer("/list/defaultSort/direction")
            .and_then(|v| v.as_str())
            .unwrap_or("asc"),
    );
    let scope_keys = ["parent_collection", "parent_id", "parent_field"];
    let scope_count = scope_keys
        .iter()
        .filter(|k| params.contains_key(**k))
        .count();
    if scope_count != 0 && scope_count != 3 {
        return Err(crate::error::WebError::bad(
            "Incomplete parent list context",
        ));
    }
    let request = ListRequest {
        ids: if let (Some(parent), Some(id), Some(field)) = (
            params.get("parent_collection"),
            params.get("parent_id"),
            params.get("parent_field"),
        ) {
            if editor.child_collection(parent, field)?.collection != name {
                return Err(crate::error::WebError::bad(
                    "Child collection does not match its parent list",
                ));
            }
            Some(editor.child_ids(parent, id, field)?)
        } else {
            None
        },
        query: q.clone(),
        search_options: structured_editor_core::SearchOptions {
            regex: params.get("regex").is_some_and(|v| v == "1"),
            case_sensitive: params.get("case_sensitive").is_some_and(|v| v == "1"),
        },
        search_fields: None,
        display_field: None,
        label_view: String::new(),
        category,
        sort: sort.into(),
        descending: direction == "desc",
        offset,
        limit,
        filters: params
            .iter()
            .filter_map(|(k, v)| {
                k.strip_prefix("filter.")
                    .map(|key| (key.to_owned(), v.clone()))
            })
            .collect(),
    };
    let (page, pattern_error) = match editor.query_collection_with_ui(name, &request, &ui) {
        Ok(page) => (page, None),
        Err(structured_editor_core::Error::SearchPattern(message)) => (
            structured_editor_core::ListPage {
                rows: Vec::new(),
                total: 0,
                matched: 0,
                facets: Vec::new(),
            },
            Some(message),
        ),
        Err(error) => return Err(error.into()),
    };
    let props = c.editor_schema["properties"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let table_columns = TableColumns::new(&c.editor_schema, &ui, params, "", &["_label", "_id"]);
    let available = &table_columns.available;
    let columns = &table_columns.selected;
    let url = query_url(&base, params);
    let mut body = format!(
        "<main id=\"workspace\" class=\"collection-workspace\" data-url=\"{}\" data-section=\"{base}\" tabindex=\"-1\"><div class=\"page-heading\"><div><p class=\"eyebrow\">Content / Collection</p><h1>{} <span class=\"heading-count mono\">{}</span></h1></div><div class=\"actions\">",
        escape(&url),
        escape(&c.label),
        page.total
    );
    if request.ids.is_some() {
        let marker = format!(
            " data-record-url=\"{}\"",
            record_url(&params["parent_collection"], &params["parent_id"])
        );
        if let Some(at) = body.find(" tabindex=") {
            body.insert_str(at, &marker);
        }
    }
    if actor.editable()
        && editor.allows(name, "create")
        && (request.ids.is_none() || editor.allows(&params["parent_collection"], "update"))
    {
        body += &format!(
            "<form action=\"{base}/edit-sessions\" method=\"post\" data-on:submit__prevent=\"@post(el.getAttribute('action'),{{contentType:'form',retry:'never',retryMaxCount:0}})\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><input type=\"hidden\" name=\"context\" value=\"{}\"><button class=\"primary\"><i class=\"ri-add-line\" aria-hidden=\"true\"></i>New {}</button></form>",
            escape(&actor.csrf),
            escape(&tree),
            escape(ui["singularLabel"].as_str().unwrap_or("record"))
        );
        if request.ids.is_some() {
            // Attach context to the create form, using the parent's current revision.
            let parent = &params["parent_collection"];
            let id = &params["parent_id"];
            let revision = editor.get(parent, id)?.revision;
            let extra = format!(
                "{}<input type=\"hidden\" name=\"parent_revision\" value=\"{}\">",
                hidden(
                    params,
                    &[
                        "q",
                        "sort",
                        "direction",
                        "offset",
                        "tree",
                        "filter.",
                        "column.",
                        "columns"
                    ]
                ),
                escape(&revision)
            );
            let index = body.rfind("</form>").unwrap();
            body.insert_str(index, &extra);
        }
    }
    body += "</div></div>";
    if request.ids.is_some() {
        body += &format!(
            "<nav class=\"collection-trail\"><a data-nav href=\"{}\"><i class=\"ri-arrow-left-line\" aria-hidden=\"true\"></i>Back to {}</a></nav>",
            record_url(&params["parent_collection"], &params["parent_id"]),
            escape(
                &editor.label(
                    &params["parent_collection"],
                    &editor
                        .get(&params["parent_collection"], &params["parent_id"])?
                        .value,
                    &params["parent_id"],
                    "record"
                )?
            )
        );
    }
    if !request.category.is_empty() {
        let roots = editor.categories(name)?;
        let mut nodes = &roots;
        let mut path = Vec::new();
        body += &format!(
            "<nav class=\"collection-trail\" aria-label=\"Collection category\"><a data-nav href=\"{base}\" class=\"tree-all\">All</a>"
        );
        for id in &request.category {
            let node = nodes
                .iter()
                .find(|n| &n.value == id)
                .ok_or_else(|| crate::error::WebError::bad("Unknown tree path"))?;
            path.push(id);
            let mut p = params.clone();
            p.insert("tree".into(), serde_json::to_string(&path).unwrap());
            p.remove("offset");
            body += &format!(
                "<i class=\"ri-arrow-right-s-line\" aria-hidden=\"true\"></i><a data-nav href=\"{}\">{}</a>",
                escape(&query_url(&base, &p)),
                escape(&node.label)
            );
            nodes = &node.children;
        }
        body += "</nav>";
    }
    body += "<div class=\"collection-layout\">";
    let get = GET_FORM;
    if !page.facets.is_empty() {
        body += &format!(
            "<details class=\"surface filter-panel\" open><summary><i class=\"ri-filter-3-line\" aria-hidden=\"true\"></i>Filters<span class=\"badge\">{}</span></summary><form action=\"{base}\" {get}>{}",
            request.filters.values().filter(|v| !v.is_empty()).count(),
            hidden(params, &["offset", "filter."])
        );
        for facet in &page.facets {
            let key = format!("filter.{}", facet.field);
            let value = params.get(&key).map(String::as_str).unwrap_or("");
            body += &format!(
                "<div class=\"filter-field\"><label for=\"filter-{}\">{}</label>",
                escape(&facet.field),
                escape(&facet.label)
            );
            match facet.kind.as_str() {
                "boolean" => {
                    body += &format!(
                        "<select id=\"filter-{}\" name=\"{}\">",
                        escape(&facet.field),
                        escape(&key)
                    );
                    for (option, label) in [
                        ("", "All values"),
                        ("true", "true"),
                        ("false", "false"),
                        ("__missing", "unset"),
                    ] {
                        body += &format!(
                            "<option value=\"{option}\" {}>{label}</option>",
                            if value == option { "selected" } else { "" }
                        );
                    }
                    body += "</select>";
                }
                "number" => {
                    for (suffix, label) in [("min", "From"), ("max", "To")] {
                        let key = format!("{key}.{suffix}");
                        body += &format!(
                            "<input type=\"number\" step=\"any\" name=\"{}\" value=\"{}\" placeholder=\"{label}\" aria-label=\"{} {label}\">",
                            escape(&key),
                            escape(params.get(&key).map(String::as_str).unwrap_or("")),
                            escape(&facet.label)
                        );
                    }
                }
                "text" => {
                    body += &format!(
                        "<input id=\"filter-{}\" name=\"{}\" value=\"{}\" placeholder=\"Contains…\">",
                        escape(&facet.field),
                        escape(&key),
                        escape(value)
                    )
                }
                _ => {
                    body += &format!(
                        "<select id=\"filter-{}\" name=\"{}\"><option value=\"\">All values</option><option value=\"__missing\" {}>Not set</option>",
                        escape(&facet.field),
                        escape(&key),
                        if value == "__missing" { "selected" } else { "" }
                    );
                    for (v, label, count) in &facet.options {
                        body += &format!(
                            "<option value=\"{}\" {}>{} ({count})</option>",
                            escape(v),
                            if v == value { "selected" } else { "" },
                            escape(label)
                        );
                    }
                    body += "</select>";
                }
            }
            body += "</div>";
        }
        let mut clear = params.clone();
        clear.retain(|k, _| !k.starts_with("filter.") && k != "offset");
        body += &format!(
            "<div class=\"actions\"><button class=\"primary\">Apply filters</button><a class=\"button quiet\" data-nav href=\"{}\">Clear</a></div></form></details>",
            escape(&query_url(&base, &clear))
        );
    }
    let show_all = if q.is_empty() {
        String::new()
    } else {
        let mut all = params.clone();
        all.retain(|key, _| {
            key != "q"
                && key != "regex"
                && key != "case_sensitive"
                && key != "offset"
                && key != "tree"
                && !key.starts_with("filter.")
        });
        format!(
            "<a class=\"button quiet\" data-nav href=\"{}\">Show All</a>",
            escape(&query_url(&base, &all))
        )
    };
    let regex_enabled = request.search_options.regex;
    let case_sensitive = request.search_options.case_sensitive;
    let search_controls = format!(
        "<input type=\"hidden\" name=\"case_sensitive\" value=\"{}\"><input type=\"hidden\" name=\"regex\" value=\"{}\"><span class=\"search-options\"><button type=\"button\" class=\"search-toggle\" data-case-toggle aria-label=\"Match case\" aria-pressed=\"{}\" title=\"Match case\"><span aria-hidden=\"true\">Aa</span></button><button type=\"button\" class=\"search-toggle\" data-regex-toggle aria-label=\"Use regular expression\" aria-pressed=\"{}\" title=\"Use regular expression\"><span aria-hidden=\"true\">.*</span></button></span>",
        if case_sensitive { "1" } else { "0" },
        if regex_enabled { "1" } else { "0" },
        case_sensitive,
        regex_enabled
    );
    let validation = format!(
        "<span id=\"search-pattern-error\" class=\"search-pattern-error\" role=\"status\">{}</span>",
        escape(pattern_error.as_deref().unwrap_or(""))
    );
    let invalid = if pattern_error.is_some() {
        "true"
    } else {
        "false"
    };
    let column_picker = table_columns.picker("collection", &base, params, "");
    body += &format!(
        "<section class=\"surface table-panel\" aria-label=\"Records\"><div class=\"list-toolbar\"><form class=\"search-form\" data-regex-search action=\"{base}\" {get}>{}<div class=\"search-input has-options\"><i class=\"ri-search-line\" aria-hidden=\"true\"></i><label class=\"sr-only\" for=\"collection-search\">Search</label><input id=\"collection-search\" aria-invalid=\"{invalid}\" aria-describedby=\"search-pattern-error\" type=\"search\" name=\"q\" value=\"{}\" placeholder=\"Search all indexed fields…\">{search_controls}</div><button>Search</button>{show_all}</form>{column_picker}</div>",
        hidden(params, &["q", "regex", "case_sensitive", "offset"]),
        escape(&q)
    );
    body += &validation;
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
                "<a class=\"icon-button\" data-nav href=\"{}\" aria-label=\"{label}\"><i class=\"{icon}\" aria-hidden=\"true\"></i></a>",
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
            if page.matched == 1 { "entry" } else { "entries" }
        } else {
            if page.matched == 1 { "match" } else { "matches" }
        },
        if page.matched == page.total {
            String::new()
        } else {
            format!("<span class=\"muted\">of {} entries</span>", page.total)
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
            "<th scope=\"col\" aria-sort=\"{}\"><a data-nav href=\"{}\">{}<i class=\"{}\" aria-hidden=\"true\"></i></a></th>",
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
    body += "<th scope=\"col\"><span class=\"sr-only\">Actions</span></th></tr></thead><tbody>";
    let placement = if request.ids.is_some() {
        Some((
            &params["parent_collection"],
            &params["parent_id"],
            &params["parent_field"],
            editor
                .get(&params["parent_collection"], &params["parent_id"])?
                .revision,
        ))
    } else {
        None
    };
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
            body += &format!(
                "<td data-label=\"{}\" class=\"{}\"><a data-nav class=\"{}\" href=\"{link}\">{text}</a></td>",
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
            "<td class=\"row-actions\"><div class=\"row-action-buttons\"><a class=\"icon-button\" data-nav href=\"{link}\" aria-label=\"Open record\"><i class=\"ri-arrow-right-up-line\" aria-hidden=\"true\"></i></a>"
        );
        if actor.editable() && editor.allows(name, "update") {
            body += &format!(
                "<a class=\"icon-button\" data-nav href=\"{link}/edit\" aria-label=\"Edit\"><i class=\"ri-edit-line\" aria-hidden=\"true\"></i></a>"
            );
        }
        body += &crate::hierarchy::record_actions(
            editor,
            actor,
            name,
            &row.id,
            &row.label,
            placement.as_ref().map(|(parent, id, field, revision)| {
                (
                    parent.as_str(),
                    id.as_str(),
                    field.as_str(),
                    revision.as_str(),
                )
            }),
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
                "<a class=\"button\" data-nav href=\"{}\">{label}</a>",
                escape(&query_url(&base, &p))
            );
        } else {
            body += &format!("<button type=\"button\" disabled>{label}</button>");
        }
    }
    body += "</div></footer></section></div></main>";
    Ok(body)
}
