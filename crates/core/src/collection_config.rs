//! Version 2 repository manifests: each collection owns settings, schema and UI files.
use crate::config::{JsonDocumentConfig, MediaCollection, SettingsConfig};
use crate::project_config::Loaded;
use crate::{Error, Result, contained};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::Path,
};
fn read(root: &Path, path: &str) -> Result<Value> {
    serde_json::from_slice(&fs::read(contained(root, path)?)?)
        .map_err(|e| Error::Config(format!("{path}: {e}")))
}
fn string<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v.get(k)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Config(format!("missing {k}")))
}
pub(crate) fn load(root: &Path, manifest: Value) -> Result<Loaded> {
    let ui_path = string(&manifest, "ui")?;
    let mut ui = read(root, ui_path)?;
    let entries = manifest["collections"]
        .as_object()
        .ok_or_else(|| Error::Config("collections must map names to settings files".into()))?;
    let mut protected = HashSet::from([
        contained(root, ".editor/repository.json")?,
        contained(root, ui_path)?,
    ]);
    let mut schemas = BTreeMap::new();
    let mut branches = Vec::new();
    let mut media = BTreeMap::new();
    let mut presentations = serde_json::Map::new();
    for (name, settings_path) in entries {
        crate::safe_name(name)?;
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(Error::Config("invalid collection name".into()));
        }
        let settings_path = settings_path.as_str().ok_or_else(|| {
            Error::Config("collection registration must name a settings file".into())
        })?;
        protected.insert(contained(root, settings_path)?);
        let settings = read(root, settings_path)?;
        let path = string(&settings, "path")?;
        let presentation_path = string(&settings, "ui")?;
        protected.insert(contained(root, presentation_path)?);
        let local = read(root, presentation_path)?;
        if !local.is_object() || ui.get("defaults").is_some_and(|v| !v.is_object()) {
            return Err(Error::Config("UI options must be objects".into()));
        }
        let presentation = crate::ui::inherit(
            &crate::ui::inherit(&ui["defaults"], &local),
            &ui["collections"][name],
        );
        if let Some(mode) = presentation.get("sidebarAllLinks")
            && !matches!(mode.as_str(), Some("none" | "root" | "all"))
        {
            return Err(Error::Config(format!(
                "{name}: sidebarAllLinks must be none, root, or all"
            )));
        }
        let label = presentation["label"].as_str().unwrap_or(name).to_owned();
        let operations = settings
            .get("operations")
            .map(|v| {
                serde_json::from_value::<Vec<String>>(v.clone())
                    .map_err(|e| Error::Config(e.to_string()))
            })
            .transpose()?;
        let kind = string(&settings, "type")?;
        let supported = if kind == "media" {
            &[
                "read", "upload", "mkdir", "move", "delete", "replace", "copy",
            ][..]
        } else {
            &["read", "create", "update", "rename", "delete"][..]
        };
        if let Some(ops) = &operations {
            if ops.iter().any(|op| !supported.contains(&op.as_str())) {
                return Err(Error::Config(format!("{name}: unsupported operation")));
            }
        }
        match kind {
            "media" => {
                media.insert(
                    name.clone(),
                    MediaCollection {
                        path: path.into(),
                        label,
                        operations,
                        ui: presentation.clone(),
                    },
                );
            }
            "data" => {
                let schema_path = string(&settings, "schema")?;
                protected.insert(contained(root, schema_path)?);
                let mut schema = read(root, schema_path)?;
                if schema.get("type").and_then(Value::as_str) != Some("object") {
                    return Err(Error::Config(format!(
                        "{name}: data collection schema must describe an object"
                    )));
                }
                if let Some(pattern) = settings.get("idPattern") {
                    jsonschema::draft202012::options()
                        .offline()
                        .build(&json!({"type":"string","pattern":pattern}))
                        .map_err(|e| Error::Config(format!("{name}: invalid idPattern: {e}")))?;
                }
                if settings["format"] == "markdown" {
                    let body = settings
                        .pointer("/content/body")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| {
                            Error::Config(format!("{name}: markdown needs content.body"))
                        })?;
                    if settings
                        .pointer("/content/frontmatter")
                        .and_then(Value::as_str)
                        != Some("*")
                        || schema["properties"][body]["type"] != "string"
                    {
                        return Err(Error::Config(format!(
                            "{name}: content maps a string body field and frontmatter '*'"
                        )));
                    }
                }
                if let Some(props) = schema["properties"].as_object() {
                    for (key, field) in props {
                        if let Some(derived) = field.get("x-derived") {
                            let via = derived["via"].as_str().ok_or_else(|| {
                                Error::Config(format!("{name}.{key}: derived field needs via"))
                            })?;
                            if props
                                .get(via)
                                .and_then(|s| s.pointer("/x-relation/collection"))
                                .and_then(Value::as_str)
                                .is_none()
                                || !derived["field"].is_string()
                            {
                                return Err(Error::Config(format!(
                                    "{name}.{key}: derived field needs a relation source and target field"
                                )));
                            }
                        }
                    }
                }
                // Reserved namespace is declared by the engine, never inferred from ordinary field names.
                if schema.pointer("/properties/_editor").is_some() {
                    return Err(Error::Config(
                        "_editor is reserved for system metadata".into(),
                    ));
                }
                if schema.get("properties").is_none() {
                    schema["properties"] = json!({});
                }
                schema["properties"]["_editor"] = json!({"type":"object","additionalProperties":false,"properties":{"schema_revision":{"type":"string","pattern":"^[a-f0-9]{40}$","x-ownership":"generated"},"manual_order":{"type":"integer"}}});
                let raw_schema = schema.clone();
                schemas.insert(name.clone(), raw_schema);
                if let Some(fields) = presentation.get("fields") {
                    super::project_config::apply_fields(&mut schema, fields)?;
                }
                schema["x-presentation"] =
                    crate::ui::inherit(&presentation["presentation"], &schema["x-presentation"]);
                schema["x-collection"] = json!({"name":name,"path":path,"format":string(&settings,"format")?,"id":string(&settings,"id")?,"label":label,"display":presentation["display"].as_str().unwrap_or("id"),"bodyField":settings.pointer("/content/body").and_then(Value::as_str).unwrap_or("body"),"schemaPath":schema_path,"operations":operations,"idPattern":settings.get("idPattern"),"ui":presentation});
                for key in [
                    "displayFields",
                    "displayFallback",
                    "showInSidebar",
                    "categories",
                    "categoryEntries",
                    "fieldOrder",
                    "search",
                ] {
                    if let Some(v) = presentation.get(key) {
                        schema["x-collection"][key] = v.clone();
                    }
                }
                if schema["x-collection"].get("search").is_none() {
                    schema["x-collection"]["search"] =
                        schema.get("x-search").cloned().unwrap_or(json!([]));
                }
                branches.push(schema);
            }
            other => return Err(Error::Config(format!("unknown collection type {other}"))),
        }
        presentations.insert(name.clone(), presentation);
    }
    ui["collections"] = Value::Object(presentations);
    let settings_file = string(&manifest, "settings")?;
    let settings_path = contained(root, settings_file)?;
    if !settings_path.is_file() || protected.contains(&settings_path) {
        return Err(Error::Config("settings must be a data file".into()));
    }
    for entry in entries.values() {
        let s = read(root, entry.as_str().unwrap())?;
        if settings_path.starts_with(contained(root, string(&s, "path")?)?) {
            return Err(Error::Config(
                "settings cannot be collection content".into(),
            ));
        }
    }
    let mut documents = BTreeMap::new();
    let mut assigned = HashSet::new();
    let mut settings_schema = json!({"type":"object"});
    if let Some(mappings) = manifest["documents"].as_object() {
        for mapping in mappings.values() {
            protected.insert(contained(root, string(mapping, "schema")?)?);
        }
        for (name, mapping) in mappings {
            crate::safe_name(name)?;
            let schema = read(root, string(mapping, "schema")?)?;
            jsonschema::draft202012::options()
                .offline()
                .should_validate_formats(true)
                .build(&schema)
                .map_err(|e| Error::Config(e.to_string()))?;
            let files: Vec<String> = serde_json::from_value(mapping["files"].clone())
                .map_err(|e| Error::Config(e.to_string()))?;
            if files.is_empty() {
                return Err(Error::Config("document mapping needs files".into()));
            }
            for file in &files {
                let p = contained(root, file)?;
                if !p.is_file()
                    || p.extension().and_then(|s| s.to_str()) != Some("json")
                    || protected.contains(&p)
                    || !assigned.insert(p.clone())
                {
                    return Err(Error::Config(format!(
                        "invalid or duplicate document assignment: {file}"
                    )));
                }
                if p == settings_path {
                    settings_schema = schema.clone();
                }
            }
            documents.insert(name.clone(), JsonDocumentConfig { schema, files });
        }
    }
    // This normalized internal shape keeps legacy consumers working. No union schema is stored.
    let schema = json!({"anyOf":branches,"x-project":{"media":media.values().next().map(|m|m.path.as_str()).unwrap_or("")}});
    let constraints = schemas.keys().map(|n| (n.clone(), Vec::new())).collect();
    Ok(Loaded {
        schema,
        data_schema: Some(json!(true)),
        data_schemas: schemas,
        constraints,
        ui,
        settings: Some(SettingsConfig {
            path: settings_file.into(),
            schema: settings_schema,
        }),
        documents,
        media_collections: media,
    })
}
