use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use roaring::RoaringBitmap;

use super::{
    BigramAlphabet, BigramBuilder, BigramContribution, BigramIndex, for_each_segment_gram,
};
use crate::index::memory::segment::projection::SlotMap;
use crate::index::storage::text::{
    BIGRAM_VERSION, BigramRequest, GramBinding, GramChannel, GramFact, GramReader, GramStore,
    GramWriter, visit_gram_blocks,
};
use crate::index::storage::transfer::{StorageError, StorageResult};
use crate::model::RecordSlot;
use crate::repository::statistics::BigramConstructionStats;

const OP: &str = "write selected grams";
const FACT_BATCH: usize = 256;

pub(in crate::index::memory) enum GramWorker {
    Native(BigramBuilder),
    Placed {
        alphabet: BigramAlphabet,
        count: u32,
        facts: Vec<GramFact>,
    },
}

impl GramWorker {
    pub fn new(
        alphabet: BigramAlphabet,
        binding: &GramBinding,
        store: Option<&dyn GramStore>,
    ) -> Self {
        if store.is_some_and(|store| store.accepts(binding, alphabet)) {
            Self::Placed {
                alphabet,
                count: 0,
                facts: Vec::new(),
            }
        } else {
            Self::Native(BigramBuilder::new(alphabet))
        }
    }

    pub fn add_record<I, S>(&mut self, slot: RecordSlot, segments: I) -> Result<(), String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        if let Self::Native(native) = self {
            return native.add_record(slot, segments);
        }
        self.add_record_with(slot, |emit| {
            for segment in segments {
                emit(segment.as_ref());
            }
        })
    }

    pub fn add_record_with(
        &mut self,
        slot: RecordSlot,
        visit: impl FnOnce(&mut dyn FnMut(&str)),
    ) -> Result<(), String> {
        match self {
            Self::Native(native) => native.add_record_with(slot, visit),
            Self::Placed {
                alphabet,
                count,
                facts,
            } => {
                if slot.0 != *count {
                    return Err("gram slots out of order".into());
                }
                *count = count.checked_add(1).ok_or("gram slot count overflow")?;
                visit(&mut |text| {
                    for_each_segment_gram(text, *alphabet, |skip, key| {
                        facts.push(GramFact {
                            slot,
                            channel: if skip {
                                GramChannel::Skip
                            } else {
                                GramChannel::Adjacent
                            },
                            key,
                        });
                    })
                });
                Ok(())
            }
        }
    }

    pub fn add_empty(&mut self, slot: RecordSlot) -> Result<(), String> {
        if let Self::Native(native) = self {
            return native.add_empty(slot);
        }
        self.add_record_with(slot, |_| {})
    }

    pub fn finish_blocks(self) -> Result<GramBatch, String> {
        Ok(match self {
            Self::Native(native) => GramBatch::Native(native.finish_blocks()?),
            Self::Placed { count, facts, .. } => GramBatch::Placed { count, facts },
        })
    }
}

pub(in crate::index::memory) enum GramBatch {
    Native(BigramContribution),
    Placed { count: u32, facts: Vec<GramFact> },
}

impl GramBatch {
    pub fn storage_stats(&self) -> BigramConstructionStats {
        match self {
            Self::Native(native) => native.storage_stats(),
            Self::Placed { .. } => BigramConstructionStats::default(),
        }
    }
}

pub(in crate::index::memory) enum GramAggregate {
    Native(BigramContribution),
    Placed {
        writer: Box<dyn GramWriter>,
        store: Arc<dyn GramStore>,
        alphabet: BigramAlphabet,
        count: u32,
        binding: GramBinding,
    },
}

impl GramAggregate {
    pub fn new(
        alphabet: BigramAlphabet,
        binding: GramBinding,
        store: Option<&Arc<dyn GramStore>>,
    ) -> StorageResult<Self> {
        if let Some(store) = store.filter(|store| store.accepts(&binding, alphabet)) {
            Ok(Self::Placed {
                writer: store.start(&binding, alphabet)?,
                store: Arc::clone(store),
                alphabet,
                count: 0,
                binding,
            })
        } else {
            Ok(Self::Native(BigramContribution::new(alphabet)))
        }
    }

    pub fn merge_shifted(&mut self, batch: GramBatch, base: u32) -> Result<(), String> {
        match (self, batch) {
            (Self::Native(to), GramBatch::Native(from)) => to.merge_shifted(from, base),
            (
                Self::Placed { writer, count, .. },
                GramBatch::Placed {
                    count: batch_count,
                    facts,
                },
            ) => {
                if base != *count || facts.last().is_some_and(|fact| fact.slot.0 >= batch_count) {
                    return Err("gram batch slot domain mismatch".into());
                }
                for facts in facts.chunks(FACT_BATCH) {
                    writer.push(base, facts)?;
                }
                *count = count
                    .checked_add(batch_count)
                    .ok_or("gram slot count overflow")?;
                Ok(())
            }
            _ => Err("gram batch placement mismatch".into()),
        }
    }

    pub fn seal(
        self,
        pool: &rayon::ThreadPool,
        placed: &mut PlacedGrams,
    ) -> Result<BigramIndex, String> {
        match self {
            Self::Native(native) => native.into_index_in_pool(pool),
            Self::Placed {
                writer,
                store,
                alphabet,
                count,
                binding,
            } => {
                let reader = writer.seal(count)?;
                check_reader(reader.as_ref(), alphabet, count)?;
                placed.0.insert(binding, PlacedGram { reader, store });
                let mut empty = BigramIndex::new(alphabet);
                empty.record_count = count;
                Ok(empty)
            }
        }
    }
}

