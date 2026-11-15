//! Temporary native-to-redb export for the phase-5 feasibility experiment.
//!
//! This module is compiled only with the `store-feasibility` feature. It reads
//! an already-built native repository and writes its index families into the
//! new tables. It is not a production path: the final migration feeds prepared
//! contributions directly from extraction and deletes this bridge.

use std::collections::HashMap;
use std::time::Instant;

use crate::index_store::value_codec::ConstructionState;
use crate::index_store::fts::{FtsBatch, apply_batch};
use crate::index_store::tables;
use crate::index_store::value_codec::{
    BindingDescriptor, BindingIdentity, BindingRole, BuildFileState, CollectionMetadata,
    FileBuildState, GramKind, RelationOccurrence, RelationStateKind, SortConstituent,
    SortDefinition, SortDirection, SortProjection, StateKind, StoredBinding, StoredContributions,
};
use crate::index_store::{
    IndexStore, StoreConfig, read, write,
};
use crate::model::{CollectionId, LogicalType, RecordSlot};
use redb::ReadableTableMetadata;
use crate::posting::ScalarKey;

#[derive(Clone, Debug, Default)]
pub struct FeasibilityReport {
    pub collections: usize,
    pub files: u64,
    pub records: u64,
    pub bindings: u64,
    pub value_members: u64,
    pub length_members: u64,
    pub state_members: u64,
    pub fts_bits: u64,
    pub fts_blocks: u64,
    pub fts_grams: u64,
    pub sort_definitions: u64,
    pub sort_entries: u64,
    pub relation_occurrences: u64,
    pub contribution_rows: u64,
    pub reverse_contribution_bytes: u64,
    pub state_blocks: u64,
    pub estimated_record_gram_bytes: u64,
    pub write_ns: u128,
    pub table_rows: Vec<(String, u64)>,
    pub table_bytes: Vec<(String, u64, u64)>,
    pub rss_before_bytes: u64,
    pub rss_after_bytes: u64,
    pub rss_peak_bytes: u64,
    pub store_bytes_after: u64,
}

impl FeasibilityReport {
    pub fn table_row(&self, name: &str) -> u64 {
        self.table_rows
            .iter()
            .find(|(table, _)| table == name)
            .map_or(0, |(_, rows)| *rows)
    }
}

struct BindingAllocator<'a> {
    tx: &'a redb::WriteTransaction,
    collection: CollectionId,
    next: u32,
}

impl BindingAllocator<'_> {
    fn allocate(
        &mut self,
        identity: BindingIdentity,
        role: BindingRole,
        kind: LogicalType,
        multi: bool,
        text_alphabet: Option<String>,
    ) -> Result<u32, String> {
        let id = self.next;
        self.next = self.next.checked_add(1).ok_or("binding overflow")?;
        let encoded = bincode::serialize(&identity).map_err(|error| error.to_string())?;
        write::put_binding(
            self.tx,
            self.collection,
            &encoded,
            &StoredBinding {
                id,
                descriptor: BindingDescriptor {
                    identity,
                    role,
                    kind,
                    multi,
                    text_alphabet,
                    text_segment: None,
                    fingerprint: [0; 32],
                },
            },
        )
        .map_err(|error| error.to_string())?;
        Ok(id)
    }
}

