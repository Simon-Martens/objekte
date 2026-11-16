//! Construction of independent component trees from native build results.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;

use rayon::prelude::*;
use roaring::RoaringBitmap;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::bigram::BigramIndex;
use crate::inventory::{FileInventory, RecordClass, RecordEntry};
use crate::model::{CollectionId, RecordSlot};
use crate::posting::{PostingIndex, StructuralKey};
use crate::relation::{RelationIndex, RelationTarget};
use crate::repository::{CollectionState, Repository};
use crate::sorting::SortColumn;

use super::StablePrototype;
use super::codec::{
    self, BindingRole, CollectionIdentity, ComponentDescriptor, ComponentId, ComponentKey,
    ComponentKind, CoverageDescriptor, FieldIdentity, FileCatalogRecord, OrderingDescriptor,
    PathCatalogEntry, PathKey, RecordCatalogRecord, RecordRef, RelationOccurrenceKey,
    RelationOccurrenceValue, RelationTargetKey, SortEntryKey,
};
use super::memory::{self, IndexTree, StoredBytes};
use super::snapshot::ManifestCollection;

pub(crate) fn from_repository(
    repository: &Repository,
    workers: Option<usize>,
) -> Result<StablePrototype, String> {
    if repository
        .collections()
        .iter()
        .any(|state| state.inference.is_some())
    {
        return Err("inference is not supported by the stable-index prototype".into());
    }
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(worker_count(workers))
        .thread_name(|index| format!("objekte-icss-build-{index}"))
        .build()
        .map_err(|error| error.to_string())?;
    from_collections(
        repository.files(),
        repository.collections(),
        &repository.files().root,
        &pool,
    )
}

