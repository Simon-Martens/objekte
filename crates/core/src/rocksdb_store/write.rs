//! Ordered transaction planning for schema setup, discovery and admission.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use bincode::Options;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::inventory::{FileEntry, RecordClass};
use crate::model::{CollectionId, FileId, RecordSlot};

use super::bitmap_block::{BLOCK_SHIFT, decode_block, encode_block};
use super::contribution_codec::ContributionScratch;
use super::metadata::{MetadataWriteStats, MetadataWriter};
use super::namespace::Namespace;
use super::prepare::PreparedStoreBatch;
use super::schema::RuntimeSchema;
use super::value_codec::{
    BuildFileState, BuildPhase, BuildReceipt, CollectionMetadata, FieldCoverageDelta,
    FileBuildState, FileMetadata, GramKind, StoredBinding, StoredContributions,
};
use super::{RocksStore, StoreError, StoreResult, keys, value_codec};

fn bincode_options() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
        .reject_trailing_bytes()
}

fn identity_bytes(identity: &super::value_codec::BindingIdentity) -> StoreResult<Vec<u8>> {
    bincode_options()
        .serialize(identity)
        .map_err(|error| StoreError::CorruptValue(format!("binding identity encode: {error}")))
}

fn physical(namespace: Namespace, logical: Vec<u8>) -> Vec<u8> {
    namespace.physical(&logical)
}

pub(crate) fn initialise_schema(store: &RocksStore, schema: &RuntimeSchema) -> StoreResult<()> {
    let transaction = store.transaction();
    for collection in &schema.collections {
        let metadata = CollectionMetadata {
            id: collection.id,
            name: collection.definition.name.clone(),
            generation: 1,
            source_fingerprint: [0; 32],
            slot_high_water: 0,
            live_records: 0,
            discovered_files: 0,
            pending_files: 0,
            indexed_files: 0,
            unavailable_files: 0,
            processed_files: 0,
            next_binding_id: collection.binding_rows.len() as u32,
        };
        transaction.put(
            physical(Namespace::Collections, keys::collection(collection.id.0)),
            value_codec::encode(&metadata)?,
        )?;
        for (identity, descriptor) in &collection.binding_rows {
            let id = collection.bindings[identity];
            transaction.put(
                physical(
                    Namespace::Bindings,
                    keys::binding_identity(collection.id.0, &identity_bytes(identity)?),
                ),
                value_codec::encode(&StoredBinding {
                    id,
                    descriptor: descriptor.clone(),
                })?,
            )?;
        }
        for definition in &collection.sort_definitions {
            transaction.put(
                physical(
                    Namespace::SortDefinitions,
                    keys::sort_definition(collection.id.0, definition.id),
                ),
                value_codec::encode(definition)?,
            )?;
        }
    }
    transaction.commit().map_err(StoreError::from)
}

#[derive(Clone, Debug, Default)]
pub(crate) struct DiscoveryReport {
    pub files: u64,
    pub memberships: u64,
    pub metadata: MetadataWriteStats,
}

