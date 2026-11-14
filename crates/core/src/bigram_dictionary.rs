//! Separate normal and skip-1 gram dictionaries over shared portable postings.

use std::sync::Arc;

use roaring::RoaringBitmap;

use crate::bigram::{BigramIndex, BigramRequest};
#[cfg(test)]
use crate::posting_dictionary::PostingPack;
use crate::posting_dictionary::{
    DictionarySpan, PostingSink, SealedPostingDictionary, append_visited_constructed,
};
use crate::segment_format::{PostingDescriptor, SectionKind, SegmentReader};
use crate::workload::BigramAlphabet;


#[derive(Clone, Copy, Debug)]
pub(crate) struct BigramSpans {
    pub normal: DictionarySpan,
    pub skip: DictionarySpan,
    pub normal_filter: DictionarySpan,
    pub skip_filter: DictionarySpan,
}

/// Encode one text binding's two gram families without cloning their bitmaps.
pub(crate) fn append_bigram_index<P: PostingSink + ?Sized>(
    section: &mut Vec<u8>,
    filters: &mut Vec<u8>,
    pack: &mut P,
    index: &BigramIndex,
    allowed: &RoaringBitmap,
    binding: u32,
) -> Result<BigramSpans, String> {
    index.validate(index.record_count as usize)?;
    index.validate_domain(allowed)?;
    append_bigram_index_inner(section, filters, pack, index, allowed, binding)
}

/// The finalized builder has already checked the eligible slot domain.
pub(crate) fn append_bigram_index_constructed<P: PostingSink + ?Sized>(
    section: &mut Vec<u8>,
    filters: &mut Vec<u8>,
    pack: &mut P,
    index: &BigramIndex,
    allowed: &RoaringBitmap,
    binding: u32,
) -> Result<BigramSpans, String> {
    append_bigram_index_inner(section, filters, pack, index, allowed, binding)
}

fn append_bigram_index_inner<P: PostingSink + ?Sized>(
    section: &mut Vec<u8>,
    filters: &mut Vec<u8>,
    pack: &mut P,
    index: &BigramIndex,
    allowed: &RoaringBitmap,
    binding: u32,
) -> Result<BigramSpans, String> {
    let normal_count = index.normal_key_count();
    let mut normal_presence = GramPresence::new(normal_count)?;
    let normal = append_visited_constructed(section, pack, allowed, |append| {
        index.for_each_normal(|key, posting| {
            append(key, posting)?;
            if let Some(presence) = &mut normal_presence {
                presence.insert(*key);
            }
            Ok(())
        })
    })?;
    let normal_filter = GramPresence::append(filters, normal_presence, normal, normal_count, binding, 0)?;
    let skip_count = index.skip_key_count();
    let mut skip_presence = GramPresence::new(skip_count)?;
    let skip = append_visited_constructed(section, pack, allowed, |append| {
        index.for_each_skip(|key, posting| {
            append(key, posting)?;
            if let Some(presence) = &mut skip_presence {
                presence.insert(*key);
            }
            Ok(())
        })
    })?;
    let skip_filter = GramPresence::append(filters, skip_presence, skip, skip_count, binding, 1)?;
    Ok(BigramSpans { normal, skip, normal_filter, skip_filter })
}


pub(crate) struct SealedBigramIndex {
    alphabet: BigramAlphabet,
    record_count: u32,
    normal: SealedPostingDictionary<u64>,
    skip: SealedPostingDictionary<u64>,
    normal_presence: Option<GramPresence>,
    skip_presence: Option<GramPresence>,
}


/// A compact absence proof for immutable gram keys. False positives still
/// reach the checked dictionary; false negatives are impossible for inserted
/// keys. Small dictionaries use the ordinary lookup directly.
pub(crate) struct GramPresence {
    words: GramWords,
    mask: u64,
    key_count: usize,
}

enum GramWords {
    Owned(Vec<u64>),
    Borrowed { reader: Arc<SegmentReader>, offset: usize },
}

const FILTER_MAGIC: &[u8; 8] = b"OBJHGRM1";
const FILTER_HEADER: usize = 48;


impl GramPresence {
    fn word_count(entries: usize) -> Result<Option<usize>, String> {
        if entries > u32::MAX as usize {
            return Err("gram filter key count overflow".into());
        }
        if entries < 32 {
            return Ok(None);
        }
        let bits = entries.saturating_mul(12).max(64).min(1 << 26).next_power_of_two();
        Ok(Some(bits / 64))
    }

