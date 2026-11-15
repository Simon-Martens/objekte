//! Metadata-first source discovery into the authoritative redb catalog.
//!
//! This is the first Phase-6 construction stage. It enumerates selected source
//! files, assigns global file IDs, writes file/path/build-state/pending rows
//! and marks the catalog complete. No content is parsed and no native
//! `FileInventory` is retained.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::path::{Path, PathBuf};

use roaring::RoaringBitmap;

use crate::discovery::discover;
use crate::inventory::{DiagnosticRef, FileEntry, FileStatus, SharedPath, SourceRevision};
use crate::model::{CollectionId, FileId};
use crate::repository::CollectionDefinition;
use crate::source::SourceRegistry;

use super::value_codec::{
    BuildFileState, CollectionMetadata, ConstructionState, FileBuildState, FileMetadata,
};
use super::value_codec::BindingIdentity;
use super::{IndexStore, read, write};
use crate::model::RecordSlot;
use crate::posting::ScalarKey;

#[path = "store_build_inference.rs"]
mod inference;
#[path = "store_build_prepare.rs"]
pub(crate) mod prepare;

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

    fn test_root(label: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "objekte-store-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(directory.join("data")).expect("directory");
        directory
    }

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

    #[test]
    fn source_build_counts_exactly_once_and_validates_all_families() {
        let root = test_root("source-build-count");
        std::fs::write(
            root.join("data/records.jsonl"),
            b"{\"title\":\"alpha\"}\n{\"title\":\"beta\"}\n",
        )
        .expect("source");
        let definition = CollectionDefinition::folder("records", "data", SourceFormat::JsonLines);
        let store = Arc::new(IndexStore::new(super::super::StoreConfig::default()).expect("store"));
        let report = build_from_source(
            &store,
            &root,
            &[definition],
            &SourceBuildOptions {
                build_workers: std::num::NonZeroUsize::new(1),
                ..SourceBuildOptions::default()
            },
            &SourceRegistry::builtin(),
        )
        .expect("build");
        assert_eq!(report.records, 2);
        let view = store.view().expect("view");
        let collection = read::collection(&view, CollectionId(0)).expect("collection");
        assert_eq!(collection.live_records, 2);
        assert_eq!(collection.pending_files, 0);
        assert_eq!(collection.indexed_files, 1);
        assert_eq!(collection.processed_files, 1);
        let validation = super::super::validation::validate(&view).expect("validation");
        assert!(validation.is_valid(), "{:?}", validation.errors);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn stale_catalog_version_rejects_prepared_file_before_slot_allocation() {
        let root = test_root("stale-admission");
        std::fs::write(root.join("data/records.jsonl"), b"{\"title\":\"alpha\"}\n")
            .expect("source");
        let definition = CollectionDefinition::folder("records", "data", SourceFormat::JsonLines);
        let store = Arc::new(IndexStore::new(super::super::StoreConfig::default()).expect("store"));
        let registry = SourceRegistry::builtin();
        discover_into_store(&store, &root, &[definition], &registry).expect("discovery");
        let metadata = store.metadata().expect("metadata");
        let view = store.view().expect("view");
        let file = read::file(&view, FileId(0)).expect("file").expect("file");
        drop(view);
        let batch_file = crate::build::BatchFile {
            entry: file.entry,
            records: 0..0,
            first_range: true,
        };
        let tx = store.begin_write().expect("write");
        let mut stale = write::get_build_state(&tx, FileId(0))
            .expect("state")
            .expect("state");
        stale.expected_file_version += 1;
        write::put_build_state(&tx, FileId(0), &stale).expect("stale state");
        let error = validate_admission_file(
            &tx,
            CollectionId(0),
            metadata.build_epoch,
            &batch_file,
            &[FileId(0)].into_iter().collect(),
        )
        .expect_err("stale version must be rejected");
        assert!(error.contains("does not match catalog expectation"), "{error}");
        drop(tx);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn overlapping_collections_admit_one_physical_file_atomically() {
        let root = test_root("overlap");
        std::fs::write(root.join("data/shared.jsonl"), b"{\"title\":\"shared\"}\n")
            .expect("source");
        let definitions = vec![
            CollectionDefinition::folder("left", "data", SourceFormat::JsonLines),
            CollectionDefinition::folder("right", "data", SourceFormat::JsonLines),
        ];
        let store = Arc::new(IndexStore::new(super::super::StoreConfig::default()).expect("store"));
        let report = build_from_source(
            &store,
            &root,
            &definitions,
            &SourceBuildOptions {
                build_workers: std::num::NonZeroUsize::new(1),
                ..SourceBuildOptions::default()
            },
            &SourceRegistry::builtin(),
        )
        .expect("build");
        assert_eq!(report.records, 2);
        let view = store.view().expect("view");
        for collection in 0..2 {
            let collection = read::collection(&view, CollectionId(collection)).expect("collection");
            assert_eq!(collection.live_records, 1);
            assert_eq!(collection.indexed_files, 1);
            assert_eq!(read::pending_files(&view, collection.id).expect("pending").len(), 0);
        }
        let validation = super::super::validation::validate(&view).expect("validation");
        assert!(validation.is_valid(), "{:?}", validation.errors);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn automatic_inference_publishes_typed_store_facts_and_reverse_contributions() {
        let root = test_root("inference");
        let records = (0..24)
            .map(|index| {
                format!(
                    "{{\"status\":\"{}\"}}\n",
                    if index % 2 == 0 { "open" } else { "closed" }
                )
            })
            .collect::<String>();
        std::fs::write(root.join("data/records.jsonl"), records).expect("source");
        let definition = CollectionDefinition::folder("records", "data", SourceFormat::JsonLines);
        let store = Arc::new(IndexStore::new(super::super::StoreConfig::default()).expect("store"));
        let report = build_from_source(
            &store,
            &root,
            &[definition],
            &SourceBuildOptions {
                build_workers: std::num::NonZeroUsize::new(1),
                inference: crate::inference::InferenceConfig {
                    mode: crate::inference::InferenceMode::AutoIndex,
                    ..crate::inference::InferenceConfig::default()
                },
                ..SourceBuildOptions::default()
            },
            &SourceRegistry::builtin(),
        )
        .expect("build");
        assert_eq!(report.records, 24);
        let view = store.view().expect("view");
        let field = read::field(&view, CollectionId(0), &["status".into()])
            .expect("field")
            .expect("inferred field");
        assert!(field.values_indexed);
        assert_eq!(field.covered_records, 24);
        let binding = read::all_bindings(&view, CollectionId(0))
            .expect("bindings")
            .into_iter()
            .find(|binding| {
                binding.descriptor.identity
                    == BindingIdentity::Declared {
                        path: vec!["status".into()],
                    }
            })
            .expect("inferred binding");
        assert_eq!(
            read::value_members(
                &view,
                CollectionId(0),
                binding.id,
                &ScalarKey::String("open".into())
            )
            .expect("members")
            .len(),
            12
        );
        let validation = super::super::validation::validate(&view).expect("validation");
        assert!(validation.is_valid(), "{:?}", validation.errors);
        std::fs::remove_dir_all(root).expect("cleanup");
    }
}

/// Options for a direct source build into redb.
#[derive(Clone, Debug)]
pub struct SourceBuildOptions {
    pub build_workers: Option<std::num::NonZeroUsize>,
    pub alphabet: crate::workload::BigramAlphabet,
    pub inference: crate::inference::InferenceConfig,
    pub profile_build: bool,
}

impl Default for SourceBuildOptions {
    fn default() -> Self {
        Self {
            build_workers: None,
            alphabet: crate::workload::BigramAlphabet::Ascii,
            inference: crate::inference::InferenceConfig::default(),
            profile_build: false,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SourceAdmissionStats {
    pub files: u64,
    pub records: u64,
    pub posting_blocks_changed: u64,
    pub state_blocks_changed: u64,
    pub fts_blocks_changed: u64,
    pub reverse_gram_rows: u64,
    pub sort_entries: u64,
    pub relation_occurrences: u64,
    pub reverse_contributions: u64,
    pub record_parses: u64,
    pub failed_parses: u64,
    pub source_reads: u64,
    pub source_bytes_read: u64,
}

#[derive(Clone, Debug, Default)]
pub struct SourceBuildReport {
    pub discovery: DiscoveryReport,
    pub records: u64,
    pub batches: u64,
    pub resolved_workers: usize,
    pub discovery_ns: u128,
    pub scheduling_job_ns: u64,
    pub receive_wait_ns: u64,
    pub admission_ns: u64,
    pub finalization_ns: u128,
    pub total_ns: u128,
    pub out_of_order_batches: u64,
    pub peak_active_jobs: usize,
    pub peak_ready_batches: usize,
    pub peak_weight_bytes: u64,
    pub work_profile: crate::build::BuildWorkProfile,
    pub admission_stats: SourceAdmissionStats,
    /// Historical field retained for evidence consumers. It now includes the
    /// final completion transaction and buffer release.
    pub elapsed_ns: u128,
}

struct SourceBuildFailureGuard<'a> {
    store: &'a Arc<IndexStore>,
    completed: bool,
}

impl Drop for SourceBuildFailureGuard<'_> {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        let Ok(tx) = self.store.begin_write() else {
            return;
        };
        let Ok(mut metadata) = read::metadata_from_write(&tx) else {
            return;
        };
        metadata.construction = ConstructionState::Failed;
        metadata.revision = metadata.revision.saturating_add(1);
        if write::put_metadata(&tx, &metadata).is_ok() {
            let _ = tx.commit();
        }
    }
}

fn finalize_coverage_metrics(
    store: &Arc<IndexStore>,
    collections: &mut [BuildCollection],
) -> Result<(), String> {
    for collection in collections {
        if collection.coverage.is_empty() {
            continue;
        }
        let view = store.view().map_err(|error| error.to_string())?;
        let mut rows = Vec::with_capacity(collection.coverage.len());
        for (binding, coverage) in &collection.coverage {
            let mut value_cardinality = 0u64;
            read::for_each_value_directory(
                &view,
                collection.id,
                *binding,
                |_, _| {
                    value_cardinality = value_cardinality
                        .checked_add(1)
                        .ok_or_else(|| {
                            crate::index_store::StoreError::CorruptIndex(
                                "value directory count overflow".into(),
                            )
                        })?;
                    Ok(true)
                },
            )
            .map_err(|error| error.to_string())?;
            let length_cardinality =
                read::length_directory_count(&view, collection.id, *binding)
                    .map_err(|error| error.to_string())?;
            let mut finalized = coverage.clone();
            finalized.value_cardinality = value_cardinality;
            finalized.length_cardinality = length_cardinality;
            rows.push((*binding, finalized));
        }
        drop(view);
        for (binding, coverage) in &rows {
            collection.coverage.insert(*binding, coverage.clone());
        }
        let tx = store.begin_write().map_err(|error| error.to_string())?;
        write::put_field_coverages_batch(&tx, collection.id, &rows)
            .map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;
    }
    Ok(())
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
    let discovery_started = std::time::Instant::now();
    let discovery = discover_into_store(store, root, definitions, sources)?;
    let mut failure_guard = SourceBuildFailureGuard {
        store,
        completed: false,
    };
    let discovery_ns = discovery_started.elapsed().as_nanos();
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
            while end < pending.len() && end - start < 256 {
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
        metadata,
        records_written: 0,
        collections: definitions
            .iter()
            .enumerate()
            .map(|(index, definition)| BuildCollection {
                id: CollectionId(index as u32),
                definition: definition.clone(),
                bindings: HashMap::new(),
                fields: HashMap::new(),
                coverage: HashMap::new(),
                next_binding: 0,
                sort_ids: HashMap::new(),
                next_sort: 0,
                relations: HashMap::new(),
                next_relation: 0,
                inference: crate::inference::InferenceBuilder::for_collection(
                    options.inference.clone(),
                    definitions.len(),
                ),
                logical_records: RoaringBitmap::new(),
                record_count: 0,
            })
            .collect(),
        collection_ids,
        work_profile: crate::build::BuildWorkProfile::default(),
        admission_stats: SourceAdmissionStats::default(),
    };
    let mut report = SourceBuildReport {
        discovery,
        resolved_workers: workers,
        discovery_ns,
        ..SourceBuildReport::default()
    };
    let scheduler = crate::build::process_repository_file_batches_prepared_profiled(
        &pool,
        Arc::new(root),
        &jobs,
        Arc::new(source_files),
        options.alphabet,
        options.inference.clone(),
        options.profile_build,
        Some(32 * 1024 * 1024),
        crate::build::process_store_batch_profiled,
        |collection_index, batch| {
            let admission_started = std::time::Instant::now();
            builder.admit_prepared(collection_index, batch)?;
            report.batches += 1;
            report.admission_ns = report
                .admission_ns
                .saturating_add(admission_started.elapsed().as_nanos() as u64);
            Ok(())
        },
    )?;
    report.records = builder.records_written;
    report.scheduling_job_ns = scheduler.job_execution_ns;
    report.receive_wait_ns = scheduler.receive_wait_ns;
    report.out_of_order_batches = scheduler.out_of_order_completed;
    report.peak_active_jobs = scheduler.peak_active_jobs;
    report.peak_ready_batches = scheduler.peak_ready_batches;
    report.peak_weight_bytes = scheduler.peak_weight_bytes;

    let finalization_started = std::time::Instant::now();
    inference::finalize(store, &mut builder.collections)?;
    finalize_coverage_metrics(store, &mut builder.collections)?;
    let tx = store.begin_write().map_err(|error| error.to_string())?;
    let mut metadata = read::metadata_from_write(&tx).map_err(|error| error.to_string())?;
    metadata.construction = ConstructionState::Complete;
    metadata.catalog_complete = true;
    metadata.revision = metadata
        .revision
        .checked_add(1)
        .ok_or_else(|| "publication revision overflow".to_string())?;
    write::put_metadata(&tx, &metadata).map_err(|error| error.to_string())?;
    tx.commit().map_err(|error| error.to_string())?;
    failure_guard.completed = true;
    report.finalization_ns = finalization_started.elapsed().as_nanos();
    report.work_profile = builder.work_profile.clone();
    report.admission_stats = builder.admission_stats.clone();
    drop(builder);
    drop(pool);
    report.total_ns = started.elapsed().as_nanos();
    report.elapsed_ns = report.total_ns;
    Ok(report)
}

struct BuildCollection {
    id: CollectionId,
    definition: CollectionDefinition,
    bindings: HashMap<BindingIdentity, u32>,
    fields: HashMap<Vec<String>, crate::index_store::value_codec::FieldDescriptor>,
    coverage: HashMap<u32, crate::index_store::value_codec::FieldCoverage>,
    next_binding: u32,
    sort_ids: HashMap<crate::model::FieldPath, (u32, u32)>,
    next_sort: u32,
    relations: HashMap<Vec<String>, u32>,
    next_relation: u32,
    inference: Option<crate::inference::InferenceBuilder>,
    logical_records: RoaringBitmap,
    record_count: u64,
}

fn validate_admission_file(
    tx: &redb::WriteTransaction,
    collection: CollectionId,
    expected_epoch: u64,
    batch_file: &crate::build::BatchFile,
    pending: &std::collections::HashSet<FileId>,
) -> Result<FileMetadata, String> {
    let current = write::get_file(tx, batch_file.entry.id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("file {} was removed before admission", batch_file.entry.id.0))?;
    let state = write::get_build_state(tx, batch_file.entry.id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("file {} has no build state before admission", batch_file.entry.id.0))?;
    if state.build_epoch != expected_epoch {
        return Err(format!(
            "stale build epoch for file {}: prepared {}, current {}",
            batch_file.entry.id.0, expected_epoch, state.build_epoch
        ));
    }
    if current.file_version != state.expected_file_version {
        return Err(format!(
            "file {} version {} does not match catalog expectation {}",
            batch_file.entry.id.0, current.file_version, state.expected_file_version
        ));
    }
    if current.entry.revision != batch_file.entry.revision {
        return Err(format!(
            "file {} source revision changed before admission",
            batch_file.entry.id.0
        ));
    }
    if !current.entry.collections.contains(&collection) {
        return Err(format!(
            "file {} no longer belongs to collection {}",
            batch_file.entry.id.0, collection.0
        ));
    }
    if !pending.contains(&batch_file.entry.id) {
        return Err(format!(
            "file {} is no longer pending in collection {}",
            batch_file.entry.id.0, collection.0
        ));
    }
    Ok(current)
}

fn merge_file_admission(
    collection: CollectionId,
    current: &FileMetadata,
    batch_file: &crate::build::BatchFile,
    local_to_global: &[RecordSlot],
) -> Result<(FileEntry, Vec<DiagnosticRef>), String> {
    let mut merged = current.entry.clone();
    for other in &batch_file.entry.collections {
        if !merged.collections.contains(other) {
            merged.collections.push(*other);
        }
    }
    if !merged.collections.contains(&collection) {
        merged.collections.push(collection);
    }
    merged.collections.sort_unstable_by_key(|collection| collection.0);
    merged.collections.dedup();
    merged.status = merged.status.max(batch_file.entry.status);
    merged.omitted_diagnostics = merged
        .omitted_diagnostics
        .checked_add(batch_file.entry.omitted_diagnostics)
        .ok_or_else(|| "file diagnostic omission count overflow".to_string())?;
    let mut remapped_refs = Vec::with_capacity(batch_file.entry.diagnostics.len());
    for mut diagnostic in batch_file.entry.diagnostics.clone() {
        if let Some(local) = diagnostic.record {
            let global = local_to_global
                .get(local.0 as usize)
                .copied()
                .ok_or_else(|| "diagnostic record slot is outside the prepared batch".to_string())?;
            diagnostic.record = Some(global);
        }
        remapped_refs.push(
            merged
                .record_diagnostic(diagnostic)
                .map_err(|error| format!("file {}: {error}", merged.id.0))?,
        );
    }
    Ok((merged, remapped_refs))
}

fn remap_diagnostic_reference(
    reference: DiagnosticRef,
    remapped: &[DiagnosticRef],
) -> DiagnosticRef {
    reference
        .stored_index()
        .and_then(|index| remapped.get(index).copied())
        .unwrap_or(reference)
}

struct AdmittedFile {
    entry: FileEntry,
    version: u64,
    diagnostic_refs: Vec<DiagnosticRef>,
}

fn sort_ids_for(
    tx: &redb::WriteTransaction,
    collection: &mut BuildCollection,
    path: &crate::model::FieldPath,
) -> Result<(u32, u32), String> {
    if let Some(ids) = collection.sort_ids.get(path) {
        return Ok(*ids);
    }
    let ids = (collection.next_sort, collection.next_sort + 1);
    collection.next_sort = collection
        .next_sort
        .checked_add(2)
        .ok_or_else(|| "sort definition overflow".to_string())?;
    let options = collection
        .definition
        .sort_overrides
        .get(path)
        .map_or(collection.definition.sort_options, |config| config.options);
    let locale = collection
        .definition
        .sort_overrides
        .get(path)
        .map_or_else(
            || collection.definition.sort_locale.clone(),
            |config| config.locale.clone(),
        );
    for (id, direction) in [
        (
            ids.0,
            crate::index_store::value_codec::SortDirection::Ascending,
        ),
        (
            ids.1,
            crate::index_store::value_codec::SortDirection::Descending,
        ),
    ] {
        write::put_sort_definition(
            tx,
            collection.id,
            &crate::index_store::value_codec::SortDefinition {
                id,
                constituents: vec![crate::index_store::value_codec::SortConstituent {
                    key: path.clone(),
                    direction,
                    options,
                }],
                locale: locale.clone(),
                collation_version: crate::index_store::tables::COLLATION_VERSION.into(),
            },
        )
        .map_err(|error| error.to_string())?;
    }
    collection.sort_ids.insert(path.clone(), ids);
    Ok(ids)
}

fn relation_ordinal_for(
    collection: &mut BuildCollection,
    path: &[String],
) -> Result<u32, String> {
    if let Some(ordinal) = collection.relations.get(path) {
        return Ok(*ordinal);
    }
    let ordinal = collection.next_relation;
    collection.next_relation = collection
        .next_relation
        .checked_add(1)
        .ok_or_else(|| "relation ordinal overflow".to_string())?;
    collection.relations.insert(path.to_vec(), ordinal);
    Ok(ordinal)
}

struct BatchStoreBuilder<'a> {
    store: &'a Arc<IndexStore>,
    metadata: crate::index_store::value_codec::StoreMetadata,
    collections: Vec<BuildCollection>,
    collection_ids: HashMap<String, CollectionId>,
    records_written: u64,
    work_profile: crate::build::BuildWorkProfile,
    admission_stats: SourceAdmissionStats,
}

