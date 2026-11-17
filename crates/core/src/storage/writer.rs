use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

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
#[allow(dead_code)]
const SORT_PAYLOAD_OBJECT_KIND: u16 = 5;
const METADATA_CHUNK_OBJECT_KIND: u16 = 6;
const SORT_COLUMN_OBJECT_KIND: u16 = 10;
const SORT_ORDER_OBJECT_KIND: u16 = 11;
const RELATION_PAYLOAD_OBJECT_KIND: u16 = 8;
static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug)]
struct CurrentRecord {
    generation: u64,
    manifest_digest: [u8; 32],
}

pub(crate) fn archive_index_to(
    index: &Index,
    directory: &Path,
    options: StorageWriteOptions,
) -> Result<StorageWriteReport, IndexError> {
    let started = Instant::now();
    let archive_started = Instant::now();
    let archive_report =
        index.archive_with_options(ArchiveOptions::new(options.archive_workers.get())?)?;
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
    let archive_report =
        repository.archive_with_options(ArchiveOptions::new(options.archive_workers.get())?)?;
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
            &repository
                .collections()
                .iter()
                .map(|c| c.definition.clone())
                .collect::<Vec<_>>(),
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
    let save_configuration_identity = parts.configuration_identity;
    let binding = parts
        .repository
        .storage_binding
        .lock()
        .map_err(|_| StorageError::Io("storage binding lock poisoned".into()))?
        .get(directory)
        .cloned();
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
    let initialized_store = current.is_none();
    let store_identity = binding
        .as_ref()
        .map_or_else(random_identity, |binding| Ok(binding.store_identity))?;
    let encode_started = Instant::now();
    let mut external_objects = Vec::new();
    let source_storage = parts
        .repository
        .storage
        .as_ref()
        .map(|session| {
            let directory = session.directory().to_path_buf();
            crate::storage::reader::read_snapshot(&directory).map(|snapshot| (directory, snapshot))
        })
        .transpose()?;
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
    if let Some((_, source_snapshot)) = &source_storage {
        snapshot.stored_objects = source_snapshot.stored_objects.clone();
    }
    for (digest, bytes) in &external_objects {
        let descriptor = model::StoredObjectDescriptor {
            kind: u16::from_le_bytes(
                bytes[10..12]
                    .try_into()
                    .expect("generated object envelope contains a kind"),
            ) as u32,
            digest: *digest,
            length: bytes.len() as u64,
        };
        if !snapshot
            .stored_objects
            .iter()
            .any(|object| object.digest == descriptor.digest)
        {
            snapshot.stored_objects.push(descriptor);
        }
    }
    let mut seen_objects = std::collections::HashSet::new();
    snapshot
        .stored_objects
        .retain(|object| seen_objects.insert(object.digest));
    let payload = format::envelope(PAYLOAD_OBJECT_KIND, &[]);
    let payload_digest = format::sha256(&payload);
    snapshot
        .stored_objects
        .push(model::StoredObjectDescriptor::new(
            PAYLOAD_OBJECT_KIND as u32,
            &payload,
        ));
    let snapshot_body =
        serde_json::to_vec(&snapshot).map_err(|error| StorageError::Io(error.to_string()))?;
    let metadata_chunks = snapshot_body
        .chunks(model::METADATA_CHUNK_BYTES)
        .map(|chunk| {
            let chunk = model::MetadataChunk {
                bytes: chunk.to_vec(),
            };
            let body = rkyv::to_bytes::<rkyv::rancor::Error>(&chunk)
                .map_err(|error| StorageError::Io(error.to_string()))?;
            let object = format::envelope(METADATA_CHUNK_OBJECT_KIND, &body);
            let digest = format::sha256(&object);
            let length = object.len() as u64;
            Ok((
                digest,
                object,
                model::StoredMetadataChunk { digest, length },
            ))
        })
        .collect::<Result<Vec<_>, StorageError>>()?;
    let metadata_manifest = model::MetadataManifest {
        schema_version: model::SNAPSHOT_SCHEMA_VERSION,
        total_length: snapshot_body.len() as u64,
        chunks: metadata_chunks
            .iter()
            .map(|(_, _, descriptor)| descriptor.clone())
            .collect(),
    };
    let metadata_body = rkyv::to_bytes::<rkyv::rancor::Error>(&metadata_manifest)
        .map_err(|error| StorageError::Io(error.to_string()))?;
    let metadata = format::envelope(METADATA_OBJECT_KIND, &metadata_body);
    let metadata_digest = format::sha256(&metadata);
    let encode_ns = encode_started.elapsed().as_nanos() as u64;

    let write_started = Instant::now();
    let objects = directory.join("objects");
    let staging = directory.join("staging");
    fs::create_dir_all(&objects).map_err(|error| StorageError::Io(error.to_string()))?;
    fs::create_dir_all(&staging).map_err(|error| StorageError::Io(error.to_string()))?;
    let mut objects_new = 0usize;
    let mut objects_reused = 0usize;
    record_object_write(
        &staging,
        &objects,
        payload_digest,
        &payload,
        &mut objects_new,
        &mut objects_reused,
    )?;
    let generated_objects = external_objects
        .iter()
        .map(|(digest, bytes)| (*digest, bytes.as_slice()))
        .collect::<std::collections::HashMap<_, _>>();
    for descriptor in &snapshot.stored_objects {
        let kind = u16::try_from(descriptor.kind).map_err(|_| {
            StorageError::InvalidFormat(format!(
                "stored object kind {} does not fit the format",
                descriptor.kind
            ))
        })?;
        if let Some(bytes) = generated_objects.get(&descriptor.digest) {
            record_object_write(
                &staging,
                &objects,
                descriptor.digest,
                bytes,
                &mut objects_new,
                &mut objects_reused,
            )?;
        } else {
            let source_directory = source_storage
                .as_ref()
                .map(|(source_directory, _)| source_directory.as_path());
            install_referenced_object(
                &staging,
                &objects,
                directory,
                source_directory,
                descriptor.digest,
                descriptor.length,
                kind,
                &mut objects_new,
                &mut objects_reused,
            )?;
        }
    }
    for (digest, bytes, _) in &metadata_chunks {
        record_object_write(
            &staging,
            &objects,
            *digest,
            bytes,
            &mut objects_new,
            &mut objects_reused,
        )?;
    }
    record_object_write(
        &staging,
        &objects,
        metadata_digest,
        &metadata,
        &mut objects_new,
        &mut objects_reused,
    )?;
    sync_directory(&objects)?;
    let write_ns = write_started.elapsed().as_nanos() as u64;

    let sync_started = Instant::now();
    let mut sync_calls = 3u64;
    let current_bytes = make_current(
        generation,
        metadata_digest,
        metadata.len() as u64,
        store_identity,
        snapshot.lineage_identity,
    );
    install_current(directory, generation, &current_bytes)?;
    sync_directory(directory).map_err(|error| {
        StorageError::CommitUncertain(format!(
            "generation {generation}, manifest {}: {error}",
            format::hex(&metadata_digest)
        ))
    })?;
    if initialized_store {
        let parent = directory.parent().ok_or_else(|| {
            StorageError::CommitUncertain(
                "storage directory has no parent entry to synchronize".into(),
            )
        })?;
        sync_directory(parent).map_err(|error| {
            StorageError::CommitUncertain(format!(
                "initializing store directory for generation {generation}: {error}"
            ))
        })?;
        sync_calls = sync_calls.saturating_add(1);
    }
    let sync_ns = sync_started.elapsed().as_nanos() as u64;

    binding_repository
        .storage_binding
        .lock()
        .map_err(|_| StorageError::Io("storage binding lock poisoned".into()))?
        .insert(
            directory.to_path_buf(),
            crate::storage::StorageBinding {
                generation,
                manifest_digest: metadata_digest,
                store_identity,
                revision: save_revision,
                configuration_identity: save_configuration_identity,
            },
        );

    Ok(StorageWriteReport {
        generation,
        logical_revision: IndexRevision(snapshot.logical_revision),
        unchanged: false,
        archive_report,
        objects_new,
        objects_reused,
        bytes_serialized: (metadata.len() + payload.len()) as u64
            + external_objects
                .iter()
                .map(|(_, bytes)| bytes.len() as u64)
                .sum::<u64>()
            + metadata_chunks
                .iter()
                .map(|(_, bytes, _)| bytes.len() as u64)
                .sum::<u64>(),
        bytes_written: (metadata.len() + payload.len()) as u64
            + external_objects
                .iter()
                .map(|(_, bytes)| bytes.len() as u64)
                .sum::<u64>()
            + metadata_chunks
                .iter()
                .map(|(_, bytes, _)| bytes.len() as u64)
                .sum::<u64>(),
        sync_calls,
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
        for relation in &mut collection.relations {
            externalize_relation(relation, objects)?;
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
    if sorting.payload.is_some() {
        return Ok(());
    }
    for column in sorting.columns.values_mut() {
        if column.stored.is_none() {
            let payload = column
                .archive_payload()
                .map_err(StorageError::UnsupportedComponent)?;
            let body = rkyv::to_bytes::<rkyv::rancor::Error>(&payload)
                .map_err(|error| StorageError::Io(error.to_string()))?;
            let object = format::envelope(SORT_COLUMN_OBJECT_KIND, &body);
            let digest = format::sha256(&object);
            let length = object.len() as u64;
            objects.push((digest, object));
            column.stored = Some(crate::sorting::StoredSortColumnRef { digest, length });
        }
        column.keys.clear();
        column.ascending.clear();
        column.descending.clear();
    }
    for composite in &mut sorting.composites {
        if composite.stored.is_none() {
            let payload = composite.archive_payload();
            let body = rkyv::to_bytes::<rkyv::rancor::Error>(&payload)
                .map_err(|error| StorageError::Io(error.to_string()))?;
            let object = format::envelope(SORT_ORDER_OBJECT_KIND, &body);
            let digest = format::sha256(&object);
            let length = object.len() as u64;
            objects.push((digest, object));
            composite.stored = Some(crate::sorting::StoredSortOrderRef { digest, length });
        }
        composite.order.clear();
    }
    Ok(())
}

fn externalize_bigram(
    bigram: &mut crate::bigram::BigramIndex,
    objects: &mut Vec<([u8; 32], Vec<u8>)>,
) -> Result<(), StorageError> {
    if bigram.payload.is_some() && bigram.normal.is_empty() && bigram.skip.is_empty() {
        return Ok(());
    }
    let mut pack = format::envelope(BIGRAM_PAYLOAD_OBJECT_KIND, &[]);
    let mut normal = bigram
        .normal
        .iter()
        .map(|(key, bitmap)| Ok((*key, append_bigram_leaf(&mut pack, bitmap)?)))
        .collect::<Result<Vec<_>, StorageError>>()?;
    normal.sort_by_key(|(key, _)| *key);
    let mut skip = bigram
        .skip
        .iter()
        .map(|(key, bitmap)| Ok((*key, append_bigram_leaf(&mut pack, bitmap)?)))
        .collect::<Result<Vec<_>, StorageError>>()?;
    skip.sort_by_key(|(key, _)| *key);
    align_object(&mut pack);
    let dictionary_offset = pack.len() as u64;
    let dictionary = crate::bigram::StoredBigramDictionary {
        alphabet: match bigram.alphabet {
            crate::workload::BigramAlphabet::Ascii => 0,
            crate::workload::BigramAlphabet::Unicode => 1,
        },
        record_count: bigram.record_count,
        normal,
        skip,
    };
    let dictionary_body = rkyv::to_bytes::<rkyv::rancor::Error>(&dictionary)
        .map_err(|error| StorageError::Io(error.to_string()))?;
    pack.extend_from_slice(&dictionary_body);
    let digest = format::sha256(&pack);
    let length = pack.len() as u64;
    objects.push((digest, pack));
    bigram.normal.clear();
    bigram.skip.clear();
    bigram.payload = Some(crate::bigram::StoredBigramPayloadRef {
        digest,
        length,
        dictionary_offset,
        dictionary_length: dictionary_body.len() as u64,
        dictionary_digest: format::sha256(&dictionary_body),
    });
    Ok(())
}

fn externalize_relation(
    relation: &mut crate::relation::RelationIndex,
    objects: &mut Vec<([u8; 32], Vec<u8>)>,
) -> Result<(), StorageError> {
    if relation.stored_payload().is_some() {
        return Ok(());
    }
    let payload = relation
        .archive_payload()
        .map_err(StorageError::UnsupportedComponent)?;
    let body = rkyv::to_bytes::<rkyv::rancor::Error>(&payload)
        .map_err(|error| StorageError::Io(error.to_string()))?;
    let object = format::envelope(RELATION_PAYLOAD_OBJECT_KIND, &body);
    let digest = format::sha256(&object);
    let length = object.len() as u64;
    objects.push((digest, object));
    relation.use_stored_payload(crate::relation::StoredRelationPayloadRef { digest, length });
    Ok(())
}

fn append_bigram_leaf(
    pack: &mut Vec<u8>,
    bitmap: &roaring::RoaringBitmap,
) -> Result<crate::bigram::StoredBigramLeafRef, StorageError> {
    align_object(pack);
    let offset = pack.len() as u64;
    let start = pack.len();
    bitmap
        .serialize_into(&mut *pack)
        .map_err(|error| StorageError::Io(format!("bigram leaf serialization: {error}")))?;
    let bytes = &pack[start..];
    Ok(crate::bigram::StoredBigramLeafRef {
        offset,
        length: bytes.len() as u64,
        cardinality: bitmap.len(),
        decoded_bytes_estimate: u64::try_from(bitmap.serialized_size())
            .map_err(|_| StorageError::Io("bigram leaf size overflow".into()))?,
        digest: format::sha256(bytes),
    })
}

fn align_object(bytes: &mut Vec<u8>) {
    let aligned = (bytes.len() + 15) & !15;
    bytes.resize(aligned, 0);
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
    if posting.payload.is_some()
        && posting.values.is_empty()
        && posting.lengths.is_empty()
        && posting.structural.is_none()
    {
        return Ok(());
    }
    let mut pack = format::envelope(POSTING_PAYLOAD_OBJECT_KIND, &[]);
    let payload = posting
        .archive_payload_with(|bitmap| append_posting_leaf(&mut pack, bitmap))
        .map_err(StorageError::UnsupportedComponent)?;
    align_object(&mut pack);
    let dictionary_offset = pack.len() as u64;
    let dictionary_body = rkyv::to_bytes::<rkyv::rancor::Error>(&payload)
        .map_err(|error| StorageError::Io(error.to_string()))?;
    pack.extend_from_slice(&dictionary_body);
    let digest = format::sha256(&pack);
    let length = pack.len() as u64;
    objects.push((digest, pack));
    posting.structural = None;
    posting.values.clear();
    posting.lengths.clear();
    posting.payload = Some(crate::posting::StoredPostingPayloadRef {
        digest,
        length,
        dictionary_offset,
        dictionary_length: dictionary_body.len() as u64,
        dictionary_digest: format::sha256(&dictionary_body),
    });
    Ok(())
}

fn append_posting_leaf(
    pack: &mut Vec<u8>,
    bitmap: &roaring::RoaringBitmap,
) -> Result<crate::posting::StoredPostingLeafRef, String> {
    align_object(pack);
    let offset = pack.len() as u64;
    let start = pack.len();
    bitmap
        .serialize_into(&mut *pack)
        .map_err(|error| format!("posting leaf serialization failed: {error}"))?;
    let bytes = &pack[start..];
    Ok(crate::posting::StoredPostingLeafRef {
        offset,
        length: bytes.len() as u64,
        cardinality: bitmap.len(),
        decoded_bytes_estimate: u64::try_from(bitmap.serialized_size())
            .map_err(|_| "posting leaf size overflow".to_string())?,
        digest: format::sha256(bytes),
    })
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
            slot.map(
                |(entry, fingerprint, configuration)| model::StoredCatalogSlot {
                    entry,
                    fingerprint,
                    configuration,
                },
            )
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
        eager_leaf_rows: collections
            .iter()
            .map(collection_leaf_rows)
            .chain(
                segments
                    .iter()
                    .flatten()
                    .map(|published| segment_leaf_rows(&published.segment)),
            )
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
    snapshot.lineage_identity =
        stable_lineage(&snapshot.configuration_identity, &snapshot.source_root);
    Ok(snapshot)
}

fn posting_leaf_rows(posting: &crate::posting::PostingIndexSnapshot) -> u64 {
    let dictionary_rows = posting
        .values
        .len()
        .saturating_add(posting.lengths.len())
        .saturating_add(
            posting
                .structural
                .as_ref()
                .map_or(0, std::collections::BTreeMap::len),
        );
    (dictionary_rows as u64).saturating_add(3)
}

fn posting_collection_leaf_rows(collection: &crate::posting::PostingCollectionSnapshot) -> u64 {
    collection.fields.values().map(posting_leaf_rows).sum()
}

fn bigram_leaf_rows(index: &crate::bigram::BigramIndex) -> u64 {
    index.normal.len().saturating_add(index.skip.len()) as u64
}

fn collection_leaf_rows(collection: &model::CollectionSnapshot) -> u64 {
    let mut rows = posting_collection_leaf_rows(&collection.postings)
        .saturating_add(bigram_leaf_rows(&collection.raw))
        .saturating_add(bigram_leaf_rows(&collection.fallback_text));
    rows = rows.saturating_add(
        collection
            .custom_text
            .values()
            .map(bigram_leaf_rows)
            .sum::<u64>(),
    );
    rows = rows.saturating_add(
        collection
            .custom_postings
            .values()
            .map(|custom| posting_leaf_rows(&custom.index))
            .sum::<u64>(),
    );
    if let Some(inference) = &collection.inference {
        rows = rows.saturating_add(
            inference
                .indexes
                .values()
                .map(posting_leaf_rows)
                .sum::<u64>(),
        );
    }
    rows
}

fn segment_leaf_rows(segment: &model::SegmentSnapshot) -> u64 {
    let mut rows = posting_collection_leaf_rows(&segment.postings)
        .saturating_add(bigram_leaf_rows(&segment.raw))
        .saturating_add(bigram_leaf_rows(&segment.fallback_text));
    rows = rows.saturating_add(
        segment
            .custom_text
            .values()
            .map(bigram_leaf_rows)
            .sum::<u64>(),
    );
    rows = rows.saturating_add(
        segment
            .custom_postings
            .values()
            .map(|custom| posting_leaf_rows(&custom.index))
            .sum::<u64>(),
    );
    if let Some(inference) = &segment.inference {
        rows = rows.saturating_add(
            inference
                .indexes
                .values()
                .map(posting_leaf_rows)
                .sum::<u64>(),
        );
    }
    rows
}

fn snapshot_collection(
    collection: &CollectionState,
) -> Result<model::CollectionSnapshot, StorageError> {
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
        let mut entries =
            fs::read_dir(directory).map_err(|error| StorageError::Io(error.to_string()))?;
        if !directory.join("CURRENT").exists() && entries.next().is_some() {
            for entry in
                fs::read_dir(directory).map_err(|error| StorageError::Io(error.to_string()))?
            {
                let entry = entry.map_err(|error| StorageError::Io(error.to_string()))?;
                let name = entry.file_name();
                if !matches!(
                    name.to_str(),
                    Some("objects" | "staging" | "writer.lock" | "readers.lock")
                ) {
                    return Err(StorageError::InvalidFormat(format!(
                        "storage target is nonempty and is not an Objekte store: {}",
                        entry.path().display()
                    )));
                }
            }
        }
        return Ok(());
    }
    let parent = directory
        .parent()
        .ok_or_else(|| StorageError::Io("storage directory has no existing parent".into()))?;
    if !parent.is_dir() {
        return Err(StorageError::Io(format!(
            "storage parent is unavailable: {}",
            parent.display()
        )));
    }
    fs::create_dir(directory).map_err(|error| StorageError::Io(error.to_string()))
}

fn record_object_write(
    staging: &Path,
    objects: &Path,
    digest: [u8; 32],
    bytes: &[u8],
    objects_new: &mut usize,
    objects_reused: &mut usize,
) -> Result<(), StorageError> {
    if write_object(staging, objects, digest, bytes)? {
        *objects_new = (*objects_new).saturating_add(1);
    } else {
        *objects_reused = (*objects_reused).saturating_add(1);
    }
    Ok(())
}

fn install_referenced_object(
    staging: &Path,
    objects: &Path,
    target_directory: &Path,
    source_directory: Option<&Path>,
    digest: [u8; 32],
    length: u64,
    kind: u16,
    objects_new: &mut usize,
    objects_reused: &mut usize,
) -> Result<(), StorageError> {
    let final_path = objects.join(format::object_file_name(&digest, kind));
    match fs::metadata(&final_path) {
        Ok(metadata) if metadata.is_file() && metadata.len() == length => {
            *objects_reused = (*objects_reused).saturating_add(1);
            return Ok(());
        }
        Ok(_) => {
            return Err(StorageError::corrupt(
                format::hex(&digest),
                "existing content-addressed object length differs",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(StorageError::Io(error.to_string())),
    }
    let source_directory = source_directory.ok_or(StorageError::MissingObject(digest))?;
    if source_directory == target_directory {
        return Err(StorageError::MissingObject(digest));
    }
    let source_path = source_directory
        .join("objects")
        .join(format::object_file_name(&digest, kind));
    let bytes = fs::read(&source_path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => StorageError::MissingObject(digest),
        _ => StorageError::Io(format!("{}: {error}", source_path.display())),
    })?;
    if bytes.len() as u64 != length || format::sha256(&bytes) != digest {
        return Err(StorageError::corrupt(
            format::hex(&digest),
            "source object length or digest mismatch during export",
        ));
    }
    if write_object(staging, objects, digest, &bytes)? {
        *objects_new = (*objects_new).saturating_add(1);
    } else {
        *objects_reused = (*objects_reused).saturating_add(1);
    }
    Ok(())
}

fn write_object(
    staging: &Path,
    objects: &Path,
    digest: [u8; 32],
    bytes: &[u8],
) -> Result<bool, StorageError> {
    let final_path = objects.join(format::object_file_name(
        &digest,
        u16::from_le_bytes(
            bytes[10..12]
                .try_into()
                .expect("storage object has a fixed envelope kind"),
        ),
    ));
    match fs::metadata(&final_path) {
        Ok(metadata) if metadata.is_file() && metadata.len() == bytes.len() as u64 => {
            return Ok(false);
        }
        Ok(_) => {
            return Err(StorageError::Corrupt {
                identity: format::hex(&digest),
                message: "existing content-addressed object length differs".into(),
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(StorageError::Io(error.to_string())),
    }
    let (staging_path, mut file) =
        create_staging_file(staging, &format!("object-{}", format::hex(&digest)))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| StorageError::Io(error.to_string()))?;
    match fs::hard_link(&staging_path, &final_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata =
                fs::metadata(&final_path).map_err(|error| StorageError::Io(error.to_string()))?;
            if !metadata.is_file() || metadata.len() != bytes.len() as u64 {
                return Err(StorageError::Corrupt {
                    identity: format::hex(&digest),
                    message: "concurrent content-addressed object differs".into(),
                });
            }
            fs::remove_file(&staging_path).map_err(|error| StorageError::Io(error.to_string()))?;
            return Ok(false);
        }
        Err(error) => return Err(StorageError::Io(error.to_string())),
    }
    fs::remove_file(&staging_path).map_err(|error| StorageError::Io(error.to_string()))?;
    File::open(&final_path)
        .and_then(|file| file.sync_all())
        .map_err(|error| StorageError::Io(error.to_string()))?;
    Ok(true)
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
    let (temporary, mut file) =
        create_staging_file(&directory.join("staging"), &format!("CURRENT-{generation}"))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| StorageError::Io(error.to_string()))?;
    fs::rename(&temporary, directory.join("CURRENT"))
        .map_err(|error| StorageError::Io(error.to_string()))
}

fn create_staging_file(directory: &Path, label: &str) -> Result<(PathBuf, File), StorageError> {
    for _ in 0..32 {
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = directory.join(format!("{label}-{}-{sequence}.tmp", std::process::id()));
        match OpenOptions::new().create_new(true).write(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(StorageError::Io(error.to_string())),
        }
    }
    Err(StorageError::StorageBusy(
        "could not allocate a unique storage staging file".into(),
    ))
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
        return Err(StorageError::InvalidFormat(
            "CURRENT checksum mismatch".into(),
        ));
    }
    if bytes[8..12] != 1u32.to_le_bytes() || bytes[12..16] != 0u32.to_le_bytes() {
        return Err(StorageError::InvalidFormat(
            "unsupported CURRENT version or flags".into(),
        ));
    }
    Ok(Some(CurrentRecord {
        generation: u64::from_le_bytes(bytes[16..24].try_into().expect("fixed generation bytes")),
        manifest_digest: bytes[24..56].try_into().expect("fixed manifest digest"),
    }))
}

fn read_prior_snapshot(
    directory: &Path,
    current: Option<CurrentRecord>,
) -> Result<Option<model::StorageSnapshot>, StorageError> {
    if current.is_none() {
        return Ok(None);
    }
    crate::storage::reader::read_snapshot(directory).map(Some)
}

fn acquire_writer_lock(directory: &Path) -> Result<File, StorageError> {
    let path = directory.join("writer.lock");
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .map_err(|error| StorageError::Io(error.to_string()))?;
    file.try_lock()
        .map_err(|_| StorageError::StorageBusy("another writer holds the storage lock".into()))?;
    Ok(file)
}

fn acquire_reader_lease(directory: &Path) -> Result<File, StorageError> {
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(directory.join("readers.lock"))
        .map_err(|error| StorageError::Io(error.to_string()))?;
    file.try_lock_shared().map_err(|_| {
        StorageError::StorageBusy("offline storage pruning holds the backend lease".into())
    })?;
    Ok(file)
}

fn random_identity() -> Result<[u8; 16], StorageError> {
    use std::io::Read;

    let mut identity = [0u8; 16];
    OpenOptions::new()
        .read(true)
        .open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut identity))
        .map_err(|error| StorageError::Io(format!("/dev/urandom: {error}")))?;
    Ok(identity)
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
