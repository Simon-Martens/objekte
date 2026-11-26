//! Sealed membership of one local record-slot domain.

use std::fmt::Debug;
use std::sync::Arc;

use roaring::RoaringBitmap;

use crate::model::RecordSlot;
use crate::query_access::membership::MembershipView;

use super::transfer::{StorageError, StorageResult};

/// Already pinned membership. Implementations expose their bounded in-memory
/// blocks without consulting a mutable publication or opening source data.
pub(crate) trait LiveSlotReader: Debug + Send + Sync {
    fn slot_count(&self) -> u32;
    fn live_count(&self) -> u64;
    fn contains(&self, slot: RecordSlot) -> bool;
    fn visit_blocks(&self, visit: &mut dyn FnMut(u32, MembershipView<'_>));
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

    pub(crate) fn contains(&self, slot: RecordSlot) -> bool {
        slot.0 < self.slot_count
            && (self.live_count == u64::from(self.slot_count) || self.reader.contains(slot))
    }

    /// Count a set of already validated local members without materializing
    /// a bitmap of the pinned live domain.
    pub(crate) fn intersection_len(&self, other: MembershipView<'_>) -> u64 {
        if self.live_count == 0 || other.is_empty() {
            return 0;
        }
        if self.live_count == u64::from(self.slot_count) {
            return other.len() as u64;
        }
        let mut count = 0;
        self.reader.visit_blocks(&mut |base, slots| {
            count += if base == 0 {
                slots.intersection_len(other)
            } else {
                slots
                    .iter()
                    .filter(|slot| other.contains(base + slot))
                    .count() as u64
            };
        });
        count
    }

    pub(crate) fn retired(&self, slots: &[RecordSlot]) -> StorageResult<Self> {
        if slots.is_empty() {
            return Ok(self.clone());
        }
        self.reader.retired(slots)
    }

    /// Materialize a query-owned bitmap for set algebra.
    pub(crate) fn bitmap(&self) -> RoaringBitmap {
        let mut output = RoaringBitmap::new();
        if self.live_count == u64::from(self.slot_count) {
            output.insert_range(0..self.slot_count);
            return output;
        }
        self.reader.visit_blocks(&mut |base, slots| {
            output.extend(slots.iter().map(|slot| base + slot));
        });
        output
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

    fn contains(&self, slot: RecordSlot) -> bool {
        self.members.contains(slot.0)
    }

    fn visit_blocks(&self, visit: &mut dyn FnMut(u32, MembershipView<'_>)) {
        visit(0, MembershipView::Borrowed(&self.members));
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
