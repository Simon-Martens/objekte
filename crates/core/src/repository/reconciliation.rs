//! Source refresh.

use super::*;
use crate::JsonFilter;


impl Repository {
    /// Build an ordered posting for a registered derived field. The extractor
    /// identity is namespaced and cannot collide with a stored JSON path.
    pub fn build_derived_posting(&mut self, collection: &str, extractor: &str) -> Result<()> {
        if self.selected_route(&[collection.to_owned()])?.is_some() {
            self.warm_selected_derived(collection, extractor)?;
            return Ok(());
        }
        let c = self.collection(collection)?;
        let extensions = self
            .extensions
            .as_ref()
            .ok_or_else(|| Error::Request("no value extractors are registered".into()))?;
        let binding = extensions.bind_value(
            extractor,
            collection,
            extensions.schema(collection).map(|schema| schema.as_ref()),
        )?;
        self.ensure_picker(&c)?;
        let picker = self
            .file_pickers
            .get_mut(collection)
            .expect("picker exists");
        if picker.refresh()? > 0 {
            self.query_cache.invalidate(collection);
        }
        picker.build_derived_posting(binding)
    }

    /// Distinct record counts for a registered, non-null derived value.
    pub fn derived_value_counts(
        &mut self,
        collection: &str,
        extractor: &str,
    ) -> Result<Vec<(crate::extensions::TypedValue, usize)>> {
        if let Some(mut selected) = self.selected_route(&[collection.to_owned()])? {
            let (c, snapshot) = selected.pop().expect("selected collection exists");
            let direct_counts = self.options.cache != CacheMode::Memory
                || self.options.freshness == Freshness::HashOnRequest;
            if !direct_counts {
                self.warm_selected_derived(collection, extractor)?;
            }
            let extensions = self.extensions.as_ref().expect("selected extensions");
            let binding = extensions.bind_value(
                extractor,
                collection,
                extensions.schema(collection).map(std::sync::Arc::as_ref),
            )?;
            if direct_counts {
                let mut counts = BTreeMap::<crate::file_picker::TypedKey, usize>::new();
                let mut bytes = Vec::new();
                let mut output = Vec::new();
                for source in snapshot.sources.iter() {
                    let record = self.selected_record(&c, source, &mut bytes, false)?;
                    output.clear();
                    let state = binding.extractor.extract(&record.data, &mut output)?;
                    if state == crate::extensions::Extraction::Missing && !output.is_empty() {
                        return Err(Error::Request(format!(
                            "extractor {} emitted values for a missing field",
                            binding.identity,
                        )));
                    }
                    let unique = output
                        .drain(..)
                        .map(crate::file_picker::TypedKey::from_extracted)
                        .collect::<Result<BTreeSet<_>>>()?;
                    for key in unique {
                        if !matches!(key, crate::file_picker::TypedKey::Null) {
                            *counts.entry(key).or_default() += 1;
                        }
                    }
                }
                return Ok(counts
                    .into_iter()
                    .filter_map(|(key, count)| {
                        let value = match key {
                            crate::file_picker::TypedKey::Bool(value) => {
                                crate::extensions::TypedValue::Bool(value)
                            }
                            crate::file_picker::TypedKey::String(value) => {
                                crate::extensions::TypedValue::Text(value)
                            }
                            crate::file_picker::TypedKey::Number(value) => {
                                crate::extensions::TypedValue::Number(value)
                            }
                            crate::file_picker::TypedKey::Null
                            | crate::file_picker::TypedKey::Missing => return None,
                        };
                        Some((value, count))
                    })
                    .collect());
            }
            let key = extension_derived_index::SelectedDerivedKey::new(
                &c,
                &binding.path,
                &snapshot.fingerprint,
                extensions,
            );
            let posting = self
                .selected_derived_indexes
                .get(&key)
                .ok_or_else(|| Error::Cache("selected derived posting is unavailable".into()))?;
            let counts = posting
                .index
                .derived_value_counts(&binding.path)
                .ok_or_else(|| Error::Cache("selected derived posting is stale".into()))?;
            return Ok(counts
                .into_iter()
                .filter_map(|(key, count)| {
                    let value = match key {
                        crate::file_picker::TypedKey::Bool(value) => {
                            crate::extensions::TypedValue::Bool(value)
                        }
                        crate::file_picker::TypedKey::String(value) => {
                            crate::extensions::TypedValue::Text(value)
                        }
                        crate::file_picker::TypedKey::Number(value) => {
                            crate::extensions::TypedValue::Number(value)
                        }
                        crate::file_picker::TypedKey::Null
                        | crate::file_picker::TypedKey::Missing => return None,
                    };
                    Some((value, count))
                })
                .collect());
        }
        self.build_derived_posting(collection, extractor)?;
        let extensions = self.extensions.as_ref().expect("bound extensions");
        let binding = extensions.bind_value(
            extractor,
            collection,
            extensions.schema(collection).map(|schema| schema.as_ref()),
        )?;
        self.file_pickers[collection]
            .derived_value_counts(&binding.path)
            .ok_or_else(|| Error::Cache("derived posting is unavailable".into()))
    }

