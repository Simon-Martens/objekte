//! Retain a bounded sorted prefix when no usable sort index exists.

use super::*;


/// A retained row and the keys used to compare it without reprojecting JSON.
struct Entry<T> {
    collection: String,
    id: String,
    keys: Vec<SortValue>,
    arrival: usize,
    value: T,
}


/// Keeps the best accessible prefix in a max heap while an unindexed scan
/// proceeds. The root is the worst retained result; skipped pages count too.
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
        let arrival = self.seen;
        if self.entries.len() == self.keep {
            let worst = &self.entries[0];
            if !Self::compare_candidate(&self.plans, worst, collection, id, &keys, arrival).is_gt()
            {
                return false;
            }
            self.entries[0] = Entry {
                collection: collection.into(),
                id: id.into(),
                keys,
                arrival,
                value: value(),
            };
            self.sift_down(0);
        } else {
            self.entries.push(Entry {
                collection: collection.into(),
                id: id.into(),
                keys,
                arrival,
                value: value(),
            });
            self.sift_up(self.entries.len() - 1);
        }
        true
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

    pub(crate) fn page(&self) -> impl Iterator<Item = &T> {
        let mut indices = (0..self.entries.len()).collect::<Vec<_>>();
        indices.sort_unstable_by(|&left, &right| {
            Self::compare_entries(&self.plans, &self.entries[left], &self.entries[right])
        });
        indices
            .into_iter()
            .skip(self.offset)
            .take(self.page_size)
            .map(|index| &self.entries[index].value)
    }

    pub(crate) fn into_page(mut self) -> Vec<T> {
        self.entries
            .sort_unstable_by(|left, right| Self::compare_entries(&self.plans, left, right));
        self.entries
            .into_iter()
            .skip(self.offset)
            .take(self.page_size)
            .map(|entry| entry.value)
            .collect()
    }
}


#[cfg(test)]
#[path = "buffer.test.rs"]
mod tests;
