//! Conservative record-level bigram candidates over separate text segments.

use std::collections::{HashMap, HashSet};

use rayon::ThreadPool;
use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};
use unicode_casefold::{Locale, UnicodeCaseFold, Variant};

#[cfg(test)]
use crate::inventory::append_shifted_bitmap;
use crate::model::RecordSlot;
use crate::source::TextSegment;
use crate::workload::BigramAlphabet;

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

    pub(crate) fn gram_count(&self, alphabet: BigramAlphabet) -> usize {
        self.grams(alphabet).len()
    }

    pub(crate) fn retained_heap_bytes(&self) -> usize {
        (self.ascii.capacity() + self.unicode.capacity())
            .saturating_mul(std::mem::size_of::<(bool, u64)>())
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


#[derive(Clone, Copy, Debug)]
struct GramBlock {
    first_slot: u32,
    bits: u64,
}


/// Construction-only grams. Worker chunks and ordered batch admission move
/// complete words; the final Roaring dictionaries are built once afterward.
pub(crate) struct BigramContribution {
    alphabet: BigramAlphabet,
    record_count: u32,
    normal: HashMap<u64, Vec<GramBlock>>,
    skip: HashMap<u64, Vec<GramBlock>>,
}


impl BigramContribution {
    pub fn new(alphabet: BigramAlphabet) -> Self {
        Self {
            alphabet,
            record_count: 0,
            normal: HashMap::new(),
            skip: HashMap::new(),
        }
    }

    pub fn merge_shifted(&mut self, batch: Self, base: u32) -> Result<(), String> {
        if self.alphabet != batch.alphabet || self.record_count != base {
            return Err("bigram batch coverage or alphabet mismatch".into());
        }
        let count = base
            .checked_add(batch.record_count)
            .ok_or("bigram record overflow")?;
        validate_blocks(&batch.normal, batch.record_count)?;
        validate_blocks(&batch.skip, batch.record_count)?;
        for (key, mut blocks) in batch.normal {
            for block in &mut blocks {
                block.first_slot = base
                    .checked_add(block.first_slot)
                    .ok_or("bigram slot overflow")?;
            }
            self.normal.entry(key).or_default().append(&mut blocks);
        }
        for (key, mut blocks) in batch.skip {
            for block in &mut blocks {
                block.first_slot = base
                    .checked_add(block.first_slot)
                    .ok_or("bigram slot overflow")?;
            }
            self.skip.entry(key).or_default().append(&mut blocks);
        }
        self.record_count = count;
        Ok(())
    }

    pub fn into_index_in_pool(self, pool: &ThreadPool) -> Result<BigramIndex, String> {
        let Self {
            alphabet,
            record_count,
            normal,
            skip,
        } = self;
        let (normal, skip) = pool.join(
            move || convert_blocks(normal, record_count),
            move || convert_blocks(skip, record_count),
        );
        Ok(BigramIndex {
            alphabet,
            record_count,
            normal: normal?,
            skip: skip?,
        })
    }

    #[cfg(test)]
    fn into_index(self) -> Result<BigramIndex, String> {
        let Self {
            alphabet,
            record_count,
            normal,
            skip,
        } = self;
        Ok(BigramIndex {
            alphabet,
            record_count,
            normal: convert_blocks(normal, record_count)?,
            skip: convert_blocks(skip, record_count)?,
        })
    }
}


fn validate_blocks(
    columns: &HashMap<u64, Vec<GramBlock>>,
    record_count: u32,
) -> Result<(), String> {
    for blocks in columns.values() {
        let mut previous_end = None;
        for block in blocks {
            if block.bits == 0 {
                return Err("empty bigram block".into());
            }
            let end = block
                .first_slot
                .checked_add(63 - block.bits.leading_zeros())
                .ok_or("bigram slot overflow")?;
            if end >= record_count
                || previous_end.is_some_and(|previous| block.first_slot <= previous)
            {
                return Err("bigram block coverage or order mismatch".into());
            }
            previous_end = Some(end);
        }
    }
    Ok(())
}

fn convert_blocks(
    columns: HashMap<u64, Vec<GramBlock>>,
    record_count: u32,
) -> Result<HashMap<u64, RoaringBitmap>, String> {
    validate_blocks(&columns, record_count)?;
    columns
        .into_iter()
        .map(|(key, blocks)| {
            let slots = blocks.into_iter().flat_map(|block| {
                let mut bits = block.bits;
                std::iter::from_fn(move || {
                    if bits == 0 {
                        return None;
                    }
                    let offset = bits.trailing_zeros();
                    bits &= bits - 1;
                    Some(block.first_slot + offset)
                })
            });
            let mut posting = RoaringBitmap::new();
            posting
                .append(slots)
                .map_err(|_| "bigram blocks are not ordered")?;
            Ok((key, posting))
        })
        .collect()
}


/// A reusable 64-slot gram accumulator. ASCII uses its compact 95×95 pair
/// domain; Unicode keeps only keys touched by the current chunk.
enum GramWords {
    Ascii {
        words: Vec<u64>,
        touched: Vec<usize>,
    },
    Unicode(HashMap<u64, u64>),
}


impl GramWords {
    fn new(alphabet: BigramAlphabet) -> Self {
        match alphabet {
            BigramAlphabet::Ascii => Self::Ascii {
                words: Vec::new(),
                touched: Vec::new(),
            },
            BigramAlphabet::Unicode => Self::Unicode(HashMap::new()),
        }
    }

    fn set(&mut self, key: u64, bit: u64) {
        match self {
            Self::Ascii { words, touched } => {
                if words.is_empty() {
                    words.resize(95 * 95, 0);
                }
                let left = ((key >> 8) as u8 - 32) as usize;
                let right = (key as u8 - 32) as usize;
                let index = left * 95 + right;
                if words[index] == 0 {
                    touched.push(index);
                }
                words[index] |= bit;
            }
            Self::Unicode(words) => {
                *words.entry(key).or_default() |= bit;
            }
        }
    }

    fn drain(&mut self, mut emit: impl FnMut(u64, u64)) {
        match self {
            Self::Ascii { words, touched } => {
                for index in touched.drain(..) {
                    let left = (index / 95 + 32) as u64;
                    let right = (index % 95 + 32) as u64;
                    emit((left << 8) | right, std::mem::take(&mut words[index]));
                }
            }
            Self::Unicode(words) => {
                for (key, word) in words.drain() {
                    emit(key, word);
                }
                // Exceptional long/random text must not pin a large worker
                // scratch table for the rest of the repository build.
                if words.capacity() > 16_384 {
                    words.shrink_to(4_096);
                }
            }
        }
    }

    fn retained_capacity_bytes(&self) -> usize {
        match self {
            Self::Ascii { words, touched } => {
                words.capacity() * std::mem::size_of::<u64>()
                    + touched.capacity() * std::mem::size_of::<usize>()
            }
            // HashMap does not expose its allocation layout. This is an
            // estimate used only for comparing opt-in diagnostic runs.
            Self::Unicode(words) => words.capacity() * (2 * std::mem::size_of::<u64>() + 1),
        }
    }
}


/// Only accumulator storage crosses batch boundaries; completed postings do
/// not. Both variants allocate lazily when a text store sees grams.
pub(crate) struct BigramScratch {
    alphabet: BigramAlphabet,
    normal: GramWords,
    skip: GramWords,
}


impl BigramScratch {
    pub(crate) fn new(alphabet: BigramAlphabet) -> Self {
        Self {
            alphabet,
            normal: GramWords::new(alphabet),
            skip: GramWords::new(alphabet),
        }
    }

    pub(crate) fn retained_capacity_bytes(&self) -> usize {
        self.normal.retained_capacity_bytes() + self.skip.retained_capacity_bytes()
    }

    pub(crate) fn alphabet(&self) -> BigramAlphabet {
        self.alphabet
    }
}


/// Construction-only state; completed indexes retain the existing format.
/// The builder can start at any final slot and publishes each touched gram
/// once per chunk, with record bits mapped to final slots at publication.
pub(crate) struct BigramBuilder {
    contribution: BigramContribution,
    scratch: BigramScratch,
    scratch_initial_capacity_bytes: usize,
    chunk_start: u32,
    chunk_len: u32,
}


impl BigramBuilder {
    pub fn new(alphabet: BigramAlphabet) -> Self {
        Self::new_at(alphabet, 0)
    }

    pub fn new_at(alphabet: BigramAlphabet, first_slot: u32) -> Self {
        Self::with_scratch(BigramScratch::new(alphabet), first_slot)
    }

    pub(crate) fn with_scratch(scratch: BigramScratch, first_slot: u32) -> Self {
        let alphabet = scratch.alphabet;
        let scratch_initial_capacity_bytes = scratch.retained_capacity_bytes();
        let mut contribution = BigramContribution::new(alphabet);
        contribution.record_count = first_slot;
        Self {
            contribution,
            scratch,
            scratch_initial_capacity_bytes,
            chunk_start: 0,
            chunk_len: 0,
        }
    }

    fn begin_slot(&mut self, slot: RecordSlot) -> Result<u64, String> {
        if slot.0 != self.contribution.record_count {
            return Err("bigram slots must be added in record order".into());
        }
        let next = self
            .contribution
            .record_count
            .checked_add(1)
            .ok_or("bigram record overflow")?;
        if self.chunk_len == 0 {
            self.chunk_start = slot.0;
        }
        let bit = 1u64 << self.chunk_len;
        self.chunk_len += 1;
        self.contribution.record_count = next;
        Ok(bit)
    }

    pub fn add_record<I, S>(&mut self, slot: RecordSlot, segments: I) -> Result<(), String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let bit = self.begin_slot(slot)?;
        for segment in segments {
            self.add_segment(bit, segment.as_ref());
        }
        if self.chunk_len == 64 {
            self.flush()?;
        }
        Ok(())
    }

    /// Visit borrowed text while the source record is still alive. The
    /// visitor is called once per segment, so gram history resets at each one.
    pub fn add_record_with(
        &mut self,
        slot: RecordSlot,
        visit: impl FnOnce(&mut dyn FnMut(&str)),
    ) -> Result<(), String> {
        let bit = self.begin_slot(slot)?;
        visit(&mut |segment| self.add_segment(bit, segment));
        if self.chunk_len == 64 {
            self.flush()?;
        }
        Ok(())
    }

    fn add_segment(&mut self, bit: u64, segment: &str) {
        let alphabet = self.contribution.alphabet;
        let (normal, skip) = (&mut self.scratch.normal, &mut self.scratch.skip);
        for_each_gram(folded_chars(segment), alphabet, |is_skip, key| {
            if is_skip {
                skip.set(key, bit);
            } else {
                normal.set(key, bit);
            }
        });
    }

    pub fn add_empty(&mut self, slot: RecordSlot) -> Result<(), String> {
        self.begin_slot(slot)?;
        if self.chunk_len == 64 {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), String> {
        let start = self.chunk_start;
        self.scratch.normal.drain(|key, word| {
            self.contribution
                .normal
                .entry(key)
                .or_default()
                .push(GramBlock {
                    first_slot: start,
                    bits: word,
                });
        });
        self.scratch.skip.drain(|key, word| {
            self.contribution
                .skip
                .entry(key)
                .or_default()
                .push(GramBlock {
                    first_slot: start,
                    bits: word,
                });
        });
        self.chunk_len = 0;
        Ok(())
    }

    pub fn finish_blocks(self) -> Result<BigramContribution, String> {
        Ok(self.finish_blocks_with_scratch()?.0)
    }

    pub(crate) fn finish_blocks_with_scratch(
        mut self,
    ) -> Result<(BigramContribution, BigramScratch, bool), String> {
        if self.chunk_len != 0 {
            self.flush()?;
        }
        let grew = self.scratch.retained_capacity_bytes() > self.scratch_initial_capacity_bytes;
        Ok((self.contribution, self.scratch, grew))
    }

    #[cfg(test)]
    pub fn finish(self) -> Result<BigramIndex, String> {
        self.finish_blocks()?.into_index()
    }
}


#[cfg(test)]
#[path = "bigram.test.rs"]
mod tests;
