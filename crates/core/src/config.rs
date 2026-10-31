//! Collection configuration, legacy view settings, and fingerprints.
mod discovery;
use crate::{
    CollectionGraph, Error, Field, Result, ValueType,
    error::{config, io},
    schema::Schema,
};
pub(crate) use discovery::Discovery;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

pub(crate) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Version/domain tagged, unambiguously serialized compound fingerprint.
pub(crate) fn fingerprint(domain: &str, value: &impl Serialize) -> String {
    // INFO: Hash writer that feeds serialized configuration bytes into SHA-256.
    struct Sink(Sha256);


    impl std::io::Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }


    let mut sink = Sink(Sha256::new());
    {
        use std::io::Write;
        // JSON emits many tiny writes. Batch them without changing the persisted bytes.
        let mut buffered = std::io::BufWriter::with_capacity(64 * 1024, &mut sink);
        serde_json::to_writer(&mut buffered, &(domain, value))
            .expect("fingerprint input serializes");
        buffered.flush().expect("in-memory hash sink cannot fail");
    }
    format!("{:x}", sink.0.finalize())
}

#[cfg(test)]
#[path = "config.test.rs"]
mod fingerprint_tests;


// INFO: Fingerprints for filter, sort, projection, and complete view definitions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ViewHashes {
    pub filters: String,
    pub sort: String,
    pub projection: String,
    pub combined: String,
}


impl ViewHashes {
    pub(crate) fn new(
        filters: &impl Serialize,
        sort: &impl Serialize,
        projection: &impl Serialize,
        dependencies: &impl Serialize,
    ) -> Self {
        let filters = fingerprint("view-filters/v1", filters);
        let sort = fingerprint("view-sort/v1", sort);
        let projection = fingerprint("view-projection/v1", projection);
        let combined = fingerprint("view/v1", &(&filters, &sort, &projection, dependencies));
        Self {
            filters,
            sort,
            projection,
            combined,
        }
    }
}


#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    #[default]
    Asc,
    Desc,
}


// INFO: Field and direction for one ordered view key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sort {
    pub field: String,
    pub direction: Direction,
}


impl Default for Sort {
    fn default() -> Self {
        Self {
            field: "@id".into(),
            direction: Direction::Asc,
        }
    }
}


// INFO: Configured columns, filters, and sort choices for a collection view.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ListOptions {
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub filters: Vec<String>,
    #[serde(default)]
    pub default_sort: Sort,
    #[serde(default)]
    pub sub_sorts: Vec<Sort>,
    pub sortable: Option<Vec<String>>,
}


// INFO: Name, type, and nullability of a configured scalar index.
#[derive(Debug, Clone, Serialize)]
pub struct IndexField {
    pub name: String,
    pub kind: ValueType,
    pub nullable: bool,
}


// INFO: Resolved collection paths, schema, UI settings, fields, and hashes.
#[derive(Clone, Serialize)]
pub struct Collection {
    pub name: String,
    pub path: PathBuf,
    /// Descendants belong to this collection when a schema declares its base.
    pub recursive: bool,
    pub schema_path: Option<PathBuf>,
    pub ui_path: Option<PathBuf>,
    pub label: String,
    pub display: String,
    pub page_size: usize,
    pub list: ListOptions,
    pub fields: BTreeMap<String, Field>,
    pub indexes: Vec<IndexField>,
    pub advanced_index_paths: Vec<crate::JsonPath>,
    pub schema_hash: Option<String>,
    pub index_hash: String,
    pub mapping_hash: String,
    pub view_hashes: ViewHashes,
    #[serde(skip)]
    pub(crate) schema: Option<Schema>,
}


impl Collection {
    /// Open one populated JSON folder as a standalone collection.
    pub fn open(folder: impl AsRef<Path>) -> Result<Self> {
        Self::open_inner(folder.as_ref(), None)
    }

    /// Open one folder and group its entire subtree under the supplied schema.
    pub fn open_with_schema(folder: impl AsRef<Path>, schema: impl AsRef<Path>) -> Result<Self> {
        Self::open_inner(folder.as_ref(), Some(schema.as_ref()))
    }

