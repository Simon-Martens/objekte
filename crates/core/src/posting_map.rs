//! Ordered key -> record-slot postings with two interchangeable backings.
//!
//! `Building` is the native `BTreeMap` used while an index is constructed and
//! by small immutable shards. `Shared` is a persistent (copy-on-write) ordered
//! map whose values are split into fixed slot ranges ("chunks"), so that a
//! clone is cheap and a write copies only one small chunk.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::ops::Bound;
use std::sync::Arc;

use imbl::OrdMap;
use roaring::RoaringBitmap;

/// High bits of a record slot; names one fixed slot range of a posting.
pub type ChunkId = u32;

/// Production chunks hold 65,536 slots, one roaring container. The forced
/// validation feature uses 256 so small test shards span several chunks.
#[cfg(not(feature = "force-shared-backing"))]
pub const POSTING_CHUNK_BITS: u32 = 16;
#[cfg(feature = "force-shared-backing")]
pub const POSTING_CHUNK_BITS: u32 = 8;

pub fn chunk_of(slot: u32) -> ChunkId {
    slot >> POSTING_CHUNK_BITS
}

fn chunk_mask(chunk: ChunkId) -> RoaringBitmap {
    let start = chunk << POSTING_CHUNK_BITS;
    let end = start | ((1u32 << POSTING_CHUNK_BITS) - 1);
    let mut mask = RoaringBitmap::new();
    mask.insert_range(start..=end);
    mask
}

/// Split one posting into per-chunk bitmaps, ascending by chunk. Every part
/// is non-empty. Slots keep their absolute values.
pub fn split_chunks(posting: RoaringBitmap) -> Vec<(ChunkId, RoaringBitmap)> {
    let (Some(min), Some(max)) = (posting.min(), posting.max()) else {
        return Vec::new();
    };
    let (first, last) = (chunk_of(min), chunk_of(max));
    if first == last {
        return vec![(first, posting)];
    }
    let span = u64::from(last - first) + 1;
    let mut parts = Vec::new();
    if posting.len() >= span.saturating_mul(4) {
        // Dense: one container-level intersection per chunk.
        for chunk in first..=last {
            let part = &posting & &chunk_mask(chunk);
            if !part.is_empty() {
                parts.push((chunk, part));
            }
        }
    } else {
        // Sparse: one pass over the members.
        let mut current: Option<(ChunkId, RoaringBitmap)> = None;
        for slot in posting.iter() {
            let chunk = chunk_of(slot);
            match &mut current {
                Some((open, part)) if *open == chunk => {
                    part.push(slot);
                }
                _ => {
                    if let Some(done) = current.take() {
                        parts.push(done);
                    }
                    let mut part = RoaringBitmap::new();
                    part.push(slot);
                    current = Some((chunk, part));
                }
            }
        }
        if let Some(done) = current {
            parts.push(done);
        }
    }
    parts
}


/// Persistent chunked postings. Invariants:
/// 1. every stored key has at least one entry;
/// 2. an entry's bitmap is non-empty, except the single entry `(key, 0)` of a
///    key whose whole posting is empty;
/// 3. the bitmap of `(key, chunk)` only holds slots of that chunk;
/// 4. `keys` is the number of distinct keys.
#[derive(Clone, Debug)]
pub struct SharedPostings<K: Ord + Clone> {
    chunks: OrdMap<(K, ChunkId), Arc<RoaringBitmap>>,
    keys: usize,
}


type ChunkEntry<'a, K> = (&'a (K, ChunkId), &'a Arc<RoaringBitmap>);


impl<K: Ord + Clone> Default for SharedPostings<K> {
    fn default() -> Self {
        Self {
            chunks: OrdMap::new(),
            keys: 0,
        }
    }
}


impl<K: Ord + Clone> SharedPostings<K> {
    fn key_bounds(key: &K) -> (Bound<(K, ChunkId)>, Bound<(K, ChunkId)>) {
        (
            Bound::Included((key.clone(), 0)),
            Bound::Included((key.clone(), ChunkId::MAX)),
        )
    }

