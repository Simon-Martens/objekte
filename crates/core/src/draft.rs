//! Transport-independent editing drafts. Invalid input and absence survive round trips.
use crate::ValidationIssue;
use serde_json::{Map, Value};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq)]
pub enum InputKind {
    Text,
    Integer,
    Number,
    Boolean,
    Json,
    Enum,
    Array,
}
#[derive(Clone, Debug, PartialEq)]
pub struct DraftField {
    pub id: String,
    pub key: String,
    pub schema: Value,
    pub kind: InputKind,
    pub present: bool,
    pub required: bool,
    pub hidden: bool,
    pub read_only: bool,
    pub raw: String,
    pub items: Vec<DraftField>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    pub fields: Vec<DraftField>,
}
#[derive(Clone, Debug)]
pub struct FieldInput {
    pub present: bool,
    pub raw: String,
}

pub(crate) fn resolve(root: &Value, schema: &Value, depth: usize) -> Value {
    if depth > 32 {
        return schema.clone();
    }
    if let Some(reference) = schema
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|s| s.strip_prefix('#'))
    {
        if let Some(target) = root.pointer(reference) {
            let mut value = resolve(root, target, depth + 1);
            if let (Some(value), Some(schema)) = (value.as_object_mut(), schema.as_object()) {
                value.extend(
                    schema
                        .iter()
                        .filter(|(k, _)| k.as_str() != "$ref")
                        .map(|(k, v)| (k.clone(), v.clone())),
                );
            }
            return value;
        }
    }
    schema.clone()
}
impl DraftField {
    fn new(
        root: &Value,
        key: String,
        schema: &Value,
        value: Option<&Value>,
        required: bool,
        depth: usize,
    ) -> Self {
        let schema = resolve(root, schema, 0);
        let kind = if schema.get("enum").is_some() {
            InputKind::Enum
        } else if schema["x-editor"] == "json" {
            InputKind::Json
        } else if depth > 16
            || schema.get("oneOf").is_some()
            || schema.get("anyOf").is_some()
            || schema.get("prefixItems").is_some()
            || schema.get("type").is_some_and(Value::is_array)
        {
            InputKind::Json
        } else if schema.get("enum").is_some() {
            InputKind::Enum
        } else {
            match schema.get("type").and_then(Value::as_str) {
                Some("string") => InputKind::Text,
                Some("integer") => InputKind::Integer,
                Some("number") => InputKind::Number,
                Some("boolean") => InputKind::Boolean,
                Some("array") => InputKind::Array,
                _ => InputKind::Json,
            }
        };
        let raw = match (&kind, value) {
            (InputKind::Text, Some(Value::String(text))) => text.clone(),
            (_, Some(v)) => serde_json::to_string_pretty(v).unwrap_or_default(),
            _ => String::new(),
        };
        let hidden = schema.get("x-editor").and_then(Value::as_str) == Some("hidden")
            || schema["x-visible"] == false
            || schema["x-children"] == true;
        let read_only = crate::policy::read_only(&schema);
        let items = if kind == InputKind::Array {
            value
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .enumerate()
                        .map(|(i, value)| {
                            Self::new(
                                root,
                                i.to_string(),
                                &item_schema(&schema, i),
                                Some(value),
                                true,
                                depth + 1,
                            )
                        })
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        Self {
            id: Uuid::new_v4().simple().to_string(),
            key,
            schema,
            kind,
            present: value.is_some(),
            required,
            hidden,
            read_only,
            raw,
            items,
        }
    }
    fn apply(&mut self, inputs: &HashMap<String, FieldInput>) {
        if self.hidden || self.read_only {
            return;
        }
        if let Some(input) = inputs.get(&self.id) {
            self.present = input.present;
            self.raw.clone_from(&input.raw);
        }
        for item in &mut self.items {
            item.apply(inputs);
        }
    }
    fn value(&self, path: &str, issues: &mut Vec<ValidationIssue>) -> Option<Value> {
        if !self.present {
            return None;
        }
        let parsed = match self.kind {
            InputKind::Text => Some(Value::String(self.raw.clone())),
            InputKind::Array => {
                let values = self
                    .items
                    .iter()
                    .enumerate()
                    .map(|(i, item)| {
                        if !item.present {
                            issues.push(ValidationIssue {
                                path: format!("{path}/{i}"),
                                message: "Array items cannot be absent; remove the item instead"
                                    .into(),
                            });
                        }
                        item.value(&format!("{path}/{i}"), issues)
                            .unwrap_or(Value::Null)
                    })
                    .collect();
                Some(Value::Array(values))
            }
            _ => serde_json::from_str::<Value>(&self.raw)
                .ok()
                .filter(|v| match self.kind {
                    InputKind::Integer => v.is_i64() || v.is_u64(),
                    InputKind::Number => v.is_number(),
                    InputKind::Boolean => v.is_boolean(),
                    _ => true,
                }),
        };
        if parsed.is_none() {
            issues.push(ValidationIssue {
                path: path.into(),
                message: format!(
                    "Enter a valid {} value",
                    match self.kind {
                        InputKind::Integer => "integer",
                        InputKind::Number => "number",
                        InputKind::Boolean => "boolean",
                        _ => "JSON",
                    }
                ),
            });
        }
        parsed
    }
    fn find_mut(&mut self, id: &str) -> Option<&mut Self> {
        if self.hidden || self.read_only {
            return None;
        }
        if self.id == id {
            return Some(self);
        }
        self.items.iter_mut().find_map(|item| item.find_mut(id))
    }
}
fn item_schema(schema: &Value, index: usize) -> Value {
    schema
        .get("prefixItems")
        .and_then(|s| s.get(index))
        .or_else(|| schema.get("items"))
        .cloned()
        .unwrap_or(serde_json::json!({}))
}
impl Draft {
    pub fn new(schema: &Value, value: &Value) -> Self {
        let properties = schema
            .get("properties")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let object = value.as_object().cloned().unwrap_or_default();
        let mut keys: Vec<String> = properties.keys().chain(object.keys()).cloned().collect();
        keys.sort();
        keys.dedup();
        if let Some(order) = schema
            .pointer("/x-collection/fieldOrder")
            .and_then(Value::as_array)
        {
            keys.sort_by_key(|key| {
                order
                    .iter()
                    .position(|v| v.as_str() == Some(key))
                    .unwrap_or(usize::MAX)
            });
        }
        Self {
            fields: keys
                .into_iter()
                .map(|key| {
                    let required = schema
                        .get("required")
                        .and_then(Value::as_array)
                        .is_some_and(|r| r.iter().any(|v| v.as_str() == Some(&key)));
                    DraftField::new(
                        schema,
                        key.clone(),
                        properties.get(&key).unwrap_or(&serde_json::json!({})),
                        object.get(&key),
                        required,
                        0,
                    )
                })
                .collect(),
        }
    }
    pub fn apply(&mut self, inputs: &HashMap<String, FieldInput>) {
        for field in &mut self.fields {
            field.apply(inputs);
        }
    }
    pub fn field(&self, id: &str) -> Option<&DraftField> {
        fn find<'a>(fields: &'a [DraftField], id: &str) -> Option<&'a DraftField> {
            fields.iter().find_map(|f| {
                if f.id == id {
                    Some(f)
                } else {
                    find(&f.items, id)
                }
            })
        }
        find(&self.fields, id)
    }
    pub fn set_presence(&mut self, id: &str, present: bool) -> Result<(), String> {
        let f = self
            .fields
            .iter_mut()
            .find_map(|f| f.find_mut(id))
            .ok_or("Field is missing or read-only")?;
        f.present = present;
        if present && f.raw.is_empty() {
            if let Some(value) = f.schema.get("default") {
                f.raw = if f.kind == InputKind::Text {
                    value.as_str().unwrap_or("").into()
                } else {
                    value.to_string()
                };
            } else if f.kind == InputKind::Boolean {
                f.raw = "false".into();
            }
        }
        Ok(())
    }
    pub fn set_field(&mut self, id: &str, value: Value) -> Result<(), String> {
        let f = self
            .fields
            .iter_mut()
            .find_map(|f| f.find_mut(id))
            .ok_or("Field is missing or read-only")?;
        let mut replacement = DraftField::new(
            &f.schema,
            f.key.clone(),
            &f.schema,
            Some(&value),
            f.required,
            0,
        );
        replacement.id = f.id.clone();
        *f = replacement;
        Ok(())
    }
    pub fn value(&self) -> Result<Value, Vec<ValidationIssue>> {
        let mut issues = Vec::new();
        let mut values = Map::new();
        for field in &self.fields {
            let path = format!("/{}", field.key.replace('~', "~0").replace('/', "~1"));
            if let Some(value) = field.value(&path, &mut issues) {
                values.insert(field.key.clone(), value);
            }
        }
        if issues.is_empty() {
            Ok(Value::Object(values))
        } else {
            Err(issues)
        }
    }
    /// Structural edits are applied only here; item IDs remain stable across reorder.
    pub fn array_action(
        &mut self,
        root_schema: &Value,
        field_id: &str,
        action: &str,
        item_id: Option<&str>,
    ) -> Result<(), String> {
        let field = self
            .fields
            .iter_mut()
            .find_map(|f| f.find_mut(field_id))
            .ok_or("Unknown array")?;
        if field.hidden || field.read_only || field.kind != InputKind::Array {
            return Err("Not an editable array".into());
        }
        if action == "add" {
            if field.schema.get("items") == Some(&Value::Bool(false)) {
                return Err("Additional items are not allowed".into());
            }
            let schema = resolve(
                root_schema,
                &item_schema(&field.schema, field.items.len()),
                0,
            );
            // Explicit defaults are permitted. Without one, the new draft remains incomplete.
            let mut item = DraftField::new(
                root_schema,
                field.items.len().to_string(),
                &schema,
                schema.get("default").or_else(|| schema.get("const")),
                true,
                1,
            );
            item.present = schema.get("default").is_some() || schema.get("const").is_some();
            field.items.push(item);
            field.present = true;
            return Ok(());
        }
        let index = field
            .items
            .iter()
            .position(|item| Some(item.id.as_str()) == item_id)
            .ok_or("Unknown array item")?;
        match action {
            "remove" => {
                field.items.remove(index);
            }
            "up" if index > 0 => field.items.swap(index, index - 1),
            "down" if index + 1 < field.items.len() => field.items.swap(index, index + 1),
            _ => return Err("Invalid array action".into()),
        }
        for (index, item) in field.items.iter_mut().enumerate() {
            item.key = index.to_string();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn absence_invalid_input_and_hidden_values_survive() {
        let schema = json!({"properties":{"n":{"type":"integer"},"flag":{"type":"boolean"},"source":{"type":"string","x-editor":"hidden"}}});
        let mut draft = Draft::new(&schema, &json!({"source":"keep"}));
        assert_eq!(draft.value().unwrap(), json!({"source":"keep"}));
        let inputs = draft
            .fields
            .iter()
            .map(|f| {
                (
                    f.id.clone(),
                    FieldInput {
                        present: true,
                        raw: if f.key == "flag" { "false" } else { "invalid" }.into(),
                    },
                )
            })
            .collect();
        draft.apply(&inputs);
        assert_eq!(draft.value().unwrap_err()[0].path, "/n");
        let n = draft.fields.iter_mut().find(|f| f.key == "n").unwrap();
        n.raw = "0".into();
        assert_eq!(
            draft.value().unwrap(),
            json!({"source":"keep","n":0,"flag":false})
        );
    }
    #[test]
    fn array_identity_survives_reordering() {
        let schema = json!({"properties":{"items":{"type":"array","items":{"type":"string"}}}});
        let mut draft = Draft::new(&schema, &json!({"items":["a","b"]}));
        let field = draft.fields[0].id.clone();
        let item = draft.fields[0].items[1].id.clone();
        draft
            .array_action(&schema, &field, "up", Some(&item))
            .unwrap();
        assert_eq!(draft.fields[0].items[0].id, item);
        assert_eq!(draft.value().unwrap(), json!({"items":["b","a"]}));
        draft.fields[0].items[0].present = false;
        assert_eq!(draft.value().unwrap_err()[0].path, "/items/0");
    }

    #[test]
    fn hidden_arrays_cannot_be_changed_through_nested_actions() {
        let schema = json!({"properties":{"source":{"type":"array","x-editor":"hidden","items":{"type":"array","items":{"type":"string"}}}}});
        let value = json!({"source":[["keep"]]});
        let mut draft = Draft::new(&schema, &value);
        let child = draft.fields[0].items[0].id.clone();
        assert!(draft.array_action(&schema, &child, "add", None).is_err());
        assert_eq!(draft.value().unwrap(), value);
    }
}

/// A bounded, transport-independent undo history, including invalid raw input.
#[derive(Clone, Debug, PartialEq)]
pub struct DraftState {
    pub draft: Draft,
    pub id: String,
    pub raw: Option<String>,
}
#[derive(Clone, Debug, Default)]
pub struct DraftHistory {
    undo: Vec<DraftState>,
    redo: Vec<DraftState>,
}
impl DraftHistory {
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn record(&mut self, before: DraftState, after: &DraftState) {
        if &before != after {
            self.undo.push(before);
            if self.undo.len() > 100 {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
    }
    pub fn undo(&mut self, current: DraftState) -> Option<DraftState> {
        let previous = self.undo.pop()?;
        self.redo.push(current);
        Some(previous)
    }
    pub fn redo(&mut self, current: DraftState) -> Option<DraftState> {
        let next = self.redo.pop()?;
        self.undo.push(current);
        Some(next)
    }
}
