//! Metadata-validated read caches. No TTL: external edits are checked on every access.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Stamp {
    len: u64,
    modified: std::time::SystemTime,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}
fn stamp(path: &Path) -> Result<Stamp> {
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
pub(super) type Signature = Vec<(PathBuf, Stamp)>;
type RecordEntry = Arc<Mutex<Option<(Stamp, Arc<Document>)>>>;
type IdEntry = Arc<Mutex<Option<(Signature, Arc<HashMap<String, PathBuf>>)>>>;
type QueryCache<T> = Mutex<Vec<(Signature, String, Arc<T>)>>;
type ReferenceIndex = HashMap<(String, String), Vec<Reference>>;
#[derive(Default)]
pub(super) struct ReadCache {
    records: Mutex<HashMap<PathBuf, RecordEntry>>,
    ids: Mutex<HashMap<String, IdEntry>>,
    pub lists: QueryCache<ListPage>,
    pub prepared: QueryCache<Vec<ListRow>>,
    pub references: Mutex<Option<(Signature, Arc<ReferenceIndex>)>>,
    #[cfg(test)]
    pub parses: std::sync::atomic::AtomicUsize,
}
impl Editor {
    pub(super) fn collection_signature(&self, name: &str) -> Result<Signature> {
        let mut paths = self.files(name)?;
        paths.sort();
        paths
            .into_iter()
            .map(|p| Ok((p.clone(), stamp(&p)?)))
            .collect()
    }
    // Derived fields and relation labels can depend on any collection.
    pub(super) fn read_signature(&self) -> Result<Signature> {
        let mut signature = Vec::new();
        for name in self.config.collections.keys() {
            signature.extend(self.collection_signature(name)?);
        }
        Ok(signature)
    }
    pub(super) fn reference_signature(&self) -> Result<Signature> {
        let mut signature = self.read_signature()?;
        let mut files = BTreeSet::new();
        for mapping in self.config.documents.values() {
            files.extend(mapping.files.iter().cloned());
        }
        if let Some(settings) = &self.config.settings {
            files.insert(settings.path.clone());
        }
        for file in files {
            let path = self.config.root.join(&file);
            reject_symlink(&path)?;
            crate::contained(&self.config.root, &file)?;
            signature.push((path.clone(), stamp(&path)?));
        }
        Ok(signature)
    }
    pub(super) fn cached_document(&self, name: &str, path: &Path) -> Result<Arc<Document>> {
        let c = self.config.collection(name)?;
        let entry = {
            let mut cache = self.read_cache.records.lock().unwrap();
            if cache.len() >= 10000 && !cache.contains_key(path) {
                cache.clear();
            }
            cache.entry(path.to_owned()).or_default().clone()
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
        if let Some((old, index)) = cached.as_ref() {
            if old == &signature {
                return index
                    .get(id)
                    .cloned()
                    .ok_or_else(|| Error::NotFound(format!("record {id}")));
            }
        }
        let mut index = HashMap::new();
        for (path, _) in &signature {
            let doc = self.cached_document(name, path)?;
            if index.insert(doc.id.clone(), path.clone()).is_some() {
                return Err(Error::Data(format!("duplicate ID {}", doc.id)));
            }
        }
        let result = index.get(id).cloned();
        *cached = Some((signature, Arc::new(index)));
        result.ok_or_else(|| Error::NotFound(format!("record {id}")))
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
    fn write(t: &tempfile::TempDir, file: &str, value: Value) {
        fs::write(
            t.path().join(format!("data/items/{file}.json")),
            value.to_string(),
        )
        .unwrap();
    }
    #[test]
    #[ignore = "manual filesystem timing; run with --ignored --nocapture"]
    fn measure_cold_and_cached_lists() {
        let (t, e) = fixture("filename");
        for i in 0..2000 {
            write(
                &t,
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
        write(&t, "a", json!({"name":"Alpha"}));
        write(&t, "b", json!({"name":"Bravo"}));
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
        write(&t, "a", json!({"name":"Zulu!"}));
        assert_eq!(
            e.query_collection("items", &request).unwrap().rows[0].id,
            "b"
        );
        write(&t, "replacement", json!({"name":"AAAAA"}));
        fs::rename(
            t.path().join("data/items/replacement.json"),
            t.path().join("data/items/a.json"),
        )
        .unwrap();
        assert_eq!(
            e.query_collection("items", &request).unwrap().rows[0].id,
            "a"
        );
        write(&t, "c", json!({"name":"Charlie"}));
        assert_eq!(e.query_collection("items", &request).unwrap().total, 3);
        fs::remove_file(t.path().join("data/items/a.json")).unwrap();
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
    fn child_lists_only_parse_members_and_concurrent_reads_share_parsing() {
        let (t, e) = fixture("filename");
        write(&t, "a", json!({"name":"Alpha"}));
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
        assert!(
            e.query_collection("items", &ListRequest::default())
                .is_err()
        );
    }
    #[test]
    fn id_and_reverse_indexes_follow_external_edits_and_guard_deletion() {
        let (t, e) = fixture("frontmatter.id");
        write(&t, "one", json!({"id":"a","name":"Alpha"}));
        write(&t, "two", json!({"id":"b","name":"Bravo","link":"a"}));
        assert_eq!(e.get("items", "a").unwrap().value["name"], "Alpha");
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
        write(&t, "one", json!({"id":"a","name":"ALTER"}));
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
        e.references_to("items", "a").unwrap();
        let parses = e.read_cache.parses.load(Ordering::Relaxed);
        let index = e
            .read_cache
            .references
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .1
            .clone();
        assert!(e.references_to("items", "b").unwrap().is_empty());
        assert!(Arc::ptr_eq(
            &index,
            &e.read_cache.references.lock().unwrap().as_ref().unwrap().1
        ));
        e.get("items", "b").unwrap();
        assert_eq!(e.read_cache.parses.load(Ordering::Relaxed), parses);
        write(&t, "two", json!({"id":"c","name":"Bravo"}));
        assert!(e.get("items", "b").is_err());
        assert!(e.get("items", "c").is_ok());
        assert!(e.references_to("items", "a").unwrap().is_empty());
        write(&t, "two", json!({"id":"c","name":"Bravo","link":"a"}));
        let a = e.get("items", "a").unwrap();
        assert!(e.delete("items", "a", &a.revision).is_err());
        write(&t, "three", json!({"id":"c","name":"Duplicate"}));
        assert!(matches!(e.get("items", "c"), Err(Error::Data(_))));
        fs::remove_file(t.path().join("data/items/three.json")).unwrap();
        assert!(e.get("items", "c").is_ok());
    }
}
