//! Packed bigram dictionaries and posting arenas.

use super::dictionary::KeyOrder;
use super::dictionary_builder::build_dictionary;
use super::error::{StorageError, StorageResult};
use super::record_set::encode_record_set;
use super::record_set::{RecordSetTag, RecordSetView};

pub const BIGRAM_DICTIONARY_MAGIC: [u8; 4] = *b"OBBD";
pub const BIGRAM_DICTIONARY_VERSION: u16 = 1;
pub const BIGRAM_POSTING_MAGIC: [u8; 4] = *b"OBBP";
pub const BIGRAM_POSTING_VERSION: u16 = 1;
pub const BIGRAM_DESCRIPTOR_LEN: usize = 64;


/// Text binding kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BigramBindingKind {
    Raw = 0,
    Fallback = 1,
    Custom = 2,
}


impl BigramBindingKind {
    pub fn from_code(code: u8) -> StorageResult<Self> {
        Ok(match code {
            0 => Self::Raw,
            1 => Self::Fallback,
            2 => Self::Custom,
            other => {
                return Err(StorageError::UnsupportedFormat(format!(
                    "unknown bigram binding kind {other}"
                )));
            }
        })
    }
}


/// One text binding supplied to the segment builder.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BigramBinding {
    Raw,
    Fallback,
    Custom { path: Vec<String>, name: String },
}


impl BigramBinding {
    pub fn kind(&self) -> BigramBindingKind {
        match self {
            Self::Raw => BigramBindingKind::Raw,
            Self::Fallback => BigramBindingKind::Fallback,
            Self::Custom { .. } => BigramBindingKind::Custom,
        }
    }

    pub fn path(&self) -> &[String] {
        match self {
            Self::Custom { path, .. } => path,
            _ => &[],
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Custom { name, .. } => name,
            _ => "",
        }
    }
}


/// One binding's normal and skip gram postings.
#[derive(Clone, Debug)]
pub struct BigramInput {
    pub binding: BigramBinding,
    pub alphabet: u8,
    pub record_count: u32,
    pub normal: Vec<(u64, Vec<u32>)>,
    pub skip: Vec<(u64, Vec<u32>)>,
}


/// Packed bigram sections.
pub struct BigramSections {
    pub dictionaries: Vec<u8>,
    pub postings: Vec<u8>,
}


/// Encode every text binding into a dictionary section and one shared arena.
pub fn build_bigrams(bindings: &[BigramInput], slot_count: u32) -> StorageResult<BigramSections> {
    let mut posts = vec![0u8; 8];
    posts[..4].copy_from_slice(&BIGRAM_POSTING_MAGIC);
    posts[4..6].copy_from_slice(&BIGRAM_POSTING_VERSION.to_le_bytes());
    posts[6..8].fill(0);
    let descriptor_len = 16 + bindings.len() * BIGRAM_DESCRIPTOR_LEN;
    let mut metadata = Vec::new();
    let mut metadata_entries = Vec::with_capacity(bindings.len());
    for binding in bindings {
        let metadata_offset = descriptor_len + metadata.len();
        let mut binding_meta = Vec::new();
        if matches!(binding.binding, BigramBinding::Custom { .. }) {
            let path_segments = u32::try_from(binding.binding.path().len())
                .map_err(|_| StorageError::ResourceLimit("bigram binding path overflow".into()))?;
            binding_meta.extend_from_slice(&path_segments.to_le_bytes());
            for segment in binding.binding.path() {
                let length = u32::try_from(segment.len()).map_err(|_| {
                    StorageError::ResourceLimit("bigram path segment too large".into())
                })?;
                binding_meta.extend_from_slice(&length.to_le_bytes());
                binding_meta.extend_from_slice(segment.as_bytes());
            }
            let name = binding.binding.name();
            let name_len = u32::try_from(name.len())
                .map_err(|_| StorageError::ResourceLimit("bigram binding name too large".into()))?;
            binding_meta.extend_from_slice(&name_len.to_le_bytes());
            binding_meta.extend_from_slice(name.as_bytes());
            metadata.extend_from_slice(&binding_meta);
        }
        metadata_entries.push((metadata_offset, binding_meta.len()));
    }
    let mut arena = Vec::new();
    let mut payload = Vec::new();
    let payload_base = descriptor_len + metadata.len();
    let mut offsets = Vec::with_capacity(bindings.len());
    for binding in bindings {
        let normal = build_gram_dictionary(&binding.normal, slot_count, &mut arena)?;
        let normal_offset = payload_base + payload.len();
        payload.extend_from_slice(&normal);
        let skip = build_gram_dictionary(&binding.skip, slot_count, &mut arena)?;
        let skip_offset = payload_base + payload.len();
        payload.extend_from_slice(&skip);
        offsets.push((normal_offset, normal.len(), skip_offset, skip.len()));
    }
    let mut dictionaries = vec![0u8; descriptor_len];
    dictionaries[..4].copy_from_slice(&BIGRAM_DICTIONARY_MAGIC);
    dictionaries[4..6].copy_from_slice(&BIGRAM_DICTIONARY_VERSION.to_le_bytes());
    dictionaries[6..8].fill(0);
    dictionaries[8..12].copy_from_slice(&(bindings.len() as u32).to_le_bytes());
    dictionaries[12..16].fill(0);
    for (index, (binding, (metadata_offset, metadata_len))) in
        bindings.iter().zip(metadata_entries).enumerate()
    {
        let (normal_offset, normal_len, skip_offset, skip_len) = offsets[index];
        let at = 16 + index * BIGRAM_DESCRIPTOR_LEN;
        let slot = &mut dictionaries[at..at + BIGRAM_DESCRIPTOR_LEN];
        slot[..4].copy_from_slice(&(index as u32).to_le_bytes());
        slot[4] = binding.binding.kind() as u8;
        slot[5] = binding.alphabet;
        slot[6..8].fill(0);
        slot[8..16].copy_from_slice(&(normal_offset as u64).to_le_bytes());
        slot[16..24].copy_from_slice(&(normal_len as u64).to_le_bytes());
        slot[24..32].copy_from_slice(&(skip_offset as u64).to_le_bytes());
        slot[32..40].copy_from_slice(&(skip_len as u64).to_le_bytes());
        slot[40..44].copy_from_slice(&binding.record_count.to_le_bytes());
        slot[44..48].fill(0);
        slot[48..56].copy_from_slice(&(metadata_offset as u64).to_le_bytes());
        slot[56..64].copy_from_slice(&(metadata_len as u64).to_le_bytes());
    }
    dictionaries.extend_from_slice(&metadata);
    dictionaries.extend_from_slice(&payload);
    posts.extend_from_slice(&arena);
    Ok(BigramSections {
        dictionaries,
        postings: posts,
    })
}

