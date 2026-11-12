//! Exact primitive state for a selected logical field path.
//!
//! Coverage is independent of the stored masks: a covered slot with no mask
//! proves absence, while an uncovered slot requires source evaluation.

use std::collections::BTreeMap;
use std::mem::size_of;

use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};

use crate::model::{Number, Value};

pub(crate) type OccurrenceMask = u16;

const NULL: OccurrenceMask = 1 << 0;
const FALSE: OccurrenceMask = 1 << 1;
const TRUE: OccurrenceMask = 1 << 2;
const ZERO: OccurrenceMask = 1 << 3;
const OTHER_NUMBER: OccurrenceMask = 1 << 4;
const EMPTY_STRING: OccurrenceMask = 1 << 5;
const OTHER_STRING: OccurrenceMask = 1 << 6;
const EMPTY_ARRAY: OccurrenceMask = 1 << 7;
const OTHER_ARRAY: OccurrenceMask = 1 << 8;
const EMPTY_OBJECT: OccurrenceMask = 1 << 9;
const OTHER_OBJECT: OccurrenceMask = 1 << 10;
const ALL_CLASSES: OccurrenceMask = (1 << 11) - 1;
const NONEMPTY: OccurrenceMask =
    FALSE | TRUE | ZERO | OTHER_NUMBER | OTHER_STRING | OTHER_ARRAY | OTHER_OBJECT;
const MIXED_CODE: u8 = 12;
const BLOCK_SLOTS: usize = 256;
const PACKED_BYTES: usize = BLOCK_SLOTS / 2;
// A sparse mixed side table has four retained bytes per entry. Above this
// threshold a 512-byte dense mask block is smaller than packed + exceptions.
const DENSE_MIXED_THRESHOLD: usize = 96;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StatePredicate {
    NonEmpty,
    String,
    Number,
    Array,
    Null,
    Object,
    Exists,
}

/// Classify one logical occurrence. Temporal logical values have no code in
/// the eleven-class format, so their records must remain uncovered.
pub(crate) fn classify_value(value: &Value) -> Option<OccurrenceMask> {
    Some(match value {
        Value::Null => NULL,
        Value::Bool(false) => FALSE,
        Value::Bool(true) => TRUE,
        Value::Number(number) if number == &Number::integer(0) => ZERO,
        Value::Number(_) => OTHER_NUMBER,
        Value::String(text) | Value::Id(text) if text.is_empty() => EMPTY_STRING,
        Value::String(_) | Value::Id(_) => OTHER_STRING,
        Value::Array(array) if array.values.is_empty() => EMPTY_ARRAY,
        Value::Array(_) => OTHER_ARRAY,
        Value::Object(map) if map.is_empty() => EMPTY_OBJECT,
        Value::Object(_) => OTHER_OBJECT,
        Value::Date(_) | Value::Datetime(_) | Value::Unixtime(_) => return None,
    })
}

/// OR all selected occurrences without allocating. `Some(0)` is a covered
/// absence; `None` means at least one occurrence cannot be represented.
pub(crate) fn classify_selected(selected: Option<&[&Value]>) -> Option<OccurrenceMask> {
    let mut mask = 0;
    for value in selected.into_iter().flatten() {
        mask |= classify_value(value)?;
    }
    Some(mask)
}

pub(crate) fn predicate_matches(mask: OccurrenceMask, predicate: StatePredicate) -> bool {
    let desired = match predicate {
        StatePredicate::NonEmpty => NONEMPTY,
        StatePredicate::String => EMPTY_STRING | OTHER_STRING,
        StatePredicate::Number => ZERO | OTHER_NUMBER,
        StatePredicate::Array => EMPTY_ARRAY | OTHER_ARRAY,
        StatePredicate::Null => NULL,
        StatePredicate::Object => EMPTY_OBJECT | OTHER_OBJECT,
        StatePredicate::Exists => ALL_CLASSES,
    };
    mask & desired != 0
}

/// Source evaluator for all logical Value variants, including temporal ones
/// that cannot be retained in this compact column.
pub(crate) fn selected_matches(selected: Option<&[&Value]>, predicate: StatePredicate) -> bool {
    selected
        .into_iter()
        .flatten()
        .any(|value| value_matches(value, predicate))
}

