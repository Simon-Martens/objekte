//! Temporary native materialization of one packed run for maintenance.
//!
//! Compaction uses this bounded, source-free view to project active slots and
//! then seals packed output. The materialized owners are dropped immediately
//! after the new packed runs are built.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use bincode::Options;
use roaring::RoaringBitmap;

use crate::bigram::BigramIndex;
use crate::build::IndexedCollectionSegment;
use crate::execution_options::BigramAlphabet;
use crate::inference::InferenceState;
use crate::inventory::{ByteRange, DiagnosticRef, FileEntry, RecordClass, RecordEntry};
use crate::lifecycle::IndexError;
use crate::model::{CollectionId, FieldPath, FileId, PathRoot, RecordId, RecordSlot};
use crate::posting::{
    CustomPostingIndex, PostingCollection, PostingIndex, PostingStats, StructuralFacts,
    StructuralKey,
};
use crate::relation::{RelationCardinality, RelationContribution, RelationDefinition};
use crate::repository::{CollectionState, GlobalInferenceSummary};
use crate::sorting::{SortCollection, SortColumn};

use super::error::StorageResult;
use super::native_pack::InferenceMetadata;
use super::packed_restore::storage_error;
use super::posting::FieldPostingsView;
use super::posting_builder::FieldSource;
use super::segment::FrozenSegment;

pub(crate) fn materialize_native_segment(
    frozen: &Arc<FrozenSegment>,
    collection: usize,
    file_entries: &[FileEntry],
    definitions: &[crate::repository::CollectionDefinition],
) -> Result<IndexedCollectionSegment, IndexError> {
    let stats = frozen.stats();
    let records_view = frozen.records();
    let mut files: Vec<crate::build::SegmentFile> = Vec::new();
    let mut file_index = HashMap::new();
    let mut records = Vec::with_capacity(stats.record_count as usize);
    let mut id_lookup = HashMap::new();
    for part_number in 0..stats.file_part_count {
        let part = records_view
            .file_part(part_number)
            .map_err(storage_error)?
            .ok_or_else(|| IndexError::InvalidPublication("packed file part missing".into()))?;
        let entry = file_entries
            .get(part.file_id as usize)
            .cloned()
            .ok_or_else(|| IndexError::InvalidPublication("packed file metadata missing".into()))?;
        let file_slot = *file_index.entry(FileId(part.file_id)).or_insert_with(|| {
            files.push(crate::build::SegmentFile {
                entry: entry.clone(),
                records: Vec::new(),
            });
            files.len() - 1
        });
        for local in part.local_record_start..part.local_record_start + part.record_count {
            let slot = part.segment_slot_start + local;
            let row = records_view
                .record(slot)
                .map_err(storage_error)?
                .ok_or_else(|| IndexError::InvalidPublication("packed record missing".into()))?;
            let record = RecordEntry {
                slot: RecordSlot(slot),
                id: RecordId(row.logical_id.into()),
                file: FileId(part.file_id),
                range: ByteRange {
                    start: row.byte_start,
                    end: row.byte_end,
                },
                revision: entry.revision,
                class: match row.record_class {
                    0 => RecordClass::Logical,
                    1 => RecordClass::RawFallback,
                    _ => {
                        return Err(IndexError::InvalidPublication(
                            "invalid packed record class".into(),
                        ));
                    }
                },
                diagnostic_ref: DiagnosticRef(row.diagnostic_ref),
            };
            id_lookup.insert(record.id.clone(), record.slot);
            files[file_slot].records.push(record.slot);
            records.push(record);
        }
    }
    let postings = frozen.postings();
    let (posting_collection, inference, custom_postings) = posting_families(&postings, frozen)?;
    let mut raw = BigramIndex::new(BigramAlphabet::Ascii);
    let mut fallback_text = BigramIndex::new(BigramAlphabet::Ascii);
    let mut custom_text = HashMap::new();
    for binding in frozen.bigrams().bindings() {
        let index = bigram_from_packed(frozen, binding)?;
        match binding {
            crate::index_storage::bigram_builder::BigramBinding::Raw => raw = index,
            crate::index_storage::bigram_builder::BigramBinding::Fallback => fallback_text = index,
            crate::index_storage::bigram_builder::BigramBinding::Custom { path, name } => {
                custom_text.insert((path.clone(), name.clone()), index);
            }
        }
    }
    let mut sorting = SortCollection {
        columns: HashMap::new(),
        composites: Vec::new(),
    };
    for column in frozen.sorting().columns() {
        let path = FieldPath {
            root: PathRoot::Data,
            segments: column.path().to_vec(),
        };
        let keys = (0..column.key_count())
            .map(|slot| column.key_at(slot))
            .collect::<StorageResult<Option<Vec<_>>>>()
            .map_err(storage_error)?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        sorting.columns.insert(
            path,
            SortColumn {
                path: FieldPath {
                    root: PathRoot::Data,
                    segments: column.path().to_vec(),
                },
                kind: column.kind().clone(),
                keys,
                ascending: column
                    .ascending_slots()
                    .map_err(storage_error)?
                    .into_iter()
                    .map(RecordSlot)
                    .collect(),
                descending: column
                    .descending_slots()
                    .map_err(storage_error)?
                    .into_iter()
                    .map(RecordSlot)
                    .collect(),
                locale: column.locale().into(),
                options: column.options(),
                collation_version: column.collation_version().into(),
                packed: None,
            },
        );
    }
    let mut logical_records = RoaringBitmap::new();
    frozen
        .coverage()
        .logical_records()
        .map_err(storage_error)?
        .union_into(&mut logical_records)
        .map_err(storage_error)?;
    let mut empty_array_records = RoaringBitmap::new();
    frozen
        .coverage()
        .empty_arrays()
        .map_err(storage_error)?
        .union_into(&mut empty_array_records)
        .map_err(storage_error)?;
    let mut relations = Vec::new();
    for relation in frozen.relations().relations() {
        let target_collection = definitions
            .get(relation.target_collection as usize)
            .ok_or_else(|| {
                IndexError::InvalidPublication("relation target collection missing".into())
            })?;
        let mut references = Vec::new();
        for slot in 0..relation.slot_count() {
            for edge in relation.edges(slot).map_err(storage_error)? {
                references.push(crate::relation::PendingReference {
                    source: RecordSlot(slot),
                    target_id: edge.target_id,
                    positions: edge.positions,
                });
            }
        }
        relations.push(RelationContribution {
            declaration: RelationDefinition {
                path: relation.path.clone(),
                target_collection: target_collection.name.clone(),
                cardinality: if relation_cardinality(relation) {
                    RelationCardinality::One
                } else {
                    RelationCardinality::Many
                },
                reverse_alias: relation.reverse_alias.clone(),
            },
            record_count: relation.slot_count(),
            references,
            present: relation
                .present()
                .map_err(storage_error)?
                .ids()
                .collect::<StorageResult<RoaringBitmap>>()
                .map_err(storage_error)?,
            nulls: relation
                .nulls()
                .map_err(storage_error)?
                .ids()
                .collect::<StorageResult<RoaringBitmap>>()
                .map_err(storage_error)?,
            empty_arrays: relation
                .empty_arrays()
                .map_err(storage_error)?
                .ids()
                .collect::<StorageResult<RoaringBitmap>>()
                .map_err(storage_error)?,
            unavailable: relation
                .unavailable()
                .map_err(storage_error)?
                .ids()
                .collect::<StorageResult<RoaringBitmap>>()
                .map_err(storage_error)?,
            packed: None,
        });
    }
    Ok(IndexedCollectionSegment {
        collection: CollectionId(collection as u32),
        source_id: stats.source_id.clone(),
        source_version: stats.source_version,
        files,
        file_index,
        records: crate::inventory::RecordStore::native(records),
        id_lookup: crate::inventory::RecordIdLookup::Native(id_lookup),
        identity_order: records_view
            .identity_order()
            .map_err(storage_error)?
            .into_iter()
            .map(RecordSlot)
            .collect(),
        postings: posting_collection,
        inference,
        custom_postings,
        raw,
        fallback_text,
        custom_text,
        logical_records,
        empty_array_records,
        sorting,
        relations,
        record_parses: 0,
        failed_parses: 0,
        accounting: Default::default(),
        packed: Some(Arc::clone(frozen)),
    })
}

