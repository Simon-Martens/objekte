//! Conservative record-level bigram candidates over separate text segments.

use std::collections::{HashMap, HashSet};

use roaring::RoaringBitmap;
use unicode_casefold::{Locale, UnicodeCaseFold, Variant};

#[cfg(test)]
use crate::inventory::append_shifted_bitmap;
use crate::model::RecordSlot;
use crate::source::TextSegment;
use crate::workload::BigramAlphabet;

mod builder;

pub(crate) use builder::{
    BigramBuilder, BigramConstructionStats, BigramContribution, storage_diagnostic_enabled,
};

pub const BIGRAM_VERSION: u32 = 3;
pub const FOLDED_REPRESENTATION: &str = "unicode-casefold-0.2.0/unicode-9.0.0/full/non-turkic";

pub(crate) fn fold(text: &str) -> String {
    let mut output = String::new();
    fold_into(text, &mut output);
    output
}

/// Write the built-in full Unicode fold into caller-owned reusable storage.
pub(crate) fn fold_into(text: &str, output: &mut String) {
    output.clear();
    if text.is_ascii() {
        output.push_str(text);
        output.make_ascii_lowercase();
    } else {
        output.extend(folded_chars(text));
    }
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

/// Construction reads borrowed UTF-8 directly. ASCII bytes need no character
/// iterator or re-encoding; only non-ASCII scalars enter the locked full fold.
trait FoldedSink {
    fn ascii(&mut self, byte: u8);
    fn scalar(&mut self, scalar: char);
}

struct AsciiGramSink<E> {
    previous: Option<u8>,
    before_previous: Option<u8>,
    emit: E,
}

impl<E: FnMut(bool, u64)> AsciiGramSink<E> {
    fn push(&mut self, byte: u8) {
        if let Some(key) = self.previous.and_then(|left| ascii_pair(left, byte)) {
            (self.emit)(false, key);
        }
        if let Some(key) = self.before_previous.and_then(|left| ascii_pair(left, byte)) {
            (self.emit)(true, key);
        }
        self.before_previous = self.previous;
        self.previous = Some(byte);
    }
}

impl<E: FnMut(bool, u64)> FoldedSink for AsciiGramSink<E> {
    fn ascii(&mut self, byte: u8) {
        self.push(byte);
    }

    fn scalar(&mut self, scalar: char) {
        let mut bytes = [0; 4];
        for &byte in scalar.encode_utf8(&mut bytes).as_bytes() {
            self.push(byte);
        }
    }
}

struct UnicodeGramSink<E> {
    previous: Option<char>,
    before_previous: Option<char>,
    emit: E,
}

impl<E: FnMut(bool, u64)> UnicodeGramSink<E> {
    fn push(&mut self, scalar: char) {
        if let Some(left) = self.previous {
            (self.emit)(false, unicode_pair(left, scalar));
        }
        if let Some(left) = self.before_previous {
            (self.emit)(true, unicode_pair(left, scalar));
        }
        self.before_previous = self.previous;
        self.previous = Some(scalar);
    }
}

impl<E: FnMut(bool, u64)> FoldedSink for UnicodeGramSink<E> {
    fn ascii(&mut self, byte: u8) {
        self.push(char::from(byte));
    }

    fn scalar(&mut self, scalar: char) {
        self.push(scalar);
    }
}

#[inline]
fn lower_ascii(byte: u8) -> u8 {
    if byte.is_ascii_uppercase() {
        byte + 32
    } else {
        byte
    }
}

#[inline]
fn fold_non_ascii(text: &str, offset: usize, sink: &mut impl FoldedSink) -> usize {
    // The caller advances by complete scalars, so this is always a UTF-8
    // boundary. Full case folding can expand one scalar into several.
    let scalar = text[offset..].chars().next().expect("non-ASCII scalar");
    for folded in scalar.case_fold_with(Variant::Full, Locale::NonTurkic) {
        sink.scalar(folded);
    }
    offset + scalar.len_utf8()
}

fn walk_folded_scalar(text: &str, sink: &mut impl FoldedSink) {
    let bytes = text.as_bytes();
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes[offset].is_ascii() {
            sink.ascii(lower_ascii(bytes[offset]));
            offset += 1;
        } else {
            offset = fold_non_ascii(text, offset, sink);
        }
    }
}

