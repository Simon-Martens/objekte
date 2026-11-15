//! Direct packed-to-packed compaction of one source run.
//!
//! Selected active slots are remapped into a dense new domain. Every family is
//! read from the source run's mapped readers and encoded into a fresh packed
//! segment; no source file is opened and no native posting/sort/relation owner
//! is retained after sealing.

use std::collections::BTreeMap;

use bincode::Options;
use roaring::RoaringBitmap;

use crate::posting::ScalarKey;

use super::bigram_builder::{BigramBinding, BigramBindingKind, BigramInput};
use super::error::{StorageError, StorageResult};
use super::native_pack::InferenceMetadata;
use super::posting_builder::FieldInput;
use super::records::{FilePartInput, RecordInput};
use super::relations::{RelationEdgeInput, RelationInput};
use super::scalar::decode_scalar_key;
use super::segment::FrozenSegment;
use super::segment_builder::{SegmentInput, build_segment};
use super::sort::SortColumnInput;

fn record_set<'a>(
    bytes: &[u8],
    field: &'a super::posting::FieldPostingsView,
    _source: &'a FrozenSegment,
) -> StorageResult<super::record_set::RecordSetView<'a>> {
    let reference = super::record_set::RecordSetRef::decode(bytes)?;
    field.view(reference)
}

/// Merge several packed runs into one output run without rescanning the
/// complete input per output page. Mappings are built once per selected merge.
pub(crate) fn compact_packed_merge(
    segments: &[&FrozenSegment],
    active: &[RoaringBitmap],
    collection: u32,
    cancelled: &dyn Fn() -> bool,
) -> StorageResult<Option<FrozenSegment>> {
    if segments.len() != active.len() {
        return Err(StorageError::InvalidArgument(
            "merge source and visibility counts differ".into(),
        ));
    }
    let mut inputs = Vec::with_capacity(segments.len());
    let mut count = 0u32;
    for (segment, active) in segments.iter().zip(active) {
        if active.is_empty() {
            continue;
        }
        let source_count = segment.stats().record_count;
        let mut mapping = vec![u32::MAX; source_count as usize];
        for old in active.iter() {
            if old >= source_count {
                return Err(StorageError::corrupt(
                    "merge active slot exceeds its source domain",
                ));
            }
            mapping[old as usize] = count;
            count = count
                .checked_add(1)
                .ok_or_else(|| StorageError::ResourceLimit("merged slot overflow".into()))?;
        }
        inputs.push(MergeInput {
            segment,
            active: active.clone(),
            mapping,
        });
    }
    if inputs.is_empty() {
        return Ok(None);
    }
    let logical_records =
        merge_cover_set(&inputs, |input| input.segment.coverage().logical_records())?;
    let empty_arrays = merge_cover_set(&inputs, |input| input.segment.coverage().empty_arrays())?;
    let logical_count = logical_records.len() as u32;
    let (file_parts, records) = merge_records(&inputs, count, cancelled)?;
    let fields = merge_fields(&inputs, logical_count, cancelled)?;
    let bigrams = merge_bigrams(&inputs, count, cancelled)?;
    let sorts = merge_sorts(&inputs, count, cancelled)?;
    let relations = merge_relations(&inputs, count, cancelled)?;
    let identity_order = merge_identity(&inputs, count, cancelled)?;
    let metadata = merge_metadata(&inputs)?;
    let stats = inputs[0].segment.stats();
    let input = SegmentInput {
        collection,
        source_id: stats.source_id.clone(),
        source_version: stats.source_version,
        file_parts,
        records,
        identity_order,
        fields,
        bigrams,
        sorts,
        relations,
        logical_records: bitmap_slots(&logical_records),
        empty_arrays: bitmap_slots(&empty_arrays),
        active_records: (0..count).collect(),
        metadata,
        estimated_index_bytes: inputs
            .iter()
            .map(|input| input.segment.stats().estimated_index_bytes)
            .sum(),
    };
    let built = build_segment(input)?;
    Ok(Some(FrozenSegment::from_bytes(built.object.into_bytes())?))
}

