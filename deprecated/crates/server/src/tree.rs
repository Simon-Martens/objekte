//! Navigation is derived from the same resolved fields as the form.
use crate::{sessions::EditSession, views::escape};
use structured_editor_core::draft::{DraftField, InputKind};
pub fn label(f: &DraftField) -> String {
    if f.key == "_editor" {
        return crate::views::metadata::label("_editor");
    }
    if let Ok(index) = f.key.parse::<usize>() {
        for key in crate::views::embedded::display_fields(&f.schema) {
            if let Some(name) = f.items.iter().find(|child| {
                child.key == key
                    && !child.hidden
                    && child.present
                    && child.kind == InputKind::Text
                    && !child.raw.is_empty()
            }) {
                return name.raw.clone();
            }
        }
        return format!(
            "{} {}",
            crate::i18n::field(&f.schema, &crate::i18n::t("Item")),
            index + 1
        );
    }
    crate::i18n::field(&f.schema, if f.key.is_empty() { "Document" } else { &f.key })
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
/// Readable names for draft rows, including linked record names.
pub fn edit_label(editor: &structured_editor_core::Editor, f: &DraftField) -> String {
    if f.key.parse::<usize>().is_ok() && f.kind == InputKind::Object {
        let name = crate::views::embedded::Cells::new(editor).text(&f.schema, &snapshot(f));
        if !name.is_empty() {
            return name;
        }
    }
    label(f)
}
pub fn snapshot(field: &DraftField) -> serde_json::Value {
    match field.kind {
        InputKind::Object => serde_json::Value::Object(
            field
                .items
                .iter()
                .filter(|f| f.present)
                .map(|f| (f.key.clone(), snapshot(f)))
                .collect(),
        ),
        InputKind::Array => serde_json::Value::Array(field.items.iter().map(snapshot).collect()),
        InputKind::Text => serde_json::Value::String(field.raw.clone()),
        _ => serde_json::from_str(&field.raw)
            .unwrap_or_else(|_| serde_json::Value::String(field.raw.clone())),
    }
}
pub fn edit_breadcrumbs(editor: &structured_editor_core::Editor, session: &EditSession) -> String {
    if session.view_path.is_empty() {
        return String::new();
    }
    let mut html = format!(
        "<nav class=\"record-breadcrumbs\" aria-label=\"{ui_text_1}\"><button class=\"small\" formaction=\"/edit-sessions/{}/browse-root\">{ui_text_0}</button>",
        session.id, ui_text_0 = crate::views::escape(&crate::i18n::t("Overview")), ui_text_1 = crate::views::escape(&crate::i18n::t("Editing location")));
    let mut path = String::new();
    for part in session.view_path.split('/').skip(1) {
        path += &format!("/{part}");
        if let Some(f) = session.draft.at_path(&path).filter(|f| navigable(f)) {
            let name = escape(&edit_label(editor, f));
            if path == session.view_path {
                html += &format!("<span aria-current=\"page\">{name}</span>");
            } else {
                html += &format!(
                    "<button class=\"small\" formaction=\"/edit-sessions/{}/fields/{}/actions/browse\">{name}</button>",
                    session.id, f.id
                );
            }
        }
    }
    html + "</nav>"
}
pub fn edit_navigation(session: &EditSession) -> String {
    fn contains(field: &DraftField, id: &str) -> bool {
        field.id == id || field.items.iter().any(|child| contains(child, id))
    }
    fn nodes(fields: &[DraftField], session: &EditSession, out: &mut String) {
        for f in fields.iter().filter(|f| {
            navigable(f) && f.key != "_editor" && f.schema["x-presentation"]["secondary"] != true
        }) {
            let label = label(f);
            if crate::views::embedded::is_table(f) {
                if session.view_path.is_empty() {
                    *out += &format!(
                        "<a class=\"record-section-link\" href=\"#field-{}\">{}<span class=\"badge\">{}</span></a>",
                        f.id, escape(&label), f.items.len()
                    );
                } else {
                    *out += &format!(
                        "<button class=\"record-section-link\" form=\"record-form\" formaction=\"/edit-sessions/{}/browse-root\">{}<span class=\"badge\">{}</span></button>",
                        session.id, escape(&label), f.items.len()
                    );
                }
                continue;
            }
            *out += &format!(
                "<button {} class=\"record-section-link\" id=\"nav-data-{}\" form=\"record-form\" formaction=\"/edit-sessions/{}/fields/{}/actions/browse\">{}{}</button>",
                if session
                    .draft
                    .at_path(&session.view_path)
                    .is_some_and(|selected| contains(f, &selected.id))
                {
                    "aria-current=\"page\""
                } else {
                    ""
                },
                f.id,
                session.id,
                f.id,
                escape(&label),
                if f.kind == InputKind::Array {
                    format!("<span class=\"badge\">{}</span>", f.items.len())
                } else {
                    String::new()
                }
            );
            if f.kind != InputKind::Array && f.schema["x-presentation"]["navigation"] != "stop" {
                nodes(&f.items, session, out);
            }
        }
    }
    let mut html = String::from("<template id=\"data-navigation\">");
    if session.raw.is_none() {
        html += &format!(
            "<p class=\"eyebrow\">{ui_text_1}</p><button {} class=\"record-section-link\" form=\"record-form\" formaction=\"/edit-sessions/{}/browse-root\">{ui_text_0}</button>",
            if session.view_path.is_empty() {
                "aria-current=\"page\""
            } else {
                ""
            },
            session.id, ui_text_0 = crate::views::escape(&crate::i18n::t("Overview")), ui_text_1 = crate::views::escape(&crate::i18n::t("This record")));
        nodes(&session.draft.fields, session, &mut html);
    }
    html + "</template>"
}

#[cfg(test)]
pub fn preview(
    editor: &structured_editor_core::Editor,
    draft: &structured_editor_core::draft::Draft,
    value: &serde_json::Value,
    url: &str,
    path: &str,
) -> String {
    preview_with_options(editor, draft, value, url, path, &Default::default(), false)
}

pub fn preview_with_options(
    editor: &structured_editor_core::Editor,
    draft: &structured_editor_core::draft::Draft,
    value: &serde_json::Value,
    url: &str,
    path: &str,
    params: &std::collections::HashMap<String, String>,
    editable: bool,
) -> String {
    use crate::views::{embedded, preview, table::query_url};
    use structured_editor_core::draft::child_path;
    fn empty(value: &serde_json::Value) -> bool {
        value.is_null()
            || value.as_str().is_some_and(|s| s.trim().is_empty())
            || value.as_array().is_some_and(|a| a.is_empty())
            || value.as_object().is_some_and(|o| o.is_empty())
    }
    fn visible(f: &DraftField, value: &serde_json::Value) -> bool {
        !f.hidden
            && f.present
            && f.schema["x-presentation"]["preview"] != "hidden"
            && !(f.schema["x-presentation"]["hideEmpty"] == true
                && empty(value)
                && !embedded::is_table(f))
    }
    fn row_label(
        editor: &structured_editor_core::Editor,
        f: &DraftField,
        value: &serde_json::Value,
    ) -> String {
        if f.key.parse::<usize>().is_ok() && value.is_object() {
            let name = embedded::Cells::new(editor).text(&f.schema, value);
            if !name.is_empty() {
                return name;
            }
        }
        label(f)
    }
    let link = |path: &str| {
        let mut p = params.clone();
        if path.is_empty() {
            p.remove("node");
        } else {
            p.insert("node".into(), path.into());
        }
        query_url(url, &p)
    };
    // Arrays are one navigation destination. Listing every row here made large
    // records overwhelm the sidebar; rows are now reached through their tables.
    fn nodes(
        fields: &[DraftField],
        parent: &str,
        root: bool,
        current: &str,
        url: &str,
        params: &std::collections::HashMap<String, String>,
        out: &mut String,
    ) {
        for f in fields.iter().filter(|f| {
            navigable(f)
                && f.key != "_editor"
                && f.schema["x-presentation"]["preview"] != "hidden"
                && f.schema["x-presentation"]["secondary"] != true
        }) {
            let path = if root {
                String::new()
            } else {
                child_path(parent, &f.key)
            };
            let active = current == path || current.starts_with(&format!("{path}/"));
            let mut destination = params.clone();
            if crate::views::embedded::is_table(f) {
                destination.remove("node");
            } else {
                destination.insert("node".into(), path.clone());
            }
            let mut destination_url = query_url(url, &destination);
            if crate::views::embedded::is_table(f) {
                destination_url += &format!("#{}", crate::views::embedded::table_anchor(&path));
            }
            *out += &format!(
                "<a class=\"record-section-link\" {}href=\"{}\"{}><span>{}</span>{}</a>",
                if crate::views::embedded::is_table(f) { "" } else { "data-nav " },
                escape(&destination_url),
                if active { " aria-current=\"page\"" } else { "" },
                escape(&label(f)),
                if f.kind == InputKind::Array {
                    format!("<small>{}</small>", f.items.len())
                } else {
                    String::new()
                }
            );
            if f.kind != InputKind::Array && f.schema["x-presentation"]["navigation"] != "stop" {
                nodes(&f.items, &path, false, current, url, params, out);
            }
        }
    }
    let mut html =
        format!("<template id=\"data-navigation\"><p class=\"eyebrow\">{ui_text_0}</p>", ui_text_0 = crate::views::escape(&crate::i18n::t("This record")));
    html += &format!(
        "<a class=\"record-section-link\" data-nav href=\"{}\"{}>{ui_text_0}</a>",
        escape(&link("")),
        if path.is_empty() {
            " aria-current=\"page\""
        } else {
            ""
        }, ui_text_0 = crate::views::escape(&crate::i18n::t("Overview")));
    nodes(
        &draft.fields,
        "",
        draft.root_value,
        path,
        url,
        params,
        &mut html,
    );
    html += "</template>";
    let selected = draft.at_path(path).filter(|f| navigable(f));
    if !path.is_empty() && selected.is_some() {
        html += &format!(
            "<nav class=\"record-breadcrumbs\" aria-label=\"Record location\"><a data-nav href=\"{}\">{ui_text_0}</a>",
            escape(&link("")), ui_text_0 = crate::views::escape(&crate::i18n::t("Overview")));
        let mut ancestor = String::new();
        for part in path.trim_start_matches('/').split('/') {
            ancestor += &format!("/{part}");
            if let Some(field) = draft.at_path(&ancestor) {
                let name = row_label(
                    editor,
                    field,
                    value.pointer(&ancestor).unwrap_or(&serde_json::Value::Null),
                );
                html += &format!(
                    "<span aria-hidden=\"true\">/</span><a data-nav href=\"{}\"{}>{}</a>",
                    escape(&link(&ancestor)),
                    if ancestor == path {
                        " aria-current=\"page\""
                    } else {
                        ""
                    },
                    escape(&name)
                );
            }
        }
        html += "</nav>";
    }
    if let Some(f) = selected.filter(|f| embedded::is_table(f)) {
        let mut focused_params = params.clone();
        focused_params.insert("node".into(), path.into());
        html += &embedded::render(
            editor,
            f,
            value.pointer(path).unwrap_or(value),
            url,
            path,
            &focused_params,
            editable,
        );
        return html;
    }
    let fields: Vec<(&DraftField, String)> = if let Some(f) = selected {
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
    let mut secondary = String::new();
    let mut details = String::new();
    let mut tables = String::new();
    for (f, field_path) in fields {
        let Some(v) = value.pointer(&field_path) else {
            continue;
        };
        if !visible(f, v) {
            continue;
        }
        if f.key == "_editor" {
            metadata += &crate::views::metadata::preview(v);
            continue;
        }
        let title = row_label(editor, f, v);
        let section = if embedded::is_table(f) {
            embedded::render(editor, f, v, url, &field_path, params, editable)
        } else {
            let content = if navigable(f) {
                format!(
                    "<a class=\"button\" data-nav href=\"{}\">Open {}</a>",
                    escape(&link(&field_path)),
                    escape(&title)
                )
            } else {
                preview::value(editor, f, v)
            };
            format!(
                "<section class=\"preview-field\"><h3>{}</h3><div>{content}</div></section>",
                escape(&title)
            )
        };
        if embedded::is_table(f) {
            tables.push_str(&section);
        } else if f.schema["x-presentation"]["secondary"] == true {
            secondary.push_str(&section);
        } else {
            details.push_str(&section);
        }
    }
    if !secondary.is_empty() {
        details += &format!(
            "<details class=\"record-secondary\"><summary>{ui_text_0}</summary>{secondary}</details>", ui_text_0 = crate::views::escape(&crate::i18n::t("Additional details")));
    }
    details += &metadata;
    if !details.is_empty() {
        html += &format!("<div class=\"surface record-details\">{details}</div>");
    }
    html + &tables
}