    fn new(entries: usize) -> Result<Option<Self>, String> {
        let Some(word_count) = Self::word_count(entries)? else {
            return Ok(None);
        };
        let mut words = Vec::new();
        words
            .try_reserve_exact(word_count)
            .map_err(|error| format!("gram presence allocation: {error}"))?;
        words.resize(word_count, 0);
        Ok(Some(Self {
            words: GramWords::Owned(words),
            mask: (word_count * 64 - 1) as u64,
            key_count: entries,
        }))
    }

    fn append(out: &mut Vec<u8>, presence: Option<Self>, dictionary: DictionarySpan, count: usize, binding: u32, family: u32) -> Result<DictionarySpan, String> {
        let start = out.len();
        let Some(presence) = presence else {
            return Ok(DictionarySpan { start, length: 0 });
        };
        let GramWords::Owned(words) = presence.words else {
            return Err("gram filter writer requires owned words".into());
        };
        out.extend_from_slice(FILTER_MAGIC);
        for value in [1u32, 1, binding, family] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        for value in [dictionary.start, dictionary.length] {
            out.extend_from_slice(&u64::try_from(value).map_err(|_| "gram dictionary span overflow")?.to_le_bytes());
        }
        out.extend_from_slice(&u32::try_from(count).map_err(|_| "gram key count overflow")?.to_le_bytes());
        out.extend_from_slice(&u32::try_from(words.len()).map_err(|_| "gram filter word count overflow")?.to_le_bytes());
        for word in words {
            out.extend_from_slice(&word.to_le_bytes());
        }
        Ok(DictionarySpan { start, length: out.len() - start })
    }

    fn open(reader: Arc<SegmentReader>, span: DictionarySpan, dictionary: DictionarySpan, binding: u32, family: u32) -> Result<Option<Self>, String> {
        let bytes = reader.section(SectionKind::GramFilters)?.ok_or("missing gram filter section")?;
        let end = span.start.checked_add(span.length).ok_or("gram filter span overflow")?;
        if end > bytes.len() {
            return Err("gram filter span outside section".into());
        }
        if span.length == 0 {
            return Ok(None);
        }
        let record = &bytes[span.start..end];
        if record.len() < FILTER_HEADER || &record[..8] != FILTER_MAGIC {
            return Err("invalid gram filter header".into());
        }
        let u32_at = |offset: usize| u32::from_le_bytes(record[offset..offset + 4].try_into().unwrap());
        let u64_at = |offset: usize| u64::from_le_bytes(record[offset..offset + 8].try_into().unwrap());
        let key_count = u32_at(40) as usize;
        let word_count = u32_at(44) as usize;
        if u32_at(8) != 1 || u32_at(12) != 1 || u32_at(16) != binding || u32_at(20) != family
            || u64_at(24) != dictionary.start as u64 || u64_at(32) != dictionary.length as u64
            || Self::word_count(key_count)? != Some(word_count)
            || FILTER_HEADER.checked_add(word_count.checked_mul(8).ok_or("gram filter length overflow")?).ok_or("gram filter length overflow")? != record.len()
        {
            return Err("gram filter binding or dimensions invalid".into());
        }
        Ok(Some(Self { words: GramWords::Borrowed { reader, offset: span.start + FILTER_HEADER }, mask: (word_count * 64 - 1) as u64, key_count }))
    }

    fn hashes(&self, key: u64) -> [usize; 3] {
        let mut value = key.wrapping_add(0x9e3779b97f4a7c15);
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
        value ^= value >> 31;
        let step = value.rotate_left(32) | 1;
        [0, 1, 2].map(|number: u64| {
            ((value.wrapping_add(number.wrapping_mul(step))) & self.mask) as usize
        })
    }

    pub(crate) fn insert(&mut self, key: u64) {
        let bits = self.hashes(key);
        let GramWords::Owned(words) = &mut self.words else { unreachable!("borrowed gram filter cannot be changed") };
        for bit in bits {
            words[bit / 64] |= 1u64 << (bit % 64);
        }
    }

