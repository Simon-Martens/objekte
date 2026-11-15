//! Two-process feasibility input for the A2 campaign.
//!
//! The producer may use the native repository. It emits a versioned,
//! deterministic message stream of configuration, files, records, reverse
//! payloads and per-record gram lists. The consumer is a fresh process that
//! never constructs a native repository and imports the stream into an empty
//! in-memory redb store.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use redb::backends::InMemoryBackend;
use redb::StorageBackend;
use serde::{Deserialize, Serialize};

use crate::index_store::states::StateBatch;
use crate::index_store::value_codec::{
    BindingDescriptor, BindingIdentity, BindingRole, BuildFileState, CollectionMetadata,
    ContributionPayload, DomainKind, FieldDescriptor, FileBuildState, GramKind,
    RecordMetadata, RelationOccurrence, SortConstituent, SortDefinition, SortDirection,
    SortProjection, StoredBinding, StoredContributions,
};
use crate::index_store::{
    IndexStore, StoreConfig, fts::FtsBatch, posting_blocks::PostingBatch, read, write,
};
use crate::inventory::FileEntry;
use crate::model::{CollectionId, LogicalType, RecordSlot};
use crate::repository::{CollectionState, Repository};

pub const INPUT_VERSION: u32 = 1;
const MAX_MESSAGE_BYTES: usize = 512 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct InputManifest {
    pub version: u32,
    pub corpus_digest: [u8; 32],
    pub configuration_digest: [u8; 32],
    pub source_root: Vec<u8>,
    pub collections: Vec<CollectionManifest>,
    pub records: u64,
    pub files: u64,
    pub value_members: u64,
    pub length_members: u64,
    pub state_facts: u64,
    pub gram_memberships: u64,
    pub sort_entries: u64,
    pub relation_occurrences: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct CollectionManifest {
    pub id: u32,
    pub name: String,
    pub records: u64,
    pub files: u64,
    pub value_members: u64,
    pub length_members: u64,
    pub state_facts: u64,
    pub gram_memberships: u64,
    pub sort_entries: u64,
    pub relation_occurrences: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CollectionInput {
    pub metadata: CollectionMetadata,
    pub bindings: Vec<(Vec<u8>, StoredBinding)>,
    pub fields: Vec<(Vec<String>, FieldDescriptor)>,
    pub sort_definitions: Vec<SortDefinition>,
    pub file_versions: Vec<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum InputMessage {
    Start(InputManifest),
    Collection(CollectionInput),
    File(FileEntry, u64, Option<[u8; 32]>),
    Record {
        collection: u32,
        slot: u32,
        payload: ContributionPayload,
    },
    Gram {
        collection: u32,
        slot: u32,
        binding: u32,
        kind: u8,
        grams: Vec<u64>,
    },
    CollectionEnd {
        collection: u32,
        metadata: CollectionMetadata,
    },
    Totals(InputManifest),
    End,
}

pub fn write_message(writer: &mut impl Write, message: &InputMessage) -> Result<(), String> {
    let bytes = bincode::serialize(message).map_err(|error| error.to_string())?;
    let len = u32::try_from(bytes.len()).map_err(|_| "input message too large")?;
    writer
        .write_all(&len.to_le_bytes())
        .and_then(|_| writer.write_all(&bytes))
        .map_err(|error| error.to_string())
}

pub fn read_message(reader: &mut impl Read) -> Result<Option<InputMessage>, String> {
    let mut len = [0u8; 4];
    match reader.read_exact(&mut len) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.to_string()),
    }
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_MESSAGE_BYTES {
        return Err("input message exceeds the supported bound".to_string());
    }
    let mut bytes = vec![0u8; len];
    reader.read_exact(&mut bytes).map_err(|error| error.to_string())?;
    bincode::deserialize(&bytes)
        .map(Some)
        .map_err(|error| format!("malformed input message: {error}"))
}

/// Producer: walk an already-built native repository and write the complete
/// deterministic input stream. This function is never used by the consumer.
pub fn prepare(repository: &Repository, output: &Path) -> Result<InputManifest, String> {
    let file = File::create(output).map_err(|error| error.to_string())?;
    let mut writer = BufWriter::with_capacity(1 << 20, file);
    let mut manifest = InputManifest {
        version: INPUT_VERSION,
        corpus_digest: corpus_digest(repository),
        configuration_digest: [0; 32],
        source_root: repository
            .files()
            .root
            .to_string_lossy()
            .into_owned()
            .into_bytes(),
        collections: Vec::new(),
        records: 0,
        files: 0,
        value_members: 0,
        length_members: 0,
        state_facts: 0,
        gram_memberships: 0,
        sort_entries: 0,
        relation_occurrences: 0,
    };
    let definitions = repository
        .collections()
        .iter()
        .map(|collection| collection.definition.clone())
        .collect::<Vec<_>>();
    manifest.configuration_digest = crate::configuration::configuration_hash(
        &definitions,
        repository.options(),
        &repository.sources,
    )
    .map_err(|error| error.to_string())?;
    write_message(&mut writer, &InputMessage::Start(manifest.clone()))?;
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
    for (index, collection) in repository.collections().iter().enumerate() {
        let collection_id =
            CollectionId(u32::try_from(index).expect("collection count fits u32"));
        let collection_manifest = prepare_collection(
            &mut writer,
            repository,
            collection,
            collection_id,
            &collection_ids,
        )?;
        manifest.records += collection_manifest.records;
        manifest.files += collection_manifest.files;
        manifest.value_members += collection_manifest.value_members;
        manifest.length_members += collection_manifest.length_members;
        manifest.state_facts += collection_manifest.state_facts;
        manifest.gram_memberships += collection_manifest.gram_memberships;
        manifest.sort_entries += collection_manifest.sort_entries;
        manifest.relation_occurrences += collection_manifest.relation_occurrences;
        manifest.collections.push(collection_manifest);
    }
    write_message(&mut writer, &InputMessage::Totals(manifest.clone()))?;
    write_message(&mut writer, &InputMessage::End)?;
    writer.flush().map_err(|error| error.to_string())?;
    Ok(manifest)
}

fn prepare_collection(
    writer: &mut impl Write,
    repository: &Repository,
    collection: &CollectionState,
    collection_id: CollectionId,
    collection_ids: &HashMap<String, CollectionId>,
) -> Result<CollectionManifest, String> {
    let record_count = collection.generation.records.len();
    let mut bindings = ProducerBindings::default();
    let mut fields = Vec::new();
    let mut definitions = Vec::new();
    let mut sort_definition = 0u32;
    for (path, column) in &collection.sorting.columns {
        for direction in [SortDirection::Ascending, SortDirection::Descending] {
            definitions.push(SortDefinition {
                id: sort_definition,
                constituents: vec![SortConstituent {
                    key: path.clone(),
                    direction,
                    options: column.options,
                }],
                locale: column.locale.clone(),
                collation_version: column.collation_version.clone(),
            });
            sort_definition += 1;
        }
    }
    let mut values_by_slot: Vec<Vec<(u32, crate::posting::ScalarKey)>> =
        vec![Vec::new(); record_count];
    let mut lengths_by_slot: Vec<Vec<(u32, crate::model::Number)>> =
        vec![Vec::new(); record_count];
    let mut state_masks_by_slot: Vec<std::collections::BTreeMap<u32, u16>> =
        vec![std::collections::BTreeMap::new(); record_count];
    let mut sorts_by_slot: Vec<Vec<SortProjection>> = vec![Vec::new(); record_count];
    let mut relations_by_slot: Vec<Vec<RelationOccurrence>> = vec![Vec::new(); record_count];
    let mut value_members = 0u64;
    let mut length_members = 0u64;
    let mut state_facts = 0u64;
    for (path, posting) in &collection.postings.fields {
        let binding = bindings.allocate(
            BindingIdentity::Declared { path: path.clone() },
            BindingRole::Structured,
            posting.kind.clone(),
            posting.multi,
            None,
        );
        fields.push((
            path.clone(),
            FieldDescriptor {
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
        ));
        for (value, slots) in &posting.values {
            for slot in slots.iter() {
                values_by_slot[slot as usize].push((binding, value.clone()));
                value_members += 1;
            }
        }
        for (length, slots) in &posting.lengths {
            for slot in slots.iter() {
                lengths_by_slot[slot as usize].push((binding, length.clone()));
                length_members += 1;
            }
        }
        for slot in posting.presence.iter() {
            set_state(&mut state_masks_by_slot, binding, 0, slot);
            state_facts += 1;
        }
        for slot in posting.empty_selections.iter() {
            set_state(&mut state_masks_by_slot, binding, 1, slot);
            state_facts += 1;
        }
        for slot in posting.nulls.iter() {
            set_state(&mut state_masks_by_slot, binding, 2, slot);
            state_facts += 1;
        }
        if let Some(structural) = &posting.structural {
            for (key, slots) in &structural.postings {
                let tag = match key {
                    crate::posting::StructuralKey::AnyNull => 3,
                    crate::posting::StructuralKey::NonEmpty => 4,
                    crate::posting::StructuralKey::String => 5,
                    crate::posting::StructuralKey::Number => 6,
                    crate::posting::StructuralKey::Array => 7,
                    crate::posting::StructuralKey::Object => 8,
                    crate::posting::StructuralKey::EmptyObject => 9,
                };
                for slot in slots.iter() {
                    set_state(&mut state_masks_by_slot, binding, tag, slot);
                    state_facts += 1;
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
        );
        for (value, slots) in &custom.index.values {
            for slot in slots.iter() {
                values_by_slot[slot as usize].push((binding, value.clone()));
                value_members += 1;
            }
        }
    }
    let mut text_bindings = Vec::new();
    text_bindings.push((
        bindings.allocate(
            BindingIdentity::RawText,
            BindingRole::Text,
            LogicalType::String,
            false,
            Some(format!("{:?}-raw", collection.raw.alphabet)),
        ),
        "raw",
        &collection.raw,
    ));
    text_bindings.push((
        bindings.allocate(
            BindingIdentity::FallbackText,
            BindingRole::Text,
            LogicalType::String,
            false,
            Some(format!("{:?}-fallback", collection.fallback_text.alphabet)),
        ),
        "fallback",
        &collection.fallback_text,
    ));
    for ((path, name), index) in &collection.custom_text {
        text_bindings.push((
            bindings.allocate(
                BindingIdentity::CustomText {
                    path: path.clone(),
                    name: name.clone(),
                },
                BindingRole::Text,
                LogicalType::String,
                false,
                Some(format!("{:?}-custom", index.alphabet)),
            ),
            "custom",
            index,
        ));
    }
    let mut sort_slot = 0u32;
    let mut sort_entries = 0u64;
    for column in collection.sorting.columns.values() {
        for _ in [SortDirection::Ascending, SortDirection::Descending] {
            let current = sort_slot;
            sort_slot += 1;
            for (slot, key) in column.keys.iter().enumerate() {
                sorts_by_slot[slot].push(SortProjection {
                    definition: current,
                    key: vec![key.clone()],
                });
                sort_entries += 1;
            }
        }
    }
    let mut relation_occurrences = 0u64;
    for (declaration, relation) in collection.relations.iter().enumerate() {
        let declaration = u32::try_from(declaration).expect("relation ordinal fits u32");
        let target_collection = collection_ids
            .get(&relation.declaration.target_collection)
            .copied()
            .unwrap_or(CollectionId(u32::MAX));
        for source in 0..record_count {
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
                relation_occurrences += 1;
            }
        }
    }
    let mut metadata = CollectionMetadata {
        id: collection_id,
        name: collection.definition.name.clone(),
        generation: collection.generation.generation.max(1),
        source_fingerprint: [0; 32],
        slot_high_water: record_count as u64,
        live_records: record_count as u64,
        discovered_files: collection.generation.files.len() as u64,
        pending_files: 0,
        indexed_files: collection.generation.files.len() as u64,
        unavailable_files: 0,
        processed_files: collection.generation.files.len() as u64,
        next_binding_id: bindings.next,
    };
    metadata.live_records = record_count as u64;
    let file_versions = vec![1u64; collection.generation.files.len()];
    write_message(
        writer,
        &InputMessage::Collection(CollectionInput {
            metadata: metadata.clone(),
            bindings: bindings.list,
            fields,
            sort_definitions: definitions.clone(),
            file_versions: file_versions.clone(),
        }),
    )?;
    for file_id in &collection.generation.files {
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
        write_message(writer, &InputMessage::File(entry.clone(), 1, fingerprint))?;
    }
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
            .collect();
        let mut sorts = std::mem::take(&mut sorts_by_slot[slot]);
        sorts.sort_by_key(|projection| projection.definition);
        let relations = std::mem::take(&mut relations_by_slot[slot]);
        let mut domains = Vec::new();
        if collection.logical_records.contains(slot as u32) {
            domains.push(DomainKind::Logical);
        }
        if matches!(entry.class, crate::inventory::RecordClass::RawFallback) {
            domains.push(DomainKind::RawFallback);
        }
        if collection.empty_array_records.contains(slot as u32) {
            domains.push(DomainKind::EmptyArray);
        }
        let payload = ContributionPayload {
            record: RecordMetadata::from_entry(entry, 1),
            values,
            lengths,
            state_masks,
            coverage: Vec::new(),
            domains,
            sorts,
            relations,
        };
        write_message(
            writer,
            &InputMessage::Record {
                collection: collection_id.0,
                slot: slot as u32,
                payload,
            },
        )?;
    }
    let mut gram_memberships = 0u64;
    for (binding, _label, index) in &text_bindings {
        for (kind, map) in [
            (GramKind::Normal, &index.normal),
            (GramKind::Skip, &index.skip),
        ] {
            for start in (0..record_count).step_by(4096) {
                let end = (start + 4096).min(record_count);
                let mut lists: Vec<Vec<u64>> = vec![Vec::new(); end - start];
                for (gram, slots) in map.iter() {
                    for slot in slots.range(start as u32..end as u32) {
                        lists[(slot as usize) - start].push(*gram);
                    }
                }
                for (offset, grams) in lists.into_iter().enumerate() {
                    if grams.is_empty() {
                        continue;
                    }
                    let mut grams = grams;
                    grams.sort_unstable();
                    grams.dedup();
                    gram_memberships += grams.len() as u64;
                    write_message(
                        writer,
                        &InputMessage::Gram {
                            collection: collection_id.0,
                            slot: (start + offset) as u32,
                            binding: *binding,
                            kind: kind.tag(),
                            grams,
                        },
                    )?;
                }
            }
        }
    }
    write_message(
        writer,
        &InputMessage::CollectionEnd {
            collection: collection_id.0,
            metadata,
        },
    )?;
    Ok(CollectionManifest {
        id: collection_id.0,
        name: collection.definition.name.clone(),
        records: record_count as u64,
        files: collection.generation.files.len() as u64,
        value_members,
        length_members,
        state_facts,
        gram_memberships,
        sort_entries,
        relation_occurrences,
    })
}

fn set_state(
    masks: &mut [std::collections::BTreeMap<u32, u16>],
    binding: u32,
    tag: u8,
    slot: u32,
) {
    if let Some(entry) = masks.get_mut(slot as usize) {
        *entry.entry(binding).or_insert(0) |= 1u16 << tag;
    }
}

#[derive(Default)]
struct ProducerBindings {
    next: u32,
    list: Vec<(Vec<u8>, StoredBinding)>,
}

impl ProducerBindings {
    fn allocate(
        &mut self,
        identity: BindingIdentity,
        role: BindingRole,
        kind: LogicalType,
        multi: bool,
        text_alphabet: Option<String>,
    ) -> u32 {
        let id = self.next;
        self.next += 1;
        let encoded = bincode::serialize(&identity).expect("binding identity");
        self.list.push((
            encoded,
            StoredBinding {
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
        ));
        id
    }
}

fn corpus_digest(repository: &Repository) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    for file in &repository.files().files {
        hash.update(file.id.0.to_le_bytes());
        hash.update(crate::path_repr::native_bytes(file.relative_path.as_path()));
        hash.update(file.revision.size.to_le_bytes());
        hash.update(file.revision.modified_ns.to_le_bytes());
    }
    hash.finalize().into()
}


#[derive(Debug)]
pub struct CountingBackend {
    inner: InMemoryBackend,
    current: Arc<AtomicU64>,
    peak: Arc<AtomicU64>,
}

impl CountingBackend {
    pub fn new() -> (Self, Arc<AtomicU64>, Arc<AtomicU64>) {
        let current = Arc::new(AtomicU64::new(0));
        let peak = Arc::new(AtomicU64::new(0));
        (
            Self {
                inner: InMemoryBackend::new(),
                current: Arc::clone(&current),
                peak: Arc::clone(&peak),
            },
            current,
            peak,
        )
    }
}

impl StorageBackend for CountingBackend {
    fn len(&self) -> Result<u64, std::io::Error> {
        self.inner.len()
    }

    fn read(&self, offset: u64, out: &mut [u8]) -> Result<(), std::io::Error> {
        self.inner.read(offset, out)
    }

    fn set_len(&self, len: u64) -> Result<(), std::io::Error> {
        self.inner.set_len(len)?;
        self.current.store(len, Ordering::Relaxed);
        self.peak.fetch_max(len, Ordering::Relaxed);
        Ok(())
    }

    fn sync_data(&self) -> Result<(), std::io::Error> {
        self.inner.sync_data()
    }

    fn write(&self, offset: u64, data: &[u8]) -> Result<(), std::io::Error> {
        self.inner.write(offset, data)
    }
}

/// Consumer: import a complete input stream into a fresh in-memory store.
/// One write transaction covers a whole collection; this process never
/// constructs a native repository.
pub fn import(
    input: &Path,
    mut config: StoreConfig,
) -> Result<(Arc<IndexStore>, ImportReport), String> {
    let file = File::open(input).map_err(|error| error.to_string())?;
    let mut reader = BufReader::with_capacity(1 << 20, file);
    let started = Instant::now();
    let Some(InputMessage::Start(manifest)) = read_message(&mut reader)? else {
        return Err("input stream does not start with a manifest".into());
    };
    if manifest.version != INPUT_VERSION {
        return Err(format!(
            "input version {} is not supported",
            manifest.version
        ));
    }
    config.configuration_digest = manifest.configuration_digest;
    config.source_root = manifest.source_root.clone();
    let (backend, backend_current, backend_peak) = CountingBackend::new();
    let store = Arc::new(
        IndexStore::with_backend(config, backend).map_err(|error| error.to_string())?,
    );
    let metadata = store.metadata().map_err(|error| error.to_string())?;
    let mut report = ImportReport {
        manifest: manifest.clone(),
        ..Default::default()
    };
    let mut tx: Option<redb::WriteTransaction> = None;
    let mut state_batch = StateBatch::new();
    let mut posting_batch = PostingBatch::new();
    let mut fts_batch = FtsBatch::new();
    let mut gram_rows: HashMap<(u32, u32, u32, u8), Vec<u64>> = HashMap::new();
    let mut records = 0u64;
    let mut pending = 0u64;
    let batch_size = std::env::var("A2_BATCH")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(4_096);
    loop {
        let Some(message) = read_message(&mut reader)? else {
            break;
        };
        match message {
            InputMessage::Start(_) => return Err("duplicate input manifest".into()),
            InputMessage::Collection(input) => {
                if tx.is_some() {
                    return Err("nested collection input".into());
                }
                let write_tx = store.begin_write().map_err(|error| error.to_string())?;
                let mut collection = input.metadata.clone();
                collection.live_records = 0;
                write::put_collection(&write_tx, &collection)
                    .map_err(|error| error.to_string())?;
                for (identity, binding) in &input.bindings {
                    write::put_binding(&write_tx, collection.id, identity, binding)
                        .map_err(|error| error.to_string())?;
                }
                for (path, field) in &input.fields {
                    write::put_field(&write_tx, collection.id, path, field)
                        .map_err(|error| error.to_string())?;
                }
                for definition in &input.sort_definitions {
                    write::put_sort_definition(&write_tx, collection.id, definition)
                        .map_err(|error| error.to_string())?;
                }
                tx = Some(write_tx);
                records = 0;
                state_batch = StateBatch::new();
                posting_batch = PostingBatch::new();
                fts_batch = FtsBatch::new();
                gram_rows = HashMap::new();
            }
            InputMessage::File(entry, version, fingerprint) => {
                let write_tx = tx.as_ref().ok_or("file outside a collection")?;
                write::put_file(write_tx, &entry, version, fingerprint)
                    .map_err(|error| error.to_string())?;
                write::put_build_state(
                    write_tx,
                    entry.id,
                    &FileBuildState {
                        state: BuildFileState::Indexed,
                        expected_file_version: version,
                        build_epoch: metadata.build_epoch,
                        last_admission_revision: metadata.revision,
                        error: None,
                    },
                )
                .map_err(|error| error.to_string())?;
                report.files += 1;
            }
            InputMessage::Record {
                collection,
                slot,
                payload,
            } => {
                let write_tx = tx.as_ref().ok_or("record outside a collection")?;
                let collection = CollectionId(collection);
                let slot = RecordSlot(slot);
                write::put_record(write_tx, collection, slot, &payload.record)
                    .map_err(|error| error.to_string())?;
                write::put_logical_id(
                    write_tx,
                    collection,
                    &payload.record.id,
                    slot,
                    payload.record.incarnation,
                )
                .map_err(|error| error.to_string())?;
                write::put_file_record(write_tx, payload.record.file, collection, slot)
                    .map_err(|error| error.to_string())?;
                for (binding, mask) in &payload.state_masks {
                    for tag in 0..=crate::index_store::tables::MAX_STATE_TAG {
                        if mask & (1u16 << tag) != 0 {
                            if let Some(state) =
                                crate::index_store::value_codec::StateKind::from_tag(tag)
                            {
                                state_batch.set(*binding, state, slot.0);
                                report.state_facts += 1;
                            }
                        }
                    }
                }
                let contribution = StoredContributions {
                    record: payload.record.clone(),
                    values: payload.values.clone(),
                    lengths: payload.lengths.clone(),
                    state_masks: payload.state_masks.clone(),
                    coverage: payload.coverage.clone(),
                    domains: payload.domains.clone(),
                    sorts: payload.sorts.clone(),
                    relations: payload.relations.clone(),
                    grams: Vec::new(),
                };
                #[cfg(not(feature = "store-posting-rows"))]
                {
                    for (binding, value) in &payload.values {
                        posting_batch.set(
                            crate::index_store::posting_blocks::PostingFamily::Value,
                            *binding,
                            value,
                            slot,
                        );
                    }
                    for (binding, length) in &payload.lengths {
                        posting_batch.set(
                            crate::index_store::posting_blocks::PostingFamily::Length,
                            *binding,
                            &crate::posting::ScalarKey::Number(length.clone()),
                            slot,
                        );
                    }
                }
                #[cfg(feature = "store-posting-rows")]
                {
                    for (binding, value) in &payload.values {
                        write::insert_value_member(write_tx, collection, *binding, value, slot)
                            .map_err(|error| error.to_string())?;
                    }
                    for (binding, length) in &payload.lengths {
                        write::insert_length_member(
                            write_tx,
                            collection,
                            *binding,
                            &crate::posting::ScalarKey::Number(length.clone()),
                            slot,
                        )
                        .map_err(|error| error.to_string())?;
                    }
                }
                write::insert_metadata_memberships(write_tx, collection, slot, &contribution)
                    .map_err(|error| error.to_string())?;
                let definitions = write::sort_definitions(write_tx, collection)
                    .map_err(|error| error.to_string())?;
                crate::index_store::sorting::insert_sort_projection(
                    write_tx,
                    collection,
                    slot,
                    &payload.record.id.0,
                    &payload.sorts,
                    &definitions,
                )
                .map_err(|error| error.to_string())?;
                crate::index_store::relations::insert_relation_occurrences(
                    write_tx,
                    collection,
                    slot,
                    &payload.relations,
                )
                .map_err(|error| error.to_string())?;
                write::put_contributions(write_tx, collection, slot, &contribution)
                    .map_err(|error| error.to_string())?;
                report.records += 1;
                records += 1;
                report.value_members += payload.values.len() as u64;
                report.length_members += payload.lengths.len() as u64;
                report.sort_entries += payload.sorts.len() as u64;
                report.relation_occurrences += payload.relations.len() as u64;
                pending += 1;
                if pending >= batch_size {
                    flush_input_batch(
                        &store,
                        &mut tx,
                        collection,
                        &mut state_batch,
                        &mut posting_batch,
                        &mut fts_batch,
                        &mut gram_rows,
                    )?;
                    pending = 0;
                }
            }
            InputMessage::Gram {
                collection,
                slot,
                binding,
                kind,
                grams,
            } => {
                tx.as_ref().ok_or("gram outside a collection")?;
                let kind = if kind == crate::index_store::tables::GRAM_SKIP {
                    GramKind::Skip
                } else {
                    GramKind::Normal
                };
                for gram in &grams {
                    fts_batch.set(binding, kind, *gram, slot);
                }
                gram_rows
                    .entry((collection, slot, binding, kind.tag()))
                    .or_default()
                    .extend_from_slice(&grams);
                report.gram_memberships += grams.len() as u64;
                pending += 1;
                if pending >= batch_size {
                    flush_input_batch(
                        &store,
                        &mut tx,
                        CollectionId(collection),
                        &mut state_batch,
                        &mut posting_batch,
                        &mut fts_batch,
                        &mut gram_rows,
                    )?;
                    pending = 0;
                }
            }
            InputMessage::CollectionEnd { collection, metadata } => {
                flush_input_batch(
                    &store,
                    &mut tx,
                    CollectionId(collection),
                    &mut state_batch,
                    &mut posting_batch,
                    &mut fts_batch,
                    &mut gram_rows,
                )?;
                let write_tx = tx.take().ok_or("collection end without a collection")?;
                let mut metadata = metadata;
                metadata.live_records = records;
                write::put_collection(&write_tx, &metadata)
                    .map_err(|error| error.to_string())?;
                write_tx.commit().map_err(|error| error.to_string())?;
                report.collections += 1;
            }
            InputMessage::Totals(totals) => {
                report.manifest = totals;
            }
            InputMessage::End => break,
        }
    }
    if tx.is_some() {
        return Err("input ended inside a collection".into());
    }
    let write_tx = store.begin_write().map_err(|error| error.to_string())?;
    let mut metadata = read::metadata_from_write(&write_tx).map_err(|error| error.to_string())?;
    metadata.construction = crate::index_store::value_codec::ConstructionState::Complete;
    metadata.catalog_complete = true;
    metadata.revision = metadata.revision.saturating_add(1);
    write::put_metadata(&write_tx, &metadata).map_err(|error| error.to_string())?;
    write_tx.commit().map_err(|error| error.to_string())?;
    report.import_ns = started.elapsed().as_nanos();
    report.backend_len_bytes = backend_current.load(Ordering::Relaxed);
    report.backend_peak_bytes = backend_peak.load(Ordering::Relaxed);
    Ok((store, report))
}

#[allow(clippy::too_many_arguments)]
fn flush_input_batch(
    store: &Arc<IndexStore>,
    tx: &mut Option<redb::WriteTransaction>,
    collection: CollectionId,
    state_batch: &mut StateBatch,
    posting_batch: &mut PostingBatch,
    fts_batch: &mut FtsBatch,
    gram_rows: &mut HashMap<(u32, u32, u32, u8), Vec<u64>>,
) -> Result<(), String> {
    let Some(write_tx) = tx.take() else {
        return Err("batch outside a collection".into());
    };
    #[cfg(not(feature = "store-posting-rows"))]
    crate::index_store::posting_blocks::apply_batch(&write_tx, collection, posting_batch)
        .map_err(|error| error.to_string())?;
    crate::index_store::states::apply_batch(&write_tx, collection, state_batch)
        .map_err(|error| error.to_string())?;
    crate::index_store::fts::apply_batch(&write_tx, collection, fts_batch)
        .map_err(|error| error.to_string())?;
    {
        let mut table = write_tx
            .open_table(crate::index_store::tables::RECORD_GRAMS)
            .map_err(|error| error.to_string())?;
        for ((collection, slot, binding, kind), mut grams) in gram_rows.drain() {
            grams.sort_unstable();
            grams.dedup();
            let key = crate::index_store::keys::record_grams(
                collection,
                slot,
                binding,
                if kind == crate::index_store::tables::GRAM_SKIP {
                    GramKind::Skip
                } else {
                    GramKind::Normal
                },
            );
            let encoded = crate::index_store::value_codec::encode_gram_list(&grams);
            table
                .insert(key.as_slice(), encoded.as_slice())
                .map_err(|error| error.to_string())?;
        }
    }
    write_tx.commit().map_err(|error| error.to_string())?;
    *state_batch = StateBatch::new();
    *posting_batch = PostingBatch::new();
    *fts_batch = FtsBatch::new();
    *tx = Some(store.begin_write().map_err(|error| error.to_string())?);
    Ok(())
}

#[derive(Clone, Debug)]
pub struct ImportReport {
    pub manifest: InputManifest,
    pub collections: u64,
    pub files: u64,
    pub records: u64,
    pub value_members: u64,
    pub length_members: u64,
    pub state_facts: u64,
    pub gram_memberships: u64,
    pub sort_entries: u64,
    pub relation_occurrences: u64,
    pub import_ns: u128,
    pub backend_len_bytes: u64,
    pub backend_peak_bytes: u64,
}

impl Default for ImportReport {
    fn default() -> Self {
        Self {
            manifest: InputManifest {
                version: INPUT_VERSION,
                corpus_digest: [0; 32],
                configuration_digest: [0; 32],
                source_root: Vec::new(),
                collections: Vec::new(),
                records: 0,
                files: 0,
                value_members: 0,
                length_members: 0,
                state_facts: 0,
                gram_memberships: 0,
                sort_entries: 0,
                relation_occurrences: 0,
            },
            collections: 0,
            files: 0,
            records: 0,
            value_members: 0,
            length_members: 0,
            state_facts: 0,
            gram_memberships: 0,
            sort_entries: 0,
            relation_occurrences: 0,
            import_ns: 0,
            backend_len_bytes: 0,
            backend_peak_bytes: 0,
        }
    }
}