    fn open_inner(folder: &Path, schema: Option<&Path>) -> Result<Self> {
        let mut settings = Discovery {
            data_root: Some(PathBuf::from(".")),
            ..Discovery::default()
        };
        if let Some(path) = schema {
            settings.schemas.push((PathBuf::from("."), path.to_owned()));
        }
        RepositoryModel::discover(folder, &settings)?
            .collections
            .remove("@root")
            .ok_or_else(|| config(folder, "", "collection must contain a JSON file"))
    }
}


// INFO: Loaded repository root, collections, relationship graph, and input ownership.
#[derive(Clone, Serialize)]
pub struct RepositoryModel {
    pub root: PathBuf,
    pub collections: BTreeMap<String, Collection>,
    pub views: BTreeMap<String, crate::ViewDefinition>,
    pub graph: CollectionGraph,
    #[serde(skip)]
    pub(crate) files: BTreeMap<PathBuf, String>,
    #[serde(skip)]
    pub(crate) input_collections: BTreeMap<PathBuf, BTreeSet<String>>,
}


// INFO: Repository-level JSON configuration before resolution.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepoFile {
    version: u32,
    ui: Option<String>,
    collections: BTreeMap<String, String>,
}


// INFO: Collection-level JSON configuration before resolution.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CollectionFile {
    r#type: String,
    path: String,
    format: String,
    id: String,
    schema: Option<String>,
    ui: Option<String>,
}


// INFO: Collection UI settings read from JSON.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct UiFile {
    label: Option<String>,
    display: String,
    page_size: Option<usize>,
    #[serde(default)]
    list: ListOptions,
}


// INFO: Repository UI configuration containing shared defaults.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RepoUi {
    #[serde(default)]
    defaults: Defaults,
}


// INFO: Default page size inherited by collection UI settings.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Defaults {
    page_size: Option<usize>,
}


pub(crate) fn inside(root: &Path, relative: &str) -> Result<PathBuf> {
    let p = Path::new(relative);
    if relative.is_empty() || p.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(config(
            root,
            relative,
            "path must be relative and cannot contain parent/current components",
        ));
    }
    let full = root.join(p);
    let canonical = fs::canonicalize(&full).map_err(|e| io(&full, e))?;
    if canonical.to_str().is_none() {
        return Err(config(
            &full,
            "",
            "configured paths must resolve to UTF-8 paths",
        ));
    }
    if !canonical.starts_with(root) {
        return Err(config(&full, "", "path escapes repository"));
    }
    Ok(canonical)
}

fn read<T: serde::de::DeserializeOwned>(
    path: &Path,
    files: &mut BTreeMap<PathBuf, String>,
) -> Result<T> {
    let bytes = fs::read(path).map_err(|e| io(path, e))?;
    let value = sonic_rs::from_slice(&bytes).map_err(|e| config(path, "", e))?;
    let hash = digest(&bytes);
    if files.get(path).is_some_and(|previous| previous != &hash) {
        return Err(Error::Unstable(path.into()));
    }
    files.insert(path.to_owned(), hash);
    Ok(value)
}

fn scalar<'a>(
    fields: &'a BTreeMap<String, Field>,
    name: &str,
    file: &Path,
) -> Result<Option<&'a Field>> {
    if name == "@id" {
        return Ok(None);
    }
    let f = fields
        .get(name)
        .ok_or_else(|| config(file, name, "unknown view field"))?;
    if !f.kind.scalar() {
        return Err(config(file, name, "view fields must be declared scalars"));
    }
    Ok(Some(f))
}

fn unique(names: &[String], file: &Path) -> Result<()> {
    if names.iter().collect::<BTreeSet<_>>().len() != names.len() {
        return Err(config(file, "/list", "duplicate field"));
    }
    Ok(())
}


