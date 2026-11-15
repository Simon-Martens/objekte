//! Canonical native-reference facts and digests.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::bigram::BigramIndex;
use crate::inventory::RecordClass;
use crate::model::FileId;
use crate::posting::{ScalarKey, StructuralKey};
use crate::repository::{CollectionDefinition, Repository};
use crate::sorting::SortKey;

use super::contribution_codec;
use super::key_codec::Cursor;
use super::namespace::Namespace;
use super::value_codec::{self, BindingIdentity, StoredBinding, StoredContributions};
use super::{RocksStore, StoreError, StoreResult, keys};

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct FamilyDigest {
    pub rows: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReferenceDigests {
    pub files: FamilyDigest,
    pub records: FamilyDigest,
    pub values: FamilyDigest,
    pub lengths: FamilyDigest,
    pub states: FamilyDigest,
    pub domains: FamilyDigest,
    pub grams: FamilyDigest,
    pub sorts: FamilyDigest,
    pub relations: FamilyDigest,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReferenceComparison {
    pub equal: bool,
    pub mismatched_families: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct RecordFact {
    collection: String,
    id: String,
    file: FileId,
    start: u64,
    end: u64,
    revision_modified_ns: i128,
    revision_size: u64,
    class: RecordClass,
}

#[derive(Serialize)]
struct ValueFact<'a> {
    record: &'a RecordFact,
    binding: &'a BindingIdentity,
    value: &'a ScalarKey,
}

#[derive(Serialize)]
struct LengthFact<'a> {
    record: &'a RecordFact,
    binding: &'a BindingIdentity,
    length: &'a crate::model::Number,
}

#[derive(Serialize)]
struct StateFact<'a> {
    record: &'a RecordFact,
    binding: &'a BindingIdentity,
    tag: u8,
}

#[derive(Serialize)]
struct DomainFact<'a> {
    record: &'a RecordFact,
    domain: value_codec::DomainKind,
}

#[derive(Serialize)]
struct GramFact<'a> {
    record: &'a RecordFact,
    binding: &'a BindingIdentity,
    kind: value_codec::GramKind,
    gram: u64,
}

#[derive(Serialize)]
struct SortFact<'a> {
    record: &'a RecordFact,
    path: Vec<String>,
    descending: bool,
    key: &'a SortKey,
}

#[derive(Serialize)]
struct RelationFact<'a> {
    record: &'a RecordFact,
    path: Vec<String>,
    occurrence_ordinal: u32,
    target_collection: String,
    target_id: &'a str,
    positions: &'a [u32],
}

#[derive(Default)]
struct Rows {
    files: Vec<Vec<u8>>,
    records: Vec<Vec<u8>>,
    values: Vec<Vec<u8>>,
    lengths: Vec<Vec<u8>>,
    states: Vec<Vec<u8>>,
    domains: Vec<Vec<u8>>,
    grams: Vec<Vec<u8>>,
    sorts: Vec<Vec<u8>>,
    relations: Vec<Vec<u8>>,
}

impl Rows {
    fn finish(self) -> StoreResult<ReferenceDigests> {
        Ok(ReferenceDigests {
            files: digest_rows(self.files)?,
            records: digest_rows(self.records)?,
            values: digest_rows(self.values)?,
            lengths: digest_rows(self.lengths)?,
            states: digest_rows(self.states)?,
            domains: digest_rows(self.domains)?,
            grams: digest_rows(self.grams)?,
            sorts: digest_rows(self.sorts)?,
            relations: digest_rows(self.relations)?,
        })
    }
}

