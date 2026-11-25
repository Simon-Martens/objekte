//! Typed posting keys and terminal selection facts.

use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};

use crate::model::{Number, Value};

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
