//! Typed ordered dictionaries over independently checked portable postings.

use std::collections::BTreeMap;
use std::io::Write;
use std::ops::Bound;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use roaring::RoaringBitmap;
use serde::Serialize;

use crate::ordered_block::{
    DictionaryCursor, EncodedKey, OrderedBlockDictionary, OrderedBlockWriter,
    write_ordered_dictionary,
};
use crate::segment_format::{
    MIN_CACHED_POSTING_BYTES, PostingDescriptor, PostingProof, SectionKind, SegmentReader,
};


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

    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.payloads
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


pub(crate) trait PostingSink {
    fn append(&mut self, bitmap: &RoaringBitmap) -> Result<PostingDescriptor, String>;
}


impl PostingSink for PostingPack {
    fn append(&mut self, bitmap: &RoaringBitmap) -> Result<PostingDescriptor, String> {
        PostingPack::append(self, bitmap)
    }
}


/// Keep only one reusable payload scratch while writing the immutable posting
/// section directly into its final owned buffer or managed file.
pub(crate) struct StreamingPostingPack<'a> {
    output: &'a mut dyn Write,
    position: u64,
    scratch: Vec<u8>,
}


impl<'a> StreamingPostingPack<'a> {
    pub(crate) fn new(output: &'a mut dyn Write) -> Self {
        Self {
            output,
            position: 0,
            scratch: Vec::new(),
        }
    }
}


impl PostingSink for StreamingPostingPack<'_> {
    fn append(&mut self, bitmap: &RoaringBitmap) -> Result<PostingDescriptor, String> {
        if bitmap.is_empty() {
            return Ok(PostingDescriptor::EMPTY);
        }
        self.scratch.clear();
        let size = bitmap.serialized_size();
        self.scratch
            .try_reserve(size)
            .map_err(|error| format!("posting scratch allocation: {error}"))?;
        bitmap
            .serialize_into(&mut self.scratch)
            .map_err(|error| format!("portable posting encode: {error}"))?;
        if self.scratch.len() != size {
            return Err("portable posting size changed during encoding".into());
        }
        let descriptor = PostingDescriptor::for_bitmap(self.position, bitmap, &self.scratch)?;
        self.output
            .write_all(&self.scratch)
            .map_err(|error| error.to_string())?;
        self.position = self
            .position
            .checked_add(size as u64)
            .ok_or("posting pack size overflow")?;
        Ok(descriptor)
    }
}


/// Append one sorted typed dictionary to a shared section. The map is a
/// mutable-builder input only; immutable query execution uses the returned
/// span and the segment's owned or mapped bytes.
pub(crate) fn append_dictionary<K: Ord + Clone + Serialize, P: PostingSink + ?Sized>(
    section: &mut Vec<u8>,
    pack: &mut P,
    map: &BTreeMap<K, RoaringBitmap>,
) -> Result<DictionarySpan, String> {
    append_entries(section, pack, map.iter())
}

pub(crate) fn append_entries<'a, K: 'a + Ord + Clone + Serialize, P: PostingSink + ?Sized>(
    section: &mut Vec<u8>,
    pack: &mut P,
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

/// Re-encode a mutable or sealed dictionary while retaining only its keys and
/// descriptors. The visitor supplies one checked posting at a time.
pub(crate) fn append_visited<K: Ord + Clone + Serialize, P: PostingSink + ?Sized>(
    section: &mut Vec<u8>,
    pack: &mut P,
    allowed: &RoaringBitmap,
    visit: impl FnOnce(&mut dyn FnMut(&K, &RoaringBitmap) -> Result<(), String>) -> Result<(), String>,
) -> Result<DictionarySpan, String> {
    append_visited_inner(section, pack, allowed, true, visit)
}

/// Only for finalized builder postings whose exact eligible domain has
/// already been checked by `SegmentAdmission::into_indexed_profiled`.
pub(crate) fn append_visited_constructed<K: Ord + Clone + Serialize, P: PostingSink + ?Sized>(
    section: &mut Vec<u8>,
    pack: &mut P,
    allowed: &RoaringBitmap,
    visit: impl FnOnce(&mut dyn FnMut(&K, &RoaringBitmap) -> Result<(), String>) -> Result<(), String>,
) -> Result<DictionarySpan, String> {
    append_visited_inner(section, pack, allowed, false, visit)
}

fn append_visited_inner<K: Ord + Clone + Serialize, P: PostingSink + ?Sized>(
    section: &mut Vec<u8>,
    pack: &mut P,
    allowed: &RoaringBitmap,
    validate_domain: bool,
    visit: impl FnOnce(&mut dyn FnMut(&K, &RoaringBitmap) -> Result<(), String>) -> Result<(), String>,
) -> Result<DictionarySpan, String> {
    let start = section.len();
    let mut ordered = OrderedBlockWriter::<K, _>::new(&mut *section, validate_domain);
    visit(&mut |key, posting| {
        if validate_domain && !posting.is_subset(allowed) {
            return Err("dictionary posting outside current segment domain".into());
        }
        ordered.push(key, pack.append(posting)?)?;
        Ok(())
    })?;
    ordered.finish()?;
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
    constructed: bool,
    /// One byte per writer-ordered leaf entry. The checked offset table gives
    /// each decoded descriptor a stable ordinal without a query-time hash.
    validated_ordinals: Option<Box<[AtomicU8]>>,
}


fn validation_slots(entries: usize) -> Result<Box<[AtomicU8]>, String> {
    let mut slots = Vec::new();
    slots
        .try_reserve_exact(entries)
        .map_err(|error| format!("posting proof allocation: {error}"))?;
    slots.resize_with(entries, || AtomicU8::new(0));
    Ok(slots.into_boxed_slice())
}


pub(crate) struct SealedPostingCursor<'a, K> {
    dictionary: &'a SealedPostingDictionary<K>,
    cursor: Option<DictionaryCursor>,
    lower: Bound<K>,
    upper: Bound<K>,
    descending: bool,
}


