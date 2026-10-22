//! Resolve repository storage, data schemas and presentation independently.
use crate::config::{CategoryLevel, JsonDocumentConfig, SettingsConfig};
use crate::{Error, Result, contained};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path};

pub(crate) struct Loaded {
    pub schema: Value,
    pub data_schema: Option<Value>,
    pub data_schemas: BTreeMap<String, Value>,
    pub constraints: BTreeMap<String, Vec<CategoryLevel>>,
    pub ui: Value,
    pub settings: Option<SettingsConfig>,
    pub documents: BTreeMap<String, JsonDocumentConfig>,
    pub media_collections: BTreeMap<String, crate::config::MediaCollection>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Repository {
    objects: String,
    ui: String,
    media: String,
    collections: BTreeMap<String, Storage>,
    settings: String,
    #[serde(default)]
    documents: BTreeMap<String, DocumentMapping>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Storage {
    path: String,
    format: String,
    id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentMapping {
    schema: String,
    files: Vec<String>,
}

fn read(root: &Path, path: &str) -> Result<Value> {
    serde_json::from_slice(&fs::read(contained(root, path)?)?)
        .map_err(|e| Error::Config(format!("{path}: {e}")))
}
fn invalid(message: impl Into<String>) -> Error {
    Error::Config(message.into())
}

pub(crate) fn load(root: &Path) -> Result<Loaded> {
    // Transitional read support for existing repositories. New projects use the split files.
    if !root.join(".editor/repository.json").exists() {
        return Ok(Loaded {
            schema: read(root, ".editor/schema.json")?,
            data_schema: None,
            data_schemas: BTreeMap::new(),
            constraints: BTreeMap::new(),
            ui: json!({}),
            settings: None,
            documents: BTreeMap::new(),
            media_collections: BTreeMap::new(),
        });
    }
    let manifest = read(root, ".editor/repository.json")?;
    if manifest.get("version").and_then(Value::as_u64) == Some(2) {
        return super::collection_config::load(root, manifest);
    }
    let repo: Repository =
        serde_json::from_value(manifest).map_err(|e| invalid(format!("repository.json: {e}")))?;
    let data = read(root, &repo.objects)?;
    let ui = read(root, &repo.ui)?;
    let presentations = ui
        .get("collections")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("ui.collections must be an object"))?;
    for name in presentations.keys() {
        if !repo.collections.contains_key(name) {
            return Err(invalid(format!("UI references unknown collection {name}")));
        }
    }
    let mut protected = vec![
        contained(root, ".editor/repository.json")?,
        contained(root, &repo.objects)?,
        contained(root, &repo.ui)?,
    ];
    for mapping in repo.documents.values() {
        protected.push(contained(root, &mapping.schema)?);
    }
    let settings_path = contained(root, &repo.settings)?;
    if !settings_path.is_file() || protected.contains(&settings_path) {
        return Err(invalid(
            "editor settings must be a data file, not configuration",
        ));
    }
    for storage in repo.collections.values() {
        if settings_path.starts_with(contained(root, &storage.path)?) {
            return Err(invalid(
                "editor instance settings cannot also be a collection record",
            ));
        }
    }
    let mut documents = BTreeMap::new();
    let mut assigned = std::collections::HashSet::new();
    let mut settings_schema = json!({"type":"object"});
    for (name, mapping) in &repo.documents {
        crate::safe_name(name)?;
        if mapping.files.is_empty() {
            return Err(invalid(format!(
                "{name}: schema mapping needs at least one file"
            )));
        }
        let document_schema = read(root, &mapping.schema)?;
        jsonschema::draft202012::options()
            .offline()
            .build(&document_schema)
            .map_err(|e| invalid(format!("{name}: invalid document schema: {e}")))?;
        for file in &mapping.files {
            let path = contained(root, file)?;
            if !path.is_file()
                || path.extension().and_then(|s| s.to_str()) != Some("json")
                || protected.contains(&path)
            {
                return Err(invalid(format!(
                    "{name}: {file} must be a JSON data file, not configuration"
                )));
            }
            if !assigned.insert(path.clone()) {
                return Err(invalid(format!("duplicate schema assignment for {file}")));
            }
            if path == settings_path {
                settings_schema = document_schema.clone();
            }
        }
        documents.insert(
            name.clone(),
            JsonDocumentConfig {
                schema: document_schema,
                files: mapping.files.clone(),
            },
        );
    }
    let mut schema = data.clone();
    let defs = data.get("$defs").cloned();
    let draft = data.get("$schema").cloned();
    let keyword = if data.get("oneOf").is_some() {
        "oneOf"
    } else {
        "anyOf"
    };
    let branches = schema
        .get_mut(keyword)
        .and_then(Value::as_array_mut)
        .ok_or_else(|| invalid("objects schema needs oneOf or anyOf"))?;
    let mut data_schemas = BTreeMap::new();
    let mut constraints = BTreeMap::new();
    for branch in branches {
        let name = branch
            .get("x-collection")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("each object variant needs an x-collection name"))?
            .to_string();
        let storage = repo
            .collections
            .get(&name)
            .ok_or_else(|| invalid(format!("no storage for {name}")))?;
        let mut effective = branch.clone();
        if let Some(defs) = &defs {
            effective
                .as_object_mut()
                .unwrap()
                .entry("$defs")
                .or_insert_with(|| defs.clone());
        }
        if let Some(draft) = &draft {
            effective
                .as_object_mut()
                .unwrap()
                .entry("$schema")
                .or_insert_with(|| draft.clone());
        }
        if data_schemas
            .insert(name.clone(), effective.clone())
            .is_some()
        {
            return Err(invalid(format!("duplicate collection {name}")));
        }
        let rules = branch
            .get("x-categoryConstraints")
            .cloned()
            .unwrap_or(json!([]));
        let rules: Vec<CategoryLevel> = serde_json::from_value(rules)
            .map_err(|e| invalid(format!("{name} constraints: {e}")))?;
        constraints.insert(name.clone(), rules);
        let mut metadata = json!({"name":name,"path":storage.path,"format":storage.format,"id":storage.id,"label":name,"display":"id"});
        if let Some(search) = branch.get("x-search") {
            metadata["search"] = search.clone();
        }
        if let Some(presentation) = presentations.get(&name) {
            let presentation = presentation
                .as_object()
                .ok_or_else(|| invalid("collection UI must be an object"))?;
            for (key, value) in presentation {
                if key == "fields" {
                    continue;
                }
                if ![
                    "label",
                    "display",
                    "displayFields",
                    "displayFallback",
                    "showInSidebar",
                    "categories",
                    "categoryEntries",
                    "fieldOrder",
                    "icon",
                    "list",
                    "edit",
                ]
                .contains(&key.as_str())
                {
                    return Err(invalid(format!("unsupported collection UI key {key}")));
                }
                metadata[key] = value.clone();
            }
            metadata["ui"] = Value::Object(presentation.clone());
        }
        // Produce the existing form contract in Rust; clients do not merge configuration.
        *branch = effective;
        if let Some(fields) = presentations.get(&name).and_then(|p| p.get("fields")) {
            apply_fields(branch, fields)?;
        }
        branch["x-collection"] = metadata;
    }
    if data_schemas.len() != repo.collections.len() {
        return Err(invalid("repository collections must match object variants"));
    }
    // Validate semantic hierarchy independently of any sidebar definition.
    for (name, rules) in &constraints {
        for (index, rule) in rules.iter().enumerate() {
            let source = &data_schemas[name];
            let property = rule
                .field
                .split('.')
                .try_fold(source, |v, part| v.get("properties")?.get(part));
            let target = rule
                .collection
                .as_deref()
                .ok_or_else(|| invalid("constraint requires collection"))?;
            if property
                .and_then(|p| p.pointer("/x-relation/collection"))
                .and_then(Value::as_str)
                != Some(target)
                || !data_schemas.contains_key(target)
            {
                return Err(invalid(format!(
                    "{name}: invalid constraint relation {}",
                    rule.field
                )));
            }
            if let Some(parent) = &rule.parent_field {
                let expected = index
                    .checked_sub(1)
                    .and_then(|i| rules[i].collection.as_deref());
                let actual = parent
                    .split('.')
                    .try_fold(&data_schemas[target], |v, part| {
                        v.get("properties")?.get(part)
                    })
                    .and_then(|p| p.pointer("/x-relation/collection"))
                    .and_then(Value::as_str);
                if expected.is_none() || actual != expected {
                    return Err(invalid(format!("{name}: invalid parent constraint")));
                }
            }
        }
    }
    schema["x-project"] = json!({"media":repo.media});
    Ok(Loaded {
        schema,
        data_schema: Some(data),
        data_schemas,
        constraints,
        ui,
        settings: Some(SettingsConfig {
            path: repo.settings,
            schema: settings_schema,
        }),
        documents,
        media_collections: BTreeMap::new(),
    })
}

pub(crate) fn apply_fields(schema: &mut Value, fields: &Value) -> Result<()> {
    for (pointer, annotations) in fields
        .as_object()
        .ok_or_else(|| invalid("UI fields must map JSON pointers to annotations"))?
    {
        let target = schema
            .pointer_mut(pointer)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| invalid(format!("unknown UI field pointer {pointer}")))?;
        for (key, value) in annotations
            .as_object()
            .ok_or_else(|| invalid("field annotations must be an object"))?
        {
            if ![
                "title",
                "description",
                "x-editor",
                "x-options",
                "x-autocomplete",
                "x-create",
                "x-presentation",
                "x-visible",
            ]
            .contains(&key.as_str())
            {
                return Err(invalid(format!("UI cannot override schema keyword {key}")));
            }
            if key == "x-create" {
                let relation = target
                    .get_mut("x-relation")
                    .and_then(Value::as_object_mut)
                    .ok_or_else(|| invalid("x-create requires a relation"))?;
                relation.insert("create".into(), value.clone());
            } else {
                target.insert(key.clone(), value.clone());
            }
        }
    }
    Ok(())
}
