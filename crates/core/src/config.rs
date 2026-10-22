use crate::{Error, Result, contained, safe_name};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct Config {
    pub collections: BTreeMap<String, Collection>,
    pub media: MediaConfig,
    pub root: PathBuf,
    pub schema_value: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Collection {
    pub label: String,
    pub path: String,
    pub format: String,
    pub id: IdStrategy,
    pub display: String,
    #[serde(default, rename = "displayFields")]
    pub display_fields: Vec<String>,
    #[serde(default = "default_true", rename = "showInSidebar")]
    pub show_in_sidebar: bool,
    #[serde(default, rename = "displayFallback")]
    pub display_fallback: Vec<String>,
    #[serde(default)]
    pub search: Vec<String>,
    #[serde(default)]
    pub categories: Vec<CategoryLevel>,
    #[serde(default, rename = "categoryEntries")]
    pub category_entries: Vec<CategoryEntry>,
    #[serde(default, rename = "fieldOrder")]
    pub field_order: Vec<String>,
    #[serde(skip)]
    pub schema_value: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryLevel {
    pub field: String,
    pub label: String,
    #[serde(default, rename = "itemLabel")]
    pub item_label: Option<String>,
    #[serde(default)]
    pub display: Option<String>,
    #[serde(default)]
    pub collection: Option<String>,
    #[serde(default, rename = "parentField")]
    pub parent_field: Option<String>,
    #[serde(default, rename = "descriptionField")]
    pub description_field: Option<String>,
    #[serde(default, rename = "orderField")]
    pub order_field: Option<String>,
}

fn default_true() -> bool {
    true
}
fn schema_field<'a>(schema: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.')
        .try_fold(schema, |current, part| current.get("properties")?.get(part))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryEntry {
    pub path: Vec<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub order: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaConfig {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum IdStrategy {
    Filename,
    Frontmatter(String),
}
impl TryFrom<String> for IdStrategy {
    type Error = String;
    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        if value == "filename" {
            Ok(Self::Filename)
        } else if let Some(field) = value.strip_prefix("frontmatter.") {
            safe_name(field).map_err(|e| e.to_string())?;
            Ok(Self::Frontmatter(field.into()))
        } else {
            Err(format!("unsupported id strategy: {value}"))
        }
    }
}
impl From<IdStrategy> for String {
    fn from(value: IdStrategy) -> Self {
        match value {
            IdStrategy::Filename => "filename".into(),
            IdStrategy::Frontmatter(field) => format!("frontmatter.{field}"),
        }
    }
}

impl Config {
    pub fn load(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().canonicalize()?;
        let schema_value: Value =
            serde_json::from_slice(&fs::read(contained(&root, ".editor/schema.json")?)?)
                .map_err(|e| Error::Config(format!("project schema: {e}")))?;
        jsonschema::draft202012::options()
            .offline()
            .build(&schema_value)
            .map_err(|e| Error::Config(format!("invalid project schema: {e}")))?;
        let branches = schema_value
            .get("oneOf")
            .or_else(|| schema_value.get("anyOf"))
            .and_then(Value::as_array)
            .filter(|items| !items.is_empty())
            .ok_or_else(|| {
                Error::Config("project schema must have a nonempty top-level oneOf or anyOf".into())
            })?;
        let media = MediaConfig {
            path: schema_value
                .get("x-project")
                .and_then(|v| v.get("media"))
                .and_then(Value::as_str)
                .ok_or_else(|| Error::Config("x-project.media is required".into()))?
                .to_owned(),
        };
        let mut collections = BTreeMap::new();
        for branch in branches {
            let metadata = branch.get("x-collection").cloned().ok_or_else(|| {
                Error::Config("every top-level variant needs x-collection".into())
            })?;
            let name = metadata
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::Config("x-collection.name is required".into()))?
                .to_owned();
            safe_name(&name)?;
            if !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return Err(Error::Config(format!("invalid collection name: {name}")));
            }
            let mut collection: Collection = serde_json::from_value(metadata)
                .map_err(|e| Error::Config(format!("{name}: {e}")))?;
            for level in &collection.categories {
                if level.field.trim().is_empty() || level.label.trim().is_empty() {
                    return Err(Error::Config(format!(
                        "{name}: category field and label are required"
                    )));
                }
                for field in [
                    Some(level.field.as_str()),
                    if level.collection.is_none() {
                        level.display.as_deref()
                    } else {
                        None
                    },
                ]
                .into_iter()
                .flatten()
                {
                    let property = field
                        .split('.')
                        .try_fold(branch, |schema, part| schema.get("properties")?.get(part))
                        .ok_or_else(|| {
                            Error::Config(format!("{name}: unknown category field {field}"))
                        })?;
                    if property.get("type").and_then(Value::as_str) != Some("string") {
                        return Err(Error::Config(format!(
                            "{name}: category field {field} must be a string"
                        )));
                    }
                }
            }
            let mut seen_categories = std::collections::HashSet::new();
            for entry in &collection.category_entries {
                if entry.path.is_empty()
                    || entry.path.len() > collection.categories.len()
                    || entry.path.iter().any(|part| part.trim().is_empty())
                    || !seen_categories.insert(entry.path.clone())
                {
                    return Err(Error::Config(format!(
                        "{name}: invalid or duplicate category entry"
                    )));
                }
            }
            for field in &collection.field_order {
                if branch
                    .get("properties")
                    .and_then(|p| p.get(field))
                    .is_none()
                {
                    return Err(Error::Config(format!(
                        "{name}: unknown ordered field {field}"
                    )));
                }
            }
            for field in std::iter::once(&collection.display).chain(&collection.display_fallback) {
                if field.trim().is_empty() {
                    return Err(Error::Config(format!("{name}: empty display field")));
                }
            }
            for field in &collection.display_fields {
                if schema_field(branch, field).is_none() {
                    return Err(Error::Config(format!(
                        "{name}: unknown display field {field}"
                    )));
                }
            }
            if !["json", "markdown"].contains(&collection.format.as_str()) {
                return Err(Error::Config(format!("{name}: unsupported format")));
            }
            let mut effective = branch.clone();
            if let Some(defs) = schema_value.get("$defs") {
                let object = effective
                    .as_object_mut()
                    .ok_or_else(|| Error::Config("variants must be objects".into()))?;
                if !object.contains_key("$defs") {
                    object.insert("$defs".into(), defs.clone());
                }
            }
            if let Some(draft) = schema_value.get("$schema") {
                effective
                    .as_object_mut()
                    .unwrap()
                    .entry("$schema")
                    .or_insert_with(|| draft.clone());
            }
            jsonschema::draft202012::options()
                .offline()
                .build(&effective)
                .map_err(|e| Error::Config(format!("{name}: invalid variant schema: {e}")))?;
            collection.schema_value = effective;
            if collections.insert(name.clone(), collection).is_some() {
                return Err(Error::Config(format!("duplicate collection {name}")));
            }
        }
        let names = collections.keys().cloned().collect();
        let editor_dir = contained(&root, ".editor")?;
        let mut collection_dirs = Vec::new();
        for (name, collection) in &collections {
            let dir = contained(&root, &collection.path)?;
            if !dir.is_dir()
                || dir.starts_with(&editor_dir)
                || editor_dir.starts_with(&dir)
                || collection_dirs
                    .iter()
                    .any(|other: &PathBuf| dir.starts_with(other) || other.starts_with(&dir))
            {
                return Err(Error::Config(format!(
                    "{name}: invalid or overlapping data directory"
                )));
            }
            collection_dirs.push(dir);
            validate_relations(&collection.schema_value, &names)?;
            validate_editors(&collection.schema_value)?;
            for (index, level) in collection.categories.iter().enumerate() {
                if let Some(target_name) = &level.collection {
                    let target = collections.get(target_name).ok_or_else(|| {
                        Error::Config(format!("{name}: unknown category collection {target_name}"))
                    })?;
                    let source_field = schema_field(&collection.schema_value, &level.field)
                        .ok_or_else(|| {
                            Error::Config(format!("{name}: unknown category field {}", level.field))
                        })?;
                    if source_field
                        .pointer("/x-relation/collection")
                        .and_then(Value::as_str)
                        != Some(target_name)
                    {
                        return Err(Error::Config(format!(
                            "{name}: category field {} must relate to {target_name}",
                            level.field
                        )));
                    }
                    for field_name in [
                        level.display.as_deref(),
                        level.description_field.as_deref(),
                        level.order_field.as_deref(),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        if schema_field(&target.schema_value, field_name).is_none() {
                            return Err(Error::Config(format!(
                                "{name}: unknown category property {target_name}.{field_name}"
                            )));
                        }
                    }
                    if let Some(parent_field) = &level.parent_field {
                        if index == 0 {
                            return Err(Error::Config(format!(
                                "{name}: first category cannot have parentField"
                            )));
                        }
                        let parent_target = collection.categories[index - 1]
                            .collection
                            .as_deref()
                            .ok_or_else(|| {
                                Error::Config(format!(
                                    "{name}: parent category must have a collection"
                                ))
                            })?;
                        if schema_field(&target.schema_value, parent_field)
                            .and_then(|value| value.pointer("/x-relation/collection"))
                            .and_then(Value::as_str)
                            != Some(parent_target)
                        {
                            return Err(Error::Config(format!(
                                "{name}: {target_name}.{parent_field} must relate to {parent_target}"
                            )));
                        }
                    } else if index > 0 {
                        return Err(Error::Config(format!(
                            "{name}: linked child category needs parentField"
                        )));
                    }
                }
            }
        }
        let media_dir = contained(&root, &media.path)?;
        if !media_dir.is_dir()
            || media_dir.starts_with(&editor_dir)
            || editor_dir.starts_with(&media_dir)
            || collection_dirs
                .iter()
                .any(|dir| media_dir.starts_with(dir) || dir.starts_with(&media_dir))
        {
            return Err(Error::Config(
                "invalid or overlapping media directory".into(),
            ));
        }
        Ok(Self {
            collections,
            media,
            root,
            schema_value,
        })
    }
    pub fn collection(&self, name: &str) -> Result<&Collection> {
        self.collections
            .get(name)
            .ok_or_else(|| Error::NotFound(format!("collection {name}")))
    }
    pub fn collection_dir(&self, name: &str) -> Result<PathBuf> {
        contained(&self.root, &self.collection(name)?.path)
    }
    pub fn media_dir(&self) -> Result<PathBuf> {
        contained(&self.root, &self.media.path)
    }
}

