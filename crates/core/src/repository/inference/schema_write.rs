//! Reconcile inferred field types and posting rules in an existing JSON Schema.

use crate::{
    CollectionInference, FieldInference, InferredKind, InferredLinkKind, JsonPathStep, Result,
    error::{config, io},
    schema::{Schema, index_paths},
};
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::Path,
};

pub(super) fn extend_schema(path: &Path, report: &CollectionInference) -> Result<bool> {
    let raw = fs::read(path).map_err(|error| io(path, error))?;
    let mut schema: Value =
        serde_json::from_slice(&raw).map_err(|error| config(path, "", error))?;
    let mut fields = report.fields.iter().collect::<Vec<_>>();
    fields.sort_by_key(|field| field.path.len());
    let by_path = fields
        .iter()
        .map(|field| (field.path.clone(), *field))
        .collect::<BTreeMap<_, _>>();
    let mut changed = false;
    for field in fields {
        changed |= add_field(&mut schema, field, &by_path, report.auto_index_requested);
    }
    let existing_paths = index_paths(&schema, path)?
        .into_iter()
        .collect::<BTreeSet<_>>();
    if report.auto_index_requested {
        let mut paths = existing_paths.clone();
        for field in &report.fields {
            paths.remove(&field.path);
            if !field.recommended_index {
                continue;
            }
            if let [JsonPathStep::Key(name)] = field.path.as_slice()
                && schema["properties"][name]["index"] == Value::Bool(true)
            {
                continue;
            }
            paths.insert(field.path.clone());
        }
        if paths != existing_paths {
            changed = true;
            let root = schema.as_object_mut().expect("compiled schema root object");
            if paths.is_empty() {
                root.remove("x-index-paths");
            } else {
                root.insert(
                    "x-index-paths".into(),
                    Value::Array(
                        paths
                            .iter()
                            .map(|path| Value::String(crate::json_filter::format_path(path)))
                            .collect(),
                    ),
                );
            }
        }
    }
    if !changed {
        return Ok(false);
    }
    // Validate the complete candidate before replacing the original file.
    Schema::compile(&schema, path)?;
    let parent = path
        .parent()
        .ok_or_else(|| config(path, "", "schema has no parent"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|error| io(path, error))?;
    let mut encoded =
        serde_json::to_vec_pretty(&schema).map_err(|error| config(path, "", error))?;
    encoded.push(b'\n');
    temporary
        .write_all(&encoded)
        .map_err(|error| io(path, error))?;
    temporary.flush().map_err(|error| io(path, error))?;
    temporary
        .as_file()
        .set_permissions(
            fs::metadata(path)
                .map_err(|error| io(path, error))?
                .permissions(),
        )
        .map_err(|error| io(path, error))?;
    temporary
        .persist(path)
        .map_err(|error| io(path, error.error))?;
    Ok(true)
}

fn add_field(
    root: &mut Value,
    field: &FieldInference,
    fields: &BTreeMap<crate::JsonPath, &FieldInference>,
    update_indexes: bool,
) -> bool {
    let mut current = root;
    for (position, step) in field.path.iter().enumerate() {
        let last = position + 1 == field.path.len();
        match step {
            JsonPathStep::Key(name) => {
                if schema_kind(current) != Some("object") {
                    return false;
                }
                let Some(object) = current.as_object_mut() else {
                    return false;
                };
                let properties = object.entry("properties").or_insert_with(|| json!({}));
                let Some(properties) = properties.as_object_mut() else {
                    return false;
                };
                if last {
                    if let Some(existing) = properties.get_mut(name) {
                        return merge_field(existing, field, fields, update_indexes);
                    }
                    let Some(declaration) = declaration(field, fields) else {
                        return false;
                    };
                    properties.insert(name.clone(), declaration);
                    return true;
                }
                let next_array = matches!(field.path[position + 1], JsonPathStep::Each);
                current = properties.entry(name.clone()).or_insert_with(|| {
                    if next_array {
                        json!({"type":"array", "items": {}})
                    } else {
                        json!({"type":"object", "properties": {}})
                    }
                });
            }
            JsonPathStep::Each => {
                if schema_kind(current) != Some("array") {
                    return false;
                }
                let Some(object) = current.as_object_mut() else {
                    return false;
                };
                if last {
                    if let Some(existing) = object.get_mut("items") {
                        return merge_field(existing, field, fields, update_indexes);
                    }
                    let Some(declaration) = declaration(field, fields) else {
                        return false;
                    };
                    object.insert("items".into(), declaration);
                    return true;
                }
                current = object
                    .entry("items")
                    .or_insert_with(|| json!({"type":"object", "properties": {}}));
            }
            JsonPathStep::Index(_) => return false,
        }
    }
    false
}

fn schema_kind(value: &Value) -> Option<&str> {
    match value.get("type")? {
        Value::String(kind) => Some(kind),
        Value::Array(kinds) => kinds
            .iter()
            .filter_map(Value::as_str)
            .find(|kind| *kind != "null"),
        _ => None,
    }
}

fn merge_field(
    existing: &mut Value,
    field: &FieldInference,
    fields: &BTreeMap<crate::JsonPath, &FieldInference>,
    update_indexes: bool,
) -> bool {
    let Some(kind) = inferred_kind(field) else {
        if update_indexes && field.path.len() == 1 && existing["index"] == Value::Bool(true) {
            existing
                .as_object_mut()
                .expect("schema field object")
                .insert("index".into(), Value::Bool(false));
            return true;
        }
        return false;
    };
    let object = existing
        .as_object_mut()
        .expect("compiled schema field object");
    let mut changed = false;
    let inferred_type = if field.null_values > 0 && kind != "null" {
        json!([kind, "null"])
    } else {
        json!(kind)
    };
    if object.get("type") != Some(&inferred_type) {
        object.insert("type".into(), inferred_type);
        changed = true;
    }
    if kind != "object" {
        for key in ["properties", "required", "additionalProperties"] {
            changed |= object.remove(key).is_some();
        }
    }
    if kind != "array" {
        changed |= object.remove("items").is_some();
    } else if !object.contains_key("items") {
        let items = declaration(field, fields).expect("array declaration")["items"].clone();
        object.insert("items".into(), items);
        changed = true;
    }
    if kind != "string" {
        // These keywords require a string type in the schema parser.
        changed |= object.remove("x-full-text").is_some();
        changed |= object.remove("x-relation").is_some();
    }
    if field.kind == InferredKind::Enum {
        let mut values = field
            .enum_values
            .as_ref()
            .expect("enum values")
            .iter()
            .cloned()
            .map(Value::String)
            .collect::<Vec<_>>();
        if field.null_values > 0 {
            values.push(Value::Null);
        }
        let inferred_enum = Value::Array(values);
        if object.get("enum") != Some(&inferred_enum) {
            object.insert("enum".into(), inferred_enum);
            changed = true;
        }
    } else {
        changed |= object.remove("enum").is_some();
    }
    if field.path.len() == 1 && update_indexes {
        if field.recommended_index {
            if object.get("index") != Some(&Value::Bool(true)) {
                object.insert("index".into(), Value::Bool(true));
                changed = true;
            }
        } else if object.get("index") == Some(&Value::Bool(true)) {
            object.insert("index".into(), Value::Bool(false));
            changed = true;
        }
    } else if !matches!(kind, "boolean" | "integer" | "number" | "string" | "null")
        && object.get("index") == Some(&Value::Bool(true))
    {
        // A true scalar posting rule would make the new shape invalid.
        object.remove("index");
        changed = true;
    }
    changed
}

fn inferred_kind(field: &FieldInference) -> Option<&'static str> {
    Some(match field.kind {
        InferredKind::Boolean => "boolean",
        InferredKind::Integer => "integer",
        InferredKind::Number => "number",
        InferredKind::Enum | InferredKind::String => "string",
        InferredKind::Array => "array",
        InferredKind::Object => "object",
        InferredKind::Null => "null",
        InferredKind::Mixed => return None,
    })
}

