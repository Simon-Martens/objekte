//! Reconcile inferred field types and posting rules in an existing JSON Schema.

use crate::{
    CollectionInference, Error, FieldInference, InferredKind, InferredLinkKind, JsonPathStep,
    Result,
    config::digest,
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

pub(super) fn extend_schema(
    path: &Path,
    report: &CollectionInference,
    expected_digest: &str,
) -> Result<bool> {
    let raw = fs::read(path).map_err(|error| io(path, error))?;
    if digest(&raw) != expected_digest {
        return Err(Error::Request(format!(
            "schema changed since inference for {}",
            report.collection
        )));
    }
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
        // Posting rules are only ever added: a declared path stays even when
        // this scan would not recommend it.
        let mut paths = existing_paths.clone();
        for field in &report.fields {
            if !field.recommended_index || field.path.len() == 1 {
                continue;
            }
            paths.insert(field.path.clone());
        }
        if paths != existing_paths {
            changed = true;
            let root = schema.as_object_mut().expect("compiled schema root object");
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
    let latest = fs::read(path).map_err(|error| io(path, error))?;
    if digest(&latest) != expected_digest {
        return Err(Error::Request(format!(
            "schema changed since inference for {}",
            report.collection
        )));
    }
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

/// Reconcile a declared field with what the scan saw, only ever widening.
/// The declared type gains `null` when nulls were seen, and `integer`
/// becomes `number` when decimals were seen. Everything else the author
/// wrote (enum, relation, text format, structure, index settings) stays.
fn merge_field(
    existing: &mut Value,
    field: &FieldInference,
    fields: &BTreeMap<crate::JsonPath, &FieldInference>,
    update_indexes: bool,
) -> bool {
    let Some(object) = existing.as_object_mut() else {
        return false;
    };
    let mut changed = false;
    let declared = declared_types(object);
    if let Some((mut kind, mut nullable)) = declared.clone() {
        let observed = inferred_kind(field);
        if kind == "integer" && observed == Some("number") {
            kind = "number".into();
        }
        if kind == "null"
            && let Some(observed) = observed.filter(|kind| *kind != "null")
        {
            // A field declared as always null turned out to hold values.
            kind = observed.into();
            nullable = true;
        }
        if field.null_values > 0 && kind != "null" {
            nullable = true;
        }
        let widened = if nullable && kind != "null" {
            json!([kind, "null"])
        } else {
            json!(kind)
        };
        if object.get("type") != Some(&widened) {
            object.insert("type".into(), widened);
            changed = true;
            // A declared enum must also admit null, or the widening is moot.
            if nullable
                && let Some(Value::Array(values)) = object.get_mut("enum")
                && !values.contains(&Value::Null)
            {
                values.push(Value::Null);
            }
        }
    }
    if declared.as_ref().is_some_and(|(kind, _)| kind == "array")
        && !object.contains_key("items")
        && let Some(items) =
            declaration(field, fields).and_then(|value| value.get("items").cloned())
    {
        object.insert("items".into(), items);
        changed = true;
    }
    if field.kind == InferredKind::Enum
        && declared.as_ref().is_some_and(|(kind, _)| kind == "string")
        && !object.contains_key("enum")
    {
        let suggested = suggestions(field);
        if object.get("x-enum-suggested") != Some(&suggested) {
            object.insert("x-enum-suggested".into(), suggested);
            changed = true;
        }
    }
    // Rules are only added; an explicit `index: false` is the author's choice.
    if update_indexes
        && field.path.len() == 1
        && field.recommended_index
        && !object.contains_key("index")
        && declared.as_ref().is_some_and(|(kind, _)| {
            matches!(kind.as_str(), "boolean" | "integer" | "number" | "string")
        })
    {
        object.insert("index".into(), Value::Bool(true));
        changed = true;
    }
    changed
}

/// Nonblank inferred enum values, offered as suggestions.
fn suggestions(field: &FieldInference) -> Value {
    Value::Array(
        field
            .enum_values
            .iter()
            .flatten()
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .map(Value::String)
            .collect(),
    )
}

/// The declared concrete type and whether it admits null.
fn declared_types(object: &Map<String, Value>) -> Option<(String, bool)> {
    match object.get("type")? {
        Value::String(kind) => Some((kind.clone(), kind == "null")),
        Value::Array(kinds) => {
            let concrete = kinds
                .iter()
                .filter_map(Value::as_str)
                .find(|kind| *kind != "null")?;
            Some((concrete.to_owned(), kinds.contains(&json!("null"))))
        }
        _ => None,
    }
}

fn inferred_kind(field: &FieldInference) -> Option<&'static str> {
    Some(match field.kind {
        InferredKind::Boolean => "boolean",
        // Whole numbers in a sample do not prove decimals cannot occur.
        InferredKind::Integer | InferredKind::Number => "number",
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
    if kind == "null" {
        // Nothing is known about a field that was always null.
        return None;
    }
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
        // A sample cannot prove the set is closed: suggest, do not enforce.
        object.insert("x-enum-suggested".into(), suggestions(field));
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
