//! Sealed membership of one local record-slot domain.

use std::fmt::Debug;
use std::sync::Arc;

use roaring::RoaringBitmap;

use crate::model::RecordSlot;
use crate::query_access::membership::MembershipView;

use super::transfer::{StorageError, StorageResult};

pub(crate) type VisitLiveBlock<'a> = dyn FnMut(u32, MembershipView<'_>) -> StorageResult<()> + 'a;

/// Membership in a pinned slot domain. Readers can fetch point membership
/// or expose bounded, ordered blocks from that same revision.
pub(crate) trait LiveSlotReader: Debug + Send + Sync {
    fn slot_count(&self) -> u32;
    fn live_count(&self) -> u64;
    fn contains(&self, slot: RecordSlot) -> StorageResult<bool>;
    fn visit_blocks(&self, visit: &mut VisitLiveBlock<'_>) -> StorageResult<()>;
    fn borrowed_bitmap(&self) -> Option<&RoaringBitmap> {
        None
    }
    fn retired(&self, slots: &[RecordSlot]) -> StorageResult<ActiveRecords>;
}

#[derive(Clone, Debug)]
pub(crate) struct ActiveRecords {
    reader: Arc<dyn LiveSlotReader>,
    slot_count: u32,
    live_count: u64,
}

impl ActiveRecords {
    pub(crate) fn new(reader: impl LiveSlotReader + 'static) -> Self {
        Self {
            slot_count: reader.slot_count(),
            live_count: reader.live_count(),
            reader: Arc::new(reader),
        }
    }

    #[cfg(test)]
    pub(crate) fn all(slot_count: usize) -> Result<Self, String> {
        Ok(super::test_factory().live_records(slot_count, InitialLiveness::All)?)
    }

    /// Temporary selection used by projection and query planning.
    pub(crate) fn from_bitmap(slot_count: usize, members: RoaringBitmap) -> Result<Self, String> {
        let count = u32::try_from(slot_count).map_err(|_| "too many records")?;
        if members.max().is_some_and(|slot| slot >= count) {
            return Err("active record slot out of bounds".into());
        }
        Ok(Self::new(Selection { count, members }))
    }

    pub(crate) fn full_selection(slot_count: usize) -> Result<Self, String> {
        let count = u32::try_from(slot_count).map_err(|_| "too many records")?;
        let mut members = RoaringBitmap::new();
        members.insert_range(0..count);
        Ok(Self::new(Selection { count, members }))
    }

    pub(crate) fn empty() -> Self {
        Self::new(Selection {
            count: 0,
            members: RoaringBitmap::new(),
        })
    }

    pub(crate) fn slot_count(&self) -> u32 {
        self.slot_count
    }

    pub(crate) fn len(&self) -> u64 {
        self.live_count
    }

    pub(crate) fn contains(&self, slot: RecordSlot) -> StorageResult<bool> {
        if slot.0 >= self.slot_count || self.live_count == 0 {
            return Ok(false);
        }
        if self.live_count == u64::from(self.slot_count) {
            return Ok(true);
        }
        self.reader.contains(slot)
    }

    pub(crate) fn filter_slot(
        &self,
        slot: Option<RecordSlot>,
    ) -> StorageResult<Option<RecordSlot>> {
        match slot {
            Some(slot) if self.contains(slot)? => Ok(Some(slot)),
            _ => Ok(None),
        }
    }

