use super::*;
use crate::views::{escape, record_url};
pub struct Reference;
impl Control for Reference {
    fn accepts(&self, f: &DraftField) -> bool {
        f.schema.get("x-relation").is_some()
            || f.schema["x-content"] == "media"
            || f.schema["x-editor"] == "media"
    }
    fn render(&self, f: &DraftField, _: &str, c: &Context<'_>, _: &Registry) -> Result<String> {
        let target = f
            .schema
            .pointer("/x-relation/collection")
            .and_then(|v| v.as_str());
        let value = if f.kind == structured_editor_core::draft::InputKind::Text {
            f.raw.clone()
        } else {
            serde_json::from_str::<serde_json::Value>(&f.raw)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default()
        };
        let label = target
            .and_then(|t| {
                c.editor
                    .get(t, &value)
                    .ok()
                    .and_then(|d| c.editor.label(t, &d.value, &value, "reference").ok())
            })
            .unwrap_or_else(|| value.clone());
        let mut html = format!(
            "<div class=\"reference-control\"><input id=\"input-{}\" name=\"value.{}\" value=\"{}\"><button formaction=\"/edit-sessions/{}/fields/{}/actions/picker\"><i class=\"ri-search-line\" aria-hidden=\"true\"></i>{}</button></div>",
            f.id,
            f.id,
            escape(&f.raw),
            c.session,
            f.id,
            if target.is_some() {
                "Choose record"
            } else {
                "Choose media"
            }
        );
        if f.kind == structured_editor_core::draft::InputKind::Json {
            html += "<p class=\"help\">JSON string or null. The picker supplies the correct JSON type.</p>";
        }
        if let Some(target) = target {
            if !value.is_empty() {
                html += &format!(
                    "<p class=\"reference-label\"><a data-nav href=\"{}\">{} <i class=\"ri-arrow-right-up-line\" aria-hidden=\"true\"></i></a></p>",
                    record_url(target, &value),
                    escape(&label)
                );
            }
        }
        if target.is_none() && !value.is_empty() {
            let collection = f
                .schema
                .pointer("/x-media/collection")
                .and_then(|v| v.as_str())
                .unwrap_or("media");
            html += &format!(
                "<p class=\"reference-label\"><a href=\"/media/{}/file?path={}\">Open media <i class=\"ri-arrow-right-up-line\" aria-hidden=\"true\"></i></a></p>",
                crate::views::encode(collection),
                crate::views::encode(&value)
            );
        }
        Ok(html)
    }
}
