use super::{escape, markdown, record_url};
use serde_json::Value;
use structured_editor_core::{
    Editor,
    draft::{DraftField, InputKind},
};

/// A friendly option label never changes the JSON value submitted by a control.
pub fn option_label<'a>(schema: &'a Value, value: &Value) -> Option<&'a str> {
    if !schema["enum"].as_array()?.contains(value) {
        return None;
    }
    let key = structured_editor_core::display_text(value);
    crate::i18n::translation(schema).and_then(|v|v.get("x-optionLabels")).and_then(|v|v.get(&key)).and_then(Value::as_str)
        .or_else(|| schema["x-optionLabels"].get(key).and_then(Value::as_str))
}

/// Use the same resolved field definitions as the editor, including array items.
pub fn value(editor: &Editor, field: &DraftField, value: &Value) -> String {
    if field.hidden {
        return String::new();
    }
    if field.key == "_editor" {
        return super::metadata::preview(value);
    }
    if field.kind == InputKind::Unsupported {
        return "<p class=\"muted\">Value preserved; structured display unavailable and automatic raw fallback disabled.</p>".into();
    }
    if field.kind == InputKind::Object {
        let mut html = format!("<div id=\"node-{}\" class=\"preview-object\">", field.id);
        let mut metadata = String::new();
        for child in &field.items {
            if !child.hidden {
                if let Some(value) = value.get(&child.key) {
                    if child.key == "_editor" {
                        metadata += &super::metadata::preview(value);
                        continue;
                    }
                    html += &format!(
                        "<section class=\"preview-field\"><h3>{}</h3>{}</section>",
                        escape(&crate::tree::label(child)),
                        self::value(editor, child, value)
                    );
                }
            }
        }
        return html + &metadata + "</div>";
    }
    if field.kind == InputKind::Array {
        if let Some(items) = value.as_array() {
            if items.is_empty() {
                return "<span class=\"muted\">No items</span>".into();
            }
            let mut html = if field.schema["x-presentation"]["inline"] == true {
                String::from("<ol class=\"preview-array preview-inline\">")
            } else {
                String::from("<ol class=\"preview-array\">")
            };
            for (index, item) in items.iter().enumerate() {
                html += "<li>";
                html += &field
                    .items
                    .get(index)
                    .map(|f| self::value(editor, f, item))
                    .unwrap_or_else(|| json(item));
                html += "</li>";
            }
            html += "</ol>";
            return html;
        }
    }
    if field.schema["x-presentation"]["editor"] == "raw" || field.schema["x-editor"] == "json" {
        return json(value);
    }
    let schema = &field.schema;
    if let (Some(target), Some(id)) = (
        schema
            .pointer("/x-relation/collection")
            .and_then(Value::as_str),
        value.as_str(),
    ) {
        let label = editor
            .get(target, id)
            .ok()
            .and_then(|d| editor.label(target, &d.value, id, "reference").ok())
            .unwrap_or_else(|| id.into());
        return format!(
            "<a data-nav href=\"{}\">{}</a>",
            record_url(target, id),
            escape(&label)
        );
    }
    if (schema["x-content"] == "media" || schema["x-editor"] == "media") && value.is_string() {
        let collection = schema
            .pointer("/x-media/collection")
            .and_then(Value::as_str)
            .unwrap_or("media");
        let path = value.as_str().unwrap();
        return format!(
            "<a href=\"/media/{}/file?path={}\">{}</a>",
            super::encode(collection),
            super::encode(path),
            escape(path)
        );
    }
    if field.kind == InputKind::Json {
        return json(value);
    }
    if let Some(label) = option_label(schema, value) {
        return format!("<span class=\"preview-text\">{}</span>", escape(label));
    }
    if let Some(text) = value.as_str() {
        if text.is_empty() {
            return "<span class=\"muted\">Empty text</span>".into();
        }
        if (schema["format"] == "uri" || schema["x-presentation"]["link"] == true)
            && (text.starts_with("https://") || text.starts_with("http://"))
        {
            return format!(
                "<a href=\"{}\" target=\"_blank\" rel=\"noopener noreferrer\">{}</a>",
                escape(text),
                escape(text)
            );
        }
        if schema["x-editor"] == "html" || schema["x-content"] == "html" {
            return format!("<pre class=\"preview-source\">{}</pre>", escape(text));
        }
        if ["x-content", "x-editor"]
            .iter()
            .any(|key| matches!(schema[key].as_str(), Some("markdown" | "inline-markdown")))
        {
            return markdown(text);
        }
        return format!("<span class=\"preview-text\">{}</span>", escape(text));
    }
    match value {
        Value::Bool(v) => format!("<span>{}</span>", if *v { "Yes" } else { "No" }),
        Value::Number(v) => format!("<span class=\"mono\">{v}</span>"),
        _ => json(value),
    }
}

fn json(value: &Value) -> String {
    format!(
        "<pre class=\"preview-source\">{}</pre>",
        escape(&serde_json::to_string_pretty(value).unwrap_or_default())
    )
}
