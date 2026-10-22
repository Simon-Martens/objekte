use super::*;
use structured_editor_core::draft::InputKind;
pub struct Scalar;
#[derive(Template)]
#[template(path = "fields/scalar.html")]
struct Input<'a> {
    id: &'a str,
    raw: &'a str,
    kind: &'a str,
    options: Vec<(String, String, bool)>,
    suggestions: Vec<String>,
    suggestions_url: String,
}
impl Control for Scalar {
    fn accepts(&self, _: &DraftField) -> bool {
        true
    }
    fn render(
        &self,
        field: &DraftField,
        _: &str,
        context: &Context<'_>,
        _: &Registry,
    ) -> Result<String> {
        let mut options = Vec::new();
        let kind = match field.kind {
            InputKind::Boolean => "boolean",
            InputKind::Enum => {
                options = field.schema["enum"]
                    .as_array()
                    .map(|values| {
                        values
                            .iter()
                            .map(|v| {
                                let raw = v.to_string();
                                let label =
                                    v.as_str().map(str::to_owned).unwrap_or_else(|| raw.clone());
                                let selected =
                                    serde_json::from_str::<serde_json::Value>(&field.raw)
                                        .ok()
                                        .as_ref()
                                        == Some(v);
                                (raw, label, selected)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                "select"
            }
            InputKind::Json => "json",
            InputKind::Integer => "integer",
            InputKind::Number => "numeric",
            InputKind::Text
                if field.schema["format"] == "date" || field.schema["x-editor"] == "date" =>
            {
                "date"
            }
            InputKind::Text
                if field.schema["format"] == "date-time"
                    || field.schema["x-editor"] == "datetime" =>
            {
                "datetime"
            }
            InputKind::Text
                if field
                    .schema
                    .pointer("/x-presentation/mode")
                    .and_then(|v| v.as_str())
                    == Some("multiline") =>
            {
                "multiline"
            }
            InputKind::Text if field.raw.contains(['\r', '\n']) => "multiline",
            _ => "text",
        };
        if kind == "select" && !field.raw.is_empty() && !options.iter().any(|o| o.2) {
            options.insert(
                0,
                (
                    field.raw.clone(),
                    format!("Current invalid value: {}", field.raw),
                    true,
                ),
            );
        }
        let mut suggestions = if matches!(
            field.schema["x-editor"].as_str(),
            Some("autocomplete" | "autocomplete-select")
        ) {
            context
                .editor
                .suggestions(context.collection, &field.key, context.value)
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        if let Some(options) = field.schema["x-options"].as_array() {
            suggestions.extend(options.iter().filter_map(|v| v.as_str()).map(str::to_owned));
        }
        suggestions.sort_by(|a, b| structured_editor_core::alphabetic_cmp(a, b).then(a.cmp(b)));
        suggestions.dedup();
        Ok(Input {
            id: &field.id,
            raw: &field.raw,
            kind,
            options,
            suggestions_url: if field.schema.get("x-options").is_some()
                || matches!(
                    field.schema["x-editor"].as_str(),
                    Some("autocomplete" | "autocomplete-select")
                ) {
                format!(
                    "/edit-sessions/{}/fields/{}/suggestions",
                    context.session, field.id
                )
            } else {
                String::new()
            },
            suggestions,
        }
        .render()?)
    }
}
