#[path = "file_changes.rs"]
mod file_changes;
#[path = "hierarchy.rs"]
mod hierarchy;
#[path = "listing.rs"]
mod listing;
#[path = "read_cache.rs"]
mod read_cache;
use crate::{Collection, Config, Error, IdStrategy, Result, safe_name};
pub use hierarchy::*;
pub use listing::*;
use pulldown_cmark::{Event, Parser, Tag};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

type FieldValueKey = (String, String, Vec<(String, String)>);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationIssue {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Document {
    pub id: String,
    pub value: Value,
    pub revision: String,
}

/// The saved record and the number of other files whose references were updated.
pub struct RenameResult {
    pub document: Document,
    pub updated_items: usize,
}

#[derive(Clone)]
enum ReferenceSource {
    Record(String),
    Document(String),
    Settings,
}

struct ReferenceUpdate {
    path: PathBuf,
    source: ReferenceSource,
    previous: Vec<u8>,
    value: Value,
    bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecordSummary {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CategoryNode {
    pub value: String,
    pub label: String,
    pub count: usize,
    pub description: Option<String>,
    pub order: Option<i64>,
    pub children: Vec<CategoryNode>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Reference {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document: Option<String>,
    pub collection: String,
    pub record_id: String,
    pub path: String,
}

#[derive(Clone)]
pub struct Editor {
    config: Arc<Config>,
    read_cache: Arc<read_cache::ReadCache>,
    validators: Arc<Validators>,
    locks: Arc<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>>,
    pub(crate) write_guard: Arc<Mutex<()>>,
    field_value_cache: Arc<Mutex<HashMap<FieldValueKey, (Instant, Vec<String>)>>>,
}

struct Validators {
    project: jsonschema::Validator,
    collections: HashMap<String, jsonschema::Validator>,
}

impl Editor {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        Self::new(Config::load(root)?)
    }

    pub fn new(config: Config) -> Result<Self> {
        let project = jsonschema::draft202012::options()
            .offline()
            .should_validate_formats(true)
            .build(&config.schema_value)
            .map_err(|e| Error::Config(e.to_string()))?;
        let mut collections = HashMap::new();
        for (name, collection) in &config.collections {
            let validator = jsonschema::draft202012::options()
                .offline()
                .should_validate_formats(true)
                .build(&collection.schema_value)
                .map_err(|e| Error::Config(format!("{name}: {e}")))?;
            collections.insert(name.clone(), validator);
        }
        Ok(Self {
            read_cache: Arc::new(read_cache::ReadCache::new(&config)),
            config: Arc::new(config),
            validators: Arc::new(Validators {
                project,
                collections,
            }),
            locks: Arc::new(Mutex::new(HashMap::new())),
            write_guard: Arc::new(Mutex::new(())),
            field_value_cache: Arc::new(Mutex::new(HashMap::new())),
        })
    }
    pub fn settings_schema(&self) -> Result<&Value> {
        Ok(&self
            .config
            .settings
            .as_ref()
            .ok_or_else(|| Error::Config("settings are not configured".into()))?
            .schema)
    }

    pub fn settings(&self) -> Result<Document> {
        let config = self
            .config
            .settings
            .as_ref()
            .ok_or_else(|| Error::Config("settings are not configured".into()))?;
        let path = crate::contained(&self.config.root, &config.path)?;
        reject_symlink(&self.config.root.join(&config.path))?;
        let bytes = fs::read(path)?;
        let value =
            serde_json::from_slice(&bytes).map_err(|e| Error::Data(format!("settings: {e}")))?;
        Ok(Document {
            id: "settings".into(),
            value,
            revision: hash(&bytes),
        })
    }

    pub fn validate_settings(&self, value: &Value) -> Result<Vec<ValidationIssue>> {
        if !value.is_object() {
            return Ok(vec![ValidationIssue {
                path: "".into(),
                message: "editor settings must be an object".into(),
            }]);
        }
        let validator = jsonschema::draft202012::options()
            .offline()
            .should_validate_formats(true)
            .build(self.settings_schema()?)
            .map_err(|e| Error::Config(format!("settings schema: {e}")))?;
        let mut issues: Vec<ValidationIssue> = validator
            .iter_errors(value)
            .map(|e| ValidationIssue {
                path: e.instance_path().to_string(),
                message: e.to_string(),
            })
            .collect();
        issues.extend(self.validate_reference_values(self.settings_schema()?, value)?);
        Ok(issues)
    }

    pub fn save_settings(&self, value: Value, expected: &str) -> Result<Document> {
        let _guard = self.write_guard.lock().unwrap();
        let issues = self.validate_settings(&value)?;
        if !issues.is_empty() {
            return Err(Error::Validation(issues));
        }
        if let Some(settings) = &self.config.settings {
            let path = crate::contained(&self.config.root, &settings.path)?;
            if let Some(name) = self.mapped_document_name(&path) {
                let issues = self.validate_document(name, &value)?;
                if !issues.is_empty() {
                    return Err(Error::Validation(issues));
                }
            }
        }
        let current = self.settings()?;
        if current.revision != expected {
            return Err(Error::Conflict("settings changed on disk".into()));
        }
        let config = self.config.settings.as_ref().unwrap();
        let path = crate::contained(&self.config.root, &config.path)?;
        let mut bytes =
            serde_json::to_vec_pretty(&value).map_err(|e| Error::Data(e.to_string()))?;
        bytes.push(b'\n');
        self.write_file(&path, &bytes)?;
        Ok(Document {
            id: "settings".into(),
            value,
            revision: hash(&bytes),
        })
    }

    pub fn for_media(&self, name: &str) -> Result<Self> {
        let media = self
            .config
            .media_collections
            .get(name)
            .ok_or_else(|| Error::NotFound(format!("media collection {name}")))?;
        let mut config = (*self.config).clone();
        config.media.path = media.path.clone();
        let mut editor = self.clone();
        editor.config = Arc::new(config);
        Ok(editor)
    }
    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn list(
        &self,
        collection: &str,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<RecordSummary>> {
        self.search_with_category(collection, query, None, None, &[], offset, limit)
    }

    pub fn list_in_category(
        &self,
        collection: &str,
        query: &str,
        category: &[String],
        offset: usize,
        limit: usize,
    ) -> Result<Vec<RecordSummary>> {
        self.search_with_category(collection, query, None, None, category, offset, limit)
    }

    pub fn categories(&self, collection: &str) -> Result<Vec<CategoryNode>> {
        let c = self.config.collection(collection)?;
        let mut roots = Vec::new();
        if c.categories.is_empty() {
            return Ok(roots);
        }
        for (depth, level) in c.categories.iter().enumerate() {
            let Some(target_name) = &level.collection else {
                continue;
            };
            let target = self.config.collection(target_name)?;
            for file_path in self.files(target_name)? {
                let value = parse(target, &fs::read(&file_path)?)?;
                let id = record_id(target, &file_path, &value)?;
                let display = level.display.as_deref().unwrap_or(&target.display);
                let label = display_label(&value, target, display, &id);
                let nodes = if depth == 0 {
                    &mut roots
                } else {
                    let parent_field = level.parent_field.as_deref().unwrap();
                    let parent_id = field(&value, parent_field)
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            Error::Data(format!("{target_name}/{id}: missing {parent_field}"))
                        })?;
                    &mut find_category_at_depth(&mut roots, depth - 1, parent_id)
                        .ok_or_else(|| {
                            Error::Data(format!(
                                "{target_name}/{id}: unknown category parent {parent_id}"
                            ))
                        })?
                        .children
                };
                let node = category_node(nodes, &id, &label);
                node.description = level
                    .description_field
                    .as_deref()
                    .and_then(|name| field(&value, name))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                node.order = level
                    .order_field
                    .as_deref()
                    .and_then(|name| field(&value, name))
                    .and_then(Value::as_i64);
            }
        }
        for entry in &c.category_entries {
            let mut nodes = &mut roots;
            for (index, part) in entry.path.iter().enumerate() {
                let node = category_node(nodes, part, part);
                if index + 1 == entry.path.len() {
                    if let Some(label) = &entry.label {
                        node.label = label.clone();
                    }
                    node.description = entry.description.clone();
                    node.order = entry.order;
                }
                nodes = &mut node.children;
            }
        }
        for path in self.files(collection)? {
            let value = self.view_value(collection, &parse(c, &fs::read(path)?)?)?;
            let mut nodes = &mut roots;
            for level in &c.categories {
                let Some(key) = field(&value, &level.field)
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                else {
                    break;
                };
                let label = level
                    .display
                    .as_deref()
                    .and_then(|name| field(&value, name))
                    .and_then(Value::as_str)
                    .unwrap_or(key);
                let node = category_node(nodes, key, label);
                node.count += 1;
                nodes = &mut node.children;
            }
        }
        sort_categories(&mut roots);
        Ok(roots)
    }

    pub fn field_values(
        &self,
        collection: &str,
        field_name: &str,
        scope: &BTreeMap<String, String>,
        query: &str,
        limit: usize,
    ) -> Result<Vec<String>> {
        let c = self.config.collection(collection)?;
        for name in std::iter::once(field_name).chain(scope.keys().map(String::as_str)) {
            let mut property = &c.schema_value;
            for part in name.split('.') {
                property = resolve_ref(&c.schema_value, property, 0)?
                    .get("properties")
                    .and_then(|properties| properties.get(part))
                    .ok_or_else(|| Error::Data(format!("unknown field {name}")))?;
            }
            let property = resolve_ref(&c.schema_value, property, 0)?;
            if property.get("type").and_then(Value::as_str) != Some("string") {
                return Err(Error::Data(format!("field {name} must be a string")));
            }
        }
        let key = (
            collection.to_owned(),
            field_name.to_owned(),
            scope
                .iter()
                .map(|(field, value)| (field.clone(), value.clone()))
                .collect(),
        );
        let cached = self.field_value_cache.lock().unwrap().get(&key).cloned();
        let values = if let Some((created, values)) = cached {
            if created.elapsed() < Duration::from_secs(60) {
                values
            } else {
                self.collect_field_values(c, collection, field_name, scope, &key)?
            }
        } else {
            self.collect_field_values(c, collection, field_name, scope, &key)?
        };
        let needle = query.to_lowercase();
        let mut matches: Vec<String> = values
            .into_iter()
            .filter(|value| value.to_lowercase().contains(&needle))
            .collect();
        matches.sort_by(|a, b| {
            let a_prefix = a.to_lowercase().starts_with(&needle);
            let b_prefix = b.to_lowercase().starts_with(&needle);
            b_prefix.cmp(&a_prefix).then(crate::alphabetic_cmp(a, b))
        });
        matches.truncate(limit.min(100));
        Ok(matches)
    }

    fn collect_field_values(
        &self,
        c: &Collection,
        collection: &str,
        field_name: &str,
        scope: &BTreeMap<String, String>,
        key: &FieldValueKey,
    ) -> Result<Vec<String>> {
        let mut values = BTreeSet::new();
        for path in self.files(collection)? {
            let value = self.view_value(collection, &parse(c, &fs::read(path)?)?)?;
            if !scope.iter().all(|(name, expected)| {
                field(&value, name).and_then(Value::as_str) == Some(expected)
            }) {
                continue;
            }
            if let Some(found) = field(&value, field_name)
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
            {
                values.insert(found.to_owned());
            }
        }
        let values: Vec<String> = values.into_iter().collect();
        self.field_value_cache
            .lock()
            .unwrap()
            .insert(key.clone(), (Instant::now(), values.clone()));
        Ok(values)
    }

    fn clear_field_values(&self, _collection: &str) {
        self.field_value_cache.lock().unwrap().clear();
    }

    pub fn search_relation(
        &self,
        collection: &str,
        query: &str,
        display: Option<&str>,
        search: Option<&[String]>,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<RecordSummary>> {
        self.search_with_category(collection, query, display, search, &[], offset, limit)
    }

    fn search_with_category(
        &self,
        collection: &str,
        query: &str,
        display: Option<&str>,
        search: Option<&[String]>,
        category: &[String],
        offset: usize,
        limit: usize,
    ) -> Result<Vec<RecordSummary>> {
        let c = self.config.collection(collection)?;
        let view = if query.is_empty() { "list" } else { "search" };
        let page = self.query_collection(
            collection,
            &ListRequest {
                query: query.into(),
                category: category.to_vec(),
                search_fields: search
                    .filter(|fields| *fields != c.search.as_slice())
                    .map(<[String]>::to_vec),
                display_field: display
                    .filter(|field| *field != c.display)
                    .map(str::to_owned),
                label_view: if c.ui.pointer(&format!("/labels/{view}")).is_some() && view != "list"
                {
                    view.into()
                } else {
                    String::new()
                },
                sort: if c.ui.pointer("/ordering/by").and_then(Value::as_str) == Some("manual") {
                    "_manual"
                } else {
                    "_label"
                }
                .into(),
                offset,
                limit: limit.min(100).max(1),
                ..Default::default()
            },
        )?;
        Ok(page
            .rows
            .into_iter()
            .take(limit)
            .map(|row| RecordSummary {
                id: row.id,
                label: row.label,
            })
            .collect())
    }

    pub fn get(&self, collection: &str, id: &str) -> Result<Document> {
        let path = self.find_path(collection, id)?;
        let doc = self.cached_document(collection, &path)?;
        if doc.id != id {
            return Err(Error::Conflict(
                "Record ID changed during lookup; retry the read".into(),
            ));
        }
        Ok((*doc).clone())
    }

    pub fn validate_new_id(&self, collection: &str, id: &str) -> Result<Vec<ValidationIssue>> {
        let c = self.config.collection(collection)?;
        if let Some(pattern) = &c.id_pattern {
            let schema = serde_json::json!({"type":"string","pattern":pattern});
            let v = jsonschema::draft202012::options()
                .offline()
                .build(&schema)
                .map_err(|e| Error::Config(e.to_string()))?;
            if !v.is_valid(&serde_json::json!(id)) {
                return Ok(vec![ValidationIssue {
                    path: "/id".into(),
                    message: "ID does not match the configured pattern".into(),
                }]);
            }
        }
        if safe_name(id).is_err() {
            return Ok(vec![ValidationIssue {
                path: "/id".into(),
                message: "ID must be 1–255 bytes and cannot contain slashes, backslashes, or control characters".into(),
            }]);
        }
        match self.find_path(collection, id) {
            Ok(_) => Ok(vec![ValidationIssue {
                path: "/id".into(),
                message: "ID already exists in this collection".into(),
            }]),
            Err(Error::NotFound(_)) => Ok(Vec::new()),
            Err(error) => Err(error),
        }
    }

    pub fn generate_id(&self, collection: &str, value: &Value) -> Result<String> {
        let c = self.config.collection(collection)?;
        let label = display_label(value, c, &c.display, "record");
        let mut stem = String::new();
        for ch in deunicode::deunicode(&label).chars() {
            if ch.is_ascii_alphanumeric() {
                stem.push(ch.to_ascii_lowercase());
            } else if !stem.is_empty() && !stem.ends_with('-') {
                stem.push('-');
            }
            if stem.len() >= 80 {
                break;
            }
        }
        let stem = stem.trim_end_matches('-');
        let stem = if stem.is_empty() { "record" } else { stem };
        for number in 1..=10000 {
            let candidate = if number == 1 {
                stem.to_string()
            } else {
                format!("{stem}-{number}")
            };
            if self.validate_new_id(collection, &candidate)?.is_empty() {
                return Ok(candidate);
            }
        }
        Err(Error::Data("could not find a unique ID".into()))
    }

    pub fn resolve_relation(
        &self,
        collection: &str,
        id: &str,
        display: Option<&str>,
    ) -> Result<RecordSummary> {
        let c = self.config.collection(collection)?;
        let doc = self.get(collection, id)?;
        let label = display_label(&doc.value, c, display.unwrap_or(&c.display), id);
        Ok(RecordSummary {
            id: id.into(),
            label,
        })
    }

    pub fn references_to(&self, collection: &str, id: &str) -> Result<Vec<Reference>> {
        self.config.collection(collection)?;
        safe_name(id)?;
        let mut sources = BTreeMap::new();
        for name in self.config.collections.keys() {
            for path in self.files(name)? {
                sources.insert(path, ReferenceSource::Record(name.clone()));
            }
        }
        for (name, mapping) in &self.config.documents {
            for file in &mapping.files {
                crate::contained(&self.config.root, file)?;
                sources.insert(
                    self.config.root.join(file),
                    ReferenceSource::Document(name.clone()),
                );
            }
        }
        if let Some(settings) = &self.config.settings {
            if !self
                .config
                .documents
                .values()
                .any(|mapping| mapping.files.contains(&settings.path))
            {
                crate::contained(&self.config.root, &settings.path)?;
                sources.insert(
                    self.config.root.join(&settings.path),
                    ReferenceSource::Settings,
                );
            }
        }
        let mut stamps = BTreeMap::new();
        for path in sources.keys() {
            stamps.insert(path.clone(), read_cache::stamp(path)?);
        }
        let mut cache = self.read_cache.references.lock().unwrap();
        let paths: BTreeSet<_> = cache.stamps.keys().chain(stamps.keys()).cloned().collect();
        let changed_paths: BTreeSet<_> = paths
            .into_iter()
            .filter(|path| cache.stamps.get(path) != stamps.get(path))
            .collect();
        let mut changed_ids = BTreeSet::new();
        for path in &changed_paths {
            if let Some(old_id) = cache.file_ids.get(path) {
                changed_ids.insert(old_id.clone());
            }
            if let Some(ReferenceSource::Record(name)) = sources.get(path) {
                changed_ids.insert((name.clone(), self.cached_document(name, path)?.id.clone()));
            }
        }
        let mut affected = changed_paths;
        for (path, dependencies) in &cache.dependencies {
            if !dependencies.is_disjoint(&changed_ids) {
                affected.insert(path.clone());
            }
        }
        for path in affected {
            if let Some(source) = sources.get(&path) {
                let (references, record_id, dependencies) =
                    self.references_from_source(&path, source)?;
                cache.replace(
                    path.clone(),
                    stamps[&path].clone(),
                    references,
                    record_id,
                    dependencies,
                );
            } else {
                cache.remove(&path);
            }
        }
        let mut references = cache
            .by_target
            .get(&(collection.to_owned(), id.to_owned()))
            .map(|files| {
                files
                    .values()
                    .flat_map(|refs| refs.iter().cloned())
                    .collect()
            })
            .unwrap_or_default();
        unique_references(&mut references);
        Ok(references)
    }

    fn references_from_source(
        &self,
        path: &Path,
        source: &ReferenceSource,
    ) -> Result<(
        read_cache::ReferenceIndex,
        Option<(String, String)>,
        BTreeSet<(String, String)>,
    )> {
        let (schema, value, document, collection, record_id, dependencies) = match source {
            ReferenceSource::Record(name) => {
                let c = self.config.collection(name)?;
                let doc = self.cached_document(name, path)?;
                let dependencies = listing::derived_relation_ids(c, &doc.value);
                (
                    &c.schema_value,
                    self.view_value(name, &doc.value)?,
                    None,
                    name.clone(),
                    doc.id.clone(),
                    dependencies,
                )
            }
            ReferenceSource::Document(name) => {
                let relative = path
                    .strip_prefix(&self.config.root)
                    .map_err(|e| Error::Data(e.to_string()))?
                    .to_string_lossy()
                    .to_string();
                let mapping = self
                    .config
                    .documents
                    .get(name)
                    .ok_or_else(|| Error::Config(format!("unknown document {name}")))?;
                (
                    &mapping.schema,
                    self.json_document(name, &relative)?.value,
                    Some(name.clone()),
                    String::new(),
                    relative,
                    BTreeSet::new(),
                )
            }
            ReferenceSource::Settings => {
                let relative = self
                    .config
                    .settings
                    .as_ref()
                    .ok_or_else(|| Error::Config("settings are not configured".into()))?
                    .path
                    .clone();
                (
                    self.settings_schema()?,
                    self.settings()?.value,
                    Some(String::new()),
                    String::new(),
                    relative,
                    BTreeSet::new(),
                )
            }
        };
        let mut index: read_cache::ReferenceIndex = HashMap::new();
        visit_relations(schema, schema, &value, "", 0, &mut |target, id, _, path| {
            index
                .entry((target.to_owned(), id.to_owned()))
                .or_default()
                .push(Reference {
                    document: document.clone(),
                    collection: collection.clone(),
                    record_id: record_id.clone(),
                    path: path.into(),
                });
            Ok(())
        })?;
        visit_markdown_links(&value, "", &mut |destination, path| {
            if let Some((target, id)) = record_link_target(destination) {
                index.entry((target, id)).or_default().push(Reference {
                    document: document.clone(),
                    collection: collection.clone(),
                    record_id: record_id.clone(),
                    path: path.into(),
                });
            }
        });
        let source_id =
            matches!(source, ReferenceSource::Record(_)).then_some((collection, record_id));
        Ok((index, source_id, dependencies))
    }

    pub fn media_references(&self, path: &str) -> Result<Vec<Reference>> {
        if !self.media_path(path, true)?.is_file() {
            return Err(Error::InvalidPath(path.into()));
        }
        self.media_references_under(path)
    }

    pub(crate) fn media_references_under(&self, path: &str) -> Result<Vec<Reference>> {
        self.media_path(path, true)?;
        let mut references = Vec::new();
        let child_prefix = format!("{path}/");
        for (source_name, source) in &self.config.collections {
            for file in self.files(source_name)? {
                let value = parse(source, &fs::read(&file)?)?;
                let source_id = record_id(source, &file, &value)?;
                visit_media_fields(
                    &source.schema_value,
                    &source.schema_value,
                    &value,
                    "",
                    0,
                    &mut |media, linked_path, field_path| {
                        if media != self.media_collection_name() {
                            return Ok(());
                        }
                        if linked_path == path || linked_path.starts_with(&child_prefix) {
                            references.push(Reference {
                                document: None,
                                collection: source_name.clone(),
                                record_id: source_id.clone(),
                                path: field_path.into(),
                            });
                        }
                        Ok(())
                    },
                )?;
            }
        }
        unique_references(&mut references);
        Ok(references)
    }

    pub(crate) fn validate_reference_values(
        &self,
        schema: &Value,
        value: &Value,
    ) -> Result<Vec<ValidationIssue>> {
        let mut issues = Vec::new();
        visit_relations(schema, schema, value, "", 0, &mut |target, id, _, path| {
            if safe_name(id).is_err() {
                issues.push(ValidationIssue {
                    path: path.into(),
                    message: format!("invalid {target} record ID: {id}"),
                });
            } else if self.config.collection(target).is_err() {
                issues.push(ValidationIssue {
                    path: path.into(),
                    message: format!("unknown reference collection: {target}"),
                });
            } else {
                match self.get(target, id) {
                    Ok(_) => {}
                    Err(Error::NotFound(_)) => issues.push(ValidationIssue {
                        path: path.into(),
                        message: format!("unresolved {target} reference: {id}"),
                    }),
                    Err(error) => return Err(error),
                }
            }
            Ok(())
        })?;
        Ok(issues)
    }

    pub fn validate(&self, collection: &str, value: &Value) -> Result<Vec<ValidationIssue>> {
        let c = self.config.collection(collection)?;
        let validator = self
            .validators
            .collections
            .get(collection)
            .ok_or_else(|| Error::Config("collection validator missing".into()))?;
        let mut issues: Vec<ValidationIssue> = validator
            .iter_errors(value)
            .map(|e| ValidationIssue {
                path: e.instance_path().to_string(),
                message: e.to_string(),
            })
            .collect();
        if issues.is_empty() {
            issues.extend(
                self.validators
                    .project
                    .iter_errors(value)
                    .map(|e| ValidationIssue {
                        path: e.instance_path().to_string(),
                        message: e.to_string(),
                    }),
            );
        }
        issues.extend(self.validate_reference_values(&c.schema_value, value)?);
        for (index, level) in c.constraints.iter().enumerate() {
            let (Some(target), Some(parent_field)) = (&level.collection, &level.parent_field)
            else {
                continue;
            };
            let Some(child_id) = field(value, &level.field).and_then(Value::as_str) else {
                continue;
            };
            let Some(parent_id) =
                field(value, &c.constraints[index - 1].field).and_then(Value::as_str)
            else {
                continue;
            };
            if let Ok(child) = self.get(target, child_id)
                && field(&child.value, parent_field).and_then(Value::as_str) != Some(parent_id)
            {
                issues.push(ValidationIssue {
                    path: format!("/{}", pointer_escape(&level.field)),
                    message: format!(
                        "{target}/{child_id} belongs to another {}",
                        c.constraints[index - 1].label
                    ),
                });
            }
        }
        validate_inline_fields(&c.schema_value, &c.schema_value, value, "", 0, &mut issues)?;
        Ok(issues)
    }

    pub fn save(
        &self,
        collection: &str,
        id: &str,
        value: Value,
        expected: Option<&str>,
    ) -> Result<Document> {
        let _write_guard = self.write_guard.lock().unwrap();
        self.save_unlocked(collection, id, value, expected)
    }

    pub(crate) fn save_unlocked(
        &self,
        collection: &str,
        id: &str,
        value: Value,
        expected: Option<&str>,
    ) -> Result<Document> {
        self.reconcile_ids_for_write(collection)?;
        self.require_operation(
            collection,
            if expected.is_some() {
                "update"
            } else {
                "create"
            },
        )?;
        let previous = if expected.is_some() {
            Some(self.get(collection, id)?.value)
        } else {
            None
        };
        let value = self.prepare_save(collection, id, value, previous.as_ref())?;
        if expected.is_none() {
            let id_issues = self.validate_new_id(collection, id)?;
            if !id_issues.is_empty() {
                return Err(Error::Validation(id_issues));
            }
        } else {
            safe_name(id)?;
        }
        let c = self.config.collection(collection)?;
        let issues = self.validate(collection, &value)?;
        if !issues.is_empty() {
            return Err(Error::Validation(issues));
        }
        if expected.is_some() {
            for parent_field in self
                .config
                .collections
                .values()
                .flat_map(|source| source.constraints.iter())
                .filter(|level| level.collection.as_deref() == Some(collection))
                .filter_map(|level| level.parent_field.as_deref())
            {
                let previous = self.get(collection, id)?;
                if field(&previous.value, parent_field) != field(&value, parent_field)
                    && !self.references_to(collection, id)?.is_empty()
                {
                    return Err(Error::Conflict(format!(
                        "change references to {collection}/{id} before changing {parent_field}"
                    )));
                }
            }
        }
        if let IdStrategy::Frontmatter(field_name) = &c.id
            && value.get(field_name).and_then(Value::as_str) != Some(id)
        {
            return Err(Error::Validation(vec![ValidationIssue {
                path: format!("/{field_name}"),
                message: "identifier must match record URL".into(),
            }]));
        }
        let dir = self.config.collection_dir(collection)?;
        let path = if expected.is_some() {
            self.find_path(collection, id)?
        } else {
            match self.find_path(collection, id) {
                Ok(_) => return Err(Error::Conflict("record already exists".into())),
                Err(Error::NotFound(_)) => {}
                Err(error) => return Err(error),
            }
            dir.join(format!("{id}.{}", extension(c)))
        };
        let file_lock = self.lock(&path);
        let _guard = file_lock.lock().unwrap();
        let current = if path.exists() {
            reject_symlink(&path)?;
            Some(fs::read(&path)?)
        } else {
            None
        };
        match (expected, &current) {
            (None, Some(_)) => return Err(Error::Conflict("record already exists".into())),
            (Some(_), None) => return Err(Error::Conflict("record was removed".into())),
            (Some(rev), Some(bytes)) if hash(bytes) != rev => {
                return Err(Error::Conflict("record changed on disk".into()));
            }
            _ => {}
        }
        if let Some(name) = self.mapped_document_name(&path) {
            let issues = self.validate_document(name, &value)?;
            if !issues.is_empty() {
                return Err(Error::Validation(issues));
            }
        }
        if self.config.settings.as_ref().is_some_and(|s| {
            crate::contained(&self.config.root, &s.path).ok().as_deref() == Some(path.as_path())
        }) {
            let issues = self.validate_settings(&value)?;
            if !issues.is_empty() {
                return Err(Error::Validation(issues));
            }
        }
        let bytes = serialize(c, &value)?;
        self.write_file(&path, &bytes)?;
        self.clear_field_values(collection);
        Ok(Document {
            id: id.into(),
            value,
            revision: hash(&bytes),
        })
    }

    pub fn rename(
        &self,
        collection: &str,
        old_id: &str,
        new_id: &str,
        value: Value,
        expected: &str,
    ) -> Result<Document> {
        Ok(self
            .rename_with_references(collection, old_id, new_id, value, expected)?
            .document)
    }

    pub fn rename_with_references(
        &self,
        collection: &str,
        old_id: &str,
        new_id: &str,
        mut value: Value,
        expected: &str,
    ) -> Result<RenameResult> {
        if old_id == new_id {
            return Ok(RenameResult {
                document: self.save(collection, old_id, value, Some(expected))?,
                updated_items: 0,
            });
        }
        let _write_guard = self.write_guard.lock().unwrap();
        self.reconcile_ids_for_write(collection)?;
        self.require_operation(collection, "rename")?;
        self.require_operation(collection, "update")?;
        value = self.prepare_save(
            collection,
            new_id,
            value,
            Some(&self.get(collection, old_id)?.value),
        )?;
        let id_issues = self.validate_new_id(collection, new_id)?;
        if !id_issues.is_empty() {
            return Err(Error::Validation(id_issues));
        }
        let collection_config = self.config.collection(collection)?;
        let old_path = self.find_path(collection, old_id)?;
        self.guard_document_location(&old_path)?;
        let old_bytes = fs::read(&old_path)?;
        if hash(&old_bytes) != expected {
            return Err(Error::Conflict("record changed on disk".into()));
        }
        let references = self.references_to(collection, old_id)?;
        if let IdStrategy::Frontmatter(field_name) = &collection_config.id {
            let object = value.as_object_mut().ok_or_else(|| {
                Error::Validation(vec![ValidationIssue {
                    path: "".into(),
                    message: "record must be an object".into(),
                }])
            })?;
            object.insert(field_name.clone(), Value::String(new_id.into()));
        }
        rewrite_references(
            &collection_config.schema_value,
            &mut value,
            collection,
            old_id,
            new_id,
        )?;
        let new_path = self
            .config
            .collection_dir(collection)?
            .join(format!("{new_id}.{}", extension(collection_config)));
        if new_path != old_path {
            reject_symlink(&new_path)?;
            if new_path.exists() {
                return Err(Error::Conflict("target filename already exists".into()));
            }
        }
        // Resolve every incoming source before touching disk. A single source may hold
        // several references, and a mapped JSON document may also be a collection record.
        let mut sources = BTreeMap::<PathBuf, ReferenceSource>::new();
        for reference in references {
            let (path, source) = if let Some(document) = reference.document {
                let path = crate::contained(&self.config.root, &reference.record_id)?;
                let source = if document.is_empty() {
                    ReferenceSource::Settings
                } else {
                    ReferenceSource::Document(document)
                };
                (path, source)
            } else {
                (
                    self.find_path(&reference.collection, &reference.record_id)?,
                    ReferenceSource::Record(reference.collection),
                )
            };
            if path != old_path {
                sources
                    .entry(path)
                    .and_modify(|existing| {
                        if matches!(source, ReferenceSource::Record(_)) {
                            *existing = source.clone();
                        }
                    })
                    .or_insert(source);
            }
        }
        let mut updates = Vec::new();
        for (path, source) in sources {
            reject_symlink(&path)?;
            if let ReferenceSource::Record(name) = &source {
                self.require_operation(name, "update")?;
            } else {
                for (name, config) in &self.config.collections {
                    if config.format == "json"
                        && path.parent() == Some(self.config.collection_dir(name)?.as_path())
                    {
                        self.require_operation(name, "update")?;
                    }
                }
            }
            let previous = fs::read(&path)?;
            let mut changed = match &source {
                ReferenceSource::Record(name) => parse(self.config.collection(name)?, &previous)?,
                ReferenceSource::Document(_) | ReferenceSource::Settings => {
                    serde_json::from_slice(&previous)
                        .map_err(|e| Error::Data(format!("{}: {e}", path.display())))?
                }
            };
            let schema = match &source {
                ReferenceSource::Record(name) => &self.config.collection(name)?.schema_value,
                ReferenceSource::Document(name) => self.document_schema(name)?,
                ReferenceSource::Settings => self.settings_schema()?,
            };
            let mut rewritten =
                rewrite_references(schema, &mut changed, collection, old_id, new_id)?;
            if let Some(name) = self.mapped_document_name(&path) {
                rewritten |= rewrite_references(
                    self.document_schema(name)?,
                    &mut changed,
                    collection,
                    old_id,
                    new_id,
                )?;
            }
            if self.config.settings.as_ref().is_some_and(|settings| {
                crate::contained(&self.config.root, &settings.path)
                    .ok()
                    .as_deref()
                    == Some(path.as_path())
            }) {
                rewritten |= rewrite_references(
                    self.settings_schema()?,
                    &mut changed,
                    collection,
                    old_id,
                    new_id,
                )?;
            }
            if !rewritten {
                return Err(Error::Conflict(format!(
                    "could not update a reference in {}",
                    path.display()
                )));
            }
            let bytes = match &source {
                ReferenceSource::Record(name) => {
                    serialize(self.config.collection(name)?, &changed)?
                }
                _ => {
                    let mut bytes = serde_json::to_vec_pretty(&changed)
                        .map_err(|e| Error::Data(e.to_string()))?;
                    bytes.push(b'\n');
                    bytes
                }
            };
            updates.push(ReferenceUpdate {
                path,
                source,
                previous,
                value: changed,
                bytes,
            });
        }
        let bytes = serialize(collection_config, &value)?;
        // Make the new ID visible while validating updated relations. All writes use
        // the same project write lock; on an error, restore each changed file.
        if new_path == old_path {
            self.write_file(&old_path, &bytes)?;
        } else {
            self.create_file(&new_path, &bytes)?;
        }
        let mut written = 0;
        let result = (|| -> Result<()> {
            let issues = self.validate(collection, &value)?;
            if !issues.is_empty() {
                return Err(Error::Validation(issues));
            }
            for update in &updates {
                let issues = match &update.source {
                    ReferenceSource::Record(name) => self.validate(name, &update.value)?,
                    ReferenceSource::Document(name) => {
                        self.validate_document(name, &update.value)?
                    }
                    ReferenceSource::Settings => self.validate_settings(&update.value)?,
                };
                if !issues.is_empty() {
                    return Err(Error::Validation(issues));
                }
                if let Some(name) = self.mapped_document_name(&update.path) {
                    let issues = self.validate_document(name, &update.value)?;
                    if !issues.is_empty() {
                        return Err(Error::Validation(issues));
                    }
                }
                if !matches!(update.source, ReferenceSource::Record(_)) {
                    for (name, config) in &self.config.collections {
                        if config.format == "json"
                            && update.path.parent()
                                == Some(self.config.collection_dir(name)?.as_path())
                        {
                            let issues = self.validate(name, &update.value)?;
                            if !issues.is_empty() {
                                return Err(Error::Validation(issues));
                            }
                        }
                    }
                }
                if self.config.settings.as_ref().is_some_and(|settings| {
                    crate::contained(&self.config.root, &settings.path)
                        .ok()
                        .as_deref()
                        == Some(update.path.as_path())
                }) {
                    let issues = self.validate_settings(&update.value)?;
                    if !issues.is_empty() {
                        return Err(Error::Validation(issues));
                    }
                }
            }
            for update in &updates {
                if fs::read(&update.path)? != update.previous {
                    return Err(Error::Conflict(format!(
                        "{} changed on disk",
                        update.path.display()
                    )));
                }
                self.write_file(&update.path, &update.bytes)?;
                written += 1;
            }
            if new_path != old_path {
                if fs::read(&old_path)? != old_bytes {
                    return Err(Error::Conflict("record changed on disk".into()));
                }
                self.remove_file(&old_path)?;
                fs::File::open(self.config.collection_dir(collection)?)?.sync_all()?;
            }
            if !self.references_to(collection, old_id)?.is_empty() {
                return Err(Error::Conflict(
                    "some references still use the old ID".into(),
                ));
            }
            Ok(())
        })();
        if let Err(error) = result {
            let rollback = (|| -> Result<()> {
                for update in updates.iter().take(written).rev() {
                    self.write_file(&update.path, &update.previous)?;
                }
                if new_path == old_path {
                    self.write_file(&old_path, &old_bytes)?;
                } else {
                    if !old_path.exists() {
                        self.create_file(&old_path, &old_bytes)?;
                    }
                    self.remove_file(&new_path)?;
                }
                Ok(())
            })();
            if let Err(rollback_error) = rollback {
                return Err(Error::Data(format!(
                    "rename failed: {error}; rollback failed: {rollback_error}"
                )));
            }
            return Err(error);
        }
        self.clear_field_values(collection);
        Ok(RenameResult {
            document: Document {
                id: new_id.into(),
                value,
                revision: hash(&bytes),
            },
            updated_items: updates.len(),
        })
    }

    pub fn delete(&self, collection: &str, id: &str, expected: &str) -> Result<()> {
        self.require_operation(collection, "delete")?;
        let _write_guard = self.write_guard.lock().unwrap();
        let references = self.references_to(collection, id)?;
        if let Some(reference) = references.first() {
            return Err(Error::Conflict(format!(
                "record is referenced by {}/{} at {}",
                reference.collection, reference.record_id, reference.path
            )));
        }
        let path = self.find_path(collection, id)?;
        let file_lock = self.lock(&path);
        let _guard = file_lock.lock().unwrap();
        reject_symlink(&path)?;
        if hash(&fs::read(&path)?) != expected {
            return Err(Error::Conflict("record changed on disk".into()));
        }
        self.guard_document_location(&path)?;
        self.remove_file(&path)?;
        self.clear_field_values(collection);
        Ok(())
    }

    fn lock(&self, path: &Path) -> Arc<Mutex<()>> {
        self.locks
            .lock()
            .unwrap()
            .entry(path.to_owned())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    fn files(&self, collection: &str) -> Result<Vec<PathBuf>> {
        let c = self.config.collection(collection)?;
        let dir = self.config.collection_dir(collection)?;
        let mut files = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some(extension(c)) {
                reject_symlink(&path)?;
                if path.is_file() {
                    files.push(path);
                }
            }
        }
        Ok(files)
    }

    fn find_path(&self, collection: &str, id: &str) -> Result<PathBuf> {
        safe_name(id)?;
        let c = self.config.collection(collection)?;
        if matches!(c.id, IdStrategy::Filename) {
            let path = self
                .config
                .collection_dir(collection)?
                .join(format!("{id}.{}", extension(c)));
            reject_symlink(&path)?;
            if !path.is_file() {
                return Err(Error::NotFound(format!("record {id}")));
            }
            return Ok(path);
        }
        self.indexed_path(collection, id)
    }
}

fn category_node<'a>(
    nodes: &'a mut Vec<CategoryNode>,
    value: &str,
    label: &str,
) -> &'a mut CategoryNode {
    if let Some(index) = nodes.iter().position(|node| node.value == value) {
        return &mut nodes[index];
    }
    nodes.push(CategoryNode {
        value: value.into(),
        label: label.into(),
        count: 0,
        description: None,
        order: None,
        children: Vec::new(),
    });
    nodes.last_mut().unwrap()
}

fn find_category_at_depth<'a>(
    nodes: &'a mut [CategoryNode],
    depth: usize,
    id: &str,
) -> Option<&'a mut CategoryNode> {
    if depth == 0 {
        return nodes.iter_mut().find(|node| node.value == id);
    }
    for node in nodes {
        if let Some(found) = find_category_at_depth(&mut node.children, depth - 1, id) {
            return Some(found);
        }
    }
    None
}

