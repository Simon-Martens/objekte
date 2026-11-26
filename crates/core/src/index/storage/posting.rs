//! Typed posting keys and terminal selection facts.

use std::num::NonZeroUsize;

use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};

use crate::model::{LogicalType, Number, RecordSlot, Value};

use super::transfer::{BatchLimit, StorageError, StorageResult};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub enum ScalarKey {
    String(String),
    Id(String),
    Number(Number),
    Bool(bool),
    Date(Date),
    Datetime(OffsetDateTime),
    Unixtime(i64),
    /// Explicit null is a terminal selection fact. It is inserted by
    /// `PostingIndex::add_values`, never by the ordinary scalar conversion.
    Null,
}

impl ScalarKey {
    /// Convert ordinary scalar values. Explicit null is deliberately omitted:
    /// null has terminal-occurrence semantics and must not be introduced while
    /// flattening array elements or evaluating unrelated scalar lookups.
    pub fn from_value(value: &Value) -> Option<Self> {
        Some(match value {
            Value::String(s) => Self::String(s.clone()),
            Value::Id(s) => Self::Id(s.clone()),
            Value::Number(n) => Self::Number(n.clone()),
            Value::Bool(b) => Self::Bool(*b),
            Value::Date(d) => Self::Date(*d),
            Value::Datetime(t) => Self::Datetime(*t),
            Value::Unixtime(t) => Self::Unixtime(*t),
            _ => return None,
        })
    }
}

const SUMMARY_NULL: u16 = 1 << 0;
const SUMMARY_STRING: u16 = 1 << 1;
const SUMMARY_ID: u16 = 1 << 2;
const SUMMARY_NUMBER: u16 = 1 << 3;
const SUMMARY_BOOL: u16 = 1 << 4;
const SUMMARY_DATE: u16 = 1 << 5;
const SUMMARY_DATETIME: u16 = 1 << 6;
const SUMMARY_UNIXTIME: u16 = 1 << 7;
const SUMMARY_OBJECT: u16 = 1 << 8;
const SUMMARY_ARRAY: u16 = 1 << 9;
const SUMMARY_STRING_OR_ID: u16 = SUMMARY_STRING | SUMMARY_ID;

/// Complete terminal-selection observation for one path and segment.
///
/// An empty kind mask means that no logical record selected a terminal value.
/// The multiplicity flag records whether any record selected more than one
/// terminal occurrence. This summary is an upper bound after projection and
/// is only published when construction observed the complete logical domain.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SelectionSummary {
    kinds: u16,
    // Here we mark if there wer objects where the path matched multiple keys in
    // the same object.
    multiple: bool,
    empty_traversal: bool,
}

impl SelectionSummary {
    pub(crate) fn observe(&mut self, value: &Value) {
        self.kinds |= match value {
            Value::Null => SUMMARY_NULL,
            Value::String(_) => SUMMARY_STRING,
            Value::Id(_) => SUMMARY_ID,
            Value::Number(_) => SUMMARY_NUMBER,
            Value::Bool(_) => SUMMARY_BOOL,
            Value::Date(_) => SUMMARY_DATE,
            Value::Datetime(_) => SUMMARY_DATETIME,
            Value::Unixtime(_) => SUMMARY_UNIXTIME,
            Value::Object(_) => SUMMARY_OBJECT,
            Value::Array(_) => SUMMARY_ARRAY,
        };
    }

    pub(crate) fn merge_shifted(&mut self, other: Self) {
        self.kinds |= other.kinds;
        self.multiple |= other.multiple;
        self.empty_traversal |= other.empty_traversal;
    }

    pub(crate) fn has_selected_values(self) -> bool {
        self.kinds != 0
    }

    pub(crate) fn may_match_string(self) -> bool {
        self.kinds & SUMMARY_STRING_OR_ID != 0
    }

    pub(crate) fn may_match_number(self) -> bool {
        self.kinds & SUMMARY_NUMBER != 0
    }

    pub(crate) fn may_match_array(self) -> bool {
        self.kinds & SUMMARY_ARRAY != 0
    }

    pub(crate) fn may_match_object(self) -> bool {
        self.kinds & SUMMARY_OBJECT != 0
    }

