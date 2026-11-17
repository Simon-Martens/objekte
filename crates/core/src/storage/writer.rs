use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use bincode::Options;
use sha2::{Digest, Sha256};

use crate::lifecycle::{
    ArchiveOptions, ArchiveReport, Index, IndexError, IndexRevision, IndexStorageParts,
};
use crate::repository::{CollectionState, Repository};
use crate::storage::{StorageError, StorageWriteOptions, StorageWriteReport, format, model};

const METADATA_OBJECT_KIND: u16 = 1;
const PAYLOAD_OBJECT_KIND: u16 = 2;

fn bincode_options() -> impl bincode::Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .reject_trailing_bytes()
}

pub(crate) fn archive_index_to(
    index: &Index,
    directory: &Path,
    options: StorageWriteOptions,
) -> Result<StorageWriteReport, IndexError> {
    let started = Instant::now();
    let archive_started = Instant::now();
    let archive_report = index.archive_with_options(ArchiveOptions::new(
        options.archive_workers.get(),
    )?)?;
    let archive_ns = archive_started.elapsed().as_nanos() as u64;
    let parts = index.storage_parts()?;
    write_parts(directory, parts, archive_report, archive_ns, started)
        .map_err(crate::storage::storage_to_index_error)
}

pub(crate) fn archive_repository_to(
    repository: &mut Repository,
    directory: &Path,
    options: StorageWriteOptions,
) -> Result<StorageWriteReport, IndexError> {
    let started = Instant::now();
    let archive_started = Instant::now();
    let archive_report = repository.archive_with_options(ArchiveOptions::new(
        options.archive_workers.get(),
    )?)?;
    let archive_ns = archive_started.elapsed().as_nanos() as u64;
    let versions = crate::inventory::FileVersions::filled(
        repository.files().files.len(),
        crate::inventory::FileVersion(repository.revision.0),
    );
    let parts = IndexStorageParts {
        repository: Arc::new(
            repository
                .archive_candidate()
                .map_err(IndexError::InvalidPublication)?,
        ),
        configuration_identity: crate::configuration::configuration_hash(
            &repository.collections().iter().map(|c| c.definition.clone()).collect::<Vec<_>>(),
            repository.options(),
            repository.sources(),
        )
        .map_err(IndexError::IncompatibleConfiguration)?,
        active: repository.active.clone(),
        segments: repository.segments.clone(),
        next_segment_ordinal: repository
            .segments
            .iter()
            .flat_map(|group| group.iter())
            .map(|segment| segment.id.get())
            .max()
            .unwrap_or(0)
            .saturating_add(1),
        last_size_maintenance_revision: 0,
        catalog: repository.catalog.clone(),
        file_versions: versions,
        processed_files: repository.catalog.active_files() as usize,
        revision: repository.revision,
        representation_generation: repository.representation_generation,
    };
    write_parts(directory, parts, archive_report, archive_ns, started)
        .map_err(crate::storage::storage_to_index_error)
}

fn write_parts(
    directory: &Path,
    parts: IndexStorageParts,
    archive_report: ArchiveReport,
    archive_ns: u64,
    started: Instant,
) -> Result<StorageWriteReport, StorageError> {
    ensure_directory(directory)?;
    let current = read_current_generation(directory)?;
    let generation = current
        .map_or(1, |generation| generation.saturating_add(1));
    let encode_started = Instant::now();
    let snapshot = snapshot_from_parts(parts, generation)?;
    let payload = format::envelope(PAYLOAD_OBJECT_KIND, &[]);
    let payload_digest = format::sha256(&payload);
    let mut snapshot = snapshot;
    snapshot
        .stored_objects
        .push(model::StoredObjectDescriptor::new(
            PAYLOAD_OBJECT_KIND as u32,
            &payload,
        ));
    let metadata = bincode_options()
        .serialize(&snapshot)
        .map_err(|error| StorageError::Io(error.to_string()))?;
    let metadata = format::envelope(METADATA_OBJECT_KIND, &metadata);
    let metadata_digest = format::sha256(&metadata);
    let encode_ns = encode_started.elapsed().as_nanos() as u64;

    let write_started = Instant::now();
    let objects = directory.join("objects");
    let staging = directory.join("staging");
    fs::create_dir_all(&objects)
        .map_err(|error| StorageError::Io(error.to_string()))?;
    fs::create_dir_all(&staging)
        .map_err(|error| StorageError::Io(error.to_string()))?;
    write_object(&staging, &objects, payload_digest, &payload)?;
    write_object(&staging, &objects, metadata_digest, &metadata)?;
    sync_directory(&objects)?;
    let write_ns = write_started.elapsed().as_nanos() as u64;

    let sync_started = Instant::now();
    let current_bytes = make_current(
        generation,
        metadata_digest,
        metadata.len() as u64,
        [generation as u8; 16],
        snapshot.lineage_identity,
    );
    install_current(directory, generation, &current_bytes)?;
    sync_directory(directory)?;
    let sync_ns = sync_started.elapsed().as_nanos() as u64;

    Ok(StorageWriteReport {
        generation,
        logical_revision: IndexRevision(snapshot.logical_revision),
        unchanged: false,
        archive_report,
        objects_new: 2,
        objects_reused: 0,
        bytes_serialized: (metadata.len() + payload.len()) as u64,
        bytes_written: (metadata.len() + payload.len()) as u64,
        sync_calls: 3,
        archive_ns,
        encode_ns,
        write_ns,
        sync_ns,
        total_ns: started.elapsed().as_nanos() as u64,
        manifest_digest: metadata_digest,
    })
}

