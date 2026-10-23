use super::*;
use crate::views::escape;
pub struct System;
impl Control for System {
    fn accepts(&self, f: &DraftField) -> bool {
        f.key == "_editor"
    }
    fn render(&self, f: &DraftField, _: &str, _: &Context<'_>, _: &Registry) -> Result<String> {
        let v = serde_json::from_str::<serde_json::Value>(&f.raw).unwrap_or_default();
        let order = v
            .get("manual_order")
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| v.to_string())
            })
            .unwrap_or_default();
        Ok(format!(
            "<label>Manual order<input name=\"manual_order.{}\" value=\"{}\" inputmode=\"numeric\"></label><p class=\"help\">Blank means unordered. Order applies within this collection.</p>",
            f.id,
            escape(&order),
        ))
    }
}