fn sort_categories(nodes: &mut [CategoryNode]) {
    nodes.sort_by(|a, b| {
        a.order
            .is_none()
            .cmp(&b.order.is_none())
            .then(a.order.cmp(&b.order))
            .then(crate::alphabetic_cmp(&a.label, &b.label))
            .then(a.value.cmp(&b.value))
    });
    for node in nodes {
        sort_categories(&mut node.children);
    }
}

fn relation_branch_matches(root: &Value, schema: &Value, value: &Value) -> Result<bool> {
    let mut candidate = crate::draft::resolve(root, schema, 0);
    if let Some(obj) = candidate.as_object_mut() {
        for key in ["$defs", "definitions"] {
            if let Some(defs) = root.get(key) {
                obj.entry(key).or_insert_with(|| defs.clone());
            }
        }
    }
    jsonschema::draft202012::options()
        .offline()
        .build(&candidate)
        .map(|v| v.is_valid(value))
        .map_err(|e| Error::Config(format!("reference branch schema: {e}")))
}

fn visit_relations(
    root: &Value,
    schema: &Value,
    value: &Value,
    path: &str,
    depth: usize,
    visit: &mut impl FnMut(&str, &str, bool, &str) -> Result<()>,
) -> Result<()> {
    if depth > 64 {
        return Err(Error::Data("relation nesting too deep".into()));
    }
    let resolved = crate::draft::resolve(root, schema, 0);
    for keyword in ["oneOf", "anyOf", "allOf"] {
        if let Some(branches) = resolved.get(keyword).and_then(Value::as_array) {
            for branch in branches {
                if keyword == "allOf" || relation_branch_matches(root, branch, value)? {
                    visit_relations(root, branch, value, path, depth + 1, visit)?;
                }
            }
        }
    }
    if resolved.get("x-relation").is_some() && !value.is_null() {
        let relation = &resolved["x-relation"];
        let target = relation["collection"]
            .as_str()
            .ok_or_else(|| Error::Config("relation missing collection".into()))?;
        visit(
            target,
            value.as_str().unwrap_or(""),
            relation["allowUnresolved"].as_bool().unwrap_or(false),
            path,
        )?;
    }
    if let Some(condition) = resolved.get("if") {
        let branch = if relation_branch_matches(root, condition, value)? {
            resolved.get("then")
        } else {
            resolved.get("else")
        };
        if let Some(branch) = branch {
            visit_relations(root, branch, value, path, depth + 1, visit)?;
        }
    }
    if let Some(object) = value.as_object() {
        if let Some(dependencies) = resolved["dependentSchemas"].as_object() {
            for (key, branch) in dependencies {
                if object.contains_key(key) {
                    visit_relations(root, branch, value, path, depth + 1, visit)?;
                }
            }
        }
        for (key, child) in object {
            let mut schemas = Vec::new();
            if let Some(s) = resolved.get("properties").and_then(|p| p.get(key)) {
                schemas.push(s);
            }
            if let Some(patterns) = resolved["patternProperties"].as_object() {
                for (pattern, s) in patterns {
                    if relation_branch_matches(
                        root,
                        &serde_json::json!({"type":"string","pattern":pattern}),
                        &Value::String(key.clone()),
                    )? {
                        schemas.push(s);
                    }
                }
            }
            if schemas.is_empty() {
                if let Some(s) = resolved
                    .get("additionalProperties")
                    .filter(|s| s.is_object())
                {
                    schemas.push(s);
                }
            }
            for child_schema in schemas {
                visit_relations(
                    root,
                    child_schema,
                    child,
                    &format!("{path}/{}", pointer_escape(key)),
                    depth + 1,
                    visit,
                )?;
            }
        }
    } else if let Some(array) = value.as_array() {
        for (index, child) in array.iter().enumerate() {
            if let Some(contains) = resolved.get("contains") {
                if relation_branch_matches(root, contains, child)? {
                    visit_relations(
                        root,
                        contains,
                        child,
                        &format!("{path}/{index}"),
                        depth + 1,
                        visit,
                    )?;
                }
            }
            if let Some(child_schema) = resolved
                .get("prefixItems")
                .and_then(|v| v.get(index))
                .or_else(|| resolved.get("items"))
            {
                visit_relations(
                    root,
                    child_schema,
                    child,
                    &format!("{path}/{index}"),
                    depth + 1,
                    visit,
                )?;
            }
        }
    }
    Ok(())
}

