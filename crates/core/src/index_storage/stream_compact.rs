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

/// Maximum records in one packed replacement run.
pub(crate) const COMPACTED_PART_RECORDS: usize = 2_048;

/// Build bounded packed runs for the selected source slots.
///
/// Chunking keeps temporary builder state proportional to one part rather than
/// the whole source segment. Each chunk is independently remapped and sealed.
pub(crate) fn compact_packed_parts(
    source: &FrozenSegment,
    active: &RoaringBitmap,
    collection: u32,
) -> StorageResult<Vec<FrozenSegment>> {
    let mut parts = Vec::new();
    let mut chunk = RoaringBitmap::new();
    for slot in active.iter() {
        chunk.insert(slot);
        if chunk.len() as usize >= COMPACTED_PART_RECORDS {
            let selection = std::mem::take(&mut chunk);
            if let Some(part) = compact_packed_run(source, &selection, collection)? {
                parts.push(part);
            }
        }
    }
    if !chunk.is_empty()
        && let Some(part) = compact_packed_run(source, &chunk, collection)?
    {
        parts.push(part);
    }
    Ok(parts)
}

/// Build one packed run containing only `active` source slots.
pub(crate) fn compact_packed_run(
    source: &FrozenSegment,
    active: &RoaringBitmap,
    collection: u32,
) -> StorageResult<Option<FrozenSegment>> {
    let source_count = source.stats().record_count;
    if active.is_empty() {
        return Ok(None);
    }
    let mut mapping = vec![u32::MAX; source_count as usize];
    for (new_slot, old_slot) in active.iter().enumerate() {
        let new_slot = u32::try_from(new_slot)
            .map_err(|_| StorageError::ResourceLimit("compacted slot overflow".into()))?;
        if old_slot >= source_count {
            return Err(StorageError::corrupt(
                "compaction active slot exceeds the source domain",
            ));
        }
        mapping[old_slot as usize] = new_slot;
    }
    let new_count = active.len() as u32;
    let source_logical = source.coverage().logical_records()?;
    let source_logical_count = source_logical.cardinality() as u32;
    let logical_records = map_set(source_logical, &mapping)?;
    let logical_count = logical_records.len() as u32;
    let empty_arrays = map_set(source.coverage().empty_arrays()?, &mapping)?;
    let (file_parts, records) = compact_records(source, &mapping, new_count)?;
    let fields = compact_fields(source, &mapping, source_logical_count, logical_count)?;
    let bigrams = compact_bigrams(source, &mapping, new_count)?;
    let sorts = compact_sorts(source, &mapping, new_count)?;
    let relations = compact_relations(source, &mapping, new_count)?;
    let active_records = (0..new_count).collect::<Vec<_>>();
    let identity_order = source
        .records()
        .identity_order()?
        .into_iter()
        .filter_map(|slot| mapping.get(slot as usize).copied())
        .filter(|slot| *slot != u32::MAX)
        .collect::<Vec<_>>();
    let metadata = compact_metadata(source, &mapping)?;
    let stats = source.stats();
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
        logical_records,
        empty_arrays,
        active_records,
        metadata,
        estimated_index_bytes: stats.estimated_index_bytes,
    };
    let built = build_segment(input)?;
    Ok(Some(FrozenSegment::from_bytes(built.object.into_bytes())?))
}