impl BatchStoreBuilder<'_> {
    fn admit_prepared(
        &mut self,
        collection_index: usize,
        batch: prepare::PreparedStoreBatch,
    ) -> Result<(), String> {
        self.work_profile.absorb(&batch.profile);
        self.admission_stats.files = self
            .admission_stats
            .files
            .checked_add(batch.files.len() as u64)
            .ok_or_else(|| "admitted file count overflow".to_string())?;
        self.admission_stats.records = self
            .admission_stats
            .records
            .checked_add(batch.records.len() as u64)
            .ok_or_else(|| "admitted record count overflow".to_string())?;
        self.admission_stats.record_parses = self
            .admission_stats
            .record_parses
            .checked_add(batch.record_parses)
            .ok_or_else(|| "record parse count overflow".to_string())?;
        self.admission_stats.failed_parses = self
            .admission_stats
            .failed_parses
            .checked_add(batch.failed_parses)
            .ok_or_else(|| "failed parse count overflow".to_string())?;
        self.admission_stats.source_reads = self
            .admission_stats
            .source_reads
            .checked_add(batch.source_reads)
            .ok_or_else(|| "source read count overflow".to_string())?;
        self.admission_stats.source_bytes_read = self
            .admission_stats
            .source_bytes_read
            .checked_add(batch.source_bytes_read)
            .ok_or_else(|| "source byte count overflow".to_string())?;
        let collection = &mut self.collections[collection_index];
        let tx = self.store.begin_write().map_err(|error| error.to_string())?;
        let store_metadata = read::metadata_from_write(&tx).map_err(|error| error.to_string())?;
        if store_metadata.build_epoch != self.metadata.build_epoch {
            return Err(format!(
                "stale prepared batch epoch {}, current {}",
                self.metadata.build_epoch, store_metadata.build_epoch
            ));
        }
        let mut collection_metadata =
            read::collection_from_write(&tx, collection.id).map_err(|error| error.to_string())?;
        let count = batch.records.len();
        let pending = read::pending_files_for_write(&tx, collection.id)
            .map_err(|error| error.to_string())?
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
        let mut current_files = BTreeMap::new();
        for file in &batch.files {
            current_files.insert(
                file.entry.id,
                validate_admission_file(&tx, collection.id, self.metadata.build_epoch, file, &pending)?,
            );
        }
        let allocated =
            super::slot_allocator::allocate_many(&tx, &mut collection_metadata, count)
                .map_err(|error| error.to_string())?;
        let local_to_global = allocated
            .iter()
            .map(|(slot, _)| *slot)
            .collect::<Vec<_>>();
        let incarnations = allocated
            .iter()
            .map(|(_, incarnation)| *incarnation)
            .collect::<Vec<_>>();
        if let Some(local) = batch.inference {
            let base = local_to_global
                .first()
                .copied()
                .unwrap_or(RecordSlot(collection_metadata.slot_high_water as u32))
                .0;
            if local_to_global
                .iter()
                .enumerate()
                .any(|(offset, slot)| slot.0 != base.saturating_add(offset as u32))
            {
                return Err("inference merge requires contiguous allocated slots".into());
            }
            collection
                .inference
                .as_mut()
                .ok_or_else(|| "prepared inference output without enabled inference".to_string())?
                .merge_shifted(local, base)
                .map_err(|error| error.to_string())?;
        }
        for local in batch.logical_records.iter() {
            let global = local_to_global
                .get(local as usize)
                .ok_or_else(|| "logical record slot is outside the prepared batch".to_string())?;
            collection.logical_records.insert(global.0);
        }

        let mut binding_ids = Vec::with_capacity(batch.bindings.len());
        for prepared in &batch.bindings {
            let identity = prepared.descriptor.identity.clone();
            let binding = if let Some(binding) = collection.bindings.get(&identity) {
                *binding
            } else {
                let binding = collection.next_binding;
                collection.next_binding = collection
                    .next_binding
                    .checked_add(1)
                    .ok_or_else(|| "binding overflow".to_string())?;
                let encoded =
                    bincode::serialize(&identity).map_err(|error| error.to_string())?;
                write::put_binding(
                    &tx,
                    collection.id,
                    &encoded,
                    &crate::index_store::value_codec::StoredBinding {
                        id: binding,
                        descriptor: prepared.descriptor.clone(),
                    },
                )
                .map_err(|error| error.to_string())?;
                collection.bindings.insert(identity, binding);
                binding
            };
            binding_ids.push(binding);
        }

        let mut field_rows = BTreeMap::new();
        for (path, posting) in &batch.fields {
            let existing = collection.fields.get(path).cloned();
            if existing
                .as_ref()
                .is_some_and(|field| field.kind != posting.kind || field.multi != posting.multi)
            {
                return Err(format!(
                    "field {} changed type or multiplicity across batches",
                    path.join(".")
                ));
            }
            let covered_records = existing
                .as_ref()
                .map_or(0, |field| u64::from(field.covered_records))
                .checked_add(u64::from(posting.covered_records))
                .and_then(|count| u32::try_from(count).ok())
                .ok_or_else(|| format!("field {} covered-record count overflow", path.join(".")))?;
            let descriptor = crate::index_store::value_codec::FieldDescriptor {
                covered_records,
                indexed: existing.as_ref().is_some_and(|field| field.indexed)
                    || posting.indexed,
                values_indexed: existing
                    .as_ref()
                    .is_none_or(|field| field.values_indexed)
                    && posting.values_indexed,
                value_complete: existing.as_ref().is_none_or(|field| field.value_complete)
                    && posting.value_complete,
                length_complete: existing.as_ref().is_none_or(|field| field.length_complete)
                    && posting.length_complete,
                empty_string_complete: existing
                    .as_ref()
                    .is_none_or(|field| field.empty_string_complete)
                    && posting.empty_string_complete,
                ..posting.clone()
            };
            collection.fields.insert(path.clone(), descriptor.clone());
            field_rows.insert(path.clone(), descriptor);
        }
        write::put_fields_batch(
            &tx,
            collection.id,
            &field_rows.into_iter().collect::<Vec<_>>(),
        )
        .map_err(|error| error.to_string())?;
        let mut coverage_rows = Vec::with_capacity(batch.coverage.len());
        for (local_binding, mut coverage) in batch.coverage {
            let binding = *binding_ids
                .get(local_binding as usize)
                .ok_or_else(|| "prepared coverage binding is out of range".to_string())?;
            if let Some(existing) = collection.coverage.get(&binding) {
                if existing.kind != coverage.kind || existing.multi != coverage.multi {
                    return Err("field coverage changed type across batches".into());
                }
                coverage.covered_records = existing
                    .covered_records
                    .checked_add(coverage.covered_records)
                    .ok_or_else(|| "field coverage count overflow".to_string())?;
                coverage.values_indexed &= existing.values_indexed;
                coverage.value_complete &= existing.value_complete;
                coverage.length_complete &= existing.length_complete;
                coverage.empty_string_complete &= existing.empty_string_complete;
                coverage.value_cardinality = coverage.value_cardinality.max(existing.value_cardinality);
                coverage.length_cardinality = coverage.length_cardinality.max(existing.length_cardinality);
            }
            collection.coverage.insert(binding, coverage.clone());
            coverage_rows.push((binding, coverage));
        }
        write::put_field_coverages_batch(&tx, collection.id, &coverage_rows)
            .map_err(|error| error.to_string())?;

        let fingerprints = batch
            .file_fingerprints
            .iter()
            .copied()
            .collect::<HashMap<_, _>>();
        let mut file_rows = Vec::with_capacity(batch.files.len());
        let mut admitted_files = BTreeMap::new();
        for file in &batch.files {
            let current = current_files
                .get(&file.entry.id)
                .ok_or_else(|| "admission file validation disappeared".to_string())?;
            let fingerprint = fingerprints
                .get(&file.entry.id)
                .copied()
                .or(current.fingerprint);
            let (entry, diagnostic_refs) =
                merge_file_admission(collection.id, current, file, &local_to_global)?;
            file_rows.push(FileMetadata {
                entry: entry.clone(),
                file_version: current.file_version,
                fingerprint,
            });
            admitted_files.insert(
                entry.id,
                AdmittedFile {
                    entry,
                    version: current.file_version,
                    diagnostic_refs,
                },
            );
        }
        write::put_files_without_paths_batch(&tx, &file_rows)
            .map_err(|error| error.to_string())?;

        let mut record_rows = Vec::with_capacity(count);
        let mut logical_rows = Vec::with_capacity(count);
        let mut file_record_rows = Vec::with_capacity(count);
        let mut contribution_rows = Vec::with_capacity(count);
        let mut posting_batch = super::posting_blocks::PostingBatch::new();
        let mut state_batch = super::states::StateBatch::new();
        let mut fts_batch = super::fts::FtsBatch::new();
        let mut reverse_gram_rows = BTreeMap::new();
        let mut sort_rows = Vec::with_capacity(count);
        let mut relation_rows = Vec::with_capacity(count);
        let mut relation_state_rows = Vec::new();
        let mut domain_rows = Vec::new();

        let mut value_at = 0usize;
        let mut length_at = 0usize;
        let mut state_at = 0usize;
        let mut sort_at = 0usize;
        let mut relation_at = 0usize;
        let mut relation_state_at = 0usize;
        let mut gram_at = 0usize;
        for (local, record_entry) in batch.records.into_iter().enumerate() {
            let slot = local_to_global[local];
            let incarnation = incarnations[local];
            let admitted = admitted_files.get(&record_entry.file).ok_or_else(|| {
                format!(
                    "record file {} is not in the prepared batch",
                    record_entry.file.0
                )
            })?;
            let mut record_entry = record_entry;
            record_entry.diagnostic_ref = remap_diagnostic_reference(
                record_entry.diagnostic_ref,
                &admitted.diagnostic_refs,
            );
            let record =
                crate::index_store::value_codec::RecordMetadata::from_entry(&record_entry, incarnation);
            record_rows.push((slot, record.clone()));
            logical_rows.push((record_entry.id.clone(), slot, incarnation));
            file_record_rows.push((record_entry.file, collection.id, slot));
            if batch.logical_records.contains(local as u32) {
                domain_rows.push((
                    collection.id,
                    crate::index_store::value_codec::DomainKind::Logical,
                    slot,
                ));
            }
            if batch.empty_array_records.contains(local as u32) {
                domain_rows.push((
                    collection.id,
                    crate::index_store::value_codec::DomainKind::EmptyArray,
                    slot,
                ));
            }

            let mut contribution =
                crate::index_store::value_codec::StoredContributions {
                    record,
                    values: Vec::new(),
                    lengths: Vec::new(),
                    state_masks: Vec::new(),
                    coverage: Vec::new(),
                    domains: batch
                        .logical_records
                        .contains(local as u32)
                        .then_some(crate::index_store::value_codec::DomainKind::Logical)
                        .into_iter()
                        .chain(
                            batch
                                .empty_array_records
                                .contains(local as u32)
                                .then_some(crate::index_store::value_codec::DomainKind::EmptyArray),
                        )
                        .collect(),
                    sorts: Vec::new(),
                    relations: Vec::new(),
                    grams: Vec::new(),
                };
            while value_at < batch.values.len()
                && batch.values[value_at].slot == local as u32
            {
                let fact = &batch.values[value_at];
                let binding = *binding_ids
                    .get(fact.binding as usize)
                    .ok_or_else(|| "prepared value binding is out of range".to_string())?;
                posting_batch.set(
                    super::posting_blocks::PostingFamily::Value,
                    binding,
                    &fact.value,
                    slot,
                );
                contribution.values.push((binding, fact.value.clone()));
                value_at += 1;
            }
            while length_at < batch.lengths.len()
                && batch.lengths[length_at].slot == local as u32
            {
                let fact = &batch.lengths[length_at];
                let binding = *binding_ids
                    .get(fact.binding as usize)
                    .ok_or_else(|| "prepared length binding is out of range".to_string())?;
                let scalar = ScalarKey::Number(fact.length.clone());
                posting_batch.set(
                    super::posting_blocks::PostingFamily::Length,
                    binding,
                    &scalar,
                    slot,
                );
                contribution.lengths.push((binding, fact.length.clone()));
                length_at += 1;
            }
            let mut state_masks = BTreeMap::<u32, u16>::new();
            while state_at < batch.states.len()
                && batch.states[state_at].slot == local as u32
            {
                let fact = &batch.states[state_at];
                let binding = *binding_ids
                    .get(fact.binding as usize)
                    .ok_or_else(|| "prepared state binding is out of range".to_string())?;
                *state_masks.entry(binding).or_insert(0) |= fact.mask;
                state_at += 1;
            }
            for (binding, mask) in state_masks {
                for tag in 0..=super::tables::MAX_STATE_TAG {
                    if mask & (1 << tag) == 0 {
                        continue;
                    }
                    let state = crate::index_store::value_codec::StateKind::from_tag(tag)
                        .ok_or_else(|| "unknown prepared state tag".to_string())?;
                    state_batch.set(binding, state, slot.0);
                }
                contribution.state_masks.push((binding, mask));
            }
            while sort_at < batch.sorts.len()
                && batch.sorts[sort_at].slot == local as u32
            {
                let fact = &batch.sorts[sort_at];
                let (ascending, descending) =
                    sort_ids_for(&tx, collection, &fact.path)?;
                contribution.sorts.push(crate::index_store::value_codec::SortProjection {
                    definition: ascending,
                    key: vec![fact.key.clone()],
                });
                contribution.sorts.push(crate::index_store::value_codec::SortProjection {
                    definition: descending,
                    key: vec![fact.key.clone()],
                });
                sort_at += 1;
            }
            sort_rows.push((
                slot,
                contribution.record.id.0.clone(),
                contribution.sorts.clone(),
            ));
            while relation_at < batch.relations.len()
                && batch.relations[relation_at].slot == local as u32
            {
                let relation = &batch.relations[relation_at];
                let ordinal = relation_ordinal_for(
                    collection,
                    &relation.path,
                )?;
                let target = self
                    .collection_ids
                    .get(&relation.target_collection)
                    .copied()
                    .ok_or_else(|| {
                        format!(
                            "relation {} targets unknown collection {}",
                            relation.path.join("."),
                            relation.target_collection
                        )
                    })?;
                let occurrence = crate::index_store::value_codec::RelationOccurrence {
                    declaration_ordinal: ordinal,
                    occurrence_ordinal: relation.occurrence_ordinal,
                    target_collection: target,
                    target_id: relation.target_id.clone(),
                    positions: relation.positions.clone(),
                };
                contribution.relations.push(occurrence);
                relation_at += 1;
            }
            relation_rows.push((slot, contribution.relations.clone()));
            while relation_state_at < batch.relation_states.len()
                && batch.relation_states[relation_state_at].slot == local as u32
            {
                let fact = &batch.relation_states[relation_state_at];
                let ordinal = relation_ordinal_for(collection, &fact.path)?;
                relation_state_rows.push((ordinal, fact.state, slot));
                relation_state_at += 1;
            }
            while gram_at < batch.grams.len()
                && batch.grams[gram_at].slot == local as u32
            {
                let fact = &batch.grams[gram_at];
                let binding = *binding_ids
                    .get(fact.binding as usize)
                    .ok_or_else(|| "prepared gram binding is out of range".to_string())?;
                for gram in &fact.grams {
                    fts_batch.set(binding, fact.kind, *gram, slot.0);
                }
                reverse_gram_rows.insert(
                    (slot.0, binding, fact.kind),
                    fact.grams.clone(),
                );
                contribution.grams.push(
                    crate::index_store::value_codec::GramProjection {
                        binding,
                        kind: fact.kind,
                        grams: fact.grams.clone(),
                    },
                );
                gram_at += 1;
            }
            contribution.normalize();
            contribution_rows.push((slot, contribution));
        }
        if value_at != batch.values.len()
            || length_at != batch.lengths.len()
            || state_at != batch.states.len()
            || sort_at != batch.sorts.len()
            || relation_at != batch.relations.len()
            || relation_state_at != batch.relation_states.len()
            || gram_at != batch.grams.len()
        {
            return Err("prepared facts do not cover the complete record batch".into());
        }

        write::put_records_batch(&tx, collection.id, &record_rows)
            .map_err(|error| error.to_string())?;
        write::put_logical_ids_batch(&tx, collection.id, &logical_rows)
            .map_err(|error| error.to_string())?;
        write::put_file_records_batch(&tx, &file_record_rows)
            .map_err(|error| error.to_string())?;
        write::put_contributions_batch(&tx, collection.id, &contribution_rows)
            .map_err(|error| error.to_string())?;
        write::insert_domain_members_batch(&tx, &domain_rows)
            .map_err(|error| error.to_string())?;
        super::posting_blocks::apply_batch(&tx, collection.id, &posting_batch)
            .map_err(|error| error.to_string())?;
        super::states::apply_batch(&tx, collection.id, &state_batch)
            .map_err(|error| error.to_string())?;
        super::fts::apply_batch(&tx, collection.id, &fts_batch)
            .map_err(|error| error.to_string())?;
        let definitions = write::sort_definitions(&tx, collection.id)
            .map_err(|error| error.to_string())?;
        crate::index_store::sorting::insert_sort_projections(
            &tx,
            collection.id,
            sort_rows
                .iter()
                .map(|(slot, id, projections)| (*slot, id.as_ref(), projections.as_slice())),
            &definitions,
        )
        .map_err(|error| error.to_string())?;
        crate::index_store::relations::insert_relation_occurrences_batch(
            &tx,
            collection.id,
            relation_rows
                .iter()
                .map(|(slot, rows)| (*slot, rows.as_slice())),
        )
        .map_err(|error| error.to_string())?;
        write::put_relation_states_batch(&tx, collection.id, &relation_state_rows)
            .map_err(|error| error.to_string())?;
        let reverse_gram_row_count = reverse_gram_rows.len();
        {
            let mut table = tx
                .open_table(super::tables::RECORD_GRAMS)
                .map_err(|error| error.to_string())?;
            for ((slot, binding, kind), grams) in reverse_gram_rows {
                let key =
                    super::keys::record_grams(collection.id.0, slot, binding, kind);
                let encoded = super::value_codec::encode_gram_list(&grams);
                table
                    .insert(key.as_slice(), encoded.as_slice())
                    .map_err(|error| error.to_string())?;
            }
        }
        self.admission_stats.posting_blocks_changed = self
            .admission_stats
            .posting_blocks_changed
            .checked_add(posting_batch.changed_blocks() as u64)
            .ok_or_else(|| "posting block count overflow".to_string())?;
        self.admission_stats.state_blocks_changed = self
            .admission_stats
            .state_blocks_changed
            .checked_add(state_batch.changed_blocks() as u64)
            .ok_or_else(|| "state block count overflow".to_string())?;
        self.admission_stats.fts_blocks_changed = self
            .admission_stats
            .fts_blocks_changed
            .checked_add(fts_batch.changed_blocks() as u64)
            .ok_or_else(|| "FTS block count overflow".to_string())?;
        self.admission_stats.reverse_gram_rows = self
            .admission_stats
            .reverse_gram_rows
            .checked_add(reverse_gram_row_count as u64)
            .ok_or_else(|| "reverse gram row count overflow".to_string())?;
        self.admission_stats.sort_entries = self
            .admission_stats
            .sort_entries
            .checked_add(
                contribution_rows
                    .iter()
                    .map(|(_, contribution)| contribution.sorts.len() as u64)
                    .sum::<u64>(),
            )
            .ok_or_else(|| "sort entry count overflow".to_string())?;
        self.admission_stats.relation_occurrences = self
            .admission_stats
            .relation_occurrences
            .checked_add(
                contribution_rows
                    .iter()
                    .map(|(_, contribution)| contribution.relations.len() as u64)
                    .sum::<u64>(),
            )
            .ok_or_else(|| "relation occurrence count overflow".to_string())?;
        self.admission_stats.reverse_contributions = self
            .admission_stats
            .reverse_contributions
            .checked_add(contribution_rows.len() as u64)
            .ok_or_else(|| "reverse contribution count overflow".to_string())?;

        let mut build_state_rows = Vec::with_capacity(admitted_files.len());
        let mut admitted_file_ids = Vec::with_capacity(admitted_files.len());
        for admitted in admitted_files.values() {
            let state = if admitted.entry.status == FileStatus::Unreadable {
                BuildFileState::Unavailable
            } else {
                BuildFileState::Indexed
            };
            build_state_rows.push((
                admitted.entry.id,
                crate::index_store::value_codec::FileBuildState {
                    state,
                    expected_file_version: admitted.version,
                    build_epoch: self.metadata.build_epoch,
                    last_admission_revision: store_metadata.revision,
                    error: None,
                },
            ));
            admitted_file_ids.push(admitted.entry.id);
            if collection_metadata.pending_files > 0 {
                collection_metadata.pending_files -= 1;
            }
            if state == BuildFileState::Indexed {
                collection_metadata.indexed_files = collection_metadata
                    .indexed_files
                    .checked_add(1)
                    .ok_or_else(|| "indexed file count overflow".to_string())?;
            } else {
                collection_metadata.unavailable_files = collection_metadata
                    .unavailable_files
                    .checked_add(1)
                    .ok_or_else(|| "unavailable file count overflow".to_string())?;
            }
            collection_metadata.processed_files = collection_metadata
                .processed_files
                .checked_add(1)
                .ok_or_else(|| "processed file count overflow".to_string())?;
        }
        write::put_build_states_batch(&tx, &build_state_rows)
            .map_err(|error| error.to_string())?;
        if write::remove_pending_files_batch(&tx, collection.id, &admitted_file_ids)
            .map_err(|error| error.to_string())?
            != admitted_file_ids.len()
        {
            return Err("a prepared file pending membership disappeared during admission".into());
        }
        write::put_collection(&tx, &collection_metadata)
            .map_err(|error| error.to_string())?;
        let mut metadata = read::metadata_from_write(&tx).map_err(|error| error.to_string())?;
        metadata.revision = metadata
            .revision
            .checked_add(1)
            .ok_or_else(|| "publication revision overflow".to_string())?;
        write::put_metadata(&tx, &metadata).map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;
        collection.record_count = collection
            .record_count
            .checked_add(count as u64)
            .ok_or_else(|| "collection record count overflow".to_string())?;
        self.records_written += count as u64;
        Ok(())
    }

}