fn validate_inline_fields(
    root: &Value,
    schema: &Value,
    value: &Value,
    path: &str,
    depth: usize,
    issues: &mut Vec<ValidationIssue>,
) -> Result<()> {
    if depth > 64 {
        return Err(Error::Data("field nesting too deep".into()));
    }
    let resolved = resolve_ref(root, schema, depth)?;
    if resolved.get("x-content").and_then(Value::as_str) == Some("inline-markdown")
        && value
            .as_str()
            .is_some_and(|text| text.contains(['\n', '\r']))
    {
        issues.push(ValidationIssue {
            path: path.into(),
            message: "one-line Markdown cannot contain a line break".into(),
        });
    }
    for keyword in ["oneOf", "anyOf", "allOf"] {
        if let Some(branches) = resolved.get(keyword).and_then(Value::as_array) {
            for branch in branches {
                if jsonschema::draft202012::options()
                    .offline()
                    .build(branch)
                    .map(|v| v.is_valid(value))
                    .unwrap_or(false)
                {
                    validate_inline_fields(root, branch, value, path, depth + 1, issues)?;
                }
            }
        }
    }
    if let Some(object) = value.as_object() {
        for (key, child) in object {
            if let Some(child_schema) =
                resolved
                    .get("properties")
                    .and_then(|p| p.get(key))
                    .or_else(|| {
                        resolved
                            .get("additionalProperties")
                            .filter(|v| v.is_object())
                    })
            {
                validate_inline_fields(
                    root,
                    child_schema,
                    child,
                    &format!("{path}/{}", pointer_escape(key)),
                    depth + 1,
                    issues,
                )?;
            }
        }
    } else if let Some(array) = value.as_array() {
        for (index, child) in array.iter().enumerate() {
            if let Some(child_schema) = resolved
                .get("prefixItems")
                .and_then(|v| v.get(index))
                .or_else(|| resolved.get("items"))
            {
                validate_inline_fields(
                    root,
                    child_schema,
                    child,
                    &format!("{path}/{index}"),
                    depth + 1,
                    issues,
                )?;
            }
        }
    }
    Ok(())
}

