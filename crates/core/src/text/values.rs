//! Schema rules and matching traversal for source and index representations.

use crate::{Field, TextFormat};
use serde_json::Value;
use sonic_rs::{JsonValueTrait, Value as SonicValue, ValueRef};
use std::collections::BTreeMap;


/// Rendering and exclusion inherited from one declared schema field.
#[derive(Debug)]
struct TextRule {
    format: Option<TextFormat>,
    excluded: bool,
    properties: BTreeMap<String, TextRule>,
    items: Option<Box<TextRule>>,
}


impl TextRule {
    fn from_field(field: &Field) -> Self {
        Self {
            format: field.text_format,
            excluded: field.exclude_fts,
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
pub(crate) fn visit_sonic_text(
    value: &SonicValue,
    rules: &TextRules,
    visit: &mut impl FnMut(&str, &str, TextFormat),
) {
    fn walk(
        value: &SonicValue,
        rule: Option<&TextRule>,
        path: &mut String,
        visit: &mut impl FnMut(&str, &str, TextFormat),
    ) {
        if rule.is_some_and(|rule| rule.excluded) {
            return;
        }
        if let Some(number) = value.as_raw_number() {
            // Source verification uses serde_json::Number, whose spelling can
            // differ from the source lexeme (for example exponent notation).
            if let Ok(number) = number.as_str().parse::<serde_json::Number>() {
                visit(path, &number.to_string(), TextFormat::Plain);
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
                    walk(
                        child,
                        rule.and_then(|rule| rule.properties.get(name)),
                        path,
                        visit,
                    );
                    path.truncate(length);
                }
            }
            ValueRef::Array(items) => {
                let length = path.len();
                path.push_str("[]");
                for child in items {
                    walk(
                        child,
                        rule.and_then(|rule| rule.items.as_deref()),
                        path,
                        visit,
                    );
                }
                path.truncate(length);
            }
            ValueRef::String(text) => visit(
                path,
                text,
                rule.and_then(|rule| rule.format)
                    .unwrap_or(TextFormat::Plain),
            ),
            ValueRef::Bool(boolean) => visit(
                path,
                if boolean { "true" } else { "false" },
                TextFormat::Plain,
            ),
            ValueRef::Null => visit(path, "null", TextFormat::Plain),
            ValueRef::Number(_) => unreachable!("raw numbers enabled"),
        }
    }

    let mut path = String::new();
    if let ValueRef::Object(fields) = value.as_ref() {
        let mut last = BTreeMap::new();
        for (name, child) in fields {
            last.insert(name, child);
        }
        for (name, child) in last {
            path.push_str(name);
            walk(child, rules.fields.get(name), &mut path, visit);
            path.clear();
        }
    } else {
        walk(value, None, &mut path, visit);
    }
}
