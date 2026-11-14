//! Convert the resident publication into packed catalogue and run objects.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use bincode::Options;
use serde::{Deserialize, Serialize};

use roaring::RoaringBitmap;

use crate::bigram::BigramIndex;
use crate::inference::InferenceState;
use crate::inventory::{FileEntry, FileVersions, RecordEntry};
use crate::lifecycle::IndexView;
use crate::model::{FieldPath, RecordSlot};
use crate::posting::{PostingCollection, PostingIndex, StructuralKey};
use crate::relation::{RelationIndex, RelationTarget};
use crate::repository::{CollectionState, GlobalInferenceSummary};
use crate::sorting::{SortCollection, SortKey};
use crate::source::DiagnosticCategory;

use super::bigram_builder::{BigramBinding, BigramInput};
use super::catalogue::{DiagnosticInput, FileRowInput, build_catalogue_with_high_water};
use super::error::StorageResult;
use super::format::{ObjectBuilder, ObjectKind, SectionKind};
use super::manifest::CollectionSummary;
use super::posting_builder::{FieldInput, FieldSource};
use super::records::{FilePartInput, RecordInput};
use super::relations::{RelationEdgeInput, RelationInput};
use super::segment_builder::{SegmentInput, build_segment};
use super::sort::SortColumnInput;
use super::store::{ObjectInput, RunInput};


/// Small inference metadata packed alongside observed postings.
#[derive(Serialize, Deserialize)]
pub(crate) struct InferenceMetadata {
    pub report: crate::inference::InferenceReport,
    pub empty_selections: HashMap<Vec<String>, RoaringBitmap>,
    pub short_string_coverage: HashMap<Vec<String>, RoaringBitmap>,
    pub global_inference: Option<GlobalInferenceSummary>,
}


/// Query model contract version stored in packed manifests.
pub const MODEL_VERSION: u32 = 1;

/// Build the packed catalogue object for one publication.
pub fn pack_catalogue(view: &IndexView) -> StorageResult<ObjectInput> {
    let published = &view.0;
    let repository = view.repository();
    let mut rows = Vec::new();
    for id in 0..published.catalog.allocated_files() {
        let file = crate::model::FileId(id as u32);
        let version = published
            .file_versions
            .get(file)
            .map_or(1, |version| version.0);
        if let Some(entry) = published.catalog.entry(file) {
            rows.push(file_row(
                entry,
                version,
                published.catalog.fingerprint(file),
                false,
            )?);
        } else if let Some(entry) = published.catalog.inactive_entry(file) {
            rows.push(file_row(entry, version, None, true)?);
        }
    }
    let sections =
        build_catalogue_with_high_water(&rows, published.catalog.allocated_files() as u32)?;
    let mut builder = ObjectBuilder::new(ObjectKind::Catalogue);
    builder.add_section(SectionKind::FileRows, sections.file_rows)?;
    builder.add_section(SectionKind::PathDictionary, sections.path_dictionary)?;
    builder.add_section(SectionKind::Diagnostics, sections.diagnostics)?;
    builder.add_section(SectionKind::Statistics, sections.statistics)?;
    let object = builder.finish()?;
    let _ = repository;
    Ok(ObjectInput {
        kind: ObjectKind::Catalogue,
        bytes: Arc::from(object.into_bytes()),
    })
}

fn file_row(
    entry: &FileEntry,
    version: u64,
    fingerprint: Option<[u8; 32]>,
    tombstone: bool,
) -> StorageResult<FileRowInput> {
    let native_path = native_path_bytes(entry.relative_path.as_path());
    let query_path = crate::path_repr::query_path(entry.relative_path.as_path()).into_bytes();
    Ok(FileRowInput {
        file_id: entry.id.0,
        status: entry.status.code(),
        tombstone,
        file_version: version,
        content_generation: entry.content_generation,
        size: entry.revision.size,
        modified_ns: entry.revision.modified_ns,
        created_ns: entry.created_ns,
        native_path,
        query_path,
        collections: entry.collections.iter().map(|id| id.0).collect(),
        diagnostics: entry
            .diagnostics
            .iter()
            .map(|diagnostic| DiagnosticInput {
                category: category_code(diagnostic.category),
                code: diagnostic.code.clone(),
                field_path: diagnostic.field_path.clone().unwrap_or_default(),
                byte_offset: diagnostic.byte_offset,
                line: diagnostic.line,
                column: diagnostic.column,
                message: diagnostic.message.clone(),
            })
            .collect(),
        omitted_diagnostics: entry.omitted_diagnostics,
        fingerprint,
    })
}

