//! Typed ordered dictionaries over independently checked portable postings.

use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;
use std::sync::{Arc, Mutex, OnceLock};

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
    append_entries(section, pack, map.iter())
}

pub(crate) fn append_entries<'a, K: 'a + Ord + Clone + Serialize>(
    section: &mut Vec<u8>,
    pack: &mut PostingPack,
    entries: impl IntoIterator<Item = (&'a K, &'a RoaringBitmap)>,
) -> Result<DictionarySpan, String> {
    let start = section.len();
    let entries = entries
        .into_iter()
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
    validated: Mutex<HashMap<PostingDescriptor, Arc<OnceLock<Result<(), String>>>>>,
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
            validated: Mutex::new(HashMap::new()),
        })
    }

    pub(crate) fn len(&self) -> usize {
        self.dictionary.len()
    }

    /// Verify persisted statistic bounds using checked dictionary keys and a
    /// seek to the claimed last key, without traversing every leaf.
    pub(crate) fn matches_bounds(&self, min: Option<&K>, max: Option<&K>) -> Result<bool, String> {
        let Some(first) = self.dictionary.first() else {
            return Ok(min.is_none() && max.is_none());
        };
        let (first_key, _) = self.dictionary.entry(first)?;
        if min != Some(&first_key) {
            return Ok(false);
        }
        let Some(max) = max else {
            return Ok(false);
        };
        let Some(last) = self.dictionary.lower_bound(max)? else {
            return Ok(false);
        };
        let (last_key, _) = self.dictionary.entry(last)?;
        Ok(&last_key == max && self.dictionary.next(last).is_none())
    }

    /// Materialize a mutable map for bounded compaction. Query paths use the
    /// checked point and range methods without retaining this map.
    pub(crate) fn materialize_all(&self) -> Result<BTreeMap<K, RoaringBitmap>, String> {
        let mut result = BTreeMap::new();
        let mut cursor = self.dictionary.first();
        while let Some(position) = cursor {
            let (key, descriptor) = self.dictionary.entry(position)?;
            if result
                .insert(key, self.materialize_descriptor(descriptor)?)
                .is_some()
            {
                return Err("sealed dictionary contains a duplicate key".into());
            }
            cursor = self.dictionary.next(position);
        }
        Ok(result)
    }

    /// Visit keys and descriptors without decoding posting payloads. The
    /// callback returns false to stop early. A negative lookup is backed by
    /// the ordered dictionary's checked directory/block evidence.
    pub(crate) fn visit_entries(
        &self,
        mut visit: impl FnMut(&K, PostingDescriptor) -> Result<bool, String>,
    ) -> Result<(), String> {
        let mut cursor = self.dictionary.first();
        while let Some(position) = cursor {
            let (key, descriptor) = self.dictionary.entry(position)?;
            if !visit(&key, descriptor)? {
                break;
            }
            cursor = self.dictionary.next(position);
        }
        Ok(())
    }

    /// Seek to a typed lower bound, then visit only the requested key range.
    /// A visitor may stop when a logical type boundary has been crossed.
    pub(crate) fn visit_bounds(
        &self,
        lower: Bound<&K>,
        upper: Bound<&K>,
        mut visit: impl FnMut(&K, PostingDescriptor) -> Result<bool, String>,
    ) -> Result<(), String> {
        let mut cursor = match &lower {
            Bound::Unbounded => self.dictionary.first(),
            Bound::Included(key) | Bound::Excluded(key) => self.dictionary.lower_bound(*key)?,
        };
        if let (Bound::Excluded(key), Some(found)) = (&lower, cursor) {
            if &self.dictionary.entry(found)?.0 == *key {
                cursor = self.dictionary.next(found);
            }
        }
        while let Some(position) = cursor {
            let (key, descriptor) = self.dictionary.entry(position)?;
            let inside = match &upper {
                Bound::Unbounded => true,
                Bound::Included(end) => &key <= *end,
                Bound::Excluded(end) => &key < *end,
            };
            if !inside || !visit(&key, descriptor)? {
                break;
            }
            cursor = self.dictionary.next(position);
        }
        Ok(())
    }

    pub(crate) fn estimated_cardinality(&self, key: &K) -> Result<Option<u32>, String> {
        Ok(self
            .dictionary
            .get(key)?
            .map(|descriptor| descriptor.cardinality))
    }

    pub(crate) fn descriptor(&self, key: &K) -> Result<Option<PostingDescriptor>, String> {
        self.dictionary.get(key)
    }

    pub(crate) fn materialize_descriptor(
        &self,
        descriptor: PostingDescriptor,
    ) -> Result<RoaringBitmap, String> {
        self.with_validation(descriptor, |first| match first {
            Some(bitmap) => Ok(bitmap),
            None => self.reader.materialize_validated_posting(descriptor),
        })
    }

    pub(crate) fn intersect_descriptor(
        &self,
        descriptor: PostingDescriptor,
        input: &RoaringBitmap,
    ) -> Result<RoaringBitmap, String> {
        self.with_validation(descriptor, |first| match first {
            Some(bitmap) => Ok(input & &bitmap),
            None => self.reader.intersect_validated_posting(descriptor, input),
        })
    }

    fn with_validation<T>(
        &self,
        descriptor: PostingDescriptor,
        use_posting: impl FnOnce(Option<RoaringBitmap>) -> Result<T, String>,
    ) -> Result<T, String> {
        let cell = self
            .validated
            .lock()
            .map_err(|_| "posting validation lock poisoned")?
            .entry(descriptor)
            .or_insert_with(|| Arc::new(OnceLock::new()))
            .clone();
        let mut first = None;
        let proof = cell.get_or_init(|| {
            let bitmap = self.reader.materialize_posting(descriptor, &self.allowed)?;
            first = Some(bitmap);
            Ok(())
        });
        proof.as_ref().map_err(Clone::clone)?;
        use_posting(first)
    }

    /// `None` is a checked absent key; a present empty descriptor returns an
    /// empty bitmap. Coverage metadata must decide whether absence is exact.
    pub(crate) fn get(&self, key: &K) -> Result<Option<RoaringBitmap>, String> {
        self.dictionary
            .get(key)?
            .map(|descriptor| self.materialize_descriptor(descriptor))
            .transpose()
    }

    pub(crate) fn get_restricted(
        &self,
        key: &K,
        input: &RoaringBitmap,
    ) -> Result<Option<RoaringBitmap>, String> {
        self.dictionary
            .get(key)?
            .map(|descriptor| self.intersect_descriptor(descriptor, input))
            .transpose()
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
            let posting = if let Some(input) = input {
                self.intersect_descriptor(descriptor, input)?
            } else {
                self.materialize_descriptor(descriptor)?
            };
            result |= posting;
            cursor = self.dictionary.next(position);
        }
        Ok(result)
    }
}

#[cfg(test)]
#[path = "posting_dictionary.test.rs"]
mod tests;
