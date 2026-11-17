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
const POSTING_PAYLOAD_OBJECT_KIND: u16 = 3;
const BIGRAM_PAYLOAD_OBJECT_KIND: u16 = 4;
const SORT_PAYLOAD_OBJECT_KIND: u16 = 5;

#[derive(Clone, Copy, Debug)]
struct CurrentRecord {
    generation: u64,
    manifest_digest: [u8; 32],
    manifest_length: u64,
    store_identity: [u8; 16],
    lineage_identity: [u8; 16],
}

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
    let _readers_lease = acquire_reader_lease(directory)?;
    let _writer_lock = acquire_writer_lock(directory)?;
    let current = read_current(directory)?;
    let binding_repository = Arc::clone(&parts.repository);
    let save_revision = parts.revision;
    let save_representation_generation = parts.representation_generation;
    let save_configuration_identity = parts.configuration_identity;
    let binding = parts
        .repository
        .storage_binding
        .lock()
        .map_err(|_| StorageError::Io("storage binding lock poisoned".into()))?
        .clone()
        .filter(|binding| binding.directory == directory);
    if let Some(binding) = &binding {
        let observed = current.ok_or_else(|| {
            StorageError::StorageConflict(
                "the bound storage destination no longer contains CURRENT".into(),
            )
        })?;
        if observed.generation != binding.generation
            || observed.manifest_digest != binding.manifest_digest
        {
            return Err(StorageError::StorageConflict(
                "another writer advanced the storage generation".into(),
            ));
        }
        if save_revision == binding.revision
            && save_configuration_identity == binding.configuration_identity
        {
            return Ok(StorageWriteReport {
                generation: binding.generation,
                logical_revision: save_revision,
                unchanged: true,
                archive_report,
                objects_new: 0,
                objects_reused: 0,
                bytes_serialized: 0,
                bytes_written: 0,
                sync_calls: 0,
                archive_ns,
                encode_ns: 0,
                write_ns: 0,
                sync_ns: 0,
                total_ns: started.elapsed().as_nanos() as u64,
                manifest_digest: binding.manifest_digest,
            });
        }
    } else if current.is_some() {
        return Err(StorageError::StorageConflict(
            "a nonempty storage destination is owned by another publication".into(),
        ));
    }
    let generation = match current {
        Some(current) => current
            .generation
            .checked_add(1)
            .ok_or_else(|| StorageError::StorageConflict("storage generation overflow".into()))?,
        None => 1,
    };
    let store_identity = binding
        .as_ref()
        .map_or_else(random_identity, |binding| binding.store_identity);
    let encode_started = Instant::now();
    let mut external_objects = Vec::new();
    let mut snapshot = snapshot_from_parts(parts, generation)?;
    if let Some(prior) = read_prior_snapshot(directory, current)? {
        let mut entries = prior
            .files
            .files
            .iter()
            .cloned()
            .map(|entry| (entry.id, entry))
            .collect::<std::collections::BTreeMap<_, _>>();
        for entry in snapshot.files.files.iter().cloned() {
            entries.insert(entry.id, entry);
        }
        for entry in snapshot
            .segments
            .iter()
            .flatten()
            .flat_map(|published| published.segment.files.iter())
            .map(|file| file.entry.clone())
        {
            entries.entry(entry.id).or_insert(entry);
        }
        snapshot.files = crate::inventory::FileInventory::from_restored_entries(
            snapshot.files.root.clone(),
            Arc::new(entries.into_values().collect()),
        )
        .map_err(StorageError::InvalidFormat)?;
    }
    externalize_posting_payloads(&mut snapshot, &mut external_objects)?;
    snapshot.stored_objects.extend(
        external_objects
            .iter()
            .map(|(digest, bytes)| model::StoredObjectDescriptor {
                kind: u16::from_le_bytes(
                    bytes[10..12]
                        .try_into()
                        .expect("generated object envelope contains a kind"),
                ) as u32,
                digest: *digest,
                length: bytes.len() as u64,
            }),
    );
    let payload = format::envelope(PAYLOAD_OBJECT_KIND, &[]);
    let payload_digest = format::sha256(&payload);
    snapshot
        .stored_objects
        .push(model::StoredObjectDescriptor::new(
            PAYLOAD_OBJECT_KIND as u32,
            &payload,
        ));
    let metadata_body = bincode_options()
        .serialize(&snapshot)
        .map_err(|error| StorageError::Io(error.to_string()))?;
    let metadata = format::envelope(METADATA_OBJECT_KIND, &metadata_body);
    let metadata_digest = format::sha256(&metadata);
    let encode_ns = encode_started.elapsed().as_nanos() as u64;

    let write_started = Instant::now();
    let objects = directory.join("objects");
    let staging = directory.join("staging");
    fs::create_dir_all(&objects)
        .map_err(|error| StorageError::Io(error.to_string()))?;
    fs::create_dir_all(&staging)
        .map_err(|error| StorageError::Io(error.to_string()))?;
    let mut objects_new = 0usize;
    write_object(&staging, &objects, payload_digest, &payload)?;
    objects_new += 1;
    for (digest, bytes) in &external_objects {
        write_object(&staging, &objects, *digest, bytes)?;
        objects_new += 1;
    }
    write_object(&staging, &objects, metadata_digest, &metadata)?;
    objects_new += 1;
    sync_directory(&objects)?;
    let write_ns = write_started.elapsed().as_nanos() as u64;

    let sync_started = Instant::now();
    let current_bytes = make_current(
        generation,
        metadata_digest,
        metadata.len() as u64,
        store_identity,
        snapshot.lineage_identity,
    );
    install_current(directory, generation, &current_bytes)?;
    sync_directory(directory)?;
    let sync_ns = sync_started.elapsed().as_nanos() as u64;

    *binding_repository
        .storage_binding
        .lock()
        .map_err(|_| StorageError::Io("storage binding lock poisoned".into()))? =
        Some(crate::storage::StorageBinding {
            directory: directory.to_path_buf(),
            generation,
            manifest_digest: metadata_digest,
            manifest_length: metadata.len() as u64,
            store_identity,
            lineage_identity: snapshot.lineage_identity,
            revision: save_revision,
            representation_generation: save_representation_generation,
            configuration_identity: save_configuration_identity,
        });

    Ok(StorageWriteReport {
        generation,
        logical_revision: IndexRevision(snapshot.logical_revision),
        unchanged: false,
        archive_report,
        objects_new,
        objects_reused: 0,
        bytes_serialized: (metadata.len() + payload.len()) as u64
            + external_objects
                .iter()
                .map(|(_, bytes)| bytes.len() as u64)
                .sum::<u64>(),
        bytes_written: (metadata.len() + payload.len()) as u64
            + external_objects
                .iter()
                .map(|(_, bytes)| bytes.len() as u64)
                .sum::<u64>(),
        sync_calls: 3,
        archive_ns,
        encode_ns,
        write_ns,
        sync_ns,
        total_ns: started.elapsed().as_nanos() as u64,
        manifest_digest: metadata_digest,
    })
}