    pub fn from_building(source: BTreeMap<K, RoaringBitmap>) -> Self {
        let mut keys = 0usize;
        let mut entries = Vec::new();
        for (key, posting) in source {
            keys += 1;
            let parts = split_chunks(posting);
            if parts.is_empty() {
                entries.push(((key, 0), Arc::new(RoaringBitmap::new())));
                continue;
            }
            for (chunk, part) in parts {
                entries.push(((key.clone(), chunk), Arc::new(part)));
            }
        }
        Self {
            chunks: entries.into_iter().collect(),
            keys,
        }
    }

    pub fn len(&self) -> usize {
        self.keys
    }

    pub fn contains_key(&self, key: &K) -> bool {
        self.chunks.range(Self::key_bounds(key)).next().is_some()
    }

    pub fn get(&self, key: &K) -> Option<Cow<'_, RoaringBitmap>> {
        let mut entries = self.chunks.range(Self::key_bounds(key));
        let (_, first) = entries.next()?;
        let Some((_, second)) = entries.next() else {
            return Some(Cow::Borrowed(first.as_ref()));
        };
        let mut whole = first.as_ref().clone();
        whole |= second.as_ref();
        for (_, more) in entries {
            whole |= more.as_ref();
        }
        Some(Cow::Owned(whole))
    }

    pub fn cardinality(&self, key: &K) -> u64 {
        self.chunks
            .range(Self::key_bounds(key))
            .map(|(_, part)| part.len())
            .sum()
    }

    fn max_slot(&self, key: &K) -> Option<u32> {
        self.chunks
            .range(Self::key_bounds(key))
            .next_back()
            .and_then(|(_, part)| part.max())
    }

    /// Add `part` (already restricted to `chunk`) to `key`.
    fn merge_part(&mut self, key: &K, chunk: ChunkId, part: RoaringBitmap) {
        let address = (key.clone(), chunk);
        if let Some(existing) = self.chunks.get_mut(&address) {
            *Arc::make_mut(existing) |= part;
            return;
        }
        if !self.contains_key(key) {
            self.keys += 1;
        } else if chunk != 0
            && self
                .chunks
                .get(&(key.clone(), 0))
                .is_some_and(|zero| zero.is_empty())
        {
            // Drop the empty-posting placeholder now that the key has members.
            self.chunks.remove(&(key.clone(), 0));
        }
        self.chunks.insert(address, Arc::new(part));
    }

    fn ensure_key(&mut self, key: &K) {
        if !self.contains_key(key) {
            self.keys += 1;
            self.chunks
                .insert((key.clone(), 0), Arc::new(RoaringBitmap::new()));
        }
    }

    pub fn insert_slot(&mut self, key: &K, slot: u32) -> bool {
        let chunk = chunk_of(slot);
        if let Some(existing) = self.chunks.get_mut(&(key.clone(), chunk)) {
            return Arc::make_mut(existing).insert(slot);
        }
        let mut part = RoaringBitmap::new();
        part.insert(slot);
        self.merge_part(key, chunk, part);
        true
    }

    pub fn append_shifted(
        &mut self,
        key: &K,
        source: &RoaringBitmap,
        base: u32,
    ) -> Result<(), String> {
        let Some(source_max) = source.max() else {
            self.ensure_key(key);
            return Ok(());
        };
        base.checked_add(source_max).ok_or("record slot overflow")?;
        let first_new = base + source.min().unwrap_or(0);
        if self
            .max_slot(key)
            .is_some_and(|existing| first_new <= existing)
        {
            return Err("batch bitmap slots overlap or are out of order".into());
        }
        let mut current: Option<(ChunkId, RoaringBitmap)> = None;
        for slot in source.iter() {
            let shifted = base + slot;
            let chunk = chunk_of(shifted);
            match &mut current {
                Some((open, part)) if *open == chunk => {
                    part.push(shifted);
                }
                _ => {
                    if let Some((done, part)) = current.take() {
                        self.merge_part(key, done, part);
                    }
                    let mut part = RoaringBitmap::new();
                    part.push(shifted);
                    current = Some((chunk, part));
                }
            }
        }
        if let Some((done, part)) = current {
            self.merge_part(key, done, part);
        }
        Ok(())
    }

    fn remove_key(&mut self, key: &K) -> bool {
        let addresses = self
            .chunks
            .range(Self::key_bounds(key))
            .map(|(address, _)| address.clone())
            .collect::<Vec<_>>();
        if addresses.is_empty() {
            return false;
        }
        for address in addresses {
            self.chunks.remove(&address);
        }
        self.keys -= 1;
        true
    }

    pub fn replace(&mut self, key: &K, posting: RoaringBitmap) {
        self.remove_key(key);
        let parts = split_chunks(posting);
        if parts.is_empty() {
            self.ensure_key(key);
            return;
        }
        for (chunk, part) in parts {
            self.merge_part(key, chunk, part);
        }
    }

    pub fn retain_keys(&mut self, mut keep: impl FnMut(&K) -> bool) {
        let mut doomed = Vec::new();
        let mut previous: Option<&K> = None;
        for ((key, _), _) in self.chunks.iter() {
            if previous == Some(key) {
                continue;
            }
            previous = Some(key);
            if !keep(key) {
                doomed.push(key.clone());
            }
        }
        for key in doomed {
            self.remove_key(&key);
        }
    }

    pub fn clear(&mut self) {
        self.chunks = OrdMap::new();
        self.keys = 0;
    }

    pub fn first_key(&self) -> Option<&K> {
        self.chunks.get_min().map(|(address, _)| &address.0)
    }

    pub fn last_key(&self) -> Option<&K> {
        self.chunks.get_max().map(|(address, _)| &address.0)
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn serialized_bytes(&self) -> usize {
        self.chunks
            .iter()
            .map(|(_, part)| part.serialized_size())
            .sum()
    }

    pub fn all_parts(&self, mut valid: impl FnMut(&RoaringBitmap) -> bool) -> bool {
        self.chunks.iter().all(|(_, part)| valid(part))
    }

    fn entries<'a>(
        &'a self,
        lower: Bound<&K>,
        upper: Bound<&K>,
        descending: bool,
    ) -> Box<dyn Iterator<Item = ChunkEntry<'a, K>> + 'a> {
        let lower_address = match lower {
            Bound::Included(key) => Bound::Included((key.clone(), 0)),
            Bound::Excluded(key) => Bound::Excluded((key.clone(), ChunkId::MAX)),
            Bound::Unbounded => Bound::Unbounded,
        };
        let upper_address = match upper {
            Bound::Included(key) => Bound::Included((key.clone(), ChunkId::MAX)),
            Bound::Excluded(key) => Bound::Excluded((key.clone(), 0)),
            Bound::Unbounded => Bound::Unbounded,
        };
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
        let range = self.chunks.range((lower_address, upper_address));
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
    Shared {
        entries: Box<dyn Iterator<Item = ChunkEntry<'a, K>> + 'a>,
        ahead: Option<ChunkEntry<'a, K>>,
    },
}