fn pointer_escape(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}
fn resolve_ref<'a>(root: &'a Value, schema: &'a Value, depth: usize) -> Result<&'a Value> {
    if depth > 64 {
        return Err(Error::Data("recursive schema too deep".into()));
    }
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let pointer = reference
            .strip_prefix('#')
            .ok_or_else(|| Error::Config("external schema references are unsupported".into()))?;
        let target = root
            .pointer(pointer)
            .ok_or_else(|| Error::Config(format!("unresolved schema reference {reference}")))?;
        resolve_ref(root, target, depth + 1)
    } else {
        Ok(schema)
    }
}
fn field<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(v, |v, key| v.get(key))
}
pub(crate) fn display_label(
    value: &Value,
    collection: &Collection,
    primary: &str,
    id: &str,
) -> String {
    let composite = if primary == collection.display && !collection.display_fields.is_empty() {
        collection
            .display_fields
            .iter()
            .filter_map(|name| field(value, name))
            .filter_map(|part| match part {
                Value::String(text) if !text.trim().is_empty() => Some(text.clone()),
                Value::Number(number) => Some(number.to_string()),
                _ => None,
            })
            .map(|part| plain_markdown(&part))
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
    } else {
        String::new()
    };
    if !composite.is_empty() {
        return composite.chars().take(150).collect();
    }
    std::iter::once(primary)
        .chain(collection.display_fallback.iter().map(String::as_str))
        .filter_map(|field_name| field(value, field_name).and_then(Value::as_str))
        .find(|text| !text.trim().is_empty())
        .map(|text| plain_markdown(text).chars().take(150).collect())
        .unwrap_or_else(|| id.into())
}
pub(crate) fn plain_markdown(text: &str) -> String {
    let mut plain = String::new();
    for event in Parser::new(text) {
        match event {
            Event::Text(part) | Event::Code(part) | Event::Html(part) | Event::InlineHtml(part) => {
                plain.push_str(&part)
            }
            Event::SoftBreak | Event::HardBreak => plain.push(' '),
            _ => {}
        }
    }
    plain.trim().to_owned()
}

