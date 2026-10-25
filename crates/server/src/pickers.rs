//! Picker content is rendered into the parent form. All selections return to Rust.
use crate::{
    error::{Result, WebError},
    sessions::EditSession,
    views,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use structured_editor_core::{Editor, MediaKind};
pub fn render(
    editor: &Editor,
    session: &EditSession,
    actor: &crate::auth::Actor,
) -> Result<String> {
    let Some(p) = &session.picker else {
        return Ok(String::new());
    };
    if p.kind == "multiple" {
        return render_multiple(editor, session);
    }
    editor.require_operation(&p.collection, "read")?;
    let base = format!("/edit-sessions/{}/fields/{}/actions", session.id, p.field);
    let kind = if editor.config().collections.contains_key(&p.collection) {
        views::record_kind(editor, &p.collection)?
    } else {
        "media".into()
    };
    let title = if p.kind == "links" {
        "Insert or edit a link".into()
    } else {
        crate::i18n::message("Choose {type}", &[("type", &kind)])
    };
    let placeholder = if p.kind == "links" {
        "Search records or files…".into()
    } else {
        format!(
            "Search {}…",
            editor
                .config()
                .collections
                .get(&p.collection)
                .map(|c| crate::i18n::text(&c.ui,"label", &c.label).to_lowercase())
                .unwrap_or_else(|| "media".into())
        )
    };
    let mut html = format!(
        "<server-picker id=\"picker-region\" data-return-field=\"{}\"><section class=\"picker\" role=\"dialog\" aria-modal=\"true\" aria-labelledby=\"picker-title\"><div class=\"picker-heading\"><h2 id=\"picker-title\">{}</h2><button class=\"icon-button\" aria-label=\"{ui_text_1}\" data-close-picker formaction=\"{base}/close-picker\"><i class=\"ri-close-line\" aria-hidden=\"true\"></i></button></div><div class=\"picker-search\"><label>{ui_text_0}<input name=\"picker_query\" type=\"search\" placeholder=\"{}\" value=\"{}\"></label>",
        views::escape(&p.field),
        views::escape(&title),
        views::escape(&placeholder),
        views::escape(&p.query), ui_text_0 = crate::views::escape(&crate::i18n::t("Find")), ui_text_1 = crate::views::escape(&crate::i18n::t("Close picker")));
    if p.kind == "links" {
        html += "<label>Collection<select name=\"picker_collection\" data-picker-collection><optgroup label=\"Records\">";
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
                views::escape(crate::i18n::text(&c.ui,"label", &c.label))
            );
        }
        html += "</optgroup><optgroup label=\"Media files\">";
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
                views::escape(crate::i18n::text(&c.ui,"label", &c.label))
            );
        }
        html += "</optgroup></select></label>";
    }
    html += &format!(
        "<button data-picker-search formaction=\"{base}/search-picker\">{ui_text_0}</button></div>", ui_text_0 = crate::views::escape(&crate::i18n::t("Search")));
    if p.kind == "links" {
        html += &format!(
            "<details class=\"picker-link-options\"><summary>Link text, image or external URL</summary><div class=\"picker-link-fields\"><label>Link text (optional)<input name=\"link_label\" placeholder=\"Use the selected item’s label\" value=\"{}\"></label><label>External URL<input name=\"link_url\" placeholder=\"https://…\" value=\"{}\"></label><label class=\"checkbox-label\"><input type=\"checkbox\" name=\"link_image\" {}>Insert as an image</label><button formaction=\"{base}/insert-link\">Insert URL</button></div></details>",
            views::escape(&p.label),
            views::escape(&p.url),
            if p.image { "checked" } else { "" }
        );
    }
    html += &format!("<div class=\"picker-results\" aria-label=\"{ui_text_0}\">", ui_text_0 = crate::views::escape(&crate::i18n::t("Search results")));
    let mut footer = String::new();
    if editor
        .config()
        .media_collections
        .contains_key(&p.collection)
    {
        let config = &editor.config().media_collections[&p.collection];
        html += &format!(
            "<div class=\"picker-location\"><i class=\"ri-folder-line\" aria-hidden=\"true\"></i><strong>{}</strong><span>{}</span></div>",
            views::escape(&config.label),
            views::escape(if p.path.is_empty() { "/" } else { &p.path })
        );
        if actor.editable() && editor.allows(&p.collection, "upload") {
            html += &format!(
                "<div class=\"picker-upload\"><input form=\"picker-upload-form\" type=\"hidden\" name=\"csrf\" value=\"{}\"><input form=\"picker-upload-form\" type=\"hidden\" name=\"picker_session\" value=\"{}\"><input form=\"picker-upload-form\" type=\"hidden\" name=\"picker_version\" value=\"{}\"><input form=\"picker-upload-form\" type=\"hidden\" name=\"path\" value=\"{}\"><label>Upload to this folder<input form=\"picker-upload-form\" type=\"file\" name=\"file\" required></label><button form=\"picker-upload-form\" formaction=\"/media/{}/upload\" data-attr:disabled=\"$_pickerUploadPending\"><i class=\"ri-upload-2-line\" aria-hidden=\"true\"></i>Upload</button><p class=\"help\">Up to 50 MiB. Uploaded files remain in this collection even if you discard the record.</p><div id=\"picker-upload-result\" aria-live=\"polite\"></div></div>",
                views::escape(&actor.csrf),
                views::escape(&session.id),
                session.version,
                views::escape(&p.path),
                views::encode(&p.collection)
            );
        }
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
        if entries.is_empty() {
            html += "<p class=\"empty-state compact\">No files match this search.</p>";
        }
        for e in entries.iter().take(50) {
            let folder = matches!(e.kind, MediaKind::Directory);
            let mut visual = if folder {
                String::new()
            } else {
                crate::media::thumbnail(editor, &p.collection, &e.path)
            };
            if visual.is_empty() {
                visual = format!(
                    "<i class=\"{}\" aria-hidden=\"true\"></i>",
                    if folder {
                        "ri-folder-line"
                    } else {
                        "ri-file-line"
                    }
                );
            }
            html += &format!(
                "<button class=\"picker-result\" name=\"choice\" value=\"{}\" formaction=\"{base}/{}\"><span class=\"picker-media-item\">{visual}<span>{}<small>{}</small></span></span><i class=\"ri-arrow-right-line\" aria-hidden=\"true\"></i></button>",
                views::escape(&e.path),
                if matches!(e.kind, MediaKind::Directory) {
                    "folder"
                } else {
                    "select"
                },
                views::escape(&e.name),
                views::escape(&e.path)
            );
        }
        if entries.len() > 50 {
            footer += &format!("<button formaction=\"{base}/next-picker\">{ui_text_0}</button>", ui_text_0 = crate::views::escape(&crate::i18n::t("Next")));
        }
    } else if editor.config().collections.contains_key(&p.collection) {
        let records = editor.list(&p.collection, &p.query, p.offset, 51)?;
        if records.is_empty() {
            html += &format!("<p class=\"empty-state compact\">{ui_text_0}</p>", ui_text_0 = crate::views::escape(&crate::i18n::t("No records match this search.")));
        }
        for r in records.iter().take(50) {
            let doc = editor.get(&p.collection, &r.id)?;
            let label = editor.label(&p.collection, &doc.value, &r.id, "reference")?;
            html += &format!(
                "<button class=\"picker-result\" name=\"choice\" value=\"{}\" formaction=\"{base}/select\"><span>{}</span><i class=\"ri-arrow-right-line\" aria-hidden=\"true\"></i></button>",
                views::escape(&r.id),
                views::escape(&label)
            );
        }
        if records.len() > 50 {
            footer += &format!("<button formaction=\"{base}/next-picker\">{ui_text_0}</button>", ui_text_0 = crate::views::escape(&crate::i18n::t("Next")));
        }
        if session
            .draft
            .field(&p.field)
            .is_some_and(|f| f.schema.pointer("/x-relation/create").is_some())
            && editor.allows(&p.collection, "create")
        {
            footer += &format!(
                "<button formaction=\"{base}/create-related\">Create a new record in this collection</button>"
            );
        }
    }
    if p.offset > 0 {
        footer += &format!("<button formaction=\"{base}/previous-picker\">{ui_text_0}</button>", ui_text_0 = crate::views::escape(&crate::i18n::t("Previous")));
    }
    html += "</div>";
    if !footer.is_empty() {
        html += &format!("<div class=\"dialog-footer\">{footer}</div>");
    }
    html += "</section></server-picker>";
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
    if action == "select-choice" {
        // Inline autocomplete uses the same validation as Browse, without leaving a picker open.
        let previous = s.picker.take();
        s.picker = Some(crate::sessions::Picker { field: field.into(), kind: "multiple".into(), ..Default::default() });
        let result = multiple_action(editor, s, &f, action, form);
        s.picker = previous;
        return result;
    }
    if action == "multiple"
        || s.picker
            .as_ref()
            .is_some_and(|p| p.field == field && p.kind == "multiple")
    {
        return multiple_action(editor, s, &f, action, form);
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
                .chain(editor.config().media_collections.keys())
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
    if p.kind == "links" {
        p.label = form.get("link_label").cloned().unwrap_or_default();
        p.url = form.get("link_url").cloned().unwrap_or_default();
        p.image = form.contains_key("link_image");
    }
    match action {
        "search-picker" => {
            p.query = form.get("picker_query").cloned().unwrap_or_default();
            if p.kind == "links" {
                let collection = form
                    .get("picker_collection")
                    .cloned()
                    .unwrap_or_else(|| p.collection.clone());
                editor.require_operation(&collection, "read")?;
                if p.collection != collection {
                    p.collection = collection;
                    p.path.clear();
                    p.query.clear();
                }
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

/// Ordered selections share the same control for relation and enum arrays.
pub(crate) fn render_multiple(editor: &Editor, session: &EditSession) -> Result<String> {
    let p = session.picker.as_ref().unwrap();
    let field = session
        .draft
        .field(&p.field)
        .ok_or_else(|| WebError::bad("Unknown field"))?;
    let schema = crate::fields::array::choices_schema(field)
        .ok_or_else(|| WebError::bad("Not a choice array"))?;
    let base = format!("/edit-sessions/{}/fields/{}/actions", session.id, p.field);
    let mut options = Vec::new();
    if let Some(target) = schema
        .pointer("/x-relation/collection")
        .and_then(Value::as_str)
    {
        editor.require_operation(target, "read")?;
        for record in editor.list(target, &p.query, p.offset, 51)? {
            let doc = editor.get(target, &record.id)?;
            options.push((
                json!(record.id),
                editor.label(target, &doc.value, &record.id, "reference")?,
            ));
        }
    } else if let Some(values) = schema["enum"].as_array() {
        options = values
            .iter()
            .map(|value| {
                let label = views::preview::option_label(schema, value)
                    .or_else(|| value.as_str())
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string());
                (
                    value.clone(),
                    if label.is_empty() {
                        "Empty value".into()
                    } else {
                        label
                    },
                )
            })
            .filter(|(_, label)| label.to_lowercase().contains(&p.query.to_lowercase()))
            .skip(p.offset)
            .take(51)
            .collect();
    }
    let mut html = format!(
        "<server-picker id=\"picker-region\" data-return-field=\"{}\" data-popover data-search-url=\"/edit-sessions/{}/fields/{}/choices\"><section class=\"picker multi-picker\" role=\"dialog\" aria-labelledby=\"picker-title\"><div class=\"picker-heading\"><h2 id=\"picker-title\">{}</h2><button class=\"icon-button\" aria-label=\"{ui_text_2}\" data-close-picker formaction=\"{base}/close-picker\"><i class=\"ri-close-line\" aria-hidden=\"true\"></i></button></div><div class=\"picker-search\"><label>{ui_text_0}<input name=\"picker_query\" type=\"search\" placeholder=\"{ui_text_3}\" value=\"{}\"></label><button data-picker-search formaction=\"{base}/search-picker\">{ui_text_1}</button></div><div data-picker-options><div class=\"picker-results\" aria-label=\"{ui_text_4}\">",
        views::escape(&field.id),
        session.id, field.id,
        views::escape(&crate::tree::label(field)),
        views::escape(&p.query), ui_text_0 = crate::views::escape(&crate::i18n::t("Find")), ui_text_1 = crate::views::escape(&crate::i18n::t("Search")), ui_text_2 = crate::views::escape(&crate::i18n::t("Close picker")), ui_text_3 = crate::views::escape(&crate::i18n::t("Search…")), ui_text_4 = crate::views::escape(&crate::i18n::t("Choices")));
    if options.is_empty() {
        html += &format!("<p class=\"empty-state compact\">{ui_text_0}</p>", ui_text_0 = crate::views::escape(&crate::i18n::t("No matches.")));
    }
    for (value, label) in options.iter().take(50) {
        let selected = field
            .items
            .iter()
            .any(|item| crate::tree::snapshot(item) == *value);
        html += &format!(
            "<button class=\"picker-result multi-option\" role=\"checkbox\" aria-checked=\"{selected}\" name=\"choice\" value=\"{}\" formaction=\"{base}/toggle-choice\"><i class=\"{}\" aria-hidden=\"true\"></i><span>{}</span></button>",
            views::escape(&value.to_string()),
            if selected {
                "ri-checkbox-line"
            } else {
                "ri-checkbox-blank-line"
            },
            views::escape(label)
        );
    }
    html += &format!(
        "</div><div class=\"dialog-footer multi-picker-footer\"><span role=\"status\" data-selection-count=\"{}\">{}</span><div class=\"actions\">",
        views::escape(&crate::i18n::t("{count} selected")),
        views::escape(&crate::i18n::message("{count} selected", &[("count", &field.items.len().to_string())]))
    );
    if p.offset > 0 {
        html += &format!("<button formaction=\"{base}/previous-picker\">{ui_text_0}</button>", ui_text_0 = crate::views::escape(&crate::i18n::t("Previous")));
    }
    if options.len() > 50 {
        html += &format!("<button formaction=\"{base}/next-picker\">{ui_text_0}</button>", ui_text_0 = crate::views::escape(&crate::i18n::t("Next")));
    }
    html += &format!(
        "<button class=\"primary\" formaction=\"{base}/close-picker\">{ui_text_0}</button></div></div></div></section></server-picker>", ui_text_0 = crate::views::escape(&crate::i18n::t("Done")));
    Ok(html)
}

fn multiple_action(
    editor: &Editor,
    session: &mut EditSession,
    field: &structured_editor_core::draft::DraftField,
    action: &str,
    form: &HashMap<String, String>,
) -> Result<()> {
    let schema = crate::fields::array::choices_schema(field)
        .ok_or_else(|| WebError::bad("Not a choice array"))?;
    let target = schema
        .pointer("/x-relation/collection")
        .and_then(Value::as_str);
    if let Some(target) = target {
        editor.require_operation(target, "read")?;
    }
    if action == "multiple" {
        session.picker = Some(crate::sessions::Picker {
            field: field.id.clone(),
            kind: "multiple".into(),
            collection: target.unwrap_or("").into(),
            ..Default::default()
        });
        return Ok(());
    }
    if action == "close-picker" {
        session.picker = None;
        return Ok(());
    }
    let p = session
        .picker
        .as_mut()
        .filter(|p| p.field == field.id)
        .ok_or_else(|| WebError::bad("Open the picker first"))?;
    if let Some(query) = form.get("picker_query") {
        if p.query != *query { p.query = query.clone(); p.offset = 0; }
    }
    match action {
        "search-picker" => {
            p.query = form.get("picker_query").cloned().unwrap_or_default();
            p.offset = 0;
        }
        "next-picker" => p.offset = p.offset.saturating_add(50),
        "previous-picker" => p.offset = p.offset.saturating_sub(50),
        "toggle-choice" | "select-choice" => {
            let choice: Value =
                serde_json::from_str(form.get("choice").map(String::as_str).unwrap_or(""))
                    .map_err(|_| WebError::bad("Invalid choice"))?;
            let selected: Vec<_> = field
                .items
                .iter()
                .filter(|item| crate::tree::snapshot(item) == choice)
                .map(|item| item.id.clone())
                .collect();
            if !selected.is_empty() {
                if action == "select-choice" { return Ok(()); }
                for id in selected {
                    session
                        .draft
                        .array_action(&session.schema, &field.id, "remove", Some(&id))
                        .map_err(WebError::bad)?;
                }
            } else {
                if let Some(target) = target {
                    editor.get(
                        target,
                        choice
                            .as_str()
                            .ok_or_else(|| WebError::bad("Invalid record ID"))?,
                    )?;
                } else if !schema["enum"]
                    .as_array()
                    .is_some_and(|options| options.contains(&choice))
                {
                    return Err(WebError::bad("Unknown choice"));
                }
                if field.schema["maxItems"]
                    .as_u64()
                    .is_some_and(|max| field.items.len() as u64 >= max)
                {
                    return Err(WebError::bad(
                        "The maximum number of selections has been reached",
                    ));
                }
                session
                    .draft
                    .array_action(&session.schema, &field.id, "add", None)
                    .map_err(WebError::bad)?;
                let id = session
                    .draft
                    .field(&field.id)
                    .unwrap()
                    .items
                    .last()
                    .unwrap()
                    .id
                    .clone();
                session
                    .draft
                    .set_field(&id, choice)
                    .map_err(WebError::bad)?;
            }
        }
        _ => return Err(WebError::bad("Unknown picker action")),
    }
    Ok(())
}
