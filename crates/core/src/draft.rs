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
    Object,
    Null,
    Unsupported,
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
    pub root_value: bool,
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
        inherited: &Value,
    ) -> Self {
        let mut schema = resolve(root, schema, 0);
        if schema.is_object() {
            schema["x-presentation"] = crate::ui::inherit(inherited, &schema["x-presentation"]);
        }
        let explicit_raw = key == "_editor"
            || schema["x-presentation"]["editor"] == "raw"
            || schema["x-editor"] == "json";
        let fallback = if schema["x-presentation"]["rawFallback"] == false {
            InputKind::Unsupported
        } else {
            InputKind::Json
        };
        let data_type = schema["type"].as_str().or_else(|| {
            if schema.get("properties").is_some() {
                Some("object")
            } else {
                value.map(|v| match v {
                    Value::Object(_) => "object",
                    Value::Array(_) => "array",
                    Value::String(_) => "string",
                    Value::Bool(_) => "boolean",
                    Value::Number(_) => "number",
                    Value::Null => "null",
                })
            }
        });
        fn shape(v: &Value) -> u8 {
            match v {
                Value::Null => 0,
                Value::Bool(_) => 1,
                Value::Number(_) => 2,
                Value::String(_) => 3,
                Value::Array(_) => 4,
                Value::Object(_) => 5,
            }
        }
        let wrong_container = value.is_some_and(|v| match data_type {
            Some("object") => !v.is_object(),
            Some("array") => !v.is_array(),
            _ => false,
        });
        let mixed = value.and_then(Value::as_array).is_some_and(|items| {
            items
                .first()
                .is_some_and(|first| items.iter().any(|v| shape(v) != shape(first)))
        });
        let complex_array = data_type == Some("array")
            && (value
                .and_then(Value::as_array)
                .is_some_and(|items| items.iter().any(|v| v.is_object() || v.is_array()))
                || matches!(
                    resolve(root, &item_schema(&schema, 0), 0)["type"].as_str(),
                    Some("object" | "array")
                ));
        let kind = if explicit_raw {
            InputKind::Json
        } else if schema.get("enum").is_some() {
            InputKind::Enum
        } else if depth > 64
            || mixed
            || wrong_container
            || schema.get("oneOf").is_some()
            || schema.get("anyOf").is_some()
            || schema.get("prefixItems").is_some()
            || schema.get("type").is_some_and(Value::is_array)
            || (inherited["navigation"] == "stop"
                && schema["x-presentation"]["navigation"] == "stop"
                && (data_type == Some("object") || complex_array))
        {
            fallback
        } else {
            match data_type {
                Some("object") => InputKind::Object,
                Some("array") => InputKind::Array,
                Some("string") => InputKind::Text,
                Some("integer") => InputKind::Integer,
                Some("number") => InputKind::Number,
                Some("boolean") => InputKind::Boolean,
                Some("null") => InputKind::Null,
                _ => fallback,
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
        let read_only = crate::policy::read_only(&schema) || kind == InputKind::Unsupported;
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
                                &schema["x-presentation"],
                            )
                        })
                        .collect()
                })
                .unwrap_or_default()
        } else if kind == InputKind::Object && value.is_some_and(Value::is_object) {
            object_fields(root, &schema, value.unwrap(), depth + 1)
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
            InputKind::Null => Some(Value::Null),
            InputKind::Object => {
                let mut object = Map::new();
                for field in &self.items {
                    if let Some(value) = field.value(&child_path(path, &field.key), issues) {
                        object.insert(field.key.clone(), value);
                    }
                }
                Some(Value::Object(object))
            }
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
pub fn child_path(parent: &str, key: &str) -> String {
    format!("{parent}/{}", key.replace('~', "~0").replace('/', "~1"))
}
fn object_fields(root: &Value, schema: &Value, value: &Value, depth: usize) -> Vec<DraftField> {
    let props = schema["properties"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let object = value.as_object().cloned().unwrap_or_default();
    let mut keys: Vec<_> = props.keys().chain(object.keys()).cloned().collect();
    keys.sort();
    keys.dedup();
    let order = schema
        .pointer("/x-presentation/fieldOrder")
        .or_else(|| schema.pointer("/x-collection/fieldOrder"))
        .and_then(Value::as_array);
    if let Some(order) = order {
        keys.sort_by_key(|key| {
            order
                .iter()
                .position(|v| v.as_str() == Some(key))
                .unwrap_or(usize::MAX)
        });
    }
    keys.into_iter()
        .map(|key| {
            let required = schema["required"]
                .as_array()
                .is_some_and(|r| r.iter().any(|v| v.as_str() == Some(&key)));
            let definition = props
                .get(&key)
                .or_else(|| schema.get("additionalProperties"))
                .cloned()
                .unwrap_or(serde_json::json!({}));
            DraftField::new(
                root,
                key.clone(),
                &definition,
                object.get(&key),
                required,
                depth,
                &schema["x-presentation"],
            )
        })
        .collect()
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
        Self {
            fields: object_fields(schema, schema, value, 0),
            root_value: false,
        }
    }
    /// Stable JSON paths address the same value in preview and draft views.
    pub fn at_path(&self, path: &str) -> Option<&DraftField> {
        self.paths()
            .into_iter()
            .find(|(p, _)| p == path)
            .map(|(_, f)| f)
    }
    pub fn paths(&self) -> Vec<(String, &DraftField)> {
        fn visit<'a>(field: &'a DraftField, path: String, out: &mut Vec<(String, &'a DraftField)>) {
            out.push((path.clone(), field));
            for child in &field.items {
                visit(child, child_path(&path, &child.key), out);
            }
        }
        let mut result = Vec::new();
        for field in &self.fields {
            visit(
                field,
                if self.root_value {
                    String::new()
                } else {
                    child_path("", &field.key)
                },
                &mut result,
            );
        }
        result
    }
    /// Explicitly switch a document to form mode without changing descendant overrides.
    pub fn document_form(schema: &Value, value: &Value) -> Self {
        let mut schema = schema.clone();
        if schema.is_object() && schema["x-presentation"]["editor"] == "raw" {
            schema["x-presentation"]["editor"] = Value::String("form".into());
        }
        Self::document(&schema, value)
    }

    /// Assigned documents may also have an array or scalar at their root.
    pub fn document(schema: &Value, value: &Value) -> Self {
        let resolved = resolve(schema, schema, 0);
        if value.is_object()
            && resolved["x-presentation"]["editor"] != "raw"
            && resolved["x-editor"] != "json"
            && resolved.get("oneOf").is_none()
            && resolved.get("anyOf").is_none()
        {
            return Self::new(&resolved, value);
        }
        Self {
            root_value: true,
            fields: vec![DraftField::new(
                schema,
                String::new(),
                &resolved,
                Some(value),
                true,
                0,
                &Value::Null,
            )],
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
    /// Adding an absent Markdown list exposes its first editor immediately.
    pub fn include_field(&mut self, root_schema: &Value, id: &str) -> Result<(), String> {
        let was_absent = self.field(id).is_some_and(|field| !field.present);
        self.set_presence(id, true)?;
        let field = self
            .fields
            .iter_mut()
            .find_map(|field| field.find_mut(id))
            .ok_or("Field is missing or read-only")?;
        if was_absent && field.kind == InputKind::Object && field.items.is_empty() {
            let value = field
                .schema
                .get("default")
                .filter(|v| v.is_object())
                .cloned()
                .unwrap_or(serde_json::json!({}));
            field.items = object_fields(root_schema, &field.schema, &value, 0);
        }
        if was_absent && field.kind == InputKind::Array && field.items.is_empty() {
            let schema = resolve(root_schema, &item_schema(&field.schema, 0), 0);
            let empty = Value::String(String::new());
            let item = DraftField::new(
                root_schema,
                "0".into(),
                &schema,
                Some(
                    schema
                        .get("default")
                        .or_else(|| schema.get("const"))
                        .unwrap_or(&empty),
                ),
                true,
                1,
                &field.schema["x-presentation"],
            );
            if item.kind == InputKind::Text
                && !item.hidden
                && !item.read_only
                && (item.schema["x-content"] == "markdown"
                    || matches!(
                        item.schema["x-editor"].as_str(),
                        Some("markdown" | "inline-markdown")
                    ))
            {
                field.items.push(item);
            }
        }
        Ok(())
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
            &Value::Null,
        );
        replacement.id = f.id.clone();
        *f = replacement;
        Ok(())
    }
    pub fn value(&self) -> Result<Value, Vec<ValidationIssue>> {
        let mut issues = Vec::new();
        if self.root_value {
            let value = self.fields[0].value("", &mut issues);
            if value.is_none() && issues.is_empty() {
                issues.push(ValidationIssue {
                    path: String::new(),
                    message: "Document root cannot be absent".into(),
                });
            }
            return if issues.is_empty() {
                Ok(value.unwrap())
            } else {
                Err(issues)
            };
        }
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
        let is_property = self.paths().iter().any(|(path, field)| {
            field.id == field_id
                && !path.is_empty()
                && (path.rsplit_once('/').is_some_and(|(parent, _)| {
                    if parent.is_empty() && !self.root_value {
                        true
                    } else {
                        self.at_path(parent)
                            .is_some_and(|f| f.kind == InputKind::Object)
                    }
                }))
        });
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
            let empty_object = serde_json::json!({});
            let initial = schema
                .get("default")
                .or_else(|| schema.get("const"))
                .or_else(|| {
                    (schema["type"] == "object" || schema.get("properties").is_some())
                        .then_some(&empty_object)
                });
            let mut item = DraftField::new(
                root_schema,
                field.items.len().to_string(),
                &schema,
                initial,
                true,
                1,
                &field.schema["x-presentation"],
            );
            item.present = initial.is_some();
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
                let removed = field.items.remove(index);
                if is_property
                    && field.items.is_empty()
                    && removed.kind == InputKind::Text
                    && (removed.schema["x-content"] == "markdown"
                        || matches!(
                            removed.schema["x-editor"].as_str(),
                            Some("markdown" | "inline-markdown")
                        ))
                {
                    field.present = false;
                }
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

#[cfg(test)]
mod presentation_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn presentation_inherits_into_items_and_resets_without_changing_values() {
        let schema = json!({"type":"object", "x-presentation":{"mode":"multiline"}, "properties":{
            "plain":{"type":"string","x-presentation":{"inherit":false}},
            "source":{"type":"array","items":{"type":"string","x-presentation":{"editor":"raw"}}},
            "normal":{"type":"array","items":{"type":"string"}}
        }});
        let original = json!({"plain":"hello","source":["<b>hi</b>"],"normal":["a"]});
        let draft = Draft::new(&schema, &original);
        let field = |key| draft.fields.iter().find(|f| f.key == key).unwrap();
        assert!(field("plain").schema["x-presentation"]["mode"].is_null());
        assert_eq!(
            field("normal").items[0].schema["x-presentation"]["mode"],
            "multiline"
        );
        assert_eq!(field("source").items[0].kind, InputKind::Json);
        assert_eq!(draft.value().unwrap(), original);
    }
    #[test]
    fn document_roots_keep_arrays_scalars_and_null_and_report_root_paths() {
        for (schema, value) in [
            (
                json!({"type":"array","items":{"type":"integer"}}),
                json!([1, 2]),
            ),
            (json!({"type":"string"}), json!("text")),
            (json!({"type":"null"}), Value::Null),
        ] {
            let mut draft = Draft::document(&schema, &value);
            assert!(draft.root_value);
            assert_eq!(draft.value().unwrap(), value);
            if draft.fields[0].kind == InputKind::Array {
                draft.fields[0].items[0].raw = "invalid".into();
                assert_eq!(draft.value().unwrap_err()[0].path, "/0");
            }
        }
    }
}

#[cfg(test)]
mod include_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn adding_markdown_list_starts_one_string_and_preserves_existing_items() {
        let schema = json!({"type":"object","$defs":{"md":{"type":"string","x-editor":"markdown"}},"properties":{
            "notes":{"type":"array","items":{"$ref":"#/$defs/md"}},
            "plain":{"type":"array","items":{"type":"string"}}
        }});
        let mut draft = Draft::new(&schema, &json!({}));
        let id = draft
            .fields
            .iter()
            .find(|f| f.key == "notes")
            .unwrap()
            .id
            .clone();
        draft.include_field(&schema, &id).unwrap();
        assert_eq!(draft.value().unwrap(), json!({"notes":[""]}));
        draft.include_field(&schema, &id).unwrap();
        assert_eq!(draft.field(&id).unwrap().items.len(), 1);
        let item = draft.field(&id).unwrap().items[0].id.clone();
        draft.set_field(&item, json!("Existing text")).unwrap();
        draft.set_presence(&id, false).unwrap();
        draft.include_field(&schema, &id).unwrap();
        assert_eq!(draft.value().unwrap(), json!({"notes":["Existing text"]}));
        let plain = draft
            .fields
            .iter()
            .find(|f| f.key == "plain")
            .unwrap()
            .id
            .clone();
        draft.include_field(&schema, &plain).unwrap();
        assert_eq!(draft.value().unwrap()["plain"], json!([]));
        let before = draft.clone();
        let item = draft.field(&id).unwrap().items[0].id.clone();
        draft
            .array_action(&schema, &id, "remove", Some(&item))
            .unwrap();
        assert_eq!(draft.value().unwrap(), json!({"plain":[]}));
        assert!(!draft.field(&id).unwrap().present);
        draft.include_field(&schema, &id).unwrap();
        assert_eq!(draft.value().unwrap()["notes"], json!([""]));
        // Restoring a history snapshot retains both the field and its last item.
        assert_eq!(before.value().unwrap()["notes"], json!(["Existing text"]));
    }
}

#[cfg(test)]
mod recursive_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn recursive_refs_paths_and_partial_edits_preserve_other_branches() {
        let schema = json!({"type":"object","$defs":{"node":{"type":"object","properties":{"name":{"type":"string"},"children":{"type":"array","items":{"$ref":"#/$defs/node"}}}}},"properties":{"a/b~c":{"$ref":"#/$defs/node"},"other":{"type":"boolean"}}});
        let mut node = json!({"name":"leaf","children":[]});
        let mut path = "/a~1b~0c".to_string();
        for _ in 0..12 {
            node = json!({"name":"branch","children":[node]});
            path += "/children/0";
        }
        let value = json!({"a/b~c":node,"other":false});
        let mut draft = Draft::new(&schema, &value);
        assert_eq!(draft.value().unwrap(), value);
        let id = draft.at_path(&(path.clone() + "/name")).unwrap().id.clone();
        draft.apply(&HashMap::from([(
            id,
            FieldInput {
                present: true,
                raw: "edited leaf".into(),
            },
        )]));
        let mut expected = value.clone();
        expected
            .pointer_mut(&(path + "/name"))
            .map(|v| *v = json!("edited leaf"));
        assert_eq!(draft.value().unwrap(), expected);
        let children = draft.at_path("/a~1b~0c/children").unwrap().id.clone();
        draft.array_action(&schema, &children, "add", None).unwrap();
        assert_eq!(
            draft.at_path("/a~1b~0c/children/1").unwrap().kind,
            InputKind::Object
        );
    }
    #[test]
    fn navigation_stops_raw_fallback_and_explicit_overrides_are_separate() {
        let schema = json!({"type":"object","x-presentation":{"rawFallback":false},"properties":{"endpoint":{"type":"object","x-presentation":{"navigation":"stop"},"properties":{"label":{"type":"string"},"notes":{"type":"array","items":{"type":"string","x-editor":"markdown"}},"blocked":{"type":"object"},"raw":{"type":"object","x-presentation":{"editor":"raw"}},"continue":{"type":"object","x-presentation":{"navigation":"continue"},"properties":{"x":{"type":"integer"}}}}}}});
        let value = json!({"endpoint":{"label":"Keep","notes":["md"],"blocked":{"x":1},"raw":{"x":2},"continue":{"x":3}}});
        let mut draft = Draft::new(&schema, &value);
        assert_eq!(draft.at_path("/endpoint").unwrap().kind, InputKind::Object);
        assert_eq!(
            draft.at_path("/endpoint/notes").unwrap().kind,
            InputKind::Array
        );
        let blocked = draft.at_path("/endpoint/blocked").unwrap();
        assert_eq!(blocked.kind, InputKind::Unsupported);
        let id = blocked.id.clone();
        assert!(draft.set_field(&id, json!({})).is_err());
        assert_eq!(
            draft.at_path("/endpoint/raw").unwrap().kind,
            InputKind::Json
        );
        assert_eq!(
            draft.at_path("/endpoint/continue").unwrap().kind,
            InputKind::Object
        );
        assert_eq!(draft.value().unwrap(), value);
    }
    #[test]
    fn mixed_arrays_and_invalid_containers_are_preserved_without_fallback() {
        let schema = json!({"type":"object","x-presentation":{"rawFallback":false},"properties":{"mixed":{"type":"array"},"invalid":{"type":"object"},"empty":{"type":"object","properties":{"next":{"type":"object"}}},"nothing":{"type":"null"}}});
        let value = json!({"mixed":[1,"x",null],"invalid":42,"nothing":null});
        let mut draft = Draft::new(&schema, &value);
        assert_eq!(
            draft.at_path("/mixed").unwrap().kind,
            InputKind::Unsupported
        );
        assert_eq!(draft.value().unwrap(), value);
        let id = draft.at_path("/empty").unwrap().id.clone();
        draft.include_field(&schema, &id).unwrap();
        assert_eq!(draft.value().unwrap()["empty"], json!({}));
        assert!(draft.at_path("/empty/next").is_some());
    }
}

#[cfg(test)]
mod recursive_boundary_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn depth_boundary_preserves_values_with_and_without_raw_fallback() {
        let mut value = json!({"leaf": "preserved"});
        for _ in 0..70 {
            value = json!({"child": value});
        }
        for fallback in [true, false] {
            let schema = json!({"type":"object","x-presentation":{"rawFallback":fallback}});
            let draft = Draft::new(&schema, &value);
            let boundary = draft
                .paths()
                .into_iter()
                .find(|(_, f)| matches!(f.kind, InputKind::Json | InputKind::Unsupported))
                .expect("bounded recursion");
            assert_eq!(
                boundary.1.kind,
                if fallback {
                    InputKind::Json
                } else {
                    InputKind::Unsupported
                }
            );
            assert!(boundary.0.split('/').count() <= 67);
            assert_eq!(draft.value().unwrap(), value);
        }
    }

    #[test]
    fn protected_nested_objects_reject_descendant_edits_and_structural_actions() {
        for protection in [
            json!({"x-ownership":"immutable"}),
            json!({"x-editor":"hidden"}),
        ] {
            let mut object = json!({"type":"object","properties":{"text":{"type":"string"},"items":{"type":"array","items":{"type":"string"}}}});
            object
                .as_object_mut()
                .unwrap()
                .extend(protection.as_object().unwrap().clone());
            let schema = json!({"type":"object","properties":{"protected":object}});
            let value = json!({"protected":{"text":"original","items":["keep"]}});
            let mut draft = Draft::new(&schema, &value);
            let text = draft.at_path("/protected/text").unwrap().id.clone();
            let items = draft.at_path("/protected/items").unwrap().id.clone();
            draft.apply(&HashMap::from([(
                text.clone(),
                FieldInput {
                    present: true,
                    raw: "changed".into(),
                },
            )]));
            assert!(draft.set_field(&text, json!("changed")).is_err());
            assert!(draft.set_presence(&text, false).is_err());
            assert!(draft.array_action(&schema, &items, "add", None).is_err());
            assert_eq!(draft.value().unwrap(), value);
        }
    }

    #[test]
    fn nested_markdown_last_item_unsets_property_but_not_array_item() {
        let md = json!({"type":"array","items":{"type":"string","x-editor":"markdown"}});
        let schema = json!({"type":"object","properties":{"object":{"type":"object","properties":{"notes":md}},"lists":{"type":"array","items":md}}});
        let mut draft = Draft::new(&schema, &json!({"object":{},"lists":[["keep"]]}));
        let notes = draft.at_path("/object/notes").unwrap().id.clone();
        draft.include_field(&schema, &notes).unwrap();
        assert_eq!(draft.value().unwrap()["object"]["notes"], json!([""]));
        let item = draft.at_path("/object/notes/0").unwrap().id.clone();
        draft
            .array_action(&schema, &notes, "remove", Some(&item))
            .unwrap();
        assert_eq!(draft.value().unwrap()["object"], json!({}));
        let list = draft.at_path("/lists/0").unwrap().id.clone();
        let item = draft.at_path("/lists/0/0").unwrap().id.clone();
        draft
            .array_action(&schema, &list, "remove", Some(&item))
            .unwrap();
        assert_eq!(draft.value().unwrap()["lists"], json!([[]]));
    }
}
