use std::collections::HashMap;

use roaring::RoaringBitmap;

use super::super::IndexedCollectionSegment;
use super::routing::{PartitionRouting, SlotMap, project_bitmap_in_range};
use crate::index::posting::{CustomPostingIndex, PostingCollection, PostingIndex, StructuralFacts};
use crate::model::RecordSlot;

pub(crate) struct PostingProjection {
    pub(crate) postings: PostingCollection,
    pub(crate) custom_postings: HashMap<(Vec<String>, String), CustomPostingIndex>,
}

fn empty_projected_posting(source: &PostingIndex, logical_count: u32) -> PostingIndex {
    PostingIndex {
        values_indexed: source.values_indexed,
        path: source.path.clone(),
        kind: source.kind.clone(),
        multi: source.multi,
        presence: RoaringBitmap::new(),
        empty_selections: RoaringBitmap::new(),
        nulls: RoaringBitmap::new(),
        structural: source
            .structural
            .as_ref()
            .map(|_| StructuralFacts::default()),
        values: Default::default(),
        lengths: Default::default(),
        value_complete: source.value_complete,
        length_complete: source.length_complete,
        empty_string_complete: source.empty_string_complete,
        covered_records: logical_count,
        stats: Default::default(),
    }
}

fn scatter_posting_key<K: Ord + Clone>(
    target: &mut std::collections::BTreeMap<K, RoaringBitmap>,
    key: &K,
    slot: u32,
) {
    if let Some(bitmap) = target.get_mut(key) {
        bitmap.insert(slot);
    } else {
        let mut bitmap = RoaringBitmap::new();
        bitmap.insert(slot);
        target.insert(key.clone(), bitmap);
    }
}

fn posting_destination(
    routing: &PartitionRouting,
    old: u32,
) -> Result<Option<(usize, RecordSlot)>, String> {
    routing
        .locations
        .get(old as usize)
        .copied()
        .ok_or_else(|| "posting slot outside source".into())
}

pub(super) fn scatter_posting_partitions(
    index: &PostingIndex,
    routing: &PartitionRouting,
) -> Result<Vec<PostingIndex>, String> {
    let mut parts = routing
        .logical_counts
        .iter()
        .copied()
        .map(|count| empty_projected_posting(index, count))
        .collect::<Vec<_>>();
    for old in index.presence.iter() {
        if let Some((part, slot)) = posting_destination(routing, old)? {
            parts[part].presence.insert(slot.0);
        }
    }
    for old in index.empty_selections.iter() {
        if let Some((part, slot)) = posting_destination(routing, old)? {
            parts[part].empty_selections.insert(slot.0);
        }
    }
    for old in index.nulls.iter() {
        if let Some((part, slot)) = posting_destination(routing, old)? {
            parts[part].nulls.insert(slot.0);
        }
    }
    if let Some(facts) = &index.structural {
        for (key, bitmap) in &facts.postings {
            for old in bitmap.iter() {
                if let Some((part, slot)) = posting_destination(routing, old)? {
                    parts[part]
                        .structural
                        .as_mut()
                        .unwrap()
                        .postings
                        .entry(*key)
                        .or_default()
                        .insert(slot.0);
                }
            }
        }
    }
    index.for_each_value(|key, bitmap| {
        for old in bitmap.iter() {
            if let Some((part, slot)) = posting_destination(routing, old)? {
                scatter_posting_key(&mut parts[part].values, key, slot.0);
            }
        }
        Ok(())
    })?;
    index.for_each_length(|key, bitmap| {
        for old in bitmap.iter() {
            if let Some((part, slot)) = posting_destination(routing, old)? {
                scatter_posting_key(&mut parts[part].lengths, key, slot.0);
            }
        }
        Ok(())
    })?;
    for part in &mut parts {
        part.finish_stats();
    }
    Ok(parts)
}

pub(crate) fn project_posting(
    index: &PostingIndex,
    slots: &SlotMap<'_>,
    logical_count: u32,
    bounds: Option<(u32, u32)>,
) -> Result<PostingIndex, String> {
    let mut projected = PostingIndex {
        values_indexed: index.values_indexed,
        path: index.path.clone(),
        kind: index.kind.clone(),
        multi: index.multi,
        presence: project_bitmap_in_range(&index.presence, slots, bounds)?,
        empty_selections: project_bitmap_in_range(&index.empty_selections, slots, bounds)?,
        nulls: project_bitmap_in_range(&index.nulls, slots, bounds)?,
        structural: index
            .structural
            .as_ref()
            .map(|_| StructuralFacts::default()),
        values: Default::default(),
        lengths: Default::default(),
        value_complete: index.value_complete,
        length_complete: index.length_complete,
        empty_string_complete: index.empty_string_complete,
        covered_records: logical_count,
        stats: Default::default(),
    };
    index.for_each_value(|key, bitmap| {
        let kept = project_bitmap_in_range(bitmap, slots, bounds)?;
        if !kept.is_empty() {
            projected.values.insert(key.clone(), kept);
        }
        Ok(())
    })?;
    index.for_each_length(|key, bitmap| {
        let kept = project_bitmap_in_range(bitmap, slots, bounds)?;
        if !kept.is_empty() {
            projected.lengths.insert(key.clone(), kept);
        }
        Ok(())
    })?;
    if let (Some(source), Some(target)) = (&index.structural, &mut projected.structural) {
        for (key, bitmap) in &source.postings {
            let kept = project_bitmap_in_range(bitmap, slots, bounds)?;
            if !kept.is_empty() {
                target.postings.insert(*key, kept);
            }
        }
    }
    projected.covered_records = logical_count;
    projected.finish_stats();
    Ok(projected)
}

impl IndexedCollectionSegment {
    pub(crate) fn project_postings_for_partitions(
        &self,
        routing: &PartitionRouting,
    ) -> Result<Vec<PostingProjection>, String> {
        let mut projected = (0..routing.selected.len())
            .map(|_| PostingProjection {
                postings: PostingCollection {
                    fields: HashMap::with_capacity(self.postings.fields.len()),
                },
                custom_postings: HashMap::with_capacity(self.custom_postings.len()),
            })
            .collect::<Vec<_>>();
        for (path, index) in &self.postings.fields {
            for (part, scattered) in projected
                .iter_mut()
                .zip(scatter_posting_partitions(index, routing)?)
            {
                part.postings.fields.insert(path.clone(), scattered);
            }
        }
        for (key, custom) in &self.custom_postings {
            for (part, scattered) in projected
                .iter_mut()
                .zip(scatter_posting_partitions(&custom.index, routing)?)
            {
                part.custom_postings.insert(
                    key.clone(),
                    CustomPostingIndex {
                        index: scattered,
                        complete: custom.complete,
                    },
                );
            }
        }
        Ok(projected)
    }
}