pub fn native_reference(repository: &Repository) -> StoreResult<ReferenceDigests> {
    let mut rows = Rows::default();
    for file in &repository.files().files {
        rows.files.push(encode(file)?);
    }
    let collection_names = repository
        .collections()
        .iter()
        .map(|collection| {
            (
                collection.generation.collection,
                collection.definition.name.clone(),
            )
        })
        .collect::<HashMap<_, _>>();
    for collection in repository.collections() {
        let collection_name = collection.definition.name.clone();
        let records = collection
            .generation
            .records
            .iter()
            .map(|record| RecordFact {
                collection: collection_name.clone(),
                id: record.id.0.to_string(),
                file: record.file,
                start: record.range.start,
                end: record.range.end,
                revision_modified_ns: record.revision.modified_ns,
                revision_size: record.revision.size,
                class: record.class,
            })
            .collect::<Vec<_>>();
        for record in &records {
            rows.records.push(encode(record)?);
        }
        let record_for = |slot: u32| -> StoreResult<&RecordFact> {
            records.get(slot as usize).ok_or_else(|| {
                StoreError::CorruptIndex("native posting slot is out of bounds".into())
            })
        };
        for (path, posting) in &collection.postings.fields {
            let binding = BindingIdentity::Declared { path: path.clone() };
            append_posting(&mut rows, &records, &binding, posting)?;
        }
        for ((path, name), custom) in &collection.custom_postings {
            let binding = BindingIdentity::Named {
                path: path.clone(),
                name: name.clone(),
            };
            append_posting(&mut rows, &records, &binding, &custom.index)?;
        }
        append_bigram(
            &mut rows,
            &records,
            &BindingIdentity::RawText,
            &collection.raw,
        )?;
        append_bigram(
            &mut rows,
            &records,
            &BindingIdentity::FallbackText,
            &collection.fallback_text,
        )?;
        for ((path, name), index) in &collection.custom_text {
            append_bigram(
                &mut rows,
                &records,
                &BindingIdentity::CustomText {
                    path: path.clone(),
                    name: name.clone(),
                },
                index,
            )?;
        }
        for (slot, record) in records.iter().enumerate() {
            let slot = slot as u32;
            if collection.logical_records.contains(slot) {
                rows.domains.push(encode(&DomainFact {
                    record,
                    domain: value_codec::DomainKind::Logical,
                })?);
            }
            if record.class == RecordClass::RawFallback {
                rows.domains.push(encode(&DomainFact {
                    record,
                    domain: value_codec::DomainKind::RawFallback,
                })?);
            }
            if collection.empty_array_records.contains(slot) {
                rows.domains.push(encode(&DomainFact {
                    record,
                    domain: value_codec::DomainKind::EmptyArray,
                })?);
            }
        }
        for column in collection.sorting.columns.values() {
            for (slot, key) in column.keys.iter().enumerate() {
                let record = record_for(slot as u32)?;
                rows.sorts.push(encode(&SortFact {
                    record,
                    path: column.path.segments.clone(),
                    descending: false,
                    key,
                })?);
                rows.sorts.push(encode(&SortFact {
                    record,
                    path: column.path.segments.clone(),
                    descending: true,
                    key,
                })?);
            }
        }
        for relation in &collection.relations {
            for (slot, record) in records.iter().enumerate() {
                if let Some(edges) = relation.edges(crate::model::RecordSlot(slot as u32)) {
                    let target = collection_names
                        .get(&relation.target_collection)
                        .cloned()
                        .unwrap_or_else(|| relation.target_collection.0.to_string());
                    for (ordinal, edge) in edges.iter().enumerate() {
                        rows.relations.push(encode(&RelationFact {
                            record,
                            path: relation.declaration.path.clone(),
                            occurrence_ordinal: ordinal as u32,
                            target_collection: target.clone(),
                            target_id: edge.target_id(),
                            positions: &edge.positions,
                        })?);
                    }
                }
            }
        }
    }
    rows.finish()
}

