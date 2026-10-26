//! Shared column choices for collection lists and individual child tables.
use super::{encode, escape, list};
use serde_json::Value;
use std::collections::HashMap;

pub(crate) struct Columns {
    pub available: Vec<(String, String)>,
    pub selected: Vec<(String, String)>,
    prefix: String,
}

impl Columns {
    pub fn new(
        schema: &Value,
        ui: &Value,
        params: &HashMap<String, String>,
        prefix: &str,
        defaults: &[&str],
    ) -> Self {
        let mut available = vec![
            ("_label".into(), "Record".into()),
            ("_id".into(), "Identifier".into()),
        ];
        if let Some(props) = schema["properties"].as_object() {
            available.extend(
                props
                    .iter()
                    .filter(|(key, field)| {
                        key.as_str() != "_editor"
                            && field["x-editor"] != "hidden"
                            && field["x-visible"] != false
                    })
                    .map(|(key, field)| {
                        (key.clone(), field["title"].as_str().unwrap_or(key).into())
                    }),
            );
        }
        let configured: Vec<&str> = ui
            .pointer("/list/columns")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().or_else(|| value["field"].as_str()))
                    .collect()
            })
            .unwrap_or_else(|| defaults.to_vec());
        let custom = params
            .get(&format!("{prefix}columns"))
            .is_some_and(|v| v == "custom");
        let mut selected: Vec<_> = available
            .iter()
            .filter(|(key, _)| {
                if custom {
                    params.contains_key(&format!("{prefix}column.{key}"))
                } else {
                    configured.contains(&key.as_str())
                }
            })
            .cloned()
            .collect();
        if selected.is_empty() {
            selected.push(available[0].clone());
        }
        selected.sort_by_key(|(key, _)| {
            configured
                .iter()
                .position(|v| *v == key)
                .unwrap_or(usize::MAX)
        });
        Self {
            available,
            selected,
            prefix: prefix.into(),
        }
    }

    pub fn picker(&self, base: &str, params: &HashMap<String, String>) -> String {
        let id = format!("column-picker-{}", encode(&self.prefix));
        let mode = format!("{}columns", self.prefix);
        let field_prefix = format!("{}column.", self.prefix);
        let mut html = format!(
            "<details id=\"{id}\" class=\"action-menu column-picker\"><summary class=\"button icon-button\" aria-label=\"Columns\" data-tooltip=\"Columns\"><i class=\"ri-layout-column-line\" aria-hidden=\"true\"></i></summary><form class=\"menu-panel\" action=\"{}\" {} data-on:change=\"el.requestSubmit()\" data-indicator:_columns-pending>{}<input type=\"hidden\" name=\"{}\" value=\"custom\"><p class=\"eyebrow\">Visible columns</p><fieldset data-attr:inert=\"$_columnsPending\">",
            escape(base),
            list::GET,
            list::hidden(params, &[&mode, &field_prefix, "offset"]),
            escape(&mode),
        );
        for (key, label) in &self.available {
            let input_id = format!("{id}-{}", encode(key));
            html += &format!(
                "<label class=\"column-option\" for=\"{input_id}\"><input id=\"{input_id}\" type=\"checkbox\" name=\"{}\" value=\"1\" {}><span>{}</span></label>",
                escape(&format!("{field_prefix}{key}")),
                if self.selected.iter().any(|(k, _)| k == key) {
                    "checked"
                } else {
                    ""
                },
                escape(label),
            );
        }
        html += "</fieldset><noscript><button class=\"primary\">Apply columns</button></noscript></form></details>";
        html
    }
}
