//! Borrowed bigram-dictionary readers over packed segments.

use roaring::RoaringBitmap;

use super::backing::Backing;
use super::bigram_builder::{
    BigramBinding, BigramBindingKind, BIGRAM_DESCRIPTOR_LEN, BIGRAM_DICTIONARY_MAGIC,
    BIGRAM_DICTIONARY_VERSION, BIGRAM_POSTING_MAGIC, BIGRAM_POSTING_VERSION,
};
use super::dictionary::DictionaryReader;
use super::error::{StorageError, StorageResult};
use super::format::SectionDescriptor;
use super::record_set::{RecordSetRef, RecordSetView};

struct BindingData {
    binding: BigramBinding,
    alphabet: u8,
    record_count: u32,
    normal: DictionaryReader,
    skip: DictionaryReader,
}

/// All bigram bindings in one frozen segment.
pub struct BigramSectionView {
    backing: Backing,
    slot_count: u32,
    arena_base: u64,
    arena_len: u64,
    bindings: Vec<BindingData>,
}

impl std::fmt::Debug for BigramSectionView {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BigramSectionView")
            .field("slot_count", &self.slot_count)
            .field("bindings", &self.bindings.len())
            .finish_non_exhaustive()
    }
}

impl BigramSectionView {
    pub fn open(
        backing: Backing,
        dictionaries: SectionDescriptor,
        postings: SectionDescriptor,
        slot_count: u32,
    ) -> StorageResult<Self> {
        let post_header = backing.read(postings.offset, 8)?;
        if post_header[..4] != BIGRAM_POSTING_MAGIC {
            return Err(StorageError::corrupt("bigram posting magic mismatch"));
        }
        let post_version = u16::from_le_bytes([post_header[4], post_header[5]]);
        if post_version != BIGRAM_POSTING_VERSION {
            return Err(StorageError::UnsupportedFormat(format!(
                "bigram posting version {post_version}"
            )));
        }
        if post_header[6..8] != [0, 0] {
            return Err(StorageError::corrupt(
                "bigram posting reserved bytes are not zero",
            ));
        }
        let header = backing.read(dictionaries.offset, 16)?;
        if header[..4] != BIGRAM_DICTIONARY_MAGIC {
            return Err(StorageError::corrupt("bigram dictionary magic mismatch"));
        }
        let version = u16::from_le_bytes([header[4], header[5]]);
        if version != BIGRAM_DICTIONARY_VERSION {
            return Err(StorageError::UnsupportedFormat(format!(
                "bigram dictionary version {version}"
            )));
        }
        if header[6..8] != [0, 0] || header[12..16] != [0, 0, 0, 0] {
            return Err(StorageError::corrupt(
                "bigram dictionary reserved bytes are not zero",
            ));
        }
        let count = u32::from_le_bytes(header[8..12].try_into().expect("bigram count")) as usize;
        let directory_len = 16usize
            .checked_add(count.checked_mul(BIGRAM_DESCRIPTOR_LEN).ok_or_else(|| {
                StorageError::corrupt("bigram dictionary directory overflow")
            })?)
            .ok_or_else(|| StorageError::corrupt("bigram dictionary directory overflow"))?;
        if directory_len as u64 > dictionaries.length {
            return Err(StorageError::corrupt(
                "bigram dictionary directory exceeds its section",
            ));
        }
        let directory = backing.read(dictionaries.offset, directory_len as u64)?;
        let mut bindings = Vec::with_capacity(count);
        for index in 0..count {
            let at = 16 + index * BIGRAM_DESCRIPTOR_LEN;
            let slot = &directory[at..at + BIGRAM_DESCRIPTOR_LEN];
            let kind = BigramBindingKind::from_code(slot[4])?;
            let alphabet = slot[5];
            if slot[6..8] != [0, 0] || slot[44..48] != [0, 0, 0, 0] {
                return Err(StorageError::corrupt(
                    "bigram descriptor reserved bytes are not zero",
                ));
            }
            let normal_offset = read_u64(slot, 8)?;
            let normal_len = read_u64(slot, 16)?;
            let skip_offset = read_u64(slot, 24)?;
            let skip_len = read_u64(slot, 32)?;
            let record_count = read_u32(slot, 40)?;
            let metadata_offset = read_u64(slot, 48)?;
            let metadata_len = read_u64(slot, 56)?;
            if record_count > slot_count {
                return Err(StorageError::corrupt(
                    "bigram binding record count exceeds the slot domain",
                ));
            }
            for (offset, length) in [
                (normal_offset, normal_len),
                (skip_offset, skip_len),
                (metadata_offset, metadata_len),
            ] {
                if offset
                    .checked_add(length)
                    .is_none_or(|end| end > dictionaries.length)
                {
                    return Err(StorageError::corrupt(
                        "bigram descriptor reference lies outside its section",
                    ));
                }
            }
            let metadata = backing.read(
                dictionaries.offset + metadata_offset,
                metadata_len,
            )?;
            let binding = decode_binding(kind, metadata)?;
            let normal = DictionaryReader::open(
                backing.clone(),
                dictionaries.offset + normal_offset,
                normal_len,
            )?;
            let skip = DictionaryReader::open(
                backing.clone(),
                dictionaries.offset + skip_offset,
                skip_len,
            )?;
            bindings.push(BindingData {
                binding,
                alphabet,
                record_count,
                normal,
                skip,
            });
        }
        Ok(Self {
            backing,
            slot_count,
            arena_base: postings.offset + 8,
            arena_len: postings.length - 8,
            bindings,
        })
    }