fn snapshot_from_parts(
    parts: IndexStorageParts,
    _generation: u64,
) -> Result<model::StorageSnapshot, StorageError> {
    let repository = Arc::clone(&parts.repository);
    let mut collections = Vec::with_capacity(repository.collections().len());
    for collection in repository.collections() {
        collections.push(snapshot_collection(collection)?);
    }
    let mut segments = Vec::with_capacity(parts.segments.len());
    for group in &parts.segments {
        let mut stored_group = Vec::with_capacity(group.len());
        for segment in group.iter() {
            let mut files = Vec::with_capacity(segment.index.file_count());
            for file in segment.index.files.iter() {
                files.push(model::StoredSegmentFile {
                    entry: file.entry.clone(),
                    records: file.records.clone(),
                });
            }
            let file_index = (0..segment.index.file_count())
                .filter_map(|ordinal| {
                    segment
                        .index
                        .files
                        .file_id(ordinal)
                        .map(|id| (id, ordinal as u32))
                })
                .collect();
            let id_lookup = segment
                .index
                .records
                .iter()
                .filter_map(|record| {
                    segment
                        .index
                        .id_lookup
                        .get(record.id.0.as_str())
                        .map(|slot| (record.id.clone(), slot))
                })
                .collect();
            let custom_postings = segment
                .index
                .custom_postings
                .iter()
                .map(|(key, custom)| {
                    Ok((
                        key.clone(),
                        model::StoredCustomPosting {
                            index: custom.index.storage_snapshot()?,
                            complete: custom.complete,
                        },
                    ))
                })
                .collect::<Result<_, String>>()
                .map_err(StorageError::UnsupportedComponent)?;
            stored_group.push(model::PublishedSegmentSnapshot {
                segment: model::SegmentSnapshot {
                    id: segment.id,
                    collection: segment.index.collection,
                    source_id: segment.index.source_id.clone(),
                    source_version: segment.index.source_version,
                    files,
                    records: segment.index.records.iter().cloned().collect(),
                    file_index,
                    id_lookup,
                    identity_order: (*segment.index.identity_order).clone(),
                    postings: segment
                        .index
                        .postings
                        .storage_snapshot()
                        .map_err(StorageError::UnsupportedComponent)?,
                    inference_observation: segment.index.inference_observation.clone(),
                    inference: segment
                        .index
                        .inference
                        .as_ref()
                        .map(crate::inference::InferenceState::storage_snapshot)
                        .transpose()
                        .map_err(StorageError::UnsupportedComponent)?,
                    custom_postings,
                    raw: segment.index.raw.as_ref().clone(),
                    fallback_text: segment.index.fallback_text.as_ref().clone(),
                    custom_text: segment
                        .index
                        .custom_text
                        .iter()
                        .map(|(key, index)| (key.clone(), index.clone()))
                        .collect(),
                    logical_records: segment.index.logical_records.clone(),
                    empty_array_records: segment.index.empty_array_records.clone(),
                    sorting: segment.index.sorting.as_ref().clone(),
                    relations: segment.index.relations.as_ref().clone(),
                    record_parses: segment.index.record_parses,
                    failed_parses: segment.index.failed_parses,
                    accounting: segment.index.accounting,
                },
                active: segment.active.bitmap(),
            });
        }
        segments.push(stored_group);
    }
    let catalog_slots = parts
        .catalog
        .snapshot_slots()
        .into_iter()
        .map(|slot| {
            slot.map(|(entry, fingerprint, configuration)| model::StoredCatalogSlot {
                entry,
                fingerprint,
                configuration,
            })
        })
        .collect();
    let counters = model::SnapshotCounters {
        eager_file_rows: repository.files().files.len() as u64,
        eager_record_rows: segments
            .iter()
            .flatten()
            .map(|segment| segment.segment.records.len() as u64)
            .sum(),
        eager_index_descriptors: repository
            .collections()
            .iter()
            .map(|collection| collection.postings.fields.len() as u64)
            .sum(),
        eager_leaf_rows: parts
            .segments
            .iter()
            .flat_map(|group| group.iter())
            .map(|segment| segment.index.postings.fields.len() as u64)
            .sum(),
    };
    let snapshot = model::StorageSnapshot {
        schema_version: model::SNAPSHOT_SCHEMA_VERSION,
        logical_revision: parts.revision.0,
        representation_generation: parts.representation_generation,
        instance_id: repository.instance_id,
        configuration_identity: parts.configuration_identity,
        source_root: repository.files().root.to_string_lossy().into_owned(),
        complete: true,
        files: (*repository.files()).clone(),
        base_fingerprints: parts.repository.base_fingerprints.as_ref().clone(),
        file_versions: model::FileVersionsSnapshot {
            versions: parts.file_versions.stored_versions(),
        },
        catalog_slots,
        base_active: parts.active.iter().map(|active| active.bitmap()).collect(),
        collections,
        segments,
        next_segment_ordinal: parts.next_segment_ordinal,
        last_size_maintenance_revision: parts.last_size_maintenance_revision,
        processed_files: parts.processed_files,
        build_parses: repository.build_parses(),
        build_failed_parses: repository.build_failed_parses(),
        stored_objects: Vec::new(),
        counters,
        lineage_identity: [0u8; 16],
    };
    model::validate_snapshot(&snapshot).map_err(StorageError::InvalidFormat)?;
    let mut snapshot = snapshot;
    snapshot.lineage_identity = stable_lineage(&snapshot.configuration_identity, &snapshot.source_root);
    Ok(snapshot)
}

