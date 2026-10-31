//! Add only missing inferred declarations to an existing JSON Schema.

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
        changed |= add_field(&mut schema, field, &by_path);
    }
    let mut paths = index_paths(&schema, path)?
        .into_iter()
        .collect::<BTreeSet<_>>();
    for field in &report.fields {
        if !field.indexed
            || !matches!(
                field.kind,
                InferredKind::Boolean
                    | InferredKind::Integer
                    | InferredKind::Number
                    | InferredKind::Enum
            )
        {
            continue;
        }
        // An explicit false remains an opt-out; true already has a static posting.
        if let [JsonPathStep::Key(name)] = field.path.as_slice()
            && schema["properties"][name]
                .get("index")
                .is_some_and(Value::is_boolean)
        {
            continue;
        }
        changed |= paths.insert(field.path.clone());
    }
    if !paths.is_empty() {
        schema
            .as_object_mut()
            .expect("compiled schema root object")
            .insert(
                "x-index-paths".into(),
                Value::Array(
                    paths
                        .iter()
                        .map(|path| Value::String(crate::json_filter::format_path(path)))
                        .collect(),
                ),
            );
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
) -> bool {
    let Some(declaration) = declaration(field, fields) else {
        return false;
    };
    let mut current = root;
    for (position, step) in field.path.iter().enumerate() {
        let last = position + 1 == field.path.len();
        match step {
            JsonPathStep::Key(name) => {
                let Some(object) = current.as_object_mut() else {
                    return false;
                };
                if object.get("type").and_then(Value::as_str) != Some("object") {
                    return false;
                }
                let properties = object.entry("properties").or_insert_with(|| json!({}));
                let Some(properties) = properties.as_object_mut() else {
                    return false;
                };
                if last {
                    if properties.contains_key(name) {
                        return false;
                    }
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
                let Some(object) = current.as_object_mut() else {
                    return false;
                };
                if object.get("type").and_then(Value::as_str) != Some("array") {
                    return false;
                }
                if last {
                    if object.contains_key("items") {
                        return false;
                    }
                    object.insert("items".into(), declaration);
                    return true;
                }
                current = object
                    .entry("items")
                    .or_insert_with(|| json!({"type":"object", "properties": {}}));
                // An explicitly untyped `items: {}` stays authoritative.
            }
            JsonPathStep::Index(_) => return false,
        }
    }
    false
}

fn declaration(
    field: &FieldInference,
    fields: &BTreeMap<crate::JsonPath, &FieldInference>,
) -> Option<Value> {
    let kind = match field.kind {
        InferredKind::Boolean => "boolean",
        InferredKind::Integer => "integer",
        InferredKind::Number => "number",
        InferredKind::Enum | InferredKind::String => "string",
        InferredKind::Array => "array",
        InferredKind::Object => "object",
        InferredKind::Null => "null",
        InferredKind::Mixed => return None,
    };
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
    if field.indexed
        && field.path.len() == 1
        && matches!(kind, "boolean" | "integer" | "number" | "string")
    {
        object.insert("index".into(), json!(true));
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
