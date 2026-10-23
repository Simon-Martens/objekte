//! Schema-generated instance settings. Content schemas and their data remain independent.
use crate::{
    auth::Actor,
    error::{Result, WebError},
    views,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use structured_editor_core::{
    Editor,
    draft::{Draft, FieldInput, InputKind},
};
pub fn value(editor: &Editor, form: &HashMap<String, String>) -> Result<Value> {
    if !form.contains_key("settings_form") {
        return serde_json::from_str(form.get("raw").map(String::as_str).unwrap_or(""))
            .map_err(|e| WebError::bad(e.to_string()));
    }
    let current = editor.settings()?;
    let schema = editor.settings_schema()?;
    let mut draft = Draft::new(schema, &current.value);
    let mut inputs = HashMap::new();
    for field in &draft.fields {
        if schema["properties"].get(&field.key).is_none() {
            continue;
        }
        let raw = form
            .get(&format!("setting.{}", field.key))
            .cloned()
            .unwrap_or_else(|| {
                if field.kind == InputKind::Boolean {
                    "false".into()
                } else {
                    field.raw.clone()
                }
            });
        inputs.insert(field.id.clone(), FieldInput { present: true, raw });
    }
    draft.apply(&inputs);
    draft.value().map_err(|issues| {
        WebError::bad(
            issues
                .into_iter()
                .map(|i| format!("{}: {}", i.path, i.message))
                .collect::<Vec<_>>()
                .join("; "),
        )
    })
}
pub fn render(
    editor: &Editor,
    actor: &Actor,
    raw: &str,
    revision: &str,
    message: &str,
) -> Result<Option<String>> {
    let schema = editor.settings_schema()?;
    let Some(props) = schema["properties"].as_object().filter(|p| !p.is_empty()) else {
        return Ok(None);
    };
    let value: Value = serde_json::from_str(raw).unwrap_or(json!({}));
    let mut body = format!(
        "<form id=\"document-form\" class=\"preferences-form\" action=\"/settings\" method=\"post\" data-live-validation=\"/settings/validation\" data-indicator:_pending data-on:submit__prevent=\"@post(el.getAttribute('action'),{{contentType:'form',retry:'never',retryMaxCount:0}})\"><span data-validation-request data-on:validate-draft=\"@post(el.closest('form').dataset.liveValidation,{{contentType:'form',requestCancellation:evt.detail.controller,retry:'never',retryMaxCount:0}})\"></span><input type=\"hidden\" name=\"settings_form\" value=\"1\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><input type=\"hidden\" name=\"revision\" value=\"{}\"><div class=\"preferences-grid\">",
        views::escape(&actor.csrf),
        views::escape(revision)
    );
    for (key, s) in props {
        let v = value
            .get(key)
            .or_else(|| s.get("default"))
            .cloned()
            .unwrap_or(Value::Null);
        let title = s["title"].as_str().unwrap_or(key);
        let name = format!("setting.{key}");
        body += &format!(
            "<section class=\"surface preference-card\"><label for=\"setting-{}\">{}</label><p class=\"help\">{}</p>",
            views::escape(key),
            views::escape(title),
            views::escape(s["description"].as_str().unwrap_or(""))
        );
        let attrs = format!(
            "id=\"setting-{}\" name=\"{}\" {}",
            views::escape(key),
            views::escape(&name),
            if actor.editable() { "" } else { "disabled" }
        );
        if let Some(options) = s["enum"].as_array() {
            body += &format!("<select {attrs}>");
            for option in options {
                body += &format!(
                    "<option value=\"{}\" {}>{}</option>",
                    views::escape(&option.to_string()),
                    if option == &v { "selected" } else { "" },
                    views::escape(&structured_editor_core::display_text(option))
                );
            }
            body += "</select>";
        } else {
            match s["type"].as_str() {
                Some("boolean") => {
                    body += &format!(
                        "<label class=\"boolean-control\"><input type=\"checkbox\" {attrs} value=\"true\" {}><span>Enabled</span></label>",
                        if v == true { "checked" } else { "" }
                    )
                }
                Some("integer" | "number") => {
                    body += &format!(
                        "<input type=\"number\" {attrs} value=\"{}\" min=\"{}\" max=\"{}\" step=\"{}\">",
                        views::escape(&structured_editor_core::display_text(&v)),
                        s["minimum"]
                            .as_f64()
                            .map(|v| v.to_string())
                            .unwrap_or_default(),
                        s["maximum"]
                            .as_f64()
                            .map(|v| v.to_string())
                            .unwrap_or_default(),
                        if s["type"] == "integer" { "1" } else { "any" }
                    )
                }
                Some("string") => {
                    body += &format!(
                        "<input {attrs} value=\"{}\">",
                        views::escape(v.as_str().unwrap_or(""))
                    )
                }
                _ => {
                    body += &format!(
                        "<textarea {attrs} class=\"source\">{}</textarea>",
                        views::escape(&v.to_string())
                    )
                }
            }
        }
        body += "</section>";
    }
    body += &format!(
        "</div><template data-notification=\"success\">{}</template><div id=\"document-validation\" class=\"help\" role=\"status\"></div><div class=\"settings-footer actions\"><span class=\"help\">Instance defaults · personal theme choices take precedence</span><button name=\"action\" value=\"validate\">Validate</button><button class=\"primary\" name=\"action\" value=\"save\" {}><i class=\"ri-save-line\" aria-hidden=\"true\"></i>Save settings</button></div></form>",
        views::escape(message),
        if actor.editable() { "" } else { "disabled" }
    );
    Ok(Some(views::panel("/settings", "Editor settings", &body)?))
}
pub fn fragment(editor: &Editor) -> String {
    let value = editor.settings().ok().map(|d| d.value).unwrap_or(json!({}));
    format!(
        "<div id=\"ui-preferences\" hidden data-density=\"{}\" data-editor-font=\"{}\" data-validation-delay=\"{}\" data-default-theme=\"{}\"></div>",
        if value["density"] == "comfortable" {
            "comfortable"
        } else {
            "compact"
        },
        value["editorFontSize"].as_u64().unwrap_or(13).clamp(11, 20),
        value["validationDelay"]
            .as_u64()
            .unwrap_or(400)
            .clamp(150, 2000),
        match value["defaultTheme"].as_str() {
            Some("dark") => "dark",
            Some("light") => "light",
            _ => "system",
        }
    )
}
