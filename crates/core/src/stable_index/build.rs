//! Construction of independent component trees from native build results.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use rayon::prelude::*;
use roaring::RoaringBitmap;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::bigram::BigramIndex;
use crate::inventory::{FileInventory, RecordClass, RecordEntry};
use crate::model::{CollectionId, FileId, RecordSlot};
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
    let configuration_identity = configuration_identity(repository);
    let repository_identity = repository_identity(repository)?;
    let slot_namespace = slot_namespace(repository, configuration_identity, repository_identity);
    let schema_fingerprint = repository
        .collections()
        .first()
        .and_then(|state| state.definition.schema.source_fingerprint());
    let root = repository.files().root.clone();
    let collection_identities = repository
        .collections()
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
        repository.files(),
        &configuration_identity,
        repository_identity,
        schema_fingerprint,
        &slot_namespace,
        &root,
    ));
    drafts.push(paths_draft(
        prepare_paths(repository)?,
        &configuration_identity,
        repository_identity,
        schema_fingerprint,
        &slot_namespace,
        &root,
    ));
    for state in repository.collections() {
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
                &root,
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
                &root,
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
                &root,
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
                    &root,
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
            &root,
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
            &root,
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
                &root,
            );
        }
        if !state.sorting.composites.is_empty() {
            return Err(
                "configured composite sorts are not implemented by the first stable prototype"
                    .into(),
            );
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
                    descending,
                    kind,
                    &configuration_identity,
                    repository_identity,
                    schema_fingerprint,
                    &slot_namespace,
                    &root,
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
                    &root,
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

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(worker_count(workers))
        .thread_name(|index| format!("objekte-icss-build-{index}"))
        .build()
        .map_err(|error| error.to_string())?;
    let build_epoch = Instant::now();
    let results = pool.install(|| {
        drafts
            .into_par_iter()
            .map(|draft| build_one(draft, build_epoch))
            .collect::<Result<Vec<_>, String>>()
    })?;

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
            duration_ns: result.duration_ns,
            start_offset_ns: result.start_offset_ns,
            end_offset_ns: result.end_offset_ns,
            worker: result.worker,
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
        build_records,
    };
    let manifest_collections = repository
        .collections()
        .iter()
        .map(|state| ManifestCollection {
            id: state.generation.collection.0,
            name: state.definition.name.clone(),
            generation: state.generation.generation,
            slot_high_water: u32::try_from(state.generation.records.len()).unwrap_or(u32::MAX),
            live_records: state.generation.records.len() as u64,
            identity_policy: format!("{:?}", state.definition.identity),
        })
        .collect::<Vec<_>>();
    prototype
        .rebuild_collection_index(&manifest_collections, &descriptor_list)
        .map_err(|error| error.to_string())?;
    for state in repository.collections() {
        if let Some(collection) = prototype.collections.get_mut(&state.generation.collection) {
            collection.identity_policy = format!("{:?}", state.definition.identity);
            collection.slot_high_water =
                u32::try_from(state.generation.records.len()).unwrap_or(u32::MAX);
            collection.live_records = state.generation.records.len() as u64;
        }
    }
    // Every posting component encodes its final bitmap exactly once during
    // construction.  Metadata catalogs use their own checked encoding.
    prototype.counters.bitmap_encodes = prototype
        .build_records
        .iter()
        .filter(|record| {
            matches!(
                record.kind,
                ComponentKind::Values
                    | ComponentKind::Lengths
                    | ComponentKind::States
                    | ComponentKind::NormalGrams
                    | ComponentKind::SkipGrams
                    | ComponentKind::Identities
                    | ComponentKind::Domains
                    | ComponentKind::RelationTargets
                    | ComponentKind::RelationStates
            )
        })
        .map(|record| record.entry_count)
        .sum();
    prototype.counters.stable_inserts = prototype
        .build_records
        .iter()
        .map(|record| record.entry_count)
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
}

