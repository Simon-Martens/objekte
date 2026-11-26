use std::collections::HashMap;

use super::super::IndexedCollectionSegment;
use super::routing::{PartitionRouting, SlotMap, project_bitmap_in_range};
use crate::index::memory::bigram::BigramIndex;
use crate::index::traits::text::{GramChannel, GramReader, visit_gram_blocks};
use crate::index::traits::transfer::StorageError;

pub(crate) struct BigramProjection {
    pub(crate) raw: BigramIndex,
    pub(crate) fallback: BigramIndex,
    pub(crate) custom: HashMap<(Vec<String>, String), BigramIndex>,
}

pub(crate) fn transfer_bigram_partitions(
    reader: &dyn GramReader,
    routing: &PartitionRouting,
    cancelled: &dyn Fn() -> bool,
) -> crate::index::traits::transfer::StorageResult<Vec<BigramIndex>> {
    let mut parts = routing
        .selected
        .iter()
        .map(|selected| {
            let mut index = BigramIndex::new(reader.descriptor().alphabet);
            index.record_count = selected.len() as u32;
            index
        })
        .collect::<Vec<_>>();
    visit_gram_blocks(
        reader,
        routing.locations.len(),
        cancelled,
        &mut |channel, gram, block| {
            for &old in block {
                let Some((part, new)) = routing.locations[old.0 as usize] else {
                    continue;
                };
                let target = match channel {
                    GramChannel::Adjacent => &mut parts[part].normal,
                    GramChannel::Skip => &mut parts[part].skip,
                };
                target.entry(gram).or_default().insert(new.0);
            }
            Ok(())
        },
    )?;
    Ok(parts)
}

pub(crate) fn transfer_bigram(
    reader: &dyn GramReader,
    slots: &SlotMap<'_>,
    covered_records: u32,
    bounds: Option<(u32, u32)>,
    cancelled: &dyn Fn() -> bool,
) -> crate::index::traits::transfer::StorageResult<BigramIndex> {
    let mut projected = BigramIndex::new(reader.descriptor().alphabet);
    projected.record_count = covered_records;
    visit_gram_blocks(
        reader,
        slots.len(),
        cancelled,
        &mut |channel, gram, block| {
            let target = match channel {
                GramChannel::Adjacent => &mut projected.normal,
                GramChannel::Skip => &mut projected.skip,
            };
            for &old in block {
                if bounds.is_some_and(|(start, end)| old.0 < start || old.0 >= end) {
                    continue;
                }
                if let Some(new) = slots.get(old.0 as usize).ok_or_else(|| {
                    StorageError::invalid("project gram blocks", "source slot outside the mapping")
                })? {
                    target.entry(gram).or_default().insert(new.0);
                }
            }
            Ok(())
        },
    )?;
    Ok(projected)
}

pub(crate) fn project_bigram(
    reader: &dyn GramReader,
    slots: &SlotMap<'_>,
    new_count: u32,
    bounds: Option<(u32, u32)>,
) -> Result<BigramIndex, String> {
    let Some(index) = (reader as &dyn std::any::Any).downcast_ref::<BigramIndex>() else {
        return transfer_bigram(reader, slots, new_count, bounds, &|| false).map_err(String::from);
    };
    let mut projected = BigramIndex::new(index.alphabet);
    projected.record_count = new_count;
    index.for_each_normal(|key, bitmap| {
        let kept = project_bitmap_in_range(bitmap, slots, bounds)?;
        if !kept.is_empty() {
            projected.normal.insert(*key, kept);
        }
        Ok(())
    })?;
    index.for_each_skip(|key, bitmap| {
        let kept = project_bitmap_in_range(bitmap, slots, bounds)?;
        if !kept.is_empty() {
            projected.skip.insert(*key, kept);
        }
        Ok(())
    })?;
    Ok(projected)
}