pub(crate) fn value_matches(value: &Value, predicate: StatePredicate) -> bool {
    match predicate {
        StatePredicate::Exists => true,
        StatePredicate::NonEmpty => match value {
            Value::Null => false,
            Value::String(text) | Value::Id(text) => !text.is_empty(),
            Value::Array(array) => !array.values.is_empty(),
            Value::Object(map) => !map.is_empty(),
            _ => true,
        },
        StatePredicate::String => matches!(value, Value::String(_) | Value::Id(_)),
        StatePredicate::Number => matches!(value, Value::Number(_)),
        StatePredicate::Array => matches!(value, Value::Array(_)),
        StatePredicate::Null => matches!(value, Value::Null),
        StatePredicate::Object => matches!(value, Value::Object(_)),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum StateBlock {
    Packed {
        codes: Vec<u8>,
        exceptions: Vec<(u8, OccurrenceMask)>,
    },
    Dense(Vec<OccurrenceMask>),
}

impl StateBlock {
    fn mask_at(&self, offset: u8) -> OccurrenceMask {
        match self {
            Self::Dense(masks) => masks[offset as usize],
            Self::Packed { codes, exceptions } => {
                let byte = codes[offset as usize / 2];
                let code = if offset % 2 == 0 {
                    byte & 0x0f
                } else {
                    byte >> 4
                };
                if code == MIXED_CODE {
                    let index = exceptions
                        .binary_search_by_key(&offset, |(slot, _)| *slot)
                        .expect("mixed code must have an exception");
                    exceptions[index].1
                } else if code == 0 {
                    0
                } else {
                    1 << (code - 1)
                }
            }
        }
    }

    fn retained_heap_bytes(&self) -> usize {
        match self {
            Self::Dense(masks) => masks.capacity() * size_of::<OccurrenceMask>(),
            Self::Packed { codes, exceptions } => {
                codes.capacity() + exceptions.capacity() * size_of::<(u8, OccurrenceMask)>()
            }
        }
    }
}

/// An immutable, serializable column for one canonical selection domain.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct FieldStateColumn {
    covered: RoaringBitmap,
    blocks: BTreeMap<u32, StateBlock>,
}

impl FieldStateColumn {
    pub(crate) fn covered_slots(&self) -> &RoaringBitmap {
        &self.covered
    }

    pub(crate) fn mask_at(&self, slot: u32) -> Option<OccurrenceMask> {
        self.covered.contains(slot).then(|| {
            self.blocks
                .get(&(slot / BLOCK_SLOTS as u32))
                .map_or(0, |block| block.mask_at((slot % BLOCK_SLOTS as u32) as u8))
        })
    }

    pub(crate) fn matching_slots(
        &self,
        predicate: StatePredicate,
        expected: bool,
        candidates: &RoaringBitmap,
    ) -> RoaringBitmap {
        let covered = candidates & &self.covered;
        covered
            .iter()
            .filter(|&slot| {
                let mask = self.mask_at(slot).expect("intersection is covered");
                predicate_matches(mask, predicate) == expected
            })
            .collect()
    }

    /// Exact bincode length used by the existing snapshot codec.
    pub(crate) fn encoded_bytes(&self) -> usize {
        bincode::serialized_size(self).expect("field-state serialization size") as usize
    }

    /// Estimated live allocation, including serialized-size coverage and a
    /// conservative BTreeMap node allowance; allocator overhead may differ.
    pub(crate) fn estimated_retained_bytes(&self) -> usize {
        size_of::<Self>()
            + self.covered.serialized_size()
            + self
                .blocks
                .values()
                .map(|block| {
                    size_of::<u32>() + size_of::<StateBlock>() + 64 + block.retained_heap_bytes()
                })
                .sum::<usize>()
    }

    pub(crate) fn mixed_entries(&self) -> usize {
        self.blocks
            .values()
            .map(|block| match block {
                StateBlock::Packed { exceptions, .. } => exceptions.len(),
                StateBlock::Dense(masks) => {
                    masks.iter().filter(|mask| mask.count_ones() > 1).count()
                }
            })
            .sum()
    }
}

/// Mutable storage is bounded to one 256-slot mask array per touched block.
/// Entirely absent covered blocks allocate no state block.
#[derive(Default)]
pub(crate) struct FieldStateColumnBuilder {
    covered: RoaringBitmap,
    blocks: BTreeMap<u32, Box<[OccurrenceMask; BLOCK_SLOTS]>>,
}

impl FieldStateColumnBuilder {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn cover_slots(&mut self, slots: &RoaringBitmap) {
        self.covered |= slots;
    }

    pub(crate) fn insert_mask(&mut self, slot: u32, mask: OccurrenceMask) {
        assert!(
            mask != 0 && mask & !ALL_CLASSES == 0,
            "invalid occurrence mask"
        );
        self.covered.insert(slot);
        self.blocks
            .entry(slot / BLOCK_SLOTS as u32)
            .or_insert_with(|| Box::new([0; BLOCK_SLOTS]))[(slot % BLOCK_SLOTS as u32) as usize] |=
            mask;
    }

    pub(crate) fn finish(self) -> FieldStateColumn {
        let blocks = self
            .blocks
            .into_iter()
            .map(|(index, masks)| {
                let mixed = masks.iter().filter(|mask| mask.count_ones() > 1).count();
                let block = if mixed > DENSE_MIXED_THRESHOLD {
                    StateBlock::Dense(masks.to_vec())
                } else {
                    let mut codes = vec![0; PACKED_BYTES];
                    let mut exceptions = Vec::with_capacity(mixed);
                    for (offset, &mask) in masks.iter().enumerate() {
                        let code = if mask.count_ones() > 1 {
                            exceptions.push((offset as u8, mask));
                            MIXED_CODE
                        } else if mask == 0 {
                            0
                        } else {
                            mask.trailing_zeros() as u8 + 1
                        };
                        if offset % 2 == 0 {
                            codes[offset / 2] |= code;
                        } else {
                            codes[offset / 2] |= code << 4;
                        }
                    }
                    StateBlock::Packed { codes, exceptions }
                };
                (index, block)
            })
            .collect();
        FieldStateColumn {
            covered: self.covered,
            blocks,
        }
    }
}

#[cfg(test)]
#[path = "field_state.test.rs"]
mod tests;