fn category_code(category: DiagnosticCategory) -> u8 {
    match category {
        DiagnosticCategory::Framing => 0,
        DiagnosticCategory::Encoding => 1,
        DiagnosticCategory::Syntax => 2,
        DiagnosticCategory::LogicalValidation => 3,
        DiagnosticCategory::ResourceLimit => 4,
        DiagnosticCategory::Io => 5,
    }
}

fn native_path_bytes(path: &std::path::Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }

    #[cfg(not(unix))]
    {
        path.to_string_lossy().into_owned().into_bytes()
    }
}

/// Pack every live base contribution and delta segment into run objects.
pub fn pack_runs(view: &IndexView) -> StorageResult<Vec<RunInput>> {
    let published = &view.0;
    let repository = view.repository();
    let mut runs = Vec::new();
    for (number, collection) in repository.collections().iter().enumerate() {
        let active = published.active[number].bitmap();
        let slot_count = collection.generation.records.len() as u32;
        if slot_count > 0 {
            let input = pack_collection(collection, &active, slot_count, &published.file_versions)?;
            let built = build_segment(input)?;
            runs.push(RunInput {
                collection: number as u32,
                ordinal: 0,
                level: 0,
                slot_count,
                active_count: active.len(),
                object: ObjectInput {
                    kind: ObjectKind::Segment,
                    bytes: Arc::from(built.object.into_bytes()),
                },
            });
        }
        for segment in published.segments[number].iter() {
            if let Some(packed) = &segment.packed {
                runs.push(RunInput {
                    collection: number as u32,
                    ordinal: segment.ordinal,
                    level: 0,
                    slot_count: packed.stats().record_count,
                    active_count: segment.active.len(),
                    object: ObjectInput {
                        kind: ObjectKind::Segment,
                        bytes: packed.object_bytes()?,
                    },
                });
                continue;
            }
            let input = pack_segment(
                &segment.index,
                &segment.active.bitmap(),
                &published.file_versions,
                repository.collections(),
            )?;
            let built = build_segment(input)?;
            runs.push(RunInput {
                collection: number as u32,
                ordinal: segment.ordinal,
                level: 0,
                slot_count: segment.index.records.len() as u32,
                active_count: segment.active.len(),
                object: ObjectInput {
                    kind: ObjectKind::Segment,
                    bytes: Arc::from(built.object.into_bytes()),
                },
            });
        }
    }
    Ok(runs)
}

/// Summaries for the manifest.
pub fn collection_summaries(view: &IndexView) -> Vec<CollectionSummary> {
    let published = &view.0;
    let repository = view.repository();
    let mut summaries = Vec::new();
    for (number, collection) in repository.collections().iter().enumerate() {
        let base_count = published.active[number].len();
        let segment_count = published.segments[number]
            .iter()
            .map(|segment| segment.active.len())
            .sum::<u64>();
        summaries.push(CollectionSummary {
            collection: number as u32,
            name: collection.definition.name.clone(),
            file_count: collection.generation.files.len() as u64,
            record_count: collection.generation.records.len() as u64
                + published.segments[number]
                    .iter()
                    .map(|segment| segment.index.records.len() as u64)
                    .sum::<u64>(),
            active_count: base_count + segment_count,
            fallback_count: collection
                .generation
                .records
                .iter()
                .filter(|record| record.class.code() == 1)
                .count() as u64,
            run_count: u32::from(collection.generation.records.len() > 0)
                + published.segments[number].len() as u32,
        });
    }
    summaries
}

fn pack_collection(
    collection: &CollectionState,
    active: &RoaringBitmap,
    slot_count: u32,
    versions: &FileVersions,
) -> StorageResult<SegmentInput> {
    let (file_parts, records) = pack_records(
        collection.generation.records.as_slice().unwrap_or(&[]),
        versions,
    )?;
    let fields = pack_fields(
        &collection.postings,
        collection.inference.as_ref(),
        &collection.custom_postings,
    )?;
    let bigrams = pack_bigrams(collection)?;
    let sorts = pack_sorts(&collection.sorting);
    let relations = pack_relations(&collection.relations, slot_count);
    let logical = bitmap_slots(&collection.logical_records);
    let empty = bitmap_slots(&collection.empty_array_records);
    let active = bitmap_slots(active);
    let identity_order = if collection.identity_order.len() == slot_count as usize {
        collection
            .identity_order
            .iter()
            .map(|slot| slot.0)
            .collect()
    } else {
        (0..slot_count).collect()
    };
    Ok(SegmentInput {
        collection: collection.generation.collection.0,
        source_id: collection.source.id().into(),
        source_version: collection.source.version(),
        file_parts,
        records,
        identity_order,
        fields,
        bigrams,
        sorts,
        relations,
        logical_records: logical,
        empty_arrays: empty,
        active_records: active,
        metadata: pack_inference_metadata(
            collection.inference.as_ref(),
            collection.global_inference.as_ref(),
        )?,
        estimated_index_bytes: collection.index_family_bytes as u64,
    })
}

