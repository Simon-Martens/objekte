//! Explicit source changes: a full diff or edits reported by the application.

use super::*;


impl Repository {
    /// Compare one collection's source files with its index and apply every
    /// created, changed, and deleted record. Moves appear as a deletion and a
    /// creation. An existing disk cache is rewritten when anything changed.
    /// Without an index in this handle, a saved disk cache is restored and
    /// diffed against the source files; without one, the index is built and
    /// there is nothing to compare.
    pub fn diff(&mut self, collection: &str) -> Result<CollectionChanges> {
        if self.selected_route(&[collection.to_owned()])?.is_some() {
            return Err(Error::Request(
                "diff handles native JSON picker changes only; use refresh for selected providers"
                    .into(),
            ));
        }
        let c = self.collection(collection)?;
        if self.options.cache != CacheMode::Memory {
            return Ok(CollectionChanges::default());
        }
        if !self.file_pickers.contains_key(collection)
            && !self.restore_cached_index_unchecked(collection)?
        {
            // A fresh build reads every source file, so no observed edge
            // from before it can be trusted.
            self.memory_edges
                .retain(|(owner, _), _| owner != collection);
            self.ensure_picker(&c)?;
            return Ok(CollectionChanges {
                indexed: true,
                ..Default::default()
            });
        }
        let changes = self
            .file_pickers
            .get_mut(collection)
            .expect("picker exists")
            .refresh_changes()?;
        self.memory_coverage.insert(c.name, "current".into());
        self.publish_source_changes(collection, changes)
    }

    /// Diff every collection in name order.
    pub fn diff_all(&mut self) -> Result<BTreeMap<String, CollectionChanges>> {
        let names = self.model.collections.keys().cloned().collect::<Vec<_>>();
        names
            .iter()
            .map(|name| Ok((name.clone(), self.diff(name)?)))
            .collect()
    }

    /// Apply files the application created, changed, deleted, or moved. Each
    /// named file is checked against its current state on disk, so the report
    /// lists what actually changed. Collections without an index in this
    /// handle are skipped; their next load diffs any disk cache. All paths
    /// are resolved before any index changes.
    pub fn apply_source_changes(
        &mut self,
        changes: &[SourceChange],
    ) -> Result<BTreeMap<String, CollectionChanges>> {
        let mut touched = BTreeMap::<String, BTreeSet<String>>::new();
        for change in changes {
            let paths = match change {
                SourceChange::Created(path)
                | SourceChange::Changed(path)
                | SourceChange::Deleted(path) => vec![path],
                SourceChange::Moved { from, to } => vec![from, to],
            };
            for path in paths {
                let (collection, id) = self.locate_source(path)?;
                touched.entry(collection).or_default().insert(id);
            }
        }
        for collection in touched.keys() {
            if self
                .selected_route(std::slice::from_ref(collection))?
                .is_some()
            {
                return Err(Error::Request("apply_source_changes handles native JSON paths only; use refresh for selected providers".into()));
            }
        }
        let mut report = BTreeMap::new();
        for (collection, ids) in touched {
            let outcome = match self.file_pickers.get_mut(&collection) {
                Some(picker) if self.options.cache == CacheMode::Memory => {
                    let ids = ids.into_iter().collect::<Vec<_>>();
                    let changes = picker.refresh_ids(&ids)?;
                    self.publish_source_changes(&collection, changes)?
                }
                _ => {
                    // Observed reference edges exist without an index too.
                    for id in ids {
                        self.memory_edges.remove(&(collection.clone(), id));
                    }
                    CollectionChanges::default()
                }
            };
            report.insert(collection, outcome);
        }
        Ok(report)
    }

    /// Invalidate dependent caches and rewrite an existing disk cache.
    fn publish_source_changes(
        &mut self,
        collection: &str,
        changes: SourceChanges,
    ) -> Result<CollectionChanges> {
        let mut saved_cache = None;
        if !changes.is_empty() {
            self.query_cache.invalidate(collection);
            self.invalidate_representation_indexes(collection);
            for id in changes.changed.iter().chain(&changes.deleted) {
                self.memory_edges
                    .remove(&(collection.to_owned(), id.clone()));
            }
            let path = self.bigram_cache_path(collection)?;
            let picker = self
                .file_pickers
                .get_mut(collection)
                .expect("picker exists");
            if picker.loaded_snapshot() || path.is_file() {
                if !picker.can_save_bigram_snapshot() {
                    picker.rebuild_index()?;
                }
                picker.save_bigram_snapshot(&path)?;
                saved_cache = Some(path);
            }
            self.reconcile_extension_selection(collection)?;
        }
        Ok(CollectionChanges {
            changes,
            indexed: true,
            saved_cache,
        })
    }

    /// Map a source file path to its owning collection and record ID.
    fn locate_source(&self, path: &Path) -> Result<(String, String)> {
        let unknown = || {
            Error::Request(format!(
                "{} is not a JSON record in any collection",
                path.display()
            ))
        };
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.model.root.join(path)
        };
        if let Some(found) = self.owning_collection(&absolute, false)? {
            return Ok(found);
        }
        // A symlinked or differently spelled root: compare canonical paths.
        // The parent is canonicalized because a deleted file no longer exists.
        let (Some(parent), Some(name)) = (absolute.parent(), absolute.file_name()) else {
            return Err(unknown());
        };
        let Ok(parent) = parent.canonicalize() else {
            return Err(unknown());
        };
        self.owning_collection(&parent.join(name), true)?
            .ok_or_else(unknown)
    }

    fn owning_collection(&self, file: &Path, canonical: bool) -> Result<Option<(String, String)>> {
        for collection in self.model.collections.values() {
            let base = if canonical {
                match collection.path.canonicalize() {
                    Ok(base) => base,
                    Err(_) => continue,
                }
            } else {
                collection.path.clone()
            };
            let Ok(relative) = file.strip_prefix(&base) else {
                continue;
            };
            let Some(id) = relative
                .to_str()
                .and_then(|text| text.strip_suffix(".json"))
            else {
                continue;
            };
            let id = id.replace('\\', "/");
            if !collection.recursive && id.contains('/') {
                continue;
            }
            validate_id(&id)?;
            return Ok(Some((collection.name.clone(), id)));
        }
        Ok(None)
    }
}
