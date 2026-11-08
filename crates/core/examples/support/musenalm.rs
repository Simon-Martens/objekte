//! Musenalm schema vocabulary belongs to the test integration, not the core.

use std::collections::HashSet;
use std::path::Path;

use objekte_search_core::model::{FieldPath, LogicalType};
use objekte_search_core::reference::{CustomBinding, FieldDefinition, Schema, Transform};
use objekte_search_core::relation::{RelationCardinality, RelationDefinition};
use objekte_search_core::repository::{CollectionDefinition, IdentityPolicy};
use objekte_search_core::source::SourceFormat;
use serde::Serialize;
use sha2::{Digest, Sha256};


#[derive(Serialize)]
pub struct LoadedSchema {
    pub collection: String,
    pub path: String,
    pub schema: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
}


#[derive(Clone, Copy, Eq, PartialEq)]
enum SchemaMode {
    Historical,
    ExtendedRelations,
}


/// Mutable state shared while registering one collection's schema fields.
struct SchemaRegistration<'a> {
    indexed_paths: &'a HashSet<Vec<String>>,
    schema: &'a mut Schema,
    mode: SchemaMode,
    source_collection: &'a str,
    relations: &'a mut Vec<RelationDefinition>,
}


/// The documents returned here are the exact parsed documents used to create
/// the benchmark's in-memory definitions, independent of later checkout edits.
pub fn load_definitions_with_schemas(
    root: &Path,
) -> Result<(Vec<CollectionDefinition>, Vec<LoadedSchema>), String> {
    load_definitions_with_mode(root, SchemaMode::Historical)
}

/// Interpret `x-relation` annotations as typed IDs and declared links.
/// The returned schema documents retain their original bytes after parsing;
/// the adapter marker distinguishes this registration from historical loads.
pub fn load_extended_definitions_with_schemas(
    root: &Path,
) -> Result<(Vec<CollectionDefinition>, Vec<LoadedSchema>), String> {
    load_definitions_with_mode(root, SchemaMode::ExtendedRelations)
}

fn load_definitions_with_mode(
    root: &Path,
    mode: SchemaMode,
) -> Result<(Vec<CollectionDefinition>, Vec<LoadedSchema>), String> {
    let directory = root.join(".objekte/collections");
    let mut paths = std::fs::read_dir(&directory)
        .map_err(|e| e.to_string())?
        .map(|e| e.map(|e| e.path()).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.retain(|p| p.extension().is_some_and(|e| e == "json"));
    paths.sort();
    let mut definitions = Vec::new();
    let mut loaded_schemas = Vec::new();
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
        let relative = path
            .strip_prefix(root)
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        loaded_schemas.push(LoadedSchema {
            collection: name.clone(),
            path: relative,
            schema: schema_json.clone(),
            adapter: (mode == SchemaMode::ExtendedRelations)
                .then(|| "musenalm_relations_v1".to_owned()),
        });
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
        let mut relations = Vec::new();
        register_properties(
            &schema_json,
            &[],
            false,
            &mut SchemaRegistration {
                indexed_paths: &indexed_paths,
                schema: &mut schema,
                mode,
                source_collection: &name,
                relations: &mut relations,
            },
        )?;
        for relation in relations {
            schema.register_relation(relation)?;
        }
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
        let sort_names: &[&str] = match name.as_str() {
            "places" => &["name", "@file.size"],
            "agents" => &["name"],
            "entries" => &["year", "preferred_title"],
            "contents" => &["numbering", "preferred_title"],
            "series" => &["title"],
            _ => &[],
        };
        definitions.push(CollectionDefinition {
            name,
            source_path,
            format,
            identity,
            discovery: Default::default(),
            schema,
            sort_fields: sort_names
                .iter()
                .map(|s| FieldPath::parse(s))
                .collect::<Result<_, _>>()?,
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        });
    }
    loaded_schemas.sort_by(|a, b| (&a.collection, &a.path).cmp(&(&b.collection, &b.path)));
    Ok((definitions, loaded_schemas))
}

pub fn load_definitions(root: &Path) -> Result<Vec<CollectionDefinition>, String> {
    load_definitions_with_schemas(root).map(|(definitions, _)| definitions)
}

pub fn load_extended_definitions(root: &Path) -> Result<Vec<CollectionDefinition>, String> {
    load_extended_definitions_with_schemas(root).map(|(definitions, _)| definitions)
}

