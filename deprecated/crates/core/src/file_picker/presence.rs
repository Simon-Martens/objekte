//! Top-level JSON field presence for schema-declared paths.

use crate::Field;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicU64, Ordering},
};


/// One bit per file slot for each declared top-level field.
#[derive(Serialize, Deserialize)]
pub(super) struct PresenceIndex {
    fields: Vec<String>,
    bits: Vec<Vec<u64>>,
}


impl PresenceIndex {
    pub(super) fn new(definitions: &BTreeMap<String, Field>) -> Self {
        Self {
            fields: definitions.keys().cloned().collect(),
            bits: Vec::new(),
        }
    }

    pub(super) fn fields(&self) -> &[String] {
        &self.fields
    }

    pub(super) fn valid_for_slots(&self, slots: usize) -> bool {
        self.bits.len() == self.fields.len()
            && self
                .bits
                .iter()
                .all(|bits| bits.len() == slots.div_ceil(64))
    }

    pub(super) fn build_words(&self, slots: usize) -> Vec<Vec<AtomicU64>> {
        (0..self.fields.len())
            .map(|_| (0..slots.div_ceil(64)).map(|_| AtomicU64::new(0)).collect())
            .collect()
    }

    pub(super) fn replace(&mut self, words: Vec<Vec<AtomicU64>>) {
        self.bits = words
            .into_iter()
            .map(|words| words.into_iter().map(|word| word.into_inner()).collect())
            .collect();
    }

    pub(super) fn add_build_row(words: &[Vec<AtomicU64>], slot: usize, fields: &[usize]) {
        for &field in fields {
            words[field][slot / 64].fetch_or(1 << (slot % 64), Ordering::Relaxed);
        }
    }

    pub(super) fn update(&mut self, slot: usize, fields: &[usize]) {
        let word = slot / 64;
        let bit = 1 << (slot % 64);
        for bits in &mut self.bits {
            if bits.len() <= word {
                bits.resize(word + 1, 0);
            }
            bits[word] &= !bit;
        }
        for &field in fields {
            self.bits[field][word] |= bit;
        }
    }

    pub(super) fn remove(&mut self, slot: usize) {
        let word = slot / 64;
        for bits in &mut self.bits {
            if let Some(bits) = bits.get_mut(word) {
                *bits &= !(1 << (slot % 64));
            }
        }
    }

    pub(super) fn bits(&self, field: &str) -> Option<&[u64]> {
        let index = self
            .fields
            .binary_search_by(|name| name.as_str().cmp(field))
            .ok()?;
        Some(&self.bits[index])
    }

    pub(super) fn candidates(&self, fields: &[String]) -> Option<Vec<u64>> {
        let (first, rest) = fields.split_first()?;
        let mut bits = self.bits(first)?.to_vec();
        for field in rest {
            let next = self.bits(field)?;
            for (word, next) in bits.iter_mut().zip(next) {
                *word &= next;
            }
        }
        Some(bits)
    }
}