    fn might_contain(&self, key: u64) -> Result<bool, String> {
        let bits = self.hashes(key);
        let borrowed = match &self.words {
            GramWords::Owned(_) => None,
            GramWords::Borrowed { reader, offset } => Some((reader.section(SectionKind::GramFilters)?.ok_or("missing gram filter section")?, *offset)),
        };
        for bit in bits {
            let word = match (&self.words, borrowed) {
                (GramWords::Owned(words), _) => words[bit / 64],
                (GramWords::Borrowed { .. }, Some((bytes, offset))) => {
                    let start = offset + (bit / 64) * 8;
                    u64::from_le_bytes(bytes[start..start + 8].try_into().unwrap())
                }
                _ => unreachable!(),
            };
            if word & (1u64 << (bit % 64)) == 0 {
                return Ok(false);
            }
        }
        Ok(true)
    }
}


impl std::fmt::Debug for SealedBigramIndex {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SealedBigramIndex")
            .field("alphabet", &self.alphabet)
            .field("record_count", &self.record_count)
            .field("normal_grams", &self.normal.len())
            .field("skip_grams", &self.skip.len())
            .finish()
    }
}


impl SealedBigramIndex {
    pub(crate) fn normal_key_count(&self) -> usize { self.normal.len() }

    pub(crate) fn skip_key_count(&self) -> usize { self.skip.len() }

    pub(crate) fn segment_reader(&self) -> Arc<SegmentReader> {
        self.normal.segment_reader()
    }

    pub(crate) fn for_each_normal(
        &self,
        mut visit: impl FnMut(&u64, &RoaringBitmap) -> Result<(), String>,
    ) -> Result<(), String> {
        self.normal.visit_entries(|key, descriptor| {
            let posting = self.normal.materialize_descriptor(descriptor)?;
            visit(key, &posting)?;
            Ok(true)
        })
    }

    pub(crate) fn for_each_skip(
        &self,
        mut visit: impl FnMut(&u64, &RoaringBitmap) -> Result<(), String>,
    ) -> Result<(), String> {
        self.skip.visit_entries(|key, descriptor| {
            let posting = self.skip.materialize_descriptor(descriptor)?;
            visit(key, &posting)?;
            Ok(true)
        })
    }

    pub(crate) fn open(
        reader: Arc<SegmentReader>,
        spans: BigramSpans,
        binding: u32,
        alphabet: BigramAlphabet,
        record_count: u32,
        allowed: Arc<RoaringBitmap>,
    ) -> Result<Self, String> {
        if allowed.max().is_some_and(|slot| slot >= record_count) {
            return Err("bigram eligible domain exceeds record slots".into());
        }
        let normal_presence = GramPresence::open(Arc::clone(&reader), spans.normal_filter, spans.normal, binding, 0)?;
        let normal = SealedPostingDictionary::open_visiting_entries(
            Arc::clone(&reader),
            SectionKind::BigramDictionaries,
            spans.normal,
            Arc::clone(&allowed),
            |entries, key| {
                if let Some(presence) = &normal_presence {
                    if presence.key_count != entries || !presence.might_contain(*key)? {
                        return Err("gram filter does not cover dictionary".into());
                    }
                }
                Ok(())
            },
        )?;
        Self::check_count(&normal_presence, normal.len())?;
        let skip_presence = GramPresence::open(Arc::clone(&reader), spans.skip_filter, spans.skip, binding, 1)?;
        let skip = SealedPostingDictionary::open_visiting_entries(
            reader,
            SectionKind::BigramDictionaries,
            spans.skip,
            allowed,
            |entries, key| {
                if let Some(presence) = &skip_presence {
                    if presence.key_count != entries || !presence.might_contain(*key)? {
                        return Err("gram filter does not cover dictionary".into());
                    }
                }
                Ok(())
            },
        )?;
        Self::check_count(&skip_presence, skip.len())?;
        Ok(Self {
            alphabet,
            record_count,
            normal,
            skip,
            normal_presence,
            skip_presence,
        })
    }

    /// A matching current store stamp proves the dictionary order and filter
    /// coverage. Posting payloads remain checked on use.
    pub(crate) fn open_stamped(
        reader: Arc<SegmentReader>,
        spans: BigramSpans,
        binding: u32,
        alphabet: BigramAlphabet,
        record_count: u32,
        allowed: Arc<RoaringBitmap>,
    ) -> Result<Self, String> {
        if allowed.max().is_some_and(|slot| slot >= record_count) {
            return Err("bigram eligible domain exceeds record slots".into());
        }
        let normal_presence = GramPresence::open(Arc::clone(&reader), spans.normal_filter, spans.normal, binding, 0)?;
        let normal = SealedPostingDictionary::open_with_proven_dictionary(
            Arc::clone(&reader),
            SectionKind::BigramDictionaries,
            spans.normal,
            Arc::clone(&allowed),
        )?;
        Self::check_count(&normal_presence, normal.len())?;
        let skip_presence = GramPresence::open(Arc::clone(&reader), spans.skip_filter, spans.skip, binding, 1)?;
        let skip = SealedPostingDictionary::open_with_proven_dictionary(
            reader,
            SectionKind::BigramDictionaries,
            spans.skip,
            allowed,
        )?;
        Self::check_count(&skip_presence, skip.len())?;
        Ok(Self {
            alphabet,
            record_count,
            normal,
            skip,
            normal_presence,
            skip_presence,
        })
    }