    pub(crate) fn only_matches_string(self) -> bool {
        self.kinds != 0 && self.kinds & !SUMMARY_STRING_OR_ID == 0
    }

    pub(crate) fn only_matches_number(self) -> bool {
        self.kinds != 0 && self.kinds & !SUMMARY_NUMBER == 0
    }

    pub(crate) fn only_matches_number_or_null(self) -> bool {
        self.kinds != 0 && self.kinds & !(SUMMARY_NUMBER | SUMMARY_NULL) == 0
    }

    pub(crate) fn only_matches_array(self) -> bool {
        self.kinds != 0 && self.kinds & !SUMMARY_ARRAY == 0
    }

    pub(crate) fn only_matches_object(self) -> bool {
        self.kinds != 0 && self.kinds & !SUMMARY_OBJECT == 0
    }

    pub(crate) fn only_matches_object_or_null(self) -> bool {
        self.kinds != 0 && self.kinds & !(SUMMARY_OBJECT | SUMMARY_NULL) == 0
    }

    pub(crate) fn only_matches_scalar_or_null(self) -> bool {
        let scalar = SUMMARY_STRING_OR_ID
            | SUMMARY_NUMBER
            | SUMMARY_BOOL
            | SUMMARY_DATE
            | SUMMARY_DATETIME
            | SUMMARY_UNIXTIME;
        self.kinds != 0 && self.kinds & !(scalar | SUMMARY_NULL) == 0
    }

    pub(crate) fn only_matches_nonempty_scalar(self) -> bool {
        let guaranteed =
            SUMMARY_NUMBER | SUMMARY_BOOL | SUMMARY_DATE | SUMMARY_DATETIME | SUMMARY_UNIXTIME;
        self.kinds != 0 && self.kinds & !guaranteed == 0
    }

    pub(crate) fn only_matches_nonempty_scalar_or_null(self) -> bool {
        let guaranteed = SUMMARY_NULL
            | SUMMARY_NUMBER
            | SUMMARY_BOOL
            | SUMMARY_DATE
            | SUMMARY_DATETIME
            | SUMMARY_UNIXTIME;
        self.kinds != 0 && self.kinds & !guaranteed == 0
    }

    pub(crate) fn only_matches_string_or_null(self) -> bool {
        self.kinds != 0 && self.kinds & !(SUMMARY_STRING_OR_ID | SUMMARY_NULL) == 0
    }

    pub(crate) fn only_matches_array_or_null(self) -> bool {
        self.kinds != 0 && self.kinds & !(SUMMARY_ARRAY | SUMMARY_NULL) == 0
    }

    pub(crate) fn may_select_multiple(self) -> bool {
        self.multiple
    }

    pub(crate) fn note_multiple(&mut self) {
        self.multiple = true;
    }

    pub(crate) fn note_empty_traversal(&mut self) {
        self.empty_traversal = true;
    }

    pub(crate) fn may_have_empty_traversal(self) -> bool {
        self.empty_traversal
    }
}

/// Captured semantics of a declared, inferred, or named dictionary.
pub(crate) struct PostingDescriptor<'a> {
    pub path: &'a [String],
    pub kind: &'a LogicalType,
    pub coverage: PostingCoverage,
}

#[derive(Clone, Copy)]
pub(crate) struct PostingCoverage {
    pub multi: bool,
    pub values_indexed: bool,
    pub value_complete: bool,
    pub length_complete: bool,
    pub empty_string_complete: bool,
    pub covered_records: u32,
    pub summary: Option<SelectionSummary>,
    pub legacy_present_equivalent: bool,
    pub legacy_null_equivalent: bool,
}

/// Terminal presence, explicit null, values and array length use distinct
/// logical labels inside the local slot domain.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum PostingKey<'a> {
    Presence,
    Value(&'a ScalarKey),
    Length(&'a Number),
}

impl PostingKey<'_> {
    pub(crate) fn bytes(self) -> usize {
        let payload = match self {
            Self::Value(ScalarKey::String(value) | ScalarKey::Id(value)) => value.len(),
            Self::Value(ScalarKey::Number(Number::ExactDecimal(value)))
            | Self::Length(Number::ExactDecimal(value)) => value.digits.len(),
            Self::Presence | Self::Value(_) | Self::Length(_) => 0,
        };
        size_of::<ScalarKey>().saturating_add(payload)
    }
}

