use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use serde::Serialize;

use crate::bigram::{
    BigramArchivePayload, BigramIndex, BigramIndexLoader, StoredBigramPayloadRef,
};
use crate::inventory::{ActiveRecords, FileVersions};
use crate::lifecycle::{
    Index, IndexError, IndexRevision, IndexStorageParts, PublishedSegment,
};
use crate::native_segment::IndexedCollectionSegment;
use crate::posting::{
    CustomPostingIndex, PostingArchivePayload, PostingCollection, PostingIndex, PostingIndexLoader,
    PostingIndexSnapshot, StoredPostingPayloadRef,
};
use crate::repository::{CollectionState, Repository};
use crate::segment_archive::{FileLookup, RecordLookup, SegmentRows};
use crate::sorting::{
    SortArchivePayload, SortCollection, SortCollectionLoader, StoredSortPayloadRef,
};
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
        .join(format::object_file_name(
            &current.manifest_digest,
            METADATA_OBJECT_KIND,
        ));
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
    let (snapshot, _, metadata_reads, metadata_bytes) =
        read_metadata_snapshot(&bytes, directory, limits)?;
    model::validate_snapshot(&snapshot).map_err(StorageError::InvalidFormat)?;
    if !snapshot.complete {
        return Err(StorageError::IncompleteBuild);
    }
    if snapshot.lineage_identity != current.lineage_identity {
        return Err(StorageError::IncompatibleConfiguration(
            "manifest and CURRENT lineage identities differ".into(),
        ));
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
        manifest_digest: current.manifest_digest,
        manifest_length: current.manifest_length,
        limits,
        index_reads: AtomicU64::new(metadata_reads),
        index_bytes: AtomicU64::new(metadata_bytes),
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
            postings: posting_collection_from_storage(
                stored.postings.clone(),
                Arc::clone(&session),
            )?,
            inference: stored
                .inference
                .clone()
                .map(|snapshot| inference_from_storage(snapshot, Arc::clone(&session)))
                .transpose()?,
            global_inference: stored.global_inference.clone(),
            custom_postings: stored
                .custom_postings
                .iter()
                .map(|(key, custom)| {
                    Ok((
                        key.clone(),
                        CustomPostingIndex {
                            index: posting_from_storage(
                                custom.index.clone(),
                                Arc::clone(&session),
                            )?,
                            complete: custom.complete,
                        },
                    ))
                })
                .collect::<Result<_, StorageError>>()?,
            raw: Arc::new(bigram_from_storage(
                stored.raw.clone(),
                Arc::clone(&session),
            )?),
            fallback_text: Arc::new(bigram_from_storage(
                stored.fallback_text.clone(),
                Arc::clone(&session),
            )?),
            logical_records: stored.logical_records.clone(),
            empty_array_records: stored.empty_array_records.clone(),
            custom_text: Arc::new(
                stored
                    .custom_text
                    .iter()
                    .map(|(key, bigram)| {
                        Ok((
                            key.clone(),
                            bigram_from_storage(bigram.clone(), Arc::clone(&session))?,
                        ))
                    })
                    .collect::<Result<_, StorageError>>()?,
            ),
            text_bindings: crate::index_catalog::TextBindingCatalog::new(&definition.schema),
            sorting: Arc::new(sorting_from_storage(
                stored.sorting.clone(),
                Arc::clone(&session),
            )?),
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
            let index = rebuild_segment(&published.segment, Arc::clone(&session))?;
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
    let (manifest_digest, _manifest_length) = session.manifest_token();
    let (store_identity, _lineage_identity) = session.identities();
    *repository
        .storage_binding
        .lock()
        .map_err(|_| StorageError::Io("storage binding lock poisoned".into()))? =
        Some(crate::storage::StorageBinding {
            directory: session.directory().to_path_buf(),
            generation: session.generation(),
            manifest_digest,
            store_identity,
            revision: repository.revision,
            configuration_identity: snapshot.configuration_identity,
        });
    repository.storage = Some(session);
    Ok((repository, base_active, groups, catalog, versions))
}

