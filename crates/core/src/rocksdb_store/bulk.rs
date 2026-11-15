//! Fresh-build complete-image accumulation and atomic batch publication.

use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use rocksdb::WriteBatchWithTransaction;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::inventory::RecordClass;
use crate::model::{CollectionId, RecordSlot};
use crate::posting::ScalarKey;

use super::bitmap_block::{BLOCK_MASK, BLOCK_SHIFT, Block, encode_block};
use super::contribution_codec::ContributionScratch;
use super::namespace::Namespace;
use super::prepare::{PreparedRelation, PreparedRelationState, PreparedSort};
use super::schema::RuntimeSchema;
use super::value_codec::{
    BuildFileState, BuildReceipt, FieldCoverage, FileBuildState, FileMetadata, GramKind,
    RelationOccurrence, SortProjection, StateKind, StoredContributions,
};
use super::write::{AdmissionBatch, PhysicalPreparedFile};
use super::{RocksStore, StoreError, StoreResult, keys, value_codec};

pub(crate) const MAX_COHORT_FILES: usize = 4_096;
pub(crate) const MAX_COHORT_SOURCE_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Clone, Debug, Default, Serialize)]
pub struct BulkConstructionReport {
    pub cohorts: u64,
    pub files: u64,
    pub memberships: u64,
    pub records: u64,
    pub actions: u64,
    pub static_actions: u64,
    pub bitmap_keys: u64,
    pub input_bitmap_memberships: u64,
    pub bitmap_puts: u64,
    pub bitmap_reads: u64,
    pub cold_reloads: u64,
    pub physical_key_encodes: u64,
    pub plan_bytes: u64,
    pub native_batch_bytes: u64,
    pub accumulation_ns: u64,
    pub encoding_ns: u64,
    pub digest_ns: u64,
    pub write_ns: u64,
    pub retries: u64,
    pub first_file_commit_ns: Option<u64>,
    pub peak_tail_keys: usize,
    pub peak_tail_bytes: u64,
    pub first_file_partial_keys: u64,
    pub seal_reasons: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum BitmapPosting {
    Value(ScalarKey),
    Length(ScalarKey),
    State(StateKind),
    Fts(GramKind, u64),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct BitmapKey {
    collection: u32,
    binding: u32,
    block: u32,
    posting: BitmapPosting,
}

#[derive(Default)]
struct BlockImage {
    offsets: Vec<u16>,
    dirty: bool,
}

#[derive(Default)]
struct CohortPlan {
    actions: BTreeMap<Vec<u8>, Vec<u8>>,
    receipt_files: Vec<(crate::model::FileId, u64)>,
    receipt_slot_ranges: Vec<(CollectionId, u32, u32)>,
    receipt_memberships: u32,
    receipt_records: u32,
    coverage: Vec<(CollectionId, u32, FieldCoverage)>,
    static_actions: u64,
    bitmap_keys: u64,
    input_bitmap_memberships: u64,
    physical_key_encodes: u64,
    touched_tails: Vec<BitmapKey>,
}

impl CohortPlan {
    fn put_static(&mut self, key: Vec<u8>, value: Vec<u8>) -> StoreResult<()> {
        let replace = super::namespace::namespace_of(&key) == Some(Namespace::LogicalIds);
        match self.actions.get(&key) {
            None => {
                self.actions.insert(key, value);
            }
            Some(existing) if existing == &value => {}
            Some(_) if replace => {
                self.actions.insert(key, value);
            }
            Some(_) => {
                return Err(StoreError::State(format!(
                    "conflicting construction actions for {:02x?}",
                    key
                )));
            }
        }
        self.static_actions = self.static_actions.saturating_add(1);
        Ok(())
    }
}

pub(crate) struct BulkConstructor<'a> {
    store: &'a RocksStore,
    schema: &'a RuntimeSchema,
    build_identity: [u8; 32],
    build_epoch: u64,
    next_cohort_id: u64,
    tails: HashMap<BitmapKey, BlockImage>,
    contribution: ContributionScratch,
    report: BulkConstructionReport,
    started: Instant,
}

impl<'a> BulkConstructor<'a> {
    pub(crate) fn new(
        store: &'a RocksStore,
        schema: &'a RuntimeSchema,
        build_identity: [u8; 32],
        build_epoch: u64,
    ) -> Self {
        Self {
            store,
            schema,
            build_identity,
            build_epoch,
            next_cohort_id: 1,
            tails: HashMap::new(),
            contribution: ContributionScratch::default(),
            report: BulkConstructionReport::default(),
            started: Instant::now(),
        }
    }

