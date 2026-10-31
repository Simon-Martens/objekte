//! Observed JSON paths and actual value shapes in the indexed file snapshot.

use super::TypedKey;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

const MAX_STRING_VALUES: usize = 64;
const MAX_STRING_KEY_BYTES: usize = 8 * 1024;
const MAX_POSTING_ROWS: usize = 500_000;


/// One key-only path reached while parsing a source record.
pub(super) type FieldPath = Vec<String>;


/// Actual JSON value kinds at an observed path.
#[derive(Clone, Copy)]
pub(super) enum ObservedKind {
    Null,
    Boolean,
    Number,
    String,
    Array,
    Object,
}


impl ObservedKind {
    pub(super) fn bit(self) -> u8 {
        1 << (self as u8)
    }
}


/// All observations of one path in one parsed source file.
pub(super) struct FieldObservation {
    pub path: FieldPath,
    pub kinds: u8,
    pub occurrences: usize,
    pub scalar: Option<TypedKey>,
}


/// Indexed counts of actual JSON shapes at one key path.
#[derive(Clone, Debug, Serialize)]
pub struct FieldProfile {
    pub path: Vec<String>,
    pub present: usize,
    /// Parseable records in which this path is absent; JSON has no undefined value.
    pub missing: usize,
    pub nulls: usize,
    pub booleans: usize,
    pub numbers: usize,
    pub strings: usize,
    pub arrays: usize,
    pub objects: usize,
    /// More than one JSON value kind occurs at this path across the collection.
    pub mixed_types: bool,
    /// Records with different kinds reached at the same path.
    pub mixed_records: usize,
    /// Records with more than one value reached at this path.
    pub multiple_values: usize,
}


/// Per-file bitsets for each actual kind and path cardinality.
struct FieldBits {
    present: Vec<u64>,
    nulls: Vec<u64>,
    booleans: Vec<u64>,
    numbers: Vec<u64>,
    strings: Vec<u64>,
    arrays: Vec<u64>,
    objects: Vec<u64>,
    mixed: Vec<u64>,
    multiple: Vec<u64>,
}


/// Bounded provisional postings for one inferred scalar path.
#[derive(Default)]
struct InferredCandidate {
    rows: Vec<(TypedKey, usize)>,
    strings: BTreeSet<String>,
    string_bytes: usize,
    rejected: bool,
}


impl InferredCandidate {
    fn observe(&mut self, slot: usize, observation: &FieldObservation) {
        if self.rejected {
            return;
        }
        if observation.occurrences != 1 || observation.kinds & ObservedKind::Array.bit() != 0 {
            self.reject();
            return;
        }
        let Some(key) = observation.scalar.as_ref() else {
            return;
        };
        if let TypedKey::String(value) = key
            && self.strings.insert(value.clone())
        {
            self.string_bytes += value.len();
            if self.strings.len() > MAX_STRING_VALUES || self.string_bytes > MAX_STRING_KEY_BYTES {
                self.reject();
                return;
            }
        }
        if self.rows.len() == MAX_POSTING_ROWS {
            self.reject();
            return;
        }
        self.rows.push((key.clone(), slot));
    }

    fn reject(&mut self) {
        self.rejected = true;
        self.rows = Vec::new();
        self.strings = BTreeSet::new();
        self.string_bytes = 0;
    }
}


impl FieldBits {
    fn new(words: usize) -> Self {
        Self {
            present: vec![0; words],
            nulls: vec![0; words],
            booleans: vec![0; words],
            numbers: vec![0; words],
            strings: vec![0; words],
            arrays: vec![0; words],
            objects: vec![0; words],
            mixed: vec![0; words],
            multiple: vec![0; words],
        }
    }

    fn clear(&mut self, word: usize, bit: u64) {
        for bits in [
            &mut self.present,
            &mut self.nulls,
            &mut self.booleans,
            &mut self.numbers,
            &mut self.strings,
            &mut self.arrays,
            &mut self.objects,
            &mut self.mixed,
            &mut self.multiple,
        ] {
            if let Some(value) = bits.get_mut(word) {
                *value &= !bit;
            }
        }
    }

    fn add(&mut self, word: usize, bit: u64, observation: &FieldObservation) {
        self.present[word] |= bit;
        for (mask, bits) in [
            (ObservedKind::Null.bit(), &mut self.nulls),
            (ObservedKind::Boolean.bit(), &mut self.booleans),
            (ObservedKind::Number.bit(), &mut self.numbers),
            (ObservedKind::String.bit(), &mut self.strings),
            (ObservedKind::Array.bit(), &mut self.arrays),
            (ObservedKind::Object.bit(), &mut self.objects),
        ] {
            if observation.kinds & mask != 0 {
                bits[word] |= bit;
            }
        }
        if observation.kinds.count_ones() > 1 {
            self.mixed[word] |= bit;
        }
        if observation.occurrences > 1 {
            self.multiple[word] |= bit;
        }
    }
}


/// Path shape index for all observed keys, including nested keys through arrays.
pub(super) struct ObservedFields {
    words: usize,
    parsed: Vec<u64>,
    empty: Vec<u64>,
    fields: BTreeMap<FieldPath, FieldBits>,
    infer_field_types: bool,
    configured: BTreeSet<FieldPath>,
    candidates: BTreeMap<FieldPath, InferredCandidate>,
}