const POSTING_PAYLOAD_OBJECT_KIND: u16 = 3;
const BIGRAM_PAYLOAD_OBJECT_KIND: u16 = 4;
const SORT_PAYLOAD_OBJECT_KIND: u16 = 5;
const METADATA_OBJECT_KIND: u16 = 1;
const METADATA_CHUNK_OBJECT_KIND: u16 = 6;

struct StoredPostingLoader {
    session: SharedStorageSession,
    reference: StoredPostingPayloadRef,
}

impl std::fmt::Debug for StoredPostingLoader {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredPostingLoader")
            .field("digest", &format::hex(&self.reference.digest))
            .field("length", &self.reference.length)
            .finish()
    }
}

impl PostingIndexLoader for StoredPostingLoader {
    fn load_posting_index(&self) -> Result<PostingIndexSnapshot, String> {
        let bytes = read_checked_object(
            &self.session,
            self.reference.digest,
            self.reference.length,
            POSTING_PAYLOAD_OBJECT_KIND,
        )?;
        let mut aligned = rkyv::util::AlignedVec::<16>::with_capacity(bytes.len());
        aligned.extend_from_slice(&bytes);
        let owner = crate::archive_storage::ValidatedArchive::<PostingArchivePayload>::checked_from_aligned(aligned)
            .map_err(|error| format!("posting payload validation failed: {error}"))?;
        let payload = rkyv::deserialize::<PostingArchivePayload, rkyv::rancor::Error>(owner.root())
            .map_err(|error| format!("posting payload decode failed: {error}"))?;
        PostingIndexSnapshot::from_archive_payload(payload)
    }
}

struct StoredBigramLoader {
    session: SharedStorageSession,
    reference: StoredBigramPayloadRef,
}

impl std::fmt::Debug for StoredBigramLoader {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredBigramLoader")
            .field("digest", &format::hex(&self.reference.digest))
            .field("length", &self.reference.length)
            .finish()
    }
}

impl BigramIndexLoader for StoredBigramLoader {
    fn load_bigram_index(&self) -> Result<BigramIndex, String> {
        let bytes = read_checked_object(
            &self.session,
            self.reference.digest,
            self.reference.length,
            BIGRAM_PAYLOAD_OBJECT_KIND,
        )?;
        let mut aligned = rkyv::util::AlignedVec::<16>::with_capacity(bytes.len());
        aligned.extend_from_slice(&bytes);
        let owner = crate::archive_storage::ValidatedArchive::<BigramArchivePayload>::checked_from_aligned(aligned)
            .map_err(|error| format!("bigram payload validation failed: {error}"))?;
        let payload = rkyv::deserialize::<BigramArchivePayload, rkyv::rancor::Error>(owner.root())
            .map_err(|error| format!("bigram payload decode failed: {error}"))?;
        BigramIndex::from_archive_payload(payload)
    }
}

struct StoredSortLoader {
    session: SharedStorageSession,
    reference: StoredSortPayloadRef,
}

impl std::fmt::Debug for StoredSortLoader {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredSortLoader")
            .field("digest", &format::hex(&self.reference.digest))
            .field("length", &self.reference.length)
            .finish()
    }
}

impl SortCollectionLoader for StoredSortLoader {
    fn load_sort_collection(&self) -> Result<SortCollection, String> {
        let bytes = read_checked_object(
            &self.session,
            self.reference.digest,
            self.reference.length,
            SORT_PAYLOAD_OBJECT_KIND,
        )?;
        let mut aligned = rkyv::util::AlignedVec::<16>::with_capacity(bytes.len());
        aligned.extend_from_slice(&bytes);
        let owner = crate::archive_storage::ValidatedArchive::<SortArchivePayload>::checked_from_aligned(aligned)
            .map_err(|error| format!("sort payload validation failed: {error}"))?;
        let payload = rkyv::deserialize::<SortArchivePayload, rkyv::rancor::Error>(owner.root())
            .map_err(|error| format!("sort payload decode failed: {error}"))?;
        SortCollection::from_archive_payload(payload)
    }
}