pub(crate) fn pack_segment(
    segment: &crate::build::IndexedCollectionSegment,
    active: &RoaringBitmap,
    versions: &FileVersions,
    collections: &[CollectionState],
) -> StorageResult<SegmentInput> {
    let (file_parts, records) = pack_records(segment.records.as_slice().unwrap_or(&[]), versions)?;
    let fields = pack_fields(
        &segment.postings,
        segment.inference.as_ref(),
        &segment.custom_postings,
    )?;
    let mut bigrams = vec![
        pack_bigram(&segment.raw, BigramBinding::Raw)?,
        pack_bigram(&segment.fallback_text, BigramBinding::Fallback)?,
    ];
    for ((path, name), index) in &segment.custom_text {
        bigrams.push(pack_bigram(
            index,
            BigramBinding::Custom {
                path: path.clone(),
                name: name.clone(),
            },
        )?);
    }
    let sorts = pack_sorts(&segment.sorting);
    let relations = pack_contributions(
        &segment.relations,
        segment.records.len() as u32,
        collections,
    );
    let logical = bitmap_slots(&segment.logical_records);
    let empty = bitmap_slots(&segment.empty_array_records);
    let slot_count = segment.records.len() as u32;
    let identity_order = if segment.identity_order.len() == slot_count as usize {
        segment.identity_order.iter().map(|slot| slot.0).collect()
    } else {
        (0..slot_count).collect()
    };
    Ok(SegmentInput {
        collection: segment.collection.0,
        source_id: segment.source_id.clone(),
        source_version: segment.source_version,
        file_parts,
        records,
        identity_order,
        fields,
        bigrams,
        sorts,
        relations,
        logical_records: logical,
        empty_arrays: empty,
        active_records: bitmap_slots(active),
        metadata: pack_inference_metadata(segment.inference.as_ref(), None)?,
        estimated_index_bytes: segment.accounting.index_family_bytes as u64,
    })
}

fn pack_records(
    records: &[RecordEntry],
    versions: &FileVersions,
) -> StorageResult<(Vec<FilePartInput>, Vec<RecordInput>)> {
    let mut parts = Vec::new();
    let mut packed = Vec::with_capacity(records.len());
    let mut start = 0usize;
    while start < records.len() {
        let file = records[start].file;
        let mut end = start + 1;
        while end < records.len() && records[end].file == file {
            end += 1;
        }
        let part_ordinal = parts.len() as u32;
        let part = FilePartInput {
            file_id: file.0,
            content_generation: versions.get(file).map_or(1, |version| version.0),
            local_record_start: 0,
            record_count: (end - start) as u32,
            segment_slot_start: start as u32,
        };
        parts.push(part);
        for slot in start..end {
            let record = &records[slot];
            packed.push(RecordInput {
                part_ordinal,
                local_record_id: (slot - start) as u32,
                byte_start: record.range.start,
                byte_end: record.range.end,
                logical_id: record.id.0.to_string(),
                diagnostic_ref: record.diagnostic_ref.0,
                record_class: record.class.code(),
            });
        }
        start = end;
    }
    Ok((parts, packed))
}