fn compact_records(
    source: &FrozenSegment,
    mapping: &[u32],
    new_count: u32,
) -> StorageResult<(Vec<FilePartInput>, Vec<RecordInput>)> {
    let stats = source.stats();
    let view = source.records();
    let mut parts = Vec::new();
    let mut records = Vec::with_capacity(new_count as usize);
    let mut new_start = 0u32;
    for part_number in 0..stats.file_part_count {
        let part = view
            .file_part(part_number)?
            .ok_or_else(|| StorageError::corrupt("packed file part missing"))?;
        let start_slot = part.segment_slot_start;
        let end_slot = start_slot.saturating_add(part.record_count);
        let selected = (start_slot..end_slot)
            .filter(|slot| {
                mapping
                    .get(*slot as usize)
                    .is_some_and(|mapped| *mapped != u32::MAX)
            })
            .collect::<Vec<_>>();
        if selected.is_empty() {
            continue;
        }
        let part_ordinal = u32::try_from(parts.len())
            .map_err(|_| StorageError::ResourceLimit("too many compacted file parts".into()))?;
        for (local, slot) in selected.iter().enumerate() {
            let row = view
                .record(*slot)?
                .ok_or_else(|| StorageError::corrupt("packed record missing"))?;
            records.push(RecordInput {
                part_ordinal,
                local_record_id: u32::try_from(local)
                    .map_err(|_| StorageError::ResourceLimit("file part overflow".into()))?,
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
            local_record_start: 0,
            record_count: selected.len() as u32,
            segment_slot_start: new_start,
        });
        new_start = new_start
            .checked_add(selected.len() as u32)
            .ok_or_else(|| StorageError::ResourceLimit("compacted slot overflow".into()))?;
    }
    if new_start != new_count {
        return Err(StorageError::corrupt(
            "compacted records do not cover the new slot domain",
        ));
    }
    Ok((parts, records))
}

fn compact_fields(
    source: &FrozenSegment,
    mapping: &[u32],
    source_logical_count: u32,
    new_logical_count: u32,
) -> StorageResult<Vec<FieldInput>> {
    let mut fields = Vec::with_capacity(source.postings().fields().len());
    for field in source.postings().fields() {
        let presence = map_set(field.presence()?, mapping)?;
        let nulls = map_set(field.nulls()?, mapping)?;
        let empty_selections = map_set(field.empty_selections()?, mapping)?;
        let empty_strings = map_set(field.empty_string()?, mapping)?;
        let mut structural = BTreeMap::new();
        for key in field.structural_keys() {
            if let Some(view) = field.structural(key)? {
                let mapped = map_set(view, mapping)?;
                if !mapped.is_empty() {
                    structural.insert(key, mapped);
                }
            }
        }
        let mut values = BTreeMap::new();
        for entry in field
            .values()
            .range(std::ops::Bound::Unbounded, std::ops::Bound::Unbounded)
        {
            let (key, bytes) = entry?;
            let key = decode_scalar_key(&key)?;
            let view = record_set(&bytes, field, source)?;
            let mapped = map_set(view, mapping)?;
            if !mapped.is_empty() {
                values.insert(key, mapped);
            }
        }
        let mut lengths = BTreeMap::new();
        if let Some(dictionary) = field.lengths() {
            for entry in dictionary.range(std::ops::Bound::Unbounded, std::ops::Bound::Unbounded) {
                let (key, bytes) = entry?;
                let ScalarKey::Number(length) = decode_scalar_key(&key)? else {
                    return Err(StorageError::corrupt(
                        "length dictionary contains a non-number key",
                    ));
                };
                let view = record_set(&bytes, field, source)?;
                let mapped = map_set(view, mapping)?;
                if !mapped.is_empty() {
                    lengths.insert(length, mapped);
                }
            }
        }
        // A field either covers every logical record of the source run or is
        // incomplete. Keep that distinction after retired slots are removed;
        // a partial count cannot be projected into the smaller domain.
        let covered_records = if field.covered_records() >= source_logical_count {
            new_logical_count
        } else {
            0
        };
        fields.push(FieldInput {
            ordinal: field.ordinal(),
            path: field.path().to_vec(),
            source: field.source().clone(),
            complete: field.complete(),
            kind: field.kind().clone(),
            multi: field.multi(),
            presence,
            nulls,
            empty_selections,
            empty_strings,
            structural,
            values,
            lengths,
            values_indexed: field.values_indexed(),
            value_complete: field.value_complete(),
            length_complete: field.length_complete(),
            empty_string_complete: field.empty_string_complete(),
            covered_records,
        });
    }
    Ok(fields)
}

fn record_set<'a>(
    bytes: &[u8],
    field: &'a super::posting::FieldPostingsView,
    _source: &'a FrozenSegment,
) -> StorageResult<super::record_set::RecordSetView<'a>> {
    let reference = super::record_set::RecordSetRef::decode(bytes)?;
    field.view(reference)
}

fn compact_bigrams(
    source: &FrozenSegment,
    mapping: &[u32],
    new_count: u32,
) -> StorageResult<Vec<BigramInput>> {
    let section = source.bigrams();
    let mut inputs = Vec::new();
    for binding in section.bindings() {
        let view = match binding {
            BigramBinding::Raw => section.binding(BigramBindingKind::Raw),
            BigramBinding::Fallback => section.binding(BigramBindingKind::Fallback),
            BigramBinding::Custom { path, name } => section.custom(path, name),
        }
        .ok_or_else(|| StorageError::corrupt("packed bigram binding missing"))?;
        let mut normal = Vec::new();
        for gram in view.normal_grams()? {
            if let Some(posting) = view.normal(gram)? {
                let mapped = map_set(posting, mapping)?;
                if !mapped.is_empty() {
                    normal.push((gram, mapped));
                }
            }
        }
        let mut skip = Vec::new();
        for gram in view.skip_grams()? {
            if let Some(posting) = view.skip(gram)? {
                let mapped = map_set(posting, mapping)?;
                if !mapped.is_empty() {
                    skip.push((gram, mapped));
                }
            }
        }
        inputs.push(BigramInput {
            binding: binding.clone(),
            alphabet: view.alphabet(),
            record_count: new_count,
            normal,
            skip,
        });
    }
    Ok(inputs)
}