pub fn store_reference(
    store: &RocksStore,
    definitions: &[CollectionDefinition],
) -> StoreResult<ReferenceDigests> {
    let view = store.view()?;
    let mut rows = Rows::default();
    let mut paths = HashMap::<u32, Vec<u8>>::new();
    scan(&view, Namespace::Files, |_logical, bytes| {
        let metadata: value_codec::FileMetadata = value_codec::decode(bytes, "file metadata")?;
        paths.insert(
            metadata.entry.id.0,
            crate::path_repr::native_bytes(metadata.entry.relative_path.as_path()),
        );
        rows.files.push(encode(&metadata.entry)?);
        Ok(())
    })?;
    let mut bindings = BTreeMap::<(u32, u32), BindingIdentity>::new();
    scan(&view, Namespace::Bindings, |logical, bytes| {
        let mut cursor = Cursor::new(logical);
        let collection = cursor.read_u32()?;
        let _identity = cursor.read_bytes()?;
        let binding: StoredBinding = value_codec::decode(bytes, "binding")?;
        bindings.insert((collection, binding.id), binding.descriptor.identity);
        Ok(())
    })?;
    let mut sort_definitions = BTreeMap::<(u32, u32), value_codec::SortDefinition>::new();
    scan(&view, Namespace::SortDefinitions, |logical, bytes| {
        let mut cursor = Cursor::new(logical);
        let collection = cursor.read_u32()?;
        let id = cursor.read_u32()?;
        sort_definitions.insert(
            (collection, id),
            value_codec::decode(bytes, "sort definition")?,
        );
        Ok(())
    })?;
    let relation_paths = definitions
        .iter()
        .enumerate()
        .map(|(index, definition)| {
            let paths = definition
                .schema
                .relations()
                .keys()
                .enumerate()
                .map(|(ordinal, path)| (ordinal as u32, path.clone()))
                .collect::<HashMap<_, _>>();
            (index as u32, paths)
        })
        .collect::<HashMap<_, _>>();

    let mut contributions = Vec::<(u32, u32, StoredContributions, RecordFact)>::new();
    scan(&view, Namespace::RecordContributions, |logical, bytes| {
        let mut cursor = Cursor::new(logical);
        let collection = cursor.read_u32()?;
        let slot = cursor.read_u32()?;
        let contribution = contribution_codec::decode(bytes)?.into_contributions(Vec::new());
        let record = RecordFact {
            collection: definitions
                .get(collection as usize)
                .map(|definition| definition.name.clone())
                .ok_or_else(|| StoreError::CorruptIndex("unknown collection".into()))?,
            id: contribution.record.id.0.to_string(),
            file: contribution.record.file,
            start: contribution.record.range.start,
            end: contribution.record.range.end,
            revision_modified_ns: contribution.record.revision.modified_ns,
            revision_size: contribution.record.revision.size,
            class: contribution.record.class,
        };
        rows.records.push(encode(&record)?);
        for (binding, value) in &contribution.values {
            let binding = bindings.get(&(collection, *binding)).ok_or_else(|| {
                StoreError::CorruptIndex("value binding identity is missing".into())
            })?;
            rows.values.push(encode(&ValueFact {
                record: &record,
                binding,
                value,
            })?);
        }
        for (binding, length) in &contribution.lengths {
            let binding = bindings.get(&(collection, *binding)).ok_or_else(|| {
                StoreError::CorruptIndex("length binding identity is missing".into())
            })?;
            rows.lengths.push(encode(&LengthFact {
                record: &record,
                binding,
                length,
            })?);
        }
        for (binding, mask) in &contribution.state_masks {
            let binding = bindings.get(&(collection, *binding)).ok_or_else(|| {
                StoreError::CorruptIndex("state binding identity is missing".into())
            })?;
            for tag in 0..=value_codec::STATE_EMPTY_OBJECT {
                if mask & (1 << tag) != 0 {
                    rows.states.push(encode(&StateFact {
                        record: &record,
                        binding,
                        tag,
                    })?);
                }
            }
        }
        for domain in &contribution.domains {
            rows.domains.push(encode(&DomainFact {
                record: &record,
                domain: *domain,
            })?);
        }
        for projection in &contribution.sorts {
            let definition = sort_definitions
                .get(&(collection, projection.definition))
                .ok_or_else(|| StoreError::CorruptIndex("sort definition is missing".into()))?;
            for (constituent, key) in definition.constituents.iter().zip(&projection.key) {
                rows.sorts.push(encode(&SortFact {
                    record: &record,
                    path: constituent.key.segments.clone(),
                    descending: constituent.direction == value_codec::SortDirection::Descending,
                    key,
                })?);
            }
        }
        for occurrence in &contribution.relations {
            let path = relation_paths
                .get(&collection)
                .and_then(|paths| paths.get(&occurrence.declaration_ordinal))
                .ok_or_else(|| {
                    StoreError::CorruptIndex("relation declaration is missing".into())
                })?;
            let target = definitions
                .get(occurrence.target_collection.0 as usize)
                .map(|definition| definition.name.as_str())
                .unwrap_or("<unknown>");
            rows.relations.push(encode(&RelationFact {
                record: &record,
                path: path.clone(),
                occurrence_ordinal: occurrence.occurrence_ordinal,
                target_collection: target.into(),
                target_id: &occurrence.target_id,
                positions: &occurrence.positions,
            })?);
        }
        contributions.push((collection, slot, contribution, record));
        Ok(())
    })?;
    let _ = contributions;

    scan(&view, Namespace::RecordGrams, |logical, bytes| {
        let mut cursor = Cursor::new(logical);
        let collection = cursor.read_u32()?;
        let slot = cursor.read_u32()?;
        let binding_id = cursor.read_u32()?;
        let kind = match cursor.read_u8()? {
            value_codec::GRAM_NORMAL => value_codec::GramKind::Normal,
            value_codec::GRAM_SKIP => value_codec::GramKind::Skip,
            tag => {
                return Err(StoreError::CorruptIndex(format!("unknown gram kind {tag}")));
            }
        };
        let binding = bindings
            .get(&(collection, binding_id))
            .ok_or_else(|| StoreError::CorruptIndex("gram binding identity is missing".into()))?;
        let record = record_for_contribution(&view, collection, slot, &mut paths, definitions)?;
        for gram in value_codec::decode_gram_list(bytes)? {
            rows.grams.push(encode(&GramFact {
                record: &record,
                binding,
                kind,
                gram,
            })?);
        }
        Ok(())
    })?;
    rows.finish()
}