/// Build one in-memory store from an existing native index for measurement.
pub fn export_native_index(
    repository: &crate::repository::Repository,
    mut config: StoreConfig,
) -> Result<(std::sync::Arc<IndexStore>, FeasibilityReport), String> {
    if config.configuration_digest == [0; 32] {
        let definitions = repository
            .collections()
            .iter()
            .map(|collection| collection.definition.clone())
            .collect::<Vec<_>>();
        config.configuration_digest = crate::configuration::configuration_hash(
            &definitions,
            repository.options(),
            &repository.sources,
        )
        .map_err(|error| error.to_string())?;
    }
    let rss_before = resident_bytes();
    let store = std::sync::Arc::new(IndexStore::new(config).map_err(|error| error.to_string())?);
    let mut report = FeasibilityReport {
        collections: repository.collections().len(),
        rss_before_bytes: rss_before,
        ..FeasibilityReport::default()
    };
    let collection_ids = repository
        .collections()
        .iter()
        .enumerate()
        .map(|(index, collection)| {
            (
                collection.definition.name.clone(),
                CollectionId(u32::try_from(index).expect("collection count fits u32")),
            )
        })
        .collect::<HashMap<_, _>>();
    let write_started = Instant::now();
    for (index, collection) in repository.collections().iter().enumerate() {
        let collection_id = CollectionId(u32::try_from(index).expect("collection count fits u32"));
        export_collection(
            &store,
            repository,
            collection,
            collection_id,
            &collection_ids,
            &mut report,
        )?;
    }
    report.write_ns = write_started.elapsed().as_nanos();
    let tx = store.begin_write().map_err(|error| error.to_string())?;
    let mut metadata = read::metadata_from_write(&tx).map_err(|error| error.to_string())?;
    metadata.construction = ConstructionState::Complete;
    metadata.catalog_complete = true;
    metadata.revision = metadata.revision.saturating_add(1);
    write::put_metadata(&tx, &metadata).map_err(|error| error.to_string())?;
    tx.commit().map_err(|error| error.to_string())?;
    let read_view = store.view().map_err(|error| error.to_string())?;
    for (name, definition) in tables::ALL_TABLES {
        let table = read_view
            .read()
            .open_table(*definition)
            .map_err(|error| error.to_string())?;
        let rows = table
            .len()
            .map_err(|error| error.to_string())?;
        let stats = table.stats().map_err(|error| error.to_string())?;
        let pages = stats.leaf_pages().saturating_add(stats.branch_pages());
        report.table_rows.push(((*name).to_string(), rows));
        report.table_bytes.push((
            (*name).to_string(),
            stats.stored_bytes(),
            pages.saturating_mul(4096),
        ));
    }
    report.rss_after_bytes = resident_bytes();
    report.rss_peak_bytes = peak_resident_bytes();
    Ok((store, report))
}