fn relation_cardinality(relation: &crate::index_storage::relations::RelationView) -> bool {
    relation.cardinality() == 0
}

fn posting_families(
    postings: &crate::index_storage::posting::PostingSectionView,
    frozen: &FrozenSegment,
) -> Result<
    (
        PostingCollection,
        Option<InferenceState>,
        HashMap<(Vec<String>, String), CustomPostingIndex>,
    ),
    IndexError,
> {
    let mut fields = HashMap::new();
    let mut observed = HashMap::new();
    let mut custom = HashMap::new();
    let mut observed_empty = HashMap::new();
    let mut observed_coverage = HashMap::new();
    let metadata = frozen
        .metadata()
        .map_err(storage_error)?
        .map(|bytes| {
            bincode::DefaultOptions::new()
                .with_fixint_encoding()
                .reject_trailing_bytes()
                .deserialize::<InferenceMetadata>(bytes)
                .map_err(|error| IndexError::InvalidPublication(error.to_string()))
        })
        .transpose()?;
    for field in postings.fields() {
        let index = posting_index(field)?;
        match field.source() {
            FieldSource::Declared => {
                fields.insert(field.path().to_vec(), index);
            }
            FieldSource::Observed => {
                observed.insert(field.path().to_vec(), index);
            }
            FieldSource::Named(binding) => {
                custom.insert(
                    (field.path().to_vec(), binding.clone()),
                    CustomPostingIndex {
                        index,
                        complete: field.complete(),
                    },
                );
            }
        }
    }
    if let Some(metadata) = &metadata {
        observed_empty = metadata.empty_selections.clone();
        observed_coverage = metadata.short_string_coverage.clone();
    }
    let inference = metadata.map(|metadata| InferenceState {
        report: metadata.report,
        indexes: observed,
        empty_selections: observed_empty,
        short_string_coverage: observed_coverage,
    });
    Ok((PostingCollection { fields }, inference, custom))
}

