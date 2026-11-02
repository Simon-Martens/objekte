//! Bounded ordered selection for views without a usable ordered index.

use super::{Plan, Scalar};
use std::cmp::Ordering;


/// One retained candidate; the payload is built only after it enters the prefix.
struct Entry<T> {
    keys: Vec<Scalar>,
    value: T,
}


/// A max heap of the best `offset + limit` candidates seen so far.
pub(crate) struct ViewSelection<'a, T> {
    plan: &'a Plan,
    entries: Vec<Entry<T>>,
    keep: usize,
    matched: usize,
}


impl<'a, T> ViewSelection<'a, T> {
    pub(crate) fn new(plan: &'a Plan) -> Self {
        Self {
            plan,
            entries: Vec::new(),
            keep: plan.offset.saturating_add(plan.limit),
            matched: 0,
        }
    }

    pub(crate) fn consider(&mut self, keys: Vec<Scalar>, value: impl FnOnce() -> T) {
        self.matched += 1;
        if self.keep == 0 {
            return;
        }
        if self.entries.len() < self.keep {
            self.entries.push(Entry {
                keys,
                value: value(),
            });
            self.sift_up(self.entries.len() - 1);
        } else if self.plan.compare_keys(&keys, &self.entries[0].keys) == Ordering::Less {
            self.entries[0] = Entry {
                keys,
                value: value(),
            };
            self.sift_down();
        }
    }

    pub(crate) fn total(&self) -> Option<usize> {
        self.plan.count.then_some(self.matched)
    }

    pub(crate) fn into_page(mut self) -> Vec<T> {
        self.entries
            .sort_unstable_by(|left, right| self.plan.compare_keys(&left.keys, &right.keys));
        self.entries
            .into_iter()
            .skip(self.plan.offset)
            .take(self.plan.limit)
            .map(|entry| entry.value)
            .collect()
    }

    fn sift_up(&mut self, mut child: usize) {
        while child > 0 {
            let parent = (child - 1) / 2;
            if self
                .plan
                .compare_keys(&self.entries[parent].keys, &self.entries[child].keys)
                != Ordering::Less
            {
                break;
            }
            self.entries.swap(parent, child);
            child = parent;
        }
    }

    fn sift_down(&mut self) {
        let mut parent = 0;
        let len = self.entries.len();
        loop {
            let left = parent * 2 + 1;
            if left >= len {
                break;
            }
            let right = left + 1;
            let child = if right < len
                && self
                    .plan
                    .compare_keys(&self.entries[left].keys, &self.entries[right].keys)
                    == Ordering::Less
            {
                right
            } else {
                left
            };
            if self
                .plan
                .compare_keys(&self.entries[parent].keys, &self.entries[child].keys)
                != Ordering::Less
            {
                break;
            }
            self.entries.swap(parent, child);
            parent = child;
        }
    }
}