    pub(crate) fn any(&self, slots: &[RecordSlot]) -> StorageResult<bool> {
        for &slot in slots {
            if self.contains(slot)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(crate) fn count(&self, slots: &[RecordSlot]) -> StorageResult<u64> {
        if self.live_count == u64::from(self.slot_count) && self.live_count != 0 {
            return Ok(slots.iter().filter(|slot| slot.0 < self.slot_count).count() as u64);
        }
        slots.iter().try_fold(
            0,
            |count, &slot| Ok(count + u64::from(self.contains(slot)?)),
        )
    }

    pub(crate) fn visit_blocks(&self, visit: &mut VisitLiveBlock<'_>) -> StorageResult<()> {
        if self.live_count == 0 {
            return Ok(());
        }
        if self.live_count == u64::from(self.slot_count) {
            let mut first = 0;
            while first < self.slot_count {
                let count = (self.slot_count - first).min(256);
                const SLOTS: [u32; 256] = {
                    let mut slots = [0; 256];
                    let mut i = 0;
                    while i < slots.len() {
                        slots[i] = i as u32;
                        i += 1;
                    }
                    slots
                };
                visit(first, MembershipView::Sorted(&SLOTS[..count as usize]))?;
                first += count;
            }
            return Ok(());
        }
        if let Some(bitmap) = self.reader.borrowed_bitmap() {
            return visit(0, MembershipView::Borrowed(bitmap));
        }
        let mut previous = None;
        let mut count = 0u64;
        self.reader.visit_blocks(&mut |base, members| {
            if let MembershipView::Sorted(slots) = members
                && slots.windows(2).any(|pair| pair[0] >= pair[1])
            {
                return Err(StorageError::invalid(
                    "read live slot blocks",
                    "block slots are not strictly increasing",
                ));
            }
            if let Some(last) = members.last() {
                let first = members
                    .iter()
                    .next()
                    .and_then(|slot| base.checked_add(slot));
                let last = base.checked_add(last);
                if first.is_none_or(|first| previous.is_some_and(|prev| first <= prev))
                    || last.is_none_or(|last| last >= self.slot_count)
                {
                    return Err(StorageError::invalid(
                        "read live slot blocks",
                        "invalid block domain or order",
                    ));
                }
                count += members.len() as u64;
                if count > self.live_count {
                    return Err(StorageError::invalid(
                        "read live slot blocks",
                        "live count exceeded",
                    ));
                }
                previous = last;
                visit(base, members)?;
            }
            Ok(())
        })?;
        if count != self.live_count {
            return Err(StorageError::invalid(
                "read live slot blocks",
                "incomplete live count",
            ));
        }
        Ok(())
    }

    pub(crate) fn borrowed_bitmap(&self) -> Option<&RoaringBitmap> {
        self.reader.borrowed_bitmap()
    }

    pub(crate) fn includes(&self, other: &RoaringBitmap) -> StorageResult<bool> {
        if other.is_empty() {
            return Ok(true);
        }
        if other.max().is_some_and(|slot| slot >= self.slot_count) || other.len() > self.live_count
        {
            return Ok(false);
        }
        if self.live_count == u64::from(self.slot_count) {
            return Ok(true);
        }
        if let Some(bitmap) = self.borrowed_bitmap() {
            return Ok(other.is_subset(bitmap));
        }
        if other.len() <= 32 {
            for slot in other.iter() {
                if !self.contains(RecordSlot(slot))? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        Ok(other.len() == self.intersection_len(MembershipView::Borrowed(other))?)
    }

    /// Intersect a selected set without opening liveness pages unrelated to a
    /// small candidate set. Native resident bitmaps use one set operation.
    pub(crate) fn intersect_bitmap(&self, other: &RoaringBitmap) -> StorageResult<RoaringBitmap> {
        if self.live_count == 0 || other.is_empty() {
            return Ok(RoaringBitmap::new());
        }
        if self.live_count == u64::from(self.slot_count) {
            let mut selected = other.clone();
            selected.remove_range(self.slot_count..);
            return Ok(selected);
        }
        if let Some(bitmap) = self.reader.borrowed_bitmap() {
            return Ok(bitmap & other);
        }
        if other.len() <= 32 {
            let mut selected = RoaringBitmap::new();
            for slot in other.iter() {
                if self.contains(RecordSlot(slot))? {
                    selected.insert(slot);
                }
            }
            return Ok(selected);
        }
        let mut selected = RoaringBitmap::new();
        self.visit_blocks(&mut |base, members| {
            if base == 0 {
                selected |= members.intersect_bitmap(other);
            } else {
                selected.extend(
                    members
                        .iter()
                        .map(|slot| base + slot)
                        .filter(|slot| other.contains(*slot)),
                );
            }
            Ok(())
        })?;
        Ok(selected)
    }

    pub(crate) fn intersection_len(&self, other: MembershipView<'_>) -> StorageResult<u64> {
        if self.live_count == 0 || other.is_empty() {
            return Ok(0);
        }
        if self.live_count == u64::from(self.slot_count) {
            return Ok(
                if other.last().is_some_and(|last| last >= self.slot_count) {
                    other
                        .iter()
                        .take_while(|&slot| slot < self.slot_count)
                        .count() as u64
                } else {
                    other.len() as u64
                },
            );
        }
        if let Some(bitmap) = self.reader.borrowed_bitmap() {
            return Ok(MembershipView::Borrowed(bitmap).intersection_len(other));
        }
        let mut count = 0;
        self.visit_blocks(&mut |base, slots| {
            count += if base == 0 {
                slots.intersection_len(other)
            } else {
                slots
                    .iter()
                    .filter(|slot| other.contains(base + slot))
                    .count() as u64
            };
            Ok(())
        })?;
        Ok(count)
    }

    pub(crate) fn retired(&self, slots: &[RecordSlot]) -> StorageResult<Self> {
        if slots.is_empty() {
            return Ok(self.clone());
        }
        self.reader.retired(slots)
    }

    /// Materialize a query-owned bitmap for set algebra.
    pub(crate) fn bitmap(&self) -> StorageResult<RoaringBitmap> {
        if let Some(bitmap) = self.reader.borrowed_bitmap() {
            return Ok(bitmap.clone());
        }
        let mut output = RoaringBitmap::new();
        if self.live_count == u64::from(self.slot_count) {
            output.insert_range(0..self.slot_count);
            return Ok(output);
        }
        self.visit_blocks(&mut |base, slots| {
            output.extend(slots.iter().map(|slot| base + slot));
            Ok(())
        })?;
        Ok(output)
    }
}

pub(crate) enum InitialLiveness<'a> {
    All,
    Selected(MembershipView<'a>),
}

#[derive(Debug)]
struct Selection {
    count: u32,
    members: RoaringBitmap,
}

impl LiveSlotReader for Selection {
    fn slot_count(&self) -> u32 {
        self.count
    }

    fn live_count(&self) -> u64 {
        self.members.len()
    }

    fn contains(&self, slot: RecordSlot) -> StorageResult<bool> {
        Ok(self.members.contains(slot.0))
    }

    fn visit_blocks(&self, visit: &mut VisitLiveBlock<'_>) -> StorageResult<()> {
        visit(0, MembershipView::Borrowed(&self.members))
    }

    fn borrowed_bitmap(&self) -> Option<&RoaringBitmap> {
        Some(&self.members)
    }

    fn retired(&self, slots: &[RecordSlot]) -> StorageResult<ActiveRecords> {
        let mut members = self.members.clone();
        for slot in slots {
            if slot.0 >= self.count {
                return Err(StorageError::invalid(
                    "retire selected slots",
                    "slot out of bounds",
                ));
            }
            members.remove(slot.0);
        }
        Ok(ActiveRecords::new(Selection {
            count: self.count,
            members,
        }))
    }
}