pub fn schema_manifest(schemas: &[LoadedSchema]) -> Result<serde_json::Value, String> {
    // The desktop's GPUI dependency enables serde_json/preserve_order for
    // workspace builds. Sort schema object keys explicitly so the provenance
    // digest does not depend on which other crates Cargo builds alongside us.
    #[derive(Serialize)]
    struct CanonicalSchema<'a> {
        collection: &'a str,
        path: &'a str,
        schema: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        adapter: Option<&'a str>,
    }


    fn sorted_objects(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(fields) => {
                let mut entries = fields.iter().collect::<Vec<_>>();
                entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
                let mut ordered = serde_json::Map::new();
                for (key, value) in entries {
                    ordered.insert(key.clone(), sorted_objects(value));
                }
                serde_json::Value::Object(ordered)
            }
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.iter().map(sorted_objects).collect())
            }
            _ => value.clone(),
        }
    }

    let canonical = schemas
        .iter()
        .map(|item| CanonicalSchema {
            collection: &item.collection,
            path: &item.path,
            schema: sorted_objects(&item.schema),
            adapter: item.adapter.as_deref(),
        })
        .collect::<Vec<_>>();
    let canonical_json = serde_json::to_string(&canonical).map_err(|error| error.to_string())?;
    let digest = Sha256::digest(canonical_json.as_bytes());
    Ok(serde_json::json!({
        "version": 1,
        "schemas": schemas,
        "canonical_json": canonical_json,
        "sha256": format!("{digest:x}"),
    }))
}

fn register_properties(
    node: &serde_json::Value,
    parent: &[String],
    multi: bool,
    registration: &mut SchemaRegistration<'_>,
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
        let kind = logical_type(child, registration.mode, false)?;
        let indexed = child
            .get("index")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            || registration.indexed_paths.contains(&path);
        registration.schema.register_field(FieldDefinition {
            path: path.clone(),
            kind: kind.clone(),
            required: required.contains(name.as_str()),
            nullable,
            indexed,
            multi: multi || matches!(kind, LogicalType::Array(_)),
            element_nullable: array_element_nullability(child),
        })?;
        if registration.mode == SchemaMode::ExtendedRelations
            && let Some(annotation) = relation_annotation(child)
        {
            let target_collection = annotation
                .get("collection")
                .and_then(|value| value.as_str())
                .ok_or_else(|| format!("{}: x-relation.collection must be text", path.join(".")))?;
            let reverse_alias = match annotation.get("reverse_alias") {
                Some(value) => value
                    .as_str()
                    .ok_or_else(|| {
                        format!("{}: x-relation.reverse_alias must be text", path.join("."))
                    })?
                    .to_owned(),
                None => format!("{}__{}", registration.source_collection, path.join("__")),
            };
            registration.relations.push(RelationDefinition {
                path: path.clone(),
                target_collection: target_collection.to_owned(),
                cardinality: if multi || matches!(kind, LogicalType::Array(_)) {
                    RelationCardinality::Many
                } else {
                    RelationCardinality::One
                },
                reverse_alias: Some(reverse_alias),
            });
        }
        match kind {
            LogicalType::Object => register_properties(child, &path, multi, registration)?,
            LogicalType::Array(_) => {
                if let Some(items) = child.get("items") {
                    register_properties(items, &path, true, registration)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn logical_type(
    node: &serde_json::Value,
    mode: SchemaMode,
    inherited_relation: bool,
) -> Result<LogicalType, String> {
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
        "string"
            if mode == SchemaMode::ExtendedRelations
                && (inherited_relation || node.get("x-relation").is_some()) =>
        {
            LogicalType::Id
        }
        "string" => LogicalType::String,
        "integer" | "number" => LogicalType::Number,
        "boolean" => LogicalType::Bool,
        "object" => LogicalType::Object,
        "array" => LogicalType::Array(Box::new(logical_type(
            node.get("items").ok_or("array without items")?,
            mode,
            inherited_relation || node.get("x-relation").is_some(),
        )?)),
        other => return Err(format!("unsupported schema type {other}")),
    })
}

fn relation_annotation(node: &serde_json::Value) -> Option<&serde_json::Value> {
    node.get("x-relation").or_else(|| {
        node.get("items")
            .filter(|_| {
                node.get("type").is_some_and(|kind| {
                    kind == "array"
                        || kind
                            .as_array()
                            .is_some_and(|types| types.iter().any(|item| item == "array"))
                })
            })
            .and_then(relation_annotation)
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
#[path = "musenalm.test.rs"]
mod tests;