pub(crate) fn from_collections(
    files: &FileInventory,
    collections: &[CollectionState],
    root: &Path,
    pool: &rayon::ThreadPool,
) -> Result<StablePrototype, String> {
    if collections.iter().any(|state| state.inference.is_some()) {
        return Err("inference is not supported by the stable-index prototype".into());
    }
    let configuration_identity = configuration_identity(collections);
    let repository_identity = repository_identity(files, collections)?;
    let slot_namespace = slot_namespace(root, configuration_identity, repository_identity);
    let schema_fingerprint = collections
        .first()
        .and_then(|state| state.definition.schema.source_fingerprint());
    let collection_identities = collections
        .iter()
        .map(|state| {
            (
                state.generation.collection,
                CollectionIdentity {
                    id: state.generation.collection,
                    name: state.definition.name.clone(),
                    generation: state.generation.generation,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();

    let mut drafts = Vec::new();
    drafts.push(files_draft(
        files,
        &configuration_identity,
        repository_identity,
        schema_fingerprint,
        &slot_namespace,
        root,
    ));
    drafts.push(paths_draft(
        prepare_paths(files, collections)?,
        &configuration_identity,
        repository_identity,
        schema_fingerprint,
        &slot_namespace,
        &root,
    ));
    for state in collections {
        let collection = &collection_identities[&state.generation.collection];
        drafts.push(records_draft(
            state,
            collection,
            &configuration_identity,
            repository_identity,
            schema_fingerprint,
            &slot_namespace,
            &root,
        ));
        drafts.push(identities_draft(
            state,
            collection,
            &configuration_identity,
            repository_identity,
            schema_fingerprint,
            &slot_namespace,
            &root,
        ));
        drafts.push(domains_draft(
            state,
            collection,
            &configuration_identity,
            repository_identity,
            schema_fingerprint,
            &slot_namespace,
            &root,
        ));
        for (path, index) in &state.postings.fields {
            let field = FieldIdentity {
                path: path.clone(),
                binding: None,
                role: BindingRole::DeclaredField,
                logical_type: Some(index.kind.clone()),
                multi: index.multi,
            };
            let coverage = CoverageDescriptor {
                values_indexed: index.values_indexed,
                value_complete: index.value_complete,
                length_complete: index.length_complete,
                empty_string_complete: index.empty_string_complete,
                covered_records: index.covered_records,
                complete_for_field: index.value_complete
                    && index.length_complete
                    && index.empty_string_complete,
                structural_complete: index.structural.is_some(),
            };
            drafts.push(posting_draft(
                state,
                collection,
                field.clone(),
                ComponentKind::Values,
                coverage.clone(),
                JobData::Values(index),
                &configuration_identity,
                repository_identity,
                schema_fingerprint,
                &slot_namespace,
                root,
            ));
            drafts.push(posting_draft(
                state,
                collection,
                field.clone(),
                ComponentKind::Lengths,
                coverage.clone(),
                JobData::Lengths(index),
                &configuration_identity,
                repository_identity,
                schema_fingerprint,
                &slot_namespace,
                root,
            ));
            drafts.push(posting_draft(
                state,
                collection,
                field,
                ComponentKind::States,
                coverage,
                JobData::States(index),
                &configuration_identity,
                repository_identity,
                schema_fingerprint,
                &slot_namespace,
                root,
            ));
        }
        for ((path, name), custom) in &state.custom_postings {
            let field = FieldIdentity {
                path: path.clone(),
                binding: Some(name.clone()),
                role: BindingRole::NamedBinding,
                logical_type: Some(custom.index.kind.clone()),
                multi: custom.index.multi,
            };
            let coverage = CoverageDescriptor {
                values_indexed: custom.index.values_indexed,
                value_complete: custom.index.value_complete,
                length_complete: custom.index.length_complete,
                empty_string_complete: custom.index.empty_string_complete,
                covered_records: custom.index.covered_records,
                complete_for_field: custom.complete,
                structural_complete: custom.index.structural.is_some(),
            };
            for (kind, data) in [
                (ComponentKind::Values, JobData::Values(&custom.index)),
                (ComponentKind::Lengths, JobData::Lengths(&custom.index)),
                (ComponentKind::States, JobData::States(&custom.index)),
            ] {
                drafts.push(posting_draft(
                    state,
                    collection,
                    field.clone(),
                    kind,
                    coverage.clone(),
                    data,
                    &configuration_identity,
                    repository_identity,
                    schema_fingerprint,
                    &slot_namespace,
                    root,
                ));
            }
        }
        append_text_drafts(
            &mut drafts,
            state,
            collection,
            BindingRole::RawText,
            Vec::new(),
            None,
            &state.raw,
            &configuration_identity,
            repository_identity,
            schema_fingerprint,
            &slot_namespace,
            root,
        );
        append_text_drafts(
            &mut drafts,
            state,
            collection,
            BindingRole::FallbackText,
            Vec::new(),
            None,
            &state.fallback_text,
            &configuration_identity,
            repository_identity,
            schema_fingerprint,
            &slot_namespace,
            root,
        );
        for ((path, name), index) in &state.custom_text {
            append_text_drafts(
                &mut drafts,
                state,
                collection,
                BindingRole::CustomText,
                path.clone(),
                Some(name.clone()),
                index,
                &configuration_identity,
                repository_identity,
                schema_fingerprint,
                &slot_namespace,
                root,
            );
        }
        for composite in &state.sorting.composites {
            let mut columns = Vec::with_capacity(composite.fields.len());
            let mut descending = Vec::with_capacity(composite.fields.len());
            let mut parts = Vec::with_capacity(composite.fields.len());
            for part in &composite.fields {
                let column = state
                    .sorting
                    .columns
                    .get(&part.path)
                    .ok_or_else(|| "composite sort constituent is missing".to_string())?;
                columns.push(column);
                descending.push(part.descending);
                parts.push(codec::CompositeOrderingPart {
                    path: part.path.segments.clone(),
                    locale: column.locale.clone(),
                    collation_version: column.collation_version.clone(),
                    options: column.options,
                    descending: part.descending,
                });
            }
            let mut descriptor = descriptor(
                ComponentKind::CompositeSort,
                Some(collection.clone()),
                None,
                None,
                None,
                CoverageDescriptor {
                    values_indexed: true,
                    value_complete: true,
                    length_complete: false,
                    empty_string_complete: false,
                    covered_records: state.generation.records.len() as u32,
                    complete_for_field: true,
                    structural_complete: true,
                },
                &configuration_identity,
                repository_identity,
                schema_fingerprint,
                &slot_namespace,
                state.generation.records.len() as u32,
                root,
            );
            descriptor.composite = parts;
            drafts.push(Draft {
                descriptor,
                data: JobData::CompositeSort {
                    columns,
                    descending,
                    records: &state.generation.records,
                },
                estimated_work: 0,
            });
        }
        for (path, column) in &state.sorting.columns {
            let field = FieldIdentity {
                path: path.segments.clone(),
                binding: None,
                role: BindingRole::DeclaredField,
                logical_type: Some(column.kind.clone()),
                multi: false,
            };
            for (kind, descending) in [
                (ComponentKind::SortAscending, false),
                (ComponentKind::SortDescending, true),
            ] {
                drafts.push(sort_draft(
                    collection,
                    field.clone(),
                    column,
                    &state.generation.records,
                    descending,
                    kind,
                    &configuration_identity,
                    repository_identity,
                    schema_fingerprint,
                    &slot_namespace,
                    root,
                ));
            }
        }
        for relation in &state.relations {
            for (kind, data) in [
                (
                    ComponentKind::RelationOccurrences,
                    JobData::RelationOccurrences(relation),
                ),
                (
                    ComponentKind::RelationTargets,
                    JobData::RelationTargets(relation),
                ),
                (
                    ComponentKind::RelationStates,
                    JobData::RelationStates(relation),
                ),
            ] {
                drafts.push(relation_draft(
                    state,
                    collection,
                    relation,
                    kind,
                    data,
                    &configuration_identity,
                    repository_identity,
                    schema_fingerprint,
                    &slot_namespace,
                    root,
                ));
            }
        }
    }

    drafts.sort_by(|left, right| {
        left.descriptor
            .identity_key()
            .cmp(&right.descriptor.identity_key())
    });
    for window in drafts.windows(2) {
        if window[0].descriptor.identity_key() == window[1].descriptor.identity_key() {
            return Err(format!(
                "duplicate stable component identity: {}",
                window[0].descriptor.identity_key()
            ));
        }
    }
    for (ordinal, draft) in drafts.iter_mut().enumerate() {
        draft.descriptor.id =
            ComponentId(u32::try_from(ordinal + 1).map_err(|_| "too many stable components")?);
    }

    for draft in &mut drafts {
        draft.estimated_work = estimate_work(&draft.data);
    }
    // IDs stay canonical. Execution is largest-first, with one component per
    // Rayon task so an indexed iterator cannot leave all catalogs in one
    // sequential partition before the small jobs.
    let mut execution = drafts.into_iter().collect::<Vec<_>>();
    execution.sort_by(|left, right| {
        right
            .estimated_work
            .cmp(&left.estimated_work)
            .then_with(|| left.descriptor.id.cmp(&right.descriptor.id))
    });
    let build_epoch = Instant::now();
    let mut results = pool.install(|| {
        execution
            .into_par_iter()
            .with_max_len(1)
            .map(|draft| build_one(draft, build_epoch))
            .collect::<Result<Vec<_>, String>>()
    })?;
    results.sort_by_key(|result| result.descriptor.id);

    let mut components = BTreeMap::new();
    let mut descriptor_list = Vec::with_capacity(results.len());
    let mut build_records = Vec::with_capacity(results.len());
    for result in results {
        descriptor_list.push((result.descriptor.id, result.descriptor.clone()));
        build_records.push(super::ComponentBuildRecord {
            id: result.descriptor.id,
            kind: result.descriptor.kind,
            collection: result.descriptor.collection.as_ref().map(|value| value.id),
            field: result
                .descriptor
                .field
                .as_ref()
                .map(|field| field.path.join(".")),
            entry_count: result.entry_count,
            payload_len: result.payload_len,
            memory_len: result.memory_len,
            memory_capacity: result.memory_capacity,
            bitmap_encodes: result.bitmap_encodes,
            metadata_encodes: result.metadata_encodes,
            tree_inserts: result.tree_inserts,
            duration_ns: result.duration_ns,
            start_offset_ns: result.start_offset_ns,
            end_offset_ns: result.end_offset_ns,
            worker: result.worker,
            estimated_work: result.estimated_work,
        });
        components.insert(
            result.descriptor.id,
            super::ComponentEntry {
                descriptor: result.descriptor,
                state: super::ComponentState::Loaded {
                    tree: result.tree,
                    dirty: true,
                },
                saved_info: None,
            },
        );
    }
    let mut prototype = StablePrototype {
        repository_identity,
        configuration_identity,
        schema_fingerprint,
        slot_namespace,
        collections: BTreeMap::new(),
        components,
        generation: None,
        manifest: None,
        counters: super::PrototypeCounters::default(),
        usable: true,
        unusable_reason: None,
        build_records,
        build_stage_ns: BTreeMap::new(),
        next_file_id: Some(
            files
                .files
                .iter()
                .map(|file| file.id.0.saturating_add(1))
                .max()
                .unwrap_or(0),
        ),
        application_started: false,
    };
    let manifest_collections = collections
        .iter()
        .map(|state| ManifestCollection {
            id: state.generation.collection.0,
            name: state.definition.name.clone(),
            generation: state.generation.generation,
            slot_high_water: u32::try_from(state.generation.records.len()).unwrap_or(u32::MAX),
            live_records: state.generation.records.len() as u64,
            identity_policy: super::PersistedIdentityPolicy::from(&state.definition.identity),
        })
        .collect::<Vec<_>>();
    let core_components = prototype
        .components
        .values()
        .filter_map(|entry| {
            let collection = entry.descriptor.collection.as_ref()?.id;
            Some((collection, entry.descriptor.kind, entry.descriptor.id))
        })
        .fold(
            BTreeMap::<CollectionId, BTreeMap<ComponentKind, ComponentId>>::new(),
            |mut map, (collection, kind, id)| {
                map.entry(collection).or_default().insert(kind, id);
                map
            },
        );
    for entry in prototype.components.values_mut() {
        let Some(collection) = entry.descriptor.collection.as_ref().map(|value| value.id) else {
            continue;
        };
        let Some(core) = core_components.get(&collection) else {
            continue;
        };
        let mut dependencies = Vec::new();
        if entry.descriptor.kind != ComponentKind::Domains {
            if let Some(domains) = core.get(&ComponentKind::Domains) {
                dependencies.push(*domains);
            }
        }
        if !matches!(
            entry.descriptor.kind,
            ComponentKind::Records | ComponentKind::Identities | ComponentKind::Domains
        ) {
            if let Some(records) = core.get(&ComponentKind::Records) {
                dependencies.push(*records);
            }
        }
        entry.descriptor.dependency_ids = dependencies;
    }
    prototype
        .rebuild_collection_index(&manifest_collections, &descriptor_list)
        .map_err(|error| error.to_string())?;
    for state in collections {
        if let Some(collection) = prototype.collections.get_mut(&state.generation.collection) {
            collection.identity_policy =
                super::PersistedIdentityPolicy::from(&state.definition.identity);
            collection.slot_high_water =
                u32::try_from(state.generation.records.len()).unwrap_or(u32::MAX);
            collection.live_records = state.generation.records.len() as u64;
        }
    }
    prototype.counters.bitmap_encodes = prototype
        .build_records
        .iter()
        .map(|record| record.bitmap_encodes)
        .sum();
    prototype.counters.stable_inserts = prototype
        .build_records
        .iter()
        .map(|record| record.tree_inserts)
        .sum();
    Ok(prototype)
}

fn worker_count(workers: Option<usize>) -> usize {
    workers.filter(|count| *count > 0).unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|value| value.get())
            .unwrap_or(1)
    })
}

struct Draft<'a> {
    descriptor: ComponentDescriptor,
    data: JobData<'a>,
    estimated_work: u64,
}