fn visit_markdown_links(value: &Value, path: &str, visit: &mut impl FnMut(&str, &str)) {
    match value {
        Value::Object(fields) => {
            for (key, child) in fields {
                visit_markdown_links(child, &format!("{path}/{}", pointer_escape(key)), visit);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                visit_markdown_links(child, &format!("{path}/{index}"), visit);
            }
        }
        Value::String(text) => {
            let mut destinations = BTreeSet::new();
            for event in Parser::new(text) {
                let destination = match event {
                    Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) => {
                        dest_url
                    }
                    _ => continue,
                };
                destinations.insert(destination.into_string());
            }
            for destination in destinations {
                visit(&destination, path);
            }
        }
        _ => {}
    }
}

fn unique_references(references: &mut Vec<Reference>) {
    references.sort_by(|a, b| {
        (&a.document, &a.collection, &a.record_id, &a.path).cmp(&(
            &b.document,
            &b.collection,
            &b.record_id,
            &b.path,
        ))
    });
    references.dedup_by(|a, b| {
        a.document == b.document
            && a.collection == b.collection
            && a.record_id == b.record_id
            && a.path == b.path
    });
}

fn visit_media_fields(
    root: &Value,
    schema: &Value,
    value: &Value,
    path: &str,
    depth: usize,
    visit: &mut impl FnMut(&str, &str, &str) -> Result<()>,
) -> Result<()> {
    if depth > 64 {
        return Err(Error::Data("media field nesting too deep".into()));
    }
    let resolved = resolve_ref(root, schema, depth)?;
    for keyword in ["oneOf", "anyOf", "allOf"] {
        if let Some(branches) = resolved.get(keyword).and_then(Value::as_array) {
            for branch in branches {
                if jsonschema::draft202012::options()
                    .offline()
                    .build(branch)
                    .map(|validator| validator.is_valid(value))
                    .unwrap_or(false)
                {
                    visit_media_fields(root, branch, value, path, depth + 1, visit)?;
                }
            }
        }
    }
    if (resolved.get("x-content").and_then(Value::as_str) == Some("media")
        || resolved.get("x-editor").and_then(Value::as_str) == Some("media"))
        && let Some(linked_path) = value.as_str()
    {
        visit(
            resolved
                .pointer("/x-media/collection")
                .and_then(Value::as_str)
                .unwrap_or("media"),
            linked_path,
            path,
        )?;
    }
    if let Some(object) = value.as_object() {
        for (key, child) in object {
            if let Some(child_schema) = resolved
                .get("properties")
                .and_then(|properties| properties.get(key))
                .or_else(|| {
                    resolved
                        .get("additionalProperties")
                        .filter(|schema| schema.is_object())
                })
            {
                visit_media_fields(
                    root,
                    child_schema,
                    child,
                    &format!("{path}/{}", pointer_escape(key)),
                    depth + 1,
                    visit,
                )?;
            }
        }
    } else if let Some(items) = value.as_array() {
        for (index, child) in items.iter().enumerate() {
            if let Some(child_schema) = resolved
                .get("prefixItems")
                .and_then(|schemas| schemas.get(index))
                .or_else(|| resolved.get("items"))
            {
                visit_media_fields(
                    root,
                    child_schema,
                    child,
                    &format!("{path}/{index}"),
                    depth + 1,
                    visit,
                )?;
            }
        }
    }
    Ok(())
}