fn externalize_posting_payloads(
    snapshot: &mut model::StorageSnapshot,
    objects: &mut Vec<([u8; 32], Vec<u8>)>,
) -> Result<(), StorageError> {
    for collection in &mut snapshot.collections {
        externalize_posting_collection(&mut collection.postings, objects)?;
        externalize_bigram(&mut collection.raw, objects)?;
        externalize_bigram(&mut collection.fallback_text, objects)?;
        externalize_sorting(&mut collection.sorting, objects)?;
        for bigram in collection.custom_text.values_mut() {
            externalize_bigram(bigram, objects)?;
        }
        if let Some(inference) = &mut collection.inference {
            for posting in inference.indexes.values_mut() {
                externalize_posting(posting, objects)?;
            }
        }
        for custom in collection.custom_postings.values_mut() {
            externalize_posting(&mut custom.index, objects)?;
        }
    }
    for group in &mut snapshot.segments {
        for published in group {
            let segment = &mut published.segment;
            externalize_posting_collection(&mut segment.postings, objects)?;
            externalize_bigram(&mut segment.raw, objects)?;
            externalize_bigram(&mut segment.fallback_text, objects)?;
            externalize_sorting(&mut segment.sorting, objects)?;
            for bigram in segment.custom_text.values_mut() {
                externalize_bigram(bigram, objects)?;
            }
            if let Some(inference) = &mut segment.inference {
                for posting in inference.indexes.values_mut() {
                    externalize_posting(posting, objects)?;
                }
            }
            for custom in segment.custom_postings.values_mut() {
                externalize_posting(&mut custom.index, objects)?;
            }
        }
    }
    Ok(())
}

