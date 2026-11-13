//! Separate normal and skip-1 gram dictionaries over shared portable postings.

use std::sync::Arc;

use roaring::RoaringBitmap;

use crate::bigram::{BigramIndex, BigramRequest};
use crate::posting_dictionary::{
    DictionarySpan, PostingPack, SealedPostingDictionary, append_entries,
};
use crate::segment_format::{PostingDescriptor, SectionKind, SegmentReader};
use crate::workload::BigramAlphabet;

#[derive(Clone, Copy, Debug)]
pub(crate) struct BigramSpans {
    pub normal: DictionarySpan,
    pub skip: DictionarySpan,
}

/// Encode one text binding's two gram families without cloning their bitmaps.
pub(crate) fn append_bigram_index(
    section: &mut Vec<u8>,
    pack: &mut PostingPack,
    index: &BigramIndex,
    allowed: &RoaringBitmap,
) -> Result<BigramSpans, String> {
    index.validate(index.record_count as usize)?;
    index.validate_domain(allowed)?;
    let mut normal = index.normal.iter().collect::<Vec<_>>();
    normal.sort_unstable_by_key(|(key, _)| **key);
    let normal = append_entries(section, pack, normal)?;
    let mut skip = index.skip.iter().collect::<Vec<_>>();
    skip.sort_unstable_by_key(|(key, _)| **key);
    let skip = append_entries(section, pack, skip)?;
    Ok(BigramSpans { normal, skip })
}

pub(crate) struct SealedBigramIndex {
    alphabet: BigramAlphabet,
    record_count: u32,
    normal: SealedPostingDictionary<u64>,
    skip: SealedPostingDictionary<u64>,
}

impl SealedBigramIndex {
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
        Ok(Self {
            alphabet,
            record_count,
            normal,
            skip,
        })
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
            let Some(descriptor) = dictionary.descriptor(&key)? else {
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