fn for_each_segment_gram(text: &str, alphabet: BigramAlphabet, mut emit: impl FnMut(bool, u64)) {
    match alphabet {
        BigramAlphabet::Ascii => {
            let mut sink = AsciiGramSink {
                previous: None,
                before_previous: None,
                emit: &mut emit,
            };
            walk_folded_scalar(text, &mut sink);
        }
        BigramAlphabet::Unicode => {
            let mut sink = UnicodeGramSink {
                previous: None,
                before_previous: None,
                emit: &mut emit,
            };
            walk_folded_scalar(text, &mut sink);
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

    pub(crate) fn grams(&self, alphabet: BigramAlphabet) -> &[(bool, u64)] {
        match alphabet {
            BigramAlphabet::Ascii => &self.ascii,
            BigramAlphabet::Unicode => &self.unicode,
        }
    }

    pub(crate) fn gram_count(&self, alphabet: BigramAlphabet) -> usize {
        self.grams(alphabet).len()
    }

    pub(crate) fn retained_heap_bytes(&self) -> usize {
        (self.ascii.capacity() + self.unicode.capacity())
            .saturating_mul(std::mem::size_of::<(bool, u64)>())
    }
}

/// Record-level bigram postings for one text binding.
#[derive(Clone, Debug)]
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

    pub(crate) fn for_each_normal(
        &self,
        mut visit: impl FnMut(&u64, &RoaringBitmap) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut keys = self.normal.keys().collect::<Vec<_>>();
        keys.sort_unstable();
        for key in keys {
            visit(key, &self.normal[key])?;
        }
        Ok(())
    }

    pub(crate) fn for_each_skip(
        &self,
        mut visit: impl FnMut(&u64, &RoaringBitmap) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut keys = self.skip.keys().collect::<Vec<_>>();
        keys.sort_unstable();
        for key in keys {
            visit(key, &self.skip[key])?;
        }
        Ok(())
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
            for_each_segment_gram(&segment.text, self.alphabet, |is_skip, key| {
                if is_skip {
                    skip.insert(key);
                } else {
                    normal.insert(key);
                }
            });
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

    #[cfg(test)]
    pub(crate) fn merge_shifted(&mut self, batch: Self, base: u32) -> Result<(), String> {
        if self.alphabet != batch.alphabet || self.record_count != base {
            return Err("bigram batch coverage or alphabet mismatch".into());
        }
        let count = base
            .checked_add(batch.record_count)
            .ok_or("bigram record overflow")?;
        for (key, posting) in batch.normal {
            append_shifted_bitmap(self.normal.entry(key).or_default(), &posting, base)?;
        }
        for (key, posting) in batch.skip {
            append_shifted_bitmap(self.skip.entry(key).or_default(), &posting, base)?;
        }
        self.record_count = count;
        Ok(())
    }

    /// `None` means no safe useful restriction; an empty bitmap is a proven
    /// absence of this necessary gram over complete index coverage.
    pub fn candidates(&self, literal: &str) -> Option<RoaringBitmap> {
        self.candidates_prepared(&BigramRequest::new(literal))
    }

    pub(crate) fn candidates_prepared(&self, request: &BigramRequest) -> Option<RoaringBitmap> {
        self.candidates_prepared_restricted(request, None)
    }

    pub(crate) fn candidates_prepared_restricted(
        &self,
        request: &BigramRequest,
        input: Option<&RoaringBitmap>,
    ) -> Option<RoaringBitmap> {
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
        let mut result = match input {
            Some(input) => *columns.first()? & input,
            None => (*columns.first()?).clone(),
        };
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
