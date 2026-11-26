use std::collections::HashMap;
use std::sync::Arc;

use crate::model::FieldPath;

use super::{COLLATION_VERSION, OrderColumnReader, SortOptions};
use crate::index::traits::segment::SegmentId;

pub(crate) const INFERRED_ORDER_CACHE_BYTES: usize = 128 * 1024 * 1024;

/// An inferred order is query-created but remains valid for a repository
/// generation. Cached handles supply the same cursor and key operations as
/// configured columns.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct InferredOrderKey {
    collection: usize,
    generation: u64,
    segment: SegmentId,
    path: FieldPath,
    locale: String,
    collation_version: &'static str,
    options: SortOptions,
    descending: bool,
}

impl InferredOrderKey {
    pub(crate) fn new(
        collection: usize,
        generation: u64,
        segment: SegmentId,
        path: FieldPath,
        locale: &str,
        options: SortOptions,
        descending: bool,
    ) -> Self {
        Self {
            collection,
            generation,
            segment,
            path,
            locale: locale.to_owned(),
            collation_version: COLLATION_VERSION,
            options,
            descending,
        }
    }

    fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(self.locale.capacity())
            .saturating_add(
                self.path
                    .segments
                    .capacity()
                    .saturating_mul(std::mem::size_of::<String>()),
            )
            .saturating_add(
                self.path
                    .segments
                    .iter()
                    .fold(0usize, |bytes, name| bytes.saturating_add(name.capacity())),
            )
    }
}

struct InferredOrderEntry {
    column: Arc<dyn OrderColumnReader>,
    bytes: usize,
    last_used: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct InferredOrderCacheStats {
    pub builds: u64,
    pub hits: u64,
    pub evictions: u64,
    pub resident_bytes: usize,
    pub entries: usize,
}

/// A lazy, bounded cache owned by one repository. Callers build a missing
/// column outside its mutex, then publish it with a second short lock.
pub(crate) struct InferredOrderCache {
    entries: HashMap<InferredOrderKey, InferredOrderEntry>,
    max_bytes: usize,
    used_bytes: usize,
    clock: u64,
    stats: InferredOrderCacheStats,
}

impl InferredOrderCache {
    pub(crate) fn new(max_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            max_bytes,
            used_bytes: 0,
            clock: 0,
            stats: InferredOrderCacheStats::default(),
        }
    }

    pub(crate) fn get(&mut self, key: &InferredOrderKey) -> Option<Arc<dyn OrderColumnReader>> {
        self.clock = self.clock.wrapping_add(1);
        if let Some(entry) = self.entries.get_mut(key) {
            entry.last_used = self.clock;
            self.stats.hits += 1;
            return Some(entry.column.clone());
        }
        None
    }

    /// A concurrent builder may already have published this key. Reuse that
    /// column and charge only one resident entry, while still counting both
    /// completed constructions in the structural build counter.
    pub(crate) fn insert_if_absent(
        &mut self,
        key: InferredOrderKey,
        column: Arc<dyn OrderColumnReader>,
    ) -> Arc<dyn OrderColumnReader> {
        self.stats.builds += 1;
        if let Some(existing) = self.get(&key) {
            return existing;
        }
        let bytes = Self::entry_bytes(&key, column.as_ref());
        if bytes <= self.max_bytes {
            while self.used_bytes.saturating_add(bytes) > self.max_bytes {
                let oldest = self
                    .entries
                    .iter()
                    .min_by_key(|(_, entry)| entry.last_used)
                    .map(|(key, _)| key.clone())
                    .expect("cache has an entry to evict");
                let removed = self.entries.remove(&oldest).expect("oldest cache entry");
                self.used_bytes -= removed.bytes;
                self.stats.evictions += 1;
            }
            self.used_bytes += bytes;
            self.entries.insert(
                key,
                InferredOrderEntry {
                    column: column.clone(),
                    bytes,
                    last_used: self.clock,
                },
            );
        }
        self.stats.resident_bytes = self.used_bytes;
        self.stats.entries = self.entries.len();
        column
    }

    #[cfg(test)]
    pub(crate) fn stats(&self) -> InferredOrderCacheStats {
        self.stats
    }

