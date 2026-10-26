use super::*;
use crate::views::{escape, record_url};
pub struct Reference;
impl Control for Reference {
    fn accepts(&self, f: &DraftField) -> bool {
        f.schema["x-presentation"]["editor"] != "raw"
            && f.schema["x-editor"] != "json"
            && (f.schema.get("x-relation").is_some()
                || f.schema["x-content"] == "media"
                || f.schema["x-editor"] == "media")
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
        let kind = target
            .map(|t| crate::views::record_kind(c.editor, t))
            .transpose()?
            .unwrap_or_else(|| "media".into());
        let choose = if value.is_empty() {
            crate::i18n::message("Choose {type}", &[("type", &kind)])
        } else {
            crate::i18n::message("Change {type}", &[("type", &kind)])
        };
        let href = if let Some(target) = target {
            record_url(target, &value)
        } else {
            let collection = f
                .schema
                .pointer("/x-media/collection")
                .and_then(|v| v.as_str())
                .unwrap_or("media");
            format!(
                "/media/{}/file?path={}",
                crate::views::encode(collection),
                crate::views::encode(&value)
            )
        };
        let selected = if value.is_empty() {
            String::new()
        } else {
            format!(
                "<a class=\"reference-name\" {} href=\"{}\">{} <i class=\"ri-arrow-right-up-line\" aria-hidden=\"true\"></i></a>",
                if target.is_some() { "data-nav" } else { "" },
                escape(&href),
                escape(&label)
            )
        };
        let mut html = format!(
            "<div class=\"reference-control\">{selected}<button aria-label=\"{}\" id=\"input-{}\" data-reference-picker aria-describedby=\"errors-{}\" formaction=\"/edit-sessions/{}/fields/{}/actions/picker\"><i class=\"ri-search-line\" aria-hidden=\"true\"></i>{}</button></div><details class=\"reference-manual\"><summary>Enter {} manually</summary><label for=\"reference-id-{}\">{}</label><input id=\"reference-id-{}\" name=\"value.{}\" value=\"{}\" aria-describedby=\"errors-{}\" aria-required=\"{}\" autocomplete=\"off\">",
            escape(&choose),
            f.id,
            f.id,
            c.session,
            f.id,
            escape(&choose),
            if target.is_some() { "ID" } else { "path" },
            f.id,
            if target.is_some() {
                "Record ID"
            } else {
                "Media path"
            },
            f.id,
            f.id,
            escape(&f.raw),
            f.id,
            f.required
        );
        if f.kind == structured_editor_core::draft::InputKind::Json {
            html += "<p class=\"help\">Enter a JSON string or null. Choosing a record fills this automatically.</p>";
        }
        html += "</details>";
        Ok(html)
    }
}