struct BuildResult {
    descriptor: ComponentDescriptor,
    tree: IndexTree,
    entry_count: u64,
    payload_len: u64,
    duration_ns: u64,
    start_offset_ns: u64,
    end_offset_ns: u64,
    worker: String,
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
    Sort(&'a SortColumn, bool),
    RelationOccurrences(&'a RelationIndex),
    RelationTargets(&'a RelationIndex),
    RelationStates(&'a RelationIndex),
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
        JobData::Sort(column, descending) => build_sort(column, descending)?,
        JobData::RelationOccurrences(relation) => build_relation_occurrences(relation)?,
        JobData::RelationTargets(relation) => build_relation_targets(relation)?,
        JobData::RelationStates(relation) => build_relation_states(relation)?,
    };
    let entry_count = tree.len();
    let payload = memory::tree_payload(tree);
    let payload_len = payload.len() as u64;
    let tree = memory::load_tree(payload)?;
    let finished = Instant::now();
    Ok(BuildResult {
        descriptor: draft.descriptor,
        tree,
        entry_count,
        payload_len,
        duration_ns: finished.duration_since(started).as_nanos() as u64,
        start_offset_ns,
        end_offset_ns: finished.duration_since(epoch).as_nanos() as u64,
        worker,
    })
}

fn build_files(files: &FileInventory) -> Result<IndexTree, String> {
    let mut tree = memory::new_tree();
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

fn build_prepared_paths(entries: Vec<(PathKey, StoredBytes)>) -> Result<IndexTree, String> {
    let mut tree = memory::new_tree();
    for (key, value) in entries {
        tree.insert(ComponentKey::Path(key), value);
    }
    Ok(tree)
}

fn prepare_paths(repository: &Repository) -> Result<Vec<(PathKey, StoredBytes)>, String> {
    let mut by_file = BTreeMap::<FileId, Vec<RecordRef>>::new();
    for state in repository.collections() {
        for record in &state.generation.records {
            by_file.entry(record.file).or_default().push(RecordRef {
                collection: state.generation.collection,
                slot: record.slot,
            });
        }
    }
    let mut entries = Vec::with_capacity(repository.files().files.len());
    let mut seen = BTreeSet::new();
    for file in &repository.files().files {
        let key = PathKey::from_path(&file.relative_path)?;
        if !seen.insert(key.clone()) {
            return Err(format!(
                "duplicate physical path in file inventory: {}",
                file.relative_path.display()
            ));
        }
        let mut refs = by_file.remove(&file.id).unwrap_or_default();
        refs.sort_unstable();
        refs.dedup();
        let value = codec::encode_meta(&PathCatalogEntry {
            file: file.id,
            refs,
        })?;
        entries.push((key, value));
    }
    if let Some((file, _)) = by_file.into_iter().next() {
        return Err(format!(
            "record references file {} which is absent from the inventory",
            file.0
        ));
    }
    Ok(entries)
}

fn build_records(records: &[RecordEntry]) -> Result<IndexTree, String> {
    let mut tree = memory::new_tree();
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

fn build_identities(records: &[RecordEntry]) -> Result<IndexTree, String> {
    let mut identities = BTreeMap::<String, RoaringBitmap>::new();
    for record in records {
        identities
            .entry(record.id.0.to_string())
            .or_default()
            .insert(record.slot.0);
    }
    let mut tree = memory::new_tree();
    for (id, slots) in identities {
        let value = memory::bitmap_value(&slots)?;
        tree.insert(ComponentKey::Identity(codec::RecordIdentityKey(id)), value);
    }
    Ok(tree)
}

fn build_domains(
    records: &[RecordEntry],
    logical: &RoaringBitmap,
    empty_array: &RoaringBitmap,
) -> Result<IndexTree, String> {
    let mut all = RoaringBitmap::new();
    let mut raw_fallback = RoaringBitmap::new();
    for record in records {
        all.insert(record.slot.0);
        if record.class == RecordClass::RawFallback {
            raw_fallback.insert(record.slot.0);
        }
    }
    let mut tree = memory::new_tree();
    for (key, bitmap) in [
        (codec::DOMAIN_ALL, &all),
        (codec::DOMAIN_LOGICAL, logical),
        (codec::DOMAIN_RAW_FALLBACK, &raw_fallback),
        (codec::DOMAIN_EMPTY_ARRAY, empty_array),
        (codec::DOMAIN_LIVE, &all),
    ] {
        tree.insert(ComponentKey::Domain(key), memory::bitmap_value(bitmap)?);
    }
    Ok(tree)
}

fn build_values(index: &PostingIndex) -> Result<IndexTree, String> {
    let mut tree = memory::new_tree();
    for (key, posting) in &index.values {
        tree.insert(
            ComponentKey::Scalar(key.clone()),
            memory::bitmap_value(posting)?,
        );
    }
    Ok(tree)
}

fn build_lengths(index: &PostingIndex) -> Result<IndexTree, String> {
    let mut tree = memory::new_tree();
    for (key, posting) in &index.lengths {
        tree.insert(
            ComponentKey::Length(key.clone()),
            memory::bitmap_value(posting)?,
        );
    }
    Ok(tree)
}

fn build_states(index: &PostingIndex) -> Result<IndexTree, String> {
    let mut tree = memory::new_tree();
    for (state, posting) in [
        (codec::STATE_PRESENCE, &index.presence),
        (codec::STATE_EMPTY_SELECTION, &index.empty_selections),
        (codec::STATE_LEGACY_NULL, &index.nulls),
    ] {
        tree.insert(ComponentKey::State(state), memory::bitmap_value(posting)?);
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
            tree.insert(ComponentKey::State(state), memory::bitmap_value(posting)?);
        }
    }
    Ok(tree)
}

fn build_grams(index: &BigramIndex, skip: bool) -> Result<IndexTree, String> {
    let source = if skip { &index.skip } else { &index.normal };
    let mut tree = memory::new_tree();
    for (gram, posting) in source {
        tree.insert(ComponentKey::Gram(*gram), memory::bitmap_value(posting)?);
    }
    Ok(tree)
}

fn build_sort(column: &SortColumn, descending: bool) -> Result<IndexTree, String> {
    let mut tree = memory::new_tree();
    for (slot, key) in column.keys.iter().enumerate() {
        let slot = RecordSlot(u32::try_from(slot).map_err(|_| "sort slot does not fit in u32")?);
        tree.insert(
            ComponentKey::Sort(SortEntryKey {
                options: column.options,
                descending,
                key: key.clone(),
                slot,
            }),
            StoredBytes::empty(),
        );
    }
    Ok(tree)
}

fn build_relation_occurrences(relation: &RelationIndex) -> Result<IndexTree, String> {
    let mut tree = memory::new_tree();
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

fn build_relation_targets(relation: &RelationIndex) -> Result<IndexTree, String> {
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
    let mut tree = memory::new_tree();
    for (target_id, sources) in targets {
        tree.insert(
            ComponentKey::RelationTarget(RelationTargetKey {
                target_collection: relation.target_collection,
                target_id,
            }),
            memory::bitmap_value(&sources)?,
        );
    }
    Ok(tree)
}

fn build_relation_states(relation: &RelationIndex) -> Result<IndexTree, String> {
    let mut tree = memory::new_tree();
    for (state, bitmap) in [
        (codec::RELATION_PRESENT, relation.present()),
        (codec::RELATION_NULLS, relation.nulls()),
        (codec::RELATION_EMPTY_ARRAY, relation.empty_arrays()),
        (codec::RELATION_UNAVAILABLE, relation.unavailable()),
    ] {
        tree.insert(
            ComponentKey::RelationState(state),
            memory::bitmap_value(bitmap)?,
        );
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
    };
    for (kind, data) in [
        (ComponentKind::NormalGrams, JobData::NormalGrams(index)),
        (ComponentKind::SkipGrams, JobData::SkipGrams(index)),
    ] {
        drafts.push(posting_draft(
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
        ));
    }
}

#[allow(clippy::too_many_arguments)]
fn sort_draft<'a>(
    collection: &CollectionIdentity,
    field: FieldIdentity,
    column: &'a SortColumn,
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
            },
            configuration_identity,
            repository_identity,
            schema_fingerprint,
            slot_namespace,
            column.keys.len() as u32,
            root,
        ),
        data: JobData::Sort(column, descending),
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
    Draft { descriptor, data }
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
        key_bound: "unbounded:component-key-v1".into(),
        collection,
        field,
        ordering,
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
    }
}

fn configuration_identity(repository: &Repository) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"objekte-icss-configuration-v1");
    for state in repository.collections() {
        let definition = &state.definition;
        hash_field(&mut hash, &definition.name);
        hash_field(&mut hash, &definition.source_path);
        hash_field(&mut hash, &format!("{:?}", definition.format));
        hash_field(&mut hash, &format!("{:?}", definition.identity));
        hash_field(&mut hash, &format!("{:?}", definition.discovery));
        hash_field(&mut hash, &definition.sort_locale);
        hash_field(&mut hash, &format!("{:?}", definition.sort_options));
        let mut fields = definition.schema.fields().iter().collect::<Vec<_>>();
        fields.sort_by(|left, right| left.0.cmp(right.0));
        for (path, field) in fields {
            hash_field(&mut hash, path);
            hash_field(&mut hash, &format!("{:?}", field.kind));
            hash_field(&mut hash, &format!("{field:?}"));
        }
        let mut custom = definition
            .schema
            .custom_bindings()
            .iter()
            .collect::<Vec<_>>();
        custom.sort_by(|left, right| left.0.cmp(right.0));
        for (key, binding) in custom {
            hash_field(&mut hash, key);
            hash_field(&mut hash, &format!("{:?}", binding));
        }
        let mut relations = definition.schema.relations().iter().collect::<Vec<_>>();
        relations.sort_by(|left, right| left.0.cmp(right.0));
        for (path, relation) in relations {
            hash_field(&mut hash, path);
            hash_field(&mut hash, &format!("{relation:?}"));
        }
        let mut sorts = definition.sort_fields.iter().collect::<Vec<_>>();
        sorts.sort_by_key(|path| path.display());
        for sort in sorts {
            hash_field(&mut hash, &sort.display());
        }
        hash_field(&mut hash, &format!("{:?}", state.raw.alphabet));
    }
    hash.finalize().into()
}