    /// Restore a saved picker without waiting for source metadata checks.
    /// Until `apply_collection_inventory` runs, searches may use stale postings.
    pub fn restore_cached_index_unchecked(&mut self, collection: &str) -> Result<bool> {
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
        let snapshot = match BigramSnapshot::load_scoped_compact(&path, &c.path, c.recursive) {
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
        self.query_cache.invalidate(collection);
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
        let selected_state = self.reconcile_extension_selection(&inventory.collection)?;
        if changed > 0 {
            self.query_cache.invalidate(&inventory.collection);
            if selected_state.is_none() {
                self.invalidate_representation_indexes(&inventory.collection);
            }
            if selected_state != Some(true) {
                self.autosave_loaded_bigram_cache(&inventory.collection)?;
            }
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
            self.query_cache.invalidate(collection);
        }
        let _ = picker.save_bigram_snapshot(&path);
        Ok(())
    }

    /// Build or rebuild an ordered posting for one JSON path in a collection.
    /// The path uses filter syntax, such as `year` or `contributors[0].type`.
    pub fn build_posting(&mut self, collection: &str, path: &str) -> Result<()> {
        let path = JsonFilter::parse_path(path)?;
        if self.selected_route(&[collection.to_owned()])?.is_some() {
            return Err(Error::Request("build_posting is a native JSON picker helper; mixed-source JSON filters use exact evaluation".into()));
        }
        let c = self.collection(collection)?;
        self.ensure_picker(&c)?;
        let picker = self
            .file_pickers
            .get_mut(collection)
            .expect("picker exists");
        let changed = picker.refresh()?;
        if changed > 0 {
            self.query_cache.invalidate(collection);
        }
        picker.build_posting(&path)?;
        self.query_cache.invalidate(collection);
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
        if let Some(mut selected) = self.selected_route(&[collection.to_owned()])?
            && let Some((_, snapshot)) = selected.pop()
            && snapshot.sources.iter().any(|source| {
                source.record_id == id && source.provider != crate::source::FILESYSTEM_JSON_IDENTITY
            })
        {
            return Err(Error::Request("record_revision tracks native JSON picker revisions; use source_descriptors for provider revisions".into()));
        }
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
        let c = self.model.collection(collection)?.clone();
        let before = self.metrics.clone();
        self.warm_bigram_index_for(&c)?;
        if self.custom_source_collections.contains(collection)
            || self.has_declared_text_indexes(&c)
            || self.selected_cache_file(collection, None).is_file()
        {
            self.warm_selected_indexes(collection, true, true)?;
        }
        Ok(self.metrics.since(&before))
    }

    /// Save one collection's compressed bigram index and file-slot metadata.
    /// Call `rebuild_bigram_index` first when an in-memory overlay is present.
    pub fn save_bigram_snapshot(&mut self, collection: &str, path: &Path) -> Result<()> {
        let c = self.model.collection(collection)?.clone();
        self.ensure_picker(&c)?;
        self.file_pickers[collection].save_bigram_snapshot(path)
    }

    /// Save a collection's current bigram index in `.objekte/.cache`.
    pub fn save_bigram_cache(&mut self, collection: &str) -> Result<std::path::PathBuf> {
        let path = self.bigram_cache_path(collection)?;
        let c = self.model.collection(collection)?.clone();
        self.ensure_picker(&c)?;
        let picker = self
            .file_pickers
            .get_mut(collection)
            .expect("picker exists");
        if !picker.can_save_bigram_snapshot() {
            let changed = picker.refresh()?;
            if changed > 0 {
                self.query_cache.invalidate(collection);
            }
            picker.rebuild_index()?;
            self.query_cache.invalidate(collection);
        }
        picker.save_bigram_snapshot(&path)?;
        if self.custom_source_collections.contains(collection)
            || self.custom_source_snapshot.contains_key(collection)
        {
            self.save_selected_indexes(collection)?;
        }
        Ok(path)
    }

    /// Save the current bigram indexes for every collection.
    pub fn save_all_bigram_caches(&mut self) -> Result<BTreeMap<String, std::path::PathBuf>> {
        let names = self.model.collections.keys().cloned().collect::<Vec<_>>();
        names
            .iter()
            .map(|name| Ok((name.clone(), self.save_bigram_cache(name)?)))
            .collect()
    }

    /// Read every source record again, rebuild one index, and replace its cache.
    pub fn reindex_bigram_cache(&mut self, collection: &str) -> Result<std::path::PathBuf> {
        self.refresh(collection)?;
        let c = self.model.collection(collection)?.clone();
        self.ensure_picker(&c)?;
        let picker = self
            .file_pickers
            .get_mut(collection)
            .expect("picker exists");
        let changed = picker.refresh()?;
        if changed > 0 {
            self.query_cache.invalidate(collection);
        }
        picker.rebuild_index()?;
        self.query_cache.invalidate(collection);
        let path = self.bigram_cache_path(collection)?;
        self.file_pickers[collection].save_bigram_snapshot(&path)?;
        if self.custom_source_collections.contains(collection) || self.has_declared_text_indexes(&c)
        {
            self.rebuild_selected_text_indexes(collection)?;
            self.save_selected_indexes(collection)?;
        }
        Ok(path)
    }

    /// Rebuild every collection and replace all saved bigram caches.
    pub fn reindex_all_bigram_caches(&mut self) -> Result<BTreeMap<String, std::path::PathBuf>> {
        let names = self.model.collections.keys().cloned().collect::<Vec<_>>();
        names
            .iter()
            .map(|name| Ok((name.clone(), self.reindex_bigram_cache(name)?)))
            .collect()
    }

    /// Warm every collection's process-local bigram picker, in name order.
    /// Call the single-collection method to report progress between collections.
    pub fn warm_all_bigram_indexes(&mut self) -> Result<BTreeMap<String, Metrics>> {
        let names = self.model.collections.keys().cloned().collect::<Vec<_>>();
        names
            .iter()
            .map(|name| Ok((name.clone(), self.warm_bigram_index(name)?)))
            .collect()
    }

    /// Refresh source inventory and rebuild one collection's bigram and scalar
    /// indexes. Rebuilding an existing picker retains its record revisions and
    /// deletion tombstones.
    pub fn rebuild_bigram_index(&mut self, collection: &str) -> Result<Metrics> {
        let before = self.metrics.clone();
        self.refresh(collection)?;
        let c = self.model.collection(collection)?.clone();
        self.rebuild_bigram_index_for(&c)?;
        if self.custom_source_collections.contains(collection) || self.has_declared_text_indexes(&c)
        {
            self.rebuild_selected_text_indexes(collection)?;
        }
        Ok(self.metrics.since(&before))
    }

    /// Rebuild every collection's process-local bigram and scalar indexes, in
    /// name order. Earlier collections remain rebuilt if a later one fails.
    pub fn rebuild_all_bigram_indexes(&mut self) -> Result<BTreeMap<String, Metrics>> {
        let names = self.model.collections.keys().cloned().collect::<Vec<_>>();
        names
            .iter()
            .map(|name| Ok((name.clone(), self.rebuild_bigram_index(name)?)))
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
            let changed = picker.refresh()?;
            if changed > 0 {
                self.query_cache.invalidate(&c.name);
            }
            self.metrics.bigram_refresh_nanos += nanos(started);
            let started = Instant::now();
            picker.rebuild_index()?;
            self.query_cache.invalidate(&c.name);
            #[cfg(feature = "index-timings")]
            let timings = picker.build_timings();
            self.metrics.bigram_rebuild_nanos += nanos(started);
            #[cfg(feature = "index-timings")]
            {
                self.metrics.bigram_inventory_nanos += timings.inventory_nanos;
                self.metrics.bigram_slots_nanos += timings.slots_nanos;
                self.metrics.bigram_scan_nanos += timings.scan_nanos;
                self.metrics.bigram_project_worker_nanos += timings.project_worker_nanos;
                self.metrics.bigram_add_worker_nanos += timings.add_worker_nanos;
                self.metrics.bigram_compress_nanos += timings.compress_nanos;
                self.metrics.bigram_rendered_compress_nanos += timings.rendered_compress.wall_nanos;
                self.metrics.bigram_field_compress_nanos += timings.field_compress.wall_nanos;
                self.metrics.bigram_typed_nanos += timings.typed_nanos;
                self.metrics.bigram_view_nanos += timings.view_nanos;
            }
        } else {
            self.ensure_picker(c)?;
        }
        Ok(self.metrics.since(&before))
    }

