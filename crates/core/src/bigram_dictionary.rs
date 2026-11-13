//! Separate normal and skip-1 gram dictionaries over shared portable postings.

use std::sync::Arc;

use roaring::RoaringBitmap;

use crate::bigram::{BigramIndex, BigramRequest};
use crate::posting_dictionary::{
    DictionarySpan, PostingSink, SealedPostingDictionary, append_visited_constructed,
};
#[cfg(test)]
use crate::posting_dictionary::PostingPack;
use crate::segment_format::{PostingDescriptor, SectionKind, SegmentReader};
use crate::workload::BigramAlphabet;

#[derive(Clone, Copy, Debug)]
pub(crate) struct BigramSpans {
    pub normal: DictionarySpan,
    pub skip: DictionarySpan,
}

/// Encode one text binding's two gram families without cloning their bitmaps.
pub(crate) fn append_bigram_index<P: PostingSink + ?Sized>(
    section: &mut Vec<u8>,
    pack: &mut P,
    index: &BigramIndex,
    allowed: &RoaringBitmap,
) -> Result<BigramSpans, String> {
    index.validate(index.record_count as usize)?;
    index.validate_domain(allowed)?;
    let normal = append_visited_constructed(section, pack, allowed, |append| {
        index.for_each_normal(|key, posting| append(key, posting))
    })?;
    let skip = append_visited_constructed(section, pack, allowed, |append| {
        index.for_each_skip(|key, posting| append(key, posting))
    })?;
    Ok(BigramSpans { normal, skip })
}

/// The finalized builder already checked slot bounds and the eligible domain.
pub(crate) fn append_bigram_index_constructed<P: PostingSink + ?Sized>(
    section: &mut Vec<u8>,
    pack: &mut P,
    index: &BigramIndex,
    allowed: &RoaringBitmap,
) -> Result<BigramSpans, String> {
    let normal = append_visited_constructed(section, pack, allowed, |append| {
        index.for_each_normal(|key, posting| append(key, posting))
    })?;
    let skip = append_visited_constructed(section, pack, allowed, |append| {
        index.for_each_skip(|key, posting| append(key, posting))
    })?;
    Ok(BigramSpans { normal, skip })
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
struct GramPresence {
    words: Vec<u64>,
    mask: u64,
}

impl GramPresence {
    fn from_dictionary(dictionary: &SealedPostingDictionary<u64>) -> Result<Option<Self>, String> {
        if dictionary.len() < 32 {
            return Ok(None);
        }
        let bits = dictionary.len().saturating_mul(12)
            .max(64).min(1 << 26).next_power_of_two();
        let mut words = Vec::new();
        words.try_reserve_exact(bits / 64)
            .map_err(|error| format!("gram presence allocation: {error}"))?;
        words.resize(bits / 64, 0);
        let mut presence = Self { words, mask: (bits - 1) as u64 };
        dictionary.visit_encoded_entries(|encoded, _| {
            let key = u64::from_le_bytes(encoded.try_into()
                .map_err(|_| "validated gram key length changed")?);
            presence.insert(key);
            Ok(())
        })?;
        Ok(Some(presence))
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

    fn insert(&mut self, key: u64) {
        for bit in self.hashes(key) {
            self.words[bit / 64] |= 1u64 << (bit % 64);
        }
    }

    fn might_contain(&self, key: u64) -> bool {
        self.hashes(key).into_iter().all(|bit| self.words[bit / 64] & (1u64 << (bit % 64)) != 0)
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
        alphabet: BigramAlphabet,
        record_count: u32,
        allowed: Arc<RoaringBitmap>,
    ) -> Result<Self, String> {
        if allowed.max().is_some_and(|slot| slot >= record_count) {
            return Err("bigram eligible domain exceeds record slots".into());
        }
        let normal = SealedPostingDictionary::open(
            Arc::clone(&reader),
            SectionKind::BigramDictionaries,
            spans.normal,
            Arc::clone(&allowed),
        )?;
        let skip = SealedPostingDictionary::open(
            reader,
            SectionKind::BigramDictionaries,
            spans.skip,
            allowed,
        )?;
        let normal_presence = GramPresence::from_dictionary(&normal)?;
        let skip_presence = GramPresence::from_dictionary(&skip)?;
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
        alphabet: BigramAlphabet,
        record_count: u32,
        allowed: Arc<RoaringBitmap>,
    ) -> Result<Self, String> {
        let normal = SealedPostingDictionary::from_constructed(
            Arc::clone(&reader),
            SectionKind::BigramDictionaries,
            spans.normal,
            Arc::clone(&allowed),
        )?;
        let skip = SealedPostingDictionary::from_constructed(
            reader,
            SectionKind::BigramDictionaries,
            spans.skip,
            allowed,
        )?;
        let normal_presence = GramPresence::from_dictionary(&normal)?;
        let skip_presence = GramPresence::from_dictionary(&skip)?;
        Ok(Self { alphabet, record_count, normal, skip, normal_presence, skip_presence })
    }

    pub(crate) fn record_count(&self) -> u32 {
        self.record_count
    }

    /// Descriptor cardinalities are planner estimates until a posting is
    /// checked; a missing gram is a checked zero under complete coverage.
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
            let presence = if skip { &self.skip_presence } else { &self.normal_presence };
            if presence.as_ref().is_some_and(|present| !present.might_contain(key)) {
                return Ok(Some(RoaringBitmap::new()));
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
            let posting = if let Some(candidates) = &result {
                dictionary.intersect_descriptor(descriptor, candidates)?
            } else {
                dictionary.materialize_descriptor(descriptor)?
            };
            if let Some(candidates) = &mut result {
                *candidates = posting;
            } else {
                result = Some(posting);
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
