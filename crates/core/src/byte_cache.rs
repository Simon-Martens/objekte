//! Approximate retained-byte budgets with real LRU eviction, not a record-count cliff.
use lru::LruCache;
use std::hash::Hash;

pub(super) struct ByteCache<K, V> {
    entries: LruCache<K, (usize, V)>,
    pub bytes: usize,
    budget: usize,
}

impl<K: Hash + Eq, V> ByteCache<K, V> {
    pub fn new(budget: usize) -> Self {
        Self {
            entries: LruCache::unbounded(),
            bytes: 0,
            budget,
        }
    }
    pub fn get(&mut self, key: &K) -> Option<&V> {
        self.entries.get(key).map(|(_, value)| value)
    }
    #[cfg(test)]
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.iter().map(|(_, (_, value))| value)
    }
    pub fn remove(&mut self, key: &K) {
        if let Some((bytes, _)) = self.entries.pop(key) {
            self.bytes -= bytes;
        }
    }
    pub fn insert(&mut self, key: K, value: V, bytes: usize) {
        self.remove(&key);
        // One oversized record must not flush every useful small record.
        if bytes > self.budget {
            return;
        }
        while self.bytes + bytes > self.budget {
            if let Some((_, (bytes, _))) = self.entries.pop_lru() {
                self.bytes -= bytes;
            } else {
                break;
            }
        }
        self.entries.put(key, (bytes, value));
        self.bytes += bytes;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn budget_evicts_least_used_and_skips_oversized_entries() {
        let mut cache = ByteCache::new(10);
        cache.insert("a", 1, 4);
        cache.insert("b", 2, 4);
        assert_eq!(cache.get(&"a"), Some(&1));
        cache.insert("c", 3, 4);
        assert!(cache.get(&"b").is_none());
        assert_eq!(cache.get(&"a"), Some(&1));
        cache.insert("huge", 4, 11);
        assert_eq!(cache.bytes, 8);
        assert_eq!(cache.get(&"c"), Some(&3));
        cache.insert("a", 5, 2);
        assert_eq!(cache.bytes, 6);
        cache.remove(&"c");
        assert_eq!(cache.bytes, 2);
    }
}

pub(super) fn value_bytes(value: &serde_json::Value) -> usize {
    use serde_json::Value;
    std::mem::size_of::<Value>()
        + match value {
            Value::String(s) => s.capacity(),
            Value::Array(items) => items.iter().map(value_bytes).sum(),
            Value::Object(fields) => fields
                .iter()
                .map(|(k, v)| k.capacity() + value_bytes(v) + 64)
                .sum(),
            _ => 0,
        }
}
