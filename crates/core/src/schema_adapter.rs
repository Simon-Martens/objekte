//! Explicit JSON Schema subset shared by the folder CLI and repository TUI.

use std::collections::HashSet;

use serde_json::Value;

use crate::model::{FieldPath, LogicalType, PathRoot};
use crate::query_path::QueryPath;
use crate::reference::{FieldDefinition, Schema};
use crate::relation::{RelationCardinality, RelationDefinition};
use crate::workload::SortField;

pub struct CompiledSchema {
    pub schema: Schema,
    pub sort_fields: Vec<FieldPath>,
}

/// Compile the supported declarations and Objekte index annotations from one
/// parsed document. Repository location, reader, and identity stay caller-owned.
pub fn compile_schema(document: &Value, collection: &str) -> Result<CompiledSchema, String> {
    let object = document
        .as_object()
        .ok_or("schema root must be an object")?;
    let indexed_paths = parse_index_paths(object.get("x-index-paths"))?;
    let mut consumed = HashSet::new();
    let mut schema = Schema::default();
    let root = resolve(document, document, &mut HashSet::new(), 0)?;
    let root_kind = logical_type(document, &root)?;
    let root_index = index_annotation(&root)?;
    if root_kind != LogicalType::Object || root_index.is_some() || root.get("properties").is_none()
    {
        let indexed = root_index == Some(true) || indexed_paths.contains(&Vec::<String>::new());
        if root_index == Some(false) && indexed {
            return Err("@root: index:false conflicts with x-index-paths".into());
        }
        if indexed && !indexable(&root_kind) {
            return Err("@root: requested index has unsupported type".into());
        }
        schema.register_field(FieldDefinition {
            path: Vec::new(),
            kind: root_kind.clone(),
            required: true,
            nullable: nullable(&root)?,
            indexed,
            multi: matches!(root_kind, LogicalType::Array(_)),
            element_nullable: array_element_nullability(document, &root)?,
        })?;
        if root_index == Some(false) {
            schema.prohibit_auto_index(Vec::new())?;
        }
        if indexed_paths.contains(&Vec::<String>::new()) {
            consumed.insert(Vec::new());
        }
    }
    register_properties(
        document,
        &root,
        &[],
        false,
        collection,
        &indexed_paths,
        &mut consumed,
        &mut schema,
        0,
    )?;
    for path in &indexed_paths {
        if !consumed.contains(path) {
            return Err(format!(
                "{}: manual index path has no supported declaration",
                FieldPath {
                    root: PathRoot::Data,
                    segments: path.clone()
                }
                .display()
            ));
        }
    }
    let sort_fields = parse_sort_indexes(object.get("x-sort-indexes"), &mut schema)?;
    Ok(CompiledSchema {
        schema,
        sort_fields,
    })
}

fn parse_index_paths(value: Option<&Value>) -> Result<HashSet<Vec<String>>, String> {
    let Some(value) = value else {
        return Ok(HashSet::new());
    };
    let array = value.as_array().ok_or("x-index-paths must be an array")?;
    let mut paths = HashSet::new();
    for entry in array {
        let text = entry
            .as_str()
            .ok_or("x-index-paths entry must be a string")?;
        let query = QueryPath::parse(text)?;
        if query.field.root != PathRoot::Data || query.has_position {
            return Err(format!("unsupported manual index path: {text}"));
        }
        if !paths.insert(query.field.segments) {
            return Err(format!("duplicate manual index path: {text}"));
        }
    }
    Ok(paths)
}

fn resolve(
    root: &Value,
    node: &Value,
    active: &mut HashSet<String>,
    depth: usize,
) -> Result<Value, String> {
    if depth >= 32 {
        return Err("schema reference depth exceeded".into());
    }
    let Some(reference) = node.get("$ref") else {
        return Ok(node.clone());
    };
    let reference = reference.as_str().ok_or("$ref must be a string")?;
    let pointer = reference
        .strip_prefix('#')
        .ok_or("external schema references are unsupported")?;
    if !pointer.starts_with('/') {
        return Err(format!("unsupported schema reference {reference}"));
    }
    if !active.insert(reference.to_owned()) {
        return Err(format!("cyclic schema reference {reference}"));
    }
    let target = root
        .pointer(pointer)
        .ok_or_else(|| format!("unresolved schema reference {reference}"))?;
    let mut result = resolve(root, target, active, depth + 1)?;
    active.remove(reference);
    let map = result
        .as_object_mut()
        .ok_or("referenced schema must be an object")?;
    for (key, value) in node.as_object().ok_or("schema node must be an object")? {
        if key != "$ref" {
            map.insert(key.clone(), value.clone());
        }
    }
    Ok(result)
}

