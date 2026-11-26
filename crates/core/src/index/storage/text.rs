//! Text-index configuration and representation identity.

use std::collections::HashMap;
use std::num::NonZeroUsize;

use crate::model::RecordSlot;
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

pub(crate) struct GramDescriptor {
    pub alphabet: BigramAlphabet,
    pub version: u32,
    pub covered_records: u32,
}

pub(crate) type VisitGramBlock<'a> =
    dyn FnMut(GramChannel, u64, &[RecordSlot]) -> StorageResult<()> + 'a;

pub(crate) trait GramReader: std::any::Any + Send + Sync {
    fn descriptor(&self) -> GramDescriptor;
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