fn record_link_target(destination: &str) -> Option<(String, String)> {
    let route = destination
        .strip_prefix("#/collections/")
        .or_else(|| destination.strip_prefix("/collections/"))?;
    let mut parts = route.split('/');
    let collection = percent_encoding::percent_decode_str(parts.next()?)
        .decode_utf8()
        .ok()?
        .into_owned();
    let next = parts.next()?;
    let next = if next == "records" {
        parts.next()?
    } else {
        next
    };
    let id = percent_encoding::percent_decode_str(next)
        .decode_utf8()
        .ok()?
        .into_owned();
    if parts.next().is_some() {
        return None;
    }
    Some((collection, id))
}

fn rewrite_references(
    schema: &Value,
    value: &mut Value,
    collection: &str,
    old_id: &str,
    new_id: &str,
) -> Result<bool> {
    let mut paths = BTreeSet::new();
    visit_relations(schema, schema, value, "", 0, &mut |target, id, _, path| {
        if target == collection && id == old_id {
            paths.insert(path.to_owned());
        }
        Ok(())
    })?;
    let mut changed = false;
    for path in paths {
        if let Some(slot) = value.pointer_mut(&path)
            && slot.as_str() == Some(old_id)
        {
            *slot = Value::String(new_id.into());
            changed = true;
        }
    }
    let mut links = BTreeSet::new();
    visit_markdown_links(value, "", &mut |destination, path| {
        if record_link_target(destination).as_ref() == Some(&(collection.into(), old_id.into())) {
            links.insert(path.to_owned());
        }
    });
    for path in links {
        if let Some(slot) = value.pointer_mut(&path)
            && let Some(text) = slot.as_str()
        {
            let updated = rewrite_markdown_source(text, collection, old_id, new_id);
            if updated != text {
                *slot = Value::String(updated);
                changed = true;
            }
        }
    }
    Ok(changed)
}