fn bitmap_slots(bitmap: &RoaringBitmap) -> Vec<u32> {
    bitmap.iter().collect()
}


struct MergeInput<'a> {
    segment: &'a FrozenSegment,
    active: RoaringBitmap,
    mapping: Vec<u32>,
}


fn merge_cover_set<'a>(
    inputs: &'a [MergeInput<'a>],
    read: impl Fn(&'a MergeInput<'a>) -> StorageResult<super::record_set::RecordSetView<'a>>,
) -> StorageResult<RoaringBitmap> {
    let mut result = RoaringBitmap::new();
    for input in inputs {
        for slot in read(input)?.ids() {
            let slot = slot?;
            if let Some(mapped) = input.mapping.get(slot as usize)
                && *mapped != u32::MAX
            {
                result.insert(*mapped);
            }
        }
    }
    Ok(result)
}

fn map_input_set(
    input: &MergeInput<'_>,
    view: super::record_set::RecordSetView<'_>,
) -> StorageResult<Vec<u32>> {
    let mut slots = Vec::new();
    for slot in view.ids() {
        let slot = slot?;
        if let Some(mapped) = input.mapping.get(slot as usize)
            && *mapped != u32::MAX
        {
            slots.push(*mapped);
        }
    }
    Ok(slots)
}