    pub(crate) fn publish(
        &mut self,
        files: Vec<PhysicalPreparedFile>,
        next_slots: &BTreeMap<u32, u32>,
        final_cohort: bool,
        seal_reason: &str,
    ) -> StoreResult<()> {
        if files.is_empty() {
            return Ok(());
        }
        let accumulation_started = Instant::now();
        let admission = AdmissionBatch {
            id: self.next_cohort_id,
            files,
        };
        self.next_cohort_id = self
            .next_cohort_id
            .checked_add(1)
            .ok_or_else(|| StoreError::State("construction cohort ID overflow".into()))?;
        let mut plan = CohortPlan::default();
        self.prepare_forward_and_files(&admission, &mut plan)?;
        let accumulation_ns = elapsed_ns(accumulation_started);

        let encoding_started = Instant::now();
        let touched = std::mem::take(&mut plan.touched_tails);
        let mut bitmap_puts = 0u64;
        for key in &touched {
            let image = self
                .tails
                .get(key)
                .ok_or_else(|| StoreError::State("construction bitmap ownership lost".into()))?;
            if !image.dirty {
                continue;
            }
            let physical_key = self.physical_key(key)?;
            let mut block = Block::default();
            for offset in &image.offsets {
                block.set(*offset);
            }
            let encoded = encode_block(&block)?;
            plan.actions.insert(physical_key, encoded);
            plan.physical_key_encodes = plan.physical_key_encodes.saturating_add(1);
            bitmap_puts = bitmap_puts.saturating_add(1);
        }
        let encoding_ns = elapsed_ns(encoding_started);

        let digest_started = Instant::now();
        let digest = self.plan_digest(&admission, &plan)?;
        let digest_ns = elapsed_ns(digest_started);
        let receipt = BuildReceipt {
            build_identity: self.build_identity,
            build_epoch: self.build_epoch,
            batch_id: admission.id,
            digest,
            files: plan.receipt_files.clone(),
            slot_ranges: plan.receipt_slot_ranges.clone(),
            file_memberships: plan.receipt_memberships,
            records: plan.receipt_records,
            coverage: plan.coverage.clone(),
        };
        let receipt_key = Namespace::BuildReceipts
            .physical(&admission.id.to_be_bytes())
            .to_vec();
        let receipt_value = value_codec::encode(&receipt)?;

        let write_started = Instant::now();
        let committed = self.submit(&receipt_key, &receipt, &plan, receipt_value)?;
        let write_ns = elapsed_ns(write_started);

        for key in &touched {
            if let Some(image) = self.tails.get_mut(key) {
                image.dirty = false;
            }
        }
        let next_tail_bytes = self.retain_or_release_tails(next_slots, final_cohort);
        let plan_bytes = plan
            .actions
            .iter()
            .map(|(key, value)| key.len().saturating_add(value.len()))
            .sum::<usize>() as u64;

        self.report.cohorts = self.report.cohorts.saturating_add(1);
        self.report.files = self
            .report
            .files
            .saturating_add(admission.files.len() as u64);
        self.report.memberships = self
            .report
            .memberships
            .saturating_add(plan.receipt_memberships as u64);
        self.report.records = self
            .report
            .records
            .saturating_add(plan.receipt_records as u64);
        self.report.actions = self
            .report
            .actions
            .saturating_add(plan.actions.len() as u64);
        self.report.static_actions = self
            .report
            .static_actions
            .saturating_add(plan.static_actions);
        self.report.bitmap_keys = self.report.bitmap_keys.saturating_add(plan.bitmap_keys);
        self.report.input_bitmap_memberships = self
            .report
            .input_bitmap_memberships
            .saturating_add(plan.input_bitmap_memberships);
        self.report.bitmap_puts = self.report.bitmap_puts.saturating_add(bitmap_puts);
        self.report.physical_key_encodes = self
            .report
            .physical_key_encodes
            .saturating_add(plan.physical_key_encodes);
        self.report.plan_bytes = self.report.plan_bytes.saturating_add(plan_bytes);
        self.report.accumulation_ns = self.report.accumulation_ns.saturating_add(accumulation_ns);
        self.report.encoding_ns = self.report.encoding_ns.saturating_add(encoding_ns);
        self.report.digest_ns = self.report.digest_ns.saturating_add(digest_ns);
        self.report.write_ns = self.report.write_ns.saturating_add(write_ns);
        self.report.peak_tail_keys = self.report.peak_tail_keys.max(self.tails.len());
        self.report.peak_tail_bytes = self.report.peak_tail_bytes.max(next_tail_bytes);
        if self.report.first_file_commit_ns.is_none() {
            self.report.first_file_commit_ns = Some(elapsed_ns(self.started));
            if admission.files.len() == 1 {
                self.report.first_file_partial_keys = plan.bitmap_keys;
            }
        }
        *self
            .report
            .seal_reasons
            .entry(seal_reason.to_owned())
            .or_default() += 1;
        let _ = committed;
        Ok(())
    }