fn snapshot_collection(collection: &CollectionState) -> Result<model::CollectionSnapshot, StorageError> {
    Ok(model::CollectionSnapshot {
        name: collection.definition.name.clone(),
        generation: collection.generation.clone(),
        postings: collection
            .postings
            .storage_snapshot()
            .map_err(StorageError::UnsupportedComponent)?,
        inference: collection
            .inference
            .as_ref()
            .map(crate::inference::InferenceState::storage_snapshot)
            .transpose()
            .map_err(StorageError::UnsupportedComponent)?,
        global_inference: collection.global_inference.clone(),
        custom_postings: collection
            .custom_postings
            .iter()
            .map(|(key, custom)| {
                Ok((
                    key.clone(),
                    model::StoredCustomPosting {
                        index: custom.index.storage_snapshot()?,
                        complete: custom.complete,
                    },
                ))
            })
            .collect::<Result<_, String>>()
            .map_err(StorageError::UnsupportedComponent)?,
        raw: collection.raw.as_ref().clone(),
        fallback_text: collection.fallback_text.as_ref().clone(),
        logical_records: collection.logical_records.clone(),
        empty_array_records: collection.empty_array_records.clone(),
        custom_text: collection
            .custom_text
            .iter()
            .map(|(key, index)| (key.clone(), index.clone()))
            .collect(),
        sorting: collection.sorting.as_ref().clone(),
        id_lookup: collection
            .id_lookup
            .iter()
            .map(|(id, slot)| (id.clone(), *slot))
            .collect(),
        identity_order: collection.identity_order.as_ref().clone(),
        stats: collection.stats,
        relations: collection.relations.as_ref().clone(),
    })
}

