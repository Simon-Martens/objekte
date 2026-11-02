//! Schema rules and matching traversal for source and index representations.

use crate::{Field, JsonPath, JsonPathStep, TextFormat};
use serde_json::Value;
use sonic_rs::{JsonValueTrait, Value as SonicValue, ValueRef};
use std::collections::BTreeMap;


/// Rendering and exclusion inherited from one declared schema field.
#[derive(Debug)]
struct TextRule {
    format: Option<TextFormat>,
    excluded: bool,
    field_id: Option<usize>,
    properties: BTreeMap<String, TextRule>,
    items: Option<Box<TextRule>>,
}


impl TextRule {
    fn from_field(field: &Field) -> Self {
        Self {
            format: field.text_format,
            excluded: field.exclude_fts,
            field_id: None,
            properties: field
                .properties
                .iter()
                .map(|(name, child)| (name.clone(), Self::from_field(child)))
                .collect(),
            items: field
                .items
                .as_ref()
                .map(|item| Box::new(Self::from_field(item))),
        }
    }

    fn has_markup(&self) -> bool {
        !self.excluded
            && (matches!(
                self.format,
                Some(TextFormat::Markdown | TextFormat::Html | TextFormat::Xml)
            ) || self.properties.values().any(Self::has_markup)
                || self.items.as_deref().is_some_and(Self::has_markup))
    }

    fn has_exclusions(&self) -> bool {
        self.excluded
            || self.properties.values().any(Self::has_exclusions)
            || self.items.as_deref().is_some_and(Self::has_exclusions)
    }
}


/// Lightweight schema rules; unknown paths default to plain searchable text.
#[derive(Debug)]
pub(crate) struct TextRules {
    fields: BTreeMap<String, TextRule>,
}


impl TextRules {
    pub(crate) fn new(fields: &BTreeMap<String, Field>) -> Self {
        Self {
            fields: fields
                .iter()
                .map(|(name, field)| (name.clone(), TextRule::from_field(field)))
                .collect(),
        }
    }

    pub(crate) fn has_markup(&self) -> bool {
        self.fields.values().any(TextRule::has_markup)
    }

    pub(crate) fn has_exclusions(&self) -> bool {
        self.fields.values().any(TextRule::has_exclusions)
    }

    /// Explicit full-text paths. Array members share one posting filter.
    pub(crate) fn registered_paths(&self) -> Vec<JsonPath> {
        fn collect(rule: &TextRule, path: &mut JsonPath, out: &mut Vec<JsonPath>) {
            if rule.excluded {
                return;
            }
            if rule.format.is_some() && rule.properties.is_empty() && rule.items.is_none() {
                out.push(path.clone());
            }
            for (name, child) in &rule.properties {
                path.push(JsonPathStep::Key(name.clone()));
                collect(child, path, out);
                path.pop();
            }
            if let Some(item) = &rule.items {
                path.push(JsonPathStep::Each);
                collect(item, path, out);
                path.pop();
            }
        }
        let mut paths = Vec::new();
        for (name, rule) in &self.fields {
            let mut path = vec![JsonPathStep::Key(name.clone())];
            collect(rule, &mut path, &mut paths);
        }
        paths
    }

    /// Bind the sorted, stable field-index positions to schema traversal nodes.
    /// Query-time rules remain uncompiled and continue producing concrete paths.
    pub(crate) fn compile_index_fields(&mut self, paths: &[JsonPath]) {
        for (field_id, path) in paths.iter().enumerate() {
            let Some(JsonPathStep::Key(name)) = path.first() else {
                continue;
            };
            let mut rule = self.fields.get_mut(name);
            for step in path.iter().skip(1) {
                rule = match (rule, step) {
                    (Some(parent), JsonPathStep::Key(name)) => parent.properties.get_mut(name),
                    (Some(parent), JsonPathStep::Each) => parent.items.as_deref_mut(),
                    _ => None,
                };
            }
            if let Some(rule) = rule {
                rule.field_id = Some(field_id);
            }
        }
    }

    pub(crate) fn scalar_format(&self, name: &str) -> Option<TextFormat> {
        let rule = self.fields.get(name);
        if rule.is_some_and(|rule| rule.excluded) {
            None
        } else {
            Some(
                rule.and_then(|rule| rule.format)
                    .unwrap_or(TextFormat::Plain),
            )
        }
    }
}