    pub(crate) fn finish(mut self) -> BulkConstructionReport {
        self.contribution = ContributionScratch::default();
        self.tails.clear();
        self.report
    }

    fn prepare_forward_and_files(
        &mut self,
        admission: &AdmissionBatch,
        plan: &mut CohortPlan,
    ) -> StoreResult<()> {
        let mut local_coverage = BTreeMap::<(CollectionId, u32), FieldCoverage>::new();
        for prepared in &admission.files {
            let mut slot_ranges = Vec::new();
            let mut file_records = 0u32;
            for mapped in &prepared.collections {
                let collection = mapped.collection;
                let runtime = self.schema.collection(collection)?;
                let batch = &mapped.batch;
                file_records = file_records.saturating_add(batch.records.len() as u32);
                slot_ranges.push((
                    collection,
                    mapped.base_slot,
                    mapped.base_slot.saturating_add(batch.records.len() as u32),
                ));
                for value in &batch.values {
                    self.add_bitmap(
                        BitmapKey {
                            collection: collection.0,
                            binding: value.binding,
                            block: value.slot >> BLOCK_SHIFT,
                            posting: BitmapPosting::Value(value.value.clone()),
                        },
                        value.slot,
                        plan,
                    );
                }
                for length in &batch.lengths {
                    self.add_bitmap(
                        BitmapKey {
                            collection: collection.0,
                            binding: length.binding,
                            block: length.slot >> BLOCK_SHIFT,
                            posting: BitmapPosting::Length(ScalarKey::Number(
                                length.length.clone(),
                            )),
                        },
                        length.slot,
                        plan,
                    );
                }
                for state in &batch.states {
                    for tag in 0..=value_codec::STATE_EMPTY_OBJECT {
                        if state.mask & (1 << tag) == 0 {
                            continue;
                        }
                        let kind = StateKind::from_tag(tag)
                            .ok_or_else(|| StoreError::CorruptIndex("unknown state tag".into()))?;
                        self.add_bitmap(
                            BitmapKey {
                                collection: collection.0,
                                binding: state.binding,
                                block: state.slot >> BLOCK_SHIFT,
                                posting: BitmapPosting::State(kind),
                            },
                            state.slot,
                            plan,
                        );
                    }
                }
                for gram_set in &batch.grams {
                    for gram in &gram_set.grams {
                        self.add_bitmap(
                            BitmapKey {
                                collection: collection.0,
                                binding: gram_set.binding,
                                block: gram_set.slot >> BLOCK_SHIFT,
                                posting: BitmapPosting::Fts(gram_set.kind, *gram),
                            },
                            gram_set.slot,
                            plan,
                        );
                    }
                }
                self.add_record_facts(&prepared.file, admission.id, mapped, runtime, plan)?;
                for (binding, coverage) in &batch.descriptors.coverage {
                    let binding = mapped.binding_ids[*binding as usize];
                    let aggregate = local_coverage
                        .entry((collection, binding))
                        .or_insert_with(|| coverage.clone());
                    aggregate.value_complete &= coverage.value_complete;
                    aggregate.length_complete &= coverage.length_complete;
                    aggregate.empty_string_complete &= coverage.empty_string_complete;
                }
            }
            plan.put_static(
                super::write::physical_key(Namespace::Files, keys::file(prepared.file.id)),
                value_codec::encode(&FileMetadata {
                    entry: prepared.file.clone(),
                    file_version: prepared.file_version,
                    fingerprint: prepared.fingerprint,
                })?,
            )?;
            plan.put_static(
                super::write::physical_key(
                    Namespace::Paths,
                    crate::path_repr::native_bytes(prepared.file.relative_path.as_path()),
                ),
                prepared.file.id.0.to_be_bytes().to_vec(),
            )?;
            let query = crate::path_repr::query_path(prepared.file.relative_path.as_path());
            plan.put_static(
                super::write::physical_key(
                    Namespace::QueryPaths,
                    keys::query_path(&query, prepared.file.id),
                ),
                Vec::new(),
            )?;
            let state = if prepared.file.status == crate::inventory::FileStatus::Unreadable {
                BuildFileState::Unavailable
            } else {
                BuildFileState::Indexed
            };
            plan.put_static(
                super::write::physical_key(Namespace::FileBuildState, keys::file(prepared.file.id)),
                value_codec::encode(&FileBuildState {
                    state,
                    expected_file_version: prepared.file_version,
                    build_epoch: self.build_epoch,
                    last_admission_revision: admission.id,
                    error: None,
                })?,
            )?;
            plan.receipt_files
                .push((prepared.file.id, prepared.file_version));
            plan.receipt_slot_ranges.extend(slot_ranges);
            plan.receipt_memberships = plan
                .receipt_memberships
                .saturating_add(prepared.collections.len() as u32);
            plan.receipt_records = plan.receipt_records.saturating_add(file_records);
        }
        plan.coverage = local_coverage
            .into_iter()
            .map(|((collection, binding), coverage)| (collection, binding, coverage))
            .collect();
        Ok(())
    }