fn pack_fields(
    postings: &PostingCollection,
    inference: Option<&InferenceState>,
    custom: &HashMap<(Vec<String>, String), crate::posting::CustomPostingIndex>,
) -> StorageResult<Vec<FieldInput>> {
    let mut sources = Vec::new();
    for (path, index) in &postings.fields {
        sources.push((path.clone(), FieldSource::Declared, true, index, None));
    }
    if let Some(inference) = inference {
        for (path, index) in &inference.indexes {
            sources.push((
                path.clone(),
                FieldSource::Observed,
                true,
                index,
                inference.empty_selections.get(path),
            ));
        }
    }
    for ((path, binding), postings) in custom {
        sources.push((
            path.clone(),
            FieldSource::Named(binding.clone()),
            postings.complete,
            &postings.index,
            None,
        ));
    }
    sources.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| field_source_order(&left.1).cmp(&field_source_order(&right.1)))
            .then_with(|| field_source_name(&left.1).cmp(field_source_name(&right.1)))
    });
    let mut result = Vec::with_capacity(sources.len());
    for (ordinal, (path, source, complete, index, empty_selections)) in
        sources.into_iter().enumerate()
    {
        result.push(FieldInput {
            ordinal: ordinal as u32,
            path,
            source,
            complete,
            kind: index.kind.clone(),
            multi: index.multi,
            presence: bitmap_slots(&index.presence),
            nulls: bitmap_slots(&index.nulls),
            empty_selections: empty_selections
                .map_or_else(|| bitmap_slots(&index.empty_selections), bitmap_slots),
            empty_strings: index
                .values
                .get(&crate::posting::ScalarKey::String(String::new()))
                .map_or_else(Vec::new, bitmap_slots),
            structural: pack_structural(index),
            values: index
                .values
                .iter()
                .map(|(key, bitmap)| (key.clone(), bitmap_slots(bitmap)))
                .collect(),
            lengths: index
                .lengths
                .iter()
                .map(|(key, bitmap)| (key.clone(), bitmap_slots(bitmap)))
                .collect(),
            values_indexed: index.values_indexed,
            value_complete: index.value_complete,
            length_complete: index.length_complete,
            empty_string_complete: index.empty_string_complete,
            covered_records: index.covered_records,
        });
    }
    Ok(result)
}

fn field_source_order(source: &FieldSource) -> u8 {
    source.code()
}

fn field_source_name(source: &FieldSource) -> &str {
    match source {
        FieldSource::Named(name) => name,
        _ => "",
    }
}

fn pack_inference_metadata(
    inference: Option<&InferenceState>,
    global: Option<&GlobalInferenceSummary>,
) -> StorageResult<Option<Vec<u8>>> {
    let Some(inference) = inference else {
        return Ok(None);
    };
    let metadata = InferenceMetadata {
        report: inference.report.clone(),
        empty_selections: inference.empty_selections.clone(),
        short_string_coverage: inference.short_string_coverage.clone(),
        global_inference: global.cloned(),
    };
    let bytes = bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .reject_trailing_bytes()
        .serialize(&metadata)
        .map_err(|error| super::error::StorageError::corrupt(error.to_string()))?;
    Ok(Some(bytes))
}

fn pack_structural(index: &PostingIndex) -> BTreeMap<u8, Vec<u32>> {
    let Some(structural) = &index.structural else {
        return BTreeMap::new();
    };
    structural
        .postings
        .iter()
        .map(|(key, bitmap)| (structural_code(*key), bitmap_slots(bitmap)))
        .collect()
}

fn structural_code(key: StructuralKey) -> u8 {
    match key {
        StructuralKey::AnyNull => 0,
        StructuralKey::NonEmpty => 1,
        StructuralKey::String => 2,
        StructuralKey::Number => 3,
        StructuralKey::Array => 4,
        StructuralKey::Object => 5,
        StructuralKey::EmptyObject => 6,
    }
}

fn pack_bigrams(collection: &CollectionState) -> StorageResult<Vec<BigramInput>> {
    let mut bigrams = vec![
        pack_bigram(&collection.raw, BigramBinding::Raw)?,
        pack_bigram(&collection.fallback_text, BigramBinding::Fallback)?,
    ];
    for ((path, name), index) in &collection.custom_text {
        bigrams.push(pack_bigram(
            index,
            BigramBinding::Custom {
                path: path.clone(),
                name: name.clone(),
            },
        )?);
    }
    Ok(bigrams)
}

fn pack_bigram(index: &BigramIndex, binding: BigramBinding) -> StorageResult<BigramInput> {
    let alphabet = match index.alphabet {
        crate::execution_options::BigramAlphabet::Ascii => 0,
        crate::execution_options::BigramAlphabet::Unicode => 1,
    };
    Ok(BigramInput {
        binding,
        alphabet,
        record_count: index.record_count,
        normal: index
            .normal
            .iter()
            .map(|(gram, bitmap)| (*gram, bitmap_slots(bitmap)))
            .collect(),
        skip: index
            .skip
            .iter()
            .map(|(gram, bitmap)| (*gram, bitmap_slots(bitmap)))
            .collect(),
    })
}