pub(crate) fn discover(
    store: &RocksStore,
    schema: &RuntimeSchema,
    root: &std::path::Path,
) -> StoreResult<DiscoveryReport> {
    let root = root
        .canonicalize()
        .map_err(|error| StoreError::Io(error.to_string()))?;
    let mut next_file_id = 0u32;
    let mut physical_files = BTreeMap::<std::path::PathBuf, (FileId, FileEntry)>::new();
    let mut memberships = Vec::<(CollectionId, FileId)>::new();
    for collection in &schema.collections {
        let paths = crate::discovery::discover(
            &root,
            &collection.definition.source_path,
            &collection.definition.discovery,
            &collection.source,
        )
        .map_err(StoreError::Io)?;
        for path in paths {
            let relative = path
                .strip_prefix(&root)
                .map_err(|error| StoreError::Io(error.to_string()))?
                .to_path_buf();
            let (file_id, entry) = if let Some((id, entry)) = physical_files.get(&relative) {
                (*id, entry.clone())
            } else {
                let metadata =
                    std::fs::metadata(&path).map_err(|error| StoreError::Io(error.to_string()))?;
                let revision = crate::inventory::SourceRevision::from_metadata(&metadata)
                    .map_err(StoreError::State)?;
                let created_ns = metadata
                    .created()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
                    .map(|duration| duration.as_nanos() as i128);
                let id = FileId(next_file_id);
                next_file_id = next_file_id
                    .checked_add(1)
                    .ok_or_else(|| StoreError::State("file ID overflow".into()))?;
                let entry = FileEntry {
                    id,
                    relative_path: relative.clone().into(),
                    revision,
                    created_ns,
                    collections: Vec::new(),
                    status: crate::inventory::FileStatus::Valid,
                    diagnostics: Vec::new(),
                    omitted_diagnostics: 0,
                };
                physical_files.insert(relative.clone(), (id, entry.clone()));
                (id, entry)
            };
            if !entry.collections.contains(&collection.id) {
                if let Some((_, entry)) = physical_files.get_mut(&relative) {
                    entry.collections.push(collection.id);
                }
                memberships.push((collection.id, file_id));
            }
        }
    }
    let view = store.view()?;
    let mut counts = BTreeMap::<u32, CollectionMetadata>::new();
    for collection in &schema.collections {
        let bytes = view
            .get(Namespace::Collections, &keys::collection(collection.id.0))?
            .ok_or_else(|| StoreError::CorruptIndex("collection metadata missing".into()))?;
        counts.insert(
            collection.id.0,
            value_codec::decode(&bytes, "collection metadata")?,
        );
    }
    drop(view);
    let mut writer = MetadataWriter::new(store);
    for (file_id, mut entry) in physical_files.values().cloned() {
        entry
            .collections
            .sort_unstable_by_key(|collection| collection.0);
        let mut operations = Vec::with_capacity(5 + entry.collections.len());
        operations.push((
            physical(Namespace::Files, keys::file(file_id)),
            Some(value_codec::encode(&FileMetadata {
                entry: entry.clone(),
                file_version: 1,
                fingerprint: None,
            })?),
        ));
        let native = crate::path_repr::native_bytes(entry.relative_path.as_path());
        operations.push((
            physical(Namespace::Paths, native.clone()),
            Some(file_id.0.to_be_bytes().to_vec()),
        ));
        let query = crate::path_repr::query_path(entry.relative_path.as_path());
        operations.push((
            physical(Namespace::QueryPaths, keys::query_path(&query, file_id)),
            Some(Vec::new()),
        ));
        operations.push((
            physical(Namespace::FileBuildState, keys::file(file_id)),
            Some(value_codec::encode(&FileBuildState {
                state: BuildFileState::Pending,
                expected_file_version: 1,
                build_epoch: 1,
                last_admission_revision: 0,
                error: None,
            })?),
        ));
        for collection in &entry.collections {
            operations.push((
                physical(
                    Namespace::PendingFiles,
                    keys::pending_file(collection.0, file_id),
                ),
                Some(Vec::new()),
            ));
        }
        writer.put_group(operations)?;
    }
    for (collection, file) in &memberships {
        let _ = file;
        let metadata = counts
            .get_mut(&collection.0)
            .ok_or_else(|| StoreError::CorruptIndex("collection counter missing".into()))?;
        metadata.discovered_files += 1;
        metadata.pending_files += 1;
    }
    for metadata in counts.values() {
        writer.put(
            physical(Namespace::Collections, keys::collection(metadata.id.0)),
            value_codec::encode(metadata)?,
        )?;
    }
    writer.flush()?;
    Ok(DiscoveryReport {
        files: physical_files.len() as u64,
        memberships: memberships.len() as u64,
        metadata: writer.stats(),
    })
}

pub(crate) struct MappedCollectionBatch {
    pub collection: CollectionId,
    pub base_slot: u32,
    pub batch: PreparedStoreBatch,
}

pub(crate) struct PhysicalPreparedFile {
    pub file: FileEntry,
    pub file_version: u64,
    pub fingerprint: Option<[u8; 32]>,
    pub collections: Vec<MappedCollectionBatch>,
}

