//! Query-owned membership grouped by each record's segment domain.

use std::collections::BTreeMap;
use std::ops::BitOrAssign;

use roaring::RoaringBitmap;

use crate::index::traits::segment::{SegmentId, SegmentRecordAddress};
use crate::model::{CollectionId, RecordSlot};

/// Compact membership spanning segments without merging local slot domains.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SegmentRecordSet {
    members: BTreeMap<(CollectionId, SegmentId), RoaringBitmap>,
}

impl SegmentRecordSet {
    pub(crate) fn insert(&mut self, address: SegmentRecordAddress) -> bool {
        self.members
            .entry((address.collection, address.segment))
            .or_default()
            .insert(address.slot.0)
    }

    pub(crate) fn len(&self) -> u64 {
        self.members
            .values()
            .fold(0u64, |total, slots| total.saturating_add(slots.len()))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.members.values().all(RoaringBitmap::is_empty)
    }

    pub(crate) fn contains(&self, address: SegmentRecordAddress) -> bool {
        self.members
            .get(&(address.collection, address.segment))
            .is_some_and(|slots| slots.contains(address.slot.0))
    }

    pub(crate) fn local(
        &self,
        collection: CollectionId,
        segment: SegmentId,
    ) -> Option<&RoaringBitmap> {
        self.members.get(&(collection, segment))
    }

    pub(crate) fn insert_local(
        &mut self,
        collection: CollectionId,
        segment: SegmentId,
        slots: RoaringBitmap,
    ) {
        if slots.is_empty() {
            return;
        }
        *self.members.entry((collection, segment)).or_default() |= slots;
    }

    pub(crate) fn intersect(&mut self, other: &Self) {
        for (&(collection, segment), slots) in &mut self.members {
            if let Some(other) = other.members.get(&(collection, segment)) {
                *slots &= other;
            } else {
                slots.clear();
            }
        }
        self.members.retain(|_, slots| !slots.is_empty());
    }

    pub(crate) fn union(&mut self, other: &Self) {
        for (&(collection, segment), slots) in &other.members {
            self.members
                .entry((collection, segment))
                .or_default()
                .bitor_assign(slots);
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = SegmentRecordAddress> + '_ {
        self.members
            .iter()
            .flat_map(|(&(collection, segment), slots)| {
                slots.iter().map(move |slot| SegmentRecordAddress {
                    collection,
                    segment,
                    slot: RecordSlot(slot),
                })
            })
    }
}