fn read_checked_object(
    session: &StorageSession,
    digest: [u8; 32],
    length: u64,
    kind: u16,
) -> Result<Vec<u8>, String> {
    let limit = match kind {
        POSTING_PAYLOAD_OBJECT_KIND | BIGRAM_PAYLOAD_OBJECT_KIND | SORT_PAYLOAD_OBJECT_KIND => {
            session.limits().lazy_dictionary_body_bytes
        }
        _ => session.limits().eager_catalog_body_bytes,
    };
    if length > limit {
        return Err(format!(
            "storage object {} requires {length} bytes, limit is {limit}",
            format::hex(&digest)
        ));
    }
    let cache_key = (digest, length, u32::from(kind));
    if let Some(bytes) = session.cache().get(&cache_key) {
        return Ok(bytes.as_ref().to_vec());
    }
    session.cache().record_miss();
    let path = session
        .directory()
        .join("objects")
        .join(format::object_file_name(&digest, kind));
    let bytes = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    session
        .index_reads
        .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    session
        .index_bytes
        .fetch_add(bytes.len() as u64, std::sync::atomic::Ordering::AcqRel);
    if bytes.len() as u64 != length {
        return Err(format!(
            "object {} length mismatch: expected {length}, got {}",
            format::hex(&digest),
            bytes.len()
        ));
    }
    if format::sha256(&bytes) != digest {
        return Err(format!("object {} checksum mismatch", format::hex(&digest)));
    }
    let body = format::checked_body(&bytes, kind)
        .map_err(|error| error.to_string())?
        .to_vec();
    session.cache().insert(cache_key, Arc::from(body.clone()));
    Ok(body)
}

fn posting_from_storage(
    snapshot: PostingIndexSnapshot,
    session: SharedStorageSession,
) -> Result<PostingIndex, StorageError> {
    match snapshot.payload {
        Some(reference) => PostingIndex::from_stored_metadata(
            snapshot,
            Arc::new(StoredPostingLoader { session, reference }),
        )
        .map_err(|message| StorageError::Corrupt {
            identity: "stored posting metadata".into(),
            message,
        }),
        None => PostingIndex::from_storage_snapshot(snapshot).map_err(|message| {
            StorageError::Corrupt {
                identity: "posting metadata".into(),
                message,
            }
        }),
    }
}

fn bigram_from_storage(
    mut snapshot: BigramIndex,
    session: SharedStorageSession,
) -> Result<BigramIndex, StorageError> {
    if let Some(reference) = snapshot.payload {
        snapshot.storage = Some(Arc::new(StoredBigramLoader { session, reference }));
    }
    Ok(snapshot)
}

fn sorting_from_storage(
    mut sorting: SortCollection,
    session: SharedStorageSession,
) -> Result<SortCollection, StorageError> {
    if let Some(reference) = sorting.payload {
        sorting.storage = Some(Arc::new(StoredSortLoader { session, reference }));
    }
    Ok(sorting)
}

fn posting_collection_from_storage(
    snapshot: crate::posting::PostingCollectionSnapshot,
    session: SharedStorageSession,
) -> Result<PostingCollection, StorageError> {
    Ok(PostingCollection {
        fields: snapshot
            .fields
            .into_iter()
            .map(|(path, posting)| Ok((path, posting_from_storage(posting, Arc::clone(&session))?)))
            .collect::<Result<_, StorageError>>()?,
    })
}

fn inference_from_storage(
    snapshot: crate::inference::InferenceStateSnapshot,
    session: SharedStorageSession,
) -> Result<crate::inference::InferenceState, StorageError> {
    Ok(crate::inference::InferenceState {
        report: snapshot.report,
        indexes: snapshot
            .indexes
            .into_iter()
            .map(|(path, posting)| {
                Ok((path, posting_from_storage(posting, Arc::clone(&session))?))
            })
            .collect::<Result<_, StorageError>>()?,
        empty_selections: snapshot.empty_selections.into_iter().collect(),
        short_string_coverage: snapshot.short_string_coverage.into_iter().collect(),
    })
}

