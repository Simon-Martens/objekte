//! Ordered key -> record-slot postings with two interchangeable backings.
//!
//! `Building` is the native `BTreeMap` used while an index is constructed and
//! by small immutable shards. `Shared` is a persistent (copy-on-write) ordered
//! map, so that a clone is cheap and a write copies only the changed path.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::ops::Bound;
use std::sync::Arc;

use imbl::OrdMap;
use roaring::RoaringBitmap;


/// Persistent ordered postings: a clone shares every node, and a write
/// copies only the nodes on the path to the changed key plus that bitmap.
#[derive(Clone, Debug)]
pub struct SharedPostings<K: Ord + Clone> {
    entries: OrdMap<K, Arc<RoaringBitmap>>,
}


type SharedEntry<'a, K> = (&'a K, &'a Arc<RoaringBitmap>);


impl<K: Ord + Clone> Default for SharedPostings<K> {
    fn default() -> Self {
        Self {
            entries: OrdMap::new(),
        }
    }
}


impl<K: Ord + Clone> SharedPostings<K> {
    pub fn from_building(source: BTreeMap<K, RoaringBitmap>) -> Self {
        Self {
            entries: source
                .into_iter()
                .map(|(key, posting)| (key, Arc::new(posting)))
                .collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn contains_key(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    pub fn get(&self, key: &K) -> Option<Cow<'_, RoaringBitmap>> {
        self.entries
            .get(key)
            .map(|posting| Cow::Borrowed(posting.as_ref()))
    }

    pub fn cardinality(&self, key: &K) -> u64 {
        self.entries.get(key).map_or(0, |posting| posting.len())
    }

    fn posting_mut(&mut self, key: &K) -> &mut RoaringBitmap {
        if !self.entries.contains_key(key) {
            self.entries
                .insert(key.clone(), Arc::new(RoaringBitmap::new()));
        }
        Arc::make_mut(self.entries.get_mut(key).expect("posting was just ensured"))
    }

    pub fn insert_slot(&mut self, key: &K, slot: u32) -> bool {
        self.posting_mut(key).insert(slot)
    }

    pub fn append_shifted(
        &mut self,
        key: &K,
        source: &RoaringBitmap,
        base: u32,
    ) -> Result<(), String> {
        if let Some(max) = source.max() {
            base.checked_add(max).ok_or("record slot overflow")?;
        }
        let first_new = source.min().map(|min| base + min);
        let existing = self.entries.get(key).and_then(|posting| posting.max());
        if let (Some(first_new), Some(existing)) = (first_new, existing)
            && first_new <= existing
        {
            return Err("batch bitmap slots overlap or are out of order".into());
        }
        self.posting_mut(key)
            .append(source.iter().map(|slot| base + slot))
            .map_err(|_| "batch bitmap slots overlap or are out of order")?;
        Ok(())
    }

    pub fn replace(&mut self, key: &K, posting: RoaringBitmap) {
        self.entries.insert(key.clone(), Arc::new(posting));
    }

    pub fn retain_keys(&mut self, mut keep: impl FnMut(&K) -> bool) {
        let doomed = self
            .entries
            .keys()
            .filter(|key| !keep(key))
            .cloned()
            .collect::<Vec<_>>();
        for key in doomed {
            self.entries.remove(&key);
        }
    }

    pub fn clear(&mut self) {
        self.entries = OrdMap::new();
    }

    pub fn first_key(&self) -> Option<&K> {
        self.entries.get_min().map(|(key, _)| key)
    }

    pub fn last_key(&self) -> Option<&K> {
        self.entries.get_max().map(|(key, _)| key)
    }

    pub fn serialized_bytes(&self) -> usize {
        self.entries
            .iter()
            .map(|(_, posting)| posting.serialized_size())
            .sum()
    }

    pub fn all_parts(&self, mut valid: impl FnMut(&RoaringBitmap) -> bool) -> bool {
        self.entries.iter().all(|(_, posting)| valid(posting))
    }

    fn entries<'a>(
        &'a self,
        lower: Bound<&K>,
        upper: Bound<&K>,
        descending: bool,
    ) -> Box<dyn Iterator<Item = SharedEntry<'a, K>> + 'a> {
        // An inverted range is empty here instead of a panic.
        let inverted = match (lower, upper) {
            (Bound::Included(a), Bound::Included(b)) => a > b,
            (Bound::Included(a), Bound::Excluded(b))
            | (Bound::Excluded(a), Bound::Included(b))
            | (Bound::Excluded(a), Bound::Excluded(b)) => a >= b,
            _ => false,
        };
        if inverted {
            return Box::new(std::iter::empty());
        }
        let range = self.entries.range((lower.cloned(), upper.cloned()));
        if descending {
            Box::new(range.rev())
        } else {
            Box::new(range)
        }
    }
}


/// Shards with at least this many records use the shared backing.
pub const SHARED_BACKING_MIN_RECORDS: usize = 16_384;

/// Whether a shard of `record_count` records should hold its postings in the
/// shared backing. The validation feature forces it for every shard so the
/// whole test suite exercises that backing.
pub fn shared_backing_wanted(record_count: usize) -> bool {
    cfg!(feature = "force-shared-backing") || record_count >= SHARED_BACKING_MIN_RECORDS
}


/// Forward-only traversal in ascending or descending key order. Each item is
/// one key with its complete posting.
pub struct PostingIter<'a, K: Ord + Clone> {
    inner: IterInner<'a, K>,
}


