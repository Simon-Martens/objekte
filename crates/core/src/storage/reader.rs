use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use bincode::Options;
use serde::Serialize;

use crate::inventory::{ActiveRecords, FileVersions};
use crate::lifecycle::{
    Index, IndexError, IndexRevision, IndexStorageParts, PublishedSegment,
};
use crate::native_segment::IndexedCollectionSegment;
use crate::posting::{CustomPostingIndex, PostingCollection};
use crate::repository::{CollectionState, Repository};
use crate::segment_archive::{FileLookup, RecordLookup, SegmentRows};
use crate::storage::format::{self, StorageError};
use crate::storage::model;
use crate::storage::{
    SharedStorageSession, StorageOpenOptions, StorageSession, StorageStatus,
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AuditOptions {
    pub worker_limit: std::num::NonZeroUsize,
}

impl Default for AuditOptions {
    fn default() -> Self {
        Self {
            worker_limit: std::num::NonZeroUsize::new(1).expect("one is nonzero"),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct AuditReport {
    pub generation: u64,
    pub objects: usize,
    pub bytes: u64,
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct PruneReport {
    pub current_generation: u64,
    pub removed_objects: usize,
    pub removed_bytes: u64,
    pub preserved_objects: usize,
}

fn bincode_options() -> impl bincode::Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .reject_trailing_bytes()
}

pub(crate) fn open_index(
    source_root: PathBuf,
    definitions: Vec<crate::repository::CollectionDefinition>,
    repository_options: crate::repository::RepositoryOptions,
    sources: crate::source::SourceRegistry,
    directory: &Path,
    options: StorageOpenOptions,
) -> Result<Index, IndexError> {
    let (snapshot, session) = open_snapshot(
        &source_root,
        &definitions,
        repository_options,
        &sources,
        directory,
        options,
    )
    .map_err(crate::storage::storage_to_index_error)?;
    let processed_files = snapshot.processed_files;
    let (repository, active, segments, catalog, file_versions) =
        rebuild_repository(snapshot, definitions, repository_options, sources, session)
            .map_err(crate::storage::storage_to_index_error)?;
    let revision = repository.revision;
    let representation_generation = repository.representation_generation;
    let configuration_identity = crate::configuration::configuration_hash(
        &repository
            .collections()
            .iter()
            .map(|collection| collection.definition.clone())
            .collect::<Vec<_>>(),
        repository.options(),
        repository.sources(),
    )
    .map_err(IndexError::IncompatibleConfiguration)?;
    let next_segment_ordinal = segments
        .iter()
        .flat_map(|group| group.iter())
        .map(|segment| segment.id.get())
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    Index::from_storage_parts(IndexStorageParts {
        repository: Arc::new(repository),
        configuration_identity,
        active,
        segments,
        next_segment_ordinal,
        last_size_maintenance_revision: 0,
        catalog,
        file_versions,
        processed_files,
        revision,
        representation_generation,
    })
}

pub(crate) fn open_repository(
    source_root: PathBuf,
    definitions: Vec<crate::repository::CollectionDefinition>,
    repository_options: crate::repository::RepositoryOptions,
    sources: crate::source::SourceRegistry,
    directory: &Path,
    options: StorageOpenOptions,
) -> Result<Repository, IndexError> {
    let (snapshot, session) = open_snapshot(
        &source_root,
        &definitions,
        repository_options,
        &sources,
        directory,
        options,
    )
    .map_err(crate::storage::storage_to_index_error)?;
    let (mut repository, active, segments, catalog, _file_versions) =
        rebuild_repository(snapshot, definitions, repository_options, sources, session)
            .map_err(crate::storage::storage_to_index_error)?;
    repository.active = active;
    repository.segments = segments;
    repository.catalog = catalog;
    Ok(repository)
}

fn open_snapshot(
    source_root: &Path,
    definitions: &[crate::repository::CollectionDefinition],
    repository_options: crate::repository::RepositoryOptions,
    sources: &crate::source::SourceRegistry,
    directory: &Path,
    options: StorageOpenOptions,
) -> Result<(model::StorageSnapshot, SharedStorageSession), StorageError> {
    let limits = options.limits.validate()?;
    let readers_lock = open_shared_lock(&directory.join("readers.lock"))?;
    let current = read_current(directory)?;
    if current.manifest_length > limits.manifest_body_bytes {
        return Err(StorageError::LimitExceeded {
            limit: limits.manifest_body_bytes,
            requested: current.manifest_length,
        });
    }
    let object_path = directory
        .join("objects")
        .join(format::hex(&current.manifest_digest));
    let bytes = fs::read(&object_path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => StorageError::MissingObject(current.manifest_digest),
        _ => StorageError::Io(error.to_string()),
    })?;
    if bytes.len() as u64 != current.manifest_length
        || format::sha256(&bytes) != current.manifest_digest
    {
        return Err(StorageError::corrupt(
            format::hex(&current.manifest_digest),
            "manifest length or digest mismatch",
        ));
    }
    let body = format::checked_body(&bytes, 1)?;
    let snapshot: model::StorageSnapshot = bincode_options()
        .deserialize(body)
        .map_err(|error| StorageError::InvalidFormat(error.to_string()))?;
    model::validate_snapshot(&snapshot).map_err(StorageError::InvalidFormat)?;
    if !snapshot.complete {
        return Err(StorageError::IncompleteBuild);
    }
    let root = source_root
        .canonicalize()
        .map_err(|error| StorageError::Io(format!("source root: {error}")))?;
    if root.to_string_lossy() != snapshot.source_root {
        return Err(StorageError::IncompatibleConfiguration(
            "stored source root differs from the supplied root".into(),
        ));
    }
    let configuration = crate::configuration::configuration_hash(
        definitions,
        repository_options,
        sources,
    )
    .map_err(StorageError::IncompatibleConfiguration)?;
    if configuration != snapshot.configuration_identity {
        return Err(StorageError::IncompatibleConfiguration(
            "collection/schema/source configuration fingerprint differs".into(),
        ));
    }
    let session = Arc::new(StorageSession {
        directory: directory.to_path_buf(),
        generation: current.generation,
        cache: format::StorageCache::new(options.cache_policy),
        _readers_lock: readers_lock,
        store_identity: current.store_identity,
        lineage_identity: current.lineage_identity,
        index_reads: AtomicU64::new(1),
        index_bytes: AtomicU64::new(bytes.len() as u64),
    });
    Ok((snapshot, session))
}

fn rebuild_repository(
    snapshot: model::StorageSnapshot,
    definitions: Vec<crate::repository::CollectionDefinition>,
    repository_options: crate::repository::RepositoryOptions,
    sources: crate::source::SourceRegistry,
    session: SharedStorageSession,
) -> Result<
    (
        Repository,
        Vec<ActiveRecords>,
        Vec<Arc<Vec<Arc<PublishedSegment>>>>,
        crate::lifecycle_catalog::FileCatalog,
        FileVersions,
    ),
    StorageError,
> {
    let root = PathBuf::from(&snapshot.source_root);
    let mut repository = Repository::build_configured_shell(
        root.clone(),
        definitions.clone(),
        repository_options,
        &sources,
    )
    .map_err(StorageError::IncompatibleConfiguration)?;
    let mut inventory = snapshot.files.clone();
    inventory.root = root;
    inventory
        .rebuild_lookup()
        .map_err(StorageError::InvalidFormat)?;
    repository.files = Arc::new(inventory);
    repository.base_fingerprints = Arc::new(snapshot.base_fingerprints.clone());
    repository.catalog = crate::lifecycle_catalog::FileCatalog::restored(
        Arc::new(repository.archive_candidate().map_err(StorageError::InvalidFormat)?),
        snapshot
            .catalog_slots
            .iter()
            .map(|slot| {
                slot.as_ref().map(|slot| {
                    (
                        slot.entry.clone(),
                        slot.fingerprint,
                        slot.configuration,
                    )
                })
            })
            .collect(),
    )
    .map_err(StorageError::InvalidFormat)?;
    if snapshot.collections.len() != definitions.len() {
        return Err(StorageError::IncompatibleConfiguration(
            "stored collection count differs from supplied definitions".into(),
        ));
    }
    let mut collections = Vec::with_capacity(snapshot.collections.len());
    for (definition, stored) in definitions.into_iter().zip(snapshot.collections.iter()) {
        if definition.name != stored.name {
            return Err(StorageError::IncompatibleConfiguration(
                "stored collection names differ from supplied definitions".into(),
            ));
        }
        let source = sources
            .resolve(&definition.format)
            .map_err(StorageError::IncompatibleConfiguration)?;
        collections.push(CollectionState {
            definition: definition.clone(),
            source,
            generation: stored.generation.clone(),
            postings: PostingCollection::from_storage_snapshot(stored.postings.clone())
                .map_err(|message| StorageError::Corrupt {
                    identity: "posting collection".into(),
                    message,
                })?,
            inference: stored
                .inference
                .clone()
                .map(crate::inference::InferenceState::from_storage_snapshot)
                .transpose()
                .map_err(|message| StorageError::Corrupt {
                    identity: "inference state".into(),
                    message,
                })?,
            global_inference: stored.global_inference.clone(),
            custom_postings: stored
                .custom_postings
                .iter()
                .map(|(key, custom)| {
                    Ok((
                        key.clone(),
                        CustomPostingIndex {
                            index: crate::posting::PostingIndex::from_storage_snapshot(
                                custom.index.clone(),
                            )
                            .map_err(StorageError::UnsupportedComponent)?,
                            complete: custom.complete,
                        },
                    ))
                })
                .collect::<Result<_, StorageError>>()?,
            raw: Arc::new(stored.raw.clone()),
            fallback_text: Arc::new(stored.fallback_text.clone()),
            logical_records: stored.logical_records.clone(),
            empty_array_records: stored.empty_array_records.clone(),
            custom_text: Arc::new(stored.custom_text.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
            text_bindings: crate::index_catalog::TextBindingCatalog::new(&definition.schema),
            sorting: Arc::new(stored.sorting.clone()),
            index_family_bytes: 0,
            id_lookup: stored.id_lookup.iter().map(|(id, slot)| (id.clone(), *slot)).collect(),
            identity_order: Arc::new(stored.identity_order.clone()),
            stats: stored.stats,
            relations: Arc::new(stored.relations.clone()),
        });
    }
    repository.collections = collections;
    repository.instance_id = snapshot.instance_id;
    repository.revision = IndexRevision(snapshot.logical_revision);
    repository.representation_generation = snapshot.representation_generation;
    repository.build_parses = snapshot.build_parses;
    repository.build_failed_parses = snapshot.build_failed_parses;
    let base_active = snapshot
        .base_active
        .iter()
        .map(|bitmap| {
            let count = bitmap.len();
            ActiveRecords::from_bitmap(count as usize, bitmap.clone())
        })
        .collect::<Result<Vec<_>, String>>()
        .map_err(|message| StorageError::Corrupt {
            identity: "base liveness".into(),
            message,
        })?;
    let mut groups = Vec::with_capacity(snapshot.segments.len());
    for (collection_number, group) in snapshot.segments.iter().enumerate() {
        let mut stored_group = Vec::with_capacity(group.len());
        for published in group {
            let index = rebuild_segment(&published.segment)?;
            let active = ActiveRecords::from_bitmap(
                index.record_count(),
                published.active.clone(),
            )
            .map_err(|message| StorageError::Corrupt {
                identity: format!("segment {}/{}", collection_number, published.segment.id),
                message,
            })?;
            let segment = PublishedSegment::checked(index.id, Arc::new(index), active)
                .map_err(|message| StorageError::InvalidFormat(message))?;
            stored_group.push(Arc::new(segment));
        }
        groups.push(Arc::new(stored_group));
    }
    repository.active = base_active.clone();
    repository.segments = groups.clone();
    let versions = FileVersions::from_versions(snapshot.file_versions.versions.clone())
        .map_err(StorageError::InvalidFormat)?;
    let routes = crate::lifecycle::initial_ownership_routes(
        &repository,
        &base_active,
        &groups,
        versions.len(),
    );
    let mut catalog = crate::lifecycle_catalog::FileCatalog::restored(
        Arc::new(repository.archive_candidate().map_err(StorageError::InvalidFormat)?),
        snapshot
            .catalog_slots
            .iter()
            .map(|slot| {
                slot.as_ref().map(|slot| {
                    (
                        slot.entry.clone(),
                        slot.fingerprint,
                        slot.configuration,
                    )
                })
            })
            .collect(),
    )
    .map_err(StorageError::InvalidFormat)?;
    catalog
        .replace_all_ownership(&versions, &routes)
        .map_err(StorageError::InvalidFormat)?;
    repository.catalog = catalog.clone();
    repository.storage = Some(session);
    Ok((repository, base_active, groups, catalog, versions))
}

fn rebuild_segment(snapshot: &model::SegmentSnapshot) -> Result<IndexedCollectionSegment, StorageError> {
    let files = snapshot
        .files
        .iter()
        .map(|file| crate::native_segment::SegmentFile {
            entry: file.entry.clone(),
            records: file.records.clone(),
        })
        .collect();
    let (segment_files, records) = SegmentRows::native(files, snapshot.records.clone())
        .map_err(StorageError::InvalidFormat)?
        .views();
    let file_index = FileLookup::native(
        snapshot
            .file_index
            .iter()
            .map(|(id, ordinal)| (*id, *ordinal as usize))
            .collect(),
    )
    .map_err(StorageError::InvalidFormat)?;
    let id_lookup = RecordLookup::native(
        snapshot
            .id_lookup
            .iter()
            .cloned()
            .collect::<HashMap<_, _>>(),
    )
    .map_err(StorageError::InvalidFormat)?;
    let custom_postings = snapshot
        .custom_postings
        .iter()
        .map(|(key, custom)| {
            Ok((
                key.clone(),
                CustomPostingIndex {
                    index: crate::posting::PostingIndex::from_storage_snapshot(
                        custom.index.clone(),
                    )
                    .map_err(StorageError::UnsupportedComponent)?,
                    complete: custom.complete,
                },
            ))
        })
        .collect::<Result<_, StorageError>>()?;
    Ok(IndexedCollectionSegment {
        id: snapshot.id,
        collection: snapshot.collection,
        source_id: snapshot.source_id.clone(),
        source_version: snapshot.source_version,
        files: segment_files,
        file_index,
        records,
        id_lookup,
        identity_order: Arc::new(snapshot.identity_order.clone()),
        postings: PostingCollection::from_storage_snapshot(snapshot.postings.clone())
            .map_err(|message| StorageError::Corrupt {
                identity: format!("segment {} postings", snapshot.id),
                message,
            })?,
        inference_observation: snapshot.inference_observation.clone(),
        inference: snapshot
            .inference
            .clone()
            .map(crate::inference::InferenceState::from_storage_snapshot)
            .transpose()
            .map_err(|message| StorageError::Corrupt {
                identity: format!("segment {} inference", snapshot.id),
                message,
            })?,
        custom_postings,
        raw: Arc::new(snapshot.raw.clone()),
        fallback_text: Arc::new(snapshot.fallback_text.clone()),
        custom_text: Arc::new(
            snapshot
                .custom_text
                .iter()
                .map(|(key, index)| (key.clone(), index.clone()))
                .collect(),
        ),
        logical_records: snapshot.logical_records.clone(),
        empty_array_records: snapshot.empty_array_records.clone(),
        sorting: Arc::new(snapshot.sorting.clone()),
        relations: Arc::new(snapshot.relations.clone()),
        record_parses: snapshot.record_parses,
        failed_parses: snapshot.failed_parses,
        accounting: snapshot.accounting,
        storage_completed: true,
    })
}

fn open_shared_lock(path: &Path) -> Result<File, StorageError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| StorageError::Io(error.to_string()))?;
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| StorageError::Io(error.to_string()))?;
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) };
    if result != 0 {
        return Err(StorageError::StorageBusy(
            "reader lease could not be acquired".into(),
        ));
    }
    Ok(file)
}

