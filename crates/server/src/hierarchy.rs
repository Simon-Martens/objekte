//! Schema-declared child lists use the same records, permissions and query engine as collections.
use crate::{
    AppState,
    auth::Actor,
    blocking,
    error::{Result, WebError},
    routes::{mutation_actor, page, page_actor},
    views::{self, encode, escape, record_url},
};
use axum::{
    Router,
    extract::{Form, Path, Query, State},
    http::HeaderMap,
    response::Response,
    routing::{get, post},
};
use std::{collections::HashMap, sync::Arc};
use structured_editor_core::{Editor, HierarchyNode, ListRequest};

const SUBMIT: &str = "data-on:submit__prevent=\"@post(el.getAttribute('action'),{contentType:'form',retry:'never',retryMaxCount:0})\"";
fn hidden(key: &str, value: &str) -> String {
    format!(
        "<input type=\"hidden\" name=\"{}\" value=\"{}\">",
        escape(key),
        escape(value)
    )
}
fn active_url(workspace: &str) -> &str {
    workspace
        .split_once("data-record-url=\"")
        .or_else(|| workspace.split_once("data-url=\""))
        .and_then(|(_, v)| v.split('"').next())
        .unwrap_or("")
}
pub fn navigation_tree(
    editor: &Editor,
    root: &str,
    workspace: &str,
    show_all_links: bool,
) -> Result<String> {
    fn nodes(
        editor: &Editor,
        items: &[HierarchyNode],
        current: &str,
        path: &str,
        show_all_links: bool,
    ) -> Result<String> {
        let mut html = String::new();
        for n in items.iter().filter(|n| n.branch) {
            let url = record_url(&n.collection, &n.id);
            let key = format!("{path}-{}-{}", encode(&n.collection), encode(&n.id));
            let c = editor.config().collection(&n.collection)?;
            let icon = views::icon_class(c.ui["icon"].as_str(), "ri-folder-line");
            let selected = if url == current {
                " aria-current=\"page\""
            } else {
                ""
            };
            let label = format!(
                "<i class=\"{icon}\" aria-hidden=\"true\"></i><span>{}</span>",
                escape(&n.label)
            );
            fn contains_current(node: &HierarchyNode, current: &str) -> bool {
                record_url(&node.collection, &node.id) == current
                    || node
                        .children
                        .iter()
                        .any(|child| contains_current(child, current))
            }
            let active = contains_current(n, current);
            let selected =
                if n.children.iter().any(|child| {
                    !child.branch && record_url(&child.collection, &child.id) == current
                }) {
                    " aria-current=\"page\""
                } else {
                    selected
                };
            if show_all_links || n.children.iter().any(|child| child.branch) {
                let children = nodes(editor, &n.children, current, &key, show_all_links)?;
                let all_link = if show_all_links {
                    format!("<a data-nav class=\"tree-all\" href=\"{url}\">All</a>")
                } else {
                    String::new()
                };
                html += &format!(
                    "<details class=\"tree-node\" id=\"nav-{key}\" data-active-branch=\"{active}\" {}><summary><a data-nav href=\"{url}\"{selected} data-tooltip=\"{}\">{label}<small>{}</small></a><i class=\"ri-arrow-right-s-line tree-chevron\" aria-hidden=\"true\"></i></summary><div class=\"tree-children\">{all_link}{children}</div></details>",
                    if active { "open" } else { "" },
                    escape(&n.label),
                    n.children.len()
                );
            } else {
                html += &format!(
                    "<a class=\"tree-leaf\" data-nav href=\"{url}\"{selected} data-tooltip=\"{}\">{label}<small>{}</small></a>",
                    escape(&n.label),
                    n.children.len()
                );
            }
        }
        Ok(html)
    }
    nodes(
        editor,
        &editor.hierarchy(root)?,
        active_url(workspace),
        root,
        show_all_links,
    )
}
pub fn parents(editor: &Editor, name: &str, id: &str) -> Result<String> {
    let parents = editor.parent_links(name, id)?;
    if parents.is_empty() {
        return Ok(String::new());
    }
    let mut html =
        "<nav class=\"parent-links\" aria-label=\"Contained in\"><span class=\"eyebrow\">In</span>"
            .to_owned();
    for parent in parents {
        for ancestor in editor.parent_links(&parent.collection, &parent.id)? {
            html += &format!(
                "<a data-nav href=\"{}\">{}</a><i class=\"ri-arrow-right-s-line\" aria-hidden=\"true\"></i>",
                record_url(&ancestor.collection, &ancestor.id),
                escape(&ancestor.label)
            );
        }
        html += &format!(
            "<a class=\"parent-chip\" data-nav href=\"{}\"><i class=\"ri-folder-open-line\" aria-hidden=\"true\"></i>{}</a>",
            record_url(&parent.collection, &parent.id),
            escape(&parent.label)
        );
    }
    html += "</nav>";
    Ok(html)
}
pub fn scoped_url(parent: &str, id: &str, field: &str, target: &str) -> String {
    format!(
        "/collections/{}?parent_collection={}&parent_id={}&parent_field={}",
        encode(target),
        encode(parent),
        encode(id),
        encode(field)
    )
}
pub fn sections(editor: &Editor, parent: &str, id: &str, actor: &Actor) -> Result<String> {
    let doc = editor.get(parent, id)?;
    let mut html = String::new();
    for child in editor.config().collection(parent)?.child_collections() {
        if !editor.allows(&child.collection, "read") {
            continue;
        }
        let c = editor.config().collection(&child.collection)?;
        let ui = editor.child_list_ui(parent, &child.field)?;
        let preview_size = ui
            .pointer("/list/previewSize")
            .and_then(|v| v.as_u64())
            .unwrap_or(10)
            .clamp(1, 200) as usize;
        let url = scoped_url(parent, id, &child.field, &child.collection);
        let base = format!(
            "{}/children/{}",
            record_url(parent, id),
            encode(&child.field)
        );
        let child_ids = editor.child_ids(parent, id, &child.field)?;
        let missing_count = child_ids.len();
        let rows = editor.query_collection_with_ui(
            &child.collection,
            &ListRequest {
                ids: Some(child_ids),
                sort: ui
                    .pointer("/list/defaultSort/field")
                    .and_then(|v| v.as_str())
                    .unwrap_or("_label")
                    .into(),
                descending: ui
                    .pointer("/list/defaultSort/direction")
                    .and_then(|v| v.as_str())
                    == Some("desc"),
                limit: preview_size,
                ..Default::default()
            },
            &ui,
        )?;
        let missing_count = missing_count.saturating_sub(rows.total);
        let editable = actor.editable() && editor.allows(parent, "update");
        html += &format!(
            "<section class=\"surface child-list\" id=\"children-{}\"><div class=\"child-heading\"><div><h2><i class=\"{}\" aria-hidden=\"true\"></i>{}<span class=\"badge mono\">{}</span></h2></div><div class=\"actions\"><a class=\"button quiet small\" data-nav href=\"{}\"><i class=\"ri-list-check\" aria-hidden=\"true\"></i>Browse list</a>",
            encode(&child.field),
            views::icon_class(ui["icon"].as_str(), "ri-folder-line"),
            escape(&child.label),
            rows.total,
            escape(&url)
        );
        if editable {
            html += &format!(
                "<a class=\"button small\" data-nav href=\"{base}/picker\"><i class=\"ri-link\" aria-hidden=\"true\"></i>Link existing</a>"
            );
            if editor.allows(&child.collection, "create") {
                let singular = ui["singularLabel"].as_str().unwrap_or("record");
                html += &format!(
                    "<form method=\"post\" action=\"/collections/{}/edit-sessions\" {SUBMIT}>{}{}{}{}{}<button class=\"small primary\"><i class=\"ri-add-line\" aria-hidden=\"true\"></i>New {}</button></form>",
                    encode(&child.collection),
                    hidden("csrf", &actor.csrf),
                    hidden("parent_collection", parent),
                    hidden("parent_id", id),
                    hidden("parent_field", &child.field),
                    hidden("parent_revision", &doc.revision),
                    escape(singular)
                );
            }
        }
        html += "</div></div>";
        if missing_count > 0 {
            html += &format!(
                "<template data-notification=\"warning\">{missing_count} linked record(s) could not be found. Review this record’s raw JSON to repair the child list.</template>"
            );
        }
        if rows.total == 0 {
            html += &format!(
                "<div class=\"child-empty\"><i class=\"ri-folder-add-line\" aria-hidden=\"true\"></i><span>No {} here yet.</span></div>",
                escape(&child.label)
            );
        } else {
            let columns: Vec<&str> = ui
                .pointer("/list/columns")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().or_else(|| v["field"].as_str()))
                        .collect()
                })
                .unwrap_or_else(|| vec!["_label"]);
            let title = |key: &str| -> String {
                match key {
                    "_label" => "Record".into(),
                    "_id" => "Identifier".into(),
                    _ => c.editor_schema["properties"][key]["title"]
                        .as_str()
                        .unwrap_or(key)
                        .into(),
                }
            };
            html +=
                "<div class=\"table-scroll\"><table class=\"data-table child-table\"><thead><tr>";
            for key in &columns {
                html += &format!(
                    "<th><a data-nav href=\"{}&amp;sort={}\">{}<i class=\"ri-arrow-up-down-line\" aria-hidden=\"true\"></i></a></th>",
                    escape(&url),
                    encode(key),
                    escape(&title(key))
                );
            }
            html += "<th class=\"row-actions-heading\"><span class=\"sr-only\">Actions</span></th></tr></thead><tbody>";
            for row in rows.rows {
                let record = record_url(&child.collection, &row.id);
                html += "<tr>";
                for (index, key) in columns.iter().enumerate() {
                    let text = match *key {
                        "_label" => row.label.clone(),
                        "_id" => row.id.clone(),
                        _ => row.display.get(*key).cloned().unwrap_or_else(|| "—".into()),
                    };
                    html += &format!("<td data-label=\"{}\">", escape(&title(key)));
                    if index == 0 {
                        html += &format!(
                            "<a class=\"record-link\" data-nav href=\"{record}\">{}</a>",
                            escape(&text)
                        );
                    } else {
                        html += &escape(&text);
                    }
                    html += "</td>";
                }
                html += "<td class=\"row-actions\"><div class=\"row-action-buttons\">";
                if actor.editable() && editor.allows(&child.collection, "update") {
                    html += &format!(
                        "<a class=\"icon-button\" data-nav href=\"{record}/edit\" data-tooltip=\"Edit\" aria-label=\"Edit {}\"><i class=\"ri-edit-line\" aria-hidden=\"true\"></i></a>",
                        escape(&row.label)
                    );
                }
                html += &record_actions(
                    editor,
                    actor,
                    &child.collection,
                    &row.id,
                    &row.label,
                    editable.then_some((parent, id, child.field.as_str(), doc.revision.as_str())),
                )?;
                html += "</div></td></tr>";
            }
            html += "</tbody></table></div>";
            if rows.total > preview_size {
                html += &format!(
                    "<div class=\"child-footer\"><span class=\"muted\">Showing {preview_size} of {}</span><a data-nav href=\"{}\">View all {} <i class=\"ri-arrow-right-line\" aria-hidden=\"true\"></i></a></div>",
                    rows.total,
                    escape(&url),
                    rows.total
                );
            }
        }
        html += "</section>";
    }
    Ok(html)
}
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/collections/{name}/records/{id}/children/{field}/picker",
            get(picker),
        )
        .route(
            "/collections/{name}/records/{id}/children/{field}/{action}",
            post(membership),
        )
}
async fn picker(
    State(state): State<Arc<AppState>>,
    Path((name, id, field)): Path<(String, String, String)>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response> {
    let actor = match page_actor(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return Ok(r),
    };
    blocking(move || {
        state.editor.require_operation(&name,"update")?;
        if !actor.editable(){return Err(WebError(axum::http::StatusCode::FORBIDDEN,"Read-only account".into()));}
        let e=&state.editor; let child=e.child_collection(&name,&field)?;let doc=e.get(&name,&id)?; let ids=e.child_ids(&name,&id,&field)?;
        let q=query.get("q").cloned().unwrap_or_default();
        let rows=e.query_collection(&child.collection,&ListRequest {query:q.clone(),sort:"_label".into(),limit:200,..Default::default()})?;
        let back=record_url(&name,&id);let url=format!("{back}/children/{}/picker",encode(&field));
        let mut html=views::record(e,&name,&id,&actor)?;
        html=html.replacen(&format!("data-url=\"{back}\""),&format!("data-url=\"{}?q={}\"",url,encode(&q)),1);
        let mut modal=format!("<server-picker><section class=\"picker\" role=\"dialog\" aria-modal=\"true\" aria-labelledby=\"child-picker-title\"><div class=\"picker-heading\"><h2 id=\"child-picker-title\">Link existing {}</h2><a class=\"icon-button\" data-nav data-close-picker href=\"{back}\" aria-label=\"Close picker\"><i class=\"ri-close-line\" aria-hidden=\"true\"></i></a></div><p class=\"help\">Choose an existing record to include in this list. Its other placements stay intact.</p><form action=\"{url}\" data-on:submit__prevent=\"@get(el.getAttribute('action')+'?'+new URLSearchParams(new FormData(el)),{{filterSignals:{{include:/^$/}}}})\"><div class=\"search\"><input type=\"search\" name=\"q\" value=\"{}\" placeholder=\"Search records…\" aria-label=\"Search records\"><button>Search</button></div></form><div class=\"picker-results\">",escape(&child.label),escape(&q));
        let mut count=0;
        for row in rows.rows.into_iter().filter(|r|!ids.contains(&r.id)) {count+=1;modal+=&format!("<form method=\"post\" action=\"{back}/children/{}/link\" {SUBMIT}>{}{}{}<button class=\"picker-result\"><span><strong>{}</strong><small class=\"mono\">{}</small></span><i class=\"ri-add-line\" aria-hidden=\"true\"></i></button></form>",encode(&field),hidden("csrf",&actor.csrf),hidden("revision",&doc.revision),hidden("child_id",&row.id),escape(&row.label),escape(&row.id));}
        if count==0 {modal+="<p class=\"empty-state\">No available records match this search.</p>";}
        if rows.matched>200 {modal+="<p class=\"help\">Showing the first 200 matches. Refine your search to find more.</p>";}
        modal+="</div></section></server-picker>";
        html.insert_str(html.rfind("</main>").unwrap(),&modal);page(&state,&actor,&headers,html)
    }).await
}
async fn membership(
    State(state): State<Arc<AppState>>,
    Path((name, id, field, action)): Path<(String, String, String, String)>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Response> {
    let actor = mutation_actor(&state, &headers, &form).await?;
    blocking(move || {
        let present = match action.as_str() {
            "link" => true,
            "unlink" => false,
            _ => return Err(WebError::bad("Unknown child-list action")),
        };
        state.editor.set_child_membership(
            &name,
            &id,
            &field,
            form.get("child_id")
                .ok_or_else(|| WebError::bad("Missing child ID"))?,
            present,
            form.get("revision")
                .ok_or_else(|| WebError::bad("Missing parent revision"))?,
        )?;
        page(
            &state,
            &actor,
            &headers,
            views::record(&state.editor, &name, &id, &actor)?,
        )
    })
    .await
}

/// Unlink changes the parent list; delete opens the target record's reference review.
pub(crate) fn record_actions(
    editor: &Editor,
    actor: &Actor,
    target: &str,
    id: &str,
    label: &str,
    placement: Option<(&str, &str, &str, &str)>,
) -> Result<String> {
    if !actor.editable() {
        return Ok(String::new());
    }
    let mut actions = String::new();
    if let Some((parent, parent_id, field, revision)) = placement {
        if editor.allows(parent, "update") {
            let base = format!(
                "{}/children/{}",
                record_url(parent, parent_id),
                encode(field)
            );
            actions += &format!(
                "<form method=\"post\" action=\"{base}/unlink\" {SUBMIT}>{}{}{}<p>Unlinking updates this list immediately. The record remains available in its collection.</p><button class=\"menu-action\"><i class=\"ri-link-unlink\" aria-hidden=\"true\"></i>Unlink from this list</button></form>",
                hidden("csrf", &actor.csrf),
                hidden("revision", revision),
                hidden("child_id", id)
            );
        }
    }
    if editor.allows(target, "delete") && editor.allows(target, "read") {
        actions += &format!(
            "<a class=\"menu-action danger\" data-nav href=\"{}/delete\" data-tooltip=\"Delete the record itself after reviewing its references\"><i class=\"ri-delete-bin-line\" aria-hidden=\"true\"></i>Delete {}…</a>",
            record_url(target, id),
            escape(&views::record_kind(editor, target)?)
        );
    }
    if actions.is_empty() {
        return Ok(actions);
    }
    Ok(format!(
        "<details class=\"action-menu\"><summary class=\"icon-button\" aria-label=\"Actions for {}\"><i class=\"ri-more-2-line\" aria-hidden=\"true\"></i></summary><div class=\"menu-panel\">{actions}</div></details>",
        escape(label)
    ))
}