fn build_gram_dictionary(
    entries: &[(u64, Vec<u32>)],
    slot_count: u32,
    arena: &mut Vec<u8>,
) -> StorageResult<Vec<u8>> {
    let mut sorted = entries.to_vec();
    sorted.sort_by_key(|(gram, _)| *gram);
    sorted.dedup_by(|left, right| left.0 == right.0);
    let mut values = Vec::with_capacity(sorted.len());
    for (gram, slots) in sorted {
        let mut encoded = encode_record_set(&slots, slot_count)?;
        if encoded.reference.tag == RecordSetTag::Blocks {
            encoded.reference.a = encoded
                .reference
                .a
                .checked_add(arena.len() as u64)
                .ok_or_else(|| {
                    StorageError::ResourceLimit("bigram arena offset overflow".into())
                })?;
        }
        arena.extend_from_slice(&encoded.arena);
        values.push((
            gram.to_le_bytes().to_vec(),
            encoded.reference.encode().to_vec(),
        ));
    }
    build_dictionary(values, KeyOrder::Unsigned, 8192)
}


/// A borrowed bigram dictionary after it has been bound to its arena.
pub struct BigramPostingView<'a> {
    reference: super::record_set::RecordSetRef,
    arena: &'a [u8],
    slot_count: u32,
}


impl<'a> BigramPostingView<'a> {
    pub fn view(&self) -> StorageResult<RecordSetView<'a>> {
        self.reference.view(self.arena, self.slot_count)
    }
}


/// A resolved bigram dictionary and its binding metadata.
pub struct BigramDictionaryView<'a> {
    pub binding: BigramBinding,
    pub alphabet: u8,
    pub record_count: u32,
    normal: &'a super::dictionary::DictionaryReader,
    skip: &'a super::dictionary::DictionaryReader,
    arena: &'a [u8],
    slot_count: u32,
}


impl<'a> BigramDictionaryView<'a> {
    pub fn normal(&self, gram: u64) -> StorageResult<Option<BigramPostingView<'a>>> {
        self.lookup(self.normal, gram)
    }

    pub fn skip(&self, gram: u64) -> StorageResult<Option<BigramPostingView<'a>>> {
        self.lookup(self.skip, gram)
    }

    pub fn normal_gram_count(&self) -> u64 {
        self.normal.entry_count()
    }

    pub fn skip_gram_count(&self) -> u64 {
        self.skip.entry_count()
    }

    fn lookup(
        &self,
        dictionary: &super::dictionary::DictionaryReader,
        gram: u64,
    ) -> StorageResult<Option<BigramPostingView<'a>>> {
        let Some(bytes) = dictionary.get(&gram.to_le_bytes())? else {
            return Ok(None);
        };
        Ok(Some(BigramPostingView {
            reference: super::record_set::RecordSetRef::decode(&bytes)?,
            arena: self.arena,
            slot_count: self.slot_count,
        }))
    }
}
