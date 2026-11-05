//! Musenalm schema vocabulary belongs to the test integration, not the core.

use std::collections::HashSet;
use std::path::Path;

use objekte_search_core::model::{FieldPath, LogicalType};
use objekte_search_core::reference::{CustomBinding, FieldDefinition, Schema, Transform};
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
        if name == "agents" {
            schema.register_custom(
                vec!["name".into()],
                CustomBinding {
                    name: "folded".into(),
                    version: 1,
                    transform: Transform::CaseFold,
                },
            )?;
        }
        definitions.push(CollectionDefinition {
            name,
            source_path,
            format,
            identity,
            schema,
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

#[cfg(test)]
mod tests {
    use super::*;
    use objekte_search_core::model::{Number, Value};
    use objekte_search_core::query::Query;
    use objekte_search_core::reference::bind_filter;
    use objekte_search_core::source::{SourceFormat, reader};
    use objekte_search_core::workload::load_manifest;

    #[test]
    fn fixture_schemas_bind_every_fixed_request() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/musenalm");
        let definitions = load_definitions(&root).unwrap();
        assert_eq!(definitions.len(), 7);
        let cases = load_manifest(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/musenalm_queries.json"),
        )
        .unwrap();
        for case in cases {
            let query = Query::from_workload(&case).unwrap();
            for definition in definitions
                .iter()
                .filter(|d| case.collection == "*" || case.collection == d.name)
            {
                if let Some(filter) = &query.filter {
                    bind_filter(filter, &definition.schema)
                        .unwrap_or_else(|e| panic!("{} {}: {e}", case.id, definition.name));
                }
                if let Some(fts) = &query.fts {
                    for field in &fts.fields {
                        definition.schema.field(&field.path).unwrap();
                    }
                }
            }
        }
    }

    #[test]
    fn fractional_numbering_remains_numeric() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/musenalm/data/contents/00mc7w4nobyg3xt.json");
        let extracted = reader(SourceFormat::Json)
            .parse(&std::fs::read(path).unwrap())
            .unwrap();
        let Value::Object(fields) = extracted.value else {
            panic!("object expected")
        };
        assert_eq!(
            fields.get("numbering"),
            Some(&Value::Number(Number::float(32.01).unwrap()))
        );
    }
}
