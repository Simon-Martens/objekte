//! Complete verified membership and ordering, independent of source payloads.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use crate::query::Query;

#[derive(Clone, Debug)]
pub(crate) struct CachedMember {
    pub collection_index: usize,
    pub slot: u32,
    pub matched_fields: Vec<String>,
}

pub(crate) struct CachedResult {
    pub members: Vec<CachedMember>,
}

struct Entry {
    result: Arc<CachedResult>,
    bytes: usize,
}

pub(crate) struct VerifiedCache {
    budget: usize,
    bytes: usize,
    entries: HashMap<String, Entry>,
    order: VecDeque<String>,
}

impl VerifiedCache {
    pub fn new(budget: usize) -> Self {
        Self {
            budget,
            bytes: 0,
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }
    pub fn enabled(&self) -> bool {
        self.budget != 0
    }
    pub fn set_budget(&mut self, budget: usize) {
        self.budget = budget;
        self.bytes = 0;
        self.entries.clear();
        self.order.clear();
    }
    pub fn get(&mut self, key: &str) -> Option<Arc<CachedResult>> {
        let result = Arc::clone(&self.entries.get(key)?.result);
        if let Some(index) = self.order.iter().position(|old| old == key) {
            self.order.remove(index);
        }
        self.order.push_back(key.into());
        Some(result)
    }
    pub fn insert(&mut self, key: String, members: Vec<CachedMember>) {
        let bytes = key.len()
            + 128
            + members
                .iter()
                .map(|member| {
                    std::mem::size_of::<CachedMember>()
                        + member
                            .matched_fields
                            .iter()
                            .map(|field| field.len() + std::mem::size_of::<String>())
                            .sum::<usize>()
                })
                .sum::<usize>();
        if bytes > self.budget {
            return;
        }
        if let Some(old) = self.entries.remove(&key) {
            self.bytes -= old.bytes;
        }
        if let Some(index) = self.order.iter().position(|old| old == &key) {
            self.order.remove(index);
        }
        while self.bytes.saturating_add(bytes) > self.budget {
            if let Some(old) = self.order.pop_front() {
                if let Some(entry) = self.entries.remove(&old) {
                    self.bytes -= entry.bytes;
                }
            } else {
                break;
            }
        }
        self.bytes += bytes;
        self.order.push_back(key.clone());
        self.entries.insert(
            key,
            Entry {
                result: Arc::new(CachedResult { members }),
                bytes,
            },
        );
    }
    #[cfg(test)]
    pub fn used_bytes(&self) -> usize {
        self.bytes
    }
}

pub(crate) fn key(query: &Query) -> String {
    // Repository ownership supplies the generation, alphabet, schema and
    // binding-version scope; options below determine membership/order/evidence.
    format!(
        "{:?}|{:?}|{:?}|{:?}|{}",
        query.collections, query.filter, query.fts, query.options.sort, query.options.evidence
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_entries_include_complete_empty_results() {
        let mut cache = VerifiedCache::new(300);
        cache.insert("empty".into(), Vec::new());
        assert_eq!(cache.get("empty").unwrap().members.len(), 0);
        cache.insert("other".into(), Vec::new());
        cache.insert("third".into(), Vec::new());
        assert!(cache.used_bytes() <= 300);
        assert!(cache.get("empty").is_none());
        cache.set_budget(0);
        assert!(!cache.enabled());
        assert!(cache.get("other").is_none());
    }
}