enum IterInner<'a, K: Ord + Clone> {
    Building(Box<dyn Iterator<Item = (&'a K, &'a RoaringBitmap)> + 'a>),
    Shared(Box<dyn Iterator<Item = SharedEntry<'a, K>> + 'a>),
}


impl<'a, K: Ord + Clone> Iterator for PostingIter<'a, K> {
    type Item = (&'a K, Cow<'a, RoaringBitmap>);

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            IterInner::Building(entries) => entries
                .next()
                .map(|(key, posting)| (key, Cow::Borrowed(posting))),
            IterInner::Shared(entries) => entries
                .next()
                .map(|(key, posting)| (key, Cow::Borrowed(posting.as_ref()))),
        }
    }
}


#[derive(Clone, Debug)]
enum Backing<K: Ord + Clone> {
    Building(BTreeMap<K, RoaringBitmap>),
    Shared(SharedPostings<K>),
}


/// One ordered posting dictionary. All index code reads and writes postings
/// through this type; nothing else may depend on the backing.
#[derive(Clone, Debug)]
pub struct PostingMap<K: Ord + Clone> {
    backing: Backing<K>,
}


impl<K: Ord + Clone> Default for PostingMap<K> {
    fn default() -> Self {
        Self::new()
    }
}


impl<K: Ord + Clone> PartialEq for PostingMap<K> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self
                .iter()
                .zip(other.iter())
                .all(|((a, left), (b, right))| a == b && left.as_ref() == right.as_ref())
    }
}


impl<K: Ord + Clone> PostingMap<K> {
    pub fn new() -> Self {
        Self {
            backing: Backing::Building(BTreeMap::new()),
        }
    }

    pub fn from_btree_map(entries: BTreeMap<K, RoaringBitmap>) -> Self {
        Self {
            backing: Backing::Building(entries),
        }
    }

    pub fn is_shared(&self) -> bool {
        matches!(self.backing, Backing::Shared(_))
    }

    /// Switch to the persistent backing. Idempotent; contents are unchanged.
    pub fn freeze(&mut self) {
        if let Backing::Building(entries) = &mut self.backing {
            let entries = std::mem::take(entries);
            self.backing = Backing::Shared(SharedPostings::from_building(entries));
        }
    }