    fn entry_bytes(key: &InferredOrderKey, column: &dyn OrderColumnReader) -> usize {
        // Charge two map buckets per entry to cover spare hash-table capacity,
        // plus the Arc header. The string and vector capacities are counted by
        // the key and column themselves.
        let map_bytes = std::mem::size_of::<InferredOrderKey>()
            .saturating_add(std::mem::size_of::<InferredOrderEntry>())
            .saturating_mul(2);
        key.retained_bytes()
            .saturating_sub(std::mem::size_of::<InferredOrderKey>())
            .saturating_add(map_bytes)
            .saturating_add(column.retained_bytes())
            .saturating_add(2 * std::mem::size_of::<usize>())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::sorting::SortColumn;
    use crate::index::traits::order::SortKey;
    use crate::model::{LogicalType, RecordSlot};

    fn inferred_test_column(path: &str) -> SortColumn {
        SortColumn {
            path: FieldPath::parse(path).unwrap(),
            kind: LogicalType::String,
            keys: vec![SortKey::String(vec![1, 2, 3])],
            ascending: vec![RecordSlot(0)],
            descending: Vec::new(),
            locale: "de".into(),
            options: SortOptions::default(),
            collation_version: COLLATION_VERSION.into(),
        }
    }

    #[test]
    fn inferred_order_cache_reuses_keys_and_evicts_least_recently_used() {
        let first = InferredOrderKey::new(
            0,
            7,
            crate::index::segment::SegmentId::new(1),
            FieldPath::parse("a").unwrap(),
            "de",
            SortOptions::default(),
            false,
        );
        let second = InferredOrderKey::new(
            0,
            7,
            crate::index::segment::SegmentId::new(1),
            FieldPath::parse("b").unwrap(),
            "de",
            SortOptions::default(),
            false,
        );
        let one_bytes = InferredOrderCache::entry_bytes(&first, &inferred_test_column("a"));
        let mut cache = InferredOrderCache::new(one_bytes);
        let first_column =
            cache.insert_if_absent(first.clone(), Arc::new(inferred_test_column("a")));
        let reused = cache.get(&first).unwrap();
        assert!(Arc::ptr_eq(&first_column, &reused));
        assert_eq!(cache.stats().builds, 1);
        assert_eq!(cache.stats().hits, 1);
        cache.insert_if_absent(second, Arc::new(inferred_test_column("b")));
        assert_eq!(cache.stats().entries, 1);
        assert_eq!(cache.stats().evictions, 1);
        assert!(cache.stats().resident_bytes <= one_bytes);
        cache.insert_if_absent(first, Arc::new(inferred_test_column("a")));
        assert_eq!(cache.stats().builds, 3);
        assert_eq!(cache.stats().evictions, 2);
    }

    #[test]
    fn inferred_order_cache_key_separates_generation_collation_and_direction() {
        let mut cache = InferredOrderCache::new(1024 * 1024);
        let base = InferredOrderKey::new(
            0,
            7,
            crate::index::segment::SegmentId::new(1),
            FieldPath::parse("a").unwrap(),
            "de",
            SortOptions::default(),
            false,
        );
        let variants = [
            base.clone(),
            InferredOrderKey::new(
                0,
                8,
                crate::index::segment::SegmentId::new(1),
                base.path.clone(),
                "de",
                base.options,
                false,
            ),
            InferredOrderKey::new(
                0,
                7,
                crate::index::segment::SegmentId::new(2),
                base.path.clone(),
                "de",
                base.options,
                false,
            ),
            InferredOrderKey::new(
                0,
                7,
                crate::index::segment::SegmentId::new(1),
                base.path.clone(),
                "fr",
                base.options,
                false,
            ),
            InferredOrderKey::new(
                0,
                7,
                crate::index::segment::SegmentId::new(1),
                base.path.clone(),
                "de",
                base.options,
                true,
            ),
            InferredOrderKey::new(
                0,
                7,
                crate::index::segment::SegmentId::new(1),
                base.path.clone(),
                "de",
                SortOptions {
                    numeric: true,
                    ..base.options
                },
                false,
            ),
            InferredOrderKey::new(
                1,
                7,
                crate::index::segment::SegmentId::new(1),
                base.path.clone(),
                "de",
                base.options,
                false,
            ),
        ];
        for key in variants {
            cache.insert_if_absent(key, Arc::new(inferred_test_column("a")));
        }
        assert_eq!(cache.stats().builds, 7);
        assert_eq!(cache.stats().hits, 0);
        assert_eq!(cache.stats().entries, 7);
        cache
            .get(&base)
            .expect("same generation and collation missed");
        assert_eq!(cache.stats().hits, 1);
    }
}