pub(crate) struct AdmissionBatch {
    pub id: u64,
    pub files: Vec<PhysicalPreparedFile>,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct AdmissionExecutionStats {
    pub planning_ns: u64,
    pub database_ns: u64,
    pub commit_ns: u64,
    pub keys: u64,
    pub encoded_bytes: u64,
    pub block_reads: u64,
    pub attempts: u64,
}

pub(crate) fn map_prepared_file(
    schema: &RuntimeSchema,
    file: FileId,
    mut batches: Vec<(CollectionId, PreparedStoreBatch)>,
    base_slots: &mut BTreeMap<u32, u32>,
) -> StoreResult<PhysicalPreparedFile> {
    let first = batches
        .first()
        .ok_or_else(|| StoreError::State("prepared physical file has no collection".into()))?;
    let file_entry = first
        .1
        .files
        .iter()
        .find(|candidate| candidate.entry.id == file)
        .map(|candidate| candidate.entry.clone())
        .ok_or_else(|| StoreError::State("prepared physical file metadata missing".into()))?;
    let fingerprint = first
        .1
        .file_fingerprints
        .iter()
        .find(|(candidate, _)| *candidate == file)
        .map(|(_, digest)| *digest);
    let mut mapped = Vec::new();
    for (collection, mut batch) in batches.drain(..) {
        let runtime = schema.collection(collection)?;
        let base_slot = *base_slots.get(&collection.0).unwrap_or(&0);
        let next = base_slot
            .checked_add(batch.records.len() as u32)
            .ok_or_else(|| StoreError::State("record slot overflow".into()))?;
        base_slots.insert(collection.0, next);
        let local_bindings = batch
            .bindings
            .iter()
            .map(|binding| {
                runtime
                    .bindings
                    .get(&binding.descriptor.identity)
                    .copied()
                    .ok_or_else(|| {
                        StoreError::State("prepared binding not in runtime schema".into())
                    })
            })
            .collect::<StoreResult<Vec<_>>>()?;
        for value in &mut batch.values {
            value.binding = local_bindings[value.binding as usize];
            value.slot += base_slot;
        }
        for length in &mut batch.lengths {
            length.binding = local_bindings[length.binding as usize];
            length.slot += base_slot;
        }
        for state in &mut batch.states {
            state.binding = local_bindings[state.binding as usize];
            state.slot += base_slot;
        }
        for sort in &mut batch.sorts {
            sort.slot += base_slot;
        }
        for relation in &mut batch.relations {
            relation.slot += base_slot;
        }
        for state in &mut batch.relation_states {
            state.slot += base_slot;
        }
        for gram in &mut batch.grams {
            gram.binding = local_bindings[gram.binding as usize];
            gram.slot += base_slot;
        }
        for (binding, _coverage) in &mut batch.coverage {
            *binding = local_bindings[*binding as usize];
        }
        mapped.push(MappedCollectionBatch {
            collection,
            base_slot,
            batch,
        });
    }
    Ok(PhysicalPreparedFile {
        file: file_entry,
        file_version: 1,
        fingerprint,
        collections: mapped,
    })
}

#[derive(Default)]
struct MutationPlan {
    values: BTreeMap<Vec<u8>, BTreeSet<u16>>,
    lengths: BTreeMap<Vec<u8>, BTreeSet<u16>>,
    states: BTreeMap<Vec<u8>, BTreeSet<u16>>,
    fts: BTreeMap<Vec<u8>, BTreeSet<u16>>,
    puts: BTreeMap<Vec<u8>, Vec<u8>>,
    required_deletes: BTreeSet<Vec<u8>>,
}

impl MutationPlan {
    fn merge(&mut self, other: Self) {
        for (key, offsets) in other.values {
            self.values.entry(key).or_default().extend(offsets);
        }
        for (key, offsets) in other.lengths {
            self.lengths.entry(key).or_default().extend(offsets);
        }
        for (key, offsets) in other.states {
            self.states.entry(key).or_default().extend(offsets);
        }
        for (key, offsets) in other.fts {
            self.fts.entry(key).or_default().extend(offsets);
        }
        self.puts.extend(other.puts);
        self.required_deletes.extend(other.required_deletes);
    }
}

fn add_block(
    blocks: &mut BTreeMap<Vec<u8>, BTreeSet<u16>>,
    namespace: Namespace,
    key: Vec<u8>,
    slot: u32,
) {
    blocks
        .entry(namespace.physical(&key))
        .or_default()
        .insert((slot & super::bitmap_block::BLOCK_MASK) as u16);
}

pub(crate) fn admit_physical_batch(
    store: &RocksStore,
    schema: &RuntimeSchema,
    admission: &AdmissionBatch,
) -> StoreResult<AdmissionExecutionStats> {
    if admission.files.is_empty() {
        return Ok(AdmissionExecutionStats::default());
    }
    let started = Instant::now();
    let preflight = store.view()?;
    let header = preflight.header().clone();
    if header.phase != BuildPhase::Building {
        return Err(StoreError::State(
            "admission requires Building phase".into(),
        ));
    }
    let mut combined = MutationPlan::default();
    let mut file_versions = BTreeMap::<Vec<u8>, u64>::new();
    let mut coverage = BTreeMap::<(CollectionId, u32), value_codec::FieldCoverage>::new();
    let expected_digest = admission_digest(admission)?;
    let receipt_key = Namespace::BuildReceipts.physical(&admission.id.to_be_bytes());
    if let Some(existing) = preflight.get(Namespace::BuildReceipts, &admission.id.to_be_bytes())? {
        let receipt: BuildReceipt = value_codec::decode(&existing, "build receipt")?;
        if receipt.build_identity == header.build_identity
            && receipt.build_epoch == header.build_epoch
            && receipt.batch_id == admission.id
            && receipt.digest == expected_digest
        {
            return Ok(AdmissionExecutionStats {
                planning_ns: started.elapsed().as_nanos() as u64,
                ..AdmissionExecutionStats::default()
            });
        }
        return Err(StoreError::State(format!(
            "admission batch ID {} reused with different content",
            admission.id
        )));
    }
    drop(preflight);
    let mut receipt_slot_ranges = Vec::new();
    let mut receipt_files = Vec::new();
    let mut receipt_memberships = 0u32;
    let mut receipt_records = 0u32;
    let mut scratch = ContributionScratch::default();
    for prepared in &admission.files {
        let mut plan = MutationPlan::default();
        let mut slot_ranges = Vec::new();
        let mut records_total = 0u32;
        for mapped in &prepared.collections {
            let collection = mapped.collection;
            let runtime = schema.collection(collection)?;
            let batch = &mapped.batch;
            records_total += batch.records.len() as u32;
            slot_ranges.push((
                collection,
                mapped.base_slot,
                mapped.base_slot + batch.records.len() as u32,
            ));
            for (local, record) in batch.records.iter().enumerate() {
                let slot = RecordSlot(mapped.base_slot + local as u32);
                plan.puts.insert(
                    physical(Namespace::Records, keys::record(collection.0, slot.0)),
                    value_codec::encode(&value_codec::RecordMetadata::from_entry(record, 1))?,
                );
                plan.puts.insert(
                    physical(
                        Namespace::LogicalIds,
                        keys::logical_id(collection.0, &record.id.0),
                    ),
                    {
                        let mut value = super::key_codec::KeyWriter::with_capacity(12);
                        value.push_u32(slot.0);
                        value.push_u64(1);
                        value.into_bytes()
                    },
                );
                plan.puts.insert(
                    physical(
                        Namespace::FileRecords,
                        keys::file_record(prepared.file.id, collection.0, slot.0),
                    ),
                    Vec::new(),
                );
                if batch.logical_records.contains(local as u32) {
                    plan.puts.insert(
                        physical(
                            Namespace::DomainMembers,
                            keys::domain_member(
                                collection.0,
                                value_codec::DomainKind::Logical,
                                slot.0,
                            ),
                        ),
                        Vec::new(),
                    );
                }
                if batch.empty_array_records.contains(local as u32) {
                    plan.puts.insert(
                        physical(
                            Namespace::DomainMembers,
                            keys::domain_member(
                                collection.0,
                                value_codec::DomainKind::EmptyArray,
                                slot.0,
                            ),
                        ),
                        Vec::new(),
                    );
                }
                if record.class == RecordClass::RawFallback {
                    plan.puts.insert(
                        physical(
                            Namespace::DomainMembers,
                            keys::domain_member(
                                collection.0,
                                value_codec::DomainKind::RawFallback,
                                slot.0,
                            ),
                        ),
                        Vec::new(),
                    );
                }
            }
            let mut values = BTreeMap::<u32, Vec<(u32, crate::posting::ScalarKey)>>::new();
            for value in &batch.values {
                add_block(
                    &mut plan.values,
                    Namespace::ValueBlocks,
                    keys::value_block(
                        collection.0,
                        value.binding,
                        &value.value,
                        value.slot >> BLOCK_SHIFT,
                    )?,
                    value.slot,
                );
                values
                    .entry(value.slot)
                    .or_default()
                    .push((value.binding, value.value.clone()));
            }
            let mut lengths = BTreeMap::<u32, Vec<(u32, crate::model::Number)>>::new();
            for length in &batch.lengths {
                let scalar = crate::posting::ScalarKey::Number(length.length.clone());
                add_block(
                    &mut plan.lengths,
                    Namespace::LengthBlocks,
                    keys::length_block(
                        collection.0,
                        length.binding,
                        &scalar,
                        length.slot >> BLOCK_SHIFT,
                    )?,
                    length.slot,
                );
                lengths
                    .entry(length.slot)
                    .or_default()
                    .push((length.binding, length.length.clone()));
            }
            let mut states = BTreeMap::<u32, BTreeMap<u32, u16>>::new();
            for state in &batch.states {
                for tag in 0..=super::value_codec::STATE_EMPTY_OBJECT {
                    if state.mask & (1 << tag) == 0 {
                        continue;
                    }
                    let state_kind = value_codec::StateKind::from_tag(tag)
                        .ok_or_else(|| StoreError::CorruptIndex("unknown state tag".into()))?;
                    add_block(
                        &mut plan.states,
                        Namespace::StateBlocks,
                        keys::state_block(
                            collection.0,
                            state.binding,
                            state_kind,
                            state.slot >> BLOCK_SHIFT,
                        ),
                        state.slot,
                    );
                }
                *states
                    .entry(state.slot)
                    .or_default()
                    .entry(state.binding)
                    .or_insert(0) |= state.mask;
            }
            let mut grams = BTreeMap::<(u32, u32, GramKind), Vec<u64>>::new();
            for gram_set in &batch.grams {
                for gram in &gram_set.grams {
                    add_block(
                        &mut plan.fts,
                        Namespace::FtsBlocks,
                        keys::fts_key(
                            collection.0,
                            gram_set.binding,
                            gram_set.kind,
                            *gram,
                            gram_set.slot >> BLOCK_SHIFT,
                        ),
                        gram_set.slot,
                    );
                }
                grams.insert(
                    (gram_set.slot, gram_set.binding, gram_set.kind),
                    gram_set.grams.clone(),
                );
            }
            for (local, record) in batch.records.iter().enumerate() {
                let slot = mapped.base_slot + local as u32;
                let mut contribution = StoredContributions {
                    record: value_codec::RecordMetadata::from_entry(record, 1),
                    values: values.remove(&slot).unwrap_or_default(),
                    lengths: lengths.remove(&slot).unwrap_or_default(),
                    state_masks: states
                        .remove(&slot)
                        .unwrap_or_default()
                        .into_iter()
                        .collect(),
                    coverage: Vec::new(),
                    domains: Vec::new(),
                    sorts: Vec::new(),
                    relations: Vec::new(),
                    grams: Vec::new(),
                };
                if batch.logical_records.contains(local as u32) {
                    contribution.domains.push(value_codec::DomainKind::Logical);
                }
                if batch.empty_array_records.contains(local as u32) {
                    contribution
                        .domains
                        .push(value_codec::DomainKind::EmptyArray);
                }
                if record.class == RecordClass::RawFallback {
                    contribution
                        .domains
                        .push(value_codec::DomainKind::RawFallback);
                }
                for sort in batch.sorts.iter().filter(|sort| sort.slot == slot) {
                    let ids = runtime.sort_ids.get(&sort.path).ok_or_else(|| {
                        StoreError::State(format!(
                            "sort field {} is not configured",
                            sort.path.display()
                        ))
                    })?;
                    for (id, direction) in [
                        (ids.0, value_codec::SortDirection::Ascending),
                        (ids.1, value_codec::SortDirection::Descending),
                    ] {
                        let definition = runtime
                            .sort_definitions
                            .iter()
                            .find(|definition| definition.id == id)
                            .ok_or_else(|| {
                                StoreError::CorruptIndex("sort definition missing".into())
                            })?;
                        let projection = value_codec::SortProjection {
                            definition: id,
                            key: vec![sort.key.clone()],
                        };
                        let order = encode_sort_projection(&projection, definition)?;
                        plan.puts.insert(
                            physical(
                                Namespace::SortEntries,
                                keys::sort_entry(collection.0, id, &order, &record.id.0, slot),
                            ),
                            Vec::new(),
                        );
                        contribution.sorts.push(projection);
                        let _ = direction;
                    }
                }
                for relation in batch
                    .relations
                    .iter()
                    .filter(|relation| relation.slot == slot)
                {
                    let ordinal = *runtime.relation_ids.get(&relation.path).ok_or_else(|| {
                        StoreError::State(format!(
                            "relation {} is not configured",
                            relation.path.join(".")
                        ))
                    })?;
                    let target_collection = schema
                        .by_name
                        .get(&relation.target_collection)
                        .copied()
                        .ok_or_else(|| {
                            StoreError::State("relation target collection missing".into())
                        })?;
                    let occurrence = value_codec::RelationOccurrence {
                        declaration_ordinal: ordinal,
                        occurrence_ordinal: relation.occurrence_ordinal,
                        target_collection,
                        target_id: relation.target_id.clone(),
                        positions: relation.positions.clone(),
                    };
                    let outgoing_key = keys::relation_outgoing(
                        collection.0,
                        ordinal,
                        slot,
                        occurrence.occurrence_ordinal,
                    );
                    plan.puts.insert(
                        physical(Namespace::RelationOutgoing, outgoing_key),
                        encode_outgoing(&occurrence)?,
                    );
                    plan.puts.insert(
                        physical(
                            Namespace::RelationIncoming,
                            keys::relation_incoming(
                                target_collection.0,
                                &occurrence.target_id,
                                collection.0,
                                ordinal,
                                slot,
                                occurrence.occurrence_ordinal,
                            ),
                        ),
                        encode_positions(&occurrence.positions),
                    );
                    contribution.relations.push(occurrence);
                }
                for relation_state in batch
                    .relation_states
                    .iter()
                    .filter(|state| state.slot == slot)
                {
                    let ordinal =
                        *runtime
                            .relation_ids
                            .get(&relation_state.path)
                            .ok_or_else(|| {
                                StoreError::State("relation state declaration missing".into())
                            })?;
                    plan.puts.insert(
                        physical(
                            Namespace::RelationStates,
                            keys::relation_state(collection.0, ordinal, relation_state.state, slot),
                        ),
                        Vec::new(),
                    );
                }
                for ((gram_slot, binding, kind), list) in grams.iter() {
                    if *gram_slot == slot {
                        contribution.grams.push(value_codec::GramProjection {
                            binding: *binding,
                            kind: *kind,
                            grams: list.clone(),
                        });
                    }
                }
                contribution.normalize();
                plan.puts.insert(
                    physical(
                        Namespace::RecordContributions,
                        keys::record(collection.0, slot),
                    ),
                    scratch.encode(&contribution)?.to_vec(),
                );
                for gram_set in &contribution.grams {
                    plan.puts.insert(
                        physical(
                            Namespace::RecordGrams,
                            keys::record_grams(collection.0, slot, gram_set.binding, gram_set.kind),
                        ),
                        value_codec::encode_gram_list(&gram_set.grams),
                    );
                }
            }
            for (binding, batch_coverage) in &batch.coverage {
                let entry = coverage
                    .entry((collection, *binding))
                    .or_insert_with(|| batch_coverage.clone());
                entry.value_complete &= batch_coverage.value_complete;
                entry.length_complete &= batch_coverage.length_complete;
                entry.empty_string_complete &= batch_coverage.empty_string_complete;
            }
            let pending = physical(
                Namespace::PendingFiles,
                keys::pending_file(collection.0, prepared.file.id),
            );
            plan.required_deletes.insert(pending);
        }

        plan.puts.insert(
            physical(Namespace::Files, keys::file(prepared.file.id)),
            value_codec::encode(&FileMetadata {
                entry: prepared.file.clone(),
                file_version: prepared.file_version,
                fingerprint: prepared.fingerprint,
            })?,
        );
        let native = crate::path_repr::native_bytes(prepared.file.relative_path.as_path());
        plan.puts.insert(
            physical(Namespace::Paths, native),
            prepared.file.id.0.to_be_bytes().to_vec(),
        );
        let query = crate::path_repr::query_path(prepared.file.relative_path.as_path());
        plan.puts.insert(
            physical(
                Namespace::QueryPaths,
                keys::query_path(&query, prepared.file.id),
            ),
            Vec::new(),
        );
        let state = if prepared.file.status == crate::inventory::FileStatus::Unreadable {
            BuildFileState::Unavailable
        } else {
            BuildFileState::Indexed
        };
        plan.puts.insert(
            physical(Namespace::FileBuildState, keys::file(prepared.file.id)),
            value_codec::encode(&FileBuildState {
                state,
                expected_file_version: prepared.file_version,
                build_epoch: header.build_epoch,
                last_admission_revision: admission.id,
                error: None,
            })?,
        );
        receipt_files.push((prepared.file.id, prepared.file_version));
        receipt_slot_ranges.extend(slot_ranges);
        receipt_memberships = receipt_memberships.saturating_add(prepared.collections.len() as u32);
        receipt_records = receipt_records.saturating_add(records_total);
        file_versions.insert(
            physical(Namespace::FileBuildState, keys::file(prepared.file.id)),
            prepared.file_version,
        );
        combined.merge(plan);
    }
    for ((collection, binding), coverage) in coverage {
        let delta = FieldCoverageDelta {
            collection,
            binding,
            batch_id: admission.id,
            coverage,
        };
        let mut key = super::key_codec::KeyWriter::with_capacity(16);
        key.push_u64(admission.id);
        key.push_u32(collection.0);
        key.push_u32(binding);
        combined.puts.insert(
            physical(Namespace::FieldCoverageDeltas, key.into_bytes()),
            value_codec::encode(&delta)?,
        );
    }
    combined.puts.insert(
        receipt_key.clone(),
        value_codec::encode(&BuildReceipt {
            build_identity: header.build_identity,
            build_epoch: header.build_epoch,
            batch_id: admission.id,
            digest: expected_digest,
            files: receipt_files,
            slot_ranges: receipt_slot_ranges,
            file_memberships: receipt_memberships,
            records: receipt_records,
        })?,
    );
    let mut stats = AdmissionExecutionStats {
        planning_ns: started.elapsed().as_nanos() as u64,
        ..AdmissionExecutionStats::default()
    };
    let transaction_started = Instant::now();

    let transaction = store.transaction();
    let header_key = Namespace::StoreMetadata.physical(b"header");
    let header_bytes = transaction
        .get_for_update(&header_key, false)?
        .ok_or_else(|| StoreError::Incompatible("missing prototype header".into()))?;
    let current_header: value_codec::StoreHeader =
        value_codec::decode(&header_bytes, "store header")?;
    if current_header.phase != BuildPhase::Building
        || current_header.build_identity != header.build_identity
        || current_header.build_epoch != header.build_epoch
    {
        return Err(StoreError::StaleHandle(
            "build gate changed before admission".into(),
        ));
    }
    if let Some(existing) = transaction.get_for_update(&receipt_key, true)? {
        let receipt: BuildReceipt = value_codec::decode(&existing, "build receipt")?;
        if receipt.build_identity == current_header.build_identity
            && receipt.build_epoch == current_header.build_epoch
            && receipt.batch_id == admission.id
            && receipt.digest == expected_digest
        {
            stats.database_ns = transaction_started.elapsed().as_nanos() as u64;
            return Ok(stats);
        }
        return Err(StoreError::State(format!(
            "admission batch ID {} reused with different content",
            admission.id
        )));
    }

    let mut all_keys = combined
        .values
        .keys()
        .chain(combined.lengths.keys())
        .chain(combined.states.keys())
        .chain(combined.fts.keys())
        .chain(combined.puts.keys())
        .chain(combined.required_deletes.iter())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    all_keys.sort();
    for key in all_keys {
        stats.keys = stats.keys.saturating_add(1);
        stats.encoded_bytes = stats.encoded_bytes.saturating_add(key.len() as u64);
        if let Some(expected_file_version) = file_versions.get(&key) {
            let bytes = transaction
                .get_for_update(&key, true)?
                .ok_or_else(|| StoreError::State("file build state missing".into()))?;
            let existing: FileBuildState = value_codec::decode(&bytes, "file build state")?;
            if existing.expected_file_version != *expected_file_version
                || existing.build_epoch != header.build_epoch
            {
                return Err(StoreError::StaleHandle("file build state changed".into()));
            }
        }
        if combined.required_deletes.contains(&key) {
            if transaction.get_for_update(&key, true)?.is_none() {
                return Err(StoreError::StaleHandle(
                    "pending file membership disappeared".into(),
                ));
            }
            transaction.delete(&key)?;
            continue;
        }
        for (blocks, namespace) in [
            (&combined.values, Namespace::ValueBlocks),
            (&combined.lengths, Namespace::LengthBlocks),
            (&combined.states, Namespace::StateBlocks),
            (&combined.fts, Namespace::FtsBlocks),
        ] {
            if blocks.contains_key(&key) {
                stats.block_reads = stats.block_reads.saturating_add(1);
                let existing = transaction.get_for_update(&key, true)?;
                let mut block = existing
                    .as_deref()
                    .map(decode_block)
                    .transpose()?
                    .unwrap_or_default();
                for offset in &blocks[&key] {
                    block.set(*offset);
                }
                let encoded = encode_block(&block)?;
                stats.encoded_bytes = stats.encoded_bytes.saturating_add(encoded.len() as u64);
                transaction.put(&key, encoded)?;
                let _ = namespace;
            }
        }
        if let Some(value) = combined.puts.get(&key) {
            transaction.put(&key, value)?;
            stats.encoded_bytes = stats.encoded_bytes.saturating_add(value.len() as u64);
        }
    }
    stats.database_ns = transaction_started.elapsed().as_nanos() as u64;
    let commit_started = Instant::now();
    transaction.commit()?;
    stats.commit_ns = commit_started.elapsed().as_nanos() as u64;
    Ok(stats)
}

fn admission_digest(admission: &AdmissionBatch) -> StoreResult<[u8; 32]> {
    let mut digest = Sha256::new();
    digest.update(b"admission-batch-v2");
    digest.update(admission.id.to_be_bytes());
    for prepared in &admission.files {
        digest.update(prepared.file.id.0.to_be_bytes());
        digest.update(prepared.file_version.to_be_bytes());
        for mapped in &prepared.collections {
            digest.update(mapped.collection.0.to_be_bytes());
            digest.update(mapped.base_slot.to_be_bytes());
            digest_fact(&mut digest, &mapped.batch.records)?;
            digest_fact(&mut digest, &mapped.batch.values)?;
            digest_fact(&mut digest, &mapped.batch.lengths)?;
            digest_fact(&mut digest, &mapped.batch.states)?;
            digest_fact(&mut digest, &mapped.batch.sorts)?;
            digest_fact(&mut digest, &mapped.batch.relations)?;
            digest_fact(&mut digest, &mapped.batch.relation_states)?;
            digest_fact(&mut digest, &mapped.batch.grams)?;
            digest_fact(&mut digest, &mapped.batch.coverage)?;
        }
    }
    Ok(digest.finalize().into())
}

fn digest_fact<T: serde::Serialize>(digest: &mut Sha256, value: &T) -> StoreResult<()> {
    let bytes = bincode_options()
        .serialize(value)
        .map_err(|error| StoreError::CorruptValue(format!("admission digest: {error}")))?;
    digest.update((bytes.len() as u64).to_le_bytes());
    digest.update(bytes);
    Ok(())
}

fn encode_sort_projection(
    projection: &value_codec::SortProjection,
    definition: &value_codec::SortDefinition,
) -> StoreResult<Vec<u8>> {
    if projection.key.len() != definition.constituents.len() {
        return Err(StoreError::CorruptIndex(
            "sort projection cardinality mismatch".into(),
        ));
    }
    let mut writer = super::key_codec::KeyWriter::with_capacity(32);
    for (key, constituent) in projection.key.iter().zip(&definition.constituents) {
        writer.push_sort_key(
            key,
            constituent.direction == value_codec::SortDirection::Descending,
            constituent.options,
        );
    }
    Ok(writer.into_bytes())
}

fn encode_outgoing(occurrence: &value_codec::RelationOccurrence) -> StoreResult<Vec<u8>> {
    let mut key = super::key_codec::KeyWriter::with_capacity(16 + occurrence.target_id.len());
    key.push_u32(occurrence.target_collection.0);
    key.push_str(&occurrence.target_id);
    let positions = encode_positions(&occurrence.positions);
    key.bytes.extend_from_slice(&positions);
    Ok(key.into_bytes())
}

fn encode_positions(positions: &[u32]) -> Vec<u8> {
    let mut key = super::key_codec::KeyWriter::with_capacity(4 + positions.len() * 4);
    key.push_u32(positions.len() as u32);
    for position in positions {
        key.push_u32(*position);
    }
    key.into_bytes()
}
