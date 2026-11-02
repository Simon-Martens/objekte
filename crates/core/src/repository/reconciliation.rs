//! Source refresh.

use super::*;
use crate::JsonFilter;


impl Repository {
    /// Restore a saved picker without waiting for source metadata checks.
    /// Until `apply_collection_inventory` runs, searches may use stale postings.
    pub fn restore_cached_index_unchecked(&mut self, collection: &str) -> Result<bool> {
        self.reload()?;
        if self.options.cache != CacheMode::Memory
            || self.options.freshness != Freshness::MetadataOnOpen
        {
            return Ok(false);
        }
        let c = self.collection(collection)?;
        if self.file_pickers.contains_key(collection) {
            return Ok(true);
        }
        let path = self.bigram_cache_path(collection)?;
        if !path.is_file() {
            return Ok(false);
        }
        let started = Instant::now();
        let snapshot = match BigramSnapshot::load_scoped(&path, &c.path, c.recursive) {
            Ok(snapshot) => snapshot,
            Err(_) => return Ok(false),
        };
        if snapshot.filter.encoding() != c.bigram_encoding.unwrap_or(self.options.bigram_encoding) {
            return Ok(false);
        }
        let Some(picker) = crate::file_picker::FilePicker::open_cached_unchecked(
            &c.path,
            c.recursive,
            &c.indexes,
            &c.advanced_index_paths,
            &c.sort_indexes,
            &c.fields,
            c.schema
                .as_ref()
                .and_then(|schema| schema.field.language.as_deref()),
            snapshot,
            &path,
            c.bigram_encoding.unwrap_or(self.options.bigram_encoding),
        )?
        else {
            return Ok(false);
        };
        self.metrics.bigram_snapshot_loads += 1;
        self.metrics.secondary_snapshot_loads += u64::from(picker.secondary_loaded());
        self.metrics.bigram_open_nanos += nanos(started);
        self.file_pickers.insert(c.name, picker);
        Ok(true)
    }

    /// Check file metadata in a separate worker without locking the live picker.
    pub fn collect_collection_inventory(&self, collection: &str) -> Result<CollectionInventory> {
        let c = self.collection(collection)?;
        let source = Source::open_scoped(&c.path, c.recursive)?;
        let files = source.inventory()?;
        Ok(CollectionInventory {
            collection: c.name,
            path: c.path,
            recursive: c.recursive,
            files,
        })
    }

    /// Reconcile one collected inventory into the live picker and save changes.
    pub fn apply_collection_inventory(&mut self, inventory: CollectionInventory) -> Result<usize> {
        self.reload()?;
        let c = self.collection(&inventory.collection)?;
        if c.path != inventory.path || c.recursive != inventory.recursive {
            return Err(Error::Cache(
                "collection scope changed during inventory".into(),
            ));
        }
        let picker = self
            .file_pickers
            .get_mut(&inventory.collection)
            .ok_or_else(|| Error::Cache("collection index is unavailable".into()))?;
        let changed = picker.reconcile_inventory(inventory.files)?;
        if changed > 0 {
            self.autosave_loaded_bigram_cache(&inventory.collection)?;
        }
        Ok(changed)
    }

    pub(in crate::repository) fn autosave_loaded_bigram_cache(
        &mut self,
        collection: &str,
    ) -> Result<()> {
        let path = self.bigram_cache_path(collection)?;
        let picker = self
            .file_pickers
            .get_mut(collection)
            .expect("picker exists");
        if !picker.loaded_snapshot() {
            return Ok(());
        }
        if !picker.can_save_bigram_snapshot() {
            picker.rebuild_index()?;
        }
        let _ = picker.save_bigram_snapshot(&path);
        Ok(())
    }

    /// Build or rebuild an ordered posting for one JSON path in a collection.
    /// The path uses filter syntax, such as `year` or `contributors[0].type`.
    pub fn build_posting(&mut self, collection: &str, path: &str) -> Result<()> {
        let path = JsonFilter::parse_path(path)?;
        self.reload()?;
        let c = self.collection(collection)?;
        self.ensure_picker(&c)?;
        let picker = self
            .file_pickers
            .get_mut(collection)
            .expect("picker exists");
        picker.refresh()?;
        picker.build_posting(&path)?;
        if let Some(paths) = self.auto_postings.get_mut(collection) {
            paths.remove(&path);
        }
        self.requested_postings
            .entry(collection.to_owned())
            .or_default()
            .insert(path);
        Ok(())
    }

    /// Build or rebuild the same path posting in every registered collection.
    pub fn build_posting_all(&mut self, path: &str) -> Result<()> {
        JsonFilter::parse_path(path)?;
        self.reload()?;
        let names = self.model.collections.keys().cloned().collect::<Vec<_>>();
        for name in names {
            self.build_posting(&name, path)?;
        }
        Ok(())
    }

    /// Return the current or deleted record revision in this repository handle.
    pub fn record_revision(
        &mut self,
        collection: &str,
        id: &str,
    ) -> Result<Option<RecordRevision>> {
        self.reload()?;
        let c = self.collection(collection)?;
        validate_id(id)?;
        if self.options.cache == CacheMode::Disabled {
            return Ok(None);
        }
        self.ensure_picker(&c)?;
        Ok(self.file_pickers[collection].revision_record(id).map(
            |(revision, changed_at_nanos, present)| RecordRevision {
                collection: collection.into(),
                id: id.into(),
                revision,
                changed_at_nanos,
                present,
            },
        ))
    }

