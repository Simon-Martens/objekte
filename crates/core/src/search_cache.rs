//! Reusable per-record projections and request-local filesystem snapshots.
use super::read_cache::{Signature, Stamp};
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) struct Snapshot {
    pub collections: HashMap<String, Arc<Signature>>,
    stamps: HashMap<PathBuf, Stamp>,
    indexes: Mutex<HashMap<String, Arc<HashMap<String, PathBuf>>>>,
    pub unstable: AtomicBool,
}

impl Snapshot {
    pub fn new(editor: &Editor) -> Result<Self> {
        let mut collections = HashMap::new();
        let mut stamps = HashMap::new();
        for name in editor.config.collections.keys() {
            let signature = editor.collection_signature(name)?;
            stamps.extend(signature.iter().cloned());
            collections.insert(name.clone(), Arc::new(signature));
        }
        Ok(Self {
            collections,
            stamps,
            indexes: Mutex::default(),
            unstable: AtomicBool::new(false),
        })
    }

    pub fn observe(&self, path: &Path, stamp: &Stamp) {
        if self.stamps.get(path) != Some(stamp) {
            self.unstable.store(true, Ordering::Relaxed);
        }
    }

    pub fn find_path(&self, editor: &Editor, name: &str, id: &str) -> Result<PathBuf> {
        safe_name(id)?;
        let collection = editor.config.collection(name)?;
        if matches!(collection.id, IdStrategy::Filename) {
            let path = editor
                .config
                .collection_dir(name)?
                .join(format!("{id}.{}", extension(collection)));
            return self
                .stamps
                .contains_key(&path)
                .then_some(path)
                .ok_or_else(|| Error::NotFound(format!("record {id}")));
        }
        let cached = self.indexes.lock().unwrap().get(name).cloned();
        let index = if let Some(index) = cached {
            index
        } else {
            let index = editor.indexed_paths(name, &self.collections[name])?;
            self.indexes
                .lock()
                .unwrap()
                .insert(name.into(), index.clone());
            index
        };
        index
            .get(id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("record {id}")))
    }

    fn dependency(&self, editor: &Editor, collection: &str, id: &str) -> Dependency {
        let file = self
            .find_path(editor, collection, id)
            .ok()
            .and_then(|path| self.stamps.get(&path).cloned().map(|stamp| (path, stamp)));
        Dependency {
            collection: collection.into(),
            id: id.into(),
            file,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Dependency {
    collection: String,
    id: String,
    // Missing targets are dependencies too: creating one must refresh its referrers.
    file: Option<(PathBuf, Stamp)>,
}

pub(super) struct RecordVersion {
    path: PathBuf,
    stamp: Stamp,
    dependencies: Vec<Dependency>,
}
impl RecordVersion {
    fn valid(&self, editor: &Editor, snapshot: &Snapshot) -> bool {
        snapshot.stamps.get(&self.path) == Some(&self.stamp)
            && self.dependencies.iter().all(|dependency| {
                snapshot.dependency(editor, &dependency.collection, &dependency.id) == *dependency
            })
    }
    fn bytes(&self) -> usize {
        128 + self.path.as_os_str().len()
            + self
                .dependencies
                .iter()
                .map(|d| {
                    192 + d.collection.capacity()
                        + d.id.capacity()
                        + d.file.as_ref().map_or(0, |(p, _)| p.as_os_str().len())
                })
                .sum::<usize>()
    }
}

pub(super) struct PreparedRecord {
    pub row: Arc<ListRow>,
    pub version: Arc<RecordVersion>,
}

/// Queries retain ordered IDs/paths and small dependency versions, never full rows.
pub(super) struct CachedList {
    pub source: Arc<Signature>,
    pub versions: Vec<Arc<RecordVersion>>,
    pub records: Vec<(String, PathBuf)>,
    pub total: usize,
    pub facets: Vec<ListFacet>,
}
impl CachedList {
    pub fn valid(&self, editor: &Editor, name: &str, snapshot: &Snapshot) -> bool {
        self.source == snapshot.collections[name]
            && self.versions.iter().all(|v| v.valid(editor, snapshot))
    }
    pub fn bytes(&self) -> usize {
        256 + self
            .source
            .iter()
            .map(|(p, _)| p.as_os_str().len() + 128)
            .sum::<usize>()
            + self.versions.iter().map(|v| v.bytes() + 8).sum::<usize>()
            + self
                .records
                .iter()
                .map(|(id, p)| id.capacity() + p.as_os_str().len() + 64)
                .sum::<usize>()
            + self
                .facets
                .iter()
                .map(|f| {
                    128 + f.field.capacity()
                        + f.label.capacity()
                        + f.kind.capacity()
                        + f.options
                            .iter()
                            .map(|(v, l, _)| v.capacity() + l.capacity() + 64)
                            .sum::<usize>()
                })
                .sum::<usize>()
    }
}

impl Editor {
    pub(super) fn prepared_record(&self, name: &str, path: &Path) -> Result<Arc<PreparedRecord>> {
        let snapshot = self.search_snapshot.as_ref().expect("search snapshot");
        let key = path.to_owned();
        let cached = self.read_cache.prepared.lock().unwrap().get(&key).cloned();
        if let Some(record) = cached {
            if record.version.valid(self, snapshot) {
                return Ok(record);
            }
            self.read_cache.prepared.lock().unwrap().remove(&key);
        }
        let mut reader = self.clone();
        let dependencies = Arc::new(Mutex::new(BTreeSet::new()));
        reader.read_dependencies = Some(dependencies.clone());
        let doc = reader.cached_document(name, path)?;
        let c = reader.config.collection(name)?;
        let value = reader.view_value(name, &doc.value)?;
        let mut display = BTreeMap::new();
        if let Some(props) = c.editor_schema["properties"].as_object() {
            for (key, schema) in props {
                if !listing::visible_field(key, schema) {
                    continue;
                }
                if let Some(v) = value.get(key) {
                    let mut text = listing::field_display(v, schema);
                    if schema["x-children"] == true {
                        text = v.as_array().map_or(0, |a| a.len()).to_string();
                    }
                    if let (Some(target), Some(id)) = (
                        schema
                            .pointer("/x-relation/collection")
                            .and_then(Value::as_str),
                        v.as_str(),
                    ) {
                        text = reader
                            .get(target, id)
                            .ok()
                            .and_then(|d| reader.label(target, &d.value, id, "reference").ok())
                            .unwrap_or_else(|| id.into());
                    }
                    display.insert(key.clone(), text);
                }
            }
        }
        let row = Arc::new(ListRow {
            label: reader.label(name, &value, &doc.id, "list")?,
            id: doc.id.clone(),
            value,
            display,
            matches: Vec::new(),
        });
        let dependencies = dependencies
            .lock()
            .unwrap()
            .iter()
            .map(|(collection, id)| snapshot.dependency(&reader, collection, id))
            .collect();
        let version = Arc::new(RecordVersion {
            path: key.clone(),
            stamp: snapshot.stamps[&key].clone(),
            dependencies,
        });
        let bytes = 256
            + key.as_os_str().len()
            + version.bytes()
            + row.id.capacity()
            + row.label.capacity()
            + byte_cache::value_bytes(&row.value)
            + row
                .display
                .iter()
                .map(|(k, v)| k.capacity() + v.capacity() + 96)
                .sum::<usize>();
        let record = Arc::new(PreparedRecord { row, version });
        #[cfg(test)]
        self.read_cache.preparations.fetch_add(1, Ordering::Relaxed);
        if !snapshot.unstable.load(Ordering::Relaxed) {
            self.read_cache
                .prepared
                .lock()
                .unwrap()
                .insert(key, record.clone(), bytes);
        }
        Ok(record)
    }

    pub(super) fn cached_page(
        &self,
        name: &str,
        page: &CachedList,
        request: &ListRequest,
        matcher: &crate::SearchMatcher,
    ) -> Result<ListPage> {
        let mut rows = Vec::new();
        for (_, path) in page
            .records
            .iter()
            .skip(request.offset)
            .take(request.limit.clamp(1, 200))
        {
            let record = self.prepared_record(name, path)?;
            let mut row = (*record.row).clone();
            row.matches = listing::row_matches(&row, self.config.collection(name)?, matcher);
            rows.push(row);
        }
        Ok(ListPage {
            rows,
            total: page.total,
            matched: page.records.len(),
            facets: page.facets.clone(),
        })
    }
}