fn rebuild_segment(
    snapshot: &model::SegmentSnapshot,
    session: SharedStorageSession,
) -> Result<IndexedCollectionSegment, StorageError> {
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
                    index: posting_from_storage(custom.index.clone(), Arc::clone(&session))?,
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
        postings: posting_collection_from_storage(
            snapshot.postings.clone(),
            Arc::clone(&session),
        )?,
        inference_observation: snapshot.inference_observation.clone(),
        inference: snapshot
            .inference
            .clone()
            .map(|inference| inference_from_storage(inference, Arc::clone(&session)))
            .transpose()?,
        custom_postings,
        raw: Arc::new(bigram_from_storage(
            snapshot.raw.clone(),
            Arc::clone(&session),
        )?),
        fallback_text: Arc::new(bigram_from_storage(
            snapshot.fallback_text.clone(),
            Arc::clone(&session),
        )?),
        custom_text: Arc::new(
            snapshot
                .custom_text
                .iter()
                .map(|(key, index)| {
                    Ok((
                        key.clone(),
                        bigram_from_storage(index.clone(), Arc::clone(&session))?,
                    ))
                })
                .collect::<Result<_, StorageError>>()?,
        ),
        logical_records: snapshot.logical_records.clone(),
        empty_array_records: snapshot.empty_array_records.clone(),
        sorting: Arc::new(sorting_from_storage(
            snapshot.sorting.clone(),
            session,
        )?),
        relations: Arc::new(snapshot.relations.clone()),
        record_parses: snapshot.record_parses,
        failed_parses: snapshot.failed_parses,
        accounting: snapshot.accounting,
        storage_completed: true,
    })
}