fn rewrite_markdown_source(text: &str, collection: &str, old_id: &str, new_id: &str) -> String {
    use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
    let mut replacements = BTreeMap::<(usize, usize), String>::new();
    let mut definitions = BTreeSet::<String>::new();
    for (event, range) in Parser::new(text).into_offset_iter() {
        let Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) = event else {
            continue;
        };
        let destination = dest_url.as_ref();
        if record_link_target(destination).as_ref() != Some(&(collection.into(), old_id.into())) {
            continue;
        }
        let Some((prefix, _)) = destination.rsplit_once('/') else {
            continue;
        };
        let next = format!("{prefix}/{}", utf8_percent_encode(new_id, NON_ALPHANUMERIC));
        let source = &text[range.clone()];
        // Inline links include their destination in the event's source span.
        // Reference-style links point to a definition elsewhere in the text.
        let destination_start = source.rfind("](").map(|start| start + 2).unwrap_or(0);
        if let Some(found) = source[destination_start..].find(destination) {
            let start = range.start + destination_start + found;
            replacements.insert((start, start + destination.len()), next);
        } else {
            definitions.insert(destination.to_owned());
        }
    }
    let mut line_start = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start_matches([' ', '\t']);
        if trimmed.starts_with('[') {
            if let Some((_, rest)) = trimmed.split_once("]:") {
                let destination_source = rest.trim_start_matches([' ', '\t']);
                let wrapped = destination_source.starts_with('<');
                let source = destination_source
                    .strip_prefix('<')
                    .unwrap_or(destination_source);
                for destination in &definitions {
                    if source.starts_with(destination)
                        && matches!(
                            source[destination.len()..].chars().next(),
                            None | Some('>' | ' ' | '\t' | '\r' | '\n')
                        )
                    {
                        let start = line_start
                            + (line.len() - trimmed.len())
                            + (trimmed.len() - rest.len())
                            + (rest.len() - destination_source.len())
                            + usize::from(wrapped);
                        let prefix = destination.rsplit_once('/').unwrap().0;
                        replacements.insert(
                            (start, start + destination.len()),
                            format!("{prefix}/{}", utf8_percent_encode(new_id, NON_ALPHANUMERIC)),
                        );
                    }
                }
            }
        }
        line_start += line.len();
    }
    let mut updated = text.to_owned();
    for ((start, end), next) in replacements.into_iter().rev() {
        updated.replace_range(start..end, &next);
    }
    updated
}

