//! Incoming links use the source collection's normal list presentation.
use super::{
    record_table::RecordTable,
    table::{TableColumns, query_url},
    *,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use structured_editor_core::{ListPage, ListRequest, Reference};

pub fn render(
    editor: &Editor,
    name: &str,
    id: &str,
    params: &HashMap<String, String>,
    actor: &Actor,
) -> Result<String> {
    editor.require_operation(name, "read")?;
    editor.get(name, id)?;
    let parents = editor.parent_links(name, id)?;
    let mut collections: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut documents = BTreeMap::new();
    for reference in editor.cached_references_to(name, id)? {
        if let Some(document) = &reference.document {
            documents
                .entry((document.clone(), reference.record_id.clone()))
                .or_insert(reference);
        } else if editor.allows(&reference.collection, "read")
            && !parents.iter().any(|parent| {
                parent.collection == reference.collection
                    && parent.id == reference.record_id
                    && reference.path.starts_with(&format!(
                        "/{}/",
                        parent.field.replace('~', "~0").replace('/', "~1")
                    ))
            })
        {
            collections
                .entry(reference.collection)
                .or_default()
                .insert(reference.record_id);
        }
    }
    let base = format!("{}/references", record_url(name, id));
    let settings = editor.settings().ok().map(|d| d.value).unwrap_or_default();
    let limit = settings["tablePageSize"]
        .as_u64()
        .unwrap_or(25)
        .clamp(1, 200) as usize;
    let selected = params.get("source");
    let mut html = String::new();
    let mut groups: Vec<_> = collections.into_iter().collect();
    groups.sort_by_key(|(source, _)| {
        let c = &editor.config().collections[source];
        (
            c.ui["position"].as_i64().unwrap_or(i64::MAX),
            crate::i18n::text(&c.ui,"label", &c.label).to_owned(),
        )
    });
    for (source, ids) in groups {
        if selected.is_some_and(|s| s != &source) {
            continue;
        }
        let mut query = if selected.is_some() {
            params.clone()
        } else {
            HashMap::new()
        };
        query.insert("source".into(), source.clone());
        let content = collection(editor, &source, ids, &base, &query, limit, actor)?;
        if selected.is_some() {
            return Ok(content);
        }
        if content.is_empty() {
            continue;
        }
        html += &wrap(&source, &base, &query, &content);
    }
    if !documents.is_empty() && selected.is_none_or(|s| s == "_documents") {
        let mut query = if selected.is_some() {
            params.clone()
        } else {
            HashMap::new()
        };
        query.insert("source".into(), "_documents".into());
        let content = document_table(
            &documents.into_values().collect::<Vec<_>>(),
            &base,
            &query,
            limit,
        );
        if selected.is_some() {
            return Ok(content);
        }
        html += &wrap("_documents", &base, &query, &content);
    }
    Ok(html)
}

fn wrap(source: &str, base: &str, query: &HashMap<String, String>, content: &str) -> String {
    format!(
        "<record-reference-table id=\"references-{}\" data-url=\"{}\">{content}</record-reference-table>",
        encode(source),
        escape(&query_url(base, query))
    )
}

fn collection(
    editor: &Editor,
    name: &str,
    ids: BTreeSet<String>,
    base: &str,
    params: &HashMap<String, String>,
    limit: usize,
    actor: &Actor,
) -> Result<String> {
    let c = editor.config().collection(name)?;
    let ui = &c.ui;
    let columns = TableColumns::new(&c.editor_schema, ui, params, "", &["_label", "_id"]);
    let mut request = ListRequest {
        ids: Some(ids),
        fields: Some(
            columns
                .selected
                .iter()
                .map(|(key, _)| key.clone())
                .collect(),
        ),
        query: params.get("q").cloned().unwrap_or_default(),
        search_options: search::options(params, ""),
        sort: params.get("sort").cloned().unwrap_or_else(|| {
            ui.pointer("/list/defaultSort/field")
                .and_then(|v| v.as_str())
                .unwrap_or("_label")
                .into()
        }),
        descending: params
            .get("direction")
            .map(String::as_str)
            .unwrap_or_else(|| {
                ui.pointer("/list/defaultSort/direction")
                    .and_then(|v| v.as_str())
                    .unwrap_or("asc")
            })
            == "desc",
        offset: params
            .get("offset")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        limit,
        ..Default::default()
    };
    let (mut page, pattern_error) = match editor.query_collection_with_ui(name, &request, ui) {
        Ok(page) => (page, None),
        Err(structured_editor_core::Error::SearchPattern(message)) => (
            ListPage {
                rows: vec![],
                total: request.ids.as_ref().map(BTreeSet::len).unwrap_or(0),
                matched: 0,
                facets: vec![],
            },
            Some(message),
        ),
        Err(error) => return Err(error.into()),
    };
    if page.total == 0 && pattern_error.is_none() {
        return Ok(String::new());
    }
    if pattern_error.is_none() && request.offset > 0 && request.offset >= page.matched {
        request.offset = page.matched.saturating_sub(1) / limit * limit;
        page = editor.query_collection_with_ui(name, &request, ui)?;
    }
    let scope = format!("reference-{}", encode(name));
    let mut html = format!(
        "<section class=\"surface child-list table-panel reference-table\" aria-labelledby=\"{scope}-title\"><div class=\"child-heading\"><h2 id=\"{scope}-title\" tabindex=\"-1\"><i class=\"{}\" aria-hidden=\"true\"></i>{}<span class=\"badge mono\">{}</span></h2><button type=\"button\" class=\"small quiet\" data-reference-refresh><i class=\"ri-refresh-line\" aria-hidden=\"true\"></i>Refresh</button></div>",
        icon_class(ui["icon"].as_str(), "ri-links-line"),
        escape(crate::i18n::text(&c.ui,"label", &c.label)),
        page.total
    );
    let mut clear = params.clone();
    for key in ["q", "regex", "case_sensitive", "offset"] {
        clear.remove(key);
    }
    let show_all = if request.query.is_empty() {
        String::new()
    } else {
        format!(
            "<a class=\"button quiet\" data-reference-query href=\"{}\">{ui_text_0}</a>",
            escape(&query_url(base, &clear)), ui_text_0 = crate::views::escape(&crate::i18n::t("Show All")))
    };
    html += &format!(
        "<div class=\"list-toolbar\">{}{}</div>",
        search::form_with_submit(
            base,
            params,
            "",
            &scope,
            &format!("Search {}…", crate::i18n::text(&c.ui,"label", &c.label).to_lowercase()),
            pattern_error.as_deref(),
            &show_all,
            "data-reference-query"
        ),
        columns.picker_with_submit(&scope, base, params, "", "data-reference-query", None)
    );
    html += &search::validation(&scope, pattern_error.as_deref());
    html += &RecordTable {
        base,
        params,
        columns: &columns,
        request: &request,
        page: &page,
        pattern_error: pattern_error.as_deref(),
        navigation: "data-reference-query",
        placement: None,
    }
    .render(editor, name, actor)?;
    html += "</section>";
    Ok(html)
}

// Mapped documents have no collection schema/list configuration. Keep them in
// their own table, with one row per source file rather than per link occurrence.
fn document_table(
    references: &[Reference],
    base: &str,
    params: &HashMap<String, String>,
    limit: usize,
) -> String {
    let offset = params
        .get("offset")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0)
        .min(references.len().saturating_sub(1) / limit * limit);
    let mut html = format!(
        "<section class=\"surface child-list table-panel reference-table\" aria-label=\"Documents\"><div class=\"child-heading\"><h2 tabindex=\"-1\">Documents<span class=\"badge mono\">{}</span></h2><button type=\"button\" class=\"small quiet\" data-reference-refresh>Refresh</button></div><div class=\"table-scroll\"><table class=\"data-table\"><thead><tr><th>{ui_text_0}</th><th>File</th></tr></thead><tbody>",
        references.len(), ui_text_0 = crate::views::escape(&crate::i18n::t("Document")));
    for reference in references.iter().skip(offset).take(limit) {
        let document = reference.document.as_deref().unwrap_or("");
        let (url, label) = if document.is_empty() {
            ("/settings".into(), "Editor settings")
        } else {
            (
                format!(
                    "/documents/{}?file={}",
                    encode(document),
                    encode(&reference.record_id)
                ),
                document,
            )
        };
        html += &format!(
            "<tr><td data-label=\"Document\"><a data-nav class=\"table-record-link\" href=\"{}\">{}</a></td><td data-label=\"File\">{}</td></tr>",
            escape(&url),
            escape(label),
            escape(&reference.record_id)
        );
    }
    html += &format!(
        "</tbody></table></div><footer class=\"list-footer\"><span class=\"muted\">{}–{} of {}</span><div class=\"actions\">",
        offset + 1,
        (offset + limit).min(references.len()),
        references.len()
    );
    for (enabled, next, label) in [
        (offset > 0, offset.saturating_sub(limit), "Previous"),
        (offset + limit < references.len(), offset + limit, "Next"),
    ] {
        if enabled {
            let mut query = params.clone();
            query.insert("offset".into(), next.to_string());
            html += &format!(
                "<a class=\"button\" data-reference-query data-reference-page href=\"{}\">{label}</a>",
                escape(&query_url(base, &query))
            );
        } else {
            html += &format!("<button type=\"button\" disabled>{label}</button>");
        }
    }
    html += "</div></footer></section>";
    html
}