/// Visit searchable leaves in the public JSON representation.
pub(crate) fn visit_json_text(
    value: &Value,
    rules: &TextRules,
    include_indices: bool,
    visit: &mut impl FnMut(&str, &[(usize, usize)], &str, TextFormat),
) {
    fn walk(
        value: &Value,
        rule: Option<&TextRule>,
        path: &mut String,
        array_indices: &mut Vec<(usize, usize)>,
        include_indices: bool,
        visit: &mut impl FnMut(&str, &[(usize, usize)], &str, TextFormat),
    ) {
        if rule.is_some_and(|rule| rule.excluded) {
            return;
        }
        match value {
            Value::Object(fields) => {
                for (name, child) in fields {
                    let length = path.len();
                    if !path.is_empty() {
                        path.push('.');
                    }
                    path.push_str(name);
                    walk(
                        child,
                        rule.and_then(|rule| rule.properties.get(name)),
                        path,
                        array_indices,
                        include_indices,
                        visit,
                    );
                    path.truncate(length);
                }
            }
            Value::Array(items) => {
                let length = path.len();
                path.push_str("[]");
                for (index, child) in items.iter().enumerate() {
                    if include_indices {
                        array_indices.push((length, index));
                    }
                    walk(
                        child,
                        rule.and_then(|rule| rule.items.as_deref()),
                        path,
                        array_indices,
                        include_indices,
                        visit,
                    );
                    if include_indices {
                        array_indices.pop();
                    }
                }
                path.truncate(length);
            }
            Value::String(text) => visit(
                path,
                array_indices,
                text,
                rule.and_then(|rule| rule.format)
                    .unwrap_or(TextFormat::Plain),
            ),
            Value::Number(number) => {
                visit(path, array_indices, &number.to_string(), TextFormat::Plain)
            }
            Value::Bool(boolean) => visit(
                path,
                array_indices,
                if *boolean { "true" } else { "false" },
                TextFormat::Plain,
            ),
            Value::Null => visit(path, array_indices, "null", TextFormat::Plain),
        }
    }

    let mut path = String::new();
    let mut array_indices = Vec::new();
    if let Value::Object(fields) = value {
        for (name, child) in fields {
            path.push_str(name);
            walk(
                child,
                rules.fields.get(name),
                &mut path,
                &mut array_indices,
                include_indices,
                visit,
            );
            path.clear();
        }
    } else {
        walk(
            value,
            None,
            &mut path,
            &mut array_indices,
            include_indices,
            visit,
        );
    }
}

/// Visit the same leaves in Sonic's index-build representation.
#[allow(
    dead_code,
    reason = "retained for concrete Sonic path consumers and parity tests"
)]
pub(crate) fn visit_sonic_text(
    value: &SonicValue,
    rules: &TextRules,
    visit: &mut impl FnMut(&str, &JsonPath, Option<&JsonPath>, &str, TextFormat, bool),
) {
    fn walk(
        value: &SonicValue,
        rule: Option<&TextRule>,
        path: &mut String,
        steps: &mut JsonPath,
        fallback: Option<&JsonPath>,
        top_level: bool,
        visit: &mut impl FnMut(&str, &JsonPath, Option<&JsonPath>, &str, TextFormat, bool),
    ) {
        if rule.is_some_and(|rule| rule.excluded) {
            return;
        }
        // Only malformed containers at declared scalar fields need to keep
        // their descendants in that field's candidate index.
        let invalid_origin = if rule.is_some_and(|rule| rule.format.is_some())
            && matches!(value.as_ref(), ValueRef::Array(_) | ValueRef::Object(_))
        {
            Some(steps.clone())
        } else {
            None
        };
        let fallback = invalid_origin.as_ref().or(fallback);
        if let Some(number) = value.as_raw_number() {
            // Source verification uses serde_json::Number, whose spelling can
            // differ from the source lexeme (for example exponent notation).
            if let Ok(number) = number.as_str().parse::<serde_json::Number>() {
                visit(
                    path,
                    steps,
                    fallback,
                    &number.to_string(),
                    TextFormat::Plain,
                    top_level,
                );
            }
            return;
        }
        match value.as_ref() {
            ValueRef::Object(fields) => {
                // The public JSON representation keeps the last duplicate key.
                let mut last = BTreeMap::new();
                for (name, child) in fields {
                    last.insert(name, child);
                }
                for (name, child) in last {
                    let length = path.len();
                    if !path.is_empty() {
                        path.push('.');
                    }
                    path.push_str(name);
                    steps.push(JsonPathStep::Key(name.to_owned()));
                    walk(
                        child,
                        rule.and_then(|rule| rule.properties.get(name)),
                        path,
                        steps,
                        fallback,
                        false,
                        visit,
                    );
                    steps.pop();
                    path.truncate(length);
                }
            }
            ValueRef::Array(items) => {
                let length = path.len();
                path.push_str("[]");
                steps.push(JsonPathStep::Each);
                for child in items {
                    walk(
                        child,
                        rule.and_then(|rule| rule.items.as_deref()),
                        path,
                        steps,
                        fallback,
                        false,
                        visit,
                    );
                }
                steps.pop();
                path.truncate(length);
            }
            ValueRef::String(text) => visit(
                path,
                steps,
                fallback,
                text,
                rule.and_then(|rule| rule.format)
                    .unwrap_or(TextFormat::Plain),
                top_level,
            ),
            ValueRef::Bool(boolean) => visit(
                path,
                steps,
                fallback,
                if boolean { "true" } else { "false" },
                TextFormat::Plain,
                top_level,
            ),
            ValueRef::Null => visit(path, steps, fallback, "null", TextFormat::Plain, top_level),
            ValueRef::Number(_) => unreachable!("raw numbers enabled"),
        }
    }

    let mut path = String::new();
    let mut steps = Vec::new();
    if let ValueRef::Object(fields) = value.as_ref() {
        let mut last = BTreeMap::new();
        for (name, child) in fields {
            last.insert(name, child);
        }
        for (name, child) in last {
            path.push_str(name);
            steps.push(JsonPathStep::Key(name.to_owned()));
            walk(
                child,
                rules.fields.get(name),
                &mut path,
                &mut steps,
                None,
                true,
                visit,
            );
            steps.pop();
            path.clear();
        }
    } else {
        walk(value, None, &mut path, &mut steps, None, false, visit);
    }
}

