pub mod list;
mod preview;
pub mod validation;
use crate::{
    assets::Assets,
    auth::Actor,
    error::Result,
    fields::{Context, Registry},
    sessions::EditSession,
};
use askama::Template;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use structured_editor_core::{Editor, ValidationIssue};
pub fn encode(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}
pub fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
pub fn record_url(collection: &str, id: &str) -> String {
    format!("/collections/{}/records/{}", encode(collection), encode(id))
}
#[derive(Template)]
#[template(path = "shell.html")]
struct Shell<'a> {
    actor: &'a Actor,
    navigation: String,
    content: &'a str,
    assets: &'a str,
    preferences: String,
}
pub fn navigation(editor: &Editor, workspace: &str) -> Result<String> {
    // The section key is emitted by our workspace templates, never inferred by browser code.
    let section = workspace
        .split_once("data-section=\"")
        .and_then(|(_, rest)| rest.split('"').next())
        .unwrap_or("");
    let workspace_url = workspace
        .split_once("data-url=\"")
        .and_then(|(_, v)| v.split('"').next())
        .unwrap_or("");
    let overview_current = |url: &str| {
        if workspace_url.split('?').next() == Some(url) && !workspace.contains("data-record-url=\"")
        {
            " aria-current=\"page\""
        } else {
            ""
        }
    };
    let current = |url: &str| {
        if section == url || section.starts_with(&format!("{url}/")) {
            " aria-current=\"page\""
        } else {
            ""
        }
    };
    let mut items: Vec<(
        i64,
        i64,
        String,
        String,
        String,
        String,
        String,
        String,
        bool,
    )> = Vec::new();
    for (name, c) in &editor.config().collections {
        if !c.show_in_sidebar || !editor.allows(name, "read") {
            continue;
        }
        let ui = if c.ui.is_object() {
            &c.ui
        } else {
            &editor.config().ui["collections"][name]
        };
        let group = ui["sidebarGroup"].as_str().unwrap_or("Content");
        let group_order = editor.config().ui["groups"]
            .as_array()
            .and_then(|v| v.iter().find(|g| g["name"] == group))
            .and_then(|g| g["position"].as_i64())
            .unwrap_or(0);
        let all_links = ui["sidebarAllLinks"].as_str().unwrap_or("none");
        let tree = if !c.child_collections().is_empty() {
            crate::hierarchy::navigation_tree(editor, name, workspace, all_links == "all")?
        } else {
            tree_navigation(editor, name, all_links == "all")?
        };
        items.push((
            group_order,
            ui["position"].as_i64().unwrap_or(0),
            group.into(),
            ui["sidebarLabel"].as_str().unwrap_or(&c.label).into(),
            format!("/collections/{}", encode(name)),
            tree,
            ui["allCollection"]
                .as_str()
                .filter(|n| {
                    editor.config().collections.contains_key(*n) && editor.allows(n, "read")
                })
                .map(|n| format!("/collections/{}", encode(n)))
                .unwrap_or_else(|| format!("/collections/{}", encode(name))),
            icon_class(
                ui["sidebarIcon"].as_str().or_else(|| ui["icon"].as_str()),
                "ri-stack-line",
            ),
            all_links != "none",
        ));
    }
    for (name, c) in &editor.config().media_collections {
        if c.ui["showInSidebar"] == false || !editor.allows(name, "read") {
            continue;
        }
        let group = c.ui["sidebarGroup"].as_str().unwrap_or("Files");
        items.push((
            editor.config().ui["groups"]
                .as_array()
                .and_then(|v| v.iter().find(|g| g["name"] == group))
                .and_then(|g| g["position"].as_i64())
                .unwrap_or(1),
            c.ui["position"].as_i64().unwrap_or(0),
            group.into(),
            c.label.clone(),
            format!("/media/{}", encode(name)),
            String::new(),
            String::new(),
            icon_class(c.ui["icon"].as_str(), "ri-folder-image-line"),
            false,
        ));
    }
    items.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.2.cmp(&b.2))
            .then(a.1.cmp(&b.1))
            .then(structured_editor_core::alphabetic_cmp(&a.3, &b.3))
    });
    let repository = editor
        .config()
        .root
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let mut html = format!(
        "<aside id=\"project-navigation\"><div class=\"repository-heading\"><i class=\"ri-git-repository-line\" aria-hidden=\"true\"></i><div><span class=\"eyebrow\">Repository</span><strong>{}</strong></div></div><nav aria-label=\"Repository navigation\">",
        escape(&repository)
    );
    let mut last = String::new();
    for (_, _, group, label, url, tree, all_url, icon, show_root_all) in items {
        if group != last {
            html += &format!(
                "<p class=\"eyebrow\">{}</p>",
                escape(
                    editor.config().ui["groups"]
                        .as_array()
                        .and_then(|v| v.iter().find(|g| g["name"] == group))
                        .and_then(|g| g["label"].as_str())
                        .unwrap_or(&group)
                )
            );
            last = group;
        }
        if tree.is_empty() {
            html += &format!(
                "<a data-nav href=\"{url}\"{}><i class=\"{icon}\" aria-hidden=\"true\"></i><span>{}</span></a>",
                current(&url),
                escape(&label)
            );
        } else {
            let all_link = if show_root_all {
                format!(
                    "<a class=\"tree-all\" data-nav href=\"{all_url}\"{}>All</a>",
                    overview_current(&all_url)
                )
            } else {
                String::new()
            };
            html += &format!(
                "<details class=\"tree-root\" id=\"nav-tree-{}\" {}><summary><a data-nav href=\"{url}\"{}><i class=\"{icon}\" aria-hidden=\"true\"></i><span>{}</span></a><i class=\"ri-arrow-right-s-line tree-chevron\" aria-hidden=\"true\"></i></summary><div class=\"tree-children\">{all_link}{tree}</div></details>",
                encode(&url),
                if !current(&url).is_empty()
                    || !overview_current(&all_url).is_empty()
                    || tree.contains("data-active-branch=\"true\"")
                {
                    "open"
                } else {
                    ""
                },
                overview_current(&url),
                escape(&label),
            );
        }
    }
    html += &format!(
        "<p class=\"eyebrow\">Workspace</p><a data-nav href=\"/documents\"{}><i class=\"ri-braces-line\" aria-hidden=\"true\"></i><span>JSON documents</span></a><a data-nav href=\"/settings\"{}><i class=\"ri-settings-3-line\" aria-hidden=\"true\"></i><span>Editor settings</span></a></nav><div class=\"sidebar-footer mono\"><span class=\"status-dot\"></span>Repository workspace</div></aside>",
        current("/documents"),
        current("/settings")
    );
    Ok(html)
}
fn tree_navigation(editor: &Editor, name: &str, show_all_links: bool) -> Result<String> {
    fn nodes(
        name: &str,
        items: &[structured_editor_core::CategoryNode],
        path: &mut Vec<String>,
        html: &mut String,
        show_all_links: bool,
    ) {
        for n in items {
            path.push(n.value.clone());
            let tree = encode(&serde_json::to_string(path).unwrap());
            let url = format!("/collections/{}?tree={}", encode(name), tree);
            let label = format!(
                "<span>{}</span><small>{}</small>",
                escape(&n.label),
                n.count
            );
            if n.children.is_empty() {
                *html += &format!("<a class=\"tree-leaf\" data-nav href=\"{url}\">{label}</a>");
            } else {
                let all_link = if show_all_links {
                    format!("<a class=\"tree-all\" data-nav href=\"{url}\">All</a>")
                } else {
                    String::new()
                };
                *html += &format!(
                    "<details class=\"tree-node\" id=\"tree-{}-{tree}\"><summary><a data-nav href=\"{url}\">{label}</a><i class=\"ri-arrow-right-s-line tree-chevron\" aria-hidden=\"true\"></i></summary><div class=\"tree-children\">{all_link}",
                    encode(name),
                );
                nodes(name, &n.children, path, html, show_all_links);
                *html += "</div></details>";
            }
            path.pop();
        }
    }
    let categories = editor.categories(name)?;
    if categories.is_empty() {
        return Ok(String::new());
    };
    let mut html = String::new();
    nodes(
        name,
        &categories,
        &mut Vec::new(),
        &mut html,
        show_all_links,
    );
    Ok(html)
}
pub fn shell(editor: &Editor, actor: &Actor, content: &str, assets: &Assets) -> Result<String> {
    Ok(Shell {
        actor,
        navigation: navigation(editor, content)?,
        content,
        assets: &assets.tags,
        preferences: crate::preferences::fragment(editor),
    }
    .render()?)
}
#[derive(Template)]
#[template(path = "login.html")]
struct Login<'a> {
    assets: &'a str,
    message: &'a str,
}
pub fn login(assets: &Assets, message: &str) -> Result<String> {
    Ok(Login {
        assets: &assets.tags,
        message,
    }
    .render()?)
}
pub fn collection(
    editor: &Editor,
    name: &str,
    query: &str,
    offset: usize,
    actor: &Actor,
) -> Result<String> {
    collection_tree(editor, name, query, offset, actor, &[])
}
pub fn collection_tree(
    editor: &Editor,
    name: &str,
    query: &str,
    offset: usize,
    actor: &Actor,
    path: &[String],
) -> Result<String> {
    list::render(
        editor,
        name,
        &std::collections::HashMap::from([
            ("q".into(), query.into()),
            ("offset".into(), offset.to_string()),
            ("tree".into(), serde_json::to_string(path).unwrap()),
        ]),
        actor,
    )
}
#[derive(Template)]
#[template(path = "record.html")]
struct RecordView<'a> {
    title: String,
    url: String,
    back: String,
    content: String,
    edit: String,
    delete: String,
    actor: &'a Actor,
    references: Vec<String>,
    children: String,
    parents: String,
    can_update: bool,
    can_delete: bool,
}
pub fn record(editor: &Editor, name: &str, id: &str, actor: &Actor) -> Result<String> {
    editor.require_operation(name, "read")?;
    let doc = editor.get(name, id)?;
    let projected = editor.view_value(name, &doc.value)?;
    let draft = editor.draft(name, &doc.value)?;
    let mut content = String::new();
    for f in &draft.fields {
        if f.hidden {
            continue;
        }
        let Some(value) = projected.get(&f.key) else {
            continue;
        };
        let label = f.schema["title"].as_str().unwrap_or(&f.key);
        if f.key == "_editor" {
            content += &format!(
                "<details class=\"preview-metadata\"><summary>Record metadata</summary><pre class=\"mono\">{}</pre></details>",
                escape(&serde_json::to_string_pretty(value).unwrap_or_default())
            );
            continue;
        }
        content += &format!(
            "<section class=\"preview-field\"><h3>{}</h3><div>",
            escape(label)
        );
        content += &preview::value(editor, f, value);
        content += "</div></section>";
    }
    let url = record_url(name, id);
    let parent_links = editor.parent_links(name, id)?;
    Ok(RecordView {
        title: editor.label(name, &doc.value, id, "record")?,
        can_update: actor.editable() && editor.allows(name, "update"),
        can_delete: actor.editable() && editor.allows(name, "delete"),
        url: url.clone(),
        back: format!("/collections/{}", encode(name)),
        content,
        children: crate::hierarchy::sections(editor, name, id, actor)?,
        parents: crate::hierarchy::parents(editor, name, id)?,
        edit: format!("{url}/edit-sessions"),
        delete: format!("{url}/delete"),
        actor,
        references: editor
            .references_to(name, id)?
            .iter()
            .filter(|r| {
                !parent_links.iter().any(|p| {
                    p.collection == r.collection
                        && p.id == r.record_id
                        && r.path.starts_with(&format!(
                            "/{}/",
                            p.field.replace('~', "~0").replace('/', "~1")
                        ))
                })
            })
            .map(|r| {
                format!(
                    "<a data-nav href=\"{}\">{}/{}</a> at {}",
                    record_url(&r.collection, &r.record_id),
                    escape(&r.collection),
                    escape(&r.record_id),
                    escape(&r.path)
                )
            })
            .collect(),
    }
    .render()?)
}
#[derive(Template)]
#[template(path = "edit.html")]
struct Edit<'a> {
    session: &'a EditSession,
    actor: &'a Actor,
    fields: String,
    metadata: String,
    sidebar_fields: String,
    available_fields: String,
    section: String,
    collection_label: String,
    record_url: String,
    title: String,
    issues: &'a [ValidationIssue],
    message: &'a str,
    picker: String,
    back: String,
}
pub fn edit(
    editor: &Editor,
    session: &EditSession,
    actor: &Actor,
    registry: &Registry,
    issues: &[ValidationIssue],
    message: &str,
) -> Result<String> {
    let value = session.value().unwrap_or(serde_json::json!({}));
    let context = Context {
        editor,
        collection: &session.collection,
        value: &value,
        preview: &session.preview,
        session: &session.id,
        version: session.version,
        issues,
    };
    let mut fields = String::new();
    let mut metadata = String::new();
    let mut sidebar_fields = String::new();
    let mut available_fields = String::new();
    let ui = &editor.config().collection(&session.collection)?.ui;
    for field in &session.draft.fields {
        if !field.present && !field.hidden && !field.read_only && field.key != "_editor" {
            available_fields += &format!(
                "<button class=\"menu-action add-field-option\" formaction=\"/edit-sessions/{}/fields/{}/actions/include\"><i class=\"ri-add-line\" aria-hidden=\"true\"></i><span>{}</span><small>{}</small></button>",
                session.id,
                field.id,
                escape(field.schema["title"].as_str().unwrap_or(&field.key)),
                escape(
                    field.schema["x-editor"]
                        .as_str()
                        .or_else(|| field.schema["type"].as_str())
                        .unwrap_or("value")
                )
            );
        }
        let destination = if field.key == "_editor" {
            &mut metadata
        } else if ui
            .pointer("/edit/sidebarFields")
            .and_then(|v| v.as_array())
            .is_some_and(|fields| fields.iter().any(|v| v.as_str() == Some(&field.key)))
        {
            &mut sidebar_fields
        } else {
            &mut fields
        };
        destination.push_str(&registry.render(
            field,
            &format!("/{}", field.key.replace('~', "~0").replace('/', "~1")),
            &context,
            false,
        )?);
    }
    Ok(Edit {
        session,
        actor,
        fields,
        metadata,
        sidebar_fields,
        available_fields,
        section: format!("/collections/{}", encode(&session.collection)),
        record_url: session
            .parent
            .as_ref()
            .map(|p| record_url(&p.collection, &p.id))
            .unwrap_or_else(|| record_url(&session.collection, &session.record_id)),
        collection_label: if let Some(parent) = &session.parent {
            editor.label(
                &parent.collection,
                &editor.get(&parent.collection, &parent.id)?.value,
                &parent.id,
                "record",
            )?
        } else {
            editor
                .config()
                .collection(&session.collection)?
                .label
                .clone()
        },
        title: if session.record_id.is_empty() {
            format!("New {}", ui["singularLabel"].as_str().unwrap_or("record"))
        } else {
            editor.label(&session.collection, &value, &session.record_id, "record")?
        },
        issues,
        message,
        picker: crate::pickers::render(editor, session)?,
        back: if let Some(parent) = &session.parent {
            record_url(&parent.collection, &parent.id)
        } else if let Some((parent, _)) = &session.return_to {
            format!("/edit-sessions/{parent}")
        } else if session.revision.is_some() {
            record_url(&session.collection, &session.record_id)
        } else {
            format!("/collections/{}", encode(&session.collection))
        },
    }
    .render()?)
}
pub fn notice(message: &str) -> String {
    format!(
        "<div id=\"notification-region\" role=\"status\">{}</div>",
        escape(message)
    )
}
/// Datastar is the only DOM patch implementation. Emit complete, short-lived SSE batches.
pub fn patches(fragments: &[String]) -> String {
    patches_mode(fragments, "outer")
}
pub(crate) fn patches_mode(fragments: &[String], mode: &str) -> String {
    fragments
        .iter()
        .map(|html| {
            format!(
                "event: datastar-patch-elements\ndata: mode {mode}\n{}\n",
                html.lines()
                    .map(|line| format!("data: elements {line}\n"))
                    .collect::<String>()
            )
        })
        .collect()
}

