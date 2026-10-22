use super::*;
use std::collections::HashMap;
use structured_editor_core::ListRequest;

/// Escape first; match ranges are computed against Unicode characters, never HTML.
pub fn highlighted(text: &str, query: &str, excerpt: bool) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut folded = String::new();
    let mut mapping = Vec::new();
    for (index, c) in chars.iter().enumerate() {
        for lower in c.to_lowercase() {
            let start = folded.len();
            folded.push(lower);
            mapping.extend(std::iter::repeat_n(index, folded.len() - start));
        }
    }
    let needle = query.trim().to_lowercase();
    let ranges: Vec<(usize, usize)> = if needle.is_empty() {
        Vec::new()
    } else {
        folded
            .match_indices(&needle)
            .map(|(i, m)| (mapping[i], mapping[i + m.len() - 1] + 1))
            .collect()
    };
    let start = if excerpt {
        ranges.first().map(|r| r.0.saturating_sub(60)).unwrap_or(0)
    } else {
        0
    };
    let end = if excerpt {
        (start + 230).min(chars.len())
    } else {
        chars.len()
    };
    let mut html = if start > 0 {
        "…".into()
    } else {
        String::new()
    };
    for i in start..end {
        if ranges.iter().any(|r| r.0 == i) {
            html += "<mark>";
        }
        html += &escape(&chars[i].to_string());
        if ranges.iter().any(|r| r.1 == i + 1) {
            html += "</mark>";
        }
    }
    if ranges.iter().any(|r| r.0 < end && r.1 > end) {
        html += "</mark>";
    }
    if end < chars.len() {
        html += "…";
    }
    html
}
fn query_url(base: &str, params: &HashMap<String, String>) -> String {
    let mut values: Vec<_> = params.iter().filter(|(_, v)| !v.is_empty()).collect();
    values.sort_by(|a, b| a.0.cmp(b.0));
    format!(
        "{base}?{}",
        values
            .into_iter()
            .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
            .collect::<Vec<_>>()
            .join("&")
    )
}
fn hidden(params: &HashMap<String, String>, exclude: &[&str]) -> String {
    params
        .iter()
        .filter(|(k, _)| {
            !exclude
                .iter()
                .any(|e| k.as_str() == *e || e.ends_with('.') && k.starts_with(e))
        })
        .map(|(k, v)| {
            format!(
                "<input type=\"hidden\" name=\"{}\" value=\"{}\">",
                escape(k),
                escape(v)
            )
        })
        .collect()
}
pub fn render(
    editor: &Editor,
    name: &str,
    params: &HashMap<String, String>,
    actor: &Actor,
) -> Result<String> {
    let c = editor.config().collection(name)?;
    let base = format!("/collections/{}", encode(name));
    let settings = editor.settings().ok().map(|d| d.value).unwrap_or_default();
    let limit = c.ui["pageSize"]
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
    let default_sort =
        c.ui.pointer("/list/defaultSort/field")
            .and_then(|v| v.as_str())
            .unwrap_or(
                if c.ui.pointer("/ordering/by").and_then(|v| v.as_str()) == Some("manual") {
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
        c.ui.pointer("/list/defaultSort/direction")
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
    let page = editor.query_collection(name, &request)?;
    let props = c.editor_schema["properties"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let mut available = vec![
        ("_label".to_owned(), "Record".to_owned()),
        ("_id".into(), "Identifier".into()),
    ];
    available.extend(
        props
            .iter()
            .filter(|(k, s)| {
                k.as_str() != "_editor" && s["x-editor"] != "hidden" && s["x-visible"] != false
            })
            .map(|(k, s)| (k.clone(), s["title"].as_str().unwrap_or(k).to_owned())),
    );
    let configured: Vec<String> =
        c.ui.pointer("/list/columns")
            .and_then(|v| v.as_array())
            .map(|v| {
                v.iter()
                    .filter_map(|v| v.as_str().or_else(|| v["field"].as_str()))
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_else(|| vec!["_label".into(), "_id".into()]);
    let columns: Vec<_> = available
        .iter()
        .filter(|(key, _)| {
            if params.get("columns").is_some_and(|v| v == "custom") {
                params.contains_key(&format!("column.{key}"))
            } else {
                configured.contains(key)
            }
        })
        .cloned()
        .collect();
    let mut columns = if columns.is_empty() {
        vec![available[0].clone()]
    } else {
        columns
    };
    columns.sort_by_key(|(key, _)| {
        configured
            .iter()
            .position(|v| v == key)
            .unwrap_or(usize::MAX)
    });
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
            "<form action=\"{base}/edit-sessions\" method=\"post\" data-on:submit__prevent=\"@post(el.getAttribute('action'),{{contentType:'form',retry:'never'}})\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><input type=\"hidden\" name=\"context\" value=\"{}\"><button class=\"primary\"><i class=\"ri-add-line\" aria-hidden=\"true\"></i>New {}</button></form>",
            escape(&actor.csrf),
            escape(&tree),
            escape(c.ui["singularLabel"].as_str().unwrap_or("record"))
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
    let get = "data-on:submit__prevent=\"@get(el.getAttribute('action')+'?'+new URLSearchParams(new FormData(el)),{filterSignals:{include:/^$/}})\"";
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
    body += &format!(
        "<section class=\"surface table-panel\" aria-label=\"Records\"><div class=\"list-toolbar\"><form class=\"search-form\" action=\"{base}\" {get}>{}<label class=\"search-input\"><i class=\"ri-search-line\" aria-hidden=\"true\"></i><span class=\"sr-only\">Search</span><input type=\"search\" name=\"q\" value=\"{}\" placeholder=\"Search all indexed fields…\"></label><button>Search</button></form><details class=\"action-menu column-picker\"><summary class=\"button\"><i class=\"ri-layout-column-line\" aria-hidden=\"true\"></i>Columns</summary><form class=\"menu-panel\" action=\"{base}\" {get}>{}<input type=\"hidden\" name=\"columns\" value=\"custom\"><p class=\"eyebrow\">Visible columns</p>",
        hidden(params, &["q", "offset"]),
        escape(&q),
        hidden(params, &["column.", "columns", "offset"])
    );
    for (key, label) in &available {
        body += &format!(
            "<label class=\"column-option\"><input type=\"checkbox\" name=\"column.{}\" value=\"1\" {}>{}</label>",
            escape(key),
            if columns.iter().any(|(k, _)| k == key) {
                "checked"
            } else {
                ""
            },
            escape(label)
        );
    }
    body += "<button class=\"primary\">Apply columns</button></form></details></div>";
    body += &format!(
        "<div class=\"result-summary\"><span><strong>{}</strong> {} {}</span><span class=\"mono\">{}–{}</span></div><div class=\"table-scroll\"><table class=\"data-table\"><thead><tr>",
        page.matched,
        if q.is_empty() && request.filters.values().all(|v| v.is_empty()) {
            "entries"
        } else {
            "matches"
        },
        if page.matched == page.total {
            String::new()
        } else {
            format!("<span class=\"muted\">of {} entries</span>", page.total)
        },
        if page.rows.is_empty() { 0 } else { offset + 1 },
        (offset + page.rows.len()).min(page.matched)
    );
    for (key, label) in &columns {
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
    for row in &page.rows {
        let link = record_url(name, &row.id);
        body += "<tr>";
        for (key, label) in &columns {
            let text = match key.as_str() {
                "_id" => Some(&row.id),
                "_label" => Some(&row.label),
                _ => row.display.get(key),
            };
            let text = text
                .map(|s| highlighted(s, &q, true))
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
            "<td class=\"row-actions\"><a class=\"icon-button\" data-nav href=\"{link}\" aria-label=\"Open record\"><i class=\"ri-arrow-right-up-line\" aria-hidden=\"true\"></i></a>"
        );
        if actor.editable() && editor.allows(name, "update") {
            body += &format!(
                "<form method=\"post\" action=\"{link}/edit-sessions\" data-on:submit__prevent=\"@post(el.getAttribute('action'),{{contentType:'form',retry:'never'}})\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><button class=\"icon-button\" aria-label=\"Edit record\"><i class=\"ri-edit-line\" aria-hidden=\"true\"></i></button></form>",
                escape(&actor.csrf)
            );
        }
        body += "</td></tr>";
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
                    highlighted(&m.text, &q, true)
                );
            }
            body += "</div></td></tr>";
        }
    }
    body += "</tbody></table></div>";
    if page.rows.is_empty() {
        body += "<div class=\"empty-state\"><i class=\"ri-search-line\" aria-hidden=\"true\"></i><h2>No matching records</h2><p>Try a broader search or clear a filter.</p></div>";
    }
    body += "<footer class=\"list-footer\"><span class=\"help\">Click a column heading to sort</span><div class=\"actions\">";
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
        }
    }
    body += "</div></footer></section></div></main>";
    Ok(body)
}