/// Index-only Sonic traversal. Field routing is compiled into the schema tree,
/// so each leaf avoids allocating a JSON path and searching registered paths.
pub(crate) fn visit_sonic_index_text(
    value: &SonicValue,
    rules: &TextRules,
    visit: &mut impl FnMut(Option<usize>, Option<&str>, &str, TextFormat),
) {
    fn walk(
        value: &SonicValue,
        rule: Option<&TextRule>,
        fallback: Option<usize>,
        top_level_name: Option<&str>,
        visit: &mut impl FnMut(Option<usize>, Option<&str>, &str, TextFormat),
    ) {
        if rule.is_some_and(|rule| rule.excluded) {
            return;
        }
        let container = matches!(value.as_ref(), ValueRef::Array(_) | ValueRef::Object(_));
        // A malformed container at a declared scalar captures its descendants.
        // An unregistered declaration also clears an older fallback, matching
        // the path-based visitor's nearest-invalid-container behavior.
        let fallback = if container && rule.is_some_and(|rule| rule.format.is_some()) {
            rule.and_then(|rule| rule.field_id)
        } else {
            fallback
        };
        let field_id = rule.and_then(|rule| rule.field_id).or(fallback);
        if let Some(number) = value.as_raw_number() {
            if let Ok(number) = number.as_str().parse::<serde_json::Number>() {
                visit(
                    field_id,
                    top_level_name,
                    &number.to_string(),
                    TextFormat::Plain,
                );
            }
            return;
        }
        match value.as_ref() {
            ValueRef::Object(fields) => {
                // Sonic preserves duplicate members; the public JSON value keeps the last.
                let mut last = BTreeMap::new();
                for (name, child) in fields {
                    last.insert(name, child);
                }
                for (name, child) in last {
                    walk(
                        child,
                        rule.and_then(|rule| rule.properties.get(name)),
                        fallback,
                        None,
                        visit,
                    );
                }
            }
            ValueRef::Array(items) => {
                for child in items {
                    walk(
                        child,
                        rule.and_then(|rule| rule.items.as_deref()),
                        fallback,
                        None,
                        visit,
                    );
                }
            }
            ValueRef::String(text) => visit(
                field_id,
                top_level_name,
                text,
                rule.and_then(|rule| rule.format)
                    .unwrap_or(TextFormat::Plain),
            ),
            ValueRef::Bool(boolean) => visit(
                field_id,
                top_level_name,
                if boolean { "true" } else { "false" },
                TextFormat::Plain,
            ),
            ValueRef::Null => visit(field_id, top_level_name, "null", TextFormat::Plain),
            ValueRef::Number(_) => unreachable!("raw numbers enabled"),
        }
    }

    if let ValueRef::Object(fields) = value.as_ref() {
        let mut last = BTreeMap::new();
        for (name, child) in fields {
            last.insert(name, child);
        }
        for (name, child) in last {
            walk(child, rules.fields.get(name), None, Some(name), visit);
        }
    } else {
        walk(value, None, None, None, visit);
    }
}
