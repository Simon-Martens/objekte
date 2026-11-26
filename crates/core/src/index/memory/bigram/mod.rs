//! Conservative record-level bigram candidates over separate text segments.

use std::collections::HashMap;
#[cfg(test)]
use std::collections::HashSet;

#[cfg(test)]
use crate::index::memory::inventory::append_shifted_bitmap;
#[cfg(test)]
use crate::model::RecordSlot;
#[cfg(test)]
use crate::source::TextSegment;
use roaring::RoaringBitmap;
use unicode_casefold::{Locale, UnicodeCaseFold, Variant};

mod builder;

pub(crate) use builder::{BigramBuilder, BigramContribution};

pub use crate::index::storage::text::BigramAlphabet;
pub(crate) use crate::index::storage::text::BigramRequest;
#[cfg(test)]
use crate::index::storage::text::for_each_gram;
#[cfg(test)]
pub use crate::index::storage::text::{BIGRAM_VERSION, FOLDED_REPRESENTATION};
use crate::index::storage::text::{ascii_pair, unicode_pair};

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

/// Record-level bigram postings for one text binding.
#[derive(Clone, Debug)]
pub struct BigramIndex {
    pub alphabet: BigramAlphabet,
    pub record_count: u32,
    pub normal: HashMap<u64, RoaringBitmap>,
    pub skip: HashMap<u64, RoaringBitmap>,
}

impl crate::index::storage::text::GramReader for BigramIndex {
    fn descriptor(&self) -> crate::index::storage::text::GramDescriptor {
        crate::index::storage::text::GramDescriptor {
            alphabet: self.alphabet,
            version: crate::index::storage::text::BIGRAM_VERSION,
            covered_records: self.record_count,
        }
    }

    fn visit_blocks(
        &self,
        limit: crate::index::storage::transfer::BatchLimit,
        visit: &mut crate::index::storage::text::VisitGramBlock<'_>,
    ) -> crate::index::storage::transfer::StorageResult<()> {
        use crate::index::storage::text::GramChannel;
        let maximum = limit.fixed_batch_len_after(
            "read native gram blocks",
            size_of::<u64>() + size_of::<GramChannel>(),
            size_of::<crate::model::RecordSlot>(),
        )?;
        for (channel, map) in [
            (GramChannel::Adjacent, &self.normal),
            (GramChannel::Skip, &self.skip),
        ] {
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            for key in keys {
                super::transfer::visit_bitmap_blocks(&map[key], maximum, |slots| {
                    visit(channel, *key, slots)
                })?;
            }
        }
        Ok(())
    }
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

    #[cfg(test)]
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

    #[cfg(test)]
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

    #[cfg(test)]
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
    #[cfg(test)]
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

    #[cfg(test)]
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

#[cfg(feature = "test-hooks")]
/// Check that conversion and borrowed text admission agree with owned strings.
pub fn assert_equivalent_logical_bigrams(
    schema: &crate::schema::Schema,
    record: &mut crate::source::ExtractedRecord,
    expected: &[&str],
) {
    use crate::model::RecordSlot;
    use crate::source::text::visit_logical_strings;

    crate::construction::file_preparation::finish_build_record(schema, record).unwrap();
    let mut strings = Vec::new();
    visit_logical_strings(&record.value, &mut |text| strings.push(text.to_owned()));
    assert_eq!(strings, expected);
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let mut borrowed = BigramBuilder::new(alphabet);
        borrowed
            .add_record_with(RecordSlot(0), |emit| {
                visit_logical_strings(&record.value, emit)
            })
            .unwrap();
        let borrowed = borrowed.finish().unwrap();
        let mut owned = BigramBuilder::new(alphabet);
        owned.add_record(RecordSlot(0), strings.iter()).unwrap();
        let owned = owned.finish().unwrap();
        assert_eq!(borrowed.normal, owned.normal);
        assert_eq!(borrowed.skip, owned.skip);
    }
}