struct BuildResult {
    descriptor: ComponentDescriptor,
    tree: IndexTree,
    entry_count: u64,
    payload_len: u64,
    memory_len: u64,
    memory_capacity: u64,
    bitmap_encodes: u64,
    metadata_encodes: u64,
    tree_inserts: u64,
    duration_ns: u64,
    start_offset_ns: u64,
    end_offset_ns: u64,
    worker: String,
    estimated_work: u64,
}

enum JobData<'a> {
    Files(&'a FileInventory),
    Paths(Vec<(PathKey, StoredBytes)>),
    Records(&'a [RecordEntry]),
    Identities(&'a [RecordEntry]),
    Domains {
        records: &'a [RecordEntry],
        logical: &'a RoaringBitmap,
        empty_array: &'a RoaringBitmap,
    },
    Values(&'a PostingIndex),
    Lengths(&'a PostingIndex),
    States(&'a PostingIndex),
    NormalGrams(&'a BigramIndex),
    SkipGrams(&'a BigramIndex),
    Sort(&'a SortColumn, bool, &'a [RecordEntry]),
    CompositeSort {
        columns: Vec<&'a SortColumn>,
        descending: Vec<bool>,
        records: &'a [RecordEntry],
    },
    RelationOccurrences(&'a RelationIndex),
    RelationTargets(&'a RelationIndex),
    RelationStates(&'a RelationIndex),
}

fn estimate_work(data: &JobData<'_>) -> u64 {
    let bitmap_work = |bitmaps: &[&RoaringBitmap]| {
        bitmaps
            .iter()
            .map(|bitmap| bitmap.len().saturating_mul(8).saturating_add(4))
            .sum::<u64>()
    };
    match data {
        JobData::Files(files) => files.files.len() as u64 * 8,
        JobData::Paths(entries) => entries
            .iter()
            .map(|(key, value)| (key.bytes.len() + value.len()) as u64 + 8)
            .sum(),
        JobData::Records(records) => records.len() as u64 * 4,
        JobData::Identities(records) => records.len() as u64 * 4,
        JobData::Domains { records, .. } => records.len() as u64 * 8,
        JobData::Values(index) => {
            index.values.len() as u64 + bitmap_work(&index.values.values().collect::<Vec<_>>())
        }
        JobData::Lengths(index) => {
            index.lengths.len() as u64 + bitmap_work(&index.lengths.values().collect::<Vec<_>>())
        }
        JobData::States(index) => {
            let mut work = index.values.len() as u64
                + index.lengths.len() as u64
                + bitmap_work(&[&index.presence, &index.empty_selections, &index.nulls]);
            if let Some(facts) = &index.structural {
                work += facts.postings.len() as u64
                    + bitmap_work(&facts.postings.values().collect::<Vec<_>>());
            }
            work
        }
        JobData::NormalGrams(index) => {
            index.normal.len() as u64 + bitmap_work(&index.normal.values().collect::<Vec<_>>())
        }
        JobData::SkipGrams(index) => {
            index.skip.len() as u64 + bitmap_work(&index.skip.values().collect::<Vec<_>>())
        }
        JobData::Sort(column, _, _) => (column.keys.len() as u64).saturating_mul(16),
        JobData::CompositeSort {
            columns, records, ..
        } => (columns.len() as u64)
            .saturating_mul(records.len() as u64)
            .saturating_mul(16),
        JobData::RelationOccurrences(relation) => relation.edge_count() as u64 * 8,
        JobData::RelationTargets(relation) => relation.edge_count() as u64 * 8,
        JobData::RelationStates(relation) => bitmap_work(&[
            relation.present(),
            relation.nulls(),
            relation.empty_arrays(),
            relation.unavailable(),
        ]),
    }
}

struct BuildTree {
    tree: IndexTree,
    inserts: u64,
    bitmap_encodes: u64,
    metadata_encodes: u64,
}

impl BuildTree {
    fn new(kind: ComponentKind) -> Self {
        Self {
            tree: memory::new_tree(kind),
            inserts: 0,
            bitmap_encodes: 0,
            metadata_encodes: 0,
        }
    }

    fn len(&self) -> u64 {
        self.tree.len()
    }

    fn insert(&mut self, key: ComponentKey, value: StoredBytes) -> Option<StoredBytes> {
        self.inserts = self.inserts.saturating_add(1);
        self.metadata_encodes = self.metadata_encodes.saturating_add(1);
        self.tree.insert(key, value)
    }

    fn insert_bitmap(&mut self, key: ComponentKey, bitmap: &RoaringBitmap) -> Result<(), String> {
        self.inserts = self.inserts.saturating_add(1);
        self.bitmap_encodes = self.bitmap_encodes.saturating_add(1);
        self.tree.insert(key, memory::bitmap_value(bitmap)?);
        Ok(())
    }
}

fn build_one(draft: Draft<'_>, epoch: Instant) -> Result<BuildResult, String> {
    let started = Instant::now();
    let start_offset_ns = started.duration_since(epoch).as_nanos() as u64;
    let worker = format!("{:?}", std::thread::current().id());
    let tree = match draft.data {
        JobData::Files(files) => build_files(files)?,
        JobData::Paths(entries) => build_prepared_paths(entries)?,
        JobData::Records(records) => build_records(records)?,
        JobData::Identities(records) => build_identities(records)?,
        JobData::Domains {
            records,
            logical,
            empty_array,
        } => build_domains(records, logical, empty_array)?,
        JobData::Values(index) => build_values(index)?,
        JobData::Lengths(index) => build_lengths(index)?,
        JobData::States(index) => build_states(index)?,
        JobData::NormalGrams(index) => build_grams(index, false)?,
        JobData::SkipGrams(index) => build_grams(index, true)?,
        JobData::Sort(column, descending, records) => build_sort(column, descending, records)?,
        JobData::CompositeSort {
            columns,
            descending,
            records,
        } => build_composite_sort(columns, descending, records)?,
        JobData::RelationOccurrences(relation) => build_relation_occurrences(relation)?,
        JobData::RelationTargets(relation) => build_relation_targets(relation)?,
        JobData::RelationStates(relation) => build_relation_states(relation)?,
    };
    let entry_count = tree.len();
    let bitmap_encodes = tree.bitmap_encodes;
    let metadata_encodes = tree.metadata_encodes;
    let tree_inserts = tree.inserts;
    let (payload, memory_len, memory_capacity) = memory::tree_image(tree.tree);
    let payload_len = payload.len() as u64;
    let tree = memory::load_tree(draft.descriptor.kind, payload)?;
    let finished = Instant::now();
    Ok(BuildResult {
        descriptor: draft.descriptor,
        tree,
        entry_count,
        payload_len,
        memory_len: memory_len as u64,
        memory_capacity: memory_capacity as u64,
        bitmap_encodes,
        metadata_encodes,
        tree_inserts,
        duration_ns: finished.duration_since(started).as_nanos() as u64,
        start_offset_ns,
        end_offset_ns: finished.duration_since(epoch).as_nanos() as u64,
        worker,
        estimated_work: draft.estimated_work,
    })
}

fn build_files(files: &FileInventory) -> Result<BuildTree, String> {
    let mut tree = BuildTree::new(ComponentKind::Files);
    for file in &files.files {
        let value = codec::encode_meta(&FileCatalogRecord {
            entry: file.clone(),
            live: true,
            incarnation: 1,
        })?;
        tree.insert(ComponentKey::FileId(file.id.0), value);
    }
    Ok(tree)
}

fn build_prepared_paths(entries: Vec<(PathKey, StoredBytes)>) -> Result<BuildTree, String> {
    let mut tree = BuildTree::new(ComponentKind::Paths);
    for (key, value) in entries {
        tree.insert(ComponentKey::Path(key), value);
    }
    Ok(tree)
}

fn prepare_paths(
    files: &FileInventory,
    collections: &[CollectionState],
) -> Result<Vec<(PathKey, StoredBytes)>, String> {
    let mut by_file = vec![Vec::<RecordRef>::new(); files.files.len()];
    for state in collections {
        for record in &state.generation.records {
            let window = by_file.get_mut(record.file.0 as usize).ok_or_else(|| {
                format!(
                    "record references file {} which is absent from the inventory",
                    record.file.0
                )
            })?;
            window.push(RecordRef {
                collection: state.generation.collection,
                slot: record.slot,
            });
        }
    }
    let mut entries = Vec::with_capacity(files.files.len());
    let mut seen = BTreeSet::new();
    for (index, file) in files.files.iter().enumerate() {
        if file.id.0 as usize != index {
            return Err("file inventory IDs are not dense".into());
        }
        let key = PathKey::from_path(&file.relative_path)?;
        if !seen.insert(key.clone()) {
            return Err(format!(
                "duplicate physical path in file inventory: {}",
                file.relative_path.display()
            ));
        }
        let mut refs = std::mem::take(&mut by_file[index]);
        refs.sort_unstable();
        refs.dedup();
        let value = codec::encode_meta(&PathCatalogEntry {
            file: file.id,
            refs,
        })?;
        entries.push((key, value));
    }
    Ok(entries)
}

fn build_records(records: &[RecordEntry]) -> Result<BuildTree, String> {
    let mut tree = BuildTree::new(ComponentKind::Records);
    for record in records {
        let value = codec::encode_meta(&RecordCatalogRecord {
            entry: record.clone(),
            live: true,
            incarnation: 1,
        })?;
        tree.insert(ComponentKey::RecordSlot(record.slot.0), value);
    }
    Ok(tree)
}

fn build_identities(records: &[RecordEntry]) -> Result<BuildTree, String> {
    let mut identities = BTreeMap::<String, RoaringBitmap>::new();
    for record in records {
        identities
            .entry(record.id.0.to_string())
            .or_default()
            .insert(record.slot.0);
    }
    let mut tree = BuildTree::new(ComponentKind::Identities);
    for (id, slots) in identities {
        tree.insert_bitmap(ComponentKey::Identity(codec::RecordIdentityKey(id)), &slots)?;
    }
    Ok(tree)
}

fn build_domains(
    records: &[RecordEntry],
    logical: &RoaringBitmap,
    empty_array: &RoaringBitmap,
) -> Result<BuildTree, String> {
    let mut all = RoaringBitmap::new();
    let mut raw_fallback = RoaringBitmap::new();
    for record in records {
        all.insert(record.slot.0);
        if record.class == RecordClass::RawFallback {
            raw_fallback.insert(record.slot.0);
        }
    }
    let mut tree = BuildTree::new(ComponentKind::Domains);
    for (key, bitmap) in [
        (codec::DOMAIN_ALL, &all),
        (codec::DOMAIN_LOGICAL, logical),
        (codec::DOMAIN_RAW_FALLBACK, &raw_fallback),
        (codec::DOMAIN_EMPTY_ARRAY, empty_array),
        (codec::DOMAIN_LIVE, &all),
    ] {
        tree.insert_bitmap(ComponentKey::Domain(key), bitmap)?;
    }
    Ok(tree)
}

fn build_values(index: &PostingIndex) -> Result<BuildTree, String> {
    let mut tree = BuildTree::new(ComponentKind::Values);
    for (key, posting) in &index.values {
        tree.insert_bitmap(ComponentKey::Scalar(key.clone()), posting)?;
    }
    Ok(tree)
}

fn build_lengths(index: &PostingIndex) -> Result<BuildTree, String> {
    let mut tree = BuildTree::new(ComponentKind::Lengths);
    for (key, posting) in &index.lengths {
        tree.insert_bitmap(ComponentKey::Length(key.clone()), posting)?;
    }
    Ok(tree)
}

fn build_states(index: &PostingIndex) -> Result<BuildTree, String> {
    let mut tree = BuildTree::new(ComponentKind::States);
    for (state, posting) in [
        (codec::STATE_PRESENCE, &index.presence),
        (codec::STATE_EMPTY_SELECTION, &index.empty_selections),
        (codec::STATE_LEGACY_NULL, &index.nulls),
    ] {
        tree.insert_bitmap(ComponentKey::State(state), posting)?;
    }
    if let Some(facts) = &index.structural {
        for (key, posting) in &facts.postings {
            let state = match key {
                StructuralKey::AnyNull => codec::STATE_ANY_NULL,
                StructuralKey::NonEmpty => codec::STATE_NON_EMPTY,
                StructuralKey::String => codec::STATE_STRING,
                StructuralKey::Number => codec::STATE_NUMBER,
                StructuralKey::Array => codec::STATE_ARRAY,
                StructuralKey::Object => codec::STATE_OBJECT,
                StructuralKey::EmptyObject => codec::STATE_EMPTY_OBJECT,
            };
            tree.insert_bitmap(ComponentKey::State(state), posting)?;
        }
    }
    Ok(tree)
}

fn build_grams(index: &BigramIndex, skip: bool) -> Result<BuildTree, String> {
    let source = if skip { &index.skip } else { &index.normal };
    let mut tree = BuildTree::new(if skip {
        ComponentKind::SkipGrams
    } else {
        ComponentKind::NormalGrams
    });
    for (gram, posting) in source {
        tree.insert_bitmap(ComponentKey::Gram(*gram), posting)?;
    }
    Ok(tree)
}

fn build_sort(
    column: &SortColumn,
    descending: bool,
    records: &[RecordEntry],
) -> Result<BuildTree, String> {
    if records.len() != column.keys.len() {
        return Err("sort column record coverage mismatch".into());
    }
    let mut tree = BuildTree::new(if descending {
        ComponentKind::SortDescending
    } else {
        ComponentKind::SortAscending
    });
    let mut order = (0..column.keys.len()).collect::<Vec<_>>();
    order.sort_by(|left, right| {
        column.keys[*left]
            .compare_with(&column.keys[*right], descending, column.options)
            .then_with(|| records[*left].id.cmp(&records[*right].id))
            .then_with(|| left.cmp(right))
    });
    for slot in order {
        let slot = RecordSlot(u32::try_from(slot).map_err(|_| "sort slot does not fit in u32")?);
        tree.insert(
            ComponentKey::Sort(SortEntryKey {
                options: column.options,
                descending,
                key: column.keys[slot.0 as usize].clone(),
                tie_break_id: records[slot.0 as usize].id.clone(),
                slot,
            }),
            StoredBytes::empty(),
        );
    }
    Ok(tree)
}

fn build_composite_sort(
    columns: Vec<&SortColumn>,
    descending: Vec<bool>,
    records: &[RecordEntry],
) -> Result<BuildTree, String> {
    if columns.len() != descending.len() || records.is_empty() {
        return Ok(BuildTree::new(ComponentKind::CompositeSort));
    }
    if columns
        .iter()
        .any(|column| column.keys.len() != records.len())
    {
        return Err("composite sort record coverage mismatch".into());
    }
    let mut tree = BuildTree::new(ComponentKind::CompositeSort);
    let mut order = (0..records.len()).collect::<Vec<_>>();
    order.sort_by(|left, right| {
        for (column, descending) in columns.iter().zip(&descending) {
            let order =
                column.keys[*left].compare_with(&column.keys[*right], *descending, column.options);
            if !order.is_eq() {
                return order;
            }
        }
        records[*left]
            .id
            .cmp(&records[*right].id)
            .then_with(|| left.cmp(right))
    });
    for slot in order {
        let slot = RecordSlot(u32::try_from(slot).map_err(|_| "sort slot overflow")?);
        let parts = columns
            .iter()
            .zip(&descending)
            .map(|(column, descending)| codec::CompositePartKey {
                options: column.options,
                descending: *descending,
                key: column.keys[slot.0 as usize].clone(),
            })
            .collect();
        tree.insert(
            ComponentKey::CompositeSort(codec::CompositeSortEntryKey {
                parts,
                tie_break_id: records[slot.0 as usize].id.clone(),
                slot,
            }),
            StoredBytes::empty(),
        );
    }
    Ok(tree)
}

fn build_relation_occurrences(relation: &RelationIndex) -> Result<BuildTree, String> {
    let mut tree = BuildTree::new(ComponentKind::RelationOccurrences);
    for slot in 0..relation_max_slot(relation) {
        let source = RecordSlot(slot);
        let Some(edges) = relation.edges(source) else {
            continue;
        };
        for (ordinal, edge) in edges.iter().enumerate() {
            let ordinal =
                u32::try_from(ordinal).map_err(|_| "relation occurrence ordinal overflow")?;
            let resolved = matches!(edge.target, RelationTarget::Resolved(_));
            let value = codec::encode_meta(&RelationOccurrenceValue {
                target_collection: Some(relation.target_collection),
                target_id: edge.target_id().to_owned(),
                positions: edge.positions.clone(),
                target_resolved: resolved,
            })?;
            tree.insert(
                ComponentKey::Occurrence(RelationOccurrenceKey { source, ordinal }),
                value,
            );
        }
    }
    Ok(tree)
}

fn build_relation_targets(relation: &RelationIndex) -> Result<BuildTree, String> {
    let mut targets = BTreeMap::<String, RoaringBitmap>::new();
    for slot in 0..relation_max_slot(relation) {
        let Some(edges) = relation.edges(RecordSlot(slot)) else {
            continue;
        };
        for edge in edges {
            targets
                .entry(edge.target_id().to_owned())
                .or_default()
                .insert(slot);
        }
    }
    let mut tree = BuildTree::new(ComponentKind::RelationTargets);
    for (target_id, sources) in targets {
        tree.insert_bitmap(
            ComponentKey::RelationTarget(RelationTargetKey {
                target_collection: relation.target_collection,
                target_id,
            }),
            &sources,
        )?;
    }
    Ok(tree)
}

fn build_relation_states(relation: &RelationIndex) -> Result<BuildTree, String> {
    let mut tree = BuildTree::new(ComponentKind::RelationStates);
    for (state, bitmap) in [
        (codec::RELATION_PRESENT, relation.present()),
        (codec::RELATION_NULLS, relation.nulls()),
        (codec::RELATION_EMPTY_ARRAY, relation.empty_arrays()),
        (codec::RELATION_UNAVAILABLE, relation.unavailable()),
    ] {
        tree.insert_bitmap(ComponentKey::RelationState(state), bitmap)?;
    }
    Ok(tree)
}

fn relation_max_slot(relation: &RelationIndex) -> u32 {
    let mut all = relation.present().clone();
    all |= relation.nulls();
    all |= relation.empty_arrays();
    all |= relation.unavailable();
    all.max().map_or(0, |max| max.saturating_add(1))
}

#[allow(clippy::too_many_arguments)]
fn files_draft<'a>(
    files: &'a FileInventory,
    configuration_identity: &[u8; 32],
    repository_identity: [u8; 32],
    schema_fingerprint: Option<[u8; 32]>,
    slot_namespace: &str,
    root: &std::path::Path,
) -> Draft<'a> {
    Draft {
        descriptor: descriptor(
            ComponentKind::Files,
            None,
            None,
            None,
            None,
            CoverageDescriptor::default(),
            configuration_identity,
            repository_identity,
            schema_fingerprint,
            slot_namespace,
            0,
            root,
        ),
        data: JobData::Files(files),
        estimated_work: 0,
    }
}

#[allow(clippy::too_many_arguments)]
fn paths_draft<'a>(
    entries: Vec<(PathKey, StoredBytes)>,
    configuration_identity: &[u8; 32],
    repository_identity: [u8; 32],
    schema_fingerprint: Option<[u8; 32]>,
    slot_namespace: &str,
    root: &std::path::Path,
) -> Draft<'a> {
    Draft {
        descriptor: descriptor(
            ComponentKind::Paths,
            None,
            None,
            None,
            None,
            CoverageDescriptor::default(),
            configuration_identity,
            repository_identity,
            schema_fingerprint,
            slot_namespace,
            0,
            root,
        ),
        data: JobData::Paths(entries),
        estimated_work: 0,
    }
}

fn records_draft<'a>(
    state: &'a CollectionState,
    collection: &CollectionIdentity,
    configuration_identity: &[u8; 32],
    repository_identity: [u8; 32],
    schema_fingerprint: Option<[u8; 32]>,
    slot_namespace: &str,
    root: &std::path::Path,
) -> Draft<'a> {
    Draft {
        descriptor: collection_descriptor(
            ComponentKind::Records,
            collection,
            CoverageDescriptor::default(),
            configuration_identity,
            repository_identity,
            schema_fingerprint,
            slot_namespace,
            root,
            state.generation.records.len(),
        ),
        data: JobData::Records(state.generation.records.as_slice()),
        estimated_work: 0,
    }
}

fn identities_draft<'a>(
    state: &'a CollectionState,
    collection: &CollectionIdentity,
    configuration_identity: &[u8; 32],
    repository_identity: [u8; 32],
    schema_fingerprint: Option<[u8; 32]>,
    slot_namespace: &str,
    root: &std::path::Path,
) -> Draft<'a> {
    Draft {
        descriptor: collection_descriptor(
            ComponentKind::Identities,
            collection,
            CoverageDescriptor::default(),
            configuration_identity,
            repository_identity,
            schema_fingerprint,
            slot_namespace,
            root,
            state.generation.records.len(),
        ),
        data: JobData::Identities(state.generation.records.as_slice()),
        estimated_work: 0,
    }
}

fn domains_draft<'a>(
    state: &'a CollectionState,
    collection: &CollectionIdentity,
    configuration_identity: &[u8; 32],
    repository_identity: [u8; 32],
    schema_fingerprint: Option<[u8; 32]>,
    slot_namespace: &str,
    root: &std::path::Path,
) -> Draft<'a> {
    Draft {
        descriptor: collection_descriptor(
            ComponentKind::Domains,
            collection,
            CoverageDescriptor::default(),
            configuration_identity,
            repository_identity,
            schema_fingerprint,
            slot_namespace,
            root,
            state.generation.records.len(),
        ),
        data: JobData::Domains {
            records: state.generation.records.as_slice(),
            logical: &state.logical_records,
            empty_array: &state.empty_array_records,
        },
        estimated_work: 0,
    }
}

