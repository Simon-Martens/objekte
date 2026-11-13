//! Typed ordered dictionaries over independently checked portable postings.

use std::collections::BTreeMap;
use std::ops::Bound;
use std::sync::Arc;

use roaring::RoaringBitmap;
use serde::{Serialize, de::DeserializeOwned};

use crate::ordered_block::{OrderedBlockDictionary, write_ordered_dictionary};
use crate::segment_format::{PostingDescriptor, SectionKind, SegmentReader};

/// A span in one immutable segment's scalar or array-length section.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DictionarySpan {
    pub start: usize,
    pub length: usize,
}

/// Pack ordinary portable Roaring payloads into a common byte region. A
/// descriptor has independent integrity and slot-domain checks on access.
pub(crate) struct PostingPack {
    payloads: Vec<u8>,
}

impl PostingPack {
    pub(crate) fn new() -> Self {
        Self {
            payloads: Vec::new(),
        }
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        &self.payloads
    }

    pub(crate) fn append(&mut self, bitmap: &RoaringBitmap) -> Result<PostingDescriptor, String> {
        if bitmap.is_empty() {
            return Ok(PostingDescriptor::EMPTY);
        }
        let offset = self.payloads.len() as u64;
        let size = bitmap.serialized_size();
        let end = self
            .payloads
            .len()
            .checked_add(size)
            .ok_or("posting pack size overflow")?;
        self.payloads
            .try_reserve(size)
            .map_err(|error| format!("posting pack allocation: {error}"))?;
        bitmap
            .serialize_into(&mut self.payloads)
            .map_err(|error| format!("portable posting encode: {error}"))?;
        if self.payloads.len() != end {
            return Err("portable posting size changed during encoding".into());
        }
        PostingDescriptor::for_bitmap(offset, bitmap, &self.payloads[offset as usize..end])
    }
}

/// Append one sorted typed dictionary to a shared section. The map is a
/// mutable-builder input only; immutable query execution uses the returned
/// span and the segment's owned or mapped bytes.
pub(crate) fn append_dictionary<K: Ord + Clone + Serialize>(
    section: &mut Vec<u8>,
    pack: &mut PostingPack,
    map: &BTreeMap<K, RoaringBitmap>,
) -> Result<DictionarySpan, String> {
    let start = section.len();
    let entries = map
        .iter()
        .map(|(key, posting)| Ok((key.clone(), pack.append(posting)?)))
        .collect::<Result<Vec<_>, String>>()?;
    write_ordered_dictionary(section, entries)?;
    Ok(DictionarySpan {
        start,
        length: section.len() - start,
    })
}

/// The same query path serves memory-owned and file-mapped segment bytes.
/// The caller supplies an already checked eligible record-slot domain.
pub(crate) struct SealedPostingDictionary<K> {
    reader: Arc<SegmentReader>,
    dictionary: OrderedBlockDictionary<K>,
    allowed: Arc<RoaringBitmap>,
}

impl<K: Ord + Clone + DeserializeOwned> SealedPostingDictionary<K> {
    pub(crate) fn open(
        reader: Arc<SegmentReader>,
        section: SectionKind,
        span: DictionarySpan,
        allowed: Arc<RoaringBitmap>,
    ) -> Result<Self, String> {
        let dictionary =
            OrderedBlockDictionary::open(Arc::clone(&reader), section, span.start, span.length)?;
        Ok(Self {
            reader,
            dictionary,
            allowed,
        })
    }

    pub(crate) fn len(&self) -> usize {
        self.dictionary.len()
    }

    pub(crate) fn estimated_cardinality(&self, key: &K) -> Result<Option<u32>, String> {
        Ok(self
            .dictionary
            .get(key)?
            .map(|descriptor| descriptor.cardinality))
    }

    /// `None` is a checked absent key; a present empty descriptor returns an
    /// empty bitmap. Coverage metadata must decide whether absence is exact.
    pub(crate) fn get(&self, key: &K) -> Result<Option<RoaringBitmap>, String> {
        self.dictionary
            .get(key)?
            .map(|descriptor| self.reader.materialize_posting(descriptor, &self.allowed))
            .transpose()
    }

    pub(crate) fn get_restricted(
        &self,
        key: &K,
        input: &RoaringBitmap,
    ) -> Result<Option<RoaringBitmap>, String> {
        self.get(key)
            .map(|found| found.map(|posting| input & &posting))
    }

    /// Seek once, then union postings in key order within the typed bounds.
    /// A restricted query intersects each posting before adding it to output.
    pub(crate) fn union_bounds(
        &self,
        lower: Bound<&K>,
        upper: Bound<&K>,
        input: Option<&RoaringBitmap>,
    ) -> Result<RoaringBitmap, String> {
        let mut cursor = match &lower {
            Bound::Unbounded => self.dictionary.first(),
            Bound::Included(key) | Bound::Excluded(key) => self.dictionary.lower_bound(*key)?,
        };
        if let (Bound::Excluded(key), Some(found)) = (&lower, cursor) {
            if &self.dictionary.entry(found)?.0 == *key {
                cursor = self.dictionary.next(found);
            }
        }
        let mut result = RoaringBitmap::new();
        while let Some(position) = cursor {
            let (key, descriptor) = self.dictionary.entry(position)?;
            let inside = match &upper {
                Bound::Unbounded => true,
                Bound::Included(end) => &key <= *end,
                Bound::Excluded(end) => &key < *end,
            };
            if !inside {
                break;
            }
            let mut posting = self.reader.materialize_posting(descriptor, &self.allowed)?;
            if let Some(input) = input {
                posting &= input;
            }
            result |= posting;
            cursor = self.dictionary.next(position);
        }
        Ok(result)
    }
}

#[cfg(test)]
#[path = "posting_dictionary.test.rs"]
mod tests;
