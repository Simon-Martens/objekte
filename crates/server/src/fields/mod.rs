//! Web control registry. Semantic decoding lives in core::draft, not these renderers.
pub(crate) mod array;
mod code;
mod object;
mod reference;
mod scalar;
mod system;
use crate::error::Result;
use crate::templates::Template;
use structured_editor_core::{ValidationIssue, draft::DraftField};
pub struct Context<'a> {
    pub focus: &'a str,
    pub session: &'a str,
    pub version: u64,
    pub table_offsets: &'a std::collections::HashMap<String, usize>,
    pub table_search: &'a std::collections::HashMap<String, crate::sessions::TableSearch>,
    pub revealed_fields: &'a std::collections::BTreeSet<String>,
    pub editor: &'a structured_editor_core::Editor,
    pub collection: &'a str,
    pub value: &'a serde_json::Value,
    pub preview: &'a std::collections::BTreeSet<String>,
    pub issues: &'a [ValidationIssue],
}
pub trait Control: Send + Sync {
    fn accepts(&self, field: &DraftField) -> bool;
    fn render(
        &self,
        field: &DraftField,
        path: &str,
        context: &Context<'_>,
        registry: &Registry,
    ) -> Result<String>;
}
pub struct Registry {
    controls: Vec<Box<dyn Control>>,
}
fn title(field: &DraftField) -> String {
    crate::tree::label(field)
}
impl Default for Registry {
    fn default() -> Self {
        Self {
            controls: vec![
                Box::new(system::System),
                Box::new(object::Object),
                Box::new(array::Array),
                Box::new(reference::Reference),
                Box::new(code::Code),
                Box::new(scalar::Scalar),
            ],
        }
    }
}
impl Registry {
    pub fn render(
        &self,
        field: &DraftField,
        path: &str,
        context: &Context<'_>,
        item: bool,
    ) -> Result<String> {
        if field.hidden
            || (!field.present && !item && !required_control(field) && field.key != "_editor")
        {
            return Ok(String::new());
        }
        let control = self
            .controls
            .iter()
            .find(|c| c.accepts(field))
            .expect("fallback control");
        let content = if crate::tree::navigable(field)
            && field.id != context.focus
            && !crate::views::embedded::is_table(field)
        {
            format!(
                "<button formaction=\"/edit-sessions/{}/fields/{}/actions/browse\">Open {} <span class=\"badge\">{}</span></button>",
                context.session,
                field.id,
                crate::views::escape(&title(field)),
                field
                    .items
                    .iter()
                    .filter(|f| f.present && !f.hidden)
                    .count()
            )
        } else {
            control.render(field, path, context, self)?
        };
        let title = title(field);
        let errors: Vec<&str> = context
            .issues
            .iter()
            .filter(|i| i.path == path)
            .map(|i| i.message.as_str())
            .collect();
        Ok(Field {
            focused: field.id == context.focus,
            heading: field.id != context.focus && !crate::views::embedded::is_table(field),
            id: &field.id,
            // An explicit Add item action creates an editable slot, even without a default.
            present: field.present || item || required_control(field),
            deletable: !field.required
                && field.id != context.focus
                && field.key != "_editor"
                && !field.key.is_empty(),
            session: context.session,
            title: &title,
            kind: if field.key == "_editor" {
                "system fields"
            } else {
                field.schema["x-editor"]
                    .as_str()
                    .or_else(|| field.schema["type"].as_str())
                    .unwrap_or("value")
            },
            description: crate::i18n::text(&field.schema, "description", ""),
            content: &content,
            errors,
            required: field.required,
            item,
            read_only: field.read_only,
        }
        .render()?)
    }
}
crate::templates::template!(Field<'_>, "fields/wrapper.html");
#[derive(serde::Serialize)]
struct Field<'a> {
    focused: bool,
    heading: bool,
    session: &'a str,
    present: bool,
    deletable: bool,
    id: &'a str,
    title: &'a str,
    kind: &'a str,
    description: &'a str,
    content: &'a str,
    errors: Vec<&'a str>,
    required: bool,
    item: bool,
    read_only: bool,
}

/// Keep empty optional controls available without overwhelming the main form.
/// Present controls stay in the form, so collapsing them never changes the data.
pub fn empty_optional(field: &DraftField, path: &str, context: &Context<'_>) -> bool {
    use structured_editor_core::draft::InputKind;
    !context.revealed_fields.contains(&field.id)
        && !field.required
        && field.schema["x-presentation"]["hideEmpty"] == true
        && field.schema["x-presentation"]["primary"] != true
        && !context
            .issues
            .iter()
            .any(|i| i.path == path || i.path.starts_with(&format!("{path}/")))
        && (!field.present
            || match field.kind {
                InputKind::Text => field.raw.trim().is_empty(),
                InputKind::Array => {
                    field.items.is_empty() && !crate::views::embedded::is_table(field)
                }
                _ => false,
            })
}
pub fn details(label: &str, html: &str) -> String {
    if html.is_empty() {
        String::new()
    } else {
        format!("<details class=\"record-secondary\"><summary>{label}</summary>{html}</details>")
    }
}

pub fn required_control(field: &DraftField) -> bool {
    use structured_editor_core::draft::InputKind;
    field.required
        && !matches!(
            field.kind,
            InputKind::Object | InputKind::Array | InputKind::Null | InputKind::Unsupported
        )
}