#[allow(clippy::too_many_arguments)]
fn posting_draft<'a>(
    state: &CollectionState,
    collection: &CollectionIdentity,
    field: FieldIdentity,
    kind: ComponentKind,
    coverage: CoverageDescriptor,
    data: JobData<'a>,
    configuration_identity: &[u8; 32],
    repository_identity: [u8; 32],
    schema_fingerprint: Option<[u8; 32]>,
    slot_namespace: &str,
    root: &std::path::Path,
) -> Draft<'a> {
    Draft {
        descriptor: descriptor(
            kind,
            Some(collection.clone()),
            Some(field),
            None,
            None,
            coverage,
            configuration_identity,
            repository_identity,
            schema_fingerprint,
            slot_namespace,
            state.generation.records.len() as u32,
            root,
        ),
        data,
        estimated_work: 0,
    }
}

#[allow(clippy::too_many_arguments)]
fn append_text_drafts<'a>(
    drafts: &mut Vec<Draft<'a>>,
    state: &'a CollectionState,
    collection: &CollectionIdentity,
    role: BindingRole,
    path: Vec<String>,
    binding: Option<String>,
    index: &'a BigramIndex,
    configuration_identity: &[u8; 32],
    repository_identity: [u8; 32],
    schema_fingerprint: Option<[u8; 32]>,
    slot_namespace: &str,
    root: &std::path::Path,
) {
    let field = FieldIdentity {
        path,
        binding,
        role,
        logical_type: Some(crate::model::LogicalType::String),
        multi: true,
    };
    let coverage = CoverageDescriptor {
        values_indexed: true,
        value_complete: true,
        length_complete: false,
        empty_string_complete: false,
        covered_records: index.record_count,
        complete_for_field: true,
        structural_complete: true,
    };
    for (kind, data) in [
        (ComponentKind::NormalGrams, JobData::NormalGrams(index)),
        (ComponentKind::SkipGrams, JobData::SkipGrams(index)),
    ] {
        let mut draft = posting_draft(
            state,
            collection,
            field.clone(),
            kind,
            coverage.clone(),
            data,
            configuration_identity,
            repository_identity,
            schema_fingerprint,
            slot_namespace,
            root,
        );
        draft.descriptor.text = Some(codec::TextRepresentationDescriptor {
            alphabet: index.alphabet,
            gram_role: if kind == ComponentKind::SkipGrams {
                1
            } else {
                0
            },
            extractor: "objekte-text".into(),
            extractor_version: crate::bigram::BIGRAM_VERSION,
            normalization: crate::bigram::FOLDED_REPRESENTATION.into(),
            segment_identity: "reader-segments-v1".into(),
        });
        drafts.push(draft);
    }
}