    pub fn slot_count(&self) -> u32 {
        self.slot_count
    }

    pub fn bindings(&self) -> impl Iterator<Item = &BigramBinding> + '_ {
        self.bindings.iter().map(|binding| &binding.binding)
    }

    pub fn binding(&self, kind: BigramBindingKind) -> Option<BigramBindingView<'_>> {
        self.bindings
            .iter()
            .find(|binding| binding.binding.kind() == kind)
            .map(|binding| self.view_for(binding))
    }

    pub fn custom(&self, path: &[String], name: &str) -> Option<BigramBindingView<'_>> {
        self.bindings
            .iter()
            .find(|binding| match &binding.binding {
                BigramBinding::Custom {
                    path: candidate_path,
                    name: candidate_name,
                } => candidate_path.as_slice() == path && candidate_name == name,
                _ => false,
            })
            .map(|binding| self.view_for(binding))
    }

    pub fn verify_full(&self) -> StorageResult<()> {
        for binding in &self.bindings {
            for dictionary in [&binding.normal, &binding.skip] {
                for entry in dictionary.range(std::ops::Bound::Unbounded, std::ops::Bound::Unbounded) {
                    let (_, bytes) = entry?;
                    let reference = RecordSetRef::decode(&bytes)?;
                    let arena = self.arena()?;
                    reference.view(arena, self.slot_count)?.verify_full()?;
                }
            }
        }
        Ok(())
    }

    fn view_for<'a>(&'a self, binding: &'a BindingData) -> BigramBindingView<'a> {
        BigramBindingView {
            section: self,
            binding,
        }
    }

    fn arena(&self) -> StorageResult<&[u8]> {
        self.backing.read(self.arena_base, self.arena_len)
    }
}

/// One text binding bound to its segment's arena.
pub struct BigramBindingView<'a> {
    section: &'a BigramSectionView,
    binding: &'a BindingData,
}

impl BigramBindingView<'_> {
    pub fn binding(&self) -> &BigramBinding {
        &self.binding.binding
    }

    pub fn alphabet(&self) -> u8 {
        self.binding.alphabet
    }

    pub fn record_count(&self) -> u32 {
        self.binding.record_count
    }

    pub fn normal_gram_count(&self) -> u64 {
        self.binding.normal.entry_count()
    }

    pub fn skip_gram_count(&self) -> u64 {
        self.binding.skip.entry_count()
    }

    pub fn normal(&self, gram: u64) -> StorageResult<Option<RecordSetView<'_>>> {
        self.lookup(&self.binding.normal, gram)
    }

    pub fn skip(&self, gram: u64) -> StorageResult<Option<RecordSetView<'_>>> {
        self.lookup(&self.binding.skip, gram)
    }

    /// Union the postings for a set of normal grams.
    pub fn union_normal(&self, grams: &[u64]) -> StorageResult<RoaringBitmap> {
        let mut result = RoaringBitmap::new();
        for gram in grams {
            if let Some(posting) = self.normal(*gram)? {
                posting.union_into(&mut result)?;
            }
        }
        Ok(result)
    }

    /// Intersect the postings for a set of normal grams. This is the candidate
    /// domain used by FTS and is deliberately query-owned scratch.
    pub fn intersect_normal(&self, grams: &[u64]) -> StorageResult<Option<RoaringBitmap>> {
        let mut result: Option<RoaringBitmap> = None;
        for gram in grams {
            let Some(posting) = self.normal(*gram)? else {
                return Ok(Some(RoaringBitmap::new()));
            };
            let mut current = RoaringBitmap::new();
            posting.union_into(&mut current)?;
            result = Some(match result {
                Some(previous) => &previous & &current,
                None => current,
            });
        }
        Ok(result)
    }

    /// Candidate intersection count without constructing a result set.
    pub fn intersection_count_normal(&self, grams: &[u64]) -> StorageResult<u64> {
        let Some(first) = grams.first() else {
            return Ok(0);
        };
        let Some(first) = self.normal(*first)? else {
            return Ok(0);
        };
        let mut count = first.cardinality();
        if count == 0 {
            return Ok(0);
        }
        for gram in &grams[1..] {
            let Some(posting) = self.normal(*gram)? else {
                return Ok(0);
            };
            count = posting.intersection_count(&first)?;
            if count == 0 {
                break;
            }
        }
        Ok(count)
    }

    /// Union the postings for a set of skip grams.
    pub fn union_skip(&self, grams: &[u64]) -> StorageResult<RoaringBitmap> {
        let mut result = RoaringBitmap::new();
        for gram in grams {
            if let Some(posting) = self.skip(*gram)? {
                posting.union_into(&mut result)?;
            }
        }
        Ok(result)
    }

    fn lookup(
        &self,
        dictionary: &DictionaryReader,
        gram: u64,
    ) -> StorageResult<Option<RecordSetView<'_>>> {
        let Some(bytes) = dictionary.get(&gram.to_le_bytes())? else {
            return Ok(None);
        };
        let reference = RecordSetRef::decode(&bytes)?;
        let arena = self.section.arena()?;
        Ok(Some(reference.view(arena, self.section.slot_count)?))
    }
}