fn nullable(node: &Value) -> Result<bool, String> {
    let Some(value) = node.get("type") else {
        return Err("schema declaration missing type".into());
    };
    if value.is_string() {
        return Ok(false);
    }
    let array = value
        .as_array()
        .ok_or("type must be a string or [type, null]")?;
    if array.len() != 2
        || array
            .iter()
            .filter(|item| item.as_str() == Some("null"))
            .count()
            != 1
    {
        return Err("only one type plus null is supported".into());
    }
    Ok(true)
}

fn logical_type(root: &Value, node: &Value) -> Result<LogicalType, String> {
    logical_type_at(root, node, 0)
}

fn logical_type_at(root: &Value, node: &Value, depth: usize) -> Result<LogicalType, String> {
    if depth >= 32 {
        return Err("schema type nesting exceeds 32 levels".into());
    }
    let value = node.get("type").ok_or("schema declaration missing type")?;
    let name = if let Some(name) = value.as_str() {
        name
    } else {
        nullable(node)?;
        value
            .as_array()
            .unwrap()
            .iter()
            .find_map(|item| item.as_str().filter(|name| *name != "null"))
            .ok_or("schema type has no non-null branch")?
    };
    Ok(match name {
        "string" => {
            if relation_annotation(node)?.is_some() {
                LogicalType::Id
            } else {
                LogicalType::String
            }
        }
        "integer" | "number" => LogicalType::Number,
        "boolean" => LogicalType::Bool,
        "object" => LogicalType::Object,
        "array" => {
            let item = resolve(
                root,
                node.get("items").ok_or("array without items")?,
                &mut HashSet::new(),
                0,
            )?;
            let kind = logical_type_at(root, &item, depth + 1)?;
            if relation_annotation(node)?.is_some() {
                if relation_annotation(&item)?.is_some() || kind != LogicalType::String {
                    return Err("relation array requires unannotated string items".into());
                }
                LogicalType::Array(Box::new(LogicalType::Id))
            } else {
                LogicalType::Array(Box::new(kind))
            }
        }
        other => return Err(format!("unsupported schema type {other}")),
    })
}

fn indexable(kind: &LogicalType) -> bool {
    matches!(
        kind,
        LogicalType::String
            | LogicalType::Id
            | LogicalType::Number
            | LogicalType::Bool
            | LogicalType::Date
            | LogicalType::Datetime
            | LogicalType::Unixtime
    ) || matches!(kind, LogicalType::Array(inner) if indexable(inner))
}

fn index_annotation(node: &Value) -> Result<Option<bool>, String> {
    node.get("index")
        .map(|value| value.as_bool().ok_or("index must be a boolean".into()))
        .transpose()
}

fn required_names(node: &Value) -> Result<HashSet<&str>, String> {
    let Some(value) = node.get("required") else {
        return Ok(HashSet::new());
    };
    let array = value.as_array().ok_or("required must be an array")?;
    array
        .iter()
        .map(|item| {
            item.as_str()
                .ok_or("required entry must be a string".into())
        })
        .collect()
}

