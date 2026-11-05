//! Bounded verified slot membership with collection-wide invalidation.

use super::ordered_result_cache::{OrderedCoverage, OrderedResultCache, OrderedResultKey};
use std::collections::BTreeMap;

pub(super) fn key_references_collection(scope: &str, collection: &str) -> bool {
    scope == collection
        || scope
            .strip_prefix("@frontend:")
            .is_some_and(|names| names.split('\0').any(|name| name == collection))
}

#[cfg(test)]
#[path = "predicate_cache.test.rs"]
mod tests;


#[derive(Clone, Eq, PartialEq, Ord, PartialOrd)]
pub(super) struct PredicateCacheKey {
    pub collection: String,
    pub expression: Vec<u8>,
}


#[derive(Clone)]
pub(super) struct VerifiedPredicateEntry {
    pub generation: u64,
    pub evaluated: Vec<u64>,
    pub matched: Vec<u64>,
    pub complete: bool,
    pub last_used: u64,
}


impl VerifiedPredicateEntry {
    pub(super) fn known(&self, slot: usize) -> Option<bool> {
        let bit = 1_u64 << (slot % 64);
        self.evaluated
            .get(slot / 64)
            .filter(|word| *word & bit != 0)
            .map(|_| self.matched[slot / 64] & bit != 0)
    }

    fn bytes(&self) -> usize {
        (self.evaluated.capacity() + self.matched.capacity()) * 8 + 96
    }
}


pub(super) struct VerifiedPredicateCache {
    generations: BTreeMap<String, u64>,
    predicates: BTreeMap<PredicateCacheKey, VerifiedPredicateEntry>,
    pub(super) ordered: OrderedResultCache,
    budget: usize,
    tick: u64,
    evictions: u64,
}


impl VerifiedPredicateCache {
    pub(super) fn new(budget: usize) -> Self {
        Self {
            generations: BTreeMap::new(),
            predicates: BTreeMap::new(),
            ordered: OrderedResultCache::default(),
            budget,
            tick: 0,
            evictions: 0,
        }
    }

    pub(super) fn enabled(&self) -> bool {
        self.budget > 0
    }

    pub(super) fn generation(&self, collection: &str) -> u64 {
        self.generations.get(collection).copied().unwrap_or(0)
    }

    pub(super) fn invalidate(&mut self, collection: &str) {
        if !self.enabled() {
            return;
        }
        *self.generations.entry(collection.to_owned()).or_default() += 1;
        self.predicates
            .retain(|key, _| !key_references_collection(&key.collection, collection));
        self.ordered.remove_collection(collection);
    }

    pub(super) fn get(&mut self, key: &PredicateCacheKey) -> Option<&VerifiedPredicateEntry> {
        self.tick += 1;
        let generation = self.generation(&key.collection);
        let entry = self.predicates.get_mut(key)?;
        if entry.generation != generation {
            return None;
        }
        entry.last_used = self.tick;
        Some(entry)
    }

    pub(super) fn record_bits(
        &mut self,
        key: PredicateCacheKey,
        evaluated: Vec<u64>,
        matched: Vec<u64>,
        complete: bool,
    ) {
        if !self.enabled() || evaluated.len() != matched.len() {
            return;
        }
        let words = evaluated.len();
        let needed = words
            .saturating_mul(16)
            .saturating_add(key.expression.len() + 160);
        if needed > self.budget {
            return;
        }
        self.tick += 1;
        let generation = self.generation(&key.collection);
        let entry = self
            .predicates
            .entry(key)
            .or_insert_with(|| VerifiedPredicateEntry {
                generation,
                evaluated: vec![0; words],
                matched: vec![0; words],
                complete: false,
                last_used: self.tick,
            });
        if entry.generation != generation || entry.evaluated.len() != words {
            *entry = VerifiedPredicateEntry {
                generation,
                evaluated: vec![0; words],
                matched: vec![0; words],
                complete: false,
                last_used: self.tick,
            };
        }
        for ((known, matches), (next_known, next_matches)) in entry
            .evaluated
            .iter_mut()
            .zip(&mut entry.matched)
            .zip(evaluated.into_iter().zip(matched))
        {
            *known |= next_known;
            *matches = (*matches & !next_known) | (next_matches & next_known);
        }
        entry.complete |= complete;
        entry.last_used = self.tick;
        self.evict();
    }

    pub(super) fn ordered(&mut self, key: &OrderedResultKey) -> Option<(&[u32], OrderedCoverage)> {
        self.tick += 1;
        self.ordered
            .get(key, self.generation(&key.predicate.collection), self.tick)
    }

    pub(super) fn completed_ordered_page(
        &mut self,
        key: &OrderedResultKey,
        offset: usize,
        limit: usize,
    ) -> Option<(usize, Vec<u32>)> {
        let (slots, coverage) = self.ordered(key)?;
        if coverage != OrderedCoverage::Complete {
            return None;
        }
        let end = slots.len().min(offset.saturating_add(limit));
        Some((slots.len(), slots.get(offset..end).unwrap_or(&[]).to_vec()))
    }

    pub(super) fn insert_ordered(&mut self, key: OrderedResultKey, slots: Vec<u32>) {
        self.insert_ordered_with_coverage(key, slots, OrderedCoverage::Complete);
    }

    pub(super) fn insert_ordered_with_coverage(
        &mut self,
        key: OrderedResultKey,
        slots: Vec<u32>,
        coverage: OrderedCoverage,
    ) {
        if !self.enabled() || slots.len().saturating_mul(4) > self.budget {
            return;
        }
        self.tick += 1;
        self.ordered.insert(
            key.clone(),
            self.generation(&key.predicate.collection),
            slots,
            coverage,
            self.tick,
        );
        self.evict();
    }

    pub(super) fn bytes(&self) -> usize {
        self.predicates
            .iter()
            .map(|(key, entry)| {
                key.collection.capacity() + key.expression.capacity() + entry.bytes() + 96
            })
            .sum::<usize>()
            + self.ordered.bytes()
            + self
                .generations
                .keys()
                .map(|name| name.capacity() + 64)
                .sum::<usize>()
    }

    pub(super) fn evictions(&self) -> u64 {
        self.evictions
    }

    fn evict(&mut self) {
        while self.bytes() > self.budget {
            let predicate = self
                .predicates
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, entry)| (key.clone(), entry.last_used));
            let ordered = self.ordered.oldest();
            match (predicate, ordered) {
                (Some((key, age)), Some((ordered_key, ordered_age))) => {
                    if age <= ordered_age {
                        self.predicates.remove(&key);
                    } else {
                        self.ordered.remove(&ordered_key);
                    }
                    self.evictions += 1;
                }
                (Some((key, _)), None) => {
                    self.predicates.remove(&key);
                    self.evictions += 1;
                }
                (None, Some((key, _))) => {
                    self.ordered.remove(&key);
                    self.evictions += 1;
                }
                (None, None) => break,
            }
        }
    }
}