#[derive(Clone)]
struct PlacedGram {
    reader: Arc<dyn GramReader>,
    store: Arc<dyn GramStore>,
}

#[derive(Clone, Default)]
pub(in crate::index::memory) struct PlacedGrams(HashMap<GramBinding, PlacedGram>);

impl fmt::Debug for PlacedGrams {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_set().entries(self.0.keys()).finish()
    }
}

fn check_reader(
    reader: &dyn GramReader,
    alphabet: BigramAlphabet,
    count: u32,
) -> StorageResult<()> {
    let descriptor = reader.descriptor();
    if descriptor.alphabet != alphabet
        || descriptor.version != BIGRAM_VERSION
        || descriptor.covered_records != count
    {
        return Err(StorageError::invalid(OP, "incompatible gram reader"));
    }
    Ok(())
}

impl PlacedGrams {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn get(&self, binding: &GramBinding) -> Option<&dyn GramReader> {
        self.0.get(binding).map(|placed| placed.reader.as_ref())
    }

    pub fn is_compatible_with(&self, other: &Self) -> bool {
        self.0.len() == other.0.len()
            && self.0.iter().all(|(binding, gram)| {
                other.0.get(binding).is_some_and(|other| {
                    gram.reader.descriptor().alphabet == other.reader.descriptor().alphabet
                })
            })
    }

    pub fn consolidate(&self, shards: &[(&Self, usize, u32)], count: u32) -> StorageResult<Self> {
        let mut result = HashMap::new();
        let mut scratch = Vec::new();
        for (binding, placed) in &self.0 {
            let alphabet = placed.reader.descriptor().alphabet;
            let mut writer = placed.store.start(binding, alphabet)?;
            for &(shard, slots, base) in shards {
                let reader = shard.get(binding).ok_or_else(|| {
                    StorageError::invalid(OP, "gram placement differs between shards")
                })?;
                visit_gram_blocks(reader, slots, &|| false, &mut |channel, key, members| {
                    scratch.clear();
                    scratch.extend(members.iter().map(|&slot| GramFact { slot, channel, key }));
                    writer.push(base, &scratch)
                })?;
            }
            let reader = writer.seal(count)?;
            check_reader(reader.as_ref(), alphabet, count)?;
            result.insert(
                binding.clone(),
                PlacedGram {
                    reader,
                    store: Arc::clone(&placed.store),
                },
            );
        }
        Ok(Self(result))
    }

    pub fn project(&self, slots: &SlotMap<'_>, count: u32) -> StorageResult<Self> {
        let mut result = HashMap::new();
        let mut scratch = Vec::new();
        for (binding, placed) in &self.0 {
            let alphabet = placed.reader.descriptor().alphabet;
            let mut writer = placed.store.start(binding, alphabet)?;
            visit_gram_blocks(
                placed.reader.as_ref(),
                slots.len(),
                &|| false,
                &mut |channel, key, members| {
                    scratch.clear();
                    scratch.extend(members.iter().filter_map(|slot| {
                        slots.get(slot.0 as usize).flatten().map(|slot| GramFact {
                            slot,
                            channel,
                            key,
                        })
                    }));
                    writer.push(0, &scratch)
                },
            )?;
            let reader = writer.seal(count)?;
            check_reader(reader.as_ref(), alphabet, count)?;
            result.insert(
                binding.clone(),
                PlacedGram {
                    reader,
                    store: Arc::clone(&placed.store),
                },
            );
        }
        Ok(Self(result))
    }
}

#[derive(Clone, Copy)]
pub(crate) enum GramAccess<'a> {
    Native(&'a BigramIndex),
    Placed(&'a dyn GramReader),
}

impl GramAccess<'_> {
    pub fn alphabet(self) -> BigramAlphabet {
        match self {
            Self::Native(index) => index.alphabet,
            Self::Placed(reader) => reader.descriptor().alphabet,
        }
    }

    pub fn record_count(self) -> u32 {
        match self {
            Self::Native(index) => index.record_count,
            Self::Placed(reader) => reader.descriptor().covered_records,
        }
    }

    pub fn estimate(self, request: &BigramRequest) -> Option<u64> {
        match self {
            Self::Native(index) => index.estimate_prepared(request),
            Self::Placed(_) => {
                (!request.grams(self.alphabet()).is_empty()).then(|| u64::from(self.record_count()))
            }
        }
    }

    pub fn candidates(
        self,
        request: &BigramRequest,
        input: Option<&RoaringBitmap>,
    ) -> StorageResult<Option<RoaringBitmap>> {
        match self {
            Self::Native(index) => Ok(index.candidates_prepared_restricted(request, input)),
            Self::Placed(reader) => {
                let mut candidates = None;
                for &(skip, key) in request.grams(self.alphabet()) {
                    let channel = if skip {
                        GramChannel::Skip
                    } else {
                        GramChannel::Adjacent
                    };
                    let members = reader.members(channel, key)?;
                    let members = members.explicit_view().ok_or_else(|| {
                        StorageError::invalid("read selected grams", "unbounded gram membership")
                    })?;
                    if members
                        .last()
                        .is_some_and(|slot| slot >= self.record_count())
                    {
                        return Err(StorageError::invalid(
                            "read selected grams",
                            "slot outside gram domain",
                        ));
                    }
                    candidates = Some(match candidates.as_ref().or(input) {
                        Some(selected) => members.intersect_bitmap(selected),
                        None => members.iter().collect(),
                    });
                    if candidates.as_ref().is_some_and(RoaringBitmap::is_empty) {
                        break;
                    }
                }
                Ok(candidates)
            }
        }
    }
}