pub(crate) type VisitPostingBlock<'a> =
    dyn FnMut(PostingKey<'_>, &[RecordSlot]) -> StorageResult<()> + 'a;

pub(crate) trait PostingReader: std::any::Any + Send + Sync {
    fn descriptor(&self) -> PostingDescriptor<'_>;

    /// A frequent key spans consecutive blocks. Within a key, slots are
    /// strictly increasing. Presence is emitted even for a zero-row domain.
    fn visit_blocks(
        &self,
        limit: BatchLimit,
        visit: &mut VisitPostingBlock<'_>,
    ) -> StorageResult<()>;
}

pub(crate) const POSTING_BLOCK_LIMIT: BatchLimit = BatchLimit::new(
    NonZeroUsize::new(256).unwrap(),
    NonZeroUsize::new(16 * 1024 * 1024).unwrap(),
);

pub(crate) fn visit_posting_blocks(
    reader: &dyn PostingReader,
    slot_count: usize,
    cancelled: &dyn Fn() -> bool,
    visit: &mut VisitPostingBlock<'_>,
) -> StorageResult<()> {
    const OP: &str = "read posting blocks";
    if cancelled() {
        return Err(StorageError::Cancelled);
    }
    if reader.descriptor().coverage.covered_records as usize > slot_count {
        return Err(StorageError::invalid(
            OP,
            "coverage exceeds the slot domain",
        ));
    }
    let mut latest = None;
    let mut last_slot: Option<RecordSlot> = None;
    reader.visit_blocks(POSTING_BLOCK_LIMIT, &mut |key, slots| {
        if cancelled() {
            return Err(StorageError::Cancelled);
        }
        let key_order = latest
            .as_ref()
            .map(|previous: &OwnedPostingKey| previous.key().cmp(&key));
        if key_order.is_none() && key != PostingKey::Presence
            || key_order == Some(std::cmp::Ordering::Greater)
            || key_order == Some(std::cmp::Ordering::Equal)
                && (last_slot.is_none() || slots.is_empty())
            || slots.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(StorageError::invalid(
                OP,
                "invalid posting key or slot order",
            ));
        }
        if key_order == Some(std::cmp::Ordering::Equal) {
            if let (Some(last), Some(first)) = (last_slot, slots.first())
                && last >= *first
            {
                return Err(StorageError::invalid(OP, "posting blocks overlap"));
            }
        } else {
            latest = Some(OwnedPostingKey::from(key));
        }
        if slots
            .last()
            .is_some_and(|slot| slot.0 as usize >= slot_count)
        {
            return Err(StorageError::invalid(
                OP,
                "posting slot outside the segment",
            ));
        }
        if slots.is_empty() {
            POSTING_BLOCK_LIMIT.accepts(OP, 0, 0, key.bytes())?;
        } else if slots.len()
            > POSTING_BLOCK_LIMIT.fixed_batch_len_after(OP, key.bytes(), size_of::<RecordSlot>())?
        {
            return Err(StorageError::invalid(
                OP,
                "posting block exceeds its limits",
            ));
        }
        last_slot = slots.last().copied();
        visit(key, slots)
    })?;
    if latest.is_none() {
        return Err(StorageError::invalid(OP, "missing presence block"));
    }
    Ok(())
}

enum OwnedPostingKey {
    Presence,
    Value(ScalarKey),
    Length(Number),
}

impl OwnedPostingKey {
    fn key(&self) -> PostingKey<'_> {
        match self {
            Self::Presence => PostingKey::Presence,
            Self::Value(value) => PostingKey::Value(value),
            Self::Length(number) => PostingKey::Length(number),
        }
    }
}

impl From<PostingKey<'_>> for OwnedPostingKey {
    fn from(value: PostingKey<'_>) -> Self {
        match value {
            PostingKey::Presence => Self::Presence,
            PostingKey::Value(value) => Self::Value(value.clone()),
            PostingKey::Length(value) => Self::Length(value.clone()),
        }
    }
}