fn register_properties(
    root: &Value,
    node: &Value,
    parent: &[String],
    multi: bool,
    collection: &str,
    indexed_paths: &HashSet<Vec<String>>,
    consumed: &mut HashSet<Vec<String>>,
    schema: &mut Schema,
    depth: usize,
) -> Result<(), String> {
    if depth >= 32 {
        return Err("schema property nesting exceeds 32 levels".into());
    }
    let required = required_names(node)?;
    let Some(properties) = node.get("properties") else {
        return Ok(());
    };
    let properties = properties
        .as_object()
        .ok_or("properties must be an object")?;
    for name in &required {
        if !properties.contains_key(*name) {
            return Err(format!("{}: required property is undeclared", name));
        }
    }
    for (name, child) in properties {
        let child = resolve(root, child, &mut HashSet::new(), 0)
            .map_err(|error| format!("{}.{name}: {error}", display_path(parent)))?;
        let mut path = parent.to_vec();
        path.push(name.clone());
        let kind = logical_type(root, &child)
            .map_err(|error| format!("{}: {error}", display_path(&path)))?;
        let annotation = index_annotation(&child)?;
        let manual = indexed_paths.contains(&path);
        if annotation == Some(false) && manual {
            return Err(format!(
                "{}: index:false conflicts with x-index-paths",
                display_path(&path)
            ));
        }
        let indexed = annotation == Some(true) || manual;
        if indexed && !indexable(&kind) {
            return Err(format!(
                "{}: requested index has unsupported type",
                display_path(&path)
            ));
        }
        if manual {
            consumed.insert(path.clone());
        }
        schema.register_field(FieldDefinition {
            path: path.clone(),
            kind: kind.clone(),
            required: required.contains(name.as_str()),
            nullable: nullable(&child)?,
            indexed,
            multi: multi || matches!(kind, LogicalType::Array(_)),
            element_nullable: array_element_nullability(root, &child)?,
        })?;
        if annotation == Some(false) {
            schema.prohibit_auto_index(path.clone())?;
        }
        let relation = match relation_annotation(&child)? {
            Some(relation) => Some(relation),
            None => child
                .get("items")
                .map(relation_annotation)
                .transpose()?
                .flatten(),
        };
        if let Some((target_collection, explicit_alias)) = relation {
            let reverse_alias =
                explicit_alias.unwrap_or_else(|| format!("{collection}__{}", path.join("__")));
            schema.register_relation(RelationDefinition {
                path: path.clone(),
                target_collection,
                cardinality: if multi || matches!(kind, LogicalType::Array(_)) {
                    RelationCardinality::Many
                } else {
                    RelationCardinality::One
                },
                reverse_alias: Some(reverse_alias),
            })?;
        }
        match kind {
            LogicalType::Object => register_properties(
                root,
                &child,
                &path,
                multi,
                collection,
                indexed_paths,
                consumed,
                schema,
                depth + 1,
            )?,
            LogicalType::Array(_) => {
                let item = resolve(
                    root,
                    child.get("items").ok_or("array without items")?,
                    &mut HashSet::new(),
                    0,
                )?;
                register_properties(
                    root,
                    &item,
                    &path,
                    true,
                    collection,
                    indexed_paths,
                    consumed,
                    schema,
                    depth + 1,
                )?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn array_element_nullability(root: &Value, node: &Value) -> Result<Vec<bool>, String> {
    let mut result = Vec::new();
    let mut current = node.clone();
    while matches!(logical_type(root, &current)?, LogicalType::Array(_)) {
        if result.len() >= 32 {
            return Err("schema array nesting exceeds 32 levels".into());
        }
        let item = resolve(
            root,
            current.get("items").ok_or("array without items")?,
            &mut HashSet::new(),
            0,
        )?;
        result.push(nullable(&item)?);
        current = item;
    }
    Ok(result)
}

fn relation_annotation(node: &Value) -> Result<Option<(String, Option<String>)>, String> {
    let Some(value) = node.get("x-relation") else {
        return Ok(None);
    };
    let object = value.as_object().ok_or("x-relation must be an object")?;
    if object
        .keys()
        .any(|key| key != "collection" && key != "reverse_alias")
    {
        return Err("x-relation contains an unsupported property".into());
    }
    let collection = object
        .get("collection")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .ok_or("x-relation.collection must be a nonempty string")?;
    let alias = object
        .get("reverse_alias")
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| "x-relation.reverse_alias must be a string".to_owned())
        })
        .transpose()?;
    Ok(Some((collection.to_owned(), alias)))
}

fn parse_sort_indexes(
    value: Option<&Value>,
    schema: &mut Schema,
) -> Result<Vec<FieldPath>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let entries = value.as_array().ok_or("x-sort-indexes must be an array")?;
    let mut fields = Vec::new();
    for entry in entries {
        let parts = entry
            .as_array()
            .ok_or("x-sort-indexes entry must be an array")?;
        if parts.is_empty() {
            return Err("x-sort-indexes entry is empty".into());
        }
        let mut composite = Vec::new();
        for part in parts {
            let object = part
                .as_object()
                .ok_or("x-sort-indexes part must be an object")?;
            let name = object
                .get("field")
                .and_then(Value::as_str)
                .ok_or("x-sort-indexes part lacks field")?;
            let direction = object
                .get("direction")
                .and_then(Value::as_str)
                .unwrap_or("asc");
            let descending = match direction {
                "asc" => false,
                "desc" => true,
                _ => return Err(format!("unsupported sort direction {direction}")),
            };
            if object
                .keys()
                .any(|key| key != "field" && key != "direction")
            {
                return Err("unsupported x-sort-indexes part".into());
            }
            let path = FieldPath::parse(name)?;
            if path.root != PathRoot::Data || !schema.fields().contains_key(&path.segments) {
                return Err(format!("unsupported sort field {name}"));
            }
            if !fields.contains(&path) {
                fields.push(path);
            }
            composite.push(SortField::simple(name, descending));
        }
        if composite.len() > 1 {
            schema.register_composite_sort(composite)?;
        }
    }
    Ok(fields)
}

fn display_path(path: &[String]) -> String {
    FieldPath {
        root: PathRoot::Data,
        segments: path.to_vec(),
    }
    .display()
}

#[cfg(test)]
#[path = "schema_adapter.test.rs"]
mod tests;
