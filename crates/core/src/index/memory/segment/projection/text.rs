use std::collections::HashMap;

use super::super::IndexedCollectionSegment;
use super::routing::{PartitionRouting, SlotMap, project_bitmap_in_range};
use crate::index::bigram::BigramIndex;

pub(crate) struct BigramProjection {
    pub(crate) raw: BigramIndex,
    pub(crate) fallback: BigramIndex,
    pub(crate) custom: HashMap<(Vec<String>, String), BigramIndex>,
}

pub(crate) fn project_bigram(
    index: &BigramIndex,
    slots: &SlotMap<'_>,
    new_count: u32,
    bounds: Option<(u32, u32)>,
) -> Result<BigramIndex, String> {
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