    pub(crate) fn from_constructed(
        reader: Arc<SegmentReader>,
        spans: BigramSpans,
        binding: u32,
        alphabet: BigramAlphabet,
        record_count: u32,
        allowed: Arc<RoaringBitmap>,
    ) -> Result<Self, String> {
        let normal_presence = GramPresence::open(Arc::clone(&reader), spans.normal_filter, spans.normal, binding, 0)?;
        let normal = SealedPostingDictionary::from_constructed(
            Arc::clone(&reader),
            SectionKind::BigramDictionaries,
            spans.normal,
            Arc::clone(&allowed),
        )?;
        Self::check_count(&normal_presence, normal.len())?;
        let skip_presence = GramPresence::open(Arc::clone(&reader), spans.skip_filter, spans.skip, binding, 1)?;
        let skip = SealedPostingDictionary::from_constructed(
            reader,
            SectionKind::BigramDictionaries,
            spans.skip,
            allowed,
        )?;
        Self::check_count(&skip_presence, skip.len())?;
        Ok(Self {
            alphabet,
            record_count,
            normal,
            skip,
            normal_presence,
            skip_presence,
        })
    }

    fn check_count(presence: &Option<GramPresence>, dictionary_count: usize) -> Result<(), String> {
        if GramPresence::word_count(dictionary_count)?.is_some() != presence.is_some()
            || presence.as_ref().is_some_and(|presence| presence.key_count != dictionary_count)
        {
            return Err("gram filter key count mismatch".into());
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn record_count(&self) -> u32 {
        self.record_count
    }

    /// Descriptor cardinalities are planner estimates until a posting is
    /// checked; a missing gram is a checked zero under complete coverage.
    #[cfg(test)]
    pub(crate) fn estimate_prepared(&self, request: &BigramRequest) -> Result<Option<u64>, String> {
        request
            .grams(self.alphabet)
            .iter()
            .map(|&(skip, key)| {
                let dictionary = if skip { &self.skip } else { &self.normal };
                Ok(dictionary.estimated_cardinality(&key)?.unwrap_or(0) as u64)
            })
            .min()
            .transpose()
    }

    /// `None` means there is no useful gram restriction; an empty bitmap
    /// means a proven absent gram in a complete text binding.
    pub(crate) fn candidates_prepared(
        &self,
        request: &BigramRequest,
    ) -> Result<Option<RoaringBitmap>, String> {
        let grams = request.grams(self.alphabet);
        if grams.is_empty() {
            return Ok(None);
        }
        let mut columns = Vec::<(bool, PostingDescriptor)>::with_capacity(grams.len());
        for &(skip, key) in grams {
            let dictionary = if skip { &self.skip } else { &self.normal };
            let presence = if skip {
                &self.skip_presence
            } else {
                &self.normal_presence
            };
            if let Some(present) = presence {
                if !present.might_contain(key)? {
                    return Ok(Some(RoaringBitmap::new()));
                }
            }
            let Some(descriptor) = dictionary.descriptor_u64(key)? else {
                return Ok(Some(RoaringBitmap::new()));
            };
            columns.push((skip, descriptor));
        }
        columns.sort_unstable_by_key(|(_, descriptor)| descriptor.cardinality);
        let mut result: Option<RoaringBitmap> = None;
        for (skip, descriptor) in columns {
            let dictionary = if skip { &self.skip } else { &self.normal };
            if let Some(candidates) = &mut result {
                dictionary.intersect_descriptor_into(descriptor, candidates)?;
            } else {
                result = Some(dictionary.materialize_descriptor(descriptor)?);
            }
            if result.as_ref().is_some_and(RoaringBitmap::is_empty) {
                break;
            }
        }
        Ok(result)
    }
}


#[cfg(test)]
#[path = "bigram_dictionary.test.rs"]
mod tests;