    /// Build the process-local bigram picker for one collection if needed.
    /// An existing picker follows the configured freshness policy.
    pub fn warm_bigram_index(&mut self, collection: &str) -> Result<Metrics> {
        self.reload()?;
        let c = self.collection(collection)?;
        self.warm_bigram_index_for(&c)
    }

    /// Save one collection's compressed bigram index and file-slot metadata.
    /// Call `rebuild_bigram_index` first when an in-memory overlay is present.
    pub fn save_bigram_snapshot(&mut self, collection: &str, path: &Path) -> Result<()> {
        self.reload()?;
        let c = self.collection(collection)?;
        self.ensure_picker(&c)?;
        self.file_pickers[collection].save_bigram_snapshot(path)
    }

    /// Save a collection's current bigram index in `.objekte/.cache`.
    pub fn save_bigram_cache(&mut self, collection: &str) -> Result<std::path::PathBuf> {
        self.reload()?;
        let path = self.bigram_cache_path(collection)?;
        let c = self.collection(collection)?;
        self.ensure_picker(&c)?;
        let picker = self
            .file_pickers
            .get_mut(collection)
            .expect("picker exists");
        if !picker.can_save_bigram_snapshot() {
            picker.refresh()?;
            picker.rebuild_index()?;
        }
        picker.save_bigram_snapshot(&path)?;
        Ok(path)
    }

    /// Save the current bigram indexes for every collection.
    pub fn save_all_bigram_caches(&mut self) -> Result<BTreeMap<String, std::path::PathBuf>> {
        self.reload()?;
        let names = self.model.collections.keys().cloned().collect::<Vec<_>>();
        names
            .iter()
            .map(|name| Ok((name.clone(), self.save_bigram_cache(name)?)))
            .collect()
    }

    /// Read every source record again, rebuild one index, and replace its cache.
    pub fn reindex_bigram_cache(&mut self, collection: &str) -> Result<std::path::PathBuf> {
        self.reload()?;
        let c = self.collection(collection)?;
        self.ensure_picker(&c)?;
        let picker = self
            .file_pickers
            .get_mut(collection)
            .expect("picker exists");
        picker.refresh()?;
        picker.rebuild_index()?;
        let path = self.bigram_cache_path(collection)?;
        self.file_pickers[collection].save_bigram_snapshot(&path)?;
        Ok(path)
    }

    /// Rebuild every collection and replace all saved bigram caches.
    pub fn reindex_all_bigram_caches(&mut self) -> Result<BTreeMap<String, std::path::PathBuf>> {
        self.reload()?;
        let names = self.model.collections.keys().cloned().collect::<Vec<_>>();
        names
            .iter()
            .map(|name| Ok((name.clone(), self.reindex_bigram_cache(name)?)))
            .collect()
    }

    /// Warm every collection's process-local bigram picker, in name order.
    /// Call the single-collection method to report progress between collections.
    pub fn warm_all_bigram_indexes(&mut self) -> Result<BTreeMap<String, Metrics>> {
        self.reload()?;
        let collections = self.model.collections.values().cloned().collect::<Vec<_>>();
        collections
            .iter()
            .map(|c| Ok((c.name.clone(), self.warm_bigram_index_for(c)?)))
            .collect()
    }

    /// Refresh source inventory and rebuild one collection's bigram and scalar
    /// indexes. Rebuilding an existing picker retains its record revisions and
    /// deletion tombstones.
    pub fn rebuild_bigram_index(&mut self, collection: &str) -> Result<Metrics> {
        self.reload()?;
        let c = self.collection(collection)?;
        self.rebuild_bigram_index_for(&c)
    }

    /// Rebuild every collection's process-local bigram and scalar indexes, in
    /// name order. Earlier collections remain rebuilt if a later one fails.
    pub fn rebuild_all_bigram_indexes(&mut self) -> Result<BTreeMap<String, Metrics>> {
        self.reload()?;
        let collections = self.model.collections.values().cloned().collect::<Vec<_>>();
        collections
            .iter()
            .map(|c| Ok((c.name.clone(), self.rebuild_bigram_index_for(c)?)))
            .collect()
    }

    fn warm_bigram_index_for(&mut self, c: &Collection) -> Result<Metrics> {
        let before = self.metrics.clone();
        self.ensure_picker(c)?;
        Ok(self.metrics.since(&before))
    }

    fn rebuild_bigram_index_for(&mut self, c: &Collection) -> Result<Metrics> {
        let before = self.metrics.clone();
        if let Some(picker) = self.file_pickers.get_mut(&c.name) {
            let started = Instant::now();
            picker.refresh()?;
            self.metrics.bigram_refresh_nanos += nanos(started);
            let started = Instant::now();
            picker.rebuild_index()?;
            self.metrics.bigram_rebuild_nanos += nanos(started);
        } else {
            self.ensure_picker(c)?;
        }
        Ok(self.metrics.since(&before))
    }

    pub fn refresh(&mut self, collection: &str) -> Result<Metrics> {
        self.reload()?;
        let before = self.metrics.clone();
        let c = self.collection(collection)?;
        match self.options.cache {
            CacheMode::Memory => {
                if let Some(picker) = self.file_pickers.get_mut(collection) {
                    let changed = picker.refresh()?;
                    if changed > 0 {
                        self.autosave_loaded_bigram_cache(collection)?;
                    }
                } else {
                    self.ensure_picker(&c)?;
                }
                self.memory_coverage
                    .insert(c.name.clone(), "current".into());
            }
            CacheMode::Disabled => {
                self.direct(&c, None, false)?;
            }
        }
        Ok(self.metrics.since(&before))
    }
}
