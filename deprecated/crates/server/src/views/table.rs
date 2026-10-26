//! Shared presentation for column choices in collection and embedded tables.
use super::{encode, escape};
use serde_json::Value;
use std::collections::HashMap;

pub const GET_FORM: &str = "data-on:submit__prevent=\"@get(el.getAttribute('action')+(el.getAttribute('action').includes('?')?'&':'?')+new URLSearchParams(new FormData(el)),{filterSignals:{include:/^$/}})\"";

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
            ("_label".into(), crate::i18n::t("Record")),
            ("_id".into(), crate::i18n::t("Identifier")),
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
                    .map(|(k, s)| (k.clone(), crate::i18n::field(s,k).to_owned())),
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
        self.picker_with_submit(scope, base, params, prefix, GET_FORM, None)
    }

    pub fn picker_with_submit(
        &self,
        scope: &str,
        base: &str,
        params: &HashMap<String, String>,
        prefix: &str,
        submit: &str,
        anchor: Option<&str>,
    ) -> String {
        let selection = format!("{prefix}columns");
        let fields = format!("{prefix}column.");
        let mut html = format!(
            "<details id=\"columns-{scope}\" class=\"action-menu column-picker\"><summary class=\"button small\" aria-label=\"{ui_text_1}\" data-tooltip=\"{ui_text_2}\"><i class=\"ri-layout-column-line\" aria-hidden=\"true\"></i><span>{ui_text_0}</span></summary><form class=\"menu-panel\" action=\"{}\" {submit} data-on:change=\"el.requestSubmit()\" data-indicator:_columns-pending>{}<input type=\"hidden\" name=\"{}\" value=\"custom\"><p class=\"eyebrow\">{ui_text_3}</p><fieldset data-attr:inert=\"$_columnsPending\">",
            escape(base),
            format!("{}{}", hidden(params, &[&selection, &fields, "offset", "table_anchor"]), anchor.map(|anchor| format!("<input type=\"hidden\" name=\"table_anchor\" value=\"{}\">", escape(anchor))).unwrap_or_default()),
            escape(&selection), ui_text_0 = crate::views::escape(&crate::i18n::t("Columns")), ui_text_1 = crate::views::escape(&crate::i18n::t("Columns")), ui_text_2 = crate::views::escape(&crate::i18n::t("Show/hide columns")), ui_text_3 = crate::views::escape(&crate::i18n::t("Visible columns")));
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
        "{base}{}{}",
        if base.contains('?') { "&" } else { "?" },
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
