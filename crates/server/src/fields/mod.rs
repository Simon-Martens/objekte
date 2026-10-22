//! Web control registry. Semantic decoding lives in core::draft, not these renderers.
mod array;
mod code;
mod reference;
mod scalar;
mod system;
use crate::error::Result;
use askama::Template;
use structured_editor_core::{ValidationIssue, draft::DraftField};
pub struct Context<'a> {
    pub session: &'a str,
    pub version: u64,
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
impl Default for Registry {
    fn default() -> Self {
        Self {
            controls: vec![
                Box::new(system::System),
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
        if field.hidden || (!field.present && field.key != "_editor") {
            return Ok(String::new());
        }
        let control = self
            .controls
            .iter()
            .find(|c| c.accepts(field))
            .expect("fallback control");
        let content = control.render(field, path, context, self)?;
        let title = field
            .schema
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or(&field.key);
        let errors: Vec<&str> = context
            .issues
            .iter()
            .filter(|i| i.path == path)
            .map(|i| i.message.as_str())
            .collect();
        Ok(Field {
            id: &field.id,
            present: field.present,
            deletable: field.key != "_editor",
            session: context.session,
            title,
            kind: field.schema["x-editor"]
                .as_str()
                .or_else(|| field.schema["type"].as_str())
                .unwrap_or("value"),
            description: field
                .schema
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
            content: &content,
            errors,
            required: field.required,
            item,
            read_only: field.read_only,
        }
        .render()?)
    }
}
#[derive(Template)]
#[template(path = "fields/wrapper.html")]
struct Field<'a> {
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