fn decode_binding(kind: BigramBindingKind, metadata: &[u8]) -> StorageResult<BigramBinding> {
    if kind != BigramBindingKind::Custom {
        if !metadata.is_empty() {
            return Err(StorageError::corrupt(
                "built-in bigram binding has metadata",
            ));
        }
        return Ok(match kind {
            BigramBindingKind::Raw => BigramBinding::Raw,
            BigramBindingKind::Fallback => BigramBinding::Fallback,
            BigramBindingKind::Custom => unreachable!(),
        });
    }
    let mut at = 0usize;
    let path_segments = read_u32_at(metadata, &mut at)? as usize;
    let mut path = Vec::with_capacity(path_segments);
    for _ in 0..path_segments {
        let length = read_u32_at(metadata, &mut at)? as usize;
        let end = at
            .checked_add(length)
            .ok_or_else(|| StorageError::corrupt("bigram path segment overflow"))?;
        let segment = metadata
            .get(at..end)
            .ok_or_else(|| StorageError::corrupt("truncated bigram path segment"))?;
        path.push(
            std::str::from_utf8(segment)
                .map_err(|_| StorageError::corrupt("bigram path segment is not UTF-8"))?
                .to_owned(),
        );
        at = end;
    }
    let name_len = read_u32_at(metadata, &mut at)? as usize;
    let end = at
        .checked_add(name_len)
        .ok_or_else(|| StorageError::corrupt("bigram name overflow"))?;
    let name = metadata
        .get(at..end)
        .ok_or_else(|| StorageError::corrupt("truncated bigram binding name"))?;
    let name = std::str::from_utf8(name)
        .map_err(|_| StorageError::corrupt("bigram binding name is not UTF-8"))?
        .to_owned();
    if end != metadata.len() {
        return Err(StorageError::corrupt(
            "bigram binding metadata has trailing bytes",
        ));
    }
    Ok(BigramBinding::Custom { path, name })
}

fn read_u32_at(bytes: &[u8], at: &mut usize) -> StorageResult<u32> {
    let end = at
        .checked_add(4)
        .ok_or_else(|| StorageError::corrupt("bigram metadata offset overflow"))?;
    let slice = bytes
        .get(*at..end)
        .ok_or_else(|| StorageError::corrupt("truncated bigram metadata"))?;
    *at = end;
    Ok(u32::from_le_bytes(
        slice.try_into().expect("four-byte metadata field"),
    ))
}

fn read_u32(bytes: &[u8], at: usize) -> StorageResult<u32> {
    let end = at
        .checked_add(4)
        .ok_or_else(|| StorageError::corrupt("bigram u32 offset overflow"))?;
    let slice = bytes
        .get(at..end)
        .ok_or_else(|| StorageError::corrupt("truncated bigram u32"))?;
    Ok(u32::from_le_bytes(
        slice.try_into().expect("four-byte bigram field"),
    ))
}

fn read_u64(bytes: &[u8], at: usize) -> StorageResult<u64> {
    let end = at
        .checked_add(8)
        .ok_or_else(|| StorageError::corrupt("bigram u64 offset overflow"))?;
    let slice = bytes
        .get(at..end)
        .ok_or_else(|| StorageError::corrupt("truncated bigram u64"))?;
    Ok(u64::from_le_bytes(
        slice.try_into().expect("eight-byte bigram field"),
    ))
}