fn repository_identity(repository: &Repository) -> Result<[u8; 32], String> {
    let mut hash = Sha256::new();
    hash.update(b"objekte-icss-repository-v1");
    for file in &repository.files().files {
        hash_serde(&mut hash, file)?;
    }
    for state in repository.collections() {
        hash_field(&mut hash, &state.definition.name);
        hash_field(&mut hash, &state.generation.generation);
        for record in &state.generation.records {
            hash_field(&mut hash, &record.slot.0);
            hash_field(&mut hash, &record.id.0.to_string());
            hash_field(&mut hash, &record.file.0);
            hash_field(&mut hash, &record.range.start);
            hash_field(&mut hash, &record.range.end);
            hash_field(&mut hash, &format!("{:?}", record.class));
        }
    }
    Ok(hash.finalize().into())
}

fn slot_namespace(
    repository: &Repository,
    configuration_identity: [u8; 32],
    repository_identity: [u8; 32],
) -> String {
    let mut hash = Sha256::new();
    hash.update(b"objekte-icss-slot-namespace-v1");
    hash.update(configuration_identity);
    hash.update(repository_identity);
    if let Some(root) = repository.files().root.to_str() {
        hash_field(&mut hash, &root);
    }
    codec::hex_digest(&hash.finalize().into())
}

fn hash_field<T: std::fmt::Debug>(hash: &mut Sha256, value: &T) {
    let text = format!("{value:?}");
    hash.update((text.len() as u64).to_le_bytes());
    hash.update(text.as_bytes());
}

fn hash_serde<T: Serialize>(hash: &mut Sha256, value: &T) -> Result<(), String> {
    let bytes = bincode::serialize(value).map_err(|error| error.to_string())?;
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
    Ok(())
}
