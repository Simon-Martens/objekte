//! Conservative record-level bigram candidates over separate text segments.

use std::collections::{HashMap, HashSet};

use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};
use unicode_casefold::{Locale, UnicodeCaseFold, Variant};

use crate::model::RecordSlot;
use crate::source::TextSegment;
use crate::workload::BigramAlphabet;

pub const BIGRAM_VERSION: u32 = 3;
pub const FOLDED_REPRESENTATION: &str = "unicode-casefold-0.2.0/unicode-9.0.0/full/non-turkic";

pub(crate) fn fold(text: &str) -> String {
    folded_chars(text).collect()
}

fn folded_chars(text: &str) -> impl Iterator<Item = char> + '_ {
    text.case_fold_with(Variant::Full, Locale::NonTurkic)
}

/// Emit windows over the folded stream without losing unsupported ASCII byte
/// positions or joining independent reader-owned segments.
fn for_each_gram(
    chars: impl Iterator<Item = char>,
    alphabet: BigramAlphabet,
    mut emit: impl FnMut(bool, u64),
) {
    match alphabet {
        BigramAlphabet::Ascii => {
            let (mut previous, mut before_previous) = (None, None);
            for scalar in chars {
                let mut buffer = [0u8; 4];
                for &byte in scalar.encode_utf8(&mut buffer).as_bytes() {
                    if let Some(left) = previous.and_then(|left| ascii_pair(left, byte)) {
                        emit(false, left);
                    }
                    if let Some(left) = before_previous.and_then(|left| ascii_pair(left, byte)) {
                        emit(true, left);
                    }
                    before_previous = previous;
                    previous = Some(byte);
                }
            }
        }
        BigramAlphabet::Unicode => {
            let (mut previous, mut before_previous) = (None, None);
            for scalar in chars {
                if let Some(left) = previous {
                    emit(false, unicode_pair(left, scalar));
                }
                if let Some(left) = before_previous {
                    emit(true, unicode_pair(left, scalar));
                }
                before_previous = previous;
                previous = Some(scalar);
            }
        }
    }
}


/// Both alphabets' necessary grams for one already prepared representation.
/// Exact matching never reads these keys as a literal.
#[derive(Clone, Debug)]
pub(crate) struct BigramRequest {
    ascii: Vec<(bool, u64)>,
    unicode: Vec<(bool, u64)>,
}


impl BigramRequest {
    pub fn from_folded(folded: &str) -> Self {
        let collect = |alphabet| {
            let mut grams = Vec::new();
            for_each_gram(folded.chars(), alphabet, |skip, key| {
                grams.push((skip, key))
            });
            grams.sort_unstable();
            grams.dedup();
            grams
        };
        Self {
            ascii: collect(BigramAlphabet::Ascii),
            unicode: collect(BigramAlphabet::Unicode),
        }
    }

    pub fn new(literal: &str) -> Self {
        Self::from_folded(&fold(literal))
    }

    fn grams(&self, alphabet: BigramAlphabet) -> &[(bool, u64)] {
        match alphabet {
            BigramAlphabet::Ascii => &self.ascii,
            BigramAlphabet::Unicode => &self.unicode,
        }
    }
}


/// Record-level bigram postings for one text binding.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BigramIndex {
    pub alphabet: BigramAlphabet,
    pub record_count: u32,
    pub normal: HashMap<u64, RoaringBitmap>,
    pub skip: HashMap<u64, RoaringBitmap>,
}


impl BigramIndex {
    pub fn validate_domain(&self, allowed: &RoaringBitmap) -> Result<(), String> {
        if self
            .normal
            .values()
            .chain(self.skip.values())
            .any(|slots| !slots.is_subset(allowed))
        {
            return Err("bigram posting outside eligible domain".into());
        }
        Ok(())
    }

    pub fn new_ascii() -> Self {
        Self::new(BigramAlphabet::Ascii)
    }

