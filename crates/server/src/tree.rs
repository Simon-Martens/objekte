//! Navigation is derived from the same resolved fields as the form.
use crate::{sessions::EditSession, views::escape};
use structured_editor_core::draft::{DraftField, InputKind};
pub fn label(f: &DraftField) -> String {
    if f.key == "_editor" {
        return crate::views::metadata::label("_editor");
    }
    if let Ok(index) = f.key.parse::<usize>() {
        if let Some(name) = f.items.iter().find(|child| {
            !child.hidden
                && child.present
                && child.kind == InputKind::Text
                && matches!(child.key.as_str(), "title" | "name")
                && !child.raw.is_empty()
        }) {
            return name.raw.clone();
        }
        return format!(
            "{} {}",
            f.schema["title"].as_str().unwrap_or("Item"),
            index + 1
        );
    }
    f.schema["title"]
        .as_str()
        .unwrap_or(if f.key.is_empty() { "Document" } else { &f.key })
        .into()
}
pub fn navigable(f: &DraftField) -> bool {
    !f.hidden
        && f.present
        && !f.read_only
        && (f.kind == InputKind::Object
            || (f.kind == InputKind::Array
                && (f.items.iter().any(|f| {
                    f.kind == InputKind::Object || (f.kind == InputKind::Array && navigable(f))
                }) || f.schema["items"]["type"] == "object")))
}
pub fn edit_navigation(session: &EditSession) -> String {
    fn nodes(fields: &[DraftField], session: &EditSession, out: &mut String) {
        for f in fields.iter().filter(|f| navigable(f)) {
            let label = label(f);
            *out += &format!(
                "<details open class=\"data-tree-node\" id=\"nav-data-{}\"><summary><button form=\"record-form\" formaction=\"/edit-sessions/{}/fields/{}/actions/browse\">{}</button></summary><div class=\"tree-children\">",
                f.id,
                session.id,
                f.id,
                escape(&label)
            );
            if f.schema["x-presentation"]["navigation"] != "stop" {
                nodes(&f.items, session, out);
            }
            *out += "</div></details>";
        }
    }
    let mut html = String::from("<template id=\"data-navigation\">");
    if session.raw.is_none() {
        nodes(&session.draft.fields, session, &mut html);
    }
    html + "</template>"
}

pub fn preview(
    editor: &structured_editor_core::Editor,
    draft: &structured_editor_core::draft::Draft,
    value: &serde_json::Value,
    url: &str,
    path: &str,
) -> String {
    use crate::views::{encode, preview};
    use structured_editor_core::draft::child_path;
    fn link(url: &str, path: &str) -> String {
        format!(
            "{url}{}node={}",
            if url.contains('?') { "&" } else { "?" },
            encode(path)
        )
    }
    fn nodes(fields: &[DraftField], parent: &str, root: bool, url: &str, out: &mut String) {
        for f in fields.iter().filter(|f| navigable(f)) {
            let path = if root {
                String::new()
            } else {
                child_path(parent, &f.key)
            };
            *out += &format!(
                "<details open class=\"data-tree-node\" id=\"nav-path-{}\"><summary><a data-nav href=\"{}\">{}</a></summary><div class=\"tree-children\">",
                encode(&path),
                escape(&link(url, &path)),
                escape(&label(f))
            );
            if f.schema["x-presentation"]["navigation"] != "stop" {
                nodes(&f.items, &path, false, url, out);
            }
            *out += "</div></details>";
        }
    }
    let mut html = String::from("<template id=\"data-navigation\">");
    nodes(&draft.fields, "", draft.root_value, url, &mut html);
    html += "</template>";
    let selected = draft.at_path(path).filter(|f| navigable(f));
    let fields: Vec<(&DraftField, String)> = if let Some(f) = selected {
        if !path.is_empty() {
            html += &format!(
                "<a class=\"button quiet\" data-nav href=\"{}\">Up one level</a><p class=\"mono\">{}</p>",
                escape(&link(
                    url,
                    path.rsplit_once('/').map(|(p, _)| p).unwrap_or("")
                )),
                escape(path)
            );
        }
        f.items
            .iter()
            .map(|child| (child, child_path(path, &child.key)))
            .collect()
    } else {
        draft
            .fields
            .iter()
            .map(|f| {
                (
                    f,
                    if draft.root_value {
                        String::new()
                    } else {
                        child_path("", &f.key)
                    },
                )
            })
            .collect()
    };
    let mut metadata = String::new();
    for (f, path) in fields {
        if f.hidden || !f.present {
            continue;
        }
        let Some(value) = value.pointer(&path) else {
            continue;
        };
        if f.key == "_editor" {
            metadata += &crate::views::metadata::preview(value);
            continue;
        }
        let title = label(f);
        html += &format!(
            "<section class=\"preview-field\"><h3>{}</h3><div>",
            escape(&title)
        );
        if navigable(f) {
            html += &format!(
                "<a class=\"button\" data-nav href=\"{}\">Open {} <span class=\"badge\">{}</span></a>",
                escape(&link(url, &path)),
                escape(&title),
                f.items.iter().filter(|f| f.present && !f.hidden).count()
            );
        } else {
            html += &preview::value(editor, f, value);
        }
        html += "</div></section>";
    }
    html + &metadata
}