pub fn compare_reference(
    native: &ReferenceDigests,
    candidate: &ReferenceDigests,
) -> ReferenceComparison {
    let mut mismatched_families = Vec::new();
    macro_rules! compare {
        ($name:literal, $field:ident) => {
            if native.$field != candidate.$field {
                mismatched_families.push($name.into());
            }
        };
    }
    compare!("files", files);
    compare!("records", records);
    compare!("values", values);
    compare!("lengths", lengths);
    compare!("states", states);
    compare!("domains", domains);
    compare!("grams", grams);
    compare!("sorts", sorts);
    compare!("relations", relations);
    ReferenceComparison {
        equal: mismatched_families.is_empty(),
        mismatched_families,
    }
}

fn append_posting(
    rows: &mut Rows,
    records: &[RecordFact],
    binding: &BindingIdentity,
    posting: &crate::posting::PostingIndex,
) -> StoreResult<()> {
    for (value, slots) in &posting.values {
        for slot in slots.iter() {
            rows.values.push(encode(&ValueFact {
                record: records.get(slot as usize).ok_or_else(|| {
                    StoreError::CorruptIndex("native value slot is out of bounds".into())
                })?,
                binding,
                value,
            })?);
        }
    }
    for (length, slots) in &posting.lengths {
        for slot in slots.iter() {
            rows.lengths.push(encode(&LengthFact {
                record: records.get(slot as usize).ok_or_else(|| {
                    StoreError::CorruptIndex("native length slot is out of bounds".into())
                })?,
                binding,
                length,
            })?);
        }
    }
    for (tag, slots) in [
        (value_codec::STATE_PATH_PRESENCE, &posting.presence),
        (
            value_codec::STATE_EMPTY_SELECTION,
            &posting.empty_selections,
        ),
        (value_codec::STATE_LEGACY_NULL_SET, &posting.nulls),
    ] {
        for slot in slots.iter() {
            rows.states.push(encode(&StateFact {
                record: records.get(slot as usize).ok_or_else(|| {
                    StoreError::CorruptIndex("native state slot is out of bounds".into())
                })?,
                binding,
                tag,
            })?);
        }
    }
    if let Some(structural) = &posting.structural {
        for (key, slots) in &structural.postings {
            let tag = match key {
                StructuralKey::AnyNull => value_codec::STATE_ANY_NULL,
                StructuralKey::NonEmpty => value_codec::STATE_NON_EMPTY,
                StructuralKey::String => value_codec::STATE_STRING,
                StructuralKey::Number => value_codec::STATE_NUMBER,
                StructuralKey::Array => value_codec::STATE_ARRAY,
                StructuralKey::Object => value_codec::STATE_OBJECT,
                StructuralKey::EmptyObject => value_codec::STATE_EMPTY_OBJECT,
            };
            for slot in slots.iter() {
                rows.states.push(encode(&StateFact {
                    record: records.get(slot as usize).ok_or_else(|| {
                        StoreError::CorruptIndex("native state slot is out of bounds".into())
                    })?,
                    binding,
                    tag,
                })?);
            }
        }
    }
    Ok(())
}

