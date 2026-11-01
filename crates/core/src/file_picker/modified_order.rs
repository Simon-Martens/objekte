//! Modification-time order over stable file slots. Updates supersede old
//! positions in a small overlay; the immutable bigram postings never move.

use super::FileEntry;
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
};


#[derive(Clone, Eq, PartialEq)]
struct Key {
    modified: i128,
    id: String,
    slot: u32,
}


impl Ord for Key {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .modified
            .cmp(&self.modified)
            .then_with(|| self.id.cmp(&other.id))
            .then_with(|| self.slot.cmp(&other.slot))
    }
}


impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}


#[derive(Default)]
pub(super) struct ModifiedOrder {
    base: Vec<u32>,
    superseded: Vec<u64>,
    overlay: BTreeSet<Key>,
    by_slot: BTreeMap<u32, Key>,
    superseded_count: usize,
}


fn id<'a>(entry: &FileEntry, ids: &'a [u8]) -> &'a str {
    let start = entry.id_start as usize;
    std::str::from_utf8(&ids[start..start + entry.id_len as usize]).expect("validated ID")
}

fn timestamp(entry: &FileEntry) -> i128 {
    entry
        .stamp
        .modified
        .parse()
        .expect("validated modification time")
}


impl ModifiedOrder {
    pub(super) fn rebuild(&mut self, files: &[Option<FileEntry>], ids: &[u8]) {
        let times = files
            .iter()
            .map(|entry| entry.as_ref().map_or(i128::MIN, timestamp))
            .collect::<Vec<_>>();
        self.base = files
            .iter()
            .enumerate()
            .filter_map(|(slot, entry)| {
                entry
                    .as_ref()
                    .map(|_| u32::try_from(slot).expect("file slot fits u32"))
            })
            .collect();
        self.base.sort_unstable_by(|&a, &b| {
            times[b as usize].cmp(&times[a as usize]).then_with(|| {
                id(files[a as usize].as_ref().unwrap(), ids)
                    .cmp(id(files[b as usize].as_ref().unwrap(), ids))
            })
        });
        self.superseded.clear();
        self.overlay.clear();
        self.by_slot.clear();
        self.superseded_count = 0;
    }

    pub(super) fn restore(&mut self, order: Vec<u32>, files: &[Option<FileEntry>]) -> bool {
        if order.len() != files.iter().filter(|entry| entry.is_some()).count() {
            return false;
        }
        let mut seen = vec![0_u64; files.len().div_ceil(64)];
        for &slot in &order {
            let slot = slot as usize;
            if !files.get(slot).is_some_and(Option::is_some) {
                return false;
            }
            let word = &mut seen[slot / 64];
            let bit = 1_u64 << (slot % 64);
            if *word & bit != 0 {
                return false;
            }
            *word |= bit;
        }
        self.base = order;
        self.superseded.clear();
        self.overlay.clear();
        self.by_slot.clear();
        self.superseded_count = 0;
        true
    }

    fn is_superseded(&self, slot: u32) -> bool {
        let slot = slot as usize;
        self.superseded
            .get(slot / 64)
            .is_some_and(|word| word & (1_u64 << (slot % 64)) != 0)
    }

    pub(super) fn touch(&mut self, slot: usize, files: &[Option<FileEntry>], ids: &[u8]) {
        let slot = u32::try_from(slot).expect("file slot fits u32");
        if let Some(old) = self.by_slot.remove(&slot) {
            self.overlay.remove(&old);
        }
        let index = slot as usize / 64;
        if self.superseded.len() <= index {
            self.superseded.resize(index + 1, 0);
        }
        let bit = 1_u64 << (slot % 64);
        if self.superseded[index] & bit == 0 {
            self.superseded[index] |= bit;
            self.superseded_count += 1;
        }
        if let Some(entry) = files[slot as usize].as_ref() {
            let key = Key {
                modified: timestamp(entry),
                id: id(entry, ids).into(),
                slot,
            };
            self.overlay.insert(key.clone());
            self.by_slot.insert(slot, key);
        }
    }

    pub(super) fn maybe_compact(&mut self, files: &[Option<FileEntry>], ids: &[u8]) {
        if self.superseded_count > self.base.len().saturating_div(8).max(4096) {
            self.rebuild(files, ids);
        }
    }

    pub(super) fn compare_slots(
        files: &[Option<FileEntry>],
        ids: &[u8],
        a: usize,
        b: usize,
    ) -> Ordering {
        let a = files[a].as_ref().expect("live modified-order slot");
        let b = files[b].as_ref().expect("live modified-order slot");
        timestamp(b)
            .cmp(&timestamp(a))
            .then_with(|| id(a, ids).cmp(id(b, ids)))
    }

    pub(super) fn iter<'a>(
        &'a self,
        files: &'a [Option<FileEntry>],
        ids: &'a [u8],
    ) -> ModifiedIter<'a> {
        ModifiedIter {
            order: self,
            files,
            ids,
            base_at: 0,
            overlay: self.overlay.iter().peekable(),
        }
    }

    pub(super) fn snapshot(&self, files: &[Option<FileEntry>], ids: &[u8]) -> Vec<u32> {
        self.iter(files, ids).collect()
    }
}


pub(super) struct ModifiedIter<'a> {
    order: &'a ModifiedOrder,
    files: &'a [Option<FileEntry>],
    ids: &'a [u8],
    base_at: usize,
    overlay: std::iter::Peekable<std::collections::btree_set::Iter<'a, Key>>,
}


impl Iterator for ModifiedIter<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(&slot) = self.order.base.get(self.base_at) {
            if !self.order.is_superseded(slot) && self.files[slot as usize].is_some() {
                break;
            }
            self.base_at += 1;
        }
        let base = self.order.base.get(self.base_at).copied();
        let overlay = self.overlay.peek().copied();
        let take_overlay = match (base, overlay) {
            (None, None) => return None,
            (None, Some(_)) => true,
            (Some(_), None) => false,
            (Some(slot), Some(key)) => {
                let entry = self.files[slot as usize].as_ref().expect("live base slot");
                key.modified > timestamp(entry)
                    || (key.modified == timestamp(entry) && key.id.as_str() < id(entry, self.ids))
            }
        };
        if take_overlay {
            self.overlay.next().map(|key| key.slot)
        } else {
            self.base_at += 1;
            base
        }
    }
}