fn ensure_directory(directory: &Path) -> Result<(), StorageError> {
    if directory.exists() {
        if !directory.is_dir() {
            return Err(StorageError::InvalidFormat(
                "storage target is not a directory".into(),
            ));
        }
        return Ok(());
    }
    let parent = directory.parent().ok_or_else(|| {
        StorageError::Io("storage directory has no existing parent".into())
    })?;
    if !parent.is_dir() {
        return Err(StorageError::Io(format!(
            "storage parent is unavailable: {}",
            parent.display()
        )));
    }
    fs::create_dir(directory).map_err(|error| StorageError::Io(error.to_string()))
}

fn write_object(
    staging: &Path,
    objects: &Path,
    digest: [u8; 32],
    bytes: &[u8],
) -> Result<(), StorageError> {
    let staging_path = staging.join(format!("{}.tmp", format::hex(&digest)));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&staging_path)
        .map_err(|error| StorageError::Io(error.to_string()))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| StorageError::Io(error.to_string()))?;
    let final_path = objects.join(format::hex(&digest));
    if final_path.exists() {
        let existing = fs::read(&final_path).map_err(|error| StorageError::Io(error.to_string()))?;
        if existing != bytes {
            return Err(StorageError::Corrupt {
                identity: format::hex(&digest),
                message: "existing content-addressed object differs".into(),
            });
        }
        fs::remove_file(&staging_path).map_err(|error| StorageError::Io(error.to_string()))?;
        return Ok(());
    }
    fs::hard_link(&staging_path, &final_path)
        .map_err(|error| StorageError::Io(error.to_string()))?;
    fs::remove_file(&staging_path).map_err(|error| StorageError::Io(error.to_string()))?;
    File::open(&final_path)
        .and_then(|file| file.sync_all())
        .map_err(|error| StorageError::Io(error.to_string()))
}

fn make_current(
    generation: u64,
    manifest_digest: [u8; 32],
    manifest_len: u64,
    store_identity: [u8; 16],
    lineage_identity: [u8; 16],
) -> [u8; format::CURRENT_LEN] {
    let mut current = [0u8; format::CURRENT_LEN];
    current[..8].copy_from_slice(&format::CURRENT_MAGIC);
    current[8..12].copy_from_slice(&1u32.to_le_bytes());
    current[12..16].copy_from_slice(&0u32.to_le_bytes());
    current[16..24].copy_from_slice(&generation.to_le_bytes());
    current[24..56].copy_from_slice(&manifest_digest);
    current[56..64].copy_from_slice(&manifest_len.to_le_bytes());
    current[64..80].copy_from_slice(&store_identity);
    current[80..96].copy_from_slice(&lineage_identity);
    let checksum = Sha256::digest(&current[..96]);
    current[96..].copy_from_slice(&checksum);
    current
}

fn install_current(
    directory: &Path,
    generation: u64,
    bytes: &[u8; format::CURRENT_LEN],
) -> Result<(), StorageError> {
    let temporary = directory
        .join("staging")
        .join(format!("CURRENT-{generation}.tmp"));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| StorageError::Io(error.to_string()))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| StorageError::Io(error.to_string()))?;
    fs::rename(&temporary, directory.join("CURRENT"))
        .map_err(|error| StorageError::Io(error.to_string()))
}

fn read_current_generation(directory: &Path) -> Result<Option<u64>, StorageError> {
    let path = directory.join("CURRENT");
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(StorageError::Io(error.to_string())),
    };
    if bytes.len() != format::CURRENT_LEN || bytes[..8] != format::CURRENT_MAGIC {
        return Err(StorageError::InvalidFormat("invalid CURRENT record".into()));
    }
    let checksum = Sha256::digest(&bytes[..96]);
    if checksum.as_slice() != &bytes[96..] {
        return Err(StorageError::InvalidFormat("CURRENT checksum mismatch".into()));
    }
    Ok(Some(u64::from_le_bytes(
        bytes[16..24].try_into().expect("fixed generation bytes"),
    )))
}

fn sync_directory(directory: &Path) -> Result<(), StorageError> {
    File::open(directory)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| StorageError::Io(error.to_string()))
}

fn stable_lineage(configuration: &[u8; 32], root: &str) -> [u8; 16] {
    let mut hash = Sha256::new();
    hash.update(configuration);
    hash.update(root.as_bytes());
    let digest = hash.finalize();
    digest[..16].try_into().expect("fixed lineage length")
}

pub(crate) fn trim_cache() -> Result<format::CacheTrimReport, IndexError> {
    Ok(format::CacheTrimReport::default())
}