fn externalize_sorting(
    sorting: &mut crate::sorting::SortCollection,
    objects: &mut Vec<([u8; 32], Vec<u8>)>,
) -> Result<(), StorageError> {
    let payload = sorting
        .archive_payload()
        .map_err(StorageError::UnsupportedComponent)?;
    let body = rkyv::to_bytes::<rkyv::rancor::Error>(&payload)
        .map_err(|error| StorageError::Io(error.to_string()))?;
    let object = format::envelope(SORT_PAYLOAD_OBJECT_KIND, &body);
    let digest = format::sha256(&object);
    let length = object.len() as u64;
    objects.push((digest, object));
    for column in sorting.columns.values_mut() {
        column.keys.clear();
        column.ascending.clear();
        column.descending.clear();
    }
    for composite in &mut sorting.composites {
        composite.order.clear();
    }
    sorting.payload = Some(crate::sorting::StoredSortPayloadRef { digest, length });
    Ok(())
}

fn externalize_bigram(
    bigram: &mut crate::bigram::BigramIndex,
    objects: &mut Vec<([u8; 32], Vec<u8>)>,
) -> Result<(), StorageError> {
    let payload = bigram
        .archive_payload()
        .map_err(StorageError::UnsupportedComponent)?;
    let body = rkyv::to_bytes::<rkyv::rancor::Error>(&payload)
        .map_err(|error| StorageError::Io(error.to_string()))?;
    let object = format::envelope(BIGRAM_PAYLOAD_OBJECT_KIND, &body);
    let digest = format::sha256(&object);
    let length = object.len() as u64;
    objects.push((digest, object));
    bigram.normal.clear();
    bigram.skip.clear();
    bigram.payload = Some(crate::bigram::StoredBigramPayloadRef { digest, length });
    Ok(())
}

fn externalize_posting_collection(
    collection: &mut crate::posting::PostingCollectionSnapshot,
    objects: &mut Vec<([u8; 32], Vec<u8>)>,
) -> Result<(), StorageError> {
    for posting in collection.fields.values_mut() {
        externalize_posting(posting, objects)?;
    }
    Ok(())
}

fn externalize_posting(
    posting: &mut crate::posting::PostingIndexSnapshot,
    objects: &mut Vec<([u8; 32], Vec<u8>)>,
) -> Result<(), StorageError> {
    if posting.values.is_empty()
        && posting.lengths.is_empty()
        && posting.structural.is_none()
    {
        return Ok(());
    }
    let payload = posting
        .archive_payload()
        .map_err(StorageError::UnsupportedComponent)?;
    let body = rkyv::to_bytes::<rkyv::rancor::Error>(&payload)
        .map_err(|error| StorageError::Io(error.to_string()))?;
    let object = format::envelope(POSTING_PAYLOAD_OBJECT_KIND, &body);
    let digest = format::sha256(&object);
    let length = object.len() as u64;
    objects.push((digest, object));
    posting.structural = None;
    posting.values.clear();
    posting.lengths.clear();
    posting.payload = Some(crate::posting::StoredPostingPayloadRef { digest, length });
    Ok(())
}

