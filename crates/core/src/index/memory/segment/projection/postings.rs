use std::collections::HashMap;

use roaring::RoaringBitmap;

use super::super::IndexedCollectionSegment;
use super::routing::{PartitionRouting, SlotMap, project_bitmap_in_range};
use crate::index::memory::posting::{
    CustomPostingIndex, PostingCollection, PostingIndex, PostingStats,
};
use crate::index::storage::posting::{
    PostingDescriptor, PostingKey, PostingReader, visit_posting_blocks,
};
use crate::index::storage::transfer::StorageError;
use crate::model::RecordSlot;

pub(crate) struct PostingProjection {
    pub(crate) postings: PostingCollection,
    pub(crate) custom_postings: HashMap<(Vec<String>, String), CustomPostingIndex>,
}

fn empty_projected_posting(source: PostingDescriptor<'_>, logical_count: u32) -> PostingIndex {
    PostingIndex {
        values_indexed: source.coverage.values_indexed,
        path: source.path.to_vec(),
        kind: source.kind.clone(),
        multi: source.coverage.multi,
        presence: RoaringBitmap::new(),
        summary: source.coverage.summary,
        values: Default::default(),
        lengths: Default::default(),
        value_complete: source.coverage.value_complete,
        length_complete: source.coverage.length_complete,
        empty_string_complete: source.coverage.empty_string_complete,
        covered_records: logical_count,
        stats: PostingStats {
            legacy_present_equivalent: source.coverage.legacy_present_equivalent,
            legacy_null_equivalent: source.coverage.legacy_null_equivalent,
            ..Default::default()
        },
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
        .map(|count| empty_projected_posting(index.descriptor(), count))
        .collect::<Vec<_>>();
    for old in index.presence.iter() {
        if let Some((part, slot)) = posting_destination(routing, old)? {
            parts[part].presence.insert(slot.0);
        }
    }
    index.for_each_value_with_null(|key, bitmap| {
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

pub(crate) fn transfer_posting_partitions(
    reader: &dyn PostingReader,
    routing: &PartitionRouting,
    cancelled: &dyn Fn() -> bool,
) -> crate::index::storage::transfer::StorageResult<Vec<PostingIndex>> {
    let mut parts = routing
        .logical_counts
        .iter()
        .map(|&count| empty_projected_posting(reader.descriptor(), count))
        .collect::<Vec<_>>();
    visit_posting_blocks(
        reader,
        routing.locations.len(),
        cancelled,
        &mut |key, block| {
            for &old in block {
                let Some((part, new)) = routing.locations[old.0 as usize] else {
                    continue;
                };
                insert_posting_slot(&mut parts[part], key, new.0);
            }
            Ok(())
        },
    )?;
    for part in &mut parts {
        part.finish_stats();
    }
    Ok(parts)
}

fn insert_posting_slot(index: &mut PostingIndex, key: PostingKey<'_>, slot: u32) {
    match key {
        PostingKey::Presence => {
            index.presence.insert(slot);
        }
        PostingKey::Value(key) => scatter_posting_key(&mut index.values, key, slot),
        PostingKey::Length(length) => scatter_posting_key(&mut index.lengths, length, slot),
    }
}

pub(crate) fn transfer_posting(
    reader: &dyn PostingReader,
    slots: &SlotMap<'_>,
    covered_records: u32,
    bounds: Option<(u32, u32)>,
    cancelled: &dyn Fn() -> bool,
) -> crate::index::storage::transfer::StorageResult<PostingIndex> {
    let mut projected = empty_projected_posting(reader.descriptor(), covered_records);
    visit_posting_blocks(reader, slots.len(), cancelled, &mut |key, block| {
        for &old in block {
            if bounds.is_some_and(|(start, end)| old.0 < start || old.0 >= end) {
                continue;
            }
            let Some(new) = slots.get(old.0 as usize).ok_or_else(|| {
                StorageError::invalid("project posting blocks", "source slot outside the mapping")
            })?
            else {
                continue;
            };
            insert_posting_slot(&mut projected, key, new.0);
        }
        Ok(())
    })?;
    projected.finish_stats();
    Ok(projected)
}

pub(crate) fn project_posting(
    reader: &dyn PostingReader,
    slots: &SlotMap<'_>,
    logical_count: u32,
    bounds: Option<(u32, u32)>,
) -> Result<PostingIndex, String> {
    let Some(index) = (reader as &dyn std::any::Any).downcast_ref::<PostingIndex>() else {
        return transfer_posting(reader, slots, logical_count, bounds, &|| false)
            .map_err(String::from);
    };
    let mut projected = PostingIndex {
        values_indexed: index.values_indexed,
        path: index.path.clone(),
        kind: index.kind.clone(),
        multi: index.multi,
        presence: project_bitmap_in_range(&index.presence, slots, bounds)?,
        summary: index.summary,
        values: Default::default(),
        lengths: Default::default(),
        value_complete: index.value_complete,
        length_complete: index.length_complete,
        empty_string_complete: index.empty_string_complete,
        covered_records: logical_count,
        stats: PostingStats {
            legacy_present_equivalent: index.stats.legacy_present_equivalent,
            legacy_null_equivalent: index.stats.legacy_null_equivalent,
            ..Default::default()
        },
    };
    index.for_each_value_with_null(|key, bitmap| {
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

#[cfg(test)]
mod transfer_tests {
    use super::*;
    use crate::index::storage::posting::{
        PostingCoverage, ScalarKey, SelectionSummary, VisitPostingBlock,
    };
    use crate::index::storage::transfer::{BatchLimit, StorageResult};
    use crate::model::{LogicalType, Number, Value};
    use crate::schema::FieldDefinition;

    enum TableKey {
        Presence,
        Value(ScalarKey),
        Length(Number),
    }

    struct PostingTable {
        path: Vec<String>,
        kind: LogicalType,
        coverage: PostingCoverage,
        blocks: Vec<(TableKey, Vec<RecordSlot>)>,
        fail_after: Option<usize>,
    }

    impl PostingReader for PostingTable {
        fn descriptor(&self) -> PostingDescriptor<'_> {
            PostingDescriptor {
                path: &self.path,
                kind: &self.kind,
                coverage: self.coverage,
            }
        }

        fn visit_blocks(
            &self,
            limit: BatchLimit,
            visit: &mut VisitPostingBlock<'_>,
        ) -> StorageResult<()> {
            for (index, (key, slots)) in self.blocks.iter().enumerate() {
                if self.fail_after == Some(index) {
                    return Err(StorageError::invalid(
                        "read posting table",
                        "injected block failure",
                    ));
                }
                let key = match key {
                    TableKey::Presence => PostingKey::Presence,
                    TableKey::Value(key) => PostingKey::Value(key),
                    TableKey::Length(length) => PostingKey::Length(length),
                };
                if slots.is_empty() {
                    visit(key, slots)?;
                    continue;
                }
                let size = limit.fixed_batch_len_after(
                    "read posting table",
                    key.bytes(),
                    size_of::<RecordSlot>(),
                )?;
                for chunk in slots.chunks(17.min(size)) {
                    visit(key, chunk)?;
                }
            }
            Ok(())
        }
    }

    #[test]
    fn frequent_keys_round_trip_through_short_independent_blocks_and_remap_slots() {
        let mut index = PostingIndex::new(&FieldDefinition {
            path: vec!["a".into()],
            kind: LogicalType::String,
            required: false,
            nullable: true,
            indexed: true,
            multi: true,
            element_nullable: Vec::new(),
        });
        let value = ScalarKey::String("shared".into());
        index.covered_records = 640;
        index.presence = (0..600).collect();
        index.values.insert(value.clone(), (0..599).collect());
        index
            .values
            .insert(ScalarKey::Null, [599].into_iter().collect());
        index
            .lengths
            .insert(Number::Integer(2), [2].into_iter().collect());
        index.length_complete = true;
        index.value_complete = false;
        index.stats.legacy_present_equivalent = false;
        let mut summary = SelectionSummary::default();
        summary.observe(&Value::String("shared".into()));
        summary.note_multiple();
        index.summary = Some(summary);
        index.finish_stats();

        let descriptor = index.descriptor();
        let mut table = PostingTable {
            path: descriptor.path.to_vec(),
            kind: descriptor.kind.clone(),
            coverage: descriptor.coverage,
            blocks: Vec::new(),
            fail_after: None,
        };
        let mut frequent_blocks = 0;
        visit_posting_blocks(&index, 640, &|| false, &mut |key, slots| {
            assert!(slots.len() <= 256);
            let stored = match key {
                PostingKey::Presence => TableKey::Presence,
                PostingKey::Value(key) => {
                    frequent_blocks += usize::from(key == &value);
                    TableKey::Value(key.clone())
                }
                PostingKey::Length(length) => TableKey::Length(length.clone()),
            };
            table.blocks.push((stored, slots.to_vec()));
            Ok(())
        })
        .unwrap();
        assert_eq!(frequent_blocks, 3);

        let identity: Vec<_> = (0..640).map(|slot| Some(RecordSlot(slot))).collect();
        let restored = project_posting(&table, &SlotMap::Dense(&identity), 640, None).unwrap();
        assert_eq!(restored.presence, index.presence);
        assert_eq!(restored.values, index.values);
        assert_eq!(restored.lengths, index.lengths);
        assert_eq!(restored.summary, Some(summary));
        assert!(!restored.value_complete);
        assert!(!restored.stats.legacy_present_equivalent);

        let mut mapping = vec![None; 640];
        mapping[2] = Some(RecordSlot(1));
        mapping[599] = Some(RecordSlot(0));
        let projected = project_posting(&table, &SlotMap::Dense(&mapping), 2, None).unwrap();
        assert_eq!(projected.presence, [0, 1].into_iter().collect());
        assert_eq!(projected.values[&value], [1].into_iter().collect());
        assert_eq!(
            projected.values[&ScalarKey::Null],
            [0].into_iter().collect()
        );
        assert_eq!(
            projected.lengths[&Number::Integer(2)],
            [1].into_iter().collect()
        );
        assert!(projected.length_complete);

        table.fail_after = Some(1);
        let error = project_posting(&table, &SlotMap::Dense(&mapping), 2, None).unwrap_err();
        assert!(error.contains("injected block failure"));
    }
}
