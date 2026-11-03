//! Complete, collection-local ordered match lists. No record contents live here.

use super::predicate_cache::PredicateCacheKey;
use std::collections::BTreeMap;


#[derive(Clone, Eq, PartialEq, Ord, PartialOrd)]
pub(super) struct OrderedResultKey {
    pub predicate: PredicateCacheKey,
    pub sort: Vec<u8>,
}


pub(super) struct OrderedResultEntry {
    pub generation: u64,
    pub slots: Vec<u32>,
    pub coverage: OrderedCoverage,
    pub last_used: u64,
}


#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum OrderedCoverage {
    Complete,
    Prefix,
    CappedPrefix,
}


#[derive(Default)]
pub(super) struct OrderedResultCache {
    entries: BTreeMap<OrderedResultKey, OrderedResultEntry>,
}


impl OrderedResultCache {
    pub(super) fn get(
        &mut self,
        key: &OrderedResultKey,
        generation: u64,
        tick: u64,
    ) -> Option<(&[u32], OrderedCoverage)> {
        let entry = self.entries.get_mut(key)?;
        if entry.generation != generation {
            return None;
        }
        entry.last_used = tick;
        Some((&entry.slots, entry.coverage))
    }

    pub(super) fn insert(
        &mut self,
        key: OrderedResultKey,
        generation: u64,
        slots: Vec<u32>,
        coverage: OrderedCoverage,
        tick: u64,
    ) {
        if let Some(previous) = self.entries.get(&key)
            && (previous.coverage == OrderedCoverage::Complete
                || previous.slots.len() >= slots.len() && coverage != OrderedCoverage::Complete)
        {
            return;
        }
        self.entries.insert(
            key,
            OrderedResultEntry {
                generation,
                slots,
                coverage,
                last_used: tick,
            },
        );
    }

    pub(super) fn remove_collection(&mut self, collection: &str) {
        self.entries
            .retain(|key, _| key.predicate.collection != collection);
    }

    pub(super) fn oldest(&self) -> Option<(OrderedResultKey, u64)> {
        self.entries
            .iter()
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(key, entry)| (key.clone(), entry.last_used))
    }

    pub(super) fn remove(&mut self, key: &OrderedResultKey) {
        self.entries.remove(key);
    }

    pub(super) fn bytes(&self) -> usize {
        self.entries
            .iter()
            .map(|(key, entry)| {
                key.predicate.collection.capacity()
                    + key.predicate.expression.capacity()
                    + key.sort.capacity()
                    + entry.slots.capacity() * std::mem::size_of::<u32>()
                    + 160
            })
            .sum()
    }
}