fn snapshot_from_parts(
    parts: IndexStorageParts,
    _generation: u64,
) -> Result<model::StorageSnapshot, StorageError> {
    let repository = Arc::clone(&parts.repository);
    let catalog_snapshot = parts.catalog.snapshot_slots();
    let mut file_entries = repository
        .files()
        .files
        .iter()
        .cloned()
        .map(|entry| (entry.id, entry))
        .collect::<std::collections::BTreeMap<_, _>>();
    for (entry, _, _) in catalog_snapshot.iter().filter_map(Option::as_ref) {
        file_entries.insert(entry.id, entry.clone());
    }
    let catalog_entries = file_entries.into_values().collect::<Vec<_>>();
    let files = crate::inventory::FileInventory::from_restored_entries(
        repository.files().root.clone(),
        Arc::new(catalog_entries),
    )
    .map_err(StorageError::InvalidFormat)?;
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
    let catalog_slots = catalog_snapshot
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
        files,
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
    let kind = u16::from_le_bytes(
        bytes[10..12]
            .try_into()
            .expect("storage object has a fixed envelope kind"),
    );
    let final_path = objects.join(format::object_file_name(&digest, kind));
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

fn read_current(directory: &Path) -> Result<Option<CurrentRecord>, StorageError> {
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
    if bytes[8..12] != 1u32.to_le_bytes() || bytes[12..16] != 0u32.to_le_bytes() {
        return Err(StorageError::InvalidFormat(
            "unsupported CURRENT version or flags".into(),
        ));
    }
    Ok(Some(CurrentRecord {
        generation: u64::from_le_bytes(
            bytes[16..24].try_into().expect("fixed generation bytes"),
        ),
        manifest_digest: bytes[24..56].try_into().expect("fixed manifest digest"),
        manifest_length: u64::from_le_bytes(
            bytes[56..64].try_into().expect("fixed manifest length"),
        ),
        store_identity: bytes[64..80].try_into().expect("fixed store identity"),
        lineage_identity: bytes[80..96].try_into().expect("fixed lineage identity"),
    }))
}

fn read_prior_snapshot(
    directory: &Path,
    current: Option<CurrentRecord>,
) -> Result<Option<model::StorageSnapshot>, StorageError> {
    let Some(current) = current else {
        return Ok(None);
    };
    let path = directory
        .join("objects")
        .join(format::object_file_name(&current.manifest_digest, METADATA_OBJECT_KIND));
    let bytes = fs::read(&path).map_err(|error| StorageError::Io(error.to_string()))?;
    if bytes.len() as u64 != current.manifest_length
        || format::sha256(&bytes) != current.manifest_digest
    {
        return Err(StorageError::corrupt(
            format::hex(&current.manifest_digest),
            "prior manifest length or digest mismatch",
        ));
    }
    bincode_options()
        .deserialize(format::checked_body(&bytes, METADATA_OBJECT_KIND)?)
        .map(Some)
        .map_err(|error| StorageError::InvalidFormat(error.to_string()))
}

fn acquire_writer_lock(directory: &Path) -> Result<File, StorageError> {
    use std::os::fd::AsRawFd;

    let path = directory.join("writer.lock");
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| StorageError::Io(error.to_string()))?;
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result != 0 {
        return Err(StorageError::StorageBusy(
            "another writer holds the storage lock".into(),
        ));
    }
    Ok(file)
}

fn acquire_reader_lease(directory: &Path) -> Result<File, StorageError> {
    use std::os::fd::AsRawFd;

    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(directory.join("readers.lock"))
        .map_err(|error| StorageError::Io(error.to_string()))?;
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) };
    if result != 0 {
        return Err(StorageError::StorageBusy(
            "offline storage pruning holds the backend lease".into(),
        ));
    }
    Ok(file)
}

fn random_identity() -> [u8; 16] {
    use std::io::Read;

    let mut identity = [0u8; 16];
    OpenOptions::new()
        .read(true)
        .open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut identity))
        .expect("Linux /dev/urandom supplies storage identities");
    identity
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