#[allow(clippy::too_many_arguments)]
fn sort_draft<'a>(
    collection: &CollectionIdentity,
    field: FieldIdentity,
    column: &'a SortColumn,
    records: &'a [RecordEntry],
    descending: bool,
    kind: ComponentKind,
    configuration_identity: &[u8; 32],
    repository_identity: [u8; 32],
    schema_fingerprint: Option<[u8; 32]>,
    slot_namespace: &str,
    root: &std::path::Path,
) -> Draft<'a> {
    Draft {
        descriptor: descriptor(
            kind,
            Some(collection.clone()),
            Some(field),
            Some(OrderingDescriptor {
                version: 1,
                locale: column.locale.clone(),
                collation_version: column.collation_version.clone(),
                options: column.options,
                descending,
            }),
            None,
            CoverageDescriptor {
                values_indexed: true,
                value_complete: true,
                length_complete: false,
                empty_string_complete: false,
                covered_records: column.keys.len() as u32,
                complete_for_field: true,
                structural_complete: true,
            },
            configuration_identity,
            repository_identity,
            schema_fingerprint,
            slot_namespace,
            column.keys.len() as u32,
            root,
        ),
        data: JobData::Sort(column, descending, records),
        estimated_work: 0,
    }
}

#[allow(clippy::too_many_arguments)]
fn relation_draft<'a>(
    state: &CollectionState,
    collection: &CollectionIdentity,
    relation: &'a RelationIndex,
    kind: ComponentKind,
    data: JobData<'a>,
    configuration_identity: &[u8; 32],
    repository_identity: [u8; 32],
    schema_fingerprint: Option<[u8; 32]>,
    slot_namespace: &str,
    root: &std::path::Path,
) -> Draft<'a> {
    let field = FieldIdentity {
        path: relation.declaration.path.clone(),
        binding: Some(relation.declaration.target_collection.clone()),
        role: BindingRole::DeclaredField,
        logical_type: Some(crate::model::LogicalType::Id),
        multi: relation.declaration.cardinality == crate::relation::RelationCardinality::Many,
    };
    let mut descriptor = descriptor(
        kind,
        Some(collection.clone()),
        Some(field),
        None,
        Some(relation.target_collection),
        CoverageDescriptor::default(),
        configuration_identity,
        repository_identity,
        schema_fingerprint,
        slot_namespace,
        state.generation.records.len() as u32,
        root,
    );
    descriptor.target_collection = Some(relation.target_collection);
    Draft {
        descriptor,
        data,
        estimated_work: 0,
    }
}