impl RepositoryModel {
    pub(crate) fn load(root: &Path) -> Result<Self> {
        let root = fs::canonicalize(root).map_err(|e| io(root, e))?;
        if root.to_str().is_none() {
            return Err(config(&root, "", "repository path must be UTF-8"));
        }
        let mut files = BTreeMap::new();
        let mut input_collections = BTreeMap::<PathBuf, BTreeSet<String>>::new();
        let rp = inside(&root, ".editor/repository.json")?;
        let r: RepoFile = read(&rp, &mut files)?;
        if r.version != 2 {
            return Err(config(&rp, "/version", "expected version 2"));
        }
        let defaults = if let Some(ui) = r.ui {
            let p = inside(&root, &ui)?;
            read::<RepoUi>(&p, &mut files)?.defaults
        } else {
            Defaults::default()
        };
        let default_page = defaults.page_size.unwrap_or(50);
        for path in files.keys() {
            input_collections.insert(path.clone(), r.collections.keys().cloned().collect());
        }
        if default_page == 0 || default_page > i64::MAX as usize {
            return Err(config(&rp, "/ui/defaults/pageSize", "invalid page size"));
        }
        let mut collections = BTreeMap::new();
        let mut sources = BTreeMap::<PathBuf, bool>::new();
        for (name, settings) in r.collections {
            if name.is_empty() {
                return Err(config(&rp, "/collections", "empty collection name"));
            }
            let p = inside(&root, &settings)?;
            let c: CollectionFile = read(&p, &mut files)?;
            if c.r#type != "data" || c.format != "json" || c.id != "filename" {
                return Err(config(
                    &p,
                    "",
                    "only data/json/filename collections are supported",
                ));
            }
            let source = inside(&root, &c.path)?;
            if !source.is_dir() || source == root || source.starts_with(root.join(".editor")) {
                return Err(config(
                    &p,
                    "/path",
                    "source must be a separate data directory",
                ));
            }
            let schema_path = match c.schema.as_deref() {
                Some(path) => Some(inside(&root, path)?),
                None => {
                    let local = source.join(".editor/schema.json");
                    let shared = root
                        .join(".editor/collections")
                        .join(&name)
                        .join("schema.json");
                    if local.is_file() {
                        Some(local)
                    } else {
                        shared.is_file().then_some(shared)
                    }
                }
            };
            let (schema, schema_hash, fields, indexes, advanced_index_paths) =
                if let Some(sp) = &schema_path {
                    let crate::json::ParsedValue(raw) = read(sp, &mut files)?;
                    let schema = Schema::compile(&raw, sp)?;
                    let advanced_index_paths = crate::schema::index_paths(&raw, sp)?;
                    let indexes = schema
                        .field
                        .properties
                        .iter()
                        .filter(|(_, field)| field.index)
                        .map(|(name, field)| IndexField {
                            name: name.clone(),
                            kind: field.kind,
                            nullable: field.nullable,
                        })
                        .collect();
                    let fields = schema.field.properties.clone();
                    (
                        Some(schema),
                        Some(fingerprint("schema/v1", &raw)),
                        fields,
                        indexes,
                        advanced_index_paths,
                    )
                } else {
                    (None, None, BTreeMap::new(), Vec::new(), Vec::new())
                };
            let recursive = schema.is_some();
            if sources.insert(source.clone(), recursive).is_some()
                || sources.iter().any(|(other, covered)| {
                    other != &source
                        && ((*covered && source.starts_with(other))
                            || (recursive && other.starts_with(&source)))
                })
            {
                return Err(config(&p, "/path", "schema base folders overlap"));
            }
            if !discovery::has_json(&source, recursive)? {
                return Err(config(&p, "/path", "collection must contain a JSON file"));
            }
            let ui_path = match c.ui.as_deref() {
                Some(path) => Some(inside(&root, path)?),
                None => {
                    let default = root.join(".editor/collections").join(&name).join("ui.json");
                    default.is_file().then_some(default)
                }
            };
            let mut ui: UiFile = if let Some(up) = &ui_path {
                read(up, &mut files)?
            } else {
                UiFile {
                    label: None,
                    display: "@id".into(),
                    page_size: None,
                    list: ListOptions::default(),
                }
            };
            for path in std::iter::once(&p)
                .chain(schema_path.iter())
                .chain(ui_path.iter())
            {
                input_collections
                    .entry(path.clone())
                    .or_default()
                    .insert(name.clone());
            }
            let up = ui_path.as_deref().unwrap_or(&p);
            let page_size = ui.page_size.unwrap_or(default_page);
            if page_size == 0 || page_size > i64::MAX as usize {
                return Err(config(up, "/pageSize", "invalid page size"));
            }
            scalar(&fields, &ui.display, up)?;
            if ui.list.columns.is_empty() {
                ui.list.columns = vec![ui.display.clone()]
            }
            let order = complete_order(
                std::iter::once(ui.list.default_sort.clone())
                    .chain(ui.list.sub_sorts.clone())
                    .collect(),
            )
            .map_err(|e| config(up, "/list/subSorts", e))?;
            if ui.list.sortable.is_none() {
                ui.list.sortable = Some(
                    order
                        .iter()
                        .map(|s| s.field.clone())
                        .filter(|s| s != "@id")
                        .collect(),
                )
            }
            let sortable = ui.list.sortable.as_ref().unwrap();
            if order
                .iter()
                .any(|s| s.field != "@id" && !sortable.contains(&s.field))
            {
                return Err(config(
                    up,
                    "/list/sortable",
                    "default sort and sub-sorts must be sortable",
                ));
            }
            for names in [&ui.list.columns, &ui.list.filters, sortable] {
                unique(names, up)?;
                for name in names {
                    scalar(&fields, name, up)?;
                }
            }
            for sort in &order {
                scalar(&fields, &sort.field, up)?;
            }
            let index_hash = fingerprint(
                "ordered-scalar-indexes/v3",
                &(&indexes, &advanced_index_paths),
            );
            let mapping_hash = fingerprint("mapping/v1", &(&source, &c.format, &c.id));
            let label = ui.label.unwrap_or_else(|| name.clone());
            let view_hashes = ViewHashes::new(
                &ui.list.filters,
                &(&order, &ui.list.sortable),
                &(&label, &ui.display, &ui.list.columns, page_size),
                &(&name, &schema_hash, &mapping_hash),
            );
            collections.insert(
                name.clone(),
                Collection {
                    name: name.clone(),
                    path: source,
                    recursive,
                    schema_path,
                    ui_path,
                    label,
                    display: ui.display,
                    page_size,
                    list: ui.list,
                    fields,
                    indexes,
                    advanced_index_paths,
                    schema_hash,
                    index_hash,
                    mapping_hash,
                    view_hashes,
                    schema,
                },
            );
        }
        let graph = CollectionGraph::build(&collections)?;
        let model = Self {
            root,
            collections,
            views: BTreeMap::new(),
            graph,
            files,
            input_collections,
        };
        if model.changed()? {
            return Err(Error::Unstable(model.root.clone()));
        }
        Ok(model)
    }

    pub fn collection(&self, name: &str) -> Result<&Collection> {
        self.collections
            .get(name)
            .ok_or_else(|| Error::Request(format!("unknown collection {name}")))
    }

    pub(crate) fn changed(&self) -> Result<bool> {
        for (p, hash) in &self.files {
            let bytes = match fs::read(p) {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(true),
                Err(e) => return Err(io(p, e)),
            };
            if digest(&bytes) != *hash {
                return Ok(true);
            }
        }
        Ok(false)
    }
}


pub(crate) fn complete_order(mut order: Vec<Sort>) -> Result<Vec<Sort>> {
    let mut names = BTreeSet::new();
    for (i, s) in order.iter().enumerate() {
        if !names.insert(&s.field) {
            return Err(Error::Request("duplicate sort field".into()));
        }
        if s.field == "@id" && i + 1 != order.len() {
            return Err(Error::Request("@id must be the last sort key".into()));
        }
    }
    if !names.contains(&"@id".to_owned()) {
        order.push(Sort::default());
    }
    Ok(order)
}