fn append_bigram(
    rows: &mut Rows,
    records: &[RecordFact],
    binding: &BindingIdentity,
    index: &BigramIndex,
) -> StoreResult<()> {
    for (kind, map) in [
        (value_codec::GramKind::Normal, &index.normal),
        (value_codec::GramKind::Skip, &index.skip),
    ] {
        for (gram, slots) in map {
            for slot in slots.iter() {
                rows.grams.push(encode(&GramFact {
                    record: records.get(slot as usize).ok_or_else(|| {
                        StoreError::CorruptIndex("native gram slot is out of bounds".into())
                    })?,
                    binding,
                    kind,
                    gram: *gram,
                })?);
            }
        }
    }
    Ok(())
}

fn record_for_contribution(
    view: &super::StoreView<'_>,
    collection: u32,
    slot: u32,
    paths: &mut HashMap<u32, Vec<u8>>,
    definitions: &[CollectionDefinition],
) -> StoreResult<RecordFact> {
    let bytes = view
        .get(Namespace::Records, &keys::record(collection, slot))?
        .ok_or_else(|| StoreError::CorruptIndex("record metadata is missing".into()))?;
    let record: value_codec::RecordMetadata = value_codec::decode(&bytes, "record metadata")?;
    let path = paths.entry(record.file.0).or_insert_with(|| Vec::new());
    if path.is_empty() {
        let bytes = view
            .get(Namespace::Files, &keys::file(record.file))?
            .ok_or_else(|| StoreError::CorruptIndex("record file is missing".into()))?;
        let metadata: value_codec::FileMetadata = value_codec::decode(&bytes, "file metadata")?;
        *path = crate::path_repr::native_bytes(metadata.entry.relative_path.as_path());
    }
    Ok(RecordFact {
        collection: definitions
            .get(collection as usize)
            .map(|definition| definition.name.clone())
            .ok_or_else(|| StoreError::CorruptIndex("unknown collection".into()))?,
        id: record.id.0.to_string(),
        file: record.file,
        start: record.range.start,
        end: record.range.end,
        revision_modified_ns: record.revision.modified_ns,
        revision_size: record.revision.size,
        class: record.class,
    })
}

fn scan(
    view: &super::StoreView<'_>,
    namespace: Namespace,
    mut visit: impl FnMut(&[u8], &[u8]) -> StoreResult<()>,
) -> StoreResult<()> {
    let mut iterator = view.namespace_prefix(namespace, b"")?;
    iterator.seek_to_first();
    while iterator.valid() {
        let key = iterator
            .key()
            .ok_or_else(|| StoreError::CorruptIndex("missing reference key".into()))?;
        visit(
            &key[1..],
            iterator
                .value()
                .ok_or_else(|| StoreError::CorruptIndex("missing reference value".into()))?,
        )?;
        iterator.next();
    }
    iterator.status()?;
    Ok(())
}

fn encode(value: &impl Serialize) -> StoreResult<Vec<u8>> {
    bincode::serialize(value)
        .map_err(|error| StoreError::CorruptValue(format!("reference encode: {error}")))
}

fn digest_rows(mut rows: Vec<Vec<u8>>) -> StoreResult<FamilyDigest> {
    rows.sort_unstable();
    let mut digest = Sha256::new();
    for row in &rows {
        digest.update((row.len() as u64).to_le_bytes());
        digest.update(row);
    }
    Ok(FamilyDigest {
        rows: rows.len() as u64,
        sha256: format!("{:x}", digest.finalize()),
    })
}
