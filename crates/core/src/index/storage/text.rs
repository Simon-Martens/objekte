//! Text-index configuration and representation identity.

use std::collections::HashMap;
use std::num::NonZeroUsize;

use crate::model::RecordSlot;
use crate::model::text::fold;
use crate::schema::Schema;

use super::transfer::{BatchLimit, StorageError, StorageResult};

use serde::{Deserialize, Serialize};

pub const BIGRAM_VERSION: u32 = 3;
pub const FOLDED_REPRESENTATION: &str = "unicode-casefold-0.2.0/unicode-9.0.0/full/non-turkic";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BigramAlphabet {
    Ascii,
    Unicode,
}

/// Emit windows over the folded stream without losing unsupported ASCII byte
/// positions or joining independent reader-owned segments.
pub(crate) fn for_each_gram(
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

pub(crate) fn ascii_pair(a: u8, b: u8) -> Option<u64> {
    if !(32..=126).contains(&a) || !(32..=126).contains(&b) {
        return None;
    }
    Some(((a as u64) << 8) | b as u64)
}

pub(crate) fn unicode_pair(a: char, b: char) -> u64 {
    ((a as u64) << 21) | b as u64
}

/// Derived once when a collection is published or restored. It contains only
/// small binding names; postings and membership remain in their owners.
#[derive(Clone, Debug, Default)]
pub(crate) struct TextBindingCatalog {
    pub(crate) canonical: HashMap<Vec<String>, String>,
}

impl TextBindingCatalog {
    pub fn new(schema: &Schema) -> Self {
        let mut canonical = HashMap::new();
        for (path, name) in schema.custom.keys() {
            canonical
                .entry(path.clone())
                .and_modify(|current: &mut String| {
                    if name < current {
                        current.clone_from(name);
                    }
                })
                .or_insert_with(|| name.clone());
        }
        Self { canonical }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct TextAccessPolicy<'a> {
    pub schema: &'a Schema,
    pub bindings: &'a TextBindingCatalog,
    pub logical_text_complete: bool,
}
#[derive(Clone, Copy)]
pub(crate) struct SegmentTextAccess<'a> {
    pub policy: TextAccessPolicy<'a>,
    pub domain: crate::query_access::membership::DomainId,
    pub instance: u64,
    pub collection: crate::model::CollectionId,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum GramChannel {
    Adjacent,
    Skip,
}

impl GramChannel {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Adjacent => "normal",
            Self::Skip => "skip",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct GramDescriptor {
    pub alphabet: BigramAlphabet,
    pub version: u32,
    pub covered_records: u32,
}

pub(crate) type VisitGramBlock<'a> =
    dyn FnMut(GramChannel, u64, &[RecordSlot]) -> StorageResult<()> + 'a;

pub(crate) trait GramReader: std::any::Any + Send + Sync {
    fn descriptor(&self) -> GramDescriptor;
    fn members(
        &self,
        _channel: GramChannel,
        _key: u64,
    ) -> StorageResult<crate::query_access::membership::SlotSet<'_>> {
        Err(StorageError::invalid(
            "read gram membership",
            "point access unavailable",
        ))
    }

    fn visit_blocks(&self, limit: BatchLimit, visit: &mut VisitGramBlock<'_>) -> StorageResult<()>;
}

pub(crate) fn visit_gram_blocks(
    reader: &dyn GramReader,
    slot_count: usize,
    cancelled: &dyn Fn() -> bool,
    visit: &mut VisitGramBlock<'_>,
) -> StorageResult<()> {
    const OP: &str = "read gram blocks";
    const LIMIT: BatchLimit = BatchLimit::new(
        NonZeroUsize::new(256).unwrap(),
        NonZeroUsize::new(16 * 1024).unwrap(),
    );
    let descriptor = reader.descriptor();
    if descriptor.version != BIGRAM_VERSION || descriptor.covered_records as usize > slot_count {
        return Err(StorageError::invalid(
            OP,
            "incompatible gram representation or coverage",
        ));
    }
    if cancelled() {
        return Err(StorageError::Cancelled);
    }
    let maximum = LIMIT.fixed_batch_len_after(
        OP,
        size_of::<u64>() + size_of::<GramChannel>(),
        size_of::<RecordSlot>(),
    )?;
    let mut previous: Option<(GramChannel, u64, Option<RecordSlot>)> = None;
    reader.visit_blocks(LIMIT, &mut |channel, key, slots| {
        if cancelled() {
            return Err(StorageError::Cancelled);
        }
        let valid_boundary = match previous {
            Some((prior_channel, prior_key, last))
                if (prior_channel, prior_key) == (channel, key) =>
            {
                last.zip(slots.first().copied())
                    .is_some_and(|(last, first)| last < first)
            }
            Some((prior_channel, prior_key, _)) => (prior_channel, prior_key) < (channel, key),
            None => true,
        };
        if !valid_boundary
            || slots.len() > maximum
            || slots.windows(2).any(|pair| pair[0] >= pair[1])
            || slots
                .last()
                .is_some_and(|slot| slot.0 as usize >= slot_count)
        {
            return Err(StorageError::invalid(OP, "invalid gram key or slot block"));
        }
        previous = Some((channel, key, slots.last().copied()));
        visit(channel, key, slots)
    })
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum GramBinding {
    Raw,
    Fallback,
    Field(Vec<String>, String),
}

#[derive(Clone, Copy)]
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "consumed by the independent conformance writer")
)]
pub(crate) struct GramFact {
    pub slot: RecordSlot,
    pub channel: GramChannel,
    pub key: u64,
}

pub(crate) trait GramWriter: std::any::Any + Send {
    #[cfg(test)]
    fn observe_worker_pending(&mut self, _facts: usize, _bytes: usize) {}

    /// Adopt a prepared local writer from the same store at its assigned slot base.
    fn merge_worker(&mut self, base: u32, worker: Box<dyn GramWriter>) -> StorageResult<()>;

    fn push(&mut self, base: u32, facts: &[GramFact]) -> StorageResult<()>;
    fn seal(self: Box<Self>, slots: u32) -> StorageResult<std::sync::Arc<dyn GramReader>>;
}

pub(crate) trait GramStore: Send + Sync {
    fn accepts(&self, binding: &GramBinding, alphabet: BigramAlphabet) -> bool;
    fn start(
        &self,
        binding: &GramBinding,
        alphabet: BigramAlphabet,
    ) -> StorageResult<Box<dyn GramWriter>>;
}
