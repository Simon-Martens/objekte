//! Dual-backed tables and orders used by an immutable shard.
//!
//! `Building` keeps the construction-time `Vec`/`HashMap`; `Shared` uses the
//! persistent `imbl` containers, so a shard clone is cheap and a write copies
//! only the changed path.

use std::collections::HashMap;
use std::hash::Hash;
use std::ops::Range;

use crate::model::RecordSlot;

/// Dense table indexed by record slot or row position.
#[derive(Clone, Debug)]
pub(crate) enum SlotTable<T: Clone> {
    Building(Vec<T>),
    Shared(imbl::Vector<T>),
}

impl<T: Clone + PartialEq> PartialEq for SlotTable<T> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().zip(other.iter()).all(|(a, b)| a == b)
    }
}

impl<T: Clone> SlotTable<T> {
    pub(crate) fn new() -> Self {
        Self::Building(Vec::new())
    }

    pub(crate) fn from_vec(values: Vec<T>) -> Self {
        Self::Building(values)
    }

    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Building(values) => values.len(),
            Self::Shared(values) => values.len(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn get(&self, index: usize) -> Option<&T> {
        match self {
            Self::Building(values) => values.get(index),
            Self::Shared(values) => values.get(index),
        }
    }

    pub(crate) fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        match self {
            Self::Building(values) => values.get_mut(index),
            Self::Shared(values) => values.get_mut(index),
        }
    }

    pub(crate) fn for_each_mut(&mut self, mut visit: impl FnMut(&mut T)) {
        match self {
            Self::Building(values) => {
                for value in values {
                    visit(value);
                }
            }
            Self::Shared(values) => {
                for index in 0..values.len() {
                    if let Some(value) = values.get_mut(index) {
                        visit(value);
                    }
                }
            }
        }
    }

    pub(crate) fn last(&self) -> Option<&T> {
        match self {
            Self::Building(values) => values.last(),
            Self::Shared(values) => values.back(),
        }
    }

    pub(crate) fn push(&mut self, value: T) {
        match self {
            Self::Building(values) => values.push(value),
            Self::Shared(values) => values.push_back(value),
        }
    }

    pub(crate) fn extend(&mut self, values: impl IntoIterator<Item = T>) {
        match self {
            Self::Building(target) => {
                target.extend(values);
            }
            Self::Shared(target) => {
                target.extend(values);
            }
        }
    }

    pub(crate) fn set(&mut self, index: usize, value: T) {
        match self {
            Self::Building(values) => values[index] = value,
            Self::Shared(values) => {
                values.set(index, value);
            }
        }
    }

    pub(crate) fn iter(&self) -> Box<dyn Iterator<Item = &T> + '_> {
        match self {
            Self::Building(values) => Box::new(values.iter()),
            Self::Shared(values) => Box::new(values.iter()),
        }
    }

    pub(crate) fn iter_from(&self, index: usize) -> Box<dyn Iterator<Item = &T> + '_> {
        match self {
            Self::Building(values) => Box::new(values[index.min(values.len())..].iter()),
            Self::Shared(values) => Box::new(values.iter().skip(index)),
        }
    }

    pub(crate) fn partition_point(&self, mut predicate: impl FnMut(&T) -> bool) -> usize {
        match self {
            Self::Building(values) => values.partition_point(|value| predicate(value)),
            Self::Shared(values) => {
                match values.binary_search_by(|value| {
                    if predicate(value) {
                        std::cmp::Ordering::Less
                    } else {
                        std::cmp::Ordering::Greater
                    }
                }) {
                    Ok(index) | Err(index) => index,
                }
            }
        }
    }

    pub(crate) fn capacity(&self) -> usize {
        match self {
            Self::Building(values) => values.capacity(),
            Self::Shared(values) => values.len(),
        }
    }

    pub(crate) fn into_vec(self) -> Vec<T> {
        match self {
            Self::Building(values) => values,
            Self::Shared(values) => values.iter().cloned().collect(),
        }
    }

    pub(crate) fn to_vec(&self) -> Vec<T> {
        match self {
            Self::Building(values) => values.clone(),
            Self::Shared(values) => values.iter().cloned().collect(),
        }
    }

    /// Switch to the persistent container. Idempotent; contents are unchanged.
    pub(crate) fn freeze(&mut self) {
        if let Self::Building(values) = self {
            *self = Self::Shared(std::mem::take(values).into());
        }
    }

    pub(crate) fn is_shared(&self) -> bool {
        matches!(self, Self::Shared(_))
    }
}

/// Ordered sequence of record slots: a sort order or a subsequence of one.
#[derive(Clone, Debug)]
pub(crate) enum SlotOrder {
    Building(Vec<RecordSlot>),
    Shared(imbl::Vector<RecordSlot>),
}

impl PartialEq for SlotOrder {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter())
    }
}

impl SlotOrder {
    pub(crate) fn from_vec(values: Vec<RecordSlot>) -> Self {
        Self::Building(values)
    }

    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Building(values) => values.len(),
            Self::Shared(values) => values.len(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn get(&self, index: usize) -> Option<RecordSlot> {
        match self {
            Self::Building(values) => values.get(index).copied(),
            Self::Shared(values) => values.get(index).copied(),
        }
    }

    pub(crate) fn first(&self) -> Option<RecordSlot> {
        match self {
            Self::Building(values) => values.first().copied(),
            Self::Shared(values) => values.front().copied(),
        }
    }

    pub(crate) fn last(&self) -> Option<RecordSlot> {
        match self {
            Self::Building(values) => values.last().copied(),
            Self::Shared(values) => values.back().copied(),
        }
    }

    pub(crate) fn iter(&self) -> Box<dyn Iterator<Item = RecordSlot> + '_> {
        match self {
            Self::Building(values) => Box::new(values.iter().copied()),
            Self::Shared(values) => Box::new(values.iter().copied()),
        }
    }