#[derive(Clone, Copy)]
struct CurrentToken {
    generation: u64,
    manifest_digest: [u8; 32],
    manifest_length: u64,
    store_identity: [u8; 16],
    lineage_identity: [u8; 16],
}

fn read_current(directory: &Path) -> Result<CurrentToken, StorageError> {
    let bytes = fs::read(directory.join("CURRENT"))
        .map_err(|error| StorageError::Io(format!("CURRENT: {error}")))?;
    if bytes.len() != format::CURRENT_LEN || bytes[..8] != format::CURRENT_MAGIC {
        return Err(StorageError::InvalidFormat("invalid CURRENT record".into()));
    }
    if bytes[8..12] != 1u32.to_le_bytes() || bytes[12..16] != 0u32.to_le_bytes() {
        return Err(StorageError::InvalidFormat(
            "unsupported CURRENT version or flags".into(),
        ));
    }
    use sha2::Digest;
    let checksum = sha2::Sha256::digest(&bytes[..96]);
    if checksum.as_slice() != &bytes[96..] {
        return Err(StorageError::InvalidFormat("CURRENT checksum mismatch".into()));
    }
    Ok(CurrentToken {
        generation: u64::from_le_bytes(bytes[16..24].try_into().expect("fixed generation")),
        manifest_digest: bytes[24..56].try_into().expect("fixed digest"),
        manifest_length: u64::from_le_bytes(bytes[56..64].try_into().expect("fixed length")),
        store_identity: bytes[64..80].try_into().expect("fixed store identity"),
        lineage_identity: bytes[80..96].try_into().expect("fixed lineage identity"),
    })
}

