//! Watcher events are hints; periodic and explicit reconciliation repair missed events.
use super::*;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

#[derive(Default)]
struct Changes {
    paths: BTreeSet<PathBuf>,
    rescan: bool,
}
struct Snapshot {
    signature: read_cache::Signature,
    dirty: BTreeSet<PathBuf>,
    scanned: Instant,
    rescan: bool,
}
pub(super) struct FileChanges {
    changes: Arc<Mutex<Changes>>,
    snapshots: Mutex<HashMap<String, Snapshot>>,
    _watcher: Mutex<Option<RecommendedWatcher>>,
    interval: Duration,
    #[cfg(test)]
    pub scans: std::sync::atomic::AtomicUsize,
}
impl FileChanges {
    pub fn new(config: &Config) -> Self {
        let changes = Arc::new(Mutex::new(Changes::default()));
        let pending = changes.clone();
        let watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
            if matches!(&event, Ok(event) if matches!(event.kind, EventKind::Access(_)) && !event.need_rescan()) {
                return;
            }
            let mut pending = pending.lock().unwrap();
            match event {
                Ok(event) if event.need_rescan() => pending.rescan = true,
                Ok(event) if !matches!(event.kind, EventKind::Access(_)) => {
                    pending.paths.extend(event.paths);
                    if pending.paths.len() > 4096 {
                        pending.paths.clear();
                        pending.rescan = true;
                    }
                }
                Err(_) => pending.rescan = true,
                _ => {}
            }
        })
        .and_then(|mut watcher| {
            let dirs: BTreeSet<_> = config
                .collections
                .keys()
                .filter_map(|name| config.collection_dir(name).ok())
                .flat_map(|dir| [dir.parent().unwrap_or(&dir).to_owned(), dir])
                .collect();
            for dir in dirs {
                watcher.watch(&dir, RecursiveMode::NonRecursive)?;
            }
            Ok(watcher)
        })
        .ok();
        let interval = Duration::from_secs(if watcher.is_some() { 30 } else { 1 });
        Self {
            changes,
            snapshots: Mutex::new(HashMap::new()),
            _watcher: Mutex::new(watcher),
            interval,
            #[cfg(test)]
            scans: Default::default(),
        }
    }

    pub fn changed(&self, path: &Path) {
        let mut pending = self.changes.lock().unwrap();
        pending.paths.insert(path.to_owned());
        if pending.paths.len() > 4096 {
            pending.paths.clear();
            pending.rescan = true;
        }
    }

    pub fn refresh(&self) {
        self.changes.lock().unwrap().rescan = true;
    }

    fn reconcile_collection(&self, name: &str) {
        if let Some(snapshot) = self.snapshots.lock().unwrap().get_mut(name) {
            snapshot.rescan = true;
        }
    }

    pub fn signature(&self, editor: &Editor, name: &str) -> Result<read_cache::Signature> {
        let mut snapshots = self.snapshots.lock().unwrap();
        let changes = std::mem::take(&mut *self.changes.lock().unwrap());
        for (name, snapshot) in snapshots.iter_mut() {
            let dir = editor.config.collection_dir(name)?;
            snapshot.rescan |= changes.rescan;
            for path in &changes.paths {
                if dir.starts_with(path) {
                    snapshot.rescan = true;
                } else if path.parent() == Some(dir.as_path())
                    && path.extension().and_then(|s| s.to_str())
                        == Some(extension(editor.config.collection(name)?))
                {
                    snapshot.dirty.insert(path.clone());
                }
            }
        }
        let snapshot = snapshots.entry(name.into()).or_insert_with(|| Snapshot {
            signature: Default::default(),
            dirty: BTreeSet::new(),
            scanned: Instant::now(),
            rescan: true,
        });
        if snapshot.rescan || snapshot.scanned.elapsed() >= self.interval {
            let collection = editor.config.collection(name)?;
            let mut paths = Vec::new();
            for entry in fs::read_dir(editor.config.collection_dir(name)?)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|extension| extension.to_str())
                    != Some(extension(collection))
                {
                    continue;
                }
                let metadata = entry.metadata()?;
                let stamp = read_cache::stamp_metadata(&path, &metadata)?;
                if metadata.is_file() {
                    paths.push((path, Some(stamp)));
                }
            }
            paths.sort_by(|(a, _), (b, _)| a.cmp(b));
            let current: read_cache::Signature = paths.into_iter().collect();
            if snapshot.signature != current {
                snapshot.signature = current;
            }
            snapshot.dirty.clear();
            snapshot.rescan = false;
            snapshot.scanned = Instant::now();
            #[cfg(test)]
            self.scans
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        } else if !snapshot.dirty.is_empty() {
            let mut current = snapshot.signature.to_vec();
            for path in &snapshot.dirty {
                let stamp = match fs::symlink_metadata(path) {
                    Ok(m) if m.file_type().is_symlink() => {
                        return Err(Error::InvalidPath(path.display().to_string()));
                    }
                    Ok(m) if m.is_file() => Some(read_cache::stamp(path)?),
                    Ok(_) => None,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                    Err(e) => return Err(e.into()),
                };
                match (current.binary_search_by(|(p, _)| p.cmp(path)), stamp) {
                    (Ok(i), Some(stamp)) => current[i].1 = Some(stamp),
                    (Ok(i), None) => {
                        current.remove(i);
                    }
                    (Err(i), Some(stamp)) => current.insert(i, (path.clone(), Some(stamp))),
                    (Err(_), None) => {}
                }
            }
            if *snapshot.signature != current {
                snapshot.signature = current.into_iter().collect();
            }
            snapshot.dirty.clear();
        }
        Ok(snapshot.signature.clone())
    }
}

