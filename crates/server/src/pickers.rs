//! Picker content is rendered into the parent form. All selections return to Rust.
use crate::{
    error::{Result, WebError},
    sessions::EditSession,
    views,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use structured_editor_core::{Editor, MediaKind};
pub fn render(editor: &Editor, session: &EditSession) -> Result<String> {
    let Some(p) = &session.picker else {
        return Ok(String::new());
    };
    editor.require_operation(&p.collection, "read")?;
    let base = format!("/edit-sessions/{}/fields/{}/actions", session.id, p.field);
    let mut html = format!(
        "<server-picker class=\"picker\" id=\"picker-region\" data-return-field=\"{}\" role=\"dialog\" aria-modal=\"true\" aria-labelledby=\"picker-title\"><div class=\"picker-heading\"><h2 id=\"picker-title\">{}</h2><button class=\"icon-button\" aria-label=\"Close picker\" data-close-picker formaction=\"{base}/close-picker\"><i class=\"ri-close-line\" aria-hidden=\"true\"></i></button></div><label>Find<input name=\"picker_query\" value=\"{}\"></label>",
        views::escape(&p.field),
        if p.kind == "links" {
            "Insert or edit Markdown link"
        } else {
            "Choose a reference"
        },
        views::escape(&p.query)
    );
    if p.kind == "links" {
        html += "<label>Collection<select name=\"picker_collection\">";
        for (name, c) in &editor.config().collections {
            if !editor.allows(name, "read") {
                continue;
            }
            html += &format!(
                "<option value=\"{}\" {}>{}</option>",
                views::escape(name),
                if *name == p.collection {
                    "selected"
                } else {
                    ""
                },
                views::escape(&c.label)
            );
        }
        for (name, c) in &editor.config().media_collections {
            if !editor.allows(name, "read") {
                continue;
            }
            html += &format!(
                "<option value=\"{}\" {}>{}</option>",
                views::escape(name),
                if *name == p.collection {
                    "selected"
                } else {
                    ""
                },
                views::escape(&c.label)
            );
        }
        html += "</select></label><label>Link label<input name=\"link_label\"></label><label>URL (optional when choosing a target)<input name=\"link_url\"></label><label><input type=\"checkbox\" name=\"link_image\">Image</label>";
        html += &format!(
            "<button formaction=\"{base}/insert-link\">Insert URL / replace selected link</button>"
        );
    }
    html +=
        &format!("<button data-picker-search formaction=\"{base}/search-picker\">Search</button>");
    if editor
        .config()
        .media_collections
        .contains_key(&p.collection)
    {
        let media = editor.for_media(&p.collection)?;
        let entries = if p.query.is_empty() {
            media.media_list(&p.path, p.offset, 51)?
        } else {
            media.media_search(&p.query, p.offset, 51)?
        };
        if !p.path.is_empty() {
            let parent = p.path.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
            html += &format!(
                "<button name=\"choice\" value=\"{}\" formaction=\"{base}/folder\">Parent folder</button>",
                views::escape(parent)
            );
        }
        for e in entries.iter().take(50) {
            html += &format!(
                "<p><button name=\"choice\" value=\"{}\" formaction=\"{base}/{}\">{}</button></p>",
                views::escape(&e.path),
                if matches!(e.kind, MediaKind::Directory) {
                    "folder"
                } else {
                    "select"
                },
                views::escape(&e.name)
            );
        }
        if entries.len() > 50 {
            html += &format!("<button formaction=\"{base}/next-picker\">Next</button>");
        }
    } else if editor.config().collections.contains_key(&p.collection) {
        let records = editor.list(&p.collection, &p.query, p.offset, 51)?;
        for r in records.iter().take(50) {
            let doc = editor.get(&p.collection, &r.id)?;
            let label = editor.label(&p.collection, &doc.value, &r.id, "reference")?;
            html += &format!(
                "<p><button name=\"choice\" value=\"{}\" formaction=\"{base}/select\">{}</button></p>",
                views::escape(&r.id),
                views::escape(&label)
            );
        }
        if records.len() > 50 {
            html += &format!("<button formaction=\"{base}/next-picker\">Next</button>");
        }
        if session
            .draft
            .field(&p.field)
            .is_some_and(|f| f.schema.pointer("/x-relation/create").is_some())
            && editor.allows(&p.collection, "create")
        {
            html += &format!(
                "<button formaction=\"{base}/create-related\">Create a new record in this collection</button>"
            );
        }
    }
    if p.offset > 0 {
        html += &format!("<button formaction=\"{base}/previous-picker\">Previous</button>");
    }
    html += "</server-picker>";
    Ok(html)
}
pub fn action(
    editor: &Editor,
    s: &mut EditSession,
    field: &str,
    action: &str,
    form: &HashMap<String, String>,
) -> Result<()> {
    let f = s
        .draft
        .field(field)
        .ok_or_else(|| WebError::bad("Unknown field"))?
        .clone();
    if f.hidden || f.read_only {
        return Err(WebError::bad("Field is not editable"));
    }
    if action == "preview" {
        if !s.preview.remove(field) {
            s.preview.insert(field.into());
        }
        return Ok(());
    }
    if action == "picker" || action == "links" {
        let collection = if action == "links" {
            editor
                .config()
                .collections
                .keys()
                .find(|name| editor.allows(name, "read"))
                .cloned()
                .unwrap_or_default()
        } else {
            f.schema
                .pointer("/x-relation/collection")
                .or_else(|| f.schema.pointer("/x-media/collection"))
                .and_then(Value::as_str)
                .unwrap_or("media")
                .into()
        };
        s.picker = Some(crate::sessions::Picker {
            field: field.into(),
            kind: action.into(),
            collection,
            from: form
                .get(&format!("selection_from.{field}"))
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            to: form
                .get(&format!("selection_to.{field}"))
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            ..Default::default()
        });
        return Ok(());
    }
    if action == "close-picker" {
        s.picker = None;
        return Ok(());
    }
    let p = s
        .picker
        .as_mut()
        .filter(|p| p.field == field)
        .ok_or_else(|| WebError::bad("Open the picker first"))?;
    editor.require_operation(&p.collection, "read")?;
    match action {
        "search-picker" => {
            p.query = form.get("picker_query").cloned().unwrap_or_default();
            if p.kind == "links" {
                p.collection = form.get("picker_collection").cloned().unwrap_or_default();
                p.path.clear();
            }
            p.offset = 0;
        }
        "next-picker" => p.offset = p.offset.saturating_add(50),
        "previous-picker" => p.offset = p.offset.saturating_sub(50),
        "folder" => {
            p.path = form.get("choice").cloned().unwrap_or_default();
            p.query.clear();
            p.offset = 0;
        }
        "select" | "insert-link" => {
            let choice = form.get("choice").cloned().unwrap_or_default();
            if p.kind == "links" {
                let (url, label) = if action == "insert-link" {
                    (
                        form.get("link_url").cloned().unwrap_or_default(),
                        form.get("link_label").cloned().unwrap_or_default(),
                    )
                } else if editor
                    .config()
                    .media_collections
                    .contains_key(&p.collection)
                {
                    editor.for_media(&p.collection)?.media_read(&choice)?;
                    (
                        format!(
                            "/media/{}/file?path={}",
                            views::encode(&p.collection),
                            views::encode(&choice)
                        ),
                        choice.clone(),
                    )
                } else {
                    let doc = editor.get(&p.collection, &choice)?;
                    (
                        views::record_url(&p.collection, &choice),
                        editor.label(&p.collection, &doc.value, &choice, "reference")?,
                    )
                };
                let supplied = form
                    .get("link_label")
                    .filter(|s| !s.is_empty())
                    .cloned()
                    .unwrap_or(label);
                let text = structured_editor_core::insert_markdown_link(
                    &f.raw,
                    p.from,
                    p.to,
                    &url,
                    &supplied,
                    form.contains_key("link_image"),
                )?;
                s.draft
                    .set_field(field, json!(text))
                    .map_err(WebError::bad)?;
            } else {
                if editor.config().collections.contains_key(&p.collection) {
                    editor.get(&p.collection, &choice)?;
                } else {
                    editor.for_media(&p.collection)?.media_read(&choice)?;
                }
                s.draft
                    .set_field(field, json!(choice))
                    .map_err(WebError::bad)?;
            }
            s.picker = None;
        }
        _ => return Err(WebError::bad("Unknown picker action")),
    }
    Ok(())
}