    pub fn len(&self) -> usize {
        match &self.backing {
            Backing::Building(entries) => entries.len(),
            Backing::Shared(shared) => shared.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn contains_key(&self, key: &K) -> bool {
        match &self.backing {
            Backing::Building(entries) => entries.contains_key(key),
            Backing::Shared(shared) => shared.contains_key(key),
        }
    }

    pub fn get(&self, key: &K) -> Option<Cow<'_, RoaringBitmap>> {
        match &self.backing {
            Backing::Building(entries) => entries.get(key).map(Cow::Borrowed),
            Backing::Shared(shared) => shared.get(key),
        }
    }

    pub fn cardinality(&self, key: &K) -> u64 {
        match &self.backing {
            Backing::Building(entries) => entries.get(key).map_or(0, RoaringBitmap::len),
            Backing::Shared(shared) => shared.cardinality(key),
        }
    }

    pub fn first_key(&self) -> Option<&K> {
        match &self.backing {
            Backing::Building(entries) => entries.first_key_value().map(|(key, _)| key),
            Backing::Shared(shared) => shared.first_key(),
        }
    }

    pub fn last_key(&self) -> Option<&K> {
        match &self.backing {
            Backing::Building(entries) => entries.last_key_value().map(|(key, _)| key),
            Backing::Shared(shared) => shared.last_key(),
        }
    }

    /// `entry(key).or_default().insert(slot)`.
    pub fn insert_slot(&mut self, key: K, slot: u32) -> bool {
        match &mut self.backing {
            Backing::Building(entries) => entries.entry(key).or_default().insert(slot),
            Backing::Shared(shared) => shared.insert_slot(&key, slot),
        }
    }

    /// `insert(key, posting)`: the posting replaces any previous one.
    pub fn replace(&mut self, key: K, posting: RoaringBitmap) {
        match &mut self.backing {
            Backing::Building(entries) => {
                entries.insert(key, posting);
            }
            Backing::Shared(shared) => shared.replace(&key, posting),
        }
    }

    /// `append_shifted_bitmap(entry(key).or_default(), source, base)`.
    pub fn append_shifted(
        &mut self,
        key: K,
        source: &RoaringBitmap,
        base: u32,
    ) -> Result<(), String> {
        match &mut self.backing {
            Backing::Building(entries) => {
                if let Some(max) = source.max() {
                    base.checked_add(max).ok_or("record slot overflow")?;
                }
                entries
                    .entry(key)
                    .or_default()
                    .append(source.iter().map(|slot| base + slot))
                    .map_err(|_| "batch bitmap slots overlap or are out of order")?;
                Ok(())
            }
            Backing::Shared(shared) => shared.append_shifted(&key, source, base),
        }
    }

    pub fn retain_keys(&mut self, mut keep: impl FnMut(&K) -> bool) {
        match &mut self.backing {
            Backing::Building(entries) => entries.retain(|key, _| keep(key)),
            Backing::Shared(shared) => shared.retain_keys(keep),
        }
    }

    /// Remove every key but keep the current backing.
    pub fn clear(&mut self) {
        match &mut self.backing {
            Backing::Building(entries) => entries.clear(),
            Backing::Shared(shared) => shared.clear(),
        }
    }

    pub fn iter(&self) -> PostingIter<'_, K> {
        self.range(Bound::Unbounded, Bound::Unbounded, false)
    }

    pub fn range<'a>(
        &'a self,
        lower: Bound<&K>,
        upper: Bound<&K>,
        descending: bool,
    ) -> PostingIter<'a, K> {
        let inner = match &self.backing {
            Backing::Building(entries) => {
                let range = entries.range::<K, _>((lower, upper));
                IterInner::Building(if descending {
                    Box::new(range.rev())
                } else {
                    Box::new(range)
                })
            }
            Backing::Shared(shared) => IterInner::Shared(shared.entries(lower, upper, descending)),
        };
        PostingIter { inner }
    }

    pub fn to_btree_map(&self) -> BTreeMap<K, RoaringBitmap> {
        self.iter()
            .map(|(key, posting)| (key.clone(), posting.into_owned()))
            .collect()
    }

    pub fn into_entries(self) -> Vec<(K, RoaringBitmap)> {
        match self.backing {
            Backing::Building(entries) => entries.into_iter().collect(),
            shared @ Backing::Shared(_) => Self { backing: shared }
                .iter()
                .map(|(key, posting)| (key.clone(), posting.into_owned()))
                .collect(),
        }
    }

    /// Sum of the stored bitmaps' serialized sizes (a planning estimate).
    pub fn serialized_bytes(&self) -> usize {
        match &self.backing {
            Backing::Building(entries) => {
                entries.values().map(RoaringBitmap::serialized_size).sum()
            }
            Backing::Shared(shared) => shared.serialized_bytes(),
        }
    }

    /// True when `valid` holds for every stored bitmap (whole or chunk).
    pub fn all_bitmaps(&self, mut valid: impl FnMut(&RoaringBitmap) -> bool) -> bool {
        match &self.backing {
            Backing::Building(entries) => entries.values().all(|posting| valid(posting)),
            Backing::Shared(shared) => shared.all_parts(valid),
        }
    }
}


#[cfg(test)]
#[path = "posting_map.test.rs"]
mod tests;