impl<'a, K: Ord + Clone> Iterator for PostingIter<'a, K> {
    type Item = (&'a K, Cow<'a, RoaringBitmap>);

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            IterInner::Building(entries) => entries
                .next()
                .map(|(key, posting)| (key, Cow::Borrowed(posting))),
            IterInner::Shared { entries, ahead } => {
                let (address, first) = ahead.take().or_else(|| entries.next())?;
                let key = &address.0;
                let mut whole: Option<RoaringBitmap> = None;
                for (next_address, part) in entries.by_ref() {
                    if next_address.0 != *key {
                        *ahead = Some((next_address, part));
                        break;
                    }
                    let target = whole.get_or_insert_with(|| first.as_ref().clone());
                    *target |= part.as_ref();
                }
                Some((key, whole.map_or(Cow::Borrowed(first.as_ref()), Cow::Owned)))
            }
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
            Backing::Shared(shared) => IterInner::Shared {
                entries: shared.entries(lower, upper, descending),
                ahead: None,
            },
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

    /// Number of stored bitmaps: one per key while building, one per chunk
    /// once shared. Used only by byte estimates and manual measurements.
    #[cfg(test)]
    pub fn stored_chunk_count(&self) -> usize {
        match &self.backing {
            Backing::Building(entries) => entries
                .values()
                .map(|posting| split_chunks(posting.clone()).len().max(1))
                .sum(),
            Backing::Shared(shared) => shared.chunk_count(),
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