fn session_for(repository: &Repository) -> Result<&StorageSession, IndexError> {
    repository
        .storage
        .as_deref()
        .ok_or_else(|| IndexError::Io("repository was not opened from storage".into()))
}

fn status_for(repository: &Repository) -> Result<StorageStatus, IndexError> {
    let session = session_for(repository)?;
    let (store_identity, lineage_identity) = session.identities();
    let snapshot =
        read_snapshot(session.directory()).map_err(crate::storage::storage_to_index_error)?;
    Ok(StorageStatus {
        directory: session.directory().to_path_buf(),
        generation: session.generation(),
        logical_revision: IndexRevision(snapshot.logical_revision),
        store_identity,
        lineage_identity,
        eager_file_rows: snapshot.files.files.len(),
        eager_record_rows: snapshot.counters.eager_record_rows as usize,
        eager_index_descriptors: snapshot.counters.eager_index_descriptors as usize,
        eager_leaf_rows: snapshot.counters.eager_leaf_rows as usize,
        stored_objects: snapshot.stored_objects.len(),
        referenced_bytes: snapshot.stored_objects.iter().map(|object| object.length).sum(),
        cache: session.cache().status(),
        source_reads: 0,
        source_parses: 0,
        index_reads: session.index_reads.load(std::sync::atomic::Ordering::Acquire),
        index_bytes: session.index_bytes.load(std::sync::atomic::Ordering::Acquire),
    })
}