#[allow(clippy::too_many_arguments)]
fn collection_descriptor(
    kind: ComponentKind,
    collection: &CollectionIdentity,
    coverage: CoverageDescriptor,
    configuration_identity: &[u8; 32],
    repository_identity: [u8; 32],
    schema_fingerprint: Option<[u8; 32]>,
    slot_namespace: &str,
    root: &std::path::Path,
    slot_high_water: usize,
) -> ComponentDescriptor {
    descriptor(
        kind,
        Some(collection.clone()),
        None,
        None,
        None,
        coverage,
        configuration_identity,
        repository_identity,
        schema_fingerprint,
        slot_namespace,
        slot_high_water as u32,
        root,
    )
}

#[allow(clippy::too_many_arguments)]
fn descriptor(
    kind: ComponentKind,
    collection: Option<CollectionIdentity>,
    field: Option<FieldIdentity>,
    ordering: Option<OrderingDescriptor>,
    target_collection: Option<CollectionId>,
    coverage: CoverageDescriptor,
    configuration_identity: &[u8; 32],
    repository_identity: [u8; 32],
    schema_fingerprint: Option<[u8; 32]>,
    slot_namespace: &str,
    slot_high_water: u32,
    root: &std::path::Path,
) -> ComponentDescriptor {
    ComponentDescriptor {
        descriptor_version: codec::DESCRIPTOR_FORMAT_VERSION,
        id: ComponentId(0),
        kind,
        key_codec_version: codec::KEY_CODEC_VERSION,
        value_codec_version: codec::VALUE_CODEC_VERSION,
        key_bound: key_bound_for(kind).into(),
        collection,
        field,
        ordering,
        text: None,
        composite: Vec::new(),
        target_collection,
        coverage,
        repository_identity,
        configuration_identity: *configuration_identity,
        schema_fingerprint,
        slot_namespace: slot_namespace.into(),
        slot_high_water,
        origin_revision: Some(1),
        origin_path: Some(root.to_path_buf()),
        dependency_ids: Vec::new(),
        detached: false,
        component_revision: 1,
    }
}

