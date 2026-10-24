//! Record caches validated by stamps; collection snapshots maintained by file events.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Stamp {
    len: u64,
    modified: std::time::SystemTime,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}
pub(super) fn stamp(path: &Path) -> Result<Stamp> {
    reject_symlink(path)?;
    let m = fs::metadata(path)?;
    Ok(Stamp {
        len: m.len(),
        modified: m.modified()?,
        #[cfg(unix)]
        identity: {
            use std::os::unix::fs::MetadataExt;
            (m.dev(), m.ino(), m.ctime(), m.ctime_nsec())
        },
    })
}
#[derive(Clone, Debug, Default)]
pub(super) struct Signature(Arc<Vec<(PathBuf, Option<Stamp>)>>);
impl std::ops::Deref for Signature {
    type Target = Vec<(PathBuf, Option<Stamp>)>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl PartialEq for Signature {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || self.0 == other.0
    }
}
impl Eq for Signature {}
impl FromIterator<(PathBuf, Option<Stamp>)> for Signature {
    fn from_iter<T: IntoIterator<Item = (PathBuf, Option<Stamp>)>>(iter: T) -> Self {
        Self(Arc::new(iter.into_iter().collect()))
    }
}
impl<'a> IntoIterator for &'a Signature {
    type Item = &'a (PathBuf, Option<Stamp>);
    type IntoIter = std::slice::Iter<'a, (PathBuf, Option<Stamp>)>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ListSignature {
    pub source: Signature,
    pub dependencies: BTreeMap<String, Signature>,
}
type RecordEntry = Arc<Mutex<Option<(Stamp, Arc<Document>)>>>;
#[derive(Default)]
struct RecordCache {
    entries: HashMap<PathBuf, RecordEntry>,
    order: std::collections::VecDeque<PathBuf>,
}
type IdEntry = Arc<Mutex<IdCache>>;
#[derive(Default)]
struct IdCache {
    signature: Signature,
    initialized: bool,
    by_id: HashMap<String, PathBuf>,
    by_path: HashMap<PathBuf, String>,
}
type QueryCache<T> = Mutex<Vec<(ListSignature, String, Arc<T>)>>;
pub(super) type ReferenceIndex = HashMap<(String, String), Vec<Reference>>;
#[derive(Default)]
pub(super) struct ReferenceCache {
    pub stamps: HashMap<PathBuf, Stamp>,
    pub contributions: HashMap<PathBuf, BTreeSet<(String, String)>>,
    pub by_target: HashMap<(String, String), BTreeMap<PathBuf, Vec<Reference>>>,
    pub file_ids: HashMap<PathBuf, (String, String)>,
    pub dependencies: HashMap<PathBuf, BTreeSet<(String, String)>>,
    #[cfg(test)]
    pub scans: usize,
}
impl ReferenceCache {
    pub fn remove(&mut self, path: &Path) {
        self.stamps.remove(path);
        self.file_ids.remove(path);
        self.dependencies.remove(path);
        if let Some(old) = self.contributions.remove(path) {
            for target in old {
                if let Some(files) = self.by_target.get_mut(&target) {
                    files.remove(path);
                    if files.is_empty() {
                        self.by_target.remove(&target);
                    }
                }
            }
        }
    }

    pub fn replace(
        &mut self,
        path: PathBuf,
        stamp: Stamp,
        refs: ReferenceIndex,
        id: Option<(String, String)>,
        dependencies: BTreeSet<(String, String)>,
    ) {
        self.remove(&path);
        let targets = refs.keys().cloned().collect();
        for (target, references) in refs {
            self.by_target
                .entry(target)
                .or_default()
                .insert(path.clone(), references);
        }
        self.contributions.insert(path.clone(), targets);
        if let Some(id) = id {
            self.file_ids.insert(path.clone(), id);
        }
        self.dependencies.insert(path.clone(), dependencies);
        self.stamps.insert(path, stamp);
        #[cfg(test)]
        {
            self.scans += 1;
        }
    }
}
pub(super) struct ReadCache {
    pub files: super::file_changes::FileChanges,
    pub matchers: Mutex<
        Vec<(
            String,
            structured_editor_search::SearchOptions,
            Arc<structured_editor_search::SearchMatcher>,
        )>,
    >,
    records: Mutex<RecordCache>,
    ids: Mutex<HashMap<String, IdEntry>>,
    pub lists: QueryCache<super::listing::CachedListPage>,
    pub prepared: QueryCache<super::listing::PreparedList>,
    pub references: Mutex<ReferenceCache>,
    #[cfg(test)]
    pub parses: std::sync::atomic::AtomicUsize,
}
impl ReadCache {
    pub fn new(config: &Config) -> Self {
        Self {
            files: super::file_changes::FileChanges::new(config),
            records: Default::default(),
            ids: Default::default(),
            lists: Default::default(),
            prepared: Default::default(),
            references: Default::default(),
            matchers: Default::default(),
            #[cfg(test)]
            parses: Default::default(),
        }
    }
}
impl Editor {
    pub(super) fn collection_signature(&self, name: &str) -> Result<Signature> {
        self.read_cache.files.signature(self, name)
    }

