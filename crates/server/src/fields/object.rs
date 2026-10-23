use super::*;
use structured_editor_core::draft::{InputKind, child_path};
pub struct Object;
impl Control for Object {
    fn accepts(&self, field: &DraftField) -> bool {
        field.kind == InputKind::Object
    }
    fn render(
        &self,
        field: &DraftField,
        path: &str,
        context: &Context<'_>,
        registry: &Registry,
    ) -> Result<String> {
        let mut html = String::new();
        for child in &field.items {
            html += &registry.render(child, &child_path(path, &child.key), context, false)?;
            if !child.present && !child.hidden && !child.read_only {
                html += &format!(
                    "<button class=\"small\" formaction=\"/edit-sessions/{}/fields/{}/actions/include\">Add {}</button>",
                    context.session,
                    child.id,
                    crate::views::escape(child.schema["title"].as_str().unwrap_or(&child.key))
                );
            }
        }
        if html.is_empty() {
            html += "<p class=\"muted\">Empty object</p>";
        }
        Ok(html)
    }
}
