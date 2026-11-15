//! Metadata-first source discovery into the authoritative redb catalog.
//!
//! This is the first Phase-6 construction stage. It enumerates selected source
//! files, assigns global file IDs, writes file/path/build-state/pending rows
//! and marks the catalog complete. No content is parsed and no native
//! `FileInventory` is retained.

use std::collections::HashMap;
use std::sync::Arc;
use std::path::{Path, PathBuf};

use crate::discovery::discover;
use crate::inventory::{FileEntry, FileStatus, SharedPath, SourceRevision};
use crate::model::{CollectionId, FileId};
use crate::repository::CollectionDefinition;
use crate::source::SourceRegistry;

use super::value_codec::{
    BuildFileState, CollectionMetadata, ConstructionState, FileBuildState,
};
use super::value_codec::{BindingIdentity, DomainKind};
use super::{IndexStore, read, write};
use crate::model::RecordSlot;
use crate::posting::ScalarKey;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DiscoveryReport {
    pub collections: usize,
    pub files: u64,
    pub memberships: u64,
    pub catalog_complete: bool,
}

/// Enumerate every selected source file and write the catalog to the store.
///
/// The store must be empty (or freshly reset). All collection membership and
/// pending-file rows commit before `catalog_complete` is set.
pub fn discover_into_store(
    store: &IndexStore,
    root: &Path,
    definitions: &[CollectionDefinition],
    sources: &SourceRegistry,
) -> Result<DiscoveryReport, String> {
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let mut report = DiscoveryReport {
        collections: definitions.len(),
        ..DiscoveryReport::default()
    };
    let tx = store.begin_write().map_err(|error| error.to_string())?;
    let mut store_metadata = read::metadata_from_write(&tx).map_err(|error| error.to_string())?;
    if store_metadata.construction != ConstructionState::Empty {
        return Err("discovery requires an empty store".into());
    }
    store_metadata.construction = ConstructionState::Discovering;
    store_metadata.catalog_complete = false;
    store_metadata.revision = store_metadata.revision.saturating_add(1);
    write::put_metadata(&tx, &store_metadata).map_err(|error| error.to_string())?;
    let mut collections = Vec::with_capacity(definitions.len());
    for definition in definitions {
        if collections
            .iter()
            .any(|collection: &CollectionMetadata| collection.name == definition.name)
        {
            return Err(format!("duplicate collection: {}", definition.name));
        }
        let id = CollectionId(
            u32::try_from(collections.len()).map_err(|_| "too many collections")?,
        );
        collections.push(CollectionMetadata {
            id,
            name: definition.name.clone(),
            generation: 1,
            source_fingerprint: [0; 32],
            slot_high_water: 0,
            live_records: 0,
            discovered_files: 0,
            pending_files: 0,
            indexed_files: 0,
            unavailable_files: 0,
            processed_files: 0,
            next_binding_id: 0,
        });
        write::put_collection(
            &tx,
            collections.last().expect("collection was just pushed"),
        )
        .map_err(|error| error.to_string())?;
    }
    tx.commit().map_err(|error| error.to_string())?;

    let mut next_file_id = store_metadata.next_file_id;
    let mut physical = HashMap::<PathBuf, FileIdCollections>::new();
    for (collection_index, definition) in definitions.iter().enumerate() {
        let collection = CollectionId(collection_index as u32);
        let source = sources.resolve(&definition.format).map_err(|error| error.to_string())?;
        let paths = discover(&root, &definition.source_path, &definition.discovery, &source)
            .map_err(|error| error.to_string())?;
        let tx = store.begin_write().map_err(|error| error.to_string())?;
        let mut collection_metadata =
            read::collection_from_write(&tx, collection).map_err(|error| error.to_string())?;
        for path in paths {
            let relative = path
                .strip_prefix(&root)
                .map_err(|error| error.to_string())?
                .to_path_buf();
            let existing = physical.get(&relative).cloned();
            let file_id = if let Some(existing) = existing {
                existing.id
            } else {
                let file_metadata =
                    std::fs::metadata(&path).map_err(|error| error.to_string())?;
                let revision = SourceRevision::from_metadata(&file_metadata)?;
                let created_ns = file_metadata
                    .created()
                    .ok()
                    .and_then(|time| {
                        time.duration_since(std::time::SystemTime::UNIX_EPOCH)
                            .ok()
                    })
                    .map(|duration| duration.as_nanos() as i128);
                let id = crate::model::FileId(next_file_id);
                next_file_id = next_file_id
                    .checked_add(1)
                    .ok_or_else(|| "file ID overflow".to_string())?;
                let entry = FileEntry {
                    id,
                    relative_path: SharedPath::from(relative.clone()),
                    revision,
                    created_ns,
                    collections: Vec::new(),
                    status: FileStatus::Valid,
                    diagnostics: Vec::new(),
                    omitted_diagnostics: 0,
                };
                write::put_file(&tx, &entry, 1, None).map_err(|error| error.to_string())?;
                write::put_build_state(
                    &tx,
                    id,
                    &FileBuildState {
                        state: BuildFileState::Pending,
                        expected_file_version: 1,
                        build_epoch: store_metadata.build_epoch,
                        last_admission_revision: store_metadata.revision,
                        error: None,
                    },
                )
                .map_err(|error| error.to_string())?;
                physical.insert(
                    relative.clone(),
                    FileIdCollections {
                        id,
                        collections: Vec::new(),
                    },
                );
                report.files += 1;
                id
            };
            let record = physical.get_mut(&relative).expect("physical file exists");
            if !record.collections.contains(&collection) {
                record.collections.push(collection);
                write::add_pending_file(&tx, collection, file_id)
                    .map_err(|error| error.to_string())?;
                collection_metadata.discovered_files += 1;
                collection_metadata.pending_files += 1;
                report.memberships += 1;
            }
        }
        write::put_collection(&tx, &collection_metadata).map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;
    }

    // Rewrite shared file ownership after all collection passes and publish
    // catalog readiness in one final transaction.
    let tx = store.begin_write().map_err(|error| error.to_string())?;
    let mut metadata = read::metadata_from_write(&tx).map_err(|error| error.to_string())?;
    for record in physical.values() {
        let Some(mut file) = write::get_file(&tx, record.id).map_err(|error| error.to_string())?
        else {
            return Err("discovered file disappeared".into());
        };
        file.entry.collections = record.collections.clone();
        write::put_file(&tx, &file.entry, file.file_version, file.fingerprint)
            .map_err(|error| error.to_string())?;
    }
    metadata.next_file_id = next_file_id;
    metadata.construction = ConstructionState::Building;
    metadata.catalog_complete = true;
    metadata.revision = metadata.revision.saturating_add(1);
    write::put_metadata(&tx, &metadata).map_err(|error| error.to_string())?;
    tx.commit().map_err(|error| error.to_string())?;
    report.catalog_complete = true;
    Ok(report)
}

