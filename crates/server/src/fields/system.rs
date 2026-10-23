use super::*;
use crate::views::{escape, metadata};
pub struct System;
impl Control for System {
    fn accepts(&self, f: &DraftField) -> bool {
        f.key == "_editor"
    }
    fn render(
        &self,
        f: &DraftField,
        path: &str,
        context: &Context<'_>,
        _: &Registry,
    ) -> Result<String> {
        let v = serde_json::from_str::<serde_json::Value>(&f.raw).unwrap_or_default();
        let order = v
            .get("manual_order")
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| v.to_string())
            })
            .unwrap_or_default();
        let mut html = format!(
            "<label>Manual order<input name=\"manual_order.{}\" value=\"{}\" inputmode=\"numeric\"></label><p class=\"help\">Blank means unordered.</p>",
            f.id,
            escape(&order)
        );
        if let Some(fields) = v.as_object() {
            for (key, value) in fields {
                if key == "manual_order" {
                    continue;
                }
                // Root record revision is already in the validation inspector.
                if key == "schema_revision" && !context.collection.is_empty() && path == "/_editor"
                {
                    continue;
                }
                html += &format!(
                    "<div class=\"system-property\"><span>{}</span> {}</div>",
                    escape(&metadata::label(key)),
                    metadata::value(key, value)
                );
            }
        }
        Ok(html)
    }
}