pub(crate) fn scatter_bigram_partitions(
    index: &BigramIndex,
    routing: &PartitionRouting,
) -> Result<Vec<BigramIndex>, String> {
    let mut parts = routing
        .selected
        .iter()
        .map(|selected| {
            let mut part = BigramIndex::new(index.alphabet);
            part.record_count =
                u32::try_from(selected.len()).map_err(|_| "partition bigram slot overflow")?;
            Ok(part)
        })
        .collect::<Result<Vec<_>, String>>()?;
    index.for_each_normal(|key, bitmap| {
        for old in bitmap.iter() {
            if let Some((part, slot)) = routing
                .locations
                .get(old as usize)
                .ok_or("bigram posting outside source")?
            {
                parts[*part].normal.entry(*key).or_default().insert(slot.0);
            }
        }
        Ok(())
    })?;
    index.for_each_skip(|key, bitmap| {
        for old in bitmap.iter() {
            if let Some((part, slot)) = routing
                .locations
                .get(old as usize)
                .ok_or("bigram posting outside source")?
            {
                parts[*part].skip.entry(*key).or_default().insert(slot.0);
            }
        }
        Ok(())
    })?;
    Ok(parts)
}

impl IndexedCollectionSegment {
    pub(crate) fn project_bigrams_for_partitions(
        &self,
        routing: &PartitionRouting,
    ) -> Result<Vec<BigramProjection>, String> {
        let raw = scatter_bigram_partitions(&self.raw, routing)?;
        let fallback = scatter_bigram_partitions(&self.fallback_text, routing)?;
        let mut custom = (0..routing.selected.len())
            .map(|_| HashMap::with_capacity(self.custom_text.len()))
            .collect::<Vec<_>>();
        for (key, index) in &self.custom_text {
            for (destination, projected) in scatter_bigram_partitions(index, routing)?
                .into_iter()
                .enumerate()
            {
                custom[destination].insert(key.clone(), projected);
            }
        }
        Ok(raw
            .into_iter()
            .zip(fallback)
            .zip(custom)
            .map(|((raw, fallback), custom)| BigramProjection {
                raw,
                fallback,
                custom,
            })
            .collect())
    }
}

#[cfg(test)]
mod transfer_tests {
    use super::*;
    use crate::index::traits::text::{
        BIGRAM_VERSION, BigramAlphabet, GramDescriptor, VisitGramBlock,
    };
    use crate::index::traits::transfer::{BatchLimit, StorageResult};
    use crate::model::RecordSlot;

    struct GramTable {
        version: u32,
        rows: Vec<(GramChannel, u64, RecordSlot)>,
    }

    impl GramReader for GramTable {
        fn descriptor(&self) -> GramDescriptor {
            GramDescriptor {
                alphabet: BigramAlphabet::Unicode,
                version: self.version,
                covered_records: 4,
            }
        }

        fn visit_blocks(
            &self,
            limit: BatchLimit,
            visit: &mut VisitGramBlock<'_>,
        ) -> StorageResult<()> {
            limit.fixed_batch_len_after(
                "table gram",
                size_of::<u64>() + size_of::<GramChannel>(),
                size_of::<RecordSlot>(),
            )?;
            for &(channel, key, slot) in &self.rows {
                visit(channel, key, &[slot])?;
            }
            Ok(())
        }
    }

    #[test]
    fn gram_projection_keeps_both_channels_with_independent_short_blocks() {
        let mut source = BigramIndex::new(BigramAlphabet::Unicode);
        source.record_count = 4;
        source.normal.insert(8, [0, 1, 3].into_iter().collect());
        source.skip.insert(7, [1, 3].into_iter().collect());
        let mut table = GramTable {
            version: BIGRAM_VERSION,
            rows: Vec::new(),
        };
        visit_gram_blocks(&source, 4, &|| false, &mut |channel, key, slots| {
            table
                .rows
                .extend(slots.iter().map(|&slot| (channel, key, slot)));
            Ok(())
        })
        .unwrap();

        let map = [Some(RecordSlot(1)), None, None, Some(RecordSlot(0))];
        let projected = project_bigram(&table, &SlotMap::Dense(&map), 2, None).unwrap();
        assert_eq!(projected.normal[&8], [0, 1].into_iter().collect());
        assert_eq!(projected.skip[&7], [0].into_iter().collect());
        assert_eq!(projected.alphabet, BigramAlphabet::Unicode);

        table.version += 1;
        assert!(project_bigram(&table, &SlotMap::Dense(&map), 2, None).is_err());
    }
}