impl<K: EncodedKey> SealedPostingCursor<'_, K> {
    pub(crate) fn next(&mut self) -> Result<Option<(K, RoaringBitmap)>, String> {
        let Some(position) = self.cursor else {
            return Ok(None);
        };
        let (key, descriptor) = self.dictionary.dictionary.entry(position)?;
        let within_lower = match &self.lower {
            Bound::Unbounded => true,
            Bound::Included(lower) => &key >= lower,
            Bound::Excluded(lower) => &key > lower,
        };
        let within_upper = match &self.upper {
            Bound::Unbounded => true,
            Bound::Included(upper) => &key <= upper,
            Bound::Excluded(upper) => &key < upper,
        };
        if !within_lower || !within_upper {
            self.cursor = None;
            return Ok(None);
        }
        self.cursor = if self.descending {
            self.dictionary.dictionary.previous(position)
        } else {
            self.dictionary.dictionary.next(position)
        };
        Ok(Some((
            key,
            self.dictionary.materialize_descriptor(descriptor)?,
        )))
    }
}


impl<K: EncodedKey> SealedPostingDictionary<K> {
    pub(crate) fn segment_reader(&self) -> Arc<SegmentReader> {
        Arc::clone(&self.reader)
    }

    pub(crate) fn cursor_range(
        &self,
        lower: Bound<K>,
        upper: Bound<K>,
        descending: bool,
    ) -> Result<SealedPostingCursor<'_, K>, String> {
        let cursor = if descending {
            match &upper {
                Bound::Unbounded => self.dictionary.last(),
                Bound::Included(key) | Bound::Excluded(key) => {
                    match self.dictionary.lower_bound(key)? {
                        Some(found) => {
                            if matches!(upper, Bound::Included(_))
                                && self.dictionary.compare_at(found, key)?.is_eq()
                            {
                                Some(found)
                            } else {
                                self.dictionary.previous(found)
                            }
                        }
                        None => self.dictionary.last(),
                    }
                }
            }
        } else {
            match &lower {
                Bound::Unbounded => self.dictionary.first(),
                Bound::Included(key) | Bound::Excluded(key) => {
                    let found = self.dictionary.lower_bound(key)?;
                    if matches!(lower, Bound::Excluded(_)) {
                        match found {
                            Some(position)
                                if self.dictionary.compare_at(position, key)?.is_eq() =>
                            {
                                self.dictionary.next(position)
                            }
                            other => other,
                        }
                    } else {
                        found
                    }
                }
            }
        };
        Ok(SealedPostingCursor {
            dictionary: self,
            cursor,
            lower,
            upper,
            descending,
        })
    }

    pub(crate) fn open(
        reader: Arc<SegmentReader>,
        section: SectionKind,
        span: DictionarySpan,
        allowed: Arc<RoaringBitmap>,
    ) -> Result<Self, String> {
        let dictionary =
            OrderedBlockDictionary::open(Arc::clone(&reader), section, span.start, span.length)?;
        let validated_ordinals = Some(validation_slots(dictionary.len())?);
        Ok(Self {
            reader,
            dictionary,
            allowed,
            constructed: false,
            validated_ordinals,
        })
    }

    /// Only a matching managed-store validator stamp may select this path.
    /// Posting payloads remain checked on access for every external segment.
    pub(crate) fn open_with_proven_dictionary(
        reader: Arc<SegmentReader>,
        section: SectionKind,
        span: DictionarySpan,
        allowed: Arc<RoaringBitmap>,
    ) -> Result<Self, String> {
        let dictionary = OrderedBlockDictionary::open_with_proven_order(
            Arc::clone(&reader),
            section,
            span.start,
            span.length,
        )?;
        let validated_ordinals = Some(validation_slots(dictionary.len())?);
        Ok(Self {
            reader,
            dictionary,
            allowed,
            constructed: false,
            validated_ordinals,
        })
    }

    pub(crate) fn open_stamped_visiting_encoded(
        reader: Arc<SegmentReader>,
        section: SectionKind,
        span: DictionarySpan,
        allowed: Arc<RoaringBitmap>,
        visit: impl FnMut(usize, &[u8]) -> Result<(), String>,
    ) -> Result<Self, String> {
        let dictionary = OrderedBlockDictionary::open_with_proven_order_visiting_encoded(
            Arc::clone(&reader),
            section,
            span.start,
            span.length,
            visit,
        )?;
        let validated_ordinals = Some(validation_slots(dictionary.len())?);
        Ok(Self {
            reader,
            dictionary,
            allowed,
            constructed: false,
            validated_ordinals,
        })
    }

    pub(crate) fn open_visiting_entries(
        reader: Arc<SegmentReader>,
        section: SectionKind,
        span: DictionarySpan,
        allowed: Arc<RoaringBitmap>,
        visit: impl FnMut(usize, &K) -> Result<(), String>,
    ) -> Result<Self, String> {
        let dictionary = OrderedBlockDictionary::open_visiting_entries(
            Arc::clone(&reader),
            section,
            span.start,
            span.length,
            visit,
        )?;
        let validated_ordinals = Some(validation_slots(dictionary.len())?);
        Ok(Self {
            reader,
            dictionary,
            allowed,
            constructed: false,
            validated_ordinals,
        })
    }

    /// The bytes and descriptors came directly from the checked in-process
    /// writer. External segments must use `open` and validate every leaf.
    pub(crate) fn from_constructed(
        reader: Arc<SegmentReader>,
        section: SectionKind,
        span: DictionarySpan,
        allowed: Arc<RoaringBitmap>,
    ) -> Result<Self, String> {
        let dictionary = OrderedBlockDictionary::from_constructed(
            Arc::clone(&reader),
            section,
            span.start,
            span.length,
        )?;
        Ok(Self {
            reader,
            dictionary,
            allowed,
            constructed: true,
            validated_ordinals: None,
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

    pub(crate) fn visit_encoded_entries(
        &self,
        visit: impl FnMut(&[u8], PostingDescriptor) -> Result<(), String>,
    ) -> Result<(), String> {
        self.dictionary.visit_encoded(visit)
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
            if self.dictionary.compare_at(found, key)?.is_eq() {
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

    fn cache_proof(&self) -> PostingProof {
        if self.constructed {
            PostingProof::Constructed
        } else {
            PostingProof::Checked
        }
    }

    fn posting_proven(&self, descriptor: PostingDescriptor) -> bool {
        self.validated_ordinals
            .as_ref()
            .and_then(|slots| slots.get(descriptor.ordinal as usize))
            .is_some_and(|slot| slot.load(Ordering::Acquire) == 1)
    }

    fn record_posting_proof(&self, descriptor: PostingDescriptor) {
        if let Some(slot) = self
            .validated_ordinals
            .as_ref()
            .and_then(|slots| slots.get(descriptor.ordinal as usize))
        {
            slot.store(1, Ordering::Release);
        }
    }

    /// The writer's private construction proof permits unchecked decoding.
    /// Every external cache miss is checked against this dictionary's exact
    /// eligible domain before it can affect a result or enter the cache.
    fn decode_operand(&self, descriptor: PostingDescriptor) -> Result<RoaringBitmap, String> {
        if self.constructed {
            self.reader.materialize_validated_posting(descriptor)
        } else if self.posting_proven(descriptor) {
            self.reader.materialize_validated_posting(descriptor)
        } else {
            let bitmap = self.reader.materialize_posting(descriptor, &self.allowed)?;
            self.record_posting_proof(descriptor);
            Ok(bitmap)
        }
    }

    fn with_operand<T>(
        &self,
        descriptor: PostingDescriptor,
        use_posting: impl FnOnce(&RoaringBitmap) -> T,
    ) -> Result<T, String> {
        let proof = self.cache_proof();
        if descriptor.length >= MIN_CACHED_POSTING_BYTES {
            if let Some(bitmap) = self.reader.cached_posting(descriptor, &self.allowed, proof) {
                return Ok(use_posting(bitmap.as_ref()));
            }
        }
        let bitmap = self.decode_operand(descriptor)?;
        if descriptor.length < MIN_CACHED_POSTING_BYTES {
            return Ok(use_posting(&bitmap));
        }
        let bitmap = self
            .reader
            .retain_posting_owned(descriptor, &self.allowed, proof, bitmap);
        Ok(use_posting(bitmap.as_ref()))
    }

    pub(crate) fn materialize_descriptor(
        &self,
        descriptor: PostingDescriptor,
    ) -> Result<RoaringBitmap, String> {
        let proof = self.cache_proof();
        if descriptor.length < MIN_CACHED_POSTING_BYTES {
            return self.decode_operand(descriptor);
        }
        if let Some(bitmap) = self.reader.cached_posting(descriptor, &self.allowed, proof) {
            return Ok((*bitmap).clone());
        }
        let bitmap = self.decode_operand(descriptor)?;
        Ok(owned_bitmap(self.reader.retain_posting_owned(
            descriptor,
            &self.allowed,
            proof,
            bitmap,
        )))
    }

    pub(crate) fn intersect_descriptor(
        &self,
        descriptor: PostingDescriptor,
        input: &RoaringBitmap,
    ) -> Result<RoaringBitmap, String> {
        if self.constructed {
            if let Some(bitmap) =
                self.reader
                    .cached_posting(descriptor, &self.allowed, PostingProof::Constructed)
            {
                return Ok(input & bitmap.as_ref());
            }
            return self.reader.intersect_validated_posting(descriptor, input);
        }
        if descriptor.length >= MIN_CACHED_POSTING_BYTES {
            if let Some(bitmap) =
                self.reader
                    .cached_posting(descriptor, &self.allowed, PostingProof::Checked)
            {
                return Ok(input & bitmap.as_ref());
            }
        }
        if self.posting_proven(descriptor) {
            return self.reader.intersect_validated_posting(descriptor, input);
        }
        self.with_operand(descriptor, |bitmap| input & bitmap)
    }

    /// Mutate only after the operand has passed its required validation.
    pub(crate) fn intersect_descriptor_into(
        &self,
        descriptor: PostingDescriptor,
        input: &mut RoaringBitmap,
    ) -> Result<(), String> {
        if self.constructed {
            if let Some(bitmap) =
                self.reader
                    .cached_posting(descriptor, &self.allowed, PostingProof::Constructed)
            {
                *input &= bitmap.as_ref();
            } else {
                let next = self.reader.intersect_validated_posting(descriptor, input)?;
                *input = next;
            }
            return Ok(());
        }
        if descriptor.length >= MIN_CACHED_POSTING_BYTES {
            if let Some(bitmap) =
                self.reader
                    .cached_posting(descriptor, &self.allowed, PostingProof::Checked)
            {
                *input &= bitmap.as_ref();
                return Ok(());
            }
        }
        if self.posting_proven(descriptor) {
            let next = self.reader.intersect_validated_posting(descriptor, input)?;
            *input = next;
            return Ok(());
        }
        self.with_operand(descriptor, |bitmap| *input &= bitmap)
    }

    /// A cache hit borrows a validated operand without cloning its bitmap.
    pub(crate) fn union_descriptor_into(
        &self,
        descriptor: PostingDescriptor,
        output: &mut RoaringBitmap,
    ) -> Result<(), String> {
        self.with_operand(descriptor, |bitmap| *output |= bitmap)
    }

    /// `None` is a checked absent key; a present empty descriptor returns an
    /// empty bitmap. Coverage metadata must decide whether absence is exact.
    pub(crate) fn get(&self, key: &K) -> Result<Option<RoaringBitmap>, String> {
        self.dictionary
            .get(key)?
            .map(|descriptor| self.materialize_descriptor(descriptor))
            .transpose()
    }

    pub(crate) fn union_key_into(
        &self,
        key: &K,
        output: &mut RoaringBitmap,
    ) -> Result<Option<u64>, String> {
        let Some(descriptor) = self.dictionary.get(key)? else {
            return Ok(None);
        };
        self.union_descriptor_into(descriptor, output)?;
        Ok(Some(descriptor.cardinality as u64))
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
            if self.dictionary.compare_at(found, key)?.is_eq() {
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
            if let Some(input) = input {
                result |= self.intersect_descriptor(descriptor, input)?;
            } else {
                self.union_descriptor_into(descriptor, &mut result)?;
            }
            cursor = self.dictionary.next(position);
        }
        Ok(result)
    }
}


fn owned_bitmap(bitmap: Arc<RoaringBitmap>) -> RoaringBitmap {
    Arc::try_unwrap(bitmap).unwrap_or_else(|shared| (*shared).clone())
}


impl SealedPostingDictionary<u64> {
    pub(crate) fn descriptor_u64(&self, key: u64) -> Result<Option<PostingDescriptor>, String> {
        self.dictionary.get_u64(key)
    }
}


#[cfg(test)]
#[path = "posting_dictionary.test.rs"]
mod tests;
