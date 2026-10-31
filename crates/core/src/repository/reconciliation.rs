//! Source refresh.

use super::*;
use crate::JsonFilter;


impl Repository {
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
                    picker.refresh()?;
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