#[derive(Clone)]
struct FileIdCollections {
    id: FileId,
    collections: Vec<CollectionId>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::SourceFormat;

    #[test]
    fn discovery_writes_catalog_and_pending_files() {
        let directory = std::env::temp_dir().join(format!(
            "objekte-store-discovery-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(directory.join("sub")).expect("directory");
        std::fs::write(directory.join("a.json"), b"{\"x\":1}").expect("a");
        std::fs::write(directory.join("sub/b.json"), b"{\"x\":2}").expect("b");
        let mut definition = CollectionDefinition::folder("test", "", SourceFormat::Json);
        definition.discovery.recursive = true;
        let store = std::sync::Arc::new(
            IndexStore::new(super::super::StoreConfig::default()).expect("store"),
        );
        let registry = SourceRegistry::builtin();
        let report = discover_into_store(
            &store,
            &directory,
            std::slice::from_ref(&definition),
            &registry,
        )
        .expect("discovery");
        assert_eq!(report.files, 2);
        assert_eq!(report.memberships, 2);
        assert!(report.catalog_complete);
        let view = store.view().expect("view");
        assert!(view.metadata().catalog_complete);
        assert_eq!(
            read::pending_files(&view, CollectionId(0)).expect("pending").len(),
            2
        );
        assert_eq!(
            read::build_state_kind(&view, FileId(0)).expect("state"),
            Some(BuildFileState::Pending)
        );
        std::fs::remove_dir_all(directory).expect("cleanup");
    }
}

/// Options for a direct source build into redb.
#[derive(Clone, Debug)]
pub struct SourceBuildOptions {
    pub build_workers: Option<std::num::NonZeroUsize>,
    pub alphabet: crate::workload::BigramAlphabet,
    pub inference: crate::inference::InferenceConfig,
}

impl Default for SourceBuildOptions {
    fn default() -> Self {
        Self {
            build_workers: None,
            alphabet: crate::workload::BigramAlphabet::Ascii,
            inference: crate::inference::InferenceConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SourceBuildReport {
    pub discovery: DiscoveryReport,
    pub records: u64,
    pub batches: u64,
    pub elapsed_ns: u128,
}

/// Build a complete redb index directly from source extraction.
///
/// Discovery writes the catalog, then the existing parallel extraction
/// pipeline produces `BatchContribution`s that are admitted into redb one
/// batch at a time. No native `Repository` or `FileInventory` is retained.
pub fn build_from_source(
    store: &Arc<IndexStore>,
    root: &Path,
    definitions: &[CollectionDefinition],
    options: &SourceBuildOptions,
    sources: &SourceRegistry,
) -> Result<SourceBuildReport, String> {
    let started = std::time::Instant::now();
    let discovery = discover_into_store(store, root, definitions, sources)?;
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let workers = crate::build::resolve_build_workers(
        options.build_workers,
        std::thread::available_parallelism().ok(),
    )?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .thread_name(|index| format!("objekte-redb-build-{index}"))
        .build()
        .map_err(|error| error.to_string())?;
    let view = store.view().map_err(|error| error.to_string())?;
    let metadata = view.metadata().clone();
    let mut source_files = Vec::new();
    for id in 0..metadata.next_file_id {
        if let Some(file) = read::file(&view, crate::model::FileId(id))
            .map_err(|error| error.to_string())?
        {
            source_files.push(crate::build::SourceFileInput {
                id: file.entry.id,
                relative_path: file.entry.relative_path,
                revision: file.entry.revision,
                created_ns: file.entry.created_ns,
            });
        }
    }
    drop(view);
    let collection_ids = definitions
        .iter()
        .enumerate()
        .map(|(index, definition)| {
            (
                definition.name.clone(),
                CollectionId(u32::try_from(index).expect("collection count fits u32")),
            )
        })
        .collect::<HashMap<_, _>>();
    let mut jobs = Vec::new();
    for (index, definition) in definitions.iter().enumerate() {
        let collection = CollectionId(index as u32);
        let view = store.view().map_err(|error| error.to_string())?;
        let pending = read::pending_files(&view, collection).map_err(|error| error.to_string())?;
        drop(view);
        let source = sources
            .resolve(&definition.format)
            .map_err(|error| error.to_string())?;
        let mut start = 0;
        while start < pending.len() {
            let mut end = start;
            let mut bytes = 0u64;
            while end < pending.len() && end - start < 512 {
                let size = source_files
                    .get(pending[end].0 as usize)
                    .map_or(0, |file| file.revision.size);
                if end > start && bytes.saturating_add(size) > 8 * 1024 * 1024 {
                    break;
                }
                bytes = bytes.saturating_add(size);
                end += 1;
            }
            jobs.push(crate::build::RepositoryFileJob {
                collection_index: index,
                collection,
                definition: Arc::new(definition.clone()),
                source: source.clone(),
                files: pending[start..end].to_vec(),
            });
            start = end;
        }
    }
    let mut builder = BatchStoreBuilder {
        store,
        pool: &pool,
        metadata,
        records_written: 0,
        collections: definitions
            .iter()
            .enumerate()
            .map(|(index, definition)| BuildCollection {
                id: CollectionId(index as u32),
                definition: definition.clone(),
                bindings: HashMap::new(),
                next_binding: 0,
                sort_ids: HashMap::new(),
                next_sort: 0,
                relations: HashMap::new(),
                next_relation: 0,
            })
            .collect(),
        collection_ids,
    };
    let mut report = SourceBuildReport {
        discovery,
        ..SourceBuildReport::default()
    };
    crate::build::process_repository_file_batches(
        &pool,
        Arc::new(root),
        &jobs,
        Arc::new(source_files),
        options.alphabet,
        options.inference.clone(),
        |collection_index, batch| {
            builder.admit(collection_index, batch)?;
            report.batches += 1;
            Ok(())
        },
    )?;
    report.records = builder.records_written;
    report.elapsed_ns = started.elapsed().as_nanos();
    let tx = store.begin_write().map_err(|error| error.to_string())?;
    let mut metadata = read::metadata_from_write(&tx).map_err(|error| error.to_string())?;
    metadata.construction = ConstructionState::Complete;
    metadata.catalog_complete = true;
    metadata.revision = metadata.revision.saturating_add(1);
    write::put_metadata(&tx, &metadata).map_err(|error| error.to_string())?;
    tx.commit().map_err(|error| error.to_string())?;
    Ok(report)
}

struct BuildCollection {
    id: CollectionId,
    definition: CollectionDefinition,
    bindings: HashMap<BindingIdentity, u32>,
    next_binding: u32,
    sort_ids: HashMap<crate::model::FieldPath, (u32, u32)>,
    next_sort: u32,
    relations: HashMap<Vec<String>, u32>,
    next_relation: u32,
}

struct BatchStoreBuilder<'a> {
    store: &'a Arc<IndexStore>,
    pool: &'a rayon::ThreadPool,
    metadata: crate::index_store::value_codec::StoreMetadata,
    collections: Vec<BuildCollection>,
    collection_ids: HashMap<String, CollectionId>,
    records_written: u64,
}

impl BatchStoreBuilder<'_> {
    fn admit(&mut self, collection_index: usize, mut batch: crate::build::BatchContribution) -> Result<(), String> {
        let collection = &mut self.collections[collection_index];
        let tx = self.store.begin_write().map_err(|error| error.to_string())?;
        let mut collection_metadata =
            read::collection_from_write(&tx, collection.id).map_err(|error| error.to_string())?;
        let batch_records = std::mem::take(&mut batch.records);
        let count = batch_records.len();
        let mut local_to_global = vec![crate::model::RecordSlot(0); count];
        let mut contributions: Vec<crate::index_store::value_codec::StoredContributions> =
            Vec::with_capacity(count);
        for entry in &batch_records {
            let (slot, incarnation) =
                super::slot_allocator::allocate(&tx, &mut collection_metadata)
                    .map_err(|error| error.to_string())?;
            local_to_global[entry.slot.0 as usize] = slot;
            let record = crate::index_store::value_codec::RecordMetadata::from_entry(entry, incarnation);
            write::put_record(&tx, collection.id, slot, &record).map_err(|error| error.to_string())?;
            write::put_logical_id(&tx, collection.id, &entry.id, slot, incarnation)
                .map_err(|error| error.to_string())?;
            write::put_file_record(&tx, entry.file, collection.id, slot)
                .map_err(|error| error.to_string())?;
            contributions.push(crate::index_store::value_codec::StoredContributions {
                record,
                values: Vec::new(),
                lengths: Vec::new(),
                state_masks: Vec::new(),
                coverage: Vec::new(),
                domains: Vec::new(),
                sorts: Vec::new(),
                relations: Vec::new(),
                grams: Vec::new(),
            });
        }
        let mut state_batch = super::states::StateBatch::new();
        let mut posting_batch = super::posting_blocks::PostingBatch::new();
        let mut fts_batch = super::fts::FtsBatch::new();
        let mut gram_rows_for_batch: HashMap<(u32, u32, crate::index_store::value_codec::GramKind), Vec<u64>> = HashMap::new();
        let mut state_masks: Vec<std::collections::BTreeMap<u32, u16>> =
            vec![std::collections::BTreeMap::new(); count];
        let mut declare_binding = |collection: &mut BuildCollection,
                                   tx: &redb::WriteTransaction,
                                   identity: BindingIdentity,
                                   kind: crate::model::LogicalType,
                                   multi: bool,
                                   role: crate::index_store::value_codec::BindingRole,
                                   alphabet: Option<String>|
         -> Result<u32, String> {
            if let Some(id) = collection.bindings.get(&identity) {
                return Ok(*id);
            }
            let id = collection.next_binding;
            collection.next_binding = collection.next_binding.checked_add(1).ok_or("binding overflow")?;
            let encoded = bincode::serialize(&identity).map_err(|error| error.to_string())?;
            write::put_binding(
                tx,
                collection.id,
                &encoded,
                &crate::index_store::value_codec::StoredBinding {
                    id,
                    descriptor: crate::index_store::value_codec::BindingDescriptor {
                        identity: identity.clone(),
                        role,
                        kind,
                        multi,
                        text_alphabet: alphabet,
                        text_segment: None,
                        fingerprint: [0; 32],
                    },
                },
            )
            .map_err(|error| error.to_string())?;
            collection.bindings.insert(identity, id);
            Ok(id)
        };
        for (path, posting) in &batch.postings.fields {
            let binding = declare_binding(
                collection,
                &tx,
                BindingIdentity::Declared { path: path.clone() },
                posting.kind.clone(),
                posting.multi,
                crate::index_store::value_codec::BindingRole::Structured,
                None,
            )?;
            write::put_field(
                &tx,
                collection.id,
                path,
                &crate::index_store::value_codec::FieldDescriptor {
                    path: crate::model::FieldPath {
                        root: crate::model::PathRoot::Data,
                        segments: path.clone(),
                    },
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
                for local in slots.iter() {
                    let global = local_to_global[local as usize];
                    posting_batch.set(
                        super::posting_blocks::PostingFamily::Value,
                        binding,
                        value,
                        global,
                    );
                    contributions[local as usize]
                        .values
                        .push((binding, value.clone()));
                }
            }
            for (length, slots) in &posting.lengths {
                for local in slots.iter() {
                    let global = local_to_global[local as usize];
                    let scalar = ScalarKey::Number(length.clone());
                    posting_batch.set(
                        super::posting_blocks::PostingFamily::Length,
                        binding,
                        &scalar,
                        global,
                    );
                    contributions[local as usize]
                        .lengths
                        .push((binding, length.clone()));
                }
            }
            for (state, slots) in [
                (crate::index_store::value_codec::StateKind::PathPresence, &posting.presence),
                (crate::index_store::value_codec::StateKind::EmptySelection, &posting.empty_selections),
                (crate::index_store::value_codec::StateKind::LegacyNullSet, &posting.nulls),
            ] {
                for local in slots.iter() {
                    let global = local_to_global[local as usize];
                    state_batch.set(binding, state, global.0);
                    *state_masks[local as usize].entry(binding).or_insert(0) |= 1 << state.tag();
                }
            }
            if let Some(structural) = &posting.structural {
                for (key, slots) in &structural.postings {
                    let state = match key {
                        crate::posting::StructuralKey::AnyNull => crate::index_store::value_codec::StateKind::AnyNull,
                        crate::posting::StructuralKey::NonEmpty => crate::index_store::value_codec::StateKind::NonEmpty,
                        crate::posting::StructuralKey::String => crate::index_store::value_codec::StateKind::String,
                        crate::posting::StructuralKey::Number => crate::index_store::value_codec::StateKind::Number,
                        crate::posting::StructuralKey::Array => crate::index_store::value_codec::StateKind::Array,
                        crate::posting::StructuralKey::Object => crate::index_store::value_codec::StateKind::Object,
                        crate::posting::StructuralKey::EmptyObject => crate::index_store::value_codec::StateKind::EmptyObject,
                    };
                    for local in slots.iter() {
                        let global = local_to_global[local as usize];
                        state_batch.set(binding, state, global.0);
                        *state_masks[local as usize].entry(binding).or_insert(0) |= 1 << state.tag();
                    }
                }
            }
        }
        for ((path, name), custom) in &batch.custom_postings {
            let binding = declare_binding(
                collection,
                &tx,
                BindingIdentity::Named { path: path.clone(), name: name.clone() },
                custom.index.kind.clone(),
                custom.index.multi,
                crate::index_store::value_codec::BindingRole::Structured,
                None,
            )?;
            for (value, slots) in &custom.index.values {
                for local in slots.iter() {
                    let global = local_to_global[local as usize];
                    posting_batch.set(
                        super::posting_blocks::PostingFamily::Value,
                        binding,
                        value,
                        global,
                    );
                    contributions[local as usize].values.push((binding, value.clone()));
                }
            }
        }
        for local in batch.logical_records.iter() {
            let global = local_to_global[local as usize];
            let kind = DomainKind::Logical;
            write::insert_domain_member(&tx, collection.id, kind, global)
                .map_err(|error| error.to_string())?;
            contributions[local as usize].domains.push(kind);
        }
        for local in batch.empty_array_records.iter() {
            let global = local_to_global[local as usize];
            let kind = DomainKind::EmptyArray;
            write::insert_domain_member(&tx, collection.id, kind, global)
                .map_err(|error| error.to_string())?;
            contributions[local as usize].domains.push(kind);
        }
        for (path, keys) in &batch.sort_keys {
            let (ascending, descending) = if let Some(ids) = collection.sort_ids.get(path) {
                *ids
            } else {
                let ids = (collection.next_sort, collection.next_sort + 1);
                collection.next_sort += 2;
                let options = collection
                    .definition
                    .sort_overrides
                    .get(path)
                    .map_or(collection.definition.sort_options, |config| config.options);
                let locale = collection
                    .definition
                    .sort_overrides
                    .get(path)
                    .map_or(collection.definition.sort_locale.clone(), |config| config.locale.clone());
                for (id, direction) in [
                    (ids.0, crate::index_store::value_codec::SortDirection::Ascending),
                    (ids.1, crate::index_store::value_codec::SortDirection::Descending),
                ] {
                    write::put_sort_definition(
                        &tx,
                        collection.id,
                        &crate::index_store::value_codec::SortDefinition {
                            id,
                            constituents: vec![crate::index_store::value_codec::SortConstituent {
                                key: path.clone(),
                                direction,
                                options,
                            }],
                            locale: locale.clone(),
                            collation_version: "icu_collator-2.4.0".into(),
                        },
                    )
                    .map_err(|error| error.to_string())?;
                }
                collection.sort_ids.insert(path.clone(), ids);
                ids
            };
            for (local, key) in keys.iter().enumerate() {
                let global = local_to_global[local];
                let id = if matches!(key, crate::sorting::SortKey::Missing | crate::sorting::SortKey::Unavailable) {
                    ascending
                } else {
                    ascending
                };
                contributions[local].sorts.push(crate::index_store::value_codec::SortProjection {
                    definition: id,
                    key: vec![key.clone()],
                });
                let id2 = descending;
                contributions[local].sorts.push(crate::index_store::value_codec::SortProjection {
                    definition: id2,
                    key: vec![key.clone()],
                });
                let _ = global;
            }
        }
        for relation in &batch.relations {
            let path = relation.declaration().path.clone();
            let ordinal = if let Some(ordinal) = collection.relations.get(&path) {
                *ordinal
            } else {
                let ordinal = collection.next_relation;
                collection.next_relation += 1;
                collection.relations.insert(path.clone(), ordinal);
                ordinal
            };
            let target = self
                .collection_ids
                .get(&relation.declaration().target_collection)
                .copied()
                .unwrap_or(CollectionId(u32::MAX));
            for local in 0..count {
                for (index, (target_id, positions)) in relation
                    .references_for_source(RecordSlot(local as u32))
                    .enumerate()
                {
                    let global = local_to_global[local];
                    let occurrence = crate::index_store::value_codec::RelationOccurrence {
                        declaration_ordinal: ordinal,
                        occurrence_ordinal: index as u32,
                        target_collection: target,
                        target_id: target_id.to_string(),
                        positions: positions.to_vec(),
                    };
                    crate::index_store::relations::insert_relation_occurrences(
                        &tx,
                        collection.id,
                        global,
                        std::slice::from_ref(&occurrence),
                    )
                    .map_err(|error| error.to_string())?;
                    contributions[local].relations.push(occurrence);
                }
                for (state, slots) in [
                    (crate::index_store::value_codec::RelationStateKind::Present, relation.present()),
                    (crate::index_store::value_codec::RelationStateKind::Nulls, relation.nulls()),
                    (crate::index_store::value_codec::RelationStateKind::EmptyArray, relation.empty_arrays()),
                    (crate::index_store::value_codec::RelationStateKind::Unavailable, relation.unavailable()),
                ] {
                    if slots.contains(local as u32) {
                        write::put_relation_state(&tx, collection.id, ordinal, state, local_to_global[local])
                            .map_err(|error| error.to_string())?;
                    }
                }
            }
        }
        let text_indices = [
            (BindingIdentity::RawText, Some(batch.raw)),
            (BindingIdentity::FallbackText, Some(batch.fallback_text)),
        ];
        for (identity, index) in text_indices {
            if let Some(index) = index {
                let binding = declare_binding(
                    collection,
                    &tx,
                    identity,
                    crate::model::LogicalType::String,
                    false,
                    crate::index_store::value_codec::BindingRole::Text,
                    None,
                )?;
                let index = index.into_index_in_pool(self.pool).map_err(|error| error.to_string())?;
                let mut gram_rows: std::collections::BTreeMap<(u32, u8), Vec<u64>> =
                    std::collections::BTreeMap::new();
                for (kind, map) in [
                    (crate::index_store::value_codec::GramKind::Normal, &index.normal),
                    (crate::index_store::value_codec::GramKind::Skip, &index.skip),
                ] {
                    for (gram, slots) in map {
                        for local in slots.iter() {
                            let global = local_to_global[local as usize];
                            gram_rows.entry((global.0, kind.tag())).or_default().push(*gram);
                        }
                    }
                }
                for ((slot, kind), mut grams) in gram_rows {
                    grams.sort_unstable();
                    grams.dedup();
                    let kind = if kind == crate::index_store::tables::GRAM_SKIP { crate::index_store::value_codec::GramKind::Skip } else { crate::index_store::value_codec::GramKind::Normal };
                    for gram in &grams {
                        fts_batch.set(binding, kind, *gram, slot);
                    }
                    gram_rows_for_batch
                        .entry((slot, binding, kind))
                        .or_default()
                        .extend(grams);
                }
            }
        }
        let mut custom_text = std::mem::take(&mut batch.custom_text);
        for ((path, name), index) in custom_text.drain() {
            let binding = declare_binding(
                collection,
                &tx,
                BindingIdentity::CustomText { path: path.clone(), name: name.clone() },
                crate::model::LogicalType::String,
                false,
                crate::index_store::value_codec::BindingRole::Text,
                None,
            )?;
            let index = index.into_index_in_pool(self.pool).map_err(|error| error.to_string())?;
            let mut gram_rows: std::collections::BTreeMap<(u32, u8), Vec<u64>> =
                std::collections::BTreeMap::new();
            for (kind, map) in [
                (crate::index_store::value_codec::GramKind::Normal, &index.normal),
                (crate::index_store::value_codec::GramKind::Skip, &index.skip),
            ] {
                for (gram, slots) in map {
                    for local in slots.iter() {
                        let global = local_to_global[local as usize];
                        gram_rows.entry((global.0, kind.tag())).or_default().push(*gram);
                    }
                }
            }
            for ((slot, kind), mut grams) in gram_rows {
                grams.sort_unstable();
                grams.dedup();
                let kind = if kind == crate::index_store::tables::GRAM_SKIP { crate::index_store::value_codec::GramKind::Skip } else { crate::index_store::value_codec::GramKind::Normal };
                crate::index_store::fts::insert_record_grams(
                    &tx,
                    collection.id,
                    RecordSlot(slot),
                    &[crate::index_store::value_codec::GramProjection { binding, kind, grams }],
                ).map_err(|error| error.to_string())?;
            }
        }
        let sort_definitions = write::sort_definitions(&tx, collection.id).map_err(|error| error.to_string())?;
        for (local, contribution) in contributions.iter().enumerate() {
            crate::index_store::sorting::insert_sort_projection(
                &tx,
                collection.id,
                local_to_global[local],
                &contribution.record.id.0,
                &contribution.sorts,
                &sort_definitions,
            ).map_err(|error| error.to_string())?;
        }
        super::posting_blocks::apply_batch(&tx, collection.id, &posting_batch).map_err(|error| error.to_string())?;
        super::fts::apply_batch(&tx, collection.id, &fts_batch).map_err(|error| error.to_string())?;
        {
            let mut table = tx
                .open_table(crate::index_store::tables::RECORD_GRAMS)
                .map_err(|error| error.to_string())?;
            for ((slot, binding, kind), mut grams) in gram_rows_for_batch.drain() {
                grams.sort_unstable();
                grams.dedup();
                let key = crate::index_store::keys::record_grams(collection.id.0, slot, binding, kind);
                let encoded = crate::index_store::value_codec::encode_gram_list(&grams);
                table.insert(key.as_slice(), encoded.as_slice()).map_err(|error| error.to_string())?;
            }
        }
        super::states::apply_batch(&tx, collection.id, &state_batch).map_err(|error| error.to_string())?;
        for (local, contribution) in contributions.iter_mut().enumerate() {
            contribution.state_masks = std::mem::take(&mut state_masks[local]).into_iter().collect();
            write::put_contributions(&tx, collection.id, local_to_global[local], contribution)
                .map_err(|error| error.to_string())?;
        }
        for batch_file in &batch.files {
            let mut entry = batch_file.entry.clone();
            if let Some(existing) = write::get_file(&tx, entry.id).map_err(|error| error.to_string())? {
                for collection in existing.entry.collections {
                    if !entry.collections.contains(&collection) {
                        entry.collections.push(collection);
                    }
                }
            }
            let version = batch_file
                .records
                .start
                .checked_add(1)
                .unwrap_or(1) as u64;
            write::put_file(&tx, &entry, version, None).map_err(|error| error.to_string())?;
            write::put_build_state(
                &tx,
                entry.id,
                &crate::index_store::value_codec::FileBuildState {
                    state: crate::index_store::value_codec::BuildFileState::Indexed,
                    expected_file_version: version,
                    build_epoch: self.metadata.build_epoch,
                    last_admission_revision: self.metadata.revision,
                    error: None,
                },
            ).map_err(|error| error.to_string())?;
            write::remove_pending_file(&tx, collection.id, entry.id).map_err(|error| error.to_string())?;
            if collection_metadata.pending_files > 0 {
                collection_metadata.pending_files -= 1;
            }
            collection_metadata.indexed_files += 1;
            collection_metadata.processed_files += 1;
        }
        collection_metadata.live_records += count as u64;
        write::put_collection(&tx, &collection_metadata).map_err(|error| error.to_string())?;
        let mut metadata = read::metadata_from_write(&tx).map_err(|error| error.to_string())?;
        metadata.revision = metadata.revision.saturating_add(1);
        write::put_metadata(&tx, &metadata).map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;
        self.records_written += count as u64;
        Ok(())
    }
}
