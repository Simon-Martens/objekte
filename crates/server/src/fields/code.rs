use super::*;
pub struct Code;
#[derive(Template)]
#[template(path = "fields/code.html")]
struct Input<'a> {
    id: &'a str,
    raw: &'a str,
    label: String,
    language: &'a str,
    version: u64,
    inline: bool,
    preview: String,
    preview_open: bool,
    actions: String,
}
impl Control for Code {
    fn accepts(&self, field: &DraftField) -> bool {
        field.kind == structured_editor_core::draft::InputKind::Text
            && matches!(
                field.schema.get("x-editor").and_then(|v| v.as_str()),
                Some("markdown" | "inline-markdown" | "html")
            )
    }
    fn render(
        &self,
        field: &DraftField,
        _: &str,
        context: &Context<'_>,
        _: &Registry,
    ) -> Result<String> {
        let editor = field.schema["x-editor"].as_str().unwrap_or("markdown");
        Ok(Input {
            id: &field.id,
            raw: &field.raw,
            label: title(field),
            language: if editor == "html" { "html" } else { "markdown" },
            version: context.version,
            inline: editor == "inline-markdown"
                || field
                    .schema
                    .pointer("/x-presentation/mode")
                    .and_then(|v| v.as_str())
                    == Some("inline"),
            actions: format!(
                "/edit-sessions/{}/fields/{}/actions",
                context.session, field.id
            ),
            preview_open: context.preview.contains(&field.id) && editor != "html",
            preview: if context.preview.contains(&field.id) && editor != "html" {
                crate::views::markdown(&field.raw)
            } else {
                String::new()
            },
        }
        .render()?)
    }
}