fn export_collection(
    store: &IndexStore,
    repository: &crate::repository::Repository,
    collection: &crate::repository::CollectionState,
    collection_id: CollectionId,
    collection_ids: &HashMap<String, CollectionId>,
    report: &mut FeasibilityReport,
) -> Result<(), String> {
    let tx = store.begin_write().map_err(|error| error.to_string())?;
    let mut metadata = read::metadata_from_write(&tx).map_err(|error| error.to_string())?;
    let collection_metadata = CollectionMetadata {
        id: collection_id,
        name: collection.definition.name.clone(),
        generation: collection.generation.generation.max(1),
        source_fingerprint: [0; 32],
        slot_high_water: collection.generation.records.len() as u64,
        live_records: collection.generation.records.len() as u64,
        discovered_files: collection.generation.files.len() as u64,
        pending_files: 0,
        indexed_files: collection.generation.files.len() as u64,
        unavailable_files: 0,
        processed_files: collection.generation.files.len() as u64,
        next_binding_id: 0,
    };
    write::put_collection(&tx, &collection_metadata).map_err(|error| error.to_string())?;
    for (file_number, file_id) in collection.generation.files.iter().enumerate() {
        let entry = repository
            .files
            .files
            .get(file_id.0 as usize)
            .ok_or_else(|| "file index out of range".to_string())?;
        let fingerprint = repository
            .base_fingerprints
            .get(file_id.0 as usize)
            .copied()
            .flatten();
        write::put_file(&tx, entry, 1, fingerprint).map_err(|error| error.to_string())?;
        write::put_build_state(
            &tx,
            entry.id,
            &FileBuildState {
                state: BuildFileState::Indexed,
                expected_file_version: 1,
                build_epoch: metadata.build_epoch,
                last_admission_revision: metadata.revision,
                error: None,
            },
        )
        .map_err(|error| error.to_string())?;
        let _ = file_number;
        report.files += 1;
    }
    for entry in &collection.generation.records {
        let incarnation = 1;
        write::put_record(
            &tx,
            collection_id,
            entry.slot,
            &crate::index_store::value_codec::RecordMetadata::from_entry(entry, incarnation),
        )
        .map_err(|error| error.to_string())?;
        write::put_logical_id(
            &tx,
            collection_id,
            &entry.id,
            entry.slot,
            incarnation,
        )
        .map_err(|error| error.to_string())?;
        write::put_file_record(&tx, entry.file, collection_id, entry.slot)
            .map_err(|error| error.to_string())?;
        report.records += 1;
    }
    for slot in collection.logical_records.iter() {
        write::insert_domain_member(
            &tx,
            collection_id,
            crate::index_store::value_codec::DomainKind::Logical,
            RecordSlot(slot),
        )
        .map_err(|error| error.to_string())?;
    }
    for slot in collection.empty_array_records.iter() {
        write::insert_domain_member(
            &tx,
            collection_id,
            crate::index_store::value_codec::DomainKind::EmptyArray,
            RecordSlot(slot),
        )
        .map_err(|error| error.to_string())?;
    }
    let record_count = collection.generation.records.len();
    let mut values_by_slot: Vec<Vec<(u32, ScalarKey)>> = vec![Vec::new(); record_count];
    let mut lengths_by_slot: Vec<Vec<(u32, crate::model::Number)>> =
        vec![Vec::new(); record_count];
    let mut state_masks_by_slot: Vec<std::collections::BTreeMap<u32, u16>> =
        vec![std::collections::BTreeMap::new(); record_count];
    let mut sorts_by_slot: Vec<Vec<SortProjection>> = vec![Vec::new(); record_count];
    let mut relations_by_slot: Vec<Vec<RelationOccurrence>> = vec![Vec::new(); record_count];
    let mut state_batch = crate::index_store::states::StateBatch::new();
    let mut bindings = BindingAllocator {
        tx: &tx,
        collection: collection_id,
        next: 0,
    };
    for (path, posting) in &collection.postings.fields {
        let binding = bindings.allocate(
            BindingIdentity::Declared { path: path.clone() },
            BindingRole::Structured,
            posting.kind.clone(),
            posting.multi,
            None,
        )?;
        let field_path = crate::model::FieldPath {
            root: crate::model::PathRoot::Data,
            segments: path.clone(),
        };
        write::put_field(
            &tx,
            collection_id,
            path,
            &crate::index_store::value_codec::FieldDescriptor {
                path: field_path,
                kind: posting.kind.clone(),
                multi: posting.multi,
                indexed: posting.values_indexed,
                values_indexed: posting.values_indexed,
                value_complete: posting.value_complete,
                length_complete: posting.length_complete,
                empty_string_complete: posting.empty_string_complete,
                covered_records: posting.covered_records,
            },
        )
        .map_err(|error| error.to_string())?;
        for (value, slots) in &posting.values {
            for slot in slots.iter() {
                write::insert_value_member(
                    &tx,
                    collection_id,
                    binding,
                    value,
                    RecordSlot(slot),
                )
                .map_err(|error| error.to_string())?;
                values_by_slot[slot as usize].push((binding, value.clone()));
                report.value_members += 1;
            }
        }
        for (length, slots) in &posting.lengths {
            for slot in slots.iter() {
                write::insert_length_member(
                    &tx,
                    collection_id,
                    binding,
                    &ScalarKey::Number(length.clone()),
                    RecordSlot(slot),
                )
                .map_err(|error| error.to_string())?;
                lengths_by_slot[slot as usize].push((binding, length.clone()));
                report.length_members += 1;
            }
        }
        for slot in posting.presence.iter() {
            record_state(
                &mut state_batch,
                &mut state_masks_by_slot,
                binding,
                StateKind::PathPresence,
                slot,
                report,
            );
        }
        for slot in posting.empty_selections.iter() {
            record_state(
                &mut state_batch,
                &mut state_masks_by_slot,
                binding,
                StateKind::EmptySelection,
                slot,
                report,
            );
        }
        for slot in posting.nulls.iter() {
            record_state(
                &mut state_batch,
                &mut state_masks_by_slot,
                binding,
                StateKind::LegacyNullSet,
                slot,
                report,
            );
        }
        if let Some(structural) = &posting.structural {
            for (key, slots) in &structural.postings {
                let state = match key {
                    crate::posting::StructuralKey::AnyNull => StateKind::AnyNull,
                    crate::posting::StructuralKey::NonEmpty => StateKind::NonEmpty,
                    crate::posting::StructuralKey::String => StateKind::String,
                    crate::posting::StructuralKey::Number => StateKind::Number,
                    crate::posting::StructuralKey::Array => StateKind::Array,
                    crate::posting::StructuralKey::Object => StateKind::Object,
                    crate::posting::StructuralKey::EmptyObject => StateKind::EmptyObject,
                };
                for slot in slots.iter() {
                    record_state(
                        &mut state_batch,
                        &mut state_masks_by_slot,
                        binding,
                        state,
                        slot,
                        report,
                    );
                }
            }
        }
    }
    for ((path, name), custom) in &collection.custom_postings {
        let binding = bindings.allocate(
            BindingIdentity::Named {
                path: path.clone(),
                name: name.clone(),
            },
            BindingRole::Structured,
            custom.index.kind.clone(),
            custom.index.multi,
            None,
        )?;
        for (value, slots) in &custom.index.values {
            for slot in slots.iter() {
                write::insert_value_member(
                    &tx,
                    collection_id,
                    binding,
                    value,
                    RecordSlot(slot),
                )
                .map_err(|error| error.to_string())?;
                values_by_slot[slot as usize].push((binding, value.clone()));
                report.value_members += 1;
            }
        }
    }
    crate::index_store::states::apply_batch(&tx, collection_id, &state_batch)
        .map_err(|error| error.to_string())?;
    export_text(
        &tx,
        collection_id,
        BindingIdentity::RawText,
        "raw",
        &collection.raw,
        &mut bindings,
        report,
    )?;
    export_text(
        &tx,
        collection_id,
        BindingIdentity::FallbackText,
        "fallback",
        &collection.fallback_text,
        &mut bindings,
        report,
    )?;
    for ((path, name), index) in &collection.custom_text {
        export_text(
            &tx,
            collection_id,
            BindingIdentity::CustomText {
                path: path.clone(),
                name: name.clone(),
            },
            "custom",
            index,
            &mut bindings,
            report,
        )?;
    }
    let mut sort_definition = 0u32;
    for (path, column) in &collection.sorting.columns {
        for direction in [SortDirection::Ascending, SortDirection::Descending] {
            let definition = SortDefinition {
                id: sort_definition,
                constituents: vec![SortConstituent {
                    key: path.clone(),
                    direction,
                    options: column.options,
                }],
                locale: column.locale.clone(),
                collation_version: column.collation_version.clone(),
            };
            sort_definition += 1;
            write::put_sort_definition(&tx, collection_id, &definition)
                .map_err(|error| error.to_string())?;
            report.sort_definitions += 1;
        }
    }
    let mut definition = 0u32;
    for column in collection.sorting.columns.values() {
        for direction in [SortDirection::Ascending, SortDirection::Descending] {
            let current = definition;
            definition += 1;
            let definitions = vec![SortDefinition {
                id: current,
                constituents: vec![SortConstituent {
                    key: column.path.clone(),
                    direction,
                    options: column.options,
                }],
                locale: column.locale.clone(),
                collation_version: column.collation_version.clone(),
            }];
            for (slot, key) in column.keys.iter().enumerate() {
                let projection = SortProjection {
                    definition: current,
                    key: vec![key.clone()],
                };
                crate::index_store::sorting::insert_sort_projection(
                    &tx,
                    collection_id,
                    RecordSlot(u32::try_from(slot).expect("slot fits u32")),
                    &collection.generation.records[slot].id.0,
                    std::slice::from_ref(&projection),
                    &definitions,
                )
                .map_err(|error| error.to_string())?;
                sorts_by_slot[slot].push(projection);
                report.sort_entries += 1;
            }
        }
    }
    for (declaration, relation) in collection.relations.iter().enumerate() {
        let declaration = u32::try_from(declaration).expect("relation ordinal fits u32");
        for slot in relation.present().iter() {
            write::put_relation_state(
                &tx,
                collection_id,
                declaration,
                RelationStateKind::Present,
                RecordSlot(slot),
            )
            .map_err(|error| error.to_string())?;
        }
        for slot in relation.nulls().iter() {
            write::put_relation_state(
                &tx,
                collection_id,
                declaration,
                RelationStateKind::Nulls,
                RecordSlot(slot),
            )
            .map_err(|error| error.to_string())?;
        }
        for slot in relation.empty_arrays().iter() {
            write::put_relation_state(
                &tx,
                collection_id,
                declaration,
                RelationStateKind::EmptyArray,
                RecordSlot(slot),
            )
            .map_err(|error| error.to_string())?;
        }
        for slot in relation.unavailable().iter() {
            write::put_relation_state(
                &tx,
                collection_id,
                declaration,
                RelationStateKind::Unavailable,
                RecordSlot(slot),
            )
            .map_err(|error| error.to_string())?;
        }
        let target_collection = collection_ids
            .get(&relation.declaration.target_collection)
            .copied()
            .unwrap_or(CollectionId(u32::MAX));
        for source in 0..collection.generation.records.len() {
            let Some(edges) = relation.edges(RecordSlot(source as u32)) else {
                continue;
            };
            for (ordinal, edge) in edges.iter().enumerate() {
                relations_by_slot[source].push(RelationOccurrence {
                    declaration_ordinal: declaration,
                    occurrence_ordinal: u32::try_from(ordinal).expect("ordinal fits u32"),
                    target_collection,
                    target_id: edge.target_id().to_string(),
                    positions: edge.positions.clone(),
                });
                report.relation_occurrences += 1;
            }
        }
    }
    report.bindings += u64::from(bindings.next);
    for (slot, entry) in collection.generation.records.iter().enumerate() {
        let mut values = std::mem::take(&mut values_by_slot[slot]);
        values.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        values.dedup();
        let mut lengths = std::mem::take(&mut lengths_by_slot[slot]);
        lengths.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        lengths.dedup();
        let state_masks = std::mem::take(&mut state_masks_by_slot[slot])
            .into_iter()
            .filter(|(_, mask)| *mask != 0)
            .collect::<Vec<_>>();
        let mut sorts = std::mem::take(&mut sorts_by_slot[slot]);
        sorts.sort_by_key(|projection| projection.definition);
        let relations = std::mem::take(&mut relations_by_slot[slot]);
        let contribution = StoredContributions {
            record: crate::index_store::value_codec::RecordMetadata::from_entry(entry, 1),
            values,
            lengths,
            state_masks,
            coverage: Vec::new(),
            domains: Vec::new(),
            sorts,
            relations,
            // The final native gram representation is gram→bitmap, not a
            // per-record list. Record gram sets are estimated separately.
            grams: Vec::new(),
        };
        let bytes = crate::index_store::contribution_codec::encode(&contribution)
            .map_err(|error| error.to_string())?;
        report.reverse_contribution_bytes += bytes.len() as u64;
        write::put_contributions(&tx, collection_id, entry.slot, &contribution)
            .map_err(|error| error.to_string())?;
        report.contribution_rows += 1;
    }
    metadata.revision = metadata.revision.saturating_add(1);
    write::put_collection(&tx, &collection_metadata).map_err(|error| error.to_string())?;
    write::put_metadata(&tx, &metadata).map_err(|error| error.to_string())?;
    tx.commit().map_err(|error| error.to_string())?;
    Ok(())
}