fn key_bound_for(kind: ComponentKind) -> &'static str {
    match kind {
        ComponentKind::Files | ComponentKind::Records => "fixed:u32",
        ComponentKind::Paths => "unbounded:path",
        ComponentKind::Identities => "unbounded:identity",
        ComponentKind::Domains | ComponentKind::States | ComponentKind::RelationStates => {
            "fixed:u8"
        }
        ComponentKind::Values => "unbounded:scalar",
        ComponentKind::Lengths => "unbounded:number",
        ComponentKind::NormalGrams | ComponentKind::SkipGrams => "fixed:u64",
        ComponentKind::SortAscending | ComponentKind::SortDescending => "unbounded:sort",
        ComponentKind::RelationOccurrences => "fixed:u64",
        ComponentKind::RelationTargets => "unbounded:relation-target",
        ComponentKind::CompositeSort => "unbounded:composite-sort",
    }
}

fn configuration_identity(collections: &[CollectionState]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"objekte-icss-configuration-v2");
    for state in collections {
        let definition = &state.definition;
        hash_text(&mut hash, &definition.name);
        hash_path(&mut hash, &definition.source_path);
        hash_text(&mut hash, definition.format.id());
        match &definition.identity {
            crate::repository::IdentityPolicy::Filename => hash_u64(&mut hash, 1),
            crate::repository::IdentityPolicy::RelativePath => hash_u64(&mut hash, 2),
            crate::repository::IdentityPolicy::Generated => hash_u64(&mut hash, 3),
            crate::repository::IdentityPolicy::Intrinsic(fields) => {
                hash_u64(&mut hash, 4);
                hash_strings(&mut hash, fields);
            }
        }
        hash_u64(&mut hash, u64::from(definition.discovery.recursive));
        hash_u64(&mut hash, u64::from(definition.discovery.hidden));
        hash_u64(
            &mut hash,
            match definition.discovery.symlinks {
                crate::discovery::SymlinkPolicy::Skip => 0,
                crate::discovery::SymlinkPolicy::FollowWithinRoot => 1,
            },
        );
        let mut include = definition.discovery.include.clone();
        include.sort();
        hash_strings(&mut hash, &include);
        let mut exclude = definition.discovery.exclude.clone();
        exclude.sort();
        hash_strings(&mut hash, &exclude);
        let mut excluded_files = definition
            .discovery
            .exclude_files
            .iter()
            .map(|path| super::PathKey::from_path(path).map(|key| key.bytes))
            .collect::<Result<Vec<_>, _>>()
            .unwrap_or_default();
        excluded_files.sort();
        for path in excluded_files {
            hash_bytes(&mut hash, &path);
        }
        let mut excluded_dirs = definition
            .discovery
            .exclude_directories
            .iter()
            .map(|path| super::PathKey::from_path(path).map(|key| key.bytes))
            .collect::<Result<Vec<_>, _>>()
            .unwrap_or_default();
        excluded_dirs.sort();
        for path in excluded_dirs {
            hash_bytes(&mut hash, &path);
        }
        hash_text(&mut hash, &definition.sort_locale);
        hash_sort_options(&mut hash, definition.sort_options);
        let mut fields = definition.schema.fields().iter().collect::<Vec<_>>();
        fields.sort_by(|left, right| left.0.cmp(right.0));
        for (path, field) in fields {
            hash_strings(&mut hash, path);
            hash_serde(&mut hash, &field.kind).expect("field kind serialization");
            hash_u64(&mut hash, u64::from(field.required));
            hash_u64(&mut hash, u64::from(field.nullable));
            hash_u64(&mut hash, u64::from(field.indexed));
            hash_u64(&mut hash, u64::from(field.multi));
            hash_u64(&mut hash, field.element_nullable.len() as u64);
            for nullable in &field.element_nullable {
                hash_u64(&mut hash, u64::from(*nullable));
            }
        }
        let mut custom = definition
            .schema
            .custom_bindings()
            .iter()
            .collect::<Vec<_>>();
        custom.sort_by(|left, right| left.0.cmp(right.0));
        for (key, binding) in custom {
            hash_strings(&mut hash, &key.0);
            hash_text(&mut hash, &key.1);
            hash_text(&mut hash, &binding.name);
            hash_u64(&mut hash, binding.version as u64);
            hash_u64(
                &mut hash,
                match binding.transform {
                    crate::reference::Transform::Identity => 0,
                    crate::reference::Transform::CaseFold => 1,
                },
            );
        }
        let mut operations = definition.schema.operations.iter().collect::<Vec<_>>();
        operations.sort_by(|left, right| left.0.cmp(right.0));
        for (key, binding) in operations {
            hash_strings(&mut hash, &key.0);
            hash_text(&mut hash, &key.1);
            hash_text(&mut hash, &binding.name);
            hash_u64(&mut hash, binding.version as u64);
        }
        let mut relations = definition.schema.relations().iter().collect::<Vec<_>>();
        relations.sort_by(|left, right| left.0.cmp(right.0));
        for (path, relation) in relations {
            hash_strings(&mut hash, path);
            hash_text(&mut hash, &relation.target_collection);
            hash_u64(
                &mut hash,
                match relation.cardinality {
                    crate::relation::RelationCardinality::One => 1,
                    crate::relation::RelationCardinality::Many => 2,
                },
            );
            hash_opt_text(&mut hash, relation.reverse_alias.as_deref());
        }
        let mut sorts = definition.sort_fields.clone();
        sorts.sort_by_key(|sort| sort.display());
        for sort in sorts {
            hash_serde(&mut hash, &sort).expect("sort field serialization");
        }
        let mut overrides = definition.sort_overrides.iter().collect::<Vec<_>>();
        overrides.sort_by_key(|(path, _)| path.display());
        for (path, config) in overrides {
            hash_serde(&mut hash, path).expect("sort path serialization");
            hash_text(&mut hash, &config.locale);
            hash_sort_options(&mut hash, config.options);
        }
        for composite in &definition.schema.composite_sorts {
            hash_u64(&mut hash, composite.len() as u64);
            for field in composite {
                hash_serde(&mut hash, field).expect("composite sort serialization");
            }
        }
        if let Some(fingerprint) = definition.schema.source_fingerprint() {
            hash_bytes(&mut hash, &fingerprint);
        }
        hash_u64(&mut hash, collection_alphabet_code(state.raw.alphabet));
    }
    hash.finalize().into()
}