fn pack_sorts(sorting: &SortCollection) -> Vec<SortColumnInput> {
    let mut columns = sorting.columns.values().collect::<Vec<_>>();
    columns.sort_by(|left, right| left.path.segments.cmp(&right.path.segments));
    columns
        .into_iter()
        .map(|column| SortColumnInput {
            path: column.path.segments.clone(),
            kind: column.kind.clone(),
            locale: column.locale.clone(),
            options: column.options,
            collation_version: column.collation_version.clone(),
            keys: column.keys.clone(),
            ascending: column.ascending.iter().map(|slot| slot.0).collect(),
            descending: column.descending.iter().map(|slot| slot.0).collect(),
        })
        .collect()
}

fn pack_relations(relations: &[RelationIndex], slot_count: u32) -> Vec<RelationInput> {
    relations
        .iter()
        .map(|relation| {
            let mut offsets = Vec::with_capacity(slot_count as usize + 1);
            let mut edges = Vec::new();
            offsets.push(0u32);
            for slot in 0..slot_count {
                if let Some(source) = relation.edges(RecordSlot(slot)) {
                    for edge in source.iter() {
                        edges.push(RelationEdgeInput {
                            source_slot: slot,
                            target_slot: match &edge.target {
                                RelationTarget::Resolved(slot) => Some(slot.0),
                                RelationTarget::Unresolved(_) => None,
                            },
                            target_id: edge.target_id().to_owned(),
                            positions: edge.positions.clone(),
                        });
                    }
                }
                offsets.push(edges.len() as u32);
            }
            RelationInput {
                id_ordinal: relation.id.ordinal,
                target_collection: relation.target_collection.0,
                path: relation.declaration.path.clone(),
                cardinality: match relation.declaration.cardinality {
                    crate::relation::RelationCardinality::One => 0,
                    crate::relation::RelationCardinality::Many => 1,
                },
                reverse_alias: relation.declaration.reverse_alias.clone(),
                offsets,
                edges,
                present: bitmap_slots(relation.present()),
                nulls: bitmap_slots(relation.nulls()),
                empty_arrays: bitmap_slots(relation.empty_arrays()),
                unavailable: bitmap_slots(relation.unavailable()),
                unresolved_count: relation.unresolved_count(),
            }
        })
        .collect()
}

fn pack_contributions(
    relations: &[crate::relation::RelationContribution],
    slot_count: u32,
    collections: &[CollectionState],
) -> Vec<RelationInput> {
    relations
        .iter()
        .enumerate()
        .map(|(ordinal, relation)| {
            let mut offsets = vec![0u32; slot_count as usize + 1];
            let mut edges = Vec::new();
            for reference in &relation.references {
                let slot = reference.source.0 as usize;
                if slot >= slot_count as usize {
                    continue;
                }
                edges.push(RelationEdgeInput {
                    source_slot: reference.source.0,
                    target_slot: None,
                    target_id: reference.target_id.clone(),
                    positions: reference.positions.clone(),
                });
                offsets[slot + 1] = offsets[slot + 1].saturating_add(1);
            }
            for index in 1..offsets.len() {
                offsets[index] = offsets[index].saturating_add(offsets[index - 1]);
            }
            RelationInput {
                id_ordinal: ordinal as u32,
                target_collection: collections
                    .iter()
                    .position(|collection| {
                        collection.definition.name == relation.declaration.target_collection
                    })
                    .unwrap_or(0) as u32,
                path: relation.declaration.path.clone(),
                cardinality: match relation.declaration.cardinality {
                    crate::relation::RelationCardinality::One => 0,
                    crate::relation::RelationCardinality::Many => 1,
                },
                reverse_alias: relation.declaration.reverse_alias.clone(),
                offsets,
                edges,
                present: bitmap_slots(&relation.present),
                nulls: bitmap_slots(&relation.nulls),
                empty_arrays: bitmap_slots(&relation.empty_arrays),
                unavailable: bitmap_slots(&relation.unavailable),
                unresolved_count: 0,
            }
        })
        .collect()
}

fn bitmap_slots(bitmap: &RoaringBitmap) -> Vec<u32> {
    bitmap.iter().collect()
}

/// Keep the path type import meaningful for readers of this module.
pub fn field_path_key(path: &FieldPath) -> Vec<String> {
    path.segments.clone()
}

/// Expose the sort-key encoder for diagnostics and tests.
pub fn sort_key_round_trip(key: &SortKey) -> StorageResult<SortKey> {
    let encoded = super::sort::encode_sort_key(key)?;
    super::sort::decode_sort_key(&encoded)
}
