//! Gram key -> record-slot postings for one bigram alphabet half.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};

use roaring::RoaringBitmap;

use crate::posting_map::PostingMap;

/// `Building` keeps the construction-time hash map. `Shared` is a frozen
/// [`PostingMap`], which gives cheap clones and chunk-sized writes.
#[derive(Clone, Debug)]
pub enum GramMap {
    Building(HashMap<u64, RoaringBitmap>),
    Shared(PostingMap<u64>),
}

impl Default for GramMap {
    fn default() -> Self {
        Self::Building(HashMap::new())
    }
}

impl GramMap {
    pub fn from_hash_map(entries: HashMap<u64, RoaringBitmap>) -> Self {
        Self::Building(entries)
    }

    pub fn is_shared(&self) -> bool {
        matches!(self, Self::Shared(_))
    }

    /// Switch to the persistent backing. Idempotent; contents are unchanged.
    pub fn freeze(&mut self) {
        if self.is_shared() {
            return;
        }
        if let Self::Building(entries) = self {
            let ordered = std::mem::take(entries)
                .into_iter()
                .collect::<BTreeMap<_, _>>();
            let mut shared = PostingMap::from_btree_map(ordered);
            shared.freeze();
            *self = Self::Shared(shared);
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Self::Building(entries) => entries.len(),
            Self::Shared(shared) => shared.len(),
        }
    }

    pub fn get(&self, key: u64) -> Option<Cow<'_, RoaringBitmap>> {
        match self {
            Self::Building(entries) => entries.get(&key).map(Cow::Borrowed),
            Self::Shared(shared) => shared.get(&key),
        }
    }

    pub fn cardinality(&self, key: u64) -> u64 {
        match self {
            Self::Building(entries) => entries.get(&key).map_or(0, RoaringBitmap::len),
            Self::Shared(shared) => shared.cardinality(&key),
        }
    }

    pub fn insert_slot(&mut self, key: u64, slot: u32) -> bool {
        match self {
            Self::Building(entries) => entries.entry(key).or_default().insert(slot),
            Self::Shared(shared) => shared.insert_slot(key, slot),
        }
    }

    pub fn replace(&mut self, key: u64, posting: RoaringBitmap) {
        match self {
            Self::Building(entries) => {
                entries.insert(key, posting);
            }
            Self::Shared(shared) => shared.replace(key, posting),
        }
    }

    pub fn append_shifted(
        &mut self,
        key: u64,
        source: &RoaringBitmap,
        base: u32,
    ) -> Result<(), String> {
        match self {
            Self::Building(entries) => {
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
            Self::Shared(shared) => shared.append_shifted(key, source, base),
        }
    }

    /// Visit every gram in ascending key order.
    pub fn for_each_sorted(
        &self,
        mut visit: impl FnMut(&u64, &RoaringBitmap) -> Result<(), String>,
    ) -> Result<(), String> {
        match self {
            Self::Building(entries) => {
                let mut keys = entries.keys().collect::<Vec<_>>();
                keys.sort_unstable();
                for key in keys {
                    visit(key, &entries[key])?;
                }
            }
            Self::Shared(shared) => {
                for (key, posting) in shared.iter() {
                    visit(key, posting.as_ref())?;
                }
            }
        }
        Ok(())
    }

    pub fn serialized_bytes(&self) -> usize {
        match self {
            Self::Building(entries) => {
                entries.values().map(RoaringBitmap::serialized_size).sum()
            }
            Self::Shared(shared) => shared.serialized_bytes(),
        }
    }

    /// Number of stored bitmaps: one per key while building, one per chunk
    /// once shared. Used only by byte estimates.
    pub fn all_bitmaps(&self, valid: impl FnMut(&RoaringBitmap) -> bool) -> bool {
        match self {
            Self::Building(entries) => entries.values().all(valid),
            Self::Shared(shared) => shared.all_bitmaps(valid),
        }
    }

    pub fn for_each_chunk(
        &self,
        mut visit: impl FnMut(u64, crate::posting_map::ChunkId, &RoaringBitmap) -> Result<(), String>,
    ) -> Result<(), String> {
        match self {
            Self::Building(entries) => {
                let ordered = entries
                    .iter()
                    .map(|(key, posting)| (*key, posting.clone()))
                    .collect::<BTreeMap<_, _>>();
                crate::posting_map::PostingMap::from_btree_map(ordered)
                    .for_each_chunk(|key, chunk, posting| visit(*key, chunk, posting))?;
            }
            Self::Shared(shared) => {
                shared.for_each_chunk(|key, chunk, posting| visit(*key, chunk, posting))?;
            }
        }
        Ok(())
    }

    pub(crate) fn shared_map(&self) -> Option<&crate::posting_map::PostingMap<u64>> {
        match self {
            Self::Shared(shared) => Some(shared),
            Self::Building(_) => None,
        }
    }

    pub fn diff_chunks(
        &self,
        older: &Self,
        visit: impl FnMut(crate::posting_map::ChunkChange<'_, u64>) -> Result<(), String>,
    ) -> Result<bool, String> {
        match (self, older) {
            (Self::Shared(new), Self::Shared(old)) => new.diff_chunks(old, visit),
            _ => Ok(false),
        }
    }

    #[cfg(test)]
    pub fn stored_chunk_count(&self) -> usize {
        match self {
            Self::Building(entries) => entries
                .values()
                .map(|posting| crate::posting_map::split_chunks(posting.clone()).len().max(1))
                .sum(),
            Self::Shared(shared) => shared.stored_chunk_count(),
        }
    }

    pub fn to_btree_map(&self) -> BTreeMap<u64, RoaringBitmap> {
        match self {
            Self::Building(entries) => entries
                .iter()
                .map(|(key, posting)| (*key, posting.clone()))
                .collect(),
            Self::Shared(shared) => shared.to_btree_map(),
        }
    }
}

#[cfg(test)]
#[path = "gram_map.test.rs"]
mod tests;
