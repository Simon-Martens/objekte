pub(crate) mod embedded;
pub mod list;
pub(crate) mod markdown_source;
pub(crate) mod metadata;
pub(crate) mod preview;
pub(crate) mod record_table;
pub(crate) mod references;
pub(crate) mod search;
pub(crate) mod table;
pub mod validation;
use crate::templates::Template;
use crate::{
    assets::Assets,
    auth::Actor,
    error::Result,
    fields::{Context, Registry},
    sessions::EditSession,
};
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
crate::templates::template!(Shell<'_>, "shell.html");
#[derive(serde::Serialize)]
struct Shell<'a> {
    actor: crate::templates::ActorContext<'a>,
    navigation: String,
    content: &'a str,
    assets: &'a str,
    preferences: String,
    language_picker: String,
}
pub fn navigation(editor: &Editor, workspace: &str) -> Result<String> {
    navigation_with_heading(editor, workspace, None)
}
fn navigation_with_heading(
    editor: &Editor,
    workspace: &str,
    heading: Option<&str>,
) -> Result<String> {
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
        let tree = if ui["sidebarTree"] == false {
            String::new()
        } else if !c.child_collections().is_empty() {
            crate::hierarchy::navigation_tree(editor, name, workspace, all_links == "all")?
        } else {
            tree_navigation(editor, name, all_links == "all")?
        };
        items.push((
            group_order,
            ui["position"].as_i64().unwrap_or(0),
            group.into(),
            crate::i18n::text(ui, "sidebarLabel", crate::i18n::text(ui, "label", &c.label)).into(),
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
    let default_heading = format!(
        "<a href=\"/\" data-nav class=\"repository-heading\" aria-label=\"Repository root\"><i class=\"ri-git-repository-line\" aria-hidden=\"true\"></i><div><span class=\"eyebrow\">Repository</span><strong>{}</strong></div></a>",
        escape(&repository)
    );
    let mut html = format!(
        "<aside id=\"project-navigation\">{}<nav aria-label=\"{ui_text_0}\">",
        heading.unwrap_or(&default_heading), ui_text_0 = crate::views::escape(&crate::i18n::t("Repository navigation")));
    let mut last = String::new();
    for (_, _, group, label, url, tree, all_url, icon, show_root_all) in items {
        if group != last {
            html += &format!(
                "<p class=\"eyebrow\">{}</p>",
                escape(
                    editor.config().ui["groups"]
                        .as_array()
                        .and_then(|v| v.iter().find(|g| g["name"] == group))
                        .map(|g| crate::i18n::text(g, "label", &group))
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
    if let Some(tree) = workspace
        .split("<template id=\"data-navigation\">")
        .nth(1)
        .and_then(|s| s.split("</template>").next())
    {
        html += tree;
    }
    html += &format!(
        "<p class=\"eyebrow\">{ui_text_0}</p><a data-nav href=\"/documents\"{}><i class=\"ri-braces-line\" aria-hidden=\"true\"></i><span>{ui_text_1}</span></a><a data-nav href=\"/settings\"{}><i class=\"ri-settings-3-line\" aria-hidden=\"true\"></i><span>{ui_text_2}</span></a></nav><div class=\"sidebar-footer mono\"><span class=\"status-dot\"></span>{ui_text_3}</div></aside>",
        current("/documents"),
        current("/settings"), ui_text_0 = crate::views::escape(&crate::i18n::t("Workspace")), ui_text_1 = crate::views::escape(&crate::i18n::t("JSON documents")), ui_text_2 = crate::views::escape(&crate::i18n::t("Editor settings")), ui_text_3 = crate::views::escape(&crate::i18n::t("Repository workspace")));
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
pub(crate) fn project_navigation(
    state: &crate::AppState,
    actor: &Actor,
    content: &str,
) -> Result<String> {
    navigation_with_heading(
        &state.editor,
        content,
        Some(&crate::projects::chooser(state, actor)),
    )
}
pub fn shell(state: &crate::AppState, actor: &Actor, content: &str) -> Result<String> {
    let editor = &state.editor;
    let assets = &state.assets;
    Ok(Shell {
        actor: actor.into(),
        navigation: project_navigation(state, actor, content)?,
        content,
        assets: &assets.tags,
        preferences: crate::preferences::fragment(editor),
        language_picker: crate::i18n::chooser(state, actor),
    }
    .render()?)
}
crate::templates::template!(Login<'_>, "login.html");
#[derive(serde::Serialize)]
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
crate::templates::template!(RecordView<'_>, "record.html");
#[derive(serde::Serialize)]
struct RecordView<'a> {
    record_kind: String,
    view_url: String,
    edit_link: String,
    title: String,
    url: String,
    back: String,
    back_label: String,
    content: String,
    delete: String,
    actor: crate::templates::ActorContext<'a>,
    references: String,
    children: String,
    parents: String,
    can_delete: bool,
    can_update: bool,
}
pub(crate) fn record_kind(editor: &Editor, name: &str) -> Result<String> {
    Ok(crate::i18n::text(&editor.config().collection(name)?.ui, "singularLabel", "record").to_owned())
}
pub fn record(editor: &Editor, name: &str, id: &str, actor: &Actor) -> Result<String> {
    record_at(editor, name, id, actor, &std::collections::HashMap::new())
}
pub fn record_at(
    editor: &Editor,
    name: &str,
    id: &str,
    actor: &Actor,
    params: &std::collections::HashMap<String, String>,
) -> Result<String> {
    let path = params.get("node").map(String::as_str).unwrap_or("");
    editor.require_operation(name, "read")?;
    let doc = editor.get(name, id)?;
    let projected = editor.view_value(name, &doc.value)?;
    let draft = editor.draft(name, &doc.value)?;
    let url = record_url(name, id);
    let content = crate::tree::preview_with_options(
        editor,
        &draft,
        &projected,
        &url,
        path,
        params,
        actor.editable() && editor.allows(name, "update"),
    );
    Ok(RecordView {
        record_kind: record_kind(editor, name)?,
        view_url: table::query_url(&url, params),
        edit_link: format!("{}/edit?node={}", record_url(name, id), encode(path)),
        title: editor.label(name, &doc.value, id, "record")?,
        can_delete: actor.editable() && editor.allows(name, "delete"),
        can_update: actor.editable() && editor.allows(name, "update"),
        url: url.clone(),
        back: format!("/collections/{}", encode(name)),
        back_label: crate::i18n::collection(editor, name),
        content,
        children: crate::hierarchy::sections(editor, name, id, actor, params)?,
        parents: crate::hierarchy::parents(editor, name, id)?,
        delete: format!("{url}/delete"),
        actor: actor.into(),
        // This is a record-level tool; table/row navigation should not scan the
        // catalogue or display links to the parent record below a nested item.
        references: if path.is_empty() {
            format!("<record-references hidden id=\"references-{}-{}-{}\" data-ignore-morph data-url=\"{url}/references\"></record-references>", encode(name), encode(id), doc.revision)
        } else { String::new() },
    }
    .render()?)
}
crate::templates::template!(Edit<'_>, "edit.html");
#[derive(serde::Serialize)]
struct Edit<'a> {
    session: SessionContext<'a>,
    edit_url: String,
    draft_key: String,
    dirty: bool,
    close_requested: bool,
    can_undo: bool,
    can_redo: bool,
    actor: crate::templates::ActorContext<'a>,
    fields: String,
    tables: String,
    table_page: usize,
    metadata: String,
    schema_revision: Option<String>,
    sidebar_fields: String,
    available_fields: String,
    section: String,
    collection_label: String,
    record_url: String,
    title: String,
    issues: &'a [ValidationIssue],
    message: &'a str,
    message_kind: &'a str,
    picker: String,
    back: String,
}
#[derive(serde::Serialize)]
struct SessionContext<'a> {
    id: &'a str,
    version: u64,
    proposed_id: &'a str,
    raw: &'a Option<String>,
    schema_warning: bool,
    document: bool,
    nested: bool,
    creating: bool,
}
pub fn edit(
    editor: &Editor,
    session: &EditSession,
    actor: &Actor,
    registry: &Registry,
    issues: &[ValidationIssue],
    message: &str,
) -> Result<String> {
    edit_with_kind(editor, session, actor, registry, issues, message, "success")
}
pub fn edit_with_kind(
    editor: &Editor,
    session: &EditSession,
    actor: &Actor,
    registry: &Registry,
    issues: &[ValidationIssue],
    message: &str,
    message_kind: &str,
) -> Result<String> {
    let value = session.value().unwrap_or_else(|_| {
        if session.draft.root_value {
            session
                .draft
                .fields
                .first()
                .map(crate::tree::snapshot)
                .unwrap_or(serde_json::Value::Null)
        } else {
            serde_json::Value::Object(
                session
                    .draft
                    .fields
                    .iter()
                    .filter(|f| f.present)
                    .map(|f| (f.key.clone(), crate::tree::snapshot(f)))
                    .collect(),
            )
        }
    });
    let selected = session
        .draft
        .at_path(&session.view_path)
        .filter(|f| crate::tree::navigable(f));
    let context = Context {
        focus: selected.map(|f| f.id.as_str()).unwrap_or(""),
        editor,
        collection: &session.collection,
        value: &value,
        preview: &session.preview,
        session: &session.id,
        version: session.version,
        table_offsets: &session.table_offsets,
        table_search: &session.table_search,
        revealed_fields: &session.revealed_fields,
        issues,
    };
    let mut fields = String::new();
    let mut metadata = String::new();
    let mut secondary = String::new();
    let mut optional = String::new();
    let mut tables = String::new();
    let schema_revision = session
        .draft
        .fields
        .iter()
        .find(|field| field.key == "_editor")
        .and_then(|field| serde_json::from_str::<serde_json::Value>(&field.raw).ok())
        .and_then(|metadata| {
            metadata["schema_revision"]
                .as_str()
                .map(|revision| revision.chars().take(8).collect::<String>())
        });
    let mut sidebar_fields = String::new();
    let mut available_fields = String::new();
    let document_ui;
    let ui = if let Some((name, _)) = &session.document {
        document_ui = editor.document_ui(name)?;
        &document_ui
    } else {
        &editor.config().collection(&session.collection)?.ui
    };
    let shown: Vec<_> = selected
        .map(|f| vec![f])
        .unwrap_or_else(|| session.draft.fields.iter().collect());
    for field in shown {
        if !field.present
            && !field.hidden
            && !field.read_only
            && !crate::fields::required_control(field)
            && field.key != "_editor"
        {
            available_fields += &format!(
                "<button class=\"menu-action add-field-option\" formaction=\"/edit-sessions/{}/fields/{}/actions/include\"><span>{}</span></button>",
                session.id,
                field.id,
                escape(crate::i18n::field(&field.schema, &field.key))
            );
        }
        let path = if selected.is_some() {
            session.view_path.clone()
        } else if session.draft.root_value {
            String::new()
        } else {
            structured_editor_core::draft::child_path("", &field.key)
        };
        let destination = if field.key == "_editor" {
            &mut metadata
        } else if embedded::is_table(field) {
            &mut tables
        } else if !field.required && !session.revealed_fields.contains(&field.id) && field.schema["x-presentation"]["secondary"] == true {
            &mut secondary
        } else if selected.is_none() && crate::fields::empty_optional(field, &path, &context) {
            &mut optional
        } else if ui
            .pointer("/edit/sidebarFields")
            .and_then(|v| v.as_array())
            .is_some_and(|fields| fields.iter().any(|v| v.as_str() == Some(&field.key)))
        {
            &mut sidebar_fields
        } else {
            &mut fields
        };
        destination.push_str(&registry.render(field, &path, &context, false)?);
    }
    fields += &crate::fields::details(&crate::i18n::t("More fields"), &optional);
    fields += &crate::fields::details(&crate::i18n::t("Additional details"), &secondary);
    // Root system controls remain available while editing another subtree.
    if selected.is_some() && metadata.is_empty() {
        if let Some(field) = session.draft.fields.iter().find(|f| f.key == "_editor") {
            metadata = registry.render(field, "/_editor", &context, false)?;
        }
    }
    fields = format!(
        "{}{}{}",
        crate::tree::edit_navigation(session),
        crate::tree::edit_breadcrumbs(editor, session),
        fields
    );
    Ok(Edit {
        session: SessionContext {
            id: &session.id,
            version: session.version,
            proposed_id: &session.proposed_id,
            raw: &session.raw,
            schema_warning: session.schema_warning,
            document: session.document.is_some(),
            nested: selected.is_some(),
            creating: session.record_id.is_empty(),
        },
        edit_url: session.edit_url(),
        draft_key: session.edit_key(),
        dirty: session.dirty(),
        close_requested: session.close_requested,
        can_undo: session.history.can_undo(),
        can_redo: session.history.can_redo(),
        actor: actor.into(),
        fields,
        tables,
        table_page: selected
            .filter(|f| embedded::is_table(f))
            .and_then(|f| session.table_offsets.get(&f.id))
            .copied()
            .unwrap_or(0),
        metadata,
        schema_revision,
        sidebar_fields,
        available_fields,
        section: if session.document.is_some() {
            "/documents".into()
        } else {
            format!("/collections/{}", encode(&session.collection))
        },
        record_url: if let Some((name, file)) = &session.document {
            format!("/documents/{}?file={}", encode(name), encode(file))
        } else {
            session
                .parent
                .as_ref()
                .map(|p| record_url(&p.collection, &p.id))
                .unwrap_or_else(|| record_url(&session.collection, &session.record_id))
        },
        collection_label: if session.document.is_some() {
            "JSON documents".into()
        } else if let Some(parent) = &session.parent {
            editor.label(
                &parent.collection,
                &editor.get(&parent.collection, &parent.id)?.value,
                &parent.id,
                "record",
            )?
        } else {
            crate::i18n::collection(editor, &session.collection)
        },
        title: if let Some((_, file)) = &session.document {
            file.clone()
        } else if session.record_id.is_empty() {
            crate::i18n::message("New {type}", &[("type", crate::i18n::text(ui,"singularLabel","record"))])
        } else {
            editor.label(&session.collection, &value, &session.record_id, "record")?
        },
        issues,
        message,
        message_kind,
        picker: crate::pickers::render(editor, session, actor)?,
        back: if let Some((name, file)) = &session.document {
            format!("/documents/{}?file={}", encode(name), encode(file))
        } else if let Some(parent) = &session.parent {
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
        "<div id=\"notification-region\" role=\"status\" data-kind=\"success\">{}</div>",
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

crate::templates::template!(Panel<'_>, "panel.html");
#[derive(serde::Serialize)]
struct Panel<'a> {
    url: &'a str,
    title: &'a str,
    body: &'a str,
    section: &'a str,
    category: &'a str,
    actions: &'a str,
}
pub fn panel(url: &str, title: &str, body: &str) -> Result<String> {
    panel_with_actions(url, title, body, "")
}
pub fn panel_with_actions(url: &str, title: &str, body: &str, actions: &str) -> Result<String> {
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
        actions,
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

#[derive(serde::Serialize)]
struct DraftExit<'a> {
    session: std::collections::HashMap<&'a str, &'a str>,
    dirty: bool,
}
crate::templates::template!(DraftExit<'_>, "draft-exit.html");
pub fn draft_exit(session: &EditSession) -> Result<String> {
    DraftExit {
        session: std::collections::HashMap::from([("id", session.id.as_str())]),
        dirty: session.dirty(),
    }
    .render()
}

crate::templates::template!(Header<'_>, "header.html");
#[derive(serde::Serialize)]
struct Header<'a> { actor: crate::templates::ActorContext<'a>, language_picker: String }
pub fn header(state: &crate::AppState, actor: &Actor) -> Result<String> {
    Header { actor: actor.into(), language_picker: crate::i18n::chooser(state, actor) }.render()
}
