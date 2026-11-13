//! Conservative record-level bigram candidates over separate text segments.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
#[cfg(feature = "detailed-profiling")]
use std::sync::OnceLock;
use std::time::Instant;

use rayon::ThreadPool;
use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};
use unicode_casefold::{Locale, UnicodeCaseFold, Variant};

use crate::bigram_dictionary::{BigramSpans, SealedBigramIndex};
#[cfg(test)]
use crate::inventory::append_shifted_bitmap;
use crate::model::RecordSlot;
use crate::segment_format::SegmentReader;
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
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BigramIndex {
    pub alphabet: BigramAlphabet,
    pub record_count: u32,
    pub normal: HashMap<u64, RoaringBitmap>,
    pub skip: HashMap<u64, RoaringBitmap>,
    #[serde(skip)]
    sealed: Option<Arc<SealedBigramIndex>>,
}

impl BigramIndex {
    pub(crate) fn is_sealed(&self) -> bool {
        self.sealed.is_some()
    }

    pub(crate) fn sealed_reader(&self) -> Option<Arc<SegmentReader>> {
        self.sealed.as_ref().map(|sealed| sealed.segment_reader())
    }

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
            sealed: None,
        }
    }

    /// Replace mutable gram maps with a checked shared reader only after the
    /// corresponding segment bytes have been published. The current snapshot
    /// writer must be retired before production callers use this transition.
    pub(crate) fn install_sealed(
        &mut self,
        reader: Arc<SegmentReader>,
        spans: BigramSpans,
        allowed: Arc<RoaringBitmap>,
    ) -> Result<(), String> {
        let sealed =
            SealedBigramIndex::open(reader, spans, self.alphabet, self.record_count, allowed)?;
        self.normal = HashMap::new();
        self.skip = HashMap::new();
        self.sealed = Some(Arc::new(sealed));
        Ok(())
    }

    pub(crate) fn install_constructed(
        &mut self,
        reader: Arc<SegmentReader>,
        spans: BigramSpans,
        allowed: Arc<RoaringBitmap>,
        normal_presence: Option<crate::bigram_dictionary::GramPresence>,
        skip_presence: Option<crate::bigram_dictionary::GramPresence>,
    ) -> Result<(), String> {
        let sealed = SealedBigramIndex::from_constructed(
            reader, spans, self.alphabet, self.record_count, allowed,
            normal_presence, skip_presence,
        )?;
        self.normal = HashMap::new();
        self.skip = HashMap::new();
        self.sealed = Some(Arc::new(sealed));
        Ok(())
    }

    pub(crate) fn for_each_normal(
        &self,
        mut visit: impl FnMut(&u64, &RoaringBitmap) -> Result<(), String>,
    ) -> Result<(), String> {
        if let Some(sealed) = &self.sealed {
            sealed.for_each_normal(visit)
        } else {
            let mut keys = self.normal.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            for key in keys {
                visit(key, &self.normal[key])?;
            }
            Ok(())
        }
    }

    pub(crate) fn for_each_skip(
        &self,
        mut visit: impl FnMut(&u64, &RoaringBitmap) -> Result<(), String>,
    ) -> Result<(), String> {
        if let Some(sealed) = &self.sealed {
            sealed.for_each_skip(visit)
        } else {
            let mut keys = self.skip.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            for key in keys {
                visit(key, &self.skip[key])?;
            }
            Ok(())
        }
    }

    pub(crate) fn try_candidates_prepared(
        &self,
        request: &BigramRequest,
    ) -> Result<Option<RoaringBitmap>, String> {
        match &self.sealed {
            Some(sealed) => sealed.candidates_prepared(request),
            None => Ok(self.candidates_prepared(request)),
        }
    }

    pub(crate) fn try_estimate_prepared(
        &self,
        request: &BigramRequest,
    ) -> Result<Option<u64>, String> {
        match &self.sealed {
            Some(sealed) => sealed.estimate_prepared(request),
            None => Ok(self.estimate_prepared(request)),
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

#[derive(Clone, Copy, Debug)]
struct PendingBlock {
    key: u64,
    block: GramBlock,
}

#[derive(Clone, Copy, Debug)]
struct ColumnSpan {
    key: u64,
    start: u32,
    len: u32,
}

#[derive(Default)]
struct PackedColumns {
    spans: Vec<ColumnSpan>,
    blocks: Vec<GramBlock>,
}

struct PackedChunk {
    /// Positions in both payloads remain local to this chunk until conversion.
    base: u32,
    count: u32,
    normal: PackedColumns,
    skip: PackedColumns,
}

pub(crate) fn storage_diagnostic_enabled() -> bool {
    #[cfg(not(feature = "detailed-profiling"))]
    {
        return false;
    }

    #[cfg(feature = "detailed-profiling")]
    {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        *ENABLED.get_or_init(|| std::env::var_os("OBJEKTE_BIGRAM_STORAGE_STATS").is_some())
    }
}

fn diagnostic_elapsed_ns(started: Option<Instant>) -> u64 {
    started.map_or(0, |started| started.elapsed().as_nanos() as u64)
}

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct BigramConstructionStats {
    pub chunk_element_size: usize,
    pub span_element_size: usize,
    pub block_element_size: usize,
    pub pending_element_size: usize,
    pub chunk_len: usize,
    pub chunk_capacity: usize,
    pub normal_span_len: usize,
    pub normal_span_capacity: usize,
    pub normal_block_len: usize,
    pub normal_block_capacity: usize,
    pub skip_span_len: usize,
    pub skip_span_capacity: usize,
    pub skip_block_len: usize,
    pub skip_block_capacity: usize,
    pub pending_normal_capacity: usize,
    pub pending_skip_capacity: usize,
    pub retained_capacity_bytes: usize,
    pub live_payload_bytes: usize,
    pub diagnostic_counter_bytes: usize,
    pub copied_block_bytes: u64,
    pub admitted_chunks: u64,
    /// Lower bound for HashMap-backed scratch: excludes control bytes and
    /// allocator rounding. Maxima are per contribution, not summed peaks.
    pub worker_scratch_peak_capacity_bytes: usize,
    pub worker_scratch_after_flush_capacity_bytes: usize,
    pub pending_peak_capacity_bytes: usize,
    pub gram_set_lookups: u64,
    pub duplicate_bit_ors: u64,
    pub sorted_entries: u64,
    pub add_segment_ns: u64,
    pub scratch_drain_ns: u64,
    pub sort_ns: u64,
    pub pack_ns: u64,
    pub validation_ns: u64,
    pub admission_merge_ns: u64,
}

impl BigramConstructionStats {
    pub fn absorb(&mut self, other: Self) {
        self.chunk_element_size = self.chunk_element_size.max(other.chunk_element_size);
        self.span_element_size = self.span_element_size.max(other.span_element_size);
        self.block_element_size = self.block_element_size.max(other.block_element_size);
        self.pending_element_size = self.pending_element_size.max(other.pending_element_size);
        self.chunk_len = self.chunk_len.saturating_add(other.chunk_len);
        self.chunk_capacity = self.chunk_capacity.saturating_add(other.chunk_capacity);
        self.normal_span_len = self.normal_span_len.saturating_add(other.normal_span_len);
        self.normal_span_capacity = self
            .normal_span_capacity
            .saturating_add(other.normal_span_capacity);
        self.normal_block_len = self.normal_block_len.saturating_add(other.normal_block_len);
        self.normal_block_capacity = self
            .normal_block_capacity
            .saturating_add(other.normal_block_capacity);
        self.skip_span_len = self.skip_span_len.saturating_add(other.skip_span_len);
        self.skip_span_capacity = self
            .skip_span_capacity
            .saturating_add(other.skip_span_capacity);
        self.skip_block_len = self.skip_block_len.saturating_add(other.skip_block_len);
        self.skip_block_capacity = self
            .skip_block_capacity
            .saturating_add(other.skip_block_capacity);
        self.pending_normal_capacity = self
            .pending_normal_capacity
            .saturating_add(other.pending_normal_capacity);
        self.pending_skip_capacity = self
            .pending_skip_capacity
            .saturating_add(other.pending_skip_capacity);
        self.retained_capacity_bytes = self
            .retained_capacity_bytes
            .saturating_add(other.retained_capacity_bytes);
        self.live_payload_bytes = self
            .live_payload_bytes
            .saturating_add(other.live_payload_bytes);
        self.diagnostic_counter_bytes = self
            .diagnostic_counter_bytes
            .saturating_add(other.diagnostic_counter_bytes);
        self.copied_block_bytes = self
            .copied_block_bytes
            .saturating_add(other.copied_block_bytes);
        self.admitted_chunks = self.admitted_chunks.saturating_add(other.admitted_chunks);
        self.worker_scratch_peak_capacity_bytes = self
            .worker_scratch_peak_capacity_bytes
            .max(other.worker_scratch_peak_capacity_bytes);
        self.worker_scratch_after_flush_capacity_bytes = self
            .worker_scratch_after_flush_capacity_bytes
            .max(other.worker_scratch_after_flush_capacity_bytes);
        self.pending_peak_capacity_bytes = self
            .pending_peak_capacity_bytes
            .max(other.pending_peak_capacity_bytes);
        self.gram_set_lookups = self.gram_set_lookups.saturating_add(other.gram_set_lookups);
        self.duplicate_bit_ors = self
            .duplicate_bit_ors
            .saturating_add(other.duplicate_bit_ors);
        self.sorted_entries = self.sorted_entries.saturating_add(other.sorted_entries);
        self.add_segment_ns = self.add_segment_ns.saturating_add(other.add_segment_ns);
        self.scratch_drain_ns = self.scratch_drain_ns.saturating_add(other.scratch_drain_ns);
        self.sort_ns = self.sort_ns.saturating_add(other.sort_ns);
        self.pack_ns = self.pack_ns.saturating_add(other.pack_ns);
        self.validation_ns = self.validation_ns.saturating_add(other.validation_ns);
        self.admission_merge_ns = self
            .admission_merge_ns
            .saturating_add(other.admission_merge_ns);
    }
}

#[derive(Default)]
struct BigramWorkCounters {
    gram_set_lookups: u64,
    duplicate_bit_ors: u64,
    sorted_entries: u64,
    add_segment_ns: u64,
    scratch_drain_ns: u64,
    sort_ns: u64,
    pack_ns: u64,
    validation_ns: u64,
    admission_merge_ns: u64,
}

impl BigramWorkCounters {
    fn absorb(&mut self, other: Self) {
        self.gram_set_lookups = self.gram_set_lookups.saturating_add(other.gram_set_lookups);
        self.duplicate_bit_ors = self
            .duplicate_bit_ors
            .saturating_add(other.duplicate_bit_ors);
        self.sorted_entries = self.sorted_entries.saturating_add(other.sorted_entries);
        self.add_segment_ns = self.add_segment_ns.saturating_add(other.add_segment_ns);
        self.scratch_drain_ns = self.scratch_drain_ns.saturating_add(other.scratch_drain_ns);
        self.sort_ns = self.sort_ns.saturating_add(other.sort_ns);
        self.pack_ns = self.pack_ns.saturating_add(other.pack_ns);
        self.validation_ns = self.validation_ns.saturating_add(other.validation_ns);
        self.admission_merge_ns = self
            .admission_merge_ns
            .saturating_add(other.admission_merge_ns);
    }
}

/// Construction-only grams. Each admitted batch owns contiguous per-column
/// word spans and one global base; admission never rebases individual words.
pub(crate) struct BigramContribution {
    alphabet: BigramAlphabet,
    record_count: u32,
    chunks: Vec<PackedChunk>,
    pending_normal: Vec<PendingBlock>,
    pending_skip: Vec<PendingBlock>,
    copied_block_bytes: u64,
    admitted_chunks: u64,
    worker_scratch_peak_capacity_bytes: usize,
    worker_scratch_after_flush_capacity_bytes: usize,
    pending_peak_capacity_bytes: usize,
    work: Option<Box<BigramWorkCounters>>,
}

impl BigramContribution {
    pub fn new(alphabet: BigramAlphabet) -> Self {
        Self {
            alphabet,
            record_count: 0,
            chunks: Vec::new(),
            pending_normal: Vec::new(),
            pending_skip: Vec::new(),
            copied_block_bytes: 0,
            admitted_chunks: 0,
            worker_scratch_peak_capacity_bytes: 0,
            worker_scratch_after_flush_capacity_bytes: 0,
            pending_peak_capacity_bytes: 0,
            work: storage_diagnostic_enabled().then(|| Box::new(BigramWorkCounters::default())),
        }
    }

    pub(crate) fn storage_stats(&self) -> BigramConstructionStats {
        let mut stats = BigramConstructionStats {
            chunk_element_size: std::mem::size_of::<PackedChunk>(),
            span_element_size: std::mem::size_of::<ColumnSpan>(),
            block_element_size: std::mem::size_of::<GramBlock>(),
            pending_element_size: std::mem::size_of::<PendingBlock>(),
            chunk_len: self.chunks.len(),
            chunk_capacity: self.chunks.capacity(),
            pending_normal_capacity: self.pending_normal.capacity(),
            pending_skip_capacity: self.pending_skip.capacity(),
            copied_block_bytes: self.copied_block_bytes,
            admitted_chunks: self.admitted_chunks,
            worker_scratch_peak_capacity_bytes: self.worker_scratch_peak_capacity_bytes,
            worker_scratch_after_flush_capacity_bytes: self
                .worker_scratch_after_flush_capacity_bytes,
            pending_peak_capacity_bytes: self.pending_peak_capacity_bytes,
            diagnostic_counter_bytes: self
                .work
                .as_ref()
                .map_or(0, |_| std::mem::size_of::<BigramWorkCounters>()),
            ..Default::default()
        };
        if let Some(work) = &self.work {
            stats.gram_set_lookups = work.gram_set_lookups;
            stats.duplicate_bit_ors = work.duplicate_bit_ors;
            stats.sorted_entries = work.sorted_entries;
            stats.add_segment_ns = work.add_segment_ns;
            stats.scratch_drain_ns = work.scratch_drain_ns;
            stats.sort_ns = work.sort_ns;
            stats.pack_ns = work.pack_ns;
            stats.validation_ns = work.validation_ns;
            stats.admission_merge_ns = work.admission_merge_ns;
        }
        stats.retained_capacity_bytes = stats.diagnostic_counter_bytes
            + self.chunks.capacity() * std::mem::size_of::<PackedChunk>()
            + (self.pending_normal.capacity() + self.pending_skip.capacity())
                * std::mem::size_of::<PendingBlock>();
        for chunk in &self.chunks {
            stats.normal_span_len += chunk.normal.spans.len();
            stats.normal_span_capacity += chunk.normal.spans.capacity();
            stats.normal_block_len += chunk.normal.blocks.len();
            stats.normal_block_capacity += chunk.normal.blocks.capacity();
            stats.skip_span_len += chunk.skip.spans.len();
            stats.skip_span_capacity += chunk.skip.spans.capacity();
            stats.skip_block_len += chunk.skip.blocks.len();
            stats.skip_block_capacity += chunk.skip.blocks.capacity();
        }
        stats.retained_capacity_bytes += (stats.normal_span_capacity + stats.skip_span_capacity)
            * std::mem::size_of::<ColumnSpan>()
            + (stats.normal_block_capacity + stats.skip_block_capacity)
                * std::mem::size_of::<GramBlock>();
        stats.live_payload_bytes = (stats.normal_span_len + stats.skip_span_len)
            * std::mem::size_of::<ColumnSpan>()
            + (stats.normal_block_len + stats.skip_block_len) * std::mem::size_of::<GramBlock>();
        stats
    }

    pub fn merge_shifted(&mut self, batch: Self, base: u32) -> Result<(), String> {
        let validation_started =
            crate::detailed_timer::start_if_enabled(storage_diagnostic_enabled());
        if self.alphabet != batch.alphabet || self.record_count != base {
            return Err("bigram batch coverage or alphabet mismatch".into());
        }
        let count = base
            .checked_add(batch.record_count)
            .ok_or("bigram record overflow")?;
        if !batch.pending_normal.is_empty() || !batch.pending_skip.is_empty() {
            return Err("unpacked bigram contribution".into());
        }
        validate_chunks(&batch.chunks, batch.record_count)?;
        let validation_ns = diagnostic_elapsed_ns(validation_started);
        let merge_started = crate::detailed_timer::start_if_enabled(storage_diagnostic_enabled());
        let admitted_chunks = batch.chunks.len() as u64;
        self.chunks.reserve(batch.chunks.len());
        for mut chunk in batch.chunks {
            chunk.base = base.checked_add(chunk.base).ok_or("bigram slot overflow")?;
            self.chunks.push(chunk);
        }
        self.copied_block_bytes = self
            .copied_block_bytes
            .saturating_add(batch.copied_block_bytes);
        self.admitted_chunks = self.admitted_chunks.saturating_add(admitted_chunks);
        self.worker_scratch_peak_capacity_bytes = self
            .worker_scratch_peak_capacity_bytes
            .max(batch.worker_scratch_peak_capacity_bytes);
        self.worker_scratch_after_flush_capacity_bytes = self
            .worker_scratch_after_flush_capacity_bytes
            .max(batch.worker_scratch_after_flush_capacity_bytes);
        self.pending_peak_capacity_bytes = self
            .pending_peak_capacity_bytes
            .max(batch.pending_peak_capacity_bytes);
        if let (Some(work), Some(batch_work)) = (self.work.as_mut(), batch.work) {
            work.absorb(*batch_work);
            work.validation_ns = work.validation_ns.saturating_add(validation_ns);
            work.admission_merge_ns = work
                .admission_merge_ns
                .saturating_add(diagnostic_elapsed_ns(merge_started));
        }
        self.record_count = count;
        Ok(())
    }

    pub fn into_index_in_pool(self, pool: &ThreadPool) -> Result<BigramIndex, String> {
        let Self {
            alphabet,
            record_count,
            chunks,
            ..
        } = self;
        // Validate the intact chunk ownership and all descriptors before
        // splitting the two alphabets' columns for parallel conversion.
        validate_chunks(&chunks, record_count)?;
        let (normal, skip) = split_chunks(chunks);
        let (normal, skip) =
            pool.join(move || convert_blocks(normal), move || convert_blocks(skip));
        Ok(BigramIndex {
            alphabet,
            record_count,
            normal: normal?,
            skip: skip?,
            sealed: None,
        })
    }

    #[cfg(test)]
    fn into_index(self) -> Result<BigramIndex, String> {
        let Self {
            alphabet,
            record_count,
            chunks,
            ..
        } = self;
        validate_chunks(&chunks, record_count)?;
        let (normal, skip) = split_chunks(chunks);
        Ok(BigramIndex {
            alphabet,
            record_count,
            normal: convert_blocks(normal)?,
            skip: convert_blocks(skip)?,
            sealed: None,
        })
    }
}

fn validate_columns(columns: &PackedColumns, count: u32) -> Result<(), String> {
    let mut next_start = 0usize;
    let mut previous_key = None;
    for span in &columns.spans {
        let start = span.start as usize;
        let end = start
            .checked_add(span.len as usize)
            .ok_or("bigram descriptor overflow")?;
        if span.len == 0
            || start != next_start
            || end > columns.blocks.len()
            || previous_key.is_some_and(|key| span.key <= key)
        {
            return Err("bigram descriptor coverage or order mismatch".into());
        }
        let mut previous_end = None;
        for block in &columns.blocks[start..end] {
            if block.bits == 0 {
                return Err("empty bigram block".into());
            }
            let last = block
                .first_slot
                .checked_add(63 - block.bits.leading_zeros())
                .ok_or("bigram slot overflow")?;
            if last >= count || previous_end.is_some_and(|prev| block.first_slot <= prev) {
                return Err("bigram block coverage or order mismatch".into());
            }
            previous_end = Some(last);
        }
        previous_key = Some(span.key);
        next_start = end;
    }
    if next_start != columns.blocks.len() {
        return Err("unreachable bigram blocks".into());
    }
    Ok(())
}

fn validate_chunks(chunks: &[PackedChunk], count: u32) -> Result<(), String> {
    let mut previous_end = 0;
    for chunk in chunks {
        let end = chunk
            .base
            .checked_add(chunk.count)
            .ok_or("bigram slot overflow")?;
        if chunk.count == 0 || chunk.base < previous_end || end > count {
            return Err("bigram chunk coverage or order mismatch".into());
        }
        validate_columns(&chunk.normal, chunk.count)?;
        validate_columns(&chunk.skip, chunk.count)?;
        previous_end = end;
    }
    Ok(())
}

fn pack_columns(
    mut pending: Vec<PendingBlock>,
    contribution: &mut BigramContribution,
) -> Result<PackedColumns, String> {
    if pending.is_empty() {
        return Ok(PackedColumns::default());
    }
    let sort_started = crate::detailed_timer::start_if_enabled(contribution.work.is_some());
    if let Some(work) = contribution.work.as_mut() {
        work.sorted_entries = work.sorted_entries.saturating_add(pending.len() as u64);
    }
    pending.sort_unstable_by_key(|entry| (entry.key, entry.block.first_slot));
    if let Some(work) = contribution.work.as_mut() {
        work.sort_ns = work
            .sort_ns
            .saturating_add(diagnostic_elapsed_ns(sort_started));
    }
    let pack_started = crate::detailed_timer::start_if_enabled(contribution.work.is_some());
    let mut packed = PackedColumns {
        spans: Vec::new(),
        blocks: Vec::with_capacity(pending.len()),
    };
    for entry in pending {
        if packed
            .spans
            .last()
            .is_none_or(|span: &ColumnSpan| span.key != entry.key)
        {
            packed.spans.push(ColumnSpan {
                key: entry.key,
                start: u32::try_from(packed.blocks.len())
                    .map_err(|_| "bigram descriptor overflow")?,
                len: 0,
            });
        }
        let span = packed.spans.last_mut().unwrap();
        span.len = span
            .len
            .checked_add(1)
            .ok_or("bigram descriptor overflow")?;
        packed.blocks.push(entry.block);
    }
    if let Some(work) = contribution.work.as_mut() {
        work.pack_ns = work
            .pack_ns
            .saturating_add(diagnostic_elapsed_ns(pack_started));
    }
    Ok(packed)
}

struct BasedColumns {
    base: u32,
    columns: PackedColumns,
}

fn split_chunks(chunks: Vec<PackedChunk>) -> (Vec<BasedColumns>, Vec<BasedColumns>) {
    let mut normal = Vec::with_capacity(chunks.len());
    let mut skip = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        normal.push(BasedColumns {
            base: chunk.base,
            columns: chunk.normal,
        });
        skip.push(BasedColumns {
            base: chunk.base,
            columns: chunk.skip,
        });
    }
    (normal, skip)
}

fn convert_blocks(chunks: Vec<BasedColumns>) -> Result<HashMap<u64, RoaringBitmap>, String> {
    let mut result = HashMap::new();
    for chunk in chunks {
        for span in &chunk.columns.spans {
            let start = span.start as usize;
            let end = start + span.len as usize;
            let slots = chunk.columns.blocks[start..end].iter().flat_map(|block| {
                let mut bits = block.bits;
                std::iter::from_fn(move || {
                    if bits == 0 {
                        return None;
                    }
                    let offset = bits.trailing_zeros();
                    bits &= bits - 1;
                    Some(chunk.base + block.first_slot + offset)
                })
            });
            result
                .entry(span.key)
                .or_insert_with(RoaringBitmap::new)
                .append(slots)
                .map_err(|_| "bigram blocks are not ordered")?;
        }
    }
    Ok(result)
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

    /// Diagnostic-only path. One lookup is counted per emitted gram; an OR
    /// is duplicate when that record's bit was already present in its word.
    fn set_profiled(&mut self, key: u64, bit: u64, lookups: &mut u64, duplicates: &mut u64) {
        *lookups = (*lookups).saturating_add(1);
        match self {
            Self::Ascii { words, touched } => {
                if words.is_empty() {
                    words.resize(95 * 95, 0);
                }
                let left = ((key >> 8) as u8 - 32) as usize;
                let right = (key as u8 - 32) as usize;
                let index = left * 95 + right;
                let word = &mut words[index];
                if *word == 0 {
                    touched.push(index);
                }
                if *word & bit != 0 {
                    *duplicates = (*duplicates).saturating_add(1);
                }
                *word |= bit;
            }
            Self::Unicode(words) => {
                let word = words.entry(key).or_default();
                if *word & bit != 0 {
                    *duplicates = (*duplicates).saturating_add(1);
                }
                *word |= bit;
            }
        }
    }

    fn capacity_bytes_lower_bound(&self) -> usize {
        match self {
            Self::Ascii { words, touched } => {
                words.capacity() * std::mem::size_of::<u64>()
                    + touched.capacity() * std::mem::size_of::<usize>()
            }
            Self::Unicode(words) => words.capacity() * std::mem::size_of::<(u64, u64)>(),
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
}

/// Construction-only state; completed indexes retain the existing format.
/// The builder can start at any final slot and publishes each touched gram
/// once per chunk, with record bits mapped to final slots at publication.
pub(crate) struct BigramBuilder {
    contribution: BigramContribution,
    normal: GramWords,
    skip: GramWords,
    chunk_start: u32,
    chunk_len: u32,
    storage_diagnostic: bool,
}

impl BigramBuilder {
    pub fn new(alphabet: BigramAlphabet) -> Self {
        Self::new_at(alphabet, 0)
    }

    pub fn new_at(alphabet: BigramAlphabet, first_slot: u32) -> Self {
        let mut contribution = BigramContribution::new(alphabet);
        contribution.record_count = first_slot;
        Self {
            contribution,
            normal: GramWords::new(alphabet),
            skip: GramWords::new(alphabet),
            chunk_start: 0,
            chunk_len: 0,
            storage_diagnostic: storage_diagnostic_enabled(),
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
        let (normal, skip) = (&mut self.normal, &mut self.skip);
        if self.storage_diagnostic {
            let started = crate::detailed_timer::DetailedInstant::now();
            let (mut lookups, mut duplicates) = (0, 0);
            for_each_segment_gram(segment, alphabet, |is_skip, key| {
                if is_skip {
                    skip.set_profiled(key, bit, &mut lookups, &mut duplicates);
                } else {
                    normal.set_profiled(key, bit, &mut lookups, &mut duplicates);
                }
            });
            if let Some(work) = self.contribution.work.as_mut() {
                work.gram_set_lookups = work.gram_set_lookups.saturating_add(lookups);
                work.duplicate_bit_ors = work.duplicate_bit_ors.saturating_add(duplicates);
                work.add_segment_ns = work
                    .add_segment_ns
                    .saturating_add(started.elapsed().as_nanos() as u64);
            }
        } else {
            for_each_segment_gram(segment, alphabet, |is_skip, key| {
                if is_skip {
                    skip.set(key, bit);
                } else {
                    normal.set(key, bit);
                }
            });
        }
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
        let diagnostic = self.storage_diagnostic;
        if diagnostic {
            self.contribution.worker_scratch_peak_capacity_bytes =
                self.contribution.worker_scratch_peak_capacity_bytes.max(
                    self.normal.capacity_bytes_lower_bound()
                        + self.skip.capacity_bytes_lower_bound(),
                );
        }
        let drain_started = crate::detailed_timer::start_if_enabled(diagnostic);
        self.normal.drain(|key, word| {
            self.contribution.pending_normal.push(PendingBlock {
                key,
                block: GramBlock {
                    first_slot: start,
                    bits: word,
                },
            });
        });
        self.skip.drain(|key, word| {
            self.contribution.pending_skip.push(PendingBlock {
                key,
                block: GramBlock {
                    first_slot: start,
                    bits: word,
                },
            });
        });
        if let Some(work) = self.contribution.work.as_mut() {
            work.scratch_drain_ns = work
                .scratch_drain_ns
                .saturating_add(diagnostic_elapsed_ns(drain_started));
        }
        if diagnostic {
            self.contribution.worker_scratch_after_flush_capacity_bytes =
                self.normal.capacity_bytes_lower_bound() + self.skip.capacity_bytes_lower_bound();
        }
        self.chunk_len = 0;
        Ok(())
    }

    pub fn finish_blocks(mut self) -> Result<BigramContribution, String> {
        if self.chunk_len != 0 {
            self.flush()?;
        }
        if self.storage_diagnostic {
            self.contribution.pending_peak_capacity_bytes =
                (self.contribution.pending_normal.capacity()
                    + self.contribution.pending_skip.capacity())
                    * std::mem::size_of::<PendingBlock>();
        }
        let pending_normal = std::mem::take(&mut self.contribution.pending_normal);
        let pending_skip = std::mem::take(&mut self.contribution.pending_skip);
        let normal = pack_columns(pending_normal, &mut self.contribution)?;
        let skip = pack_columns(pending_skip, &mut self.contribution)?;
        if !normal.blocks.is_empty() || !skip.blocks.is_empty() {
            if self.storage_diagnostic {
                self.contribution.copied_block_bytes = ((normal.blocks.len() + skip.blocks.len())
                    * std::mem::size_of::<GramBlock>())
                    as u64;
            }
            self.contribution.chunks.push(PackedChunk {
                base: 0,
                count: self.contribution.record_count,
                normal,
                skip,
            });
        }
        let validation_started = crate::detailed_timer::start_if_enabled(self.storage_diagnostic);
        validate_chunks(&self.contribution.chunks, self.contribution.record_count)?;
        if let Some(work) = self.contribution.work.as_mut() {
            work.validation_ns = work
                .validation_ns
                .saturating_add(diagnostic_elapsed_ns(validation_started));
        }
        Ok(self.contribution)
    }

    #[cfg(test)]
    pub fn finish(self) -> Result<BigramIndex, String> {
        self.finish_blocks()?.into_index()
    }
}

#[cfg(test)]
#[path = "bigram.test.rs"]
mod tests;