fn compact_sorts(
    source: &FrozenSegment,
    mapping: &[u32],
    new_count: u32,
) -> StorageResult<Vec<SortColumnInput>> {
    let mut columns = Vec::new();
    for column in source.sorting().columns() {
        let mut keys = Vec::with_capacity(new_count as usize);
        for slot in 0..source.stats().record_count {
            if mapping[slot as usize] != u32::MAX {
                keys.push(
                    column
                        .key_at(slot)?
                        .ok_or_else(|| StorageError::corrupt("sort key missing for active slot"))?,
                );
            }
        }
        let ascending = order_slots(column, mapping, false)?;
        let descending = order_slots(column, mapping, true)?;
        columns.push(SortColumnInput {
            path: column.path().to_vec(),
            kind: column.kind().clone(),
            locale: column.locale().into(),
            options: column.options(),
            collation_version: column.collation_version().into(),
            keys,
            ascending,
            descending,
        });
    }
    Ok(columns)
}

fn order_slots(
    column: &super::sort::SortColumnView,
    mapping: &[u32],
    descending: bool,
) -> StorageResult<Vec<u32>> {
    let mut slots = Vec::new();
    let count = column.key_count();
    for index in 0..count {
        let slot = if descending {
            column.descending_slot(index)?
        } else {
            column.ascending_slot(index)?
        };
        if let Some(slot) = slot
            && let Some(mapped) = mapping.get(slot as usize)
            && *mapped != u32::MAX
        {
            slots.push(*mapped);
        }
    }
    Ok(slots)
}

fn compact_relations(
    source: &FrozenSegment,
    mapping: &[u32],
    new_count: u32,
) -> StorageResult<Vec<RelationInput>> {
    let mut relations = Vec::new();
    for (ordinal, relation) in source.relations().relations().iter().enumerate() {
        if relation.slot_count() as usize > mapping.len() {
            return Err(StorageError::corrupt(
                "relation slot domain exceeds the source record domain",
            ));
        }
        let mut offsets = vec![0u32];
        let mut edges = Vec::new();
        for slot in 0..relation.slot_count() {
            if mapping[slot as usize] == u32::MAX {
                continue;
            }
            for edge in relation.edges(slot)? {
                edges.push(RelationEdgeInput {
                    source_slot: mapping[slot as usize],
                    target_slot: edge.target_slot,
                    target_id: edge.target_id,
                    positions: edge.positions,
                });
            }
            offsets.push(u32::try_from(edges.len()).map_err(|_| {
                StorageError::ResourceLimit("compact relation edge overflow".into())
            })?);
        }
        if offsets.len() != new_count as usize + 1 {
            return Err(StorageError::corrupt(
                "compact relation offsets do not cover the new domain",
            ));
        }
        // Only unresolved edges contribute after compaction; target slots stay
        // in their own collection domain.
        let unresolved = edges
            .iter()
            .filter(|edge| edge.target_slot.is_none())
            .count() as u64;
        relations.push(RelationInput {
            id_ordinal: ordinal as u32,
            target_collection: relation.target_collection,
            path: relation.path.clone(),
            cardinality: relation.cardinality(),
            reverse_alias: relation.reverse_alias.clone(),
            offsets,
            edges,
            present: map_set(relation.present()?, mapping)?,
            nulls: map_set(relation.nulls()?, mapping)?,
            empty_arrays: map_set(relation.empty_arrays()?, mapping)?,
            unavailable: map_set(relation.unavailable()?, mapping)?,
            unresolved_count: unresolved,
        });
    }
    Ok(relations)
}

fn compact_metadata(source: &FrozenSegment, mapping: &[u32]) -> StorageResult<Option<Vec<u8>>> {
    let Some(bytes) = source.metadata()? else {
        return Ok(None);
    };
    let mut metadata: InferenceMetadata = bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .reject_trailing_bytes()
        .deserialize(bytes)
        .map_err(|error| StorageError::corrupt(error.to_string()))?;
    for set in metadata.empty_selections.values_mut() {
        *set = map_bitmap(set, mapping)?;
    }
    for set in metadata.short_string_coverage.values_mut() {
        *set = map_bitmap(set, mapping)?;
    }
    let bytes = bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .reject_trailing_bytes()
        .serialize(&metadata)
        .map_err(|error| StorageError::corrupt(error.to_string()))?;
    Ok(Some(bytes))
}

fn map_set(view: super::record_set::RecordSetView<'_>, mapping: &[u32]) -> StorageResult<Vec<u32>> {
    let mut slots = Vec::new();
    for slot in view.ids() {
        let slot = slot?;
        if let Some(mapped) = mapping.get(slot as usize)
            && *mapped != u32::MAX
        {
            slots.push(*mapped);
        }
    }
    Ok(slots)
}

fn map_bitmap(bitmap: &RoaringBitmap, mapping: &[u32]) -> StorageResult<RoaringBitmap> {
    let mut result = RoaringBitmap::new();
    for slot in bitmap.iter() {
        let mapped = mapping
            .get(slot as usize)
            .ok_or_else(|| StorageError::corrupt("inference coverage slot out of bounds"))?;
        if *mapped != u32::MAX {
            result.insert(*mapped);
        }
    }
    Ok(result)
}