fn extension(c: &Collection) -> &'static str {
    if c.format == "json" { "json" } else { "md" }
}
pub(crate) fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub(crate) fn reject_symlink(path: &Path) -> Result<()> {
    if let Ok(meta) = fs::symlink_metadata(path)
        && meta.file_type().is_symlink()
    {
        return Err(Error::InvalidPath(path.display().to_string()));
    }
    Ok(())
}

pub(crate) fn record_id(c: &Collection, path: &Path, value: &Value) -> Result<String> {
    let id = match &c.id {
        IdStrategy::Filename => path.file_stem().and_then(|s| s.to_str()),
        IdStrategy::Frontmatter(field) => value.get(field).and_then(Value::as_str),
    }
    .ok_or_else(|| Error::Data(format!("missing ID in {}", path.display())))?;
    safe_name(id)?;
    Ok(id.into())
}

fn parse(c: &Collection, bytes: &[u8]) -> Result<Value> {
    if c.format == "json" {
        return serde_json::from_slice(bytes).map_err(|e| Error::Data(e.to_string()));
    }
    let text = std::str::from_utf8(bytes).map_err(|e| Error::Data(e.to_string()))?;
    if !text.starts_with("---\n") {
        let mut map = Map::new();
        map.insert(
            if c.body_field.is_empty() {
                "body"
            } else {
                &c.body_field
            }
            .into(),
            Value::String(text.into()),
        );
        return Ok(Value::Object(map));
    }
    let end = text[4..]
        .find("\n---\n")
        .ok_or_else(|| Error::Data("unterminated frontmatter".into()))?
        + 4;
    let metadata: Value =
        serde_yaml::from_str(&text[4..end]).map_err(|e| Error::Data(e.to_string()))?;
    let mut map = metadata
        .as_object()
        .cloned()
        .ok_or_else(|| Error::Data("frontmatter must be an object".into()))?;
    if map.contains_key(if c.body_field.is_empty() {
        "body"
    } else {
        &c.body_field
    }) {
        return Err(Error::Data("frontmatter key 'body' is reserved".into()));
    }
    map.insert(
        if c.body_field.is_empty() {
            "body"
        } else {
            &c.body_field
        }
        .into(),
        Value::String(text[end + 5..].into()),
    );
    Ok(Value::Object(map))
}

fn serialize(c: &Collection, value: &Value) -> Result<Vec<u8>> {
    if c.format == "json" {
        let mut bytes = serde_json::to_vec_pretty(value).map_err(|e| Error::Data(e.to_string()))?;
        bytes.push(b'\n');
        return Ok(bytes);
    }
    let mut map = value
        .as_object()
        .cloned()
        .ok_or_else(|| Error::Data("Markdown document must be an object".into()))?;
    let body = map
        .remove(if c.body_field.is_empty() {
            "body"
        } else {
            &c.body_field
        })
        .and_then(|v| v.as_str().map(str::to_owned))
        .ok_or_else(|| Error::Data("body must be a string".into()))?;
    if map.is_empty() {
        return Ok(body.into_bytes());
    }
    let yaml = serde_yaml::to_string(&map).map_err(|e| Error::Data(e.to_string()))?;
    Ok(format!("---\n{}---\n{}", yaml, body).into_bytes())
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::InvalidPath(path.display().to_string()))?;
    let temp = parent.join(format!(".editor-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn atomic_create(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::InvalidPath(path.display().to_string()))?;
    let temp = parent.join(format!(".editor-{}.tmp", uuid::Uuid::new_v4()));
    let mut linked = false;
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::hard_link(&temp, path)?;
        linked = true;
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    let _ = fs::remove_file(&temp);
    if result.is_err() && linked {
        let _ = fs::remove_file(path);
    }
    result
}