    fn add_record_facts(
        &mut self,
        file: &crate::inventory::FileEntry,
        cohort_id: u64,
        mapped: &super::write::MappedCollectionBatch,
        runtime: &super::schema::CollectionRuntime,
        plan: &mut CohortPlan,
    ) -> StoreResult<()> {
        let collection = mapped.collection;
        let batch = &mapped.batch;
        let mut value_index = 0usize;
        let mut length_index = 0usize;
        let mut state_index = 0usize;
        let mut sort_index = 0usize;
        let mut relation_index = 0usize;
        let mut relation_state_index = 0usize;
        let mut gram_index = 0usize;
        for (local, record) in batch.records.iter().enumerate() {
            let slot = RecordSlot(mapped.base_slot + local as u32);
            plan.put_static(
                super::write::physical_key(Namespace::Records, keys::record(collection.0, slot.0)),
                value_codec::encode(&value_codec::RecordMetadata::from_entry(record, 1))?,
            )?;
            let mut logical_value = super::key_codec::KeyWriter::with_capacity(12);
            logical_value.push_u32(slot.0);
            logical_value.push_u64(1);
            plan.put_static(
                super::write::physical_key(
                    Namespace::LogicalIds,
                    keys::logical_id(collection.0, &record.id.0),
                ),
                logical_value.into_bytes(),
            )?;
            plan.put_static(
                super::write::physical_key(
                    Namespace::FileRecords,
                    keys::file_record(file.id, collection.0, slot.0),
                ),
                Vec::new(),
            )?;
            for (kind, present) in [
                (
                    value_codec::DomainKind::Logical,
                    batch.logical_records.contains(local as u32),
                ),
                (
                    value_codec::DomainKind::EmptyArray,
                    batch.empty_array_records.contains(local as u32),
                ),
                (
                    value_codec::DomainKind::RawFallback,
                    record.class == RecordClass::RawFallback,
                ),
            ] {
                if present {
                    plan.put_static(
                        super::write::physical_key(
                            Namespace::DomainMembers,
                            keys::domain_member(collection.0, kind, slot.0),
                        ),
                        Vec::new(),
                    )?;
                }
            }

            let values_start =
                advance_group(&batch.values, &mut value_index, slot.0, |fact| fact.slot)?;
            let values_end = group_end(&batch.values, values_start, slot.0, |fact| fact.slot);
            let lengths_start =
                advance_group(&batch.lengths, &mut length_index, slot.0, |fact| fact.slot)?;
            let lengths_end = group_end(&batch.lengths, lengths_start, slot.0, |fact| fact.slot);
            let states_start =
                advance_group(&batch.states, &mut state_index, slot.0, |fact| fact.slot)?;
            let states_end = group_end(&batch.states, states_start, slot.0, |fact| fact.slot);
            let sorts_start =
                advance_group(&batch.sorts, &mut sort_index, slot.0, |fact| fact.slot)?;
            let sorts_end = sort_group_end(&batch.sorts, sorts_start, slot.0);
            let relations_start =
                advance_group(&batch.relations, &mut relation_index, slot.0, |fact| {
                    fact.slot
                })?;
            let relations_end = relation_group_end(&batch.relations, relations_start, slot.0);
            let relation_states_start = advance_group(
                &batch.relation_states,
                &mut relation_state_index,
                slot.0,
                |fact| fact.slot,
            )?;
            let relation_states_end =
                relation_state_group_end(&batch.relation_states, relation_states_start, slot.0);
            let grams_start =
                advance_group(&batch.grams, &mut gram_index, slot.0, |fact| fact.slot)?;
            let grams_end = gram_group_end(&batch.grams, grams_start, slot.0);

            let mut contribution = StoredContributions {
                record: value_codec::RecordMetadata::from_entry(record, 1),
                values: batch.values[values_start..values_end]
                    .iter()
                    .map(|fact| (fact.binding, fact.value.clone()))
                    .collect(),
                lengths: batch.lengths[lengths_start..lengths_end]
                    .iter()
                    .map(|fact| (fact.binding, fact.length.clone()))
                    .collect(),
                state_masks: batch.states[states_start..states_end]
                    .iter()
                    .map(|fact| (fact.binding, fact.mask))
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
            for sort in &batch.sorts[sorts_start..sorts_end] {
                self.add_sort_rows(
                    collection,
                    slot.0,
                    record,
                    sort,
                    runtime,
                    &mut contribution,
                    plan,
                )?;
            }
            for relation in &batch.relations[relations_start..relations_end] {
                self.add_relation_rows(
                    collection,
                    slot.0,
                    relation,
                    runtime,
                    &mut contribution,
                    plan,
                )?;
            }
            for relation_state in &batch.relation_states[relation_states_start..relation_states_end]
            {
                self.add_relation_state(collection, slot.0, relation_state, runtime, plan)?;
            }
            contribution.normalize();
            let encoded = self.contribution.encode(&contribution)?.to_vec();
            plan.put_static(
                super::write::physical_key(
                    Namespace::RecordContributions,
                    keys::record(collection.0, slot.0),
                ),
                encoded,
            )?;
            for gram in &batch.grams[grams_start..grams_end] {
                plan.put_static(
                    super::write::physical_key(
                        Namespace::RecordGrams,
                        keys::record_grams(collection.0, slot.0, gram.binding, gram.kind),
                    ),
                    value_codec::encode_gram_list(&gram.grams),
                )?;
            }
            let _ = cohort_id;
        }
        Ok(())
    }

    fn add_bitmap(&mut self, key: BitmapKey, slot: u32, plan: &mut CohortPlan) {
        let was_dirty = self.tails.get(&key).is_some_and(|image| image.dirty);
        if !was_dirty {
            plan.touched_tails.push(key.clone());
            plan.bitmap_keys = plan.bitmap_keys.saturating_add(1);
        }
        let image = self.tails.entry(key).or_default();
        image.offsets.push((slot & BLOCK_MASK) as u16);
        image.dirty = true;
        plan.input_bitmap_memberships = plan.input_bitmap_memberships.saturating_add(1);
    }

    fn add_sort_rows(
        &self,
        collection: CollectionId,
        slot: u32,
        record: &crate::inventory::RecordEntry,
        sort: &PreparedSort,
        runtime: &super::schema::CollectionRuntime,
        contribution: &mut StoredContributions,
        plan: &mut CohortPlan,
    ) -> StoreResult<()> {
        let ids = runtime.sort_ids.get(&sort.path).ok_or_else(|| {
            StoreError::State(format!(
                "sort field {} is not configured",
                sort.path.display()
            ))
        })?;
        for id in [ids.0, ids.1] {
            let definition = runtime
                .sort_definitions
                .iter()
                .find(|definition| definition.id == id)
                .ok_or_else(|| StoreError::CorruptIndex("sort definition missing".into()))?;
            let projection = SortProjection {
                definition: id,
                key: vec![sort.key.clone()],
            };
            let order = super::write::encode_sort_projection(&projection, definition)?;
            plan.put_static(
                super::write::physical_key(
                    Namespace::SortEntries,
                    keys::sort_entry(collection.0, id, &order, &record.id.0, slot),
                ),
                Vec::new(),
            )?;
            contribution.sorts.push(projection);
        }
        Ok(())
    }

    fn add_relation_rows(
        &self,
        collection: CollectionId,
        slot: u32,
        relation: &PreparedRelation,
        runtime: &super::schema::CollectionRuntime,
        contribution: &mut StoredContributions,
        plan: &mut CohortPlan,
    ) -> StoreResult<()> {
        let ordinal = *runtime.relation_ids.get(&relation.path).ok_or_else(|| {
            StoreError::State(format!(
                "relation {} is not configured",
                relation.path.join(".")
            ))
        })?;
        let target_collection = self
            .schema
            .by_name
            .get(&relation.target_collection)
            .copied()
            .ok_or_else(|| StoreError::State("relation target collection missing".into()))?;
        let occurrence = RelationOccurrence {
            declaration_ordinal: ordinal,
            occurrence_ordinal: relation.occurrence_ordinal,
            target_collection,
            target_id: relation.target_id.clone(),
            positions: relation.positions.clone(),
        };
        plan.put_static(
            super::write::physical_key(
                Namespace::RelationOutgoing,
                keys::relation_outgoing(collection.0, ordinal, slot, occurrence.occurrence_ordinal),
            ),
            super::write::encode_outgoing(&occurrence)?,
        )?;
        plan.put_static(
            super::write::physical_key(
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
            super::write::encode_positions(&occurrence.positions),
        )?;
        contribution.relations.push(occurrence);
        Ok(())
    }

    fn add_relation_state(
        &self,
        collection: CollectionId,
        slot: u32,
        state: &PreparedRelationState,
        runtime: &super::schema::CollectionRuntime,
        plan: &mut CohortPlan,
    ) -> StoreResult<()> {
        let ordinal = *runtime
            .relation_ids
            .get(&state.path)
            .ok_or_else(|| StoreError::State("relation state declaration missing".into()))?;
        plan.put_static(
            super::write::physical_key(
                Namespace::RelationStates,
                keys::relation_state(collection.0, ordinal, state.state, slot),
            ),
            Vec::new(),
        )
    }

    fn physical_key(&self, key: &BitmapKey) -> StoreResult<Vec<u8>> {
        let logical = match &key.posting {
            BitmapPosting::Value(value) => {
                keys::value_block(key.collection, key.binding, value, key.block)?
            }
            BitmapPosting::Length(value) => {
                keys::length_block(key.collection, key.binding, value, key.block)?
            }
            BitmapPosting::State(state) => {
                keys::state_block(key.collection, key.binding, *state, key.block)
            }
            BitmapPosting::Fts(kind, gram) => {
                keys::fts_key(key.collection, key.binding, *kind, *gram, key.block)
            }
        };
        let namespace = match key.posting {
            BitmapPosting::Value(_) => Namespace::ValueBlocks,
            BitmapPosting::Length(_) => Namespace::LengthBlocks,
            BitmapPosting::State(_) => Namespace::StateBlocks,
            BitmapPosting::Fts(_, _) => Namespace::FtsBlocks,
        };
        Ok(super::write::physical_key(namespace, logical))
    }

    fn retain_or_release_tails(
        &mut self,
        next_slots: &BTreeMap<u32, u32>,
        final_cohort: bool,
    ) -> u64 {
        self.tails.retain(|key, _| {
            if final_cohort {
                return false;
            }
            let end = ((key.block as u64) + 1) * (BLOCK_MASK as u64 + 1);
            next_slots
                .get(&key.collection)
                .is_none_or(|next| u64::from(*next) < end)
        });
        self.tails
            .values()
            .map(|image| {
                std::mem::size_of::<BitmapKey>()
                    .saturating_add(image.offsets.capacity() * std::mem::size_of::<u16>())
            })
            .fold(0usize, usize::saturating_add) as u64
    }

    fn plan_digest(&self, admission: &AdmissionBatch, plan: &CohortPlan) -> StoreResult<[u8; 32]> {
        let mut digest = Sha256::new();
        digest.update(b"rocksdb-bulk-cohort-v4");
        digest.update(self.build_identity);
        digest.update(self.build_epoch.to_be_bytes());
        digest.update(admission.id.to_be_bytes());
        digest.update((plan.receipt_files.len() as u64).to_be_bytes());
        for (file, version) in &plan.receipt_files {
            digest.update(file.0.to_be_bytes());
            digest.update(version.to_be_bytes());
        }
        digest.update((plan.receipt_slot_ranges.len() as u64).to_be_bytes());
        for (collection, start, end) in &plan.receipt_slot_ranges {
            digest.update(collection.0.to_be_bytes());
            digest.update(start.to_be_bytes());
            digest.update(end.to_be_bytes());
        }
        digest.update((plan.coverage.len() as u64).to_be_bytes());
        for (collection, binding, coverage) in &plan.coverage {
            digest.update(collection.0.to_be_bytes());
            digest.update(binding.to_be_bytes());
            digest.update([
                u8::from(coverage.values_indexed),
                u8::from(coverage.value_complete),
                u8::from(coverage.length_complete),
                u8::from(coverage.empty_string_complete),
            ]);
            digest.update(coverage.covered_records.to_be_bytes());
            digest.update(coverage.value_cardinality.to_be_bytes());
            digest.update(coverage.length_cardinality.to_be_bytes());
        }
        for (key, value) in &plan.actions {
            digest.update((key.len() as u64).to_be_bytes());
            digest.update(key);
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value);
        }
        Ok(digest.finalize().into())
    }

    fn submit(
        &mut self,
        receipt_key: &[u8],
        expected: &BuildReceipt,
        plan: &CohortPlan,
        receipt_value: Vec<u8>,
    ) -> StoreResult<bool> {
        for attempt in 0..4 {
            let view = self.store.view()?;
            if let Some(bytes) =
                view.get(Namespace::BuildReceipts, &expected.batch_id.to_be_bytes())?
            {
                let actual: BuildReceipt = value_codec::decode(&bytes, "build receipt")?;
                if actual.digest == expected.digest
                    && actual.build_identity == expected.build_identity
                    && actual.build_epoch == expected.build_epoch
                    && actual.batch_id == expected.batch_id
                {
                    return Ok(false);
                }
                return Err(StoreError::State(format!(
                    "construction cohort {} reused with different content",
                    expected.batch_id
                )));
            }
            drop(view);
            let mut batch = WriteBatchWithTransaction::<true>::default();
            for (key, value) in &plan.actions {
                batch.put(key, value);
            }
            batch.put(receipt_key, &receipt_value);
            self.report.native_batch_bytes = self.report.native_batch_bytes.saturating_add(
                plan.actions
                    .iter()
                    .map(|(key, value)| key.len().saturating_add(value.len()))
                    .sum::<usize>() as u64
                    + receipt_key.len() as u64
                    + receipt_value.len() as u64,
            );
            match self.store.atomic_write(batch) {
                Ok(()) => return Ok(true),
                Err(error) if error.is_contention() && attempt < 3 => {
                    self.report.retries = self.report.retries.saturating_add(1);
                    std::thread::sleep(std::time::Duration::from_millis(1u64 << attempt));
                }
                Err(error) => return Err(error),
            }
        }
        Err(StoreError::Database(
            "construction batch retry exhaustion".into(),
        ))
    }
}

fn advance_group<T>(
    facts: &[T],
    index: &mut usize,
    slot: u32,
    slot_of: impl Fn(&T) -> u32,
) -> StoreResult<usize> {
    while *index < facts.len() && slot_of(&facts[*index]) < slot {
        *index += 1;
    }
    if *index < facts.len() && slot_of(&facts[*index]) != slot {
        return Err(StoreError::CorruptIndex(
            "prepared facts are not associated with the expected record".into(),
        ));
    }
    Ok(*index)
}

fn group_end<T>(facts: &[T], start: usize, slot: u32, slot_of: impl Fn(&T) -> u32) -> usize {
    let mut end = start;
    while end < facts.len() && slot_of(&facts[end]) == slot {
        end += 1;
    }
    end
}

fn sort_group_end(facts: &[PreparedSort], start: usize, slot: u32) -> usize {
    group_end(facts, start, slot, |fact| fact.slot)
}

fn relation_group_end(facts: &[PreparedRelation], start: usize, slot: u32) -> usize {
    group_end(facts, start, slot, |fact| fact.slot)
}

fn relation_state_group_end(facts: &[PreparedRelationState], start: usize, slot: u32) -> usize {
    group_end(facts, start, slot, |fact| fact.slot)
}

fn gram_group_end(facts: &[super::prepare::PreparedGramSet], start: usize, slot: u32) -> usize {
    group_end(facts, start, slot, |fact| fact.slot)
}

fn elapsed_ns(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
}