pub(crate) fn index_status(index: &Index) -> Result<StorageStatus, IndexError> {
    let view = index.view()?;
    status_for(view.repository())
}

pub(crate) fn repository_status(repository: &Repository) -> Result<StorageStatus, IndexError> {
    status_for(repository)
}

pub fn audit(directory: &Path, options: AuditOptions) -> Result<AuditReport, StorageError> {
    if options.worker_limit.get() == 0 {
        return Err(StorageError::limit("audit worker limit must be nonzero"));
    }
    let current = read_current(directory)?;
    let snapshot = read_snapshot(directory)?;
    let mut errors = Vec::new();
    let mut bytes = 0u64;
    for object in &snapshot.stored_objects {
        let path = directory.join("objects").join(format::hex(&object.digest));
        match fs::read(&path) {
            Ok(data) if data.len() as u64 == object.length && format::sha256(&data) == object.digest => {
                bytes = bytes.saturating_add(data.len() as u64);
            }
            Ok(_) => errors.push(format!("object {} failed verification", format::hex(&object.digest))),
            Err(error) => errors.push(format!("object {}: {error}", format::hex(&object.digest))),
        }
    }
    Ok(AuditReport {
        generation: current.generation,
        objects: snapshot.stored_objects.len(),
        bytes,
        errors,
    })
}