fn declaration(
    field: &FieldInference,
    fields: &BTreeMap<crate::JsonPath, &FieldInference>,
) -> Option<Value> {
    let kind = inferred_kind(field)?;
    let mut object = Map::new();
    object.insert(
        "type".into(),
        if field.null_values > 0 && kind != "null" {
            json!([kind, "null"])
        } else {
            json!(kind)
        },
    );
    if kind == "array" {
        let mut item_path = field.path.clone();
        item_path.push(JsonPathStep::Each);
        let items = fields
            .get(&item_path)
            .and_then(|item| declaration(item, fields))
            .unwrap_or_else(|| json!({}));
        object.insert("items".into(), items);
    }
    if field.recommended_index
        && field.path.len() == 1
        && matches!(kind, "boolean" | "integer" | "number" | "string")
    {
        object.insert("index".into(), json!(true));
    }
    if field.kind == InferredKind::Enum {
        let mut values = field
            .enum_values
            .as_ref()
            .expect("enum values")
            .iter()
            .cloned()
            .map(Value::String)
            .collect::<Vec<_>>();
        if field.null_values > 0 {
            values.push(Value::Null);
        }
        object.insert("enum".into(), Value::Array(values));
    }
    if let Some(format) = field.text_format {
        object.insert(
            "x-full-text".into(),
            json!(match format {
                crate::TextFormat::Markdown => "markdown",
                crate::TextFormat::Html => "html",
                crate::TextFormat::Xml => "xml",
                crate::TextFormat::Plain => "plain",
            }),
        );
    }
    if let Some(link) = &field.link
        && link.kind == InferredLinkKind::RecordId
        && link.resolved_values == link.total_values
    {
        object.insert(
            "x-relation".into(),
            json!({"collection": link.target_collection}),
        );
    }
    Some(Value::Object(object))
}