    pub(crate) fn iter_range(&self, range: Range<usize>) -> Box<dyn Iterator<Item = RecordSlot> + '_> {
        match self {
            Self::Building(values) => Box::new(values[range].iter().copied()),
            Self::Shared(values) => Box::new(
                values
                    .iter()
                    .skip(range.start)
                    .take(range.end.saturating_sub(range.start))
                    .copied(),
            ),
        }
    }

    pub(crate) fn partition_point(&self, mut predicate: impl FnMut(RecordSlot) -> bool) -> usize {
        match self {
            Self::Building(values) => values.partition_point(|slot| predicate(*slot)),
            Self::Shared(values) => {
                match values.binary_search_by(|slot| {
                    if predicate(*slot) {
                        std::cmp::Ordering::Less
                    } else {
                        std::cmp::Ordering::Greater
                    }
                }) {
                    Ok(index) | Err(index) => index,
                }
            }
        }
    }

    /// Insert `slot` at the position given by `compare`, which returns how an
    /// existing slot orders relative to the new one.
    pub(crate) fn insert_sorted(
        &mut self,
        slot: RecordSlot,
        mut compare: impl FnMut(RecordSlot) -> std::cmp::Ordering,
    ) {
        let index = self.partition_point(|existing| compare(existing) != std::cmp::Ordering::Greater);
        match self {
            Self::Building(values) => values.insert(index, slot),
            Self::Shared(values) => values.insert(index, slot),
        }
    }

    pub(crate) fn insert_at(&mut self, index: usize, slot: RecordSlot) {
        match self {
            Self::Building(values) => values.insert(index, slot),
            Self::Shared(values) => values.insert(index, slot),
        }
    }

    pub(crate) fn retain(&mut self, mut keep: impl FnMut(RecordSlot) -> bool) {
        match self {
            Self::Building(values) => values.retain(|slot| keep(*slot)),
            Self::Shared(values) => {
                let kept = values
                    .iter()
                    .copied()
                    .filter(|slot| keep(*slot))
                    .collect::<Vec<_>>();
                *values = imbl::Vector::from(kept);
            }
        }
    }

    pub(crate) fn as_slice(&self) -> Option<&[RecordSlot]> {
        match self {
            Self::Building(values) => Some(values),
            Self::Shared(_) => None,
        }
    }

    pub(crate) fn push(&mut self, slot: RecordSlot) {
        match self {
            Self::Building(values) => values.push(slot),
            Self::Shared(values) => values.push_back(slot),
        }
    }

    pub(crate) fn capacity(&self) -> usize {
        match self {
            Self::Building(values) => values.capacity(),
            Self::Shared(values) => values.len(),
        }
    }

    pub(crate) fn to_vec(&self) -> Vec<RecordSlot> {
        match self {
            Self::Building(values) => values.clone(),
            Self::Shared(values) => values.iter().copied().collect(),
        }
    }

    pub(crate) fn freeze(&mut self) {
        if let Self::Building(values) = self {
            *self = Self::Shared(std::mem::take(values).into());
        }
    }

    pub(crate) fn is_shared(&self) -> bool {
        matches!(self, Self::Shared(_))
    }
}

/// Hash map with cheap clones once shared.
#[derive(Clone, Debug)]
pub(crate) enum SharedMap<K: Clone + Eq + Hash, V: Clone> {
    Building(HashMap<K, V>),
    Shared(imbl::HashMap<K, V>),
}

impl<K: Clone + Eq + Hash, V: Clone + PartialEq> PartialEq for SharedMap<K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().all(|(key, value)| other.get(key) == Some(value))
    }
}

impl<K: Clone + Eq + Hash, V: Clone> SharedMap<K, V> {
    pub(crate) fn new() -> Self {
        Self::Building(HashMap::new())
    }

    pub(crate) fn from_hash_map(values: HashMap<K, V>) -> Self {
        Self::Building(values)
    }

    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Building(values) => values.len(),
            Self::Shared(values) => values.len(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn capacity(&self) -> usize {
        match self {
            Self::Building(values) => values.capacity(),
            Self::Shared(values) => values.len(),
        }
    }

    pub(crate) fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: std::borrow::Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        match self {
            Self::Building(values) => values.get(key),
            Self::Shared(values) => values.get(key),
        }
    }

    pub(crate) fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: std::borrow::Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        match self {
            Self::Building(values) => values.contains_key(key),
            Self::Shared(values) => values.contains_key(key),
        }
    }

    pub(crate) fn insert(&mut self, key: K, value: V) -> Option<V> {
        match self {
            Self::Building(values) => values.insert(key, value),
            Self::Shared(values) => values.insert(key, value),
        }
    }

    pub(crate) fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: std::borrow::Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        match self {
            Self::Building(values) => values.remove(key),
            Self::Shared(values) => values.remove(key),
        }
    }

    pub(crate) fn iter(&self) -> Box<dyn Iterator<Item = (&K, &V)> + '_> {
        match self {
            Self::Building(values) => Box::new(values.iter()),
            Self::Shared(values) => Box::new(values.iter()),
        }
    }

    pub(crate) fn freeze(&mut self) {
        if let Self::Building(values) = self {
            *self = Self::Shared(std::mem::take(values).into());
        }
    }

    pub(crate) fn is_shared(&self) -> bool {
        matches!(self, Self::Shared(_))
    }
}

#[cfg(test)]
#[path = "slot_store.test.rs"]
mod tests;
