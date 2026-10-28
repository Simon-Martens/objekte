use crate::{
    CollectionGraph, Error, Field, Result, ValueType,
    error::{config, io},
    schema::Schema,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
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
mod fingerprint_tests {
    use super::*;
    #[test]
    fn buffered_fingerprints_preserve_existing_bytes_across_buffer_boundaries() {
        let records = (0..2000)
            .map(|i| {
                (
                    format!("record-{i}"),
                    serde_json::json!({
                        "size":i,"modified":"-123456789","identity":"device:inode:ctime",
                        "escaped":"quote\" slash\\ newline\n é 🐈", "large":"x".repeat(i%90)
                    }),
                )
            })
            .collect::<BTreeMap<_, _>>();
        for domain in ["source-manifest/v2", "different-domain"] {
            assert_eq!(
                fingerprint(domain, &records),
                digest(&serde_json::to_vec(&(domain, &records)).unwrap())
            );
        }
        assert_eq!(
            fingerprint("empty", &Vec::<String>::new()),
            digest(br#"["empty",[]]"#)
        );
        let oversized = "a".repeat(128 * 1024);
        assert_eq!(
            fingerprint("large", &oversized),
            digest(&serde_json::to_vec(&("large", &oversized)).unwrap())
        );
    }
}
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
#[derive(Debug, Clone, Serialize)]
pub struct IndexField {
    pub name: String,
    pub kind: ValueType,
    pub nullable: bool,
}
#[derive(Clone, Serialize)]
pub struct Collection {
    pub name: String,
    pub path: PathBuf,
    pub schema_path: PathBuf,
    pub ui_path: PathBuf,
    pub label: String,
    pub display: String,
    pub page_size: usize,
    pub list: ListOptions,
    pub fields: BTreeMap<String, Field>,
    pub indexes: Vec<IndexField>,
    pub schema_hash: String,
    pub index_hash: String,
    pub mapping_hash: String,
    pub view_hashes: ViewHashes,
    #[serde(skip)]
    pub(crate) schema: Schema,
}
#[derive(Clone, Serialize)]
pub struct RepositoryModel {
    pub root: PathBuf,
    pub collections: BTreeMap<String, Collection>,
    pub graph: CollectionGraph,
    #[serde(skip)]
    pub(crate) files: BTreeMap<PathBuf, String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepoFile {
    version: u32,
    ui: Option<String>,
    collections: BTreeMap<String, String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CollectionFile {
    r#type: String,
    path: String,
    format: String,
    id: String,
    schema: String,
    ui: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct UiFile {
    label: Option<String>,
    display: String,
    page_size: Option<usize>,
    #[serde(default)]
    list: ListOptions,
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RepoUi {
    #[serde(default)]
    defaults: Defaults,
}
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
    let value = serde_json::from_slice(&bytes).map_err(|e| config(path, "", e))?;
    files.insert(path.to_owned(), digest(&bytes));
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
        if default_page == 0 || default_page > i64::MAX as usize {
            return Err(config(&rp, "/ui/defaults/pageSize", "invalid page size"));
        }
        let mut collections = BTreeMap::new();
        let mut sources = BTreeSet::new();
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
            if !sources.insert(source.clone())
                || sources.iter().any(|other| {
                    other != &source && (other.starts_with(&source) || source.starts_with(other))
                })
            {
                return Err(config(&p, "/path", "collection source directories overlap"));
            }
            let sp = inside(&root, &c.schema)?;
            let raw: Value = read(&sp, &mut files)?;
            let schema = Schema::compile(&raw, &sp)?;
            let up = inside(&root, &c.ui)?;
            let mut ui: UiFile = read(&up, &mut files)?;
            let page_size = ui.page_size.unwrap_or(default_page);
            if page_size == 0 || page_size > i64::MAX as usize {
                return Err(config(&up, "/pageSize", "invalid page size"));
            }
            scalar(&schema.field.properties, &ui.display, &up)?;
            if ui.list.columns.is_empty() {
                ui.list.columns = vec![ui.display.clone()]
            }
            let order = complete_order(
                std::iter::once(ui.list.default_sort.clone())
                    .chain(ui.list.sub_sorts.clone())
                    .collect(),
            )
            .map_err(|e| config(&up, "/list/subSorts", e))?;
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
                    &up,
                    "/list/sortable",
                    "default sort and sub-sorts must be sortable",
                ));
            }
            for names in [&ui.list.columns, &ui.list.filters, sortable] {
                unique(names, &up)?;
                for name in names {
                    scalar(&schema.field.properties, name, &up)?;
                }
            }
            for sort in &order {
                scalar(&schema.field.properties, &sort.field, &up)?;
            }
            let names = ui
                .list
                .filters
                .iter()
                .chain(sortable)
                .chain(order.iter().map(|s| &s.field))
                .cloned()
                .chain(std::iter::once("@id".into()))
                .collect::<BTreeSet<_>>();
            let indexes = names
                .into_iter()
                .map(|name| {
                    let f = schema.field.properties.get(&name);
                    IndexField {
                        name,
                        kind: f.map_or(ValueType::String, |f| f.kind),
                        nullable: f.is_some_and(|f| f.nullable),
                    }
                })
                .collect::<Vec<_>>();
            let index_hash = fingerprint("typed-indexes/v1", &indexes);
            let schema_hash = fingerprint("schema/v1", &raw);
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
                    schema_path: sp,
                    ui_path: up,
                    label,
                    display: ui.display,
                    page_size,
                    list: ui.list,
                    fields: schema.field.properties.clone(),
                    indexes,
                    schema_hash,
                    index_hash,
                    mapping_hash,
                    view_hashes,
                    schema,
                },
            );
        }
        let graph = CollectionGraph::build(&collections)?;
        Ok(Self {
            root,
            collections,
            graph,
            files,
        })
    }
    pub fn collection(&self, name: &str) -> Result<&Collection> {
        self.collections
            .get(name)
            .ok_or_else(|| Error::Request(format!("unknown collection {name}")))
    }
    pub(crate) fn changed(&self) -> Result<bool> {
        for (p, hash) in &self.files {
            let bytes = fs::read(p).map_err(|e| io(p, e))?;
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
