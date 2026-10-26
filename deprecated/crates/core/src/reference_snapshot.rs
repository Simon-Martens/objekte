//! Optional on-disk backlink cache. Source stamps are reconciled before use;
//! invalid/corrupt snapshots are discarded and never affect project content.
use super::*;

const VERSION: u32 = 1;
#[derive(Serialize, Deserialize)]
struct Snapshot {
    version: u32,
    fingerprint: String,
    signatures: BTreeMap<String, Vec<(PathBuf, Option<read_cache::Stamp>)>>,
    documents: Vec<(PathBuf, Option<read_cache::Stamp>)>,
    sources: Vec<Source>,
}
#[derive(Serialize, Deserialize)]
struct Source {
    path: PathBuf,
    stamp: read_cache::Stamp,
    id: Option<(String, String)>,
    dependencies: BTreeSet<(String, String)>,
    references: Vec<((String, String), Vec<Reference>)>,
}
impl Editor {
    /// Keep the optional reverse-link index in administrative storage, outside
    /// project content. All collection/file/schema changes are checked on reuse.
    pub fn enable_reference_snapshot(&self, directory: &Path) -> Result<()> {
        fs::create_dir_all(directory)?;
        let directory = directory.canonicalize()?;
        if directory.starts_with(&self.config.root) {
            return Err(Error::Config(
                "reference snapshots must be outside project content".into(),
            ));
        }
        let collections: Vec<_> = self
            .config
            .collections
            .iter()
            .map(|(name, c)| serde_json::json!([name, c, c.schema_value]))
            .collect();
        let documents: Vec<_> = self
            .config
            .documents
            .iter()
            .map(|(name, d)| serde_json::json!([name, d.files, d.schema]))
            .collect();
        let settings = self
            .config
            .settings
            .as_ref()
            .map(|s| serde_json::json!([s.path, s.schema]));
        let fingerprint = hash(
            &serde_json::to_vec(&serde_json::json!([
                self.config.root,
                collections,
                documents,
                settings
            ]))
            .map_err(|e| Error::Data(e.to_string()))?,
        );
        let path = directory.join(format!(
            "references-{}.json",
            hash(self.config.root.to_string_lossy().as_bytes())
        ));
        // Establish the same lock order as the writer.
        let mut cache = self.read_cache.references.lock().unwrap();
        let populated = !cache.signatures.is_empty();
        if !populated {
            if let Ok(bytes) = fs::read(&path) {
                if let Ok(snapshot) = serde_json::from_slice::<Snapshot>(&bytes) {
                    if snapshot.version == VERSION && snapshot.fingerprint == fingerprint {
                        for source in snapshot.sources {
                            cache.replace(
                                source.path,
                                source.stamp,
                                source.references.into_iter().collect(),
                                source.id,
                                source.dependencies,
                            );
                        }
                        cache.signatures = snapshot
                            .signatures
                            .into_iter()
                            .map(|(name, files)| (name, files.into_iter().collect()))
                            .collect();
                        cache.documents = snapshot.documents.into_iter().collect();
                    }
                }
            }
        }
        *self.read_cache.reference_file.lock().unwrap() = Some((path, fingerprint));
        if populated {
            self.persist_reference_index(&cache);
        }
        Ok(())
    }

    pub(super) fn persist_reference_index(&self, cache: &read_cache::ReferenceCache) {
        let file = self.read_cache.reference_file.lock().unwrap();
        let Some((path, fingerprint)) = file.as_ref() else {
            return;
        };
        let snapshot = Snapshot {
            version: VERSION,
            fingerprint: fingerprint.clone(),
            signatures: cache
                .signatures
                .iter()
                .map(|(name, files)| (name.clone(), files.iter().cloned().collect()))
                .collect(),
            documents: cache.documents.iter().cloned().collect(),
            sources: cache
                .stamps
                .iter()
                .map(|(path, stamp)| Source {
                    path: path.clone(),
                    stamp: stamp.clone(),
                    id: cache.file_ids.get(path).cloned(),
                    dependencies: cache.dependencies.get(path).cloned().unwrap_or_default(),
                    references: cache
                        .contributions
                        .get(path)
                        .into_iter()
                        .flatten()
                        .filter_map(|target| {
                            Some((
                                target.clone(),
                                cache.by_target.get(target)?.get(path)?.clone(),
                            ))
                        })
                        .collect(),
                })
                .collect(),
        };
        let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        let write = || -> Result<()> {
            let bytes = serde_json::to_vec(&snapshot).map_err(|e| Error::Data(e.to_string()))?;
            fs::write(&temporary, bytes)?;
            fs::rename(&temporary, path)?;
            Ok(())
        };
        // Cache persistence is optional; source reads/writes still succeed if
        // the cache directory is read-only or the disk fills up.
        if write().is_err() {
            let _ = fs::remove_file(&temporary);
        }
    }
}