#[derive(Template)]
#[template(path = "panel.html")]
struct Panel<'a> {
    url: &'a str,
    title: &'a str,
    body: &'a str,
    section: &'a str,
    category: &'a str,
}
pub fn panel(url: &str, title: &str, body: &str) -> Result<String> {
    let section = url.split('?').next().unwrap_or(url);
    let category = if url.starts_with("/media/") {
        "Files / Media collection"
    } else {
        "Workspace"
    };
    Ok(Panel {
        url,
        title,
        body,
        section,
        category,
    }
    .render()?)
}

pub fn markdown(source: &str) -> String {
    let mut html = String::new();
    let events = pulldown_cmark::Parser::new(source).map(|mut event| {
        if let pulldown_cmark::Event::Start(pulldown_cmark::Tag::Link { dest_url, .. }) = &mut event
        {
            if let Some(path) = dest_url.strip_prefix("#/collections/") {
                if let Some((collection, id)) = path.split_once('/') {
                    *dest_url = format!("/collections/{collection}/records/{id}").into();
                }
            }
        }
        event
    });
    pulldown_cmark::html::push_html(&mut html, events);
    ammonia::clean(&html)
}

pub fn icon_class(value: Option<&str>, fallback: &str) -> String {
    value
        .filter(|v| {
            v.starts_with("ri-")
                && v.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
        .unwrap_or(fallback)
        .into()
}