impl Editor {
    /// Reconcile external edits on the next query. Internal writes are immediate;
    /// external edits normally arrive through notify, with a 30s safety rescan
    /// (1s if watching is unavailable). Clients can call this for explicit refresh.
    pub fn refresh_read_cache(&self) {
        self.read_cache.files.refresh();
    }

    // ID uniqueness on writes must also see external edits not delivered by the
    // watcher yet. Filename IDs are checked directly at the target path.
    pub(super) fn reconcile_ids_for_write(&self, name: &str) -> Result<()> {
        if !matches!(self.config.collection(name)?.id, IdStrategy::Filename) {
            self.read_cache.files.reconcile_collection(name);
            self.collection_signature(name)?;
        }
        Ok(())
    }

    pub(crate) fn write_file(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        let result = atomic_write(path, bytes);
        self.read_cache.files.changed(path);
        result
    }
    pub(super) fn create_file(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        let result = atomic_create(path, bytes);
        self.read_cache.files.changed(path);
        result
    }
    pub(super) fn remove_file(&self, path: &Path) -> Result<()> {
        let result = fs::remove_file(path).map_err(Error::from);
        self.read_cache.files.changed(path);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn watcher_and_reconciliation_refresh_external_edits() {
        let t = tempfile::tempdir().unwrap();
        for dir in [".editor", "data", "media"] {
            fs::create_dir(t.path().join(dir)).unwrap();
        }
        fs::write(t.path().join(".editor/schema.json"), serde_json::json!({
            "x-project":{"media":"media"}, "oneOf":[{"type":"object",
            "x-collection":{"name":"items","label":"Items","path":"data","format":"json","id":"filename","display":"name","search":["name"]},
            "properties":{"name":{"type":"string"}}}]
        }).to_string()).unwrap();
        let path = t.path().join("data/a.json");
        fs::write(&path, r#"{"name":"Alpha"}"#).unwrap();
        let e = Editor::open(t.path()).unwrap();
        let request = ListRequest::default();
        assert_eq!(
            e.query_collection("items", &request).unwrap().rows[0].label,
            "Alpha"
        );
        fs::write(&path, r#"{"name":"Bravo"}"#).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if e.query_collection("items", &request).unwrap().rows[0].label == "Bravo" {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "watcher or fallback did not deliver update"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        // Simulate a missed event: force the periodic reconciliation deadline.
        fs::write(&path, r#"{"name":"Delta"}"#).unwrap();
        e.read_cache.files.changes.lock().unwrap().paths.clear();
        e.read_cache
            .files
            .snapshots
            .lock()
            .unwrap()
            .get_mut("items")
            .unwrap()
            .scanned = Instant::now() - Duration::from_secs(31);
        assert_eq!(
            e.query_collection("items", &request).unwrap().rows[0].label,
            "Delta"
        );
        // Overflow/error hints also force a full reconciliation.
        fs::remove_file(&path).unwrap();
        e.read_cache.files.changes.lock().unwrap().rescan = true;
        assert_eq!(e.query_collection("items", &request).unwrap().total, 0);
    }
}
