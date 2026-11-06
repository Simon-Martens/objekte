//! Retain a bounded sorted prefix when no usable sort index exists.

use super::*;


/// A retained row and the keys used to compare it without reprojecting JSON.
struct Entry<T> {
    collection: String,
    id: String,
    keys: Vec<SortValue>,
    arrival: usize,
    value: Option<T>,
}


/// Keeps the best accessible prefix in a max heap while an unindexed scan
/// proceeds. The root is the worst retained result; skipped pages count too.
pub(crate) struct SortBuffer<T> {
    plans: BTreeMap<String, SortPlan>,
    entries: Vec<Entry<T>>,
    page_order: Vec<usize>,
    page_values: Vec<T>,
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
            page_order: Vec::new(),
            page_values: Vec::new(),
            keep: offset.saturating_add(page_size).min(total_limit),
            offset,
            page_size,
            total_limit,
            seen: 0,
        }
    }

    pub(crate) fn project(
        &self,
        collection: &str,
        record: &SortRecord<'_>,
        data: &Value,
    ) -> Result<Vec<SortValue>> {
        self.plans[collection].project_record(record, data)
    }

    pub(crate) fn uses_status(&self) -> bool {
        self.plans.values().any(SortPlan::uses_status)
    }

    pub(crate) fn insert_keys(
        &mut self,
        collection: &str,
        id: &str,
        keys: Vec<SortValue>,
        value: impl FnOnce() -> T,
    ) -> bool {
        if !self.admits(collection, id, &keys) {
            return false;
        }
        self.place(Entry {
            collection: collection.into(),
            id: id.into(),
            keys,
            arrival: self.seen,
            value: Some(value()),
        });
        true
    }

    /// Move an internal hit into the heap after borrowing its identity for
    /// admission. Rejected hits are dropped without copying their payload.
    pub(crate) fn insert_owned_keys(
        &mut self,
        keys: Vec<SortValue>,
        value: T,
        identity: impl FnOnce(&T) -> (&str, &str),
    ) -> bool {
        let (collection, id) = identity(&value);
        if !self.admits(collection, id, &keys) {
            return false;
        }
        let (collection, id) = (collection.to_owned(), id.to_owned());
        self.place(Entry {
            collection,
            id,
            keys,
            arrival: self.seen,
            value: Some(value),
        });
        true
    }

    fn admits(&mut self, collection: &str, id: &str, keys: &[SortValue]) -> bool {
        self.seen += 1;
        self.keep != 0
            && (self.entries.len() < self.keep
                || Self::compare_candidate(
                    &self.plans,
                    &self.entries[0],
                    collection,
                    id,
                    keys,
                    self.seen,
                )
                .is_gt())
    }

    fn place(&mut self, entry: Entry<T>) {
        if self.entries.len() == self.keep {
            self.entries[0] = entry;
            self.sift_down(0);
        } else {
            self.entries.push(entry);
            self.sift_up(self.entries.len() - 1);
        }
    }

    fn compare_candidate(
        plans: &BTreeMap<String, SortPlan>,
        entry: &Entry<T>,
        collection: &str,
        id: &str,
        keys: &[SortValue],
        arrival: usize,
    ) -> Ordering {
        entry
            .collection
            .as_str()
            .cmp(collection)
            .then_with(|| plans[collection].compare(&entry.keys, &entry.id, keys, id))
            .then_with(|| arrival.cmp(&entry.arrival))
    }

    fn compare_entries(
        plans: &BTreeMap<String, SortPlan>,
        left: &Entry<T>,
        right: &Entry<T>,
    ) -> Ordering {
        Self::compare_candidate(
            plans,
            left,
            &right.collection,
            &right.id,
            &right.keys,
            right.arrival,
        )
    }

    fn sift_up(&mut self, mut index: usize) {
        while index > 0 {
            let parent = (index - 1) / 2;
            if !Self::compare_entries(&self.plans, &self.entries[parent], &self.entries[index])
                .is_lt()
            {
                break;
            }
            self.entries.swap(parent, index);
            index = parent;
        }
    }

    fn sift_down(&mut self, mut index: usize) {
        let len = self.entries.len();
        loop {
            let left = index * 2 + 1;
            if left >= len {
                break;
            }
            let right = left + 1;
            let child = if right < len
                && Self::compare_entries(&self.plans, &self.entries[left], &self.entries[right])
                    .is_lt()
            {
                right
            } else {
                left
            };
            if !Self::compare_entries(&self.plans, &self.entries[index], &self.entries[child])
                .is_lt()
            {
                break;
            }
            self.entries.swap(index, child);
            index = child;
        }
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

    #[cfg(test)]
    pub(crate) fn page(&self) -> impl Iterator<Item = &T> {
        let mut indices = (0..self.entries.len()).collect::<Vec<_>>();
        indices.sort_unstable_by(|&left, &right| {
            Self::compare_entries(&self.plans, &self.entries[left], &self.entries[right])
        });
        indices
            .into_iter()
            .skip(self.offset)
            .take(self.page_size)
            .map(|index| {
                self.entries[index]
                    .value
                    .as_ref()
                    .expect("retained payload")
            })
    }

    /// The callback requires a contiguous slice. Move its ordered page into
    /// scratch storage, call it, then return each payload to its heap slot.
    pub(crate) fn with_page(&mut self, callback: impl FnOnce(&[T]) -> Result<()>) -> Result<()> {
        self.page_order.clear();
        self.page_order.extend(0..self.entries.len());
        self.page_order.sort_unstable_by(|&left, &right| {
            Self::compare_entries(&self.plans, &self.entries[left], &self.entries[right])
        });
        self.page_values.clear();
        for &index in self
            .page_order
            .iter()
            .skip(self.offset)
            .take(self.page_size)
        {
            self.page_values
                .push(self.entries[index].value.take().expect("retained payload"));
        }
        let result = callback(&self.page_values);
        for (&index, value) in self
            .page_order
            .iter()
            .skip(self.offset)
            .take(self.page_size)
            .zip(self.page_values.drain(..))
        {
            self.entries[index].value = Some(value);
        }
        result
    }

    pub(crate) fn into_page(mut self) -> Vec<T> {
        self.entries
            .sort_unstable_by(|left, right| Self::compare_entries(&self.plans, left, right));
        self.entries
            .into_iter()
            .skip(self.offset)
            .take(self.page_size)
            .map(|entry| entry.value.expect("retained payload"))
            .collect()
    }
}


#[cfg(test)]
#[path = "buffer.test.rs"]
mod tests;
