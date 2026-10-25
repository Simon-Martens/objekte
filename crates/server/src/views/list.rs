use super::table::{GET_FORM, TableColumns, hidden, query_url};
use super::*;
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
    let table_columns = TableColumns::new(&c.editor_schema, &ui, params, "", &["_label", "_id"]);
    let request = ListRequest {
        fields: Some(
            table_columns
                .selected
                .iter()
                .map(|(key, _)| key.clone())
                .collect(),
        ),
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
        search_options: super::search::options(params, ""),
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
    let url = query_url(&base, params);
    let mut body = format!(
        "<main id=\"workspace\" class=\"collection-workspace\" data-url=\"{}\" data-section=\"{base}\" tabindex=\"-1\"><div class=\"page-heading\"><div><p class=\"eyebrow\">{ui_text_0}</p><h1>{} <span class=\"heading-count mono\">{}</span></h1></div><div class=\"actions\">",
        escape(&url),
        escape(crate::i18n::text(&c.ui,"label", &c.label)),
        page.total, ui_text_0 = crate::views::escape(&crate::i18n::t("Content / Collection")));
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
            "<details class=\"surface filter-panel\" open><summary><i class=\"ri-filter-3-line\" aria-hidden=\"true\"></i>{ui_text_0}<span class=\"badge\">{}</span></summary><form action=\"{base}\" {get}>{}",
            request.filters.values().filter(|v| !v.is_empty()).count(),
            hidden(params, &["offset", "filter."]), ui_text_0 = crate::views::escape(&crate::i18n::t("Filters")));
        for facet in &page.facets {
            let key = format!("filter.{}", facet.field);
            let value = params.get(&key).map(String::as_str).unwrap_or("");
            body += &format!(
                "<div class=\"filter-field\"><label for=\"filter-{}\">{}</label>",
                escape(&facet.field),
                escape(crate::i18n::field(&c.editor_schema["properties"][&facet.field], &facet.label))
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
                            escape(crate::i18n::field(&c.editor_schema["properties"][&facet.field], &facet.label))
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
                        "<select id=\"filter-{}\" name=\"{}\"><option value=\"\">{ui_text_0}</option><option value=\"__missing\" {}>{ui_text_1}</option>",
                        escape(&facet.field),
                        escape(&key),
                        if value == "__missing" { "selected" } else { "" }, ui_text_0 = crate::views::escape(&crate::i18n::t("All values")), ui_text_1 = crate::views::escape(&crate::i18n::t("Not set")));
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
            "<a class=\"button quiet\" data-nav href=\"{}\">{ui_text_0}</a>",
            escape(&query_url(&base, &all)), ui_text_0 = crate::views::escape(&crate::i18n::t("Show All")))
    };
    let search_form = super::search::form(
        &base,
        params,
        "",
        "search-pattern",
        "Search all indexed fields…",
        pattern_error.as_deref(),
        &show_all,
    );
    let column_picker = table_columns.picker("collection", &base, params, "");
    body += &format!(
        "<section class=\"surface table-panel\" aria-label=\"Records\"><div class=\"list-toolbar\">{search_form}{column_picker}</div>"
    );
    body += &super::search::validation("search-pattern", pattern_error.as_deref());
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
    body += &super::record_table::RecordTable {
        base: &base,
        params,
        columns: &table_columns,
        request: &request,
        page: &page,
        pattern_error: pattern_error.as_deref(),
        navigation: "data-nav",
        placement: placement.as_ref().map(|(parent, id, field, revision)| {
            (
                parent.as_str(),
                id.as_str(),
                field.as_str(),
                revision.as_str(),
            )
        }),
    }
    .render(editor, name, actor)?;
    body += "</section></div></main>";
    Ok(body)
}
