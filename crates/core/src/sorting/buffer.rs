//! Retain a bounded sorted prefix when no usable sort index exists.

use super::*;


/// A retained row and the keys used to compare it without reprojecting JSON.
struct Entry<T> {
    collection: String,
    id: String,
    keys: Vec<SortValue>,
    value: T,
}


/// Keeps only the best accessible prefix while an unindexed scan proceeds.
/// The prefix includes skipped pages so later page requests stay deterministic.
pub(crate) struct SortBuffer<T> {
    plans: BTreeMap<String, SortPlan>,
    entries: Vec<Entry<T>>,
    keep: usize,
    offset: usize,
    page_size: usize,
    total_limit: usize,
    seen: usize,
}


impl<T> SortBuffer<T> {
    pub(crate) fn new(
        plans: BTreeMap<String, SortPlan>,
        offset: usize,
        page_size: usize,
        total_limit: usize,
    ) -> Self {
        Self {
            plans,
            entries: Vec::new(),
            keep: offset.saturating_add(page_size).min(total_limit),
            offset,
            page_size,
            total_limit,
            seen: 0,
        }
    }

    pub(crate) fn insert(
        &mut self,
        collection: &str,
        id: &str,
        data: &Value,
        value: impl FnOnce() -> T,
    ) -> Result<bool> {
        if self.keep == 0 {
            self.seen += 1;
            return Ok(false);
        }
        let keys = self.project(collection, id, data)?;
        Ok(self.insert_keys(collection, id, keys, value))
    }

    pub(crate) fn project(
        &self,
        collection: &str,
        id: &str,
        data: &Value,
    ) -> Result<Vec<SortValue>> {
        self.plans[collection].project(id, data)
    }

    pub(crate) fn insert_keys(
        &mut self,
        collection: &str,
        id: &str,
        keys: Vec<SortValue>,
        value: impl FnOnce() -> T,
    ) -> bool {
        self.seen += 1;
        if self.keep == 0 {
            return false;
        }
        let plan = &self.plans[collection];
        let position =
            self.entries
                .partition_point(|entry| match entry.collection.as_str().cmp(collection) {
                    Ordering::Less => true,
                    Ordering::Greater => false,
                    Ordering::Equal => plan.compare(&entry.keys, &entry.id, &keys, id).is_lt(),
                });
        if position >= self.keep {
            return false;
        }
        self.entries.insert(
            position,
            Entry {
                collection: collection.into(),
                id: id.into(),
                keys,
                value: value(),
            },
        );
        if self.entries.len() > self.keep {
            self.entries.pop();
        }
        true
    }

    pub(crate) fn seen(&self) -> usize {
        self.seen
    }

    pub(crate) fn total(&self, count: bool) -> Option<usize> {
        count.then_some(self.seen.min(self.total_limit))
    }

    pub(crate) fn truncated(&self) -> bool {
        self.seen > self.total_limit
    }

    pub(crate) fn page(&self) -> impl Iterator<Item = &T> {
        self.entries
            .iter()
            .skip(self.offset)
            .take(self.page_size)
            .map(|entry| &entry.value)
    }

    pub(crate) fn into_page(self) -> Vec<T> {
        self.entries
            .into_iter()
            .skip(self.offset)
            .take(self.page_size)
            .map(|entry| entry.value)
            .collect()
    }
}