    pub fn new(alphabet: BigramAlphabet) -> Self {
        Self {
            alphabet,
            record_count: 0,
            normal: HashMap::new(),
            skip: HashMap::new(),
        }
    }

    pub fn add_record(&mut self, slot: RecordSlot, segments: &[TextSegment]) -> Result<(), String> {
        if segments.is_empty() {
            return self.add_empty(slot);
        }
        if slot.0 != self.record_count {
            return Err("bigram slots must be added in record order".into());
        }
        self.record_count = self
            .record_count
            .checked_add(1)
            .ok_or("bigram record overflow")?;
        let mut normal = HashSet::new();
        let mut skip = HashSet::new();
        for segment in segments {
            for_each_gram(
                folded_chars(&segment.text),
                self.alphabet,
                |is_skip, key| {
                    if is_skip {
                        skip.insert(key);
                    } else {
                        normal.insert(key);
                    }
                },
            );
        }
        for gram in normal {
            self.normal.entry(gram).or_default().insert(slot.0);
        }
        for gram in skip {
            self.skip.entry(gram).or_default().insert(slot.0);
        }
        Ok(())
    }

    pub fn add_empty(&mut self, slot: RecordSlot) -> Result<(), String> {
        if slot.0 != self.record_count {
            return Err("bigram slots must be added in record order".into());
        }
        self.record_count = self
            .record_count
            .checked_add(1)
            .ok_or("bigram record overflow")?;
        Ok(())
    }

    /// `None` means no safe useful restriction; an empty bitmap is a proven
    /// absence of this necessary gram over complete index coverage.
    pub fn candidates(&self, literal: &str) -> Option<RoaringBitmap> {
        self.candidates_prepared(&BigramRequest::new(literal))
    }

    pub(crate) fn candidates_prepared(&self, request: &BigramRequest) -> Option<RoaringBitmap> {
        let grams = request.grams(self.alphabet);
        if grams.is_empty() {
            return None;
        }
        let mut columns = Vec::new();
        for &(skip, key) in grams {
            let column = if skip {
                self.skip.get(&key)
            } else {
                self.normal.get(&key)
            };
            let Some(column) = column else {
                return Some(RoaringBitmap::new());
            };
            columns.push(column);
        }
        columns.sort_by_key(|column| column.len());
        let mut result = (*columns.first()?).clone();
        for column in columns.into_iter().skip(1) {
            result &= column;
            if result.is_empty() {
                break;
            }
        }
        Some(result)
    }

    pub fn estimate_min_cardinality(&self, literal: &str) -> Option<u64> {
        self.estimate_prepared(&BigramRequest::new(literal))
    }

    pub(crate) fn estimate_prepared(&self, request: &BigramRequest) -> Option<u64> {
        request
            .grams(self.alphabet)
            .iter()
            .map(|&(skip, key)| {
                if skip {
                    self.skip.get(&key)
                } else {
                    self.normal.get(&key)
                }
                .map(RoaringBitmap::len)
                .unwrap_or(0)
            })
            .min()
    }

    pub fn validate(&self, record_count: usize) -> Result<(), String> {
        if self.record_count as usize != record_count {
            return Err("bigram coverage mismatch".into());
        }
        if self
            .normal
            .values()
            .chain(self.skip.values())
            .any(|column| column.max().is_some_and(|slot| slot >= self.record_count))
        {
            return Err("bigram slot out of bounds".into());
        }
        Ok(())
    }

    pub fn estimated_heap_bytes(&self) -> usize {
        self.normal
            .values()
            .chain(self.skip.values())
            .map(|set| set.serialized_size() as usize + 48)
            .sum()
    }
}


fn ascii_pair(a: u8, b: u8) -> Option<u64> {
    if !(32..=126).contains(&a) || !(32..=126).contains(&b) {
        return None;
    }
    Some(((a as u64) << 8) | b as u64)
}

fn unicode_pair(a: char, b: char) -> u64 {
    ((a as u64) << 21) | b as u64
}

#[cfg(test)]
#[path = "bigram.test.rs"]
mod tests;
