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
        let mut optional = String::new();
        let mut secondary = String::new();
        let mut tables = String::new();
        for child in &field.items {
            let destination = if crate::views::embedded::is_table(child) {
                &mut tables
            } else if !child.required && !context.revealed_fields.contains(&child.id) && child.schema["x-presentation"]["secondary"] == true {
                &mut secondary
            } else if empty_optional(child, &child_path(path, &child.key), context) {
                &mut optional
            } else {
                &mut html
            };
            *destination +=
                &registry.render(child, &child_path(path, &child.key), context, false)?;
            if !child.present && !child.hidden && !child.read_only && !required_control(child) {
                *destination += &format!(
                    "<button class=\"small\" formaction=\"/edit-sessions/{}/fields/{}/actions/include\">Add {}</button>",
                    context.session,
                    child.id,
                    crate::views::escape(crate::i18n::field(&child.schema, &child.key))
                );
            }
        }
        html += &details(&crate::i18n::t("More fields"), &optional);
        html += &details(&crate::i18n::t("Additional details"), &secondary);
        html += &tables;
        if html.is_empty() {
            html += "<p class=\"muted\">Empty object</p>";
        }
        Ok(html)
    }
}