impl ObservedFields {
    pub(super) fn new(slots: usize, infer_field_types: bool, configured: &[String]) -> Self {
        let words = slots.div_ceil(64);
        Self {
            words,
            parsed: vec![0; words],
            empty: vec![0; words],
            fields: BTreeMap::new(),
            infer_field_types,
            configured: configured.iter().map(|field| vec![field.clone()]).collect(),
            candidates: BTreeMap::new(),
        }
    }

    pub(super) fn add(&mut self, slot: usize, observations: &[FieldObservation]) {
        self.add_shapes(slot, observations);
        if self.infer_field_types {
            for observation in observations {
                if !self.configured.contains(&observation.path) {
                    self.candidates
                        .entry(observation.path.clone())
                        .or_default()
                        .observe(slot, observation);
                }
            }
        }
    }

    fn add_shapes(&mut self, slot: usize, observations: &[FieldObservation]) {
        let word = slot / 64;
        let bit = 1u64 << (slot % 64);
        self.grow(word + 1);
        self.parsed[word] |= bit;
        for observation in observations {
            self.fields
                .entry(observation.path.clone())
                .or_insert_with(|| FieldBits::new(self.words))
                .add(word, bit, observation);
        }
    }

    pub(super) fn update(&mut self, slot: usize, observations: Option<&[FieldObservation]>) {
        let word = slot / 64;
        let bit = 1u64 << (slot % 64);
        if let Some(value) = self.parsed.get_mut(word) {
            *value &= !bit;
        }
        for field in self.fields.values_mut() {
            field.clear(word, bit);
        }
        if let Some(observations) = observations {
            self.add_shapes(slot, observations);
        }
    }

    pub(super) fn take_inferred(&mut self) -> BTreeMap<FieldPath, Vec<(TypedKey, usize)>> {
        std::mem::take(&mut self.candidates)
            .into_iter()
            .filter_map(|(path, candidate)| {
                (!candidate.rejected && !candidate.rows.is_empty())
                    .then_some((path, candidate.rows))
            })
            .collect()
    }

    pub(super) fn presence_bits(&self, path: &[String]) -> &[u64] {
        self.fields
            .get(path)
            .map_or(&self.empty, |field| &field.present)
    }

    pub(super) fn null_bits(&self, path: &[String]) -> &[u64] {
        self.fields
            .get(path)
            .map_or(&self.empty, |field| &field.nulls)
    }

    pub(super) fn non_null_scalar_bits(&self, path: &[String]) -> Vec<u64> {
        let Some(field) = self.fields.get(path) else {
            return self.empty.clone();
        };
        field
            .booleans
            .iter()
            .zip(&field.numbers)
            .zip(&field.strings)
            .map(|((&boolean, &number), &string)| boolean | number | string)
            .collect()
    }

    pub(super) fn missing_bits(&self, path: &[String]) -> Vec<u64> {
        self.parsed
            .iter()
            .zip(self.presence_bits(path))
            .map(|(&parsed, &present)| parsed & !present)
            .collect()
    }

    pub(super) fn profiles(&self) -> Vec<FieldProfile> {
        fn count(bits: &[u64]) -> usize {
            bits.iter().map(|word| word.count_ones() as usize).sum()
        }
        self.fields
            .iter()
            .map(|(path, field)| {
                let type_counts = [
                    count(&field.nulls),
                    count(&field.booleans),
                    count(&field.numbers),
                    count(&field.strings),
                    count(&field.arrays),
                    count(&field.objects),
                ];
                FieldProfile {
                    path: path.clone(),
                    present: count(&field.present),
                    missing: count(&self.missing_bits(path)),
                    nulls: type_counts[0],
                    booleans: type_counts[1],
                    numbers: type_counts[2],
                    strings: type_counts[3],
                    arrays: type_counts[4],
                    objects: type_counts[5],
                    mixed_types: type_counts.iter().filter(|&&count| count != 0).count() > 1,
                    mixed_records: count(&field.mixed),
                    multiple_values: count(&field.multiple),
                }
            })
            .collect()
    }

    pub(super) fn has_array(&self, path: &[String]) -> bool {
        self.fields
            .get(path)
            .is_some_and(|field| field.arrays.iter().any(|word| *word != 0))
    }

    pub(super) fn has_multiple(&self, path: &[String]) -> bool {
        self.fields
            .get(path)
            .is_some_and(|field| field.multiple.iter().any(|word| *word != 0))
    }

    pub(super) fn candidates(&self, fields: &[String]) -> Option<Vec<u64>> {
        let (first, rest) = fields.split_first()?;
        let mut bits = self.presence_bits(std::slice::from_ref(first)).to_vec();
        for field in rest {
            for (word, next) in bits
                .iter_mut()
                .zip(self.presence_bits(std::slice::from_ref(field)))
            {
                *word &= next;
            }
        }
        Some(bits)
    }

    fn grow(&mut self, words: usize) {
        if self.words >= words {
            return;
        }
        self.words = words;
        self.parsed.resize(words, 0);
        self.empty.resize(words, 0);
        for field in self.fields.values_mut() {
            for bits in [
                &mut field.present,
                &mut field.nulls,
                &mut field.booleans,
                &mut field.numbers,
                &mut field.strings,
                &mut field.arrays,
                &mut field.objects,
                &mut field.mixed,
                &mut field.multiple,
            ] {
                bits.resize(words, 0);
            }
        }
    }
}


#[cfg(test)]
#[path = "observed.test.rs"]
mod tests;
