//! Repository schema files are an optional adapter around the in-memory core.

use std::collections::HashSet;
use std::path::Path;

use objekte_search_core::model::{FieldPath, LogicalType};
use objekte_search_core::reference::{FieldDefinition, Schema};
use objekte_search_core::repository::{CollectionDefinition, IdentityPolicy};
use objekte_search_core::source::SourceFormat;

pub fn load_definitions(root: &Path) -> Result<Vec<CollectionDefinition>, String> {
    let directory = root.join(".objekte/collections");
    let mut paths = std::fs::read_dir(&directory)
        .map_err(|e| e.to_string())?
        .map(|e| e.map(|e| e.path()).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.retain(|p| p.extension().is_some_and(|e| e == "json"));
    paths.sort();
    let mut definitions = Vec::new();
    for path in paths {
        let schema_json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).map_err(|e| e.to_string())?)
                .map_err(|e| format!("{}: {e}", path.display()))?;
        let meta = schema_json.get("x-objekte").ok_or("missing x-objekte")?;
        let string = |name: &str| -> Result<&str, String> {
            meta.get(name)
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("{}: missing x-objekte.{name}", path.display()))
        };
        let name = string("name")?.to_owned();
        let source_path = string("path")?.into();
        let format = match string("format")? {
            "json" => SourceFormat::Json,
            "jsonl" => SourceFormat::JsonLines,
            other => return Err(format!("unsupported source format {other}")),
        };
        let identity = match string("id")? {
            "filename" => IdentityPolicy::Filename,
            "generated" => IdentityPolicy::Generated,
            other => return Err(format!("unsupported identity policy {other}")),
        };
        let mut schema = Schema::default();
        let indexed_paths: HashSet<Vec<String>> = schema_json
            .get("x-index-paths")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .map(|v| {
                FieldPath::parse(v.as_str().ok_or("non-string x-index-path")?).map(|p| p.segments)
            })
            .collect::<Result<_, _>>()?;
        register_properties(&schema_json, &[], false, &indexed_paths, &mut schema)?;
        let mut sort_fields = Vec::new();
        for field in schema_json
            .get("x-sort-indexes")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
            .filter_map(|index| index.as_array())
            .flatten()
            .filter_map(|part| part.get("field").and_then(|value| value.as_str()))
        {
            let path = FieldPath::parse(field)?;
            if !sort_fields.contains(&path) {
                sort_fields.push(path);
            }
        }
        definitions.push(CollectionDefinition {
            name,
            source_path,
            format,
            identity,
            discovery: Default::default(),
            schema,
            sort_fields,
            sort_locale: "de".into(),
        });
    }
    Ok(definitions)
}

fn register_properties(
    node: &serde_json::Value,
    parent: &[String],
    multi: bool,
    indexed_paths: &HashSet<Vec<String>>,
    schema: &mut Schema,
) -> Result<(), String> {
    let Some(properties) = node.get("properties").and_then(|v| v.as_object()) else {
        return Ok(());
    };
    let required: HashSet<&str> = node
        .get("required")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
        .collect();
    for (name, child) in properties {
        if child.get("$ref").is_some() {
            return Err(format!("unresolved schema reference at {name}"));
        }
        let mut path = parent.to_vec();
        path.push(name.clone());
        let nullable = child
            .get("type")
            .and_then(|v| v.as_array())
            .is_some_and(|a| a.iter().any(|v| v == "null"));
        let kind = logical_type(child)?;
        let indexed = child
            .get("index")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            || indexed_paths.contains(&path);
        schema.register_field(FieldDefinition {
            path: path.clone(),
            kind: kind.clone(),
            required: required.contains(name.as_str()),
            nullable,
            indexed,
            multi: multi || matches!(kind, LogicalType::Array(_)),
            element_nullable: array_element_nullability(child),
        })?;
        match kind {
            LogicalType::Object => register_properties(child, &path, multi, indexed_paths, schema)?,
            LogicalType::Array(_) => {
                if let Some(items) = child.get("items") {
                    register_properties(items, &path, true, indexed_paths, schema)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn logical_type(node: &serde_json::Value) -> Result<LogicalType, String> {
    let value = node.get("type").ok_or("schema field missing type")?;
    let name = if let Some(s) = value.as_str() {
        s
    } else {
        value
            .as_array()
            .and_then(|a| a.iter().filter_map(|v| v.as_str()).find(|s| *s != "null"))
            .ok_or("schema field has no non-null type")?
    };
    Ok(match name {
        "string" => LogicalType::String,
        "integer" | "number" => LogicalType::Number,
        "boolean" => LogicalType::Bool,
        "object" => LogicalType::Object,
        "array" => LogicalType::Array(Box::new(logical_type(
            node.get("items").ok_or("array without items")?,
        )?)),
        other => return Err(format!("unsupported schema type {other}")),
    })
}

fn array_element_nullability(node: &serde_json::Value) -> Vec<bool> {
    let mut result = Vec::new();
    let mut current = node;
    while current.get("type").is_some_and(|kind| {
        kind == "array"
            || kind
                .as_array()
                .is_some_and(|types| types.iter().any(|item| item == "array"))
    }) {
        let Some(items) = current.get("items") else {
            break;
        };
        let nullable = items
            .get("type")
            .and_then(|kind| kind.as_array())
            .is_some_and(|types| types.iter().any(|item| item == "null"));
        result.push(nullable);
        current = items;
    }
    result
}

#[cfg(test)]
#[path = "schema.test.rs"]
mod tests;