    pub(super) fn list_signature(
        &self,
        name: &str,
        ids: Option<&BTreeSet<String>>,
    ) -> Result<ListSignature> {
        let collection = self.config.collection(name)?;
        let all = self.collection_signature(name)?;
        let source = if let (IdStrategy::Filename, Some(ids)) = (&collection.id, ids) {
            for id in ids {
                safe_name(id)?;
            }
            all.iter()
                .filter(|(path, _)| {
                    path.file_stem()
                        .and_then(|s| s.to_str())
                        .is_some_and(|id| ids.contains(id))
                })
                .cloned()
                .collect()
        } else {
            all
        };
        let mut dependencies = BTreeMap::new();
        for target in self.list_dependency_names(name)? {
            dependencies.insert(target.clone(), self.collection_signature(&target)?);
        }
        Ok(ListSignature {
            source,
            dependencies,
        })
    }

    fn list_dependency_names(&self, name: &str) -> Result<BTreeSet<String>> {
        fn derived_targets(collection: &Collection) -> BTreeSet<String> {
            let props = collection.schema_value["properties"].as_object();
            props
                .into_iter()
                .flatten()
                .filter_map(|(_, schema)| {
                    let via = schema.pointer("/x-derived/via")?.as_str()?;
                    props?
                        .get(via)?
                        .pointer("/x-relation/collection")?
                        .as_str()
                        .map(str::to_owned)
                })
                .collect()
        }
        let collection = self.config.collection(name)?;
        let mut pending = derived_targets(collection);
        if let Some(props) = collection.editor_schema["properties"].as_object() {
            for (key, schema) in props {
                if key != "_editor"
                    && schema["x-editor"] != "hidden"
                    && schema["x-visible"] != false
                    && schema["x-children"] != true
                    && let Some(target) = schema
                        .pointer("/x-relation/collection")
                        .and_then(Value::as_str)
                {
                    pending.insert(target.into());
                }
            }
        }
        let mut seen = BTreeSet::new();
        while let Some(target) = pending.pop_first() {
            if !seen.insert(target.clone()) {
                continue;
            }
            pending.extend(derived_targets(self.config.collection(&target)?));
        }
        seen.remove(name);
        Ok(seen)
    }
    pub(super) fn cached_document(&self, name: &str, path: &Path) -> Result<Arc<Document>> {
        let c = self.config.collection(name)?;
        let entry = {
            let mut cache = self.read_cache.records.lock().unwrap();
            if !cache.entries.contains_key(path) {
                if cache.entries.len() >= 10000
                    && let Some(oldest) = cache.order.pop_front()
                {
                    cache.entries.remove(&oldest);
                }
                cache.order.push_back(path.to_owned());
            }
            cache.entries.entry(path.to_owned()).or_default().clone()
        };
        let mut cache = entry.lock().unwrap();
        let before = stamp(path)?;
        if let Some((old, doc)) = cache.as_ref() {
            if old == &before {
                return Ok(doc.clone());
            }
        }
        let bytes = fs::read(path)?;
        let value = parse(c, &bytes)?;
        let doc = Arc::new(Document {
            id: record_id(c, path, &value)?,
            value,
            revision: hash(&bytes),
        });
        #[cfg(test)]
        self.read_cache
            .parses
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // Do not cache a read if the file changed while it was being read.
        if stamp(path)? == before {
            *cache = Some((before, doc.clone()));
        }
        Ok(doc)
    }
    pub(super) fn indexed_path(&self, name: &str, id: &str) -> Result<PathBuf> {
        let signature = self.collection_signature(name)?;
        let entry = self
            .read_cache
            .ids
            .lock()
            .unwrap()
            .entry(name.to_owned())
            .or_default()
            .clone();
        let mut cached = entry.lock().unwrap();
        if !cached.initialized || cached.signature != signature {
            let mut by_id = cached.by_id.clone();
            let mut by_path = cached.by_path.clone();
            let changed = super::listing::changed_paths(&cached.signature, &signature);
            for (path, _) in &changed {
                if let Some(old_id) = by_path.remove(*path) {
                    by_id.remove(&old_id);
                }
            }
            for (path, stamp) in changed {
                if stamp.is_some() {
                    let new_id = self.cached_document(name, path)?.id.clone();
                    if by_id.insert(new_id.clone(), path.clone()).is_some() {
                        return Err(Error::Data(format!("duplicate ID {new_id}")));
                    }
                    by_path.insert(path.clone(), new_id);
                }
            }
            cached.signature = signature;
            cached.by_id = by_id;
            cached.by_path = by_path;
            cached.initialized = true;
        }
        cached
            .by_id
            .get(id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("record {id}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::Ordering;

    fn fixture(id: &str) -> (tempfile::TempDir, Editor) {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join(".editor")).unwrap();
        fs::create_dir_all(t.path().join("data/items")).unwrap();
        fs::create_dir_all(t.path().join("media")).unwrap();
        let schema = json!({"type":"object","x-project":{"media":"media"},
            "x-collection":{"name":"items","label":"Items","path":"data/items","format":"json","id":id,"display":"name","search":["name"]},
            "properties":{"id":{"type":"string"},"name":{"type":"string"},"link":{"type":"string","x-relation":{"collection":"items"}}}});
        fs::write(
            t.path().join(".editor/schema.json"),
            json!({"x-project":{"media":"media"}, "oneOf":[schema]}).to_string(),
        )
        .unwrap();
        let e = Editor::open(t.path()).unwrap();
        (t, e)
    }
    fn write(t: &tempfile::TempDir, e: &Editor, file: &str, value: Value) {
        fs::write(
            t.path().join(format!("data/items/{file}.json")),
            value.to_string(),
        )
        .unwrap();
        // Explicit refresh makes external-write tests independent of watcher scheduling.
        e.refresh_read_cache();
    }
    #[test]
    fn regex_queries_share_normalized_text_and_page_only_highlights() {
        let (t, _) = fixture("filename");
        let schema_path = t.path().join(".editor/schema.json");
        let mut schema: Value = serde_json::from_slice(&fs::read(&schema_path).unwrap()).unwrap();
        schema["oneOf"][0]["properties"]["name"]["x-content"] = json!("markdown");
        let e = {
            fs::write(&schema_path, schema.to_string()).unwrap();
            Editor::open(t.path()).unwrap()
        };
        write(
            &t,
            &e,
            "a",
            json!({"name":"**Über** [Alpha](https://example.test)"}),
        );
        write(&t, &e, "b", json!({"name":"Über Beta"}));
        let regex = crate::SearchOptions {
            regex: true,
            case_sensitive: false,
        };
        let request = ListRequest {
            query: r"^über\s+(alpha|beta)$".into(),
            search_options: regex,
            limit: 1,
            ..Default::default()
        };
        let page = e.query_collection("items", &request).unwrap();
        assert_eq!(page.matched, 2);
        assert_eq!(page.rows[0].display["name"], "Über Alpha");
        let m = page.rows[0]
            .matches
            .iter()
            .find(|m| m.field == "name")
            .unwrap();
        assert_eq!(&m.text[m.ranges[0].start..m.ranges[0].end], "Über Alpha");
        let parsed = e.read_cache.parses.load(Ordering::Relaxed);
        let scans = e.read_cache.files.scans.load(Ordering::Relaxed);
        let prepared = e.read_cache.prepared.lock().unwrap()[0].2.clone();
        for q in ["Beta", "Alpha", "nothing", "Über Alpha"] {
            e.query_collection(
                "items",
                &ListRequest {
                    query: q.into(),
                    limit: 1,
                    ..Default::default()
                },
            )
            .unwrap();
        }
        assert_eq!(
            e.search_relation("items", "Über Alpha", None, None, 0, 10)
                .unwrap()[0]
                .id,
            "a"
        );
        assert!(
            e.search_relation("items", "example.test", None, None, 0, 10)
                .unwrap()
                .is_empty()
        );
        assert_eq!(e.read_cache.parses.load(Ordering::Relaxed), parsed);
        assert_eq!(e.read_cache.files.scans.load(Ordering::Relaxed), scans);
        assert!(Arc::ptr_eq(
            &prepared,
            &e.read_cache.prepared.lock().unwrap()[0].2
        ));
        assert!(
            prepared
                .rows
                .iter()
                .all(|row| row.matches.is_empty() && row.highlights.is_empty())
        );
        assert!(matches!(
            e.query_collection(
                "items",
                &ListRequest {
                    query: "[".into(),
                    ..request.clone()
                }
            ),
            Err(Error::SearchPattern(_))
        ));
        assert_eq!(
            e.query_collection(
                "items",
                &ListRequest {
                    query: "^".into(),
                    ..request.clone()
                }
            )
            .unwrap()
            .matched,
            2
        );
        assert_eq!(
            e.query_collection(
                "items",
                &ListRequest {
                    query: "^über".into(),
                    search_options: crate::SearchOptions {
                        case_sensitive: true,
                        ..regex
                    },
                    ..request
                }
            )
            .unwrap()
            .matched,
            0
        );
    }

    #[test]
    fn retained_sort_orders_follow_edits_additions_and_deletions() {
        let (t, e) = fixture("filename");
        for i in 0..40 {
            write(
                &t,
                &e,
                &format!("item-{i}"),
                json!({"name": format!("Name {}", 40-i)}),
            );
        }
        let requests: Vec<_> = ["_id", "_label", "name"]
            .into_iter()
            .flat_map(|sort| {
                [false, true].map(|descending| ListRequest {
                    sort: sort.into(),
                    descending,
                    limit: 100,
                    ..Default::default()
                })
            })
            .collect();
        for request in &requests {
            e.query_collection("items", request).unwrap();
        }
        let doc = e.get("items", "item-12").unwrap();
        e.save(
            "items",
            "item-12",
            json!({"name":"A changed name"}),
            Some(&doc.revision),
        )
        .unwrap();
        e.save("items", "new", json!({"name":"Between"}), None)
            .unwrap();
        let doc = e.get("items", "item-15").unwrap();
        e.delete("items", &doc.id, &doc.revision).unwrap();
        let fresh = Editor::open(t.path()).unwrap();
        for request in &requests {
            let ids = |editor: &Editor| {
                editor
                    .query_collection("items", request)
                    .unwrap()
                    .rows
                    .into_iter()
                    .map(|row| row.id)
                    .collect::<Vec<_>>()
            };
            assert_eq!(ids(&e), ids(&fresh));
        }
    }

    #[test]
    fn rename_immediately_updates_cached_queries_and_reference_labels() {
        let (t, e) = fixture("frontmatter.id");
        write(&t, &e, "one", json!({"id":"a", "name":"Alpha"}));
        write(&t, &e, "two", json!({"id":"b", "name":"Bravo", "link":"a"}));
        let request = ListRequest {
            query: "^a$".into(),
            search_options: crate::SearchOptions {
                regex: true,
                ..Default::default()
            },
            limit: 10,
            ..Default::default()
        };
        assert_eq!(e.query_collection("items", &request).unwrap().matched, 1);
        let doc = e.get("items", "a").unwrap();
        let renamed = e
            .rename_with_references(
                "items",
                "a",
                "z",
                json!({"id":"z", "name":"Zulu"}),
                &doc.revision,
            )
            .unwrap();
        assert_eq!(renamed.updated_items, 1);
        assert_eq!(e.query_collection("items", &request).unwrap().matched, 0);
        let all = e
            .query_collection(
                "items",
                &ListRequest {
                    limit: 10,
                    ..Default::default()
                },
            )
            .unwrap();
        let referring = all.rows.iter().find(|row| row.id == "b").unwrap();
        assert_eq!(referring.value["link"], "z");
        assert_eq!(referring.display["link"], "Zulu");
    }

    #[test]
    #[ignore = "release benchmark; SEARCH_BENCH_RECORDS=100000 cargo test -p structured-editor-core --release measure_markdown_search -- --ignored --nocapture"]
    fn measure_markdown_search() {
        let count = std::env::var("SEARCH_BENCH_RECORDS")
            .ok()
            .and_then(|n| n.parse::<usize>().ok())
            .unwrap_or(10000);
        let (t, _) = fixture("frontmatter.id");
        let path = t.path().join(".editor/schema.json");
        let mut schema: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        schema["oneOf"][0]["x-collection"]["search"] = json!(["name", "body"]);
        schema["oneOf"][0]["properties"]["body"] = json!({"type":"string", "x-content":"markdown"});
        fs::write(path, schema.to_string()).unwrap();
        for i in 0..count {
            fs::write(t.path().join(format!("data/items/{i:06}.json")), json!({
                "id": format!("item-{i:06}"), "name": format!("Record {i:06}"),
                "body": format!("# Entry {i}\n\n{}\n\nMarker {i:06}", "Some **Markdown** with [readable links](https://example.test), `code`, Über and Ελληνικά. ".repeat(6)),
                "link": format!("item-{:06}", i.saturating_sub(1))
            }).to_string()).unwrap();
        }
        let e = Editor::open(t.path()).unwrap();
        let request = ListRequest {
            limit: 50,
            ..Default::default()
        };
        let start = Instant::now();
        e.query_collection("items", &request).unwrap();
        let cold = start.elapsed();
        let start = Instant::now();
        for page in 0..10 {
            e.query_collection(
                "items",
                &ListRequest {
                    offset: page * 50,
                    ..request.clone()
                },
            )
            .unwrap();
        }
        eprintln!(
            "{count} Markdown records with frontmatter relations: cold {cold:?}, cached page {:?}",
            start.elapsed() / 10
        );
        for (query, regex) in [
            ("Marker 000019", false),
            ("Markdown", false),
            ("absent phrase", false),
            (r"Marker 00\d{4}", true),
            (r"\p{Greek}{3,}", true),
        ] {
            let start = Instant::now();
            let page = e
                .query_collection(
                    "items",
                    &ListRequest {
                        query: query.into(),
                        search_options: crate::SearchOptions {
                            regex,
                            case_sensitive: false,
                        },
                        ..request.clone()
                    },
                )
                .unwrap();
            eprintln!(
                "  {query:?}, regex={regex}: {:?}, {} matches",
                start.elapsed(),
                page.matched
            );
        }
        let parses = e.read_cache.parses.load(Ordering::Relaxed);
        let doc = e.get("items", "item-000010").unwrap();
        let mut value = doc.value;
        value["name"] = json!("Changed record");
        e.save("items", &doc.id, value, Some(&doc.revision))
            .unwrap();
        let start = Instant::now();
        e.query_collection("items", &request).unwrap();
        eprintln!(
            "  refresh after single edit: {:?}, {} additional parses",
            start.elapsed(),
            e.read_cache.parses.load(Ordering::Relaxed) - parses
        );
    }
    #[test]
    #[ignore = "manual filesystem timing; run with --ignored --nocapture"]
    fn measure_cold_and_cached_lists() {
        let (t, e) = fixture("filename");
        for i in 0..2000 {
            write(
                &t,
                &e,
                &format!("item-{i:04}"),
                json!({"name":format!("Record {i:04}"),"description":"Some content to parse and retain. ".repeat(20)}),
            );
        }
        let request = ListRequest {
            limit: 50,
            ..Default::default()
        };
        let start = Instant::now();
        e.query_collection("items", &request).unwrap();
        let cold = start.elapsed();
        let parses = e.read_cache.parses.load(Ordering::Relaxed);
        let start = Instant::now();
        for page in 1..=10 {
            e.query_collection(
                "items",
                &ListRequest {
                    offset: page * 50,
                    ..request.clone()
                },
            )
            .unwrap();
        }
        let page_average = start.elapsed() / 10;
        let start = Instant::now();
        e.query_collection(
            "items",
            &ListRequest {
                query: "019".into(),
                ..request
            },
        )
        .unwrap();
        let search = start.elapsed();
        assert_eq!(e.read_cache.parses.load(Ordering::Relaxed), parses);
        eprintln!(
            "2,000 records: cold list {cold:?}; cached page average {page_average:?}; new search over prepared values {search:?}; additional parses: 0"
        );
    }
    #[test]
    fn pages_reuse_results_and_external_changes_invalidate_them() {
        let (t, e) = fixture("filename");
        write(&t, &e, "a", json!({"name":"Alpha"}));
        write(&t, &e, "b", json!({"name":"Bravo"}));
        let request = ListRequest {
            limit: 1,
            ..Default::default()
        };
        assert_eq!(
            e.query_collection("items", &request).unwrap().rows[0].id,
            "a"
        );
        let cached = e.read_cache.lists.lock().unwrap()[0].2.clone();
        let parses = e.read_cache.parses.load(Ordering::Relaxed);
        let next = ListRequest {
            offset: 1,
            ..request.clone()
        };
        assert_eq!(e.query_collection("items", &next).unwrap().rows[0].id, "b");
        assert!(Arc::ptr_eq(
            &cached,
            &e.read_cache.lists.lock().unwrap()[0].2
        ));
        assert_eq!(e.read_cache.parses.load(Ordering::Relaxed), parses);
        let prepared = e.read_cache.prepared.lock().unwrap()[0].2.clone();
        let search = ListRequest {
            query: "Bravo".into(),
            ..request.clone()
        };
        assert_eq!(
            e.query_collection("items", &search).unwrap().rows[0].id,
            "b"
        );
        assert!(Arc::ptr_eq(
            &prepared,
            &e.read_cache.prepared.lock().unwrap()[0].2
        ));
        assert_eq!(e.read_cache.parses.load(Ordering::Relaxed), parses);
        // Same-length rewrite, then an atomic file replacement, addition and deletion.
        write(&t, &e, "a", json!({"name":"Zulu!"}));
        assert_eq!(
            e.query_collection("items", &request).unwrap().rows[0].id,
            "b"
        );
        write(&t, &e, "replacement", json!({"name":"AAAAA"}));
        fs::rename(
            t.path().join("data/items/replacement.json"),
            t.path().join("data/items/a.json"),
        )
        .unwrap();
        e.refresh_read_cache();
        assert_eq!(
            e.query_collection("items", &request).unwrap().rows[0].id,
            "a"
        );
        write(&t, &e, "c", json!({"name":"Charlie"}));
        assert_eq!(e.query_collection("items", &request).unwrap().total, 3);
        fs::remove_file(t.path().join("data/items/a.json")).unwrap();
        e.refresh_read_cache();
        assert_eq!(e.query_collection("items", &request).unwrap().total, 2);
        // Internal writes must also invalidate cached rows immediately.
        let b = e.get("items", "b").unwrap();
        e.save("items", "b", json!({"name":"ZZZZZ"}), Some(&b.revision))
            .unwrap();
        assert_eq!(
            e.query_collection("items", &request).unwrap().rows[0].id,
            "c"
        );
    }
    #[test]
    fn changed_record_rebuilds_only_its_row_and_rows_that_display_it() {
        let (t, e) = fixture("filename");
        write(&t, &e, "a", json!({"name":"Alpha"}));
        write(&t, &e, "b", json!({"name":"Bravo","link":"a"}));
        write(&t, &e, "c", json!({"name":"Charlie"}));
        let request = ListRequest {
            limit: 10,
            ..Default::default()
        };
        e.query_collection("items", &request).unwrap();
        let previous = e.read_cache.prepared.lock().unwrap()[0].2.clone();
        let scans = e.read_cache.parses.load(Ordering::Relaxed);
        write(&t, &e, "a", json!({"name":"ALTER"}));
        let page = e.query_collection("items", &request).unwrap();
        let current = e.read_cache.prepared.lock().unwrap()[0].2.clone();
        let path = |id: &str| t.path().join(format!("data/items/{id}.json"));
        for id in ["a", "b"] {
            assert!(!Arc::ptr_eq(
                previous.by_path[&path(id)].row.as_ref().unwrap(),
                current.by_path[&path(id)].row.as_ref().unwrap(),
            ));
        }
        assert!(Arc::ptr_eq(
            previous.by_path[&path("c")].row.as_ref().unwrap(),
            current.by_path[&path("c")].row.as_ref().unwrap(),
        ));
        assert_eq!(e.read_cache.parses.load(Ordering::Relaxed), scans + 1);
        assert_eq!(
            page.rows.iter().find(|r| r.id == "b").unwrap().display["link"],
            "ALTER"
        );
    }
    #[test]
    fn unrelated_target_changes_keep_prepared_rows_and_search_results() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join(".editor")).unwrap();
        fs::create_dir_all(t.path().join("data/items")).unwrap();
        fs::create_dir_all(t.path().join("data/people")).unwrap();
        fs::create_dir_all(t.path().join("media")).unwrap();
        let item = json!({"type":"object", "x-collection":{"name":"items","label":"Items","path":"data/items","format":"json","id":"filename","display":"name","search":["name"]},
            "properties":{"name":{"type":"string"},"person":{"type":"string","x-relation":{"collection":"people"}}}});
        let person = json!({"type":"object", "x-collection":{"name":"people","label":"People","path":"data/people","format":"json","id":"filename","display":"name","search":["name"]},
            "properties":{"name":{"type":"string"}}});
        fs::write(
            t.path().join(".editor/schema.json"),
            json!({"x-project":{"media":"media"},"oneOf":[item, person]}).to_string(),
        )
        .unwrap();
        fs::write(
            t.path().join("data/items/a.json"),
            json!({"name":"Alpha","person":"x"}).to_string(),
        )
        .unwrap();
        fs::write(
            t.path().join("data/items/b.json"),
            json!({"name":"Bravo"}).to_string(),
        )
        .unwrap();
        fs::write(
            t.path().join("data/people/x.json"),
            json!({"name":"Xavier"}).to_string(),
        )
        .unwrap();
        fs::write(
            t.path().join("data/people/y.json"),
            json!({"name":"Yvonne"}).to_string(),
        )
        .unwrap();
        let e = Editor::open(t.path()).unwrap();
        let request = ListRequest {
            query: "Alpha".into(),
            ..Default::default()
        };
        e.query_collection("items", &request).unwrap();
        let old_rows = e.read_cache.prepared.lock().unwrap()[0].2.clone();
        let old_page = e.read_cache.lists.lock().unwrap()[0].2.clone();
        fs::write(
            t.path().join("data/people/y.json"),
            json!({"name":"Yara"}).to_string(),
        )
        .unwrap();
        e.refresh_read_cache();
        e.query_collection("items", &request).unwrap();
        let new_rows = e.read_cache.prepared.lock().unwrap()[0].2.clone();
        let new_page = e.read_cache.lists.lock().unwrap()[0].2.clone();
        assert!(Arc::ptr_eq(&old_page, &new_page));
        assert!(Arc::ptr_eq(&old_rows.rows[0], &new_rows.rows[0]));
        assert!(Arc::ptr_eq(&old_rows.rows[1], &new_rows.rows[1]));
        fs::write(
            t.path().join("data/people/x.json"),
            json!({"name":"Xena"}).to_string(),
        )
        .unwrap();
        e.refresh_read_cache();
        let page = e.query_collection("items", &request).unwrap();
        assert_eq!(page.rows[0].display["person"], "Xena");
        let updated = e.read_cache.prepared.lock().unwrap()[0].2.clone();
        assert!(!Arc::ptr_eq(&new_rows.rows[0], &updated.rows[0]));
        assert!(Arc::ptr_eq(&new_rows.rows[1], &updated.rows[1]));
    }
    #[test]
    fn child_lists_only_parse_members_and_concurrent_reads_share_parsing() {
        let (t, e) = fixture("filename");
        write(&t, &e, "a", json!({"name":"Alpha"}));
        fs::write(t.path().join("data/items/b.json"), "invalid json").unwrap();
        let request = ListRequest {
            ids: Some(BTreeSet::from(["a".into()])),
            limit: 10,
            ..Default::default()
        };
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let request = &request;
                let e = &e;
                scope.spawn(move || {
                    assert_eq!(e.query_collection("items", request).unwrap().rows.len(), 1)
                });
            }
        });
        assert_eq!(e.read_cache.parses.load(Ordering::Relaxed), 1);
        let with_missing = ListRequest {
            ids: Some(BTreeSet::from(["a".into(), "missing".into()])),
            ..request.clone()
        };
        assert_eq!(
            e.query_collection("items", &with_missing)
                .unwrap()
                .rows
                .len(),
            1
        );
        write(&t, &e, "missing", json!({"name":"Found"}));
        assert_eq!(
            e.query_collection("items", &with_missing)
                .unwrap()
                .rows
                .len(),
            2
        );
        assert!(
            e.query_collection("items", &ListRequest::default())
                .is_err()
        );
    }
    #[test]
    fn relation_picker_reuses_parsed_records_and_refreshes_changed_files() {
        let (t, e) = fixture("filename");
        write(&t, &e, "a", json!({"name":"Alpha"}));
        write(&t, &e, "b", json!({"name":"Bravo"}));
        assert_eq!(
            e.search_relation("items", "Alpha", None, None, 0, 10)
                .unwrap()[0]
                .id,
            "a"
        );
        let parses = e.read_cache.parses.load(Ordering::Relaxed);
        assert_eq!(
            e.search_relation("items", "Bravo", None, None, 0, 10)
                .unwrap()[0]
                .id,
            "b"
        );
        assert_eq!(e.read_cache.parses.load(Ordering::Relaxed), parses);
        write(&t, &e, "a", json!({"name":"Updated"}));
        assert_eq!(
            e.search_relation("items", "Updated", None, None, 0, 10)
                .unwrap()[0]
                .id,
            "a"
        );
        assert_eq!(e.read_cache.parses.load(Ordering::Relaxed), parses + 1);
    }
    #[test]
    fn id_and_reverse_indexes_follow_external_edits_and_guard_deletion() {
        let (t, e) = fixture("frontmatter.id");
        write(&t, &e, "one", json!({"id":"a","name":"Alpha"}));
        write(&t, &e, "two", json!({"id":"b","name":"Bravo","link":"a"}));
        assert_eq!(e.get("items", "a").unwrap().value["name"], "Alpha");
        let parsed_before = e.read_cache.parses.load(Ordering::Relaxed);
        assert_eq!(e.references_to("items", "a").unwrap().len(), 1);
        let list = ListRequest {
            limit: 10,
            ..Default::default()
        };
        assert_eq!(
            e.query_collection("items", &list)
                .unwrap()
                .rows
                .iter()
                .find(|r| r.id == "b")
                .unwrap()
                .display["link"],
            "Alpha"
        );
        write(&t, &e, "one", json!({"id":"a","name":"ALTER"}));
        assert_eq!(e.get("items", "a").unwrap().value["name"], "ALTER");
        assert_eq!(
            e.read_cache.parses.load(Ordering::Relaxed),
            parsed_before + 1
        );
        assert_eq!(
            e.query_collection("items", &list)
                .unwrap()
                .rows
                .iter()
                .find(|r| r.id == "b")
                .unwrap()
                .display["link"],
            "ALTER"
        );
        let scans_before = e.read_cache.references.lock().unwrap().scans;
        e.references_to("items", "a").unwrap();
        assert_eq!(
            e.read_cache.references.lock().unwrap().scans,
            scans_before + 1
        );
        let parses = e.read_cache.parses.load(Ordering::Relaxed);
        let scans = e.read_cache.references.lock().unwrap().scans;
        assert!(e.references_to("items", "b").unwrap().is_empty());
        assert_eq!(e.read_cache.references.lock().unwrap().scans, scans);
        e.get("items", "b").unwrap();
        assert_eq!(e.read_cache.parses.load(Ordering::Relaxed), parses);
        write(&t, &e, "two", json!({"id":"c","name":"Bravo"}));
        assert!(e.get("items", "b").is_err());
        assert!(e.get("items", "c").is_ok());
        assert!(e.references_to("items", "a").unwrap().is_empty());
        write(&t, &e, "two", json!({"id":"c","name":"Bravo","link":"a"}));
        let a = e.get("items", "a").unwrap();
        assert!(e.delete("items", "a", &a.revision).is_err());
        write(&t, &e, "three", json!({"id":"c","name":"Duplicate"}));
        assert!(matches!(e.get("items", "c"), Err(Error::Data(_))));
        fs::remove_file(t.path().join("data/items/three.json")).unwrap();
        e.refresh_read_cache();
        assert!(e.get("items", "c").is_ok());
    }
    #[test]
    fn reverse_index_rescans_only_changed_records_and_derived_dependents() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join(".editor")).unwrap();
        fs::create_dir_all(t.path().join("data/items")).unwrap();
        fs::create_dir_all(t.path().join("data/groups")).unwrap();
        fs::create_dir_all(t.path().join("media")).unwrap();
        let items = json!({"type":"object", "x-collection":{"name":"items","label":"Items","path":"data/items","format":"json","id":"filename","display":"name","search":["name"]},
            "properties":{"name":{"type":"string"},"group":{"type":"string","x-relation":{"collection":"groups"}},
                "derived":{"type":"string","x-relation":{"collection":"items"},"x-derived":{"via":"group","field":"owner"}}}});
        let groups = json!({"type":"object", "x-collection":{"name":"groups","label":"Groups","path":"data/groups","format":"json","id":"filename","display":"name","search":["name"]},
            "properties":{"name":{"type":"string"},"owner":{"type":"string","x-relation":{"collection":"items"}}}});
        fs::write(
            t.path().join(".editor/schema.json"),
            json!({"x-project":{"media":"media"},"oneOf":[items, groups]}).to_string(),
        )
        .unwrap();
        for (path, value) in [
            ("data/items/a.json", json!({"name":"Alpha"})),
            ("data/items/b.json", json!({"name":"Bravo"})),
            (
                "data/items/ref.json",
                json!({"name":"Reference","group":"g"}),
            ),
            ("data/groups/g.json", json!({"name":"Group","owner":"a"})),
        ] {
            fs::write(t.path().join(path), value.to_string()).unwrap();
        }
        let e = Editor::open(t.path()).unwrap();
        assert_eq!(e.references_to("items", "a").unwrap().len(), 2);
        let scans = e.read_cache.references.lock().unwrap().scans;
        assert_eq!(scans, 4);
        fs::write(
            t.path().join("data/groups/g.json"),
            json!({"name":"Group","owner":"b"}).to_string(),
        )
        .unwrap();
        assert!(e.references_to("items", "a").unwrap().is_empty());
        assert_eq!(e.references_to("items", "b").unwrap().len(), 2);
        assert_eq!(e.read_cache.references.lock().unwrap().scans, scans + 2);
    }
}