fn collection_alphabet_code(alphabet: crate::workload::BigramAlphabet) -> u64 {
    match alphabet {
        crate::workload::BigramAlphabet::Ascii => 0,
        crate::workload::BigramAlphabet::Unicode => 1,
    }
}

fn hash_sort_options(hash: &mut Sha256, options: crate::sorting::SortOptions) {
    let strength = match options.strength {
        crate::sorting::CollationStrength::LocaleDefault => 0,
        crate::sorting::CollationStrength::Primary => 1,
        crate::sorting::CollationStrength::Secondary => 2,
        crate::sorting::CollationStrength::Tertiary => 3,
        crate::sorting::CollationStrength::Quaternary => 4,
        crate::sorting::CollationStrength::Identical => 5,
    };
    let nulls = match options.nulls {
        crate::sorting::Placement::First => 0,
        crate::sorting::Placement::Last => 1,
    };
    let missing = match options.missing {
        crate::sorting::Placement::First => 0,
        crate::sorting::Placement::Last => 1,
    };
    hash_u64(hash, strength);
    hash_u64(hash, u64::from(options.numeric));
    hash_u64(hash, nulls);
    hash_u64(hash, missing);
}

fn hash_u64(hash: &mut Sha256, value: u64) {
    hash.update(value.to_le_bytes());
}

fn hash_text(hash: &mut Sha256, value: &str) {
    hash_bytes(hash, value.as_bytes());
}

fn hash_opt_text(hash: &mut Sha256, value: Option<&str>) {
    match value {
        Some(value) => {
            hash_u64(hash, 1);
            hash_text(hash, value);
        }
        None => hash_u64(hash, 0),
    }
}

fn hash_strings(hash: &mut Sha256, values: &[String]) {
    hash_u64(hash, values.len() as u64);
    for value in values {
        hash_text(hash, value);
    }
}

fn hash_bytes(hash: &mut Sha256, value: &[u8]) {
    hash_u64(hash, value.len() as u64);
    hash.update(value);
}

fn hash_path(hash: &mut Sha256, path: &Path) {
    match super::PathKey::from_path(path) {
        Ok(key) => {
            hash_u64(hash, key.platform as u64);
            hash_bytes(hash, &key.bytes);
        }
        Err(_) => hash_bytes(hash, path.to_string_lossy().as_bytes()),
    }
}

fn repository_identity(
    files: &FileInventory,
    collections: &[CollectionState],
) -> Result<[u8; 32], String> {
    let mut hash = Sha256::new();
    hash.update(b"objekte-icss-repository-v1");
    for file in &files.files {
        hash_serde(&mut hash, file)?;
    }
    for state in collections {
        hash_text(&mut hash, &state.definition.name);
        hash_u64(&mut hash, state.generation.generation);
        for record in &state.generation.records {
            hash_u64(&mut hash, record.slot.0 as u64);
            hash_text(&mut hash, &record.id.0);
            hash_u64(&mut hash, record.file.0 as u64);
            hash_u64(&mut hash, record.range.start);
            hash_u64(&mut hash, record.range.end);
            hash_u64(&mut hash, record.class.code() as u64);
        }
    }
    Ok(hash.finalize().into())
}

fn slot_namespace(
    root: &Path,
    configuration_identity: [u8; 32],
    repository_identity: [u8; 32],
) -> String {
    let mut hash = Sha256::new();
    hash.update(b"objekte-icss-slot-namespace-v1");
    hash.update(configuration_identity);
    hash.update(repository_identity);
    hash_path(&mut hash, root);
    codec::hex_digest(&hash.finalize().into())
}

fn hash_serde<T: Serialize>(hash: &mut Sha256, value: &T) -> Result<(), String> {
    let bytes = bincode::serialize(value).map_err(|error| error.to_string())?;
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
    Ok(())
}