fn posting_index(field: &FieldPostingsView) -> Result<PostingIndex, IndexError> {
    let presence = collect(field.presence().map_err(storage_error)?)?;
    let nulls = collect(field.nulls().map_err(storage_error)?)?;
    let empty_selections = collect(field.empty_selections().map_err(storage_error)?)?;
    let mut structural = BTreeMap::new();
    for key in field.structural_keys() {
        if let Some(view) = field.structural(key).map_err(storage_error)? {
            if let Some(key) = structural_key(key) {
                structural.insert(key, collect(view)?);
            }
        }
    }
    let mut values = BTreeMap::new();
    for entry in field
        .values()
        .range(std::ops::Bound::Unbounded, std::ops::Bound::Unbounded)
    {
        let (key, _) = entry.map_err(storage_error)?;
        let key = super::scalar::decode_scalar_key(&key).map_err(storage_error)?;
        if let Some(view) = field
            .value_encoded(&super::scalar::encode_scalar_key(&key).map_err(storage_error)?)
            .map_err(storage_error)?
        {
            values.insert(key, collect(view)?);
        }
    }
    let mut lengths = BTreeMap::new();
    if let Some(dictionary) = field.lengths() {
        for entry in dictionary.range(std::ops::Bound::Unbounded, std::ops::Bound::Unbounded) {
            let (key, _) = entry.map_err(storage_error)?;
            let crate::posting::ScalarKey::Number(length) =
                super::scalar::decode_scalar_key(&key).map_err(storage_error)?
            else {
                continue;
            };
            if let Some(view) = field.length(&length).map_err(storage_error)? {
                lengths.insert(length, collect(view)?);
            }
        }
    }
    let mut index = PostingIndex {
        path: field.path().to_vec(),
        kind: field.kind().clone(),
        multi: field.multi(),
        presence,
        empty_selections,
        nulls,
        structural: (!structural.is_empty()).then_some(StructuralFacts {
            postings: structural,
        }),
        values,
        lengths,
        values_indexed: field.values_indexed(),
        value_complete: field.value_complete(),
        length_complete: field.length_complete(),
        empty_string_complete: field.empty_string_complete(),
        covered_records: field.covered_records(),
        stats: PostingStats::default(),
    };
    index.finish_stats();
    Ok(index)
}

fn structural_key(code: u8) -> Option<StructuralKey> {
    Some(match code {
        0 => StructuralKey::AnyNull,
        1 => StructuralKey::NonEmpty,
        2 => StructuralKey::String,
        3 => StructuralKey::Number,
        4 => StructuralKey::Array,
        5 => StructuralKey::Object,
        6 => StructuralKey::EmptyObject,
        _ => return None,
    })
}

fn collect(view: super::record_set::RecordSetView<'_>) -> Result<RoaringBitmap, IndexError> {
    let mut bitmap = RoaringBitmap::new();
    view.union_into(&mut bitmap).map_err(storage_error)?;
    Ok(bitmap)
}

fn bigram_from_packed(
    frozen: &FrozenSegment,
    binding: &crate::index_storage::bigram_builder::BigramBinding,
) -> Result<BigramIndex, IndexError> {
    let view = match binding {
        crate::index_storage::bigram_builder::BigramBinding::Raw => frozen
            .bigrams()
            .binding(crate::index_storage::bigram_builder::BigramBindingKind::Raw),
        crate::index_storage::bigram_builder::BigramBinding::Fallback => frozen
            .bigrams()
            .binding(crate::index_storage::bigram_builder::BigramBindingKind::Fallback),
        crate::index_storage::bigram_builder::BigramBinding::Custom { path, name } => {
            frozen.bigrams().custom(path, name)
        }
    }
    .ok_or_else(|| IndexError::InvalidPublication("packed bigram binding missing".into()))?;
    let alphabet = match view.alphabet() {
        0 => BigramAlphabet::Ascii,
        _ => BigramAlphabet::Unicode,
    };
    let mut normal = HashMap::new();
    for gram in view.normal_grams().map_err(storage_error)? {
        if let Some(view) = view.normal(gram).map_err(storage_error)? {
            normal.insert(gram, collect(view)?);
        }
    }
    let mut skip = HashMap::new();
    for gram in view.skip_grams().map_err(storage_error)? {
        if let Some(view) = view.skip(gram).map_err(storage_error)? {
            skip.insert(gram, collect(view)?);
        }
    }
    Ok(BigramIndex {
        alphabet,
        record_count: view.record_count(),
        normal,
        skip,
    })
}