fn merge_records(
    inputs: &[MergeInput<'_>],
    count: u32,
    cancelled: &dyn Fn() -> bool,
) -> StorageResult<(Vec<FilePartInput>, Vec<RecordInput>)> {
    let mut parts = Vec::new();
    let mut records = Vec::with_capacity(count as usize);
    for input in inputs {
        if cancelled() {
            return Err(StorageError::Cancelled);
        }
        let view = input.segment.records();
        for part_number in 0..input.segment.stats().file_part_count {
            let part = view
                .file_part(part_number)?
                .ok_or_else(|| StorageError::corrupt("packed file part missing"))?;
            let start_slot = part.segment_slot_start;
            let end_slot = start_slot.saturating_add(part.record_count);
            let mut run_start = None;
            let mut run_len = 0u32;
            let mut run_local = 0u32;
            let mut run_segment = 0u32;
            let mut flush = |run_start: &mut Option<u32>,
                             run_len: &mut u32,
                             run_local: u32,
                             run_segment: u32|
             -> StorageResult<()> {
                let Some(start) = run_start.take() else {
                    return Ok(());
                };
                let part_ordinal = u32::try_from(parts.len())
                    .map_err(|_| StorageError::ResourceLimit("too many merge parts".into()))?;
                for offset in 0..*run_len {
                    let row = view.record(start + offset)?.ok_or_else(|| {
                        StorageError::corrupt("packed record missing during merge")
                    })?;
                    records.push(RecordInput {
                        part_ordinal,
                        local_record_id: row.local_record_id,
                        byte_start: row.byte_start,
                        byte_end: row.byte_end,
                        logical_id: row.logical_id,
                        diagnostic_ref: row.diagnostic_ref,
                        record_class: row.record_class,
                    });
                }
                parts.push(FilePartInput {
                    file_id: part.file_id,
                    content_generation: part.content_generation,
                    local_record_start: run_local,
                    record_count: *run_len,
                    segment_slot_start: run_segment,
                });
                *run_len = 0;
                Ok(())
            };
            for slot in start_slot..end_slot {
                let Some(mapped) = input.mapping.get(slot as usize).copied() else {
                    continue;
                };
                if mapped == u32::MAX {
                    if run_start.is_some() {
                        flush(&mut run_start, &mut run_len, run_local, run_segment)?;
                    }
                    continue;
                }
                let row = view
                    .record(slot)?
                    .ok_or_else(|| StorageError::corrupt("packed record missing during merge"))?;
                if run_start.is_some() {
                    let local = run_local + run_len;
                    if row.local_record_id == local && mapped == run_segment + run_len {
                        run_len += 1;
                        continue;
                    }
                    flush(&mut run_start, &mut run_len, run_local, run_segment)?;
                }
                run_start = Some(slot);
                run_local = row.local_record_id;
                run_segment = mapped;
                run_len = 1;
            }
            flush(&mut run_start, &mut run_len, run_local, run_segment)?;
        }
    }
    if records.len() != count as usize {
        return Err(StorageError::corrupt(
            "merged records do not cover the output domain",
        ));
    }
    Ok((parts, records))
}

fn merge_fields(
    inputs: &[MergeInput<'_>],
    logical_count: u32,
    cancelled: &dyn Fn() -> bool,
) -> StorageResult<Vec<FieldInput>> {
    let mut merged = std::collections::BTreeMap::<(Vec<String>, u8, String), FieldInput>::new();
    for input in inputs {
        if cancelled() {
            return Err(StorageError::Cancelled);
        }
        for field in input.segment.postings().fields() {
            let key = (
                field.path().to_vec(),
                match field.source() {
                    super::posting_builder::FieldSource::Declared => 0,
                    super::posting_builder::FieldSource::Observed => 1,
                    super::posting_builder::FieldSource::Named(_) => 2,
                },
                match field.source() {
                    super::posting_builder::FieldSource::Named(name) => name.clone(),
                    _ => String::new(),
                },
            );
            let entry = merged.entry(key).or_insert_with(|| FieldInput {
                ordinal: 0,
                path: field.path().to_vec(),
                source: field.source().clone(),
                complete: true,
                kind: field.kind().clone(),
                multi: field.multi(),
                presence: Vec::new(),
                nulls: Vec::new(),
                empty_selections: Vec::new(),
                empty_strings: Vec::new(),
                structural: BTreeMap::new(),
                values: BTreeMap::new(),
                lengths: BTreeMap::new(),
                values_indexed: field.values_indexed(),
                value_complete: field.value_complete(),
                length_complete: field.length_complete(),
                empty_string_complete: field.empty_string_complete(),
                covered_records: 0,
            });
            entry.complete &= field.complete();
            entry.values_indexed &= field.values_indexed();
            entry.value_complete &= field.value_complete();
            entry.length_complete &= field.length_complete();
            entry.empty_string_complete &= field.empty_string_complete();
            entry
                .presence
                .extend(map_input_set(input, field.presence()?)?);
            entry.nulls.extend(map_input_set(input, field.nulls()?)?);
            entry
                .empty_selections
                .extend(map_input_set(input, field.empty_selections()?)?);
            entry
                .empty_strings
                .extend(map_input_set(input, field.empty_string()?)?);
            for key in field.structural_keys() {
                if let Some(view) = field.structural(key)? {
                    entry
                        .structural
                        .entry(key)
                        .or_default()
                        .extend(map_input_set(input, view)?);
                }
            }
            for (position, body) in field
                .values()
                .range(std::ops::Bound::Unbounded, std::ops::Bound::Unbounded)
                .enumerate()
            {
                if position % 256 == 0 && cancelled() {
                    return Err(StorageError::Cancelled);
                }
                let (key, bytes) = body?;
                let key = decode_scalar_key(&key)?;
                let view = record_set(&bytes, field, input.segment)?;
                entry
                    .values
                    .entry(key)
                    .or_default()
                    .extend(map_input_set(input, view)?);
            }
            if let Some(dictionary) = field.lengths() {
                for body in dictionary.range(std::ops::Bound::Unbounded, std::ops::Bound::Unbounded)
                {
                    let (key, bytes) = body?;
                    let ScalarKey::Number(length) = decode_scalar_key(&key)? else {
                        return Err(StorageError::corrupt(
                            "length dictionary contains a non-number key",
                        ));
                    };
                    let view = record_set(&bytes, field, input.segment)?;
                    entry
                        .lengths
                        .entry(length)
                        .or_default()
                        .extend(map_input_set(input, view)?);
                }
            }
            let source_logical = input.segment.coverage().logical_records()?.cardinality() as u32;
            if field.covered_records() >= source_logical {
                entry.covered_records = entry.covered_records.saturating_add(logical_count);
            }
        }
    }
    let mut result = Vec::with_capacity(merged.len());
    for (ordinal, (_, mut field)) in merged.into_iter().enumerate() {
        field.ordinal = ordinal as u32;
        field.covered_records = field.covered_records.min(logical_count);
        for slots in field.values.values_mut() {
            slots.sort_unstable();
            slots.dedup();
        }
        for slots in field.lengths.values_mut() {
            slots.sort_unstable();
            slots.dedup();
        }
        for slots in field.structural.values_mut() {
            slots.sort_unstable();
            slots.dedup();
        }
        result.push(field);
    }
    Ok(result)
}

fn merge_bigrams(
    inputs: &[MergeInput<'_>],
    count: u32,
    cancelled: &dyn Fn() -> bool,
) -> StorageResult<Vec<BigramInput>> {
    type Binding = (
        BigramBinding,
        BTreeMap<u64, Vec<u32>>,
        BTreeMap<u64, Vec<u32>>,
        u8,
    );
    let mut bindings: Vec<Binding> = Vec::new();
    for input in inputs {
        if cancelled() {
            return Err(StorageError::Cancelled);
        }
        let section = input.segment.bigrams();
        for binding in section.bindings() {
            let view = match binding {
                BigramBinding::Raw => section.binding(BigramBindingKind::Raw),
                BigramBinding::Fallback => section.binding(BigramBindingKind::Fallback),
                BigramBinding::Custom { path, name } => section.custom(path, name),
            }
            .ok_or_else(|| StorageError::corrupt("packed bigram binding missing"))?;
            let at = bindings
                .iter()
                .position(|(candidate, ..)| candidate == binding)
                .unwrap_or_else(|| {
                    bindings.push((
                        binding.clone(),
                        BTreeMap::new(),
                        BTreeMap::new(),
                        view.alphabet(),
                    ));
                    bindings.len() - 1
                });
            for (position, gram) in view.normal_grams()?.into_iter().enumerate() {
                if position % 256 == 0 && cancelled() {
                    return Err(StorageError::Cancelled);
                }
                if let Some(posting) = view.normal(gram)? {
                    let mapped = map_input_set(input, posting)?;
                    bindings[at].1.entry(gram).or_default().extend(mapped);
                }
            }
            for (position, gram) in view.skip_grams()?.into_iter().enumerate() {
                if position % 256 == 0 && cancelled() {
                    return Err(StorageError::Cancelled);
                }
                if let Some(posting) = view.skip(gram)? {
                    let mapped = map_input_set(input, posting)?;
                    bindings[at].2.entry(gram).or_default().extend(mapped);
                }
            }
        }
    }
    Ok(bindings
        .into_iter()
        .map(|(binding, normal, skip, alphabet)| BigramInput {
            binding,
            alphabet,
            record_count: count,
            normal: normal
                .into_iter()
                .map(|(gram, mut slots)| {
                    slots.sort_unstable();
                    slots.dedup();
                    (gram, slots)
                })
                .collect(),
            skip: skip
                .into_iter()
                .map(|(gram, mut slots)| {
                    slots.sort_unstable();
                    slots.dedup();
                    (gram, slots)
                })
                .collect(),
        })
        .collect())
}

fn merge_sorts(
    inputs: &[MergeInput<'_>],
    count: u32,
    cancelled: &dyn Fn() -> bool,
) -> StorageResult<Vec<SortColumnInput>> {
    let mut columns: Vec<(
        Vec<String>,
        SortColumnInput,
        BTreeMap<u32, crate::sorting::SortKey>,
    )> = Vec::new();
    for input in inputs {
        if cancelled() {
            return Err(StorageError::Cancelled);
        }
        for column in input.segment.sorting().columns() {
            let at = columns
                .iter()
                .position(|(path, ..)| path == column.path())
                .unwrap_or_else(|| {
                    columns.push((
                        column.path().to_vec(),
                        SortColumnInput {
                            path: column.path().to_vec(),
                            kind: column.kind().clone(),
                            locale: column.locale().into(),
                            options: column.options(),
                            collation_version: column.collation_version().into(),
                            keys: Vec::new(),
                            ascending: Vec::new(),
                            descending: Vec::new(),
                        },
                        BTreeMap::new(),
                    ));
                    columns.len() - 1
                });
            for slot in input.active.iter() {
                if let Some(mapped) = input.mapping.get(slot as usize)
                    && *mapped != u32::MAX
                {
                    let key = column
                        .key_at(slot)?
                        .ok_or_else(|| StorageError::corrupt("sort key missing for active slot"))?;
                    columns[at].2.insert(*mapped, key);
                }
            }
        }
    }
    let mut result = Vec::with_capacity(columns.len());
    for (_, mut column, keys_by_slot) in columns {
        let mut ordering = (0..count)
            .map(|slot| {
                let key = keys_by_slot
                    .get(&slot)
                    .cloned()
                    .unwrap_or(crate::sorting::SortKey::Missing);
                (slot, key)
            })
            .collect::<Vec<_>>();
        ordering.sort_by(|left, right| {
            left.1
                .compare_with(&right.1, false, column.options)
                .then_with(|| left.0.cmp(&right.0))
        });
        column.keys = ordering.iter().map(|(_, key)| key.clone()).collect();
        column.ascending = ordering.iter().map(|(slot, _)| *slot).collect();
        let ascending_slots = column
            .ascending
            .iter()
            .map(|slot| crate::model::RecordSlot(*slot))
            .collect::<Vec<_>>();
        column.descending =
            crate::sorting::descending_order(&ascending_slots, &column.keys, column.options)
                .into_iter()
                .map(|slot| slot.0)
                .collect();
        result.push(column);
    }
    Ok(result)
}

fn merge_relations(
    inputs: &[MergeInput<'_>],
    count: u32,
    cancelled: &dyn Fn() -> bool,
) -> StorageResult<Vec<RelationInput>> {
    let mut merged: Vec<(u32, Vec<String>, RelationInput, Vec<Vec<RelationEdgeInput>>)> =
        Vec::new();
    for input in inputs {
        if cancelled() {
            return Err(StorageError::Cancelled);
        }
        for (ordinal, relation) in input.segment.relations().relations().iter().enumerate() {
            let at = merged
                .iter()
                .position(|(target, path, ..)| {
                    *target == relation.target_collection && path == &relation.path
                })
                .unwrap_or_else(|| {
                    merged.push((
                        relation.target_collection,
                        relation.path.clone(),
                        RelationInput {
                            id_ordinal: ordinal as u32,
                            target_collection: relation.target_collection,
                            path: relation.path.clone(),
                            cardinality: relation.cardinality(),
                            reverse_alias: relation.reverse_alias.clone(),
                            offsets: Vec::new(),
                            edges: Vec::new(),
                            present: Vec::new(),
                            nulls: Vec::new(),
                            empty_arrays: Vec::new(),
                            unavailable: Vec::new(),
                            unresolved_count: 0,
                        },
                        vec![Vec::new(); count as usize],
                    ));
                    merged.len() - 1
                });
            let entry = &mut merged[at];
            entry
                .2
                .present
                .extend(map_input_set(input, relation.present()?)?);
            entry
                .2
                .nulls
                .extend(map_input_set(input, relation.nulls()?)?);
            entry
                .2
                .empty_arrays
                .extend(map_input_set(input, relation.empty_arrays()?)?);
            entry
                .2
                .unavailable
                .extend(map_input_set(input, relation.unavailable()?)?);
            for slot in 0..relation.slot_count() {
                if slot % 256 == 0 && cancelled() {
                    return Err(StorageError::Cancelled);
                }
                let Some(mapped) = input.mapping.get(slot as usize).copied() else {
                    continue;
                };
                if mapped == u32::MAX {
                    continue;
                }
                for edge in relation.edges(slot)? {
                    entry.3[mapped as usize].push(RelationEdgeInput {
                        source_slot: mapped,
                        target_slot: edge.target_slot,
                        target_id: edge.target_id,
                        positions: edge.positions,
                    });
                }
            }
        }
    }
    let mut result = Vec::with_capacity(merged.len());
    for (_, _, mut relation, edges) in merged {
        relation.offsets = vec![0u32];
        for (slot, mut slot_edges) in edges.into_iter().enumerate() {
            debug_assert_eq!(slot as u32, relation.offsets.len() as u32 - 1);
            slot_edges.sort_by_key(|edge| (edge.target_id.clone(), edge.positions.clone()));
            relation.edges.extend(slot_edges);
            relation.offsets.push(relation.edges.len() as u32);
        }
        relation.unresolved_count = relation
            .edges
            .iter()
            .filter(|edge| edge.target_slot.is_none())
            .count() as u64;
        relation.present.sort_unstable();
        relation.present.dedup();
        relation.nulls.sort_unstable();
        relation.nulls.dedup();
        relation.empty_arrays.sort_unstable();
        relation.empty_arrays.dedup();
        relation.unavailable.sort_unstable();
        relation.unavailable.dedup();
        result.push(relation);
    }
    Ok(result)
}

fn merge_identity(
    inputs: &[MergeInput<'_>],
    count: u32,
    cancelled: &dyn Fn() -> bool,
) -> StorageResult<Vec<u32>> {
    let mut identities = Vec::with_capacity(count as usize);
    for input in inputs {
        if cancelled() {
            return Err(StorageError::Cancelled);
        }
        for slot in input.segment.records().identity_order()? {
            if let Some(mapped) = input.mapping.get(slot as usize)
                && *mapped != u32::MAX
            {
                let row = input
                    .segment
                    .records()
                    .record(slot)?
                    .ok_or_else(|| StorageError::corrupt("identity record missing"))?;
                identities.push((row.logical_id, *mapped));
            }
        }
    }
    identities.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    Ok(identities.into_iter().map(|(_, slot)| slot).collect())
}

fn map_input_bitmap(input: &MergeInput<'_>, bitmap: &RoaringBitmap) -> Vec<u32> {
    bitmap
        .iter()
        .filter_map(|slot| input.mapping.get(slot as usize).copied())
        .filter(|slot| *slot != u32::MAX)
        .collect()
}

fn merge_metadata(inputs: &[MergeInput<'_>]) -> StorageResult<Option<Vec<u8>>> {
    let mut merged: Option<InferenceMetadata> = None;
    for input in inputs {
        let Some(bytes) = input.segment.metadata()? else {
            continue;
        };
        let metadata: InferenceMetadata = bincode::DefaultOptions::new()
            .with_fixint_encoding()
            .reject_trailing_bytes()
            .deserialize(bytes)
            .map_err(|error| StorageError::corrupt(error.to_string()))?;
        let target = merged.get_or_insert_with(|| InferenceMetadata {
            report: metadata.report.clone(),
            empty_selections: std::collections::HashMap::new(),
            short_string_coverage: std::collections::HashMap::new(),
            global_inference: metadata.global_inference.clone(),
        });
        for (path, set) in metadata.empty_selections {
            target
                .empty_selections
                .entry(path)
                .or_default()
                .extend(map_input_bitmap(input, &set));
        }
        for (path, set) in metadata.short_string_coverage {
            target
                .short_string_coverage
                .entry(path)
                .or_default()
                .extend(map_input_bitmap(input, &set));
        }
    }
    merged
        .map(|metadata| {
            bincode::DefaultOptions::new()
                .with_fixint_encoding()
                .reject_trailing_bytes()
                .serialize(&metadata)
                .map_err(|error| StorageError::corrupt(error.to_string()))
        })
        .transpose()
}