fn record_state(
    batch: &mut crate::index_store::states::StateBatch,
    masks: &mut [std::collections::BTreeMap<u32, u16>],
    binding: u32,
    state: StateKind,
    slot: u32,
    report: &mut FeasibilityReport,
) {
    batch.set(binding, state, slot);
    let mask = masks
        .get_mut(slot as usize)
        .expect("state slot must be in range")
        .entry(binding)
        .or_insert(0);
    *mask |= 1u16 << state.tag();
    report.state_members += 1;
}

fn export_text(
    tx: &redb::WriteTransaction,
    collection: CollectionId,
    identity: BindingIdentity,
    segment: &str,
    index: &crate::bigram::BigramIndex,
    allocate_binding: &mut BindingAllocator<'_>,
    report: &mut FeasibilityReport,
) -> Result<(), String> {
    let binding = allocate_binding.allocate(
        identity,
        BindingRole::Text,
        LogicalType::String,
        false,
        Some(format!("{:?}-{segment}", index.alphabet)),
    )?;
    let mut batch = FtsBatch::new();
    let mut in_batch = 0usize;
    for (gram, slots) in &index.normal {
        for slot in slots.iter() {
            batch.set(binding, GramKind::Normal, *gram, slot);
            in_batch += 1;
        }
        if in_batch >= 1_000_000 {
            apply_batch(tx, collection, &batch).map_err(|error| error.to_string())?;
            batch = FtsBatch::new();
            in_batch = 0;
        }
    }
    for (gram, slots) in &index.skip {
        for slot in slots.iter() {
            batch.set(binding, GramKind::Skip, *gram, slot);
            in_batch += 1;
        }
        if in_batch >= 1_000_000 {
            apply_batch(tx, collection, &batch).map_err(|error| error.to_string())?;
            batch = FtsBatch::new();
            in_batch = 0;
        }
    }
    if !batch.is_empty() {
        apply_batch(tx, collection, &batch).map_err(|error| error.to_string())?;
    }
    let mut bits = 0u64;
    let mut blocks = 0u64;
    for slots in index.normal.values().chain(index.skip.values()) {
        bits += slots.len();
        blocks += slots.len().div_ceil(4096) as u64;
    }
    report.fts_bits += bits;
    report.fts_blocks += blocks;
    report.fts_grams += (index.normal.len() + index.skip.len()) as u64;
    // Reverse record gram sets are not available from the final gram→bitmap
    // native representation. Estimate their encoded bytes from membership
    // count: each delta varint costs 1–3 bytes in practice.
    report.estimated_record_gram_bytes += bits.saturating_mul(2);
    Ok(())
}

fn resident_bytes() -> u64 {
    proc_status_kib("VmRSS:").saturating_mul(1024)
}

fn peak_resident_bytes() -> u64 {
    proc_status_kib("VmHWM:").saturating_mul(1024)
}

fn proc_status_kib(field: &str) -> u64 {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return 0;
    };
    for line in status.lines() {
        if let Some(value) = line.strip_prefix(field) {
            return value
                .trim()
                .trim_end_matches("kB")
                .trim()
                .parse::<u64>()
                .unwrap_or(0);
        }
    }
    0
}