fn open_shared_lock(path: &Path) -> Result<File, StorageError> {
    let file = OpenOptions::new()
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

fn read_metadata_snapshot(
    manifest_object: &[u8],
    directory: &Path,
    limits: crate::storage::StorageReadLimits,
) -> Result<(model::StorageSnapshot, model::MetadataManifest, u64, u64), StorageError> {
    let body = format::checked_body(manifest_object, METADATA_OBJECT_KIND)?;
    let mut aligned = rkyv::util::AlignedVec::<16>::with_capacity(body.len());
    aligned.extend_from_slice(body);
    let owner =
        crate::archive_storage::ValidatedArchive::<model::MetadataManifest>::checked_from_aligned(
            aligned,
        )
        .map_err(StorageError::InvalidFormat)?;
    let manifest =
        rkyv::deserialize::<model::MetadataManifest, rkyv::rancor::Error>(owner.root())
            .map_err(|error| StorageError::InvalidFormat(error.to_string()))?;
    if manifest.schema_version != model::SNAPSHOT_SCHEMA_VERSION {
        return Err(StorageError::IncompatibleConfiguration(
            "metadata manifest schema version differs".into(),
        ));
    }
    if manifest.total_length > limits.total_eager_catalog_bytes {
        return Err(StorageError::LimitExceeded {
            limit: limits.total_eager_catalog_bytes,
            requested: manifest.total_length,
        });
    }
    let mut body = Vec::with_capacity(usize::try_from(manifest.total_length).map_err(|_| {
        StorageError::LimitExceeded {
            limit: limits.total_eager_catalog_bytes,
            requested: manifest.total_length,
        }
    })?);
    let mut bytes_read = manifest_object.len() as u64;
    let mut read_count = 1u64;
    for chunk in &manifest.chunks {
        if chunk.length > limits.eager_catalog_body_bytes {
            return Err(StorageError::LimitExceeded {
                limit: limits.eager_catalog_body_bytes,
                requested: chunk.length,
            });
        }
        let path = directory
            .join("objects")
            .join(format::object_file_name(
                &chunk.digest,
                METADATA_CHUNK_OBJECT_KIND,
            ));
        let bytes = fs::read(&path).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => StorageError::MissingObject(chunk.digest),
            _ => StorageError::Io(error.to_string()),
        })?;
        if bytes.len() as u64 != chunk.length || format::sha256(&bytes) != chunk.digest {
            return Err(StorageError::corrupt(
                format::hex(&chunk.digest),
                "metadata chunk length or digest mismatch",
            ));
        }
        read_count = read_count.saturating_add(1);
        bytes_read = bytes_read.saturating_add(bytes.len() as u64);
        let chunk_body = format::checked_body(&bytes, METADATA_CHUNK_OBJECT_KIND)?;
        let mut aligned = rkyv::util::AlignedVec::<16>::with_capacity(chunk_body.len());
        aligned.extend_from_slice(chunk_body);
        let owner =
            crate::archive_storage::ValidatedArchive::<model::MetadataChunk>::checked_from_aligned(
                aligned,
            )
            .map_err(StorageError::InvalidFormat)?;
        let chunk = rkyv::deserialize::<model::MetadataChunk, rkyv::rancor::Error>(owner.root())
            .map_err(|error| StorageError::InvalidFormat(error.to_string()))?;
        let next_length = body.len().checked_add(chunk.bytes.len()).ok_or(
            StorageError::LimitExceeded {
                limit: limits.total_eager_catalog_bytes,
                requested: u64::MAX,
            },
        )?;
        if next_length as u64 > manifest.total_length {
            return Err(StorageError::InvalidFormat(
                "metadata chunks exceed the declared body length".into(),
            ));
        }
        body.extend_from_slice(&chunk.bytes);
    }
    if body.len() as u64 != manifest.total_length {
        return Err(StorageError::InvalidFormat(
            "metadata chunks do not cover the declared body length".into(),
        ));
    }
    let snapshot = serde_json::from_slice(&body)
        .map_err(|error| StorageError::InvalidFormat(error.to_string()))?;
    Ok((snapshot, manifest, read_count, bytes_read))
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
    let current = read_current(session.directory())
        .map_err(crate::storage::storage_to_index_error)?;
    let manifest_path = session
        .directory()
        .join("objects")
        .join(format::object_file_name(
            &current.manifest_digest,
            METADATA_OBJECT_KIND,
        ));
    let manifest_object = fs::read(&manifest_path)
        .map_err(|error| IndexError::Io(error.to_string()))?;
    let (_, metadata_manifest, _, metadata_bytes) = read_metadata_snapshot(
        &manifest_object,
        session.directory(),
        session.limits(),
    )
    .map_err(crate::storage::storage_to_index_error)?;
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
        stored_objects: snapshot
            .stored_objects
            .len()
            .saturating_add(metadata_manifest.chunks.len())
            .saturating_add(1),
        referenced_bytes: metadata_bytes.saturating_add(
            snapshot
                .stored_objects
                .iter()
                .map(|object| object.length)
                .sum(),
        ),
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

pub(crate) fn index_trim_cache(index: &Index) -> Result<format::CacheTrimReport, IndexError> {
    let view = index.view()?;
    Ok(session_for(view.repository())?.cache().trim())
}

pub(crate) fn repository_trim_cache(
    repository: &Repository,
) -> Result<format::CacheTrimReport, IndexError> {
    Ok(session_for(repository)?.cache().trim())
}

pub fn audit(directory: &Path, options: AuditOptions) -> Result<AuditReport, StorageError> {
    if options.worker_limit.get() == 0 {
        return Err(StorageError::limit("audit worker limit must be nonzero"));
    }
    let _readers = open_shared_lock(&directory.join("readers.lock"))?;
    let current = read_current(directory)?;
    let snapshot = read_snapshot(directory)?;
    let manifest_path = directory
        .join("objects")
        .join(format::object_file_name(
            &current.manifest_digest,
            METADATA_OBJECT_KIND,
        ));
    let manifest_bytes = fs::read(&manifest_path)
        .map_err(|error| StorageError::Io(error.to_string()))?;
    let (_, metadata_manifest, _, _) = read_metadata_snapshot(
        &manifest_bytes,
        directory,
        crate::storage::StorageReadLimits::default(),
    )?;
    let mut errors = Vec::new();
    let mut bytes = 0u64;
    let mut objects = snapshot
        .stored_objects
        .iter()
        .map(|object| (object.digest, object.length, u16::try_from(object.kind).unwrap_or(u16::MAX)))
        .collect::<Vec<_>>();
    objects.push((current.manifest_digest, current.manifest_length, METADATA_OBJECT_KIND));
    objects.extend(metadata_manifest.chunks.iter().map(|chunk| {
        (chunk.digest, chunk.length, METADATA_CHUNK_OBJECT_KIND)
    }));
    for (digest, length, kind) in &objects {
        let path = directory
            .join("objects")
            .join(format::object_file_name(digest, *kind));
        match fs::read(&path) {
            Ok(data) if data.len() as u64 == *length && format::sha256(&data) == *digest => {
                bytes = bytes.saturating_add(data.len() as u64);
            }
            Ok(_) => errors.push(format!("object {} failed verification", format::hex(digest))),
            Err(error) => errors.push(format!("object {}: {error}", format::hex(digest))),
        }
    }
    Ok(AuditReport {
        generation: current.generation,
        objects: objects.len(),
        bytes,
        errors,
    })
}

