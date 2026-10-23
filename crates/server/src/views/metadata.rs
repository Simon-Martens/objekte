//! System metadata has one readable presentation across structured views.
use super::escape;
use serde_json::Value;

pub fn label(key: &str) -> String {
    match key {
        "_editor" => "Editor metadata".into(),
        "schema_revision" => "Schema revision".into(),
        "manual_order" => "Manual order".into(),
        _ => key.replace('_', " "),
    }
}
pub fn value(key: &str, value: &Value) -> String {
    match value {
        Value::Object(fields) => {
            if fields.is_empty() {
                return "<span class=\"muted\">Not assigned</span>".into();
            }
            let mut html = String::from("<dl class=\"system-metadata\">");
            for (key, child) in fields {
                html += &format!(
                    "<dt>{}</dt><dd>{}</dd>",
                    escape(&label(key)),
                    self::value(key, child)
                );
            }
            html + "</dl>"
        }
        Value::Array(items) => {
            if items.is_empty() {
                return "<span class=\"muted\">No items</span>".into();
            }
            let mut html = String::from("<ul>");
            for item in items {
                html += &format!("<li>{}</li>", self::value(key, item));
            }
            html + "</ul>"
        }
        Value::String(text) if key == "schema_revision" => {
            let short = if text.len() == 40 && text.bytes().all(|b| b.is_ascii_hexdigit()) {
                &text[..8]
            } else {
                text
            };
            format!("<code>{}</code>", escape(short))
        }
        Value::String(text) => format!("<span>{}</span>", escape(text)),
        Value::Number(number) => format!("<span class=\"mono\">{number}</span>"),
        Value::Bool(v) => if *v { "Yes" } else { "No" }.into(),
        Value::Null => "<span class=\"muted\">Not assigned</span>".into(),
    }
}