pub fn prune(directory: &Path) -> Result<PruneReport, StorageError> {
    let readers = open_exclusive_lock(&directory.join("readers.lock"))?;
    let writer = open_exclusive_lock(&directory.join("writer.lock"))?;
    let current = read_current(directory)?;
    let snapshot = read_snapshot(directory)?;
    let keep = snapshot
        .stored_objects
        .iter()
        .map(|object| format::hex(&object.digest))
        .chain(std::iter::once(format::hex(&current.manifest_digest)))
        .collect::<std::collections::HashSet<_>>();
    let mut removed_objects = 0usize;
    let mut removed_bytes = 0u64;
    for entry in fs::read_dir(directory.join("objects"))
        .map_err(|error| StorageError::Io(error.to_string()))?
    {
        let entry = entry.map_err(|error| StorageError::Io(error.to_string()))?;
        if !entry.path().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !keep.contains(&name) {
            let length = entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);
            fs::remove_file(entry.path()).map_err(|error| StorageError::Io(error.to_string()))?;
            removed_objects += 1;
            removed_bytes = removed_bytes.saturating_add(length);
        }
    }
    sync_directory(&directory.join("objects"))?;
    drop(writer);
    drop(readers);
    Ok(PruneReport {
        current_generation: current.generation,
        removed_objects,
        removed_bytes,
        preserved_objects: keep.len(),
    })
}

fn open_exclusive_lock(path: &Path) -> Result<File, StorageError> {
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| StorageError::Io(error.to_string()))?;
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result != 0 {
        return Err(StorageError::StorageBusy(
            "storage exclusive lock is busy".into(),
        ));
    }
    Ok(file)
}

fn read_snapshot(directory: &Path) -> Result<model::StorageSnapshot, StorageError> {
    let current = read_current(directory)?;
    let bytes = fs::read(directory.join("objects").join(format::hex(&current.manifest_digest)))
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => StorageError::MissingObject(current.manifest_digest),
            _ => StorageError::Io(error.to_string()),
        })?;
    if bytes.len() as u64 != current.manifest_length
        || format::sha256(&bytes) != current.manifest_digest
    {
        return Err(StorageError::InvalidFormat("manifest verification failed".into()));
    }
    bincode_options()
        .deserialize(format::checked_body(&bytes, 1)?)
        .map_err(|error| StorageError::InvalidFormat(error.to_string()))
}

fn sync_directory(directory: &Path) -> Result<(), StorageError> {
    File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|error| StorageError::Io(error.to_string()))
}