fn validate_editors(schema: &Value) -> Result<()> {
    let is_string_field = || match schema.get("type") {
        Some(Value::String(kind)) => kind == "string",
        Some(Value::Array(types)) => {
            types.len() == 2
                && types.iter().any(|kind| kind.as_str() == Some("string"))
                && types.iter().any(|kind| kind.as_str() == Some("null"))
        }
        _ => false,
    };
    if let Some(editor) = schema.get("x-editor").and_then(Value::as_str) {
        if ![
            "hidden",
            "html",
            "markdown",
            "inline-markdown",
            "json",
            "media",
            "relation",
            "autocomplete",
            "autocomplete-select",
        ]
        .contains(&editor)
        {
            return Err(Error::Config(format!("unsupported field editor {editor}")));
        }
        if [
            "html",
            "markdown",
            "inline-markdown",
            "media",
            "relation",
            "autocomplete",
            "autocomplete-select",
        ]
        .contains(&editor)
            && !is_string_field()
        {
            return Err(Error::Config(format!("{editor} requires a string field")));
        }
    }
    if let Some(options) = schema.get("x-options") {
        if !options
            .as_array()
            .is_some_and(|items| items.iter().all(Value::is_string))
            || !is_string_field()
        {
            return Err(Error::Config(
                "x-options requires a string field and a string array".into(),
            ));
        }
    }
    if let Some(autocomplete) = schema.get("x-autocomplete") {
        let valid = autocomplete.is_boolean()
            || autocomplete.as_object().is_some_and(|options| {
                options.len() == 1
                    && options
                        .get("scope")
                        .and_then(Value::as_array)
                        .is_some_and(|scope| {
                            scope
                                .iter()
                                .all(|field| field.as_str().is_some_and(|name| !name.is_empty()))
                        })
            });
        if !valid || !is_string_field() {
            return Err(Error::Config(
                "x-autocomplete requires a string field and a boolean or {scope: [field names]}"
                    .into(),
            ));
        }
    }
    match schema {
        Value::Array(items) => {
            for item in items {
                validate_editors(item)?;
            }
        }
        Value::Object(entries) => {
            for item in entries.values() {
                validate_editors(item)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_relations(schema: &Value, names: &std::collections::BTreeSet<String>) -> Result<()> {
    if let Some(target) = schema
        .get("x-relation")
        .and_then(|v| v.get("collection"))
        .and_then(Value::as_str)
        && !names.contains(target)
    {
        return Err(Error::Config(format!("unknown relation target {target}")));
    }
    match schema {
        Value::Array(items) => {
            for item in items {
                validate_relations(item, names)?;
            }
        }
        Value::Object(entries) => {
            for item in entries.values() {
                validate_relations(item, names)?;
            }
        }
        _ => {}
    }
    Ok(())
}
