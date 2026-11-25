//! Segment identity, record addresses, and sizing policy.

use std::{borrow::Cow, fmt, num::NonZeroUsize};

use serde::{Deserialize, Serialize};

use crate::model::{CollectionId, FieldPath, FileId, RecordSlot};
use crate::query::options::SortField;
use crate::records::RecordHeader;

use super::order::SortKey;
use super::ordered_slots::OrderedSlots;
use super::transfer::{BatchLimit, StorageResult};

/// Deterministic soft target used by every ordinary segment-producing path.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SegmentPolicy {
    pub target_records: NonZeroUsize,
    pub max_files: NonZeroUsize,
}

impl Default for SegmentPolicy {
    fn default() -> Self {
        Self {
            target_records: NonZeroUsize::new(16_384).expect("nonzero default"),
            max_files: NonZeroUsize::new(16_384).expect("nonzero default"),
        }
    }
}

impl SegmentPolicy {
    pub(crate) fn validate(self) -> Result<Self, String> {
        let local_limit = u32::MAX as usize;
        if self.target_records.get() > local_limit {
            return Err("segment target exceeds the supported local-slot range".into());
        }
        if self.max_files.get() > local_limit {
            return Err("segment file limit exceeds the supported local-slot range".into());
        }
        Ok(self)
    }

    pub fn with_target_records(target_records: NonZeroUsize) -> Result<Self, String> {
        Self {
            target_records,
            max_files: target_records,
        }
        .validate()
    }

    /// Apply the soft boundary only between completed contributions. An empty
    /// draft always accepts one oversized indivisible contribution; the next
    /// contribution starts a fresh segment.
    pub(crate) fn can_grow(
        self,
        current_records: usize,
        current_files: usize,
        contribution_records: usize,
        contribution_new_files: usize,
    ) -> bool {
        if current_records == 0 && current_files == 0 {
            return true;
        }
        current_records
            .checked_add(contribution_records)
            .is_some_and(|records| records <= self.target_records.get())
            && current_files
                .checked_add(contribution_new_files)
                .is_some_and(|files| files <= self.max_files.get())
    }
}

/// Stable identity for one immutable segment.
///
/// Segment-local record slots are meaningful only together with this ID and
/// the owning collection.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct SegmentId(u64);

impl SegmentId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for SegmentId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Address that remains unambiguous after crossing an immutable segment
/// boundary. Record slots are meaningful only with the collection and segment
/// components of this value.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct SegmentRecordAddress {
    pub collection: CollectionId,
    pub segment: SegmentId,
    pub slot: RecordSlot,
}

/// Immutable logical segment metadata, captured at seal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SegmentDescriptor {
    pub(crate) id: SegmentId,
    pub(crate) collection: CollectionId,
    pub(crate) slots: usize,
}

/// Reads from a pinned segment. Returned keys and headers can borrow the
/// pinned representation or hold an owned value decoded by the reader.
pub(crate) trait SegmentReader: Send + Sync {
    fn descriptor(&self) -> SegmentDescriptor;

    fn slot_for_record(&self, id: &str) -> StorageResult<Option<RecordSlot>>;

    fn header(&self, slot: RecordSlot) -> StorageResult<Option<Cow<'_, RecordHeader>>>;

    fn visit_file_slots(
        &self,
        file: FileId,
        limit: BatchLimit,
        visit: &mut dyn FnMut(&[RecordSlot]) -> StorageResult<()>,
    ) -> StorageResult<()>;

    fn stored_key(
        &self,
        path: &FieldPath,
        slot: RecordSlot,
    ) -> StorageResult<Option<Cow<'_, SortKey>>>;

    fn ordered_slots(
        &self,
        sort: &[SortField],
        scalar: Option<(&FieldPath, bool)>,
    ) -> StorageResult<Option<&dyn OrderedSlots>>;
}