pub fn prune(directory: &Path) -> Result<PruneReport, StorageError> {
    let readers = open_exclusive_lock(&directory.join("readers.lock"))?;
    let writer = open_exclusive_lock(&directory.join("writer.lock"))?;
    let current = read_current(directory)?;
    let snapshot = read_snapshot(directory)?;
    let manifest_path = directory
        .join("objects")
        .join(format::object_file_name(
            &current.manifest_digest,
            METADATA_OBJECT_KIND,
        ));
    let manifest_bytes = fs::read(&manifest_path)
        .map_err(|error| StorageError::Io(error.to_string()))?;
    let (_, metadata_manifest, _, _) = read_metadata_snapshot(
        &manifest_bytes,
        directory,
        crate::storage::StorageReadLimits::default(),
    )?;
    let keep = snapshot
        .stored_objects
        .iter()
        .map(|object| {
            format::object_file_name(
                &object.digest,
                u16::try_from(object.kind).unwrap_or(u16::MAX),
            )
        })
        .chain(std::iter::once(format::object_file_name(
            &current.manifest_digest,
            METADATA_OBJECT_KIND,
        )))
        .chain(metadata_manifest.chunks.iter().map(|chunk| {
            format::object_file_name(&chunk.digest, METADATA_CHUNK_OBJECT_KIND)
        }))
        .collect::<std::collections::HashSet<_>>();
    let mut removed_objects = 0usize;
    let mut removed_bytes = 0u64;
    for entry in fs::read_dir(directory.join("objects"))
        .map_err(|error| StorageError::Io(error.to_string()))?
    {
        let entry = entry.map_err(|error| StorageError::Io(error.to_string()))?;
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| StorageError::Io(error.to_string()))?;
        if !metadata.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_owned_object_name(&name) {
            continue;
        }
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

fn is_owned_object_name(name: &str) -> bool {
    let Some((digest, suffix)) = name.rsplit_once('.') else {
        return false;
    };
    digest.len() == 64
        && digest.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        && matches!(suffix, "meta" | "pack")
}

fn open_exclusive_lock(path: &Path) -> Result<File, StorageError> {
    let file = OpenOptions::new()
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

pub(crate) fn read_snapshot(directory: &Path) -> Result<model::StorageSnapshot, StorageError> {
    let current = read_current(directory)?;
    let bytes = fs::read(
        directory
            .join("objects")
            .join(format::object_file_name(
                &current.manifest_digest,
                METADATA_OBJECT_KIND,
            )),
    )
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => StorageError::MissingObject(current.manifest_digest),
            _ => StorageError::Io(error.to_string()),
        })?;
    if bytes.len() as u64 != current.manifest_length
        || format::sha256(&bytes) != current.manifest_digest
    {
        return Err(StorageError::InvalidFormat("manifest verification failed".into()));
    }
    read_metadata_snapshot(
        &bytes,
        directory,
        crate::storage::StorageReadLimits::default(),
    )
    .map(|(snapshot, _, _, _)| snapshot)
}

fn sync_directory(directory: &Path) -> Result<(), StorageError> {
    File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|error| StorageError::Io(error.to_string()))
}
