//! Shared presentation for column choices in collection and embedded tables.
use super::{encode, escape};
use serde_json::Value;
use std::collections::HashMap;

pub const GET_FORM: &str = "data-on:submit__prevent=\"@get(el.getAttribute('action')+'?'+new URLSearchParams(new FormData(el)),{filterSignals:{include:/^$/}})\"";

pub struct TableColumns {
    pub available: Vec<(String, String)>,
    pub selected: Vec<(String, String)>,
}
impl TableColumns {
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
                    .filter(|(k, s)| {
                        k.as_str() != "_editor"
                            && s["x-editor"] != "hidden"
                            && s["x-visible"] != false
                    })
                    .map(|(k, s)| (k.clone(), s["title"].as_str().unwrap_or(k).to_owned())),
            );
        }
        let configured: Vec<&str> = ui
            .pointer("/list/columns")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().or_else(|| v["field"].as_str()))
                    .collect()
            })
            .unwrap_or_else(|| defaults.to_vec());
        let mut selected: Vec<_> = available
            .iter()
            .filter(|(key, _)| {
                if params
                    .get(&format!("{prefix}columns"))
                    .is_some_and(|v| v == "custom")
                {
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
        }
    }

    pub fn picker(
        &self,
        scope: &str,
        base: &str,
        params: &HashMap<String, String>,
        prefix: &str,
    ) -> String {
        let selection = format!("{prefix}columns");
        let fields = format!("{prefix}column.");
        let mut html = format!(
            "<details id=\"columns-{scope}\" class=\"action-menu column-picker\"><summary class=\"button icon-button\" aria-label=\"Columns\" data-tooltip=\"Show/hide columns\"><i class=\"ri-layout-column-line\" aria-hidden=\"true\"></i></summary><form class=\"menu-panel\" action=\"{}\" {GET_FORM} data-on:change=\"el.requestSubmit()\" data-indicator:_columns-pending>{}<input type=\"hidden\" name=\"{}\" value=\"custom\"><p class=\"eyebrow\">Visible columns</p><fieldset data-attr:inert=\"$_columnsPending\">",
            escape(base),
            hidden(params, &[&selection, &fields, "offset"]),
            escape(&selection)
        );
        for (key, label) in &self.available {
            let id = format!("{scope}-column-{}", encode(key));
            html += &format!(
                "<label class=\"column-option\" for=\"{id}\"><input id=\"{id}\" type=\"checkbox\" name=\"{}\" value=\"1\" {}><span>{}</span></label>",
                escape(&format!("{fields}{key}")),
                if self.selected.iter().any(|(k, _)| k == key) {
                    "checked"
                } else {
                    ""
                },
                escape(label)
            );
        }
        html += "</fieldset><noscript><button class=\"primary\">Apply columns</button></noscript></form></details>";
        html
    }
}

pub fn query_url(base: &str, params: &HashMap<String, String>) -> String {
    let mut values: Vec<_> = params.iter().filter(|(_, v)| !v.is_empty()).collect();
    values.sort_by(|a, b| a.0.cmp(b.0));
    if values.is_empty() {
        return base.into();
    }
    format!(
        "{base}?{}",
        values
            .into_iter()
            .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
            .collect::<Vec<_>>()
            .join("&")
    )
}
pub fn hidden(params: &HashMap<String, String>, exclude: &[&str]) -> String {
    params
        .iter()
        .filter(|(k, _)| {
            !exclude
                .iter()
                .any(|e| k.as_str() == *e || e.ends_with('.') && k.starts_with(e))
        })
        .map(|(k, v)| {
            format!(
                "<input type=\"hidden\" name=\"{}\" value=\"{}\">",
                escape(k),
                escape(v)
            )
        })
        .collect()
}