    pub fn refresh(&mut self, collection: &str) -> Result<Metrics> {
        let before = self.metrics.clone();
        let c = self.model.collection(collection)?.clone();
        match self.options.cache {
            CacheMode::Memory => {
                let mut native_changed = false;
                if let Some(picker) = self.file_pickers.get_mut(collection) {
                    let changed = picker.refresh()?;
                    if changed > 0 {
                        native_changed = true;
                        self.query_cache.invalidate(collection);
                        self.autosave_loaded_bigram_cache(collection)?;
                    }
                }
                // Reconcile selected postings after the concrete picker has
                // finished invalidating its own generation. The selected
                // builder must see the same inventory that queries will pin.
                let selected_state = self.reconcile_extension_selection(collection)?;
                if native_changed && selected_state.is_none() {
                    self.invalidate_representation_indexes(collection);
                }
                let has_custom = selected_state == Some(true);
                if !self.file_pickers.contains_key(collection)
                    && (!has_custom
                        || self
                            .custom_source_snapshot
                            .get(collection)
                            .is_some_and(|snapshot| {
                                snapshot.sources.iter().any(|source| {
                                    source.provider == crate::source::FILESYSTEM_JSON_IDENTITY
                                })
                            }))
                {
                    self.ensure_picker(&c)?;
                }
                self.memory_coverage
                    .insert(c.name.clone(), "current".into());
            }
            CacheMode::Disabled => {
                self.reconcile_extension_selection(collection)?;
                self.direct(&c, None, false)?;
            }
        }
        Ok(self.metrics.since(&before))
    }
}
