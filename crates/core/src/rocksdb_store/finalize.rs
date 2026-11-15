//! Timed exact finalization for the RocksDB prototype.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::model::{CollectionId, FieldPath, PathRoot};

use super::bitmap_block::decode_block;
use super::key_codec::Cursor;
use super::namespace::Namespace;
use super::value_codec::{
    BuildFileState, BuildPhase, CollectionMetadata, FieldCoverage, FieldDescriptor, FileMetadata,
    LengthDirectoryEntry, RecordMetadata, StateKind, StoredBinding, StoredContributions,
    ValueDirectoryEntry,
};
use super::{RocksStore, StoreError, StoreResult, keys, value_codec};

const MAX_METADATA_BATCH_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct FinalizationReport {
    pub scan_ns: u64,
    pub metadata_write_ns: u64,
    pub records: u64,
    pub value_directory_rows: u64,
    pub length_directory_rows: u64,
    pub state_statistic_rows: u64,
    pub fts_statistic_rows: u64,
    pub field_rows: u64,
    pub coverage_rows: u64,
    pub metadata_batches: u64,
}

#[derive(Default)]
struct CollectionScan {
    records: u64,
    logical_records: u64,
    high_water: u64,
    discovered_files: u64,
    indexed_files: u64,
    unavailable_files: u64,
    pending_files: u64,
    hash: Sha256,
}

#[derive(Clone)]
enum BindingIdentityView {
    Declared(Vec<String>),
    Named(Vec<String>),
}

pub(crate) fn finalize(store: &RocksStore) -> StoreResult<FinalizationReport> {
    transition_phase(store, BuildPhase::Building, BuildPhase::Finalizing)?;
    let scan_started = Instant::now();
    let view = store.view()?;
    let mut collections = (0..256)
        .map(|_| CollectionScan::default())
        .collect::<Vec<_>>();
    let mut binding_rows = BTreeMap::<(u32, u32), StoredBinding>::new();
    let mut structured_bindings = BTreeMap::<(u32, u32), BindingIdentityView>::new();
    let mut coverage_flags = BTreeMap::<(u32, u32), FieldCoverage>::new();
    let mut declared_covered = BTreeMap::<(u32, u32), u64>::new();
    let mut named_covered = BTreeMap::<(u32, u32), u64>::new();
    let mut value_directory = BTreeMap::<(u32, u32, crate::posting::ScalarKey), u64>::new();
    let mut length_directory = BTreeMap::<(u32, u32, crate::posting::ScalarKey), u64>::new();
    let mut state_statistics = BTreeMap::<(u32, u32, StateKind), u64>::new();
    let mut fts_statistics = BTreeMap::<(u32, u32, value_codec::GramKind, u64), u64>::new();
    let mut file_memberships = BTreeMap::<u32, Vec<CollectionId>>::new();
    let mut file_states = BTreeMap::<u32, BuildFileState>::new();
    let mut pending_count = 0u64;
    let mut free_slot_count = 0u64;

    scan_namespace(&view, Namespace::Collections, |_logical, bytes| {
        let metadata: CollectionMetadata = value_codec::decode(bytes, "collection metadata")?;
        let scan = collections
            .get_mut(metadata.id.0 as usize)
            .ok_or_else(|| StoreError::CorruptIndex("collection ID is too large".into()))?;
        scan.discovered_files = metadata.discovered_files;
        Ok(())
    })?;

    scan_namespace(&view, Namespace::Bindings, |logical, bytes| {
        let mut cursor = Cursor::new(logical);
        let collection = cursor.read_u32()?;
        let _identity_bytes = cursor.read_bytes()?;
        let stored: StoredBinding = value_codec::decode(bytes, "binding")?;
        binding_rows.insert((collection, stored.id), stored);
        Ok(())
    })?;
    for ((collection, binding), stored) in &binding_rows {
        let identity = match &stored.descriptor.identity {
            value_codec::BindingIdentity::Declared { path } => {
                Some(BindingIdentityView::Declared(path.clone()))
            }
            value_codec::BindingIdentity::Named { path, name: _ } => {
                Some(BindingIdentityView::Named(path.clone()))
            }
            _ => None,
        };
        if let Some(identity) = identity {
            structured_bindings.insert((*collection, *binding), identity);
        }
    }

    scan_namespace(&view, Namespace::FieldCoverageDeltas, |logical, bytes| {
        let mut cursor = Cursor::new(logical);
        let _batch = cursor.read_u64()?;
        let collection = cursor.read_u32()?;
        let binding = cursor.read_u32()?;
        if !cursor.is_empty() {
            return Err(StoreError::CorruptIndex(
                "coverage delta key has trailing bytes".into(),
            ));
        }
        let delta: value_codec::FieldCoverageDelta =
            value_codec::decode(bytes, "field coverage delta")?;
        if delta.collection.0 != collection || delta.binding != binding {
            return Err(StoreError::CorruptIndex(
                "coverage delta key/value mismatch".into(),
            ));
        }
        coverage_flags.insert((collection, binding), delta.coverage);
        Ok(())
    })?;

    scan_namespace(&view, Namespace::Files, |logical, bytes| {
        let mut cursor = Cursor::new(logical);
        let file = cursor.read_u32()?;
        let metadata: FileMetadata = value_codec::decode(bytes, "file metadata")?;
        if metadata.entry.id.0 != file {
            return Err(StoreError::CorruptIndex(
                "file metadata key/value mismatch".into(),
            ));
        }
        for collection in &metadata.entry.collections {
            let scan = collections
                .get_mut(collection.0 as usize)
                .ok_or_else(|| StoreError::CorruptIndex("file collection is unknown".into()))?;
            scan.hash.update(metadata.entry.id.0.to_be_bytes());
            scan.hash.update(crate::path_repr::native_bytes(
                metadata.entry.relative_path.as_path(),
            ));
            scan.hash.update(metadata.entry.revision.size.to_be_bytes());
            scan.hash
                .update(metadata.entry.revision.modified_ns.to_be_bytes());
        }
        file_memberships.insert(file, metadata.entry.collections);
        Ok(())
    })?;

    scan_namespace(&view, Namespace::FileBuildState, |logical, bytes| {
        let mut cursor = Cursor::new(logical);
        let file = cursor.read_u32()?;
        let state: value_codec::FileBuildState = value_codec::decode(bytes, "file build state")?;
        file_states.insert(file, state.state);
        Ok(())
    })?;

    scan_namespace(&view, Namespace::PendingFiles, |_logical, _bytes| {
        pending_count = pending_count.saturating_add(1);
        Ok(())
    })?;
    scan_namespace(&view, Namespace::FreeSlots, |_logical, _bytes| {
        free_slot_count = free_slot_count.saturating_add(1);
        Ok(())
    })?;

    scan_namespace(&view, Namespace::Records, |logical, bytes| {
        let mut cursor = Cursor::new(logical);
        let collection = cursor.read_u32()?;
        let slot = cursor.read_u32()?;
        let _record: RecordMetadata = value_codec::decode(bytes, "record metadata")?;
        let scan = collections
            .get_mut(collection as usize)
            .ok_or_else(|| StoreError::CorruptIndex("record collection is unknown".into()))?;
        scan.records = scan.records.saturating_add(1);
        scan.high_water = scan.high_water.max(u64::from(slot) + 1);
        Ok(())
    })?;

    scan_namespace(&view, Namespace::RecordContributions, |logical, bytes| {
        let mut cursor = Cursor::new(logical);
        let collection = cursor.read_u32()?;
        let _slot = cursor.read_u32()?;
        let contribution: StoredContributions =
            super::contribution_codec::decode(bytes)?.into_contributions(Vec::new());
        if contribution
            .domains
            .contains(&value_codec::DomainKind::Logical)
        {
            let scan = collections
                .get_mut(collection as usize)
                .ok_or_else(|| StoreError::CorruptIndex("record collection is unknown".into()))?;
            scan.logical_records = scan.logical_records.saturating_add(1);
            for ((c, binding), identity) in &structured_bindings {
                if *c == collection && matches!(identity, BindingIdentityView::Declared(_)) {
                    *declared_covered.entry((*c, *binding)).or_default() += 1;
                }
            }
        }
        let mut present = BTreeSet::new();
        present.extend(contribution.values.iter().map(|(binding, _)| *binding));
        present.extend(contribution.lengths.iter().map(|(binding, _)| *binding));
        present.extend(contribution.state_masks.iter().map(|(binding, _)| *binding));
        for binding in present {
            if matches!(
                structured_bindings.get(&(collection, binding)),
                Some(BindingIdentityView::Named(_))
            ) {
                *named_covered.entry((collection, binding)).or_default() += 1;
            }
        }
        Ok(())
    })?;

    scan_namespace(&view, Namespace::ValueBlocks, |logical, bytes| {
        let mut cursor = Cursor::new(logical);
        let collection = cursor.read_u32()?;
        let binding = cursor.read_u32()?;
        let value = cursor.read_scalar()?;
        let _block = cursor.read_u32()?;
        if !cursor.is_empty() {
            return Err(StoreError::CorruptIndex(
                "value block key has trailing bytes".into(),
            ));
        }
        let block = decode_block(bytes)?;
        *value_directory
            .entry((collection, binding, value))
            .or_default() += u64::from(block.cardinality());
        Ok(())
    })?;

    scan_namespace(&view, Namespace::LengthBlocks, |logical, bytes| {
        let mut cursor = Cursor::new(logical);
        let collection = cursor.read_u32()?;
        let binding = cursor.read_u32()?;
        let value = cursor.read_scalar()?;
        let _block = cursor.read_u32()?;
        if !cursor.is_empty() {
            return Err(StoreError::CorruptIndex(
                "length block key has trailing bytes".into(),
            ));
        }
        let block = decode_block(bytes)?;
        *length_directory
            .entry((collection, binding, value))
            .or_default() += u64::from(block.cardinality());
        Ok(())
    })?;

    scan_namespace(&view, Namespace::StateBlocks, |logical, bytes| {
        let mut cursor = Cursor::new(logical);
        let collection = cursor.read_u32()?;
        let binding = cursor.read_u32()?;
        let state = StateKind::from_tag(cursor.read_u8()?)
            .ok_or_else(|| StoreError::CorruptIndex("unknown state key tag".into()))?;
        let _block = cursor.read_u32()?;
        if !cursor.is_empty() {
            return Err(StoreError::CorruptIndex(
                "state block key has trailing bytes".into(),
            ));
        }
        let block = decode_block(bytes)?;
        *state_statistics
            .entry((collection, binding, state))
            .or_default() += u64::from(block.cardinality());
        Ok(())
    })?;

    scan_namespace(&view, Namespace::FtsBlocks, |logical, bytes| {
        let mut cursor = Cursor::new(logical);
        let collection = cursor.read_u32()?;
        let binding = cursor.read_u32()?;
        let kind = match cursor.read_u8()? {
            value_codec::GRAM_NORMAL => value_codec::GramKind::Normal,
            value_codec::GRAM_SKIP => value_codec::GramKind::Skip,
            tag => {
                return Err(StoreError::CorruptIndex(format!(
                    "unknown gram kind tag {tag}"
                )));
            }
        };
        let gram = cursor.read_u64()?;
        let _block = cursor.read_u32()?;
        if !cursor.is_empty() {
            return Err(StoreError::CorruptIndex(
                "FTS block key has trailing bytes".into(),
            ));
        }
        let block = decode_block(bytes)?;
        *fts_statistics
            .entry((collection, binding, kind, gram))
            .or_default() += u64::from(block.cardinality());
        Ok(())
    })?;

    for (file, collections_for_file) in &file_memberships {
        let state = file_states
            .get(file)
            .copied()
            .ok_or_else(|| StoreError::CorruptIndex(format!("file {file} has no build state")))?;
        for collection in collections_for_file {
            let scan = collections
                .get_mut(collection.0 as usize)
                .ok_or_else(|| StoreError::CorruptIndex("file collection is unknown".into()))?;
            match state {
                BuildFileState::Indexed => {
                    scan.indexed_files = scan.indexed_files.saturating_add(1)
                }
                BuildFileState::Unavailable => {
                    scan.unavailable_files = scan.unavailable_files.saturating_add(1)
                }
                BuildFileState::Pending => {
                    scan.pending_files = scan.pending_files.saturating_add(1)
                }
            }
        }
    }
    if pending_count != 0 {
        return Err(StoreError::State(format!(
            "{pending_count} pending file memberships remain after admission"
        )));
    }
    if free_slot_count != 0 {
        return Err(StoreError::State(
            "free slots are reserved and must be empty in this prototype".into(),
        ));
    }
    let scan_ns = elapsed_ns(scan_started);
    drop(view);

    let write_started = Instant::now();
    let mut writer = MetadataWriter::new(store);
    for ((collection, binding, value), cardinality) in &value_directory {
        writer.put(
            Namespace::ValueDirectory.physical(&keys::value_directory_key(
                *collection,
                *binding,
                value,
            )?),
            value_codec::encode(&ValueDirectoryEntry {
                cardinality: *cardinality,
                representative: value.clone(),
            })?,
        )?;
    }
    for ((collection, binding, value), cardinality) in &length_directory {
        writer.put(
            Namespace::LengthDirectory.physical(&keys::length_directory_key(
                *collection,
                *binding,
                value,
            )?),
            value_codec::encode(&LengthDirectoryEntry {
                cardinality: *cardinality,
            })?,
        )?;
    }
    for ((collection, binding, state), cardinality) in &state_statistics {
        writer.put(
            Namespace::StateStatistics.physical(&keys::state_statistics(
                *collection,
                *binding,
                *state,
            )),
            value_codec::encode(cardinality)?,
        )?;
    }
    for ((collection, binding, kind, gram), cardinality) in &fts_statistics {
        writer.put(
            Namespace::FtsGramStatistics.physical(&keys::fts_statistics_key(
                *collection,
                *binding,
                *kind,
                *gram,
            )),
            value_codec::encode(cardinality)?,
        )?;
    }

    for ((collection, binding), identity) in &structured_bindings {
        let flags = coverage_flags
            .get(&(*collection, *binding))
            .ok_or_else(|| {
                StoreError::CorruptIndex(format!(
                    "collection {collection} binding {binding} has no coverage metadata"
                ))
            })?
            .clone();
        let values = value_directory
            .keys()
            .filter(|(c, b, _)| c == collection && b == binding)
            .count() as u64;
        let lengths = length_directory
            .keys()
            .filter(|(c, b, _)| c == collection && b == binding)
            .count() as u64;
        let covered_records = match identity {
            BindingIdentityView::Declared(_) => declared_covered
                .get(&(*collection, *binding))
                .copied()
                .unwrap_or(0),
            BindingIdentityView::Named(_) => named_covered
                .get(&(*collection, *binding))
                .copied()
                .unwrap_or(0),
        };
        let kind = flags.kind.clone();
        let coverage = FieldCoverage {
            covered_records: u32::try_from(covered_records)
                .map_err(|_| StoreError::State("field covered-record count exceeds u32".into()))?,
            value_cardinality: values,
            length_cardinality: lengths,
            ..flags
        };
        writer.put(
            Namespace::FieldCoverage.physical(&keys::binding(*collection, *binding)),
            value_codec::encode(&coverage)?,
        )?;
        let path = match identity {
            BindingIdentityView::Declared(path) | BindingIdentityView::Named(path) => FieldPath {
                root: PathRoot::Data,
                segments: path.clone(),
            },
        };
        writer.put(
            Namespace::Fields.physical(&keys::field(*collection, &path.segments)),
            value_codec::encode(&FieldDescriptor {
                path,
                kind,
                multi: flags.multi,
                indexed: flags.values_indexed,
                values_indexed: flags.values_indexed,
                value_complete: flags.value_complete,
                length_complete: flags.length_complete,
                empty_string_complete: flags.empty_string_complete,
                covered_records: coverage.covered_records,
            })?,
        )?;
    }

    for (index, scan) in collections.iter_mut().enumerate() {
        if scan.records == 0
            && scan.discovered_files == 0
            && binding_rows
                .keys()
                .all(|(collection, _)| *collection != index as u32)
        {
            continue;
        }
        let existing_key = Namespace::Collections.physical(&keys::collection(index as u32));
        let mut metadata = store
            .view()?
            .get(Namespace::Collections, &keys::collection(index as u32))?
            .map(|bytes| value_codec::decode::<CollectionMetadata>(&bytes, "collection metadata"))
            .transpose()?
            .ok_or_else(|| {
                StoreError::CorruptIndex(format!("collection {index} metadata is missing"))
            })?;
        metadata.slot_high_water = scan.high_water;
        metadata.live_records = scan.records;
        metadata.discovered_files = scan.discovered_files;
        metadata.pending_files = scan.pending_files;
        metadata.indexed_files = scan.indexed_files;
        metadata.unavailable_files = scan.unavailable_files;
        metadata.processed_files = scan.indexed_files.saturating_add(scan.unavailable_files);
        metadata.source_fingerprint = scan.hash.clone().finalize().into();
        writer.put(existing_key, value_codec::encode(&metadata)?)?;
    }

    let delta_view = store.view()?;
    scan_namespace(
        &delta_view,
        Namespace::FieldCoverageDeltas,
        |logical, _bytes| {
            writer.delete(Namespace::FieldCoverageDeltas.physical(logical));
            Ok(())
        },
    )?;
    drop(delta_view);
    writer.flush()?;
    let metadata_batches = writer.batches;
    transition_phase(store, BuildPhase::Finalizing, BuildPhase::Complete)?;
    let metadata_write_ns = elapsed_ns(write_started);

    Ok(FinalizationReport {
        scan_ns,
        metadata_write_ns,
        records: collections
            .iter()
            .map(|collection| collection.records)
            .sum(),
        value_directory_rows: value_directory.len() as u64,
        length_directory_rows: length_directory.len() as u64,
        state_statistic_rows: state_statistics.len() as u64,
        fts_statistic_rows: fts_statistics.len() as u64,
        field_rows: structured_bindings.len() as u64,
        coverage_rows: structured_bindings.len() as u64,
        metadata_batches,
    })
}

fn transition_phase(store: &RocksStore, expected: BuildPhase, next: BuildPhase) -> StoreResult<()> {
    let transaction = store.transaction();
    let key = Namespace::StoreMetadata.physical(b"header");
    let bytes = transaction
        .get_for_update(&key, true)?
        .ok_or_else(|| StoreError::Incompatible("missing prototype header".into()))?;
    let mut header: value_codec::StoreHeader = value_codec::decode(&bytes, "store header")?;
    if header.phase != expected {
        return Err(StoreError::State(format!(
            "expected build phase {expected:?}, found {:?}",
            header.phase
        )));
    }
    header.phase = next;
    transaction.put(&key, value_codec::encode(&header)?)?;
    transaction.commit()?;
    Ok(())
}

fn scan_namespace(
    view: &super::StoreView<'_>,
    namespace: Namespace,
    mut visit: impl FnMut(&[u8], &[u8]) -> StoreResult<()>,
) -> StoreResult<()> {
    let mut iterator = view.namespace_prefix(namespace, b"")?;
    iterator.seek_to_first();
    while iterator.valid() {
        let key = iterator
            .key()
            .ok_or_else(|| StoreError::CorruptIndex("missing namespace key".into()))?;
        if super::namespace::namespace_of(key) != Some(namespace) {
            return Err(StoreError::CorruptIndex(
                "namespace iterator crossed a boundary".into(),
            ));
        }
        let value = iterator
            .value()
            .ok_or_else(|| StoreError::CorruptIndex("missing namespace value".into()))?;
        visit(&key[1..], value)?;
        iterator.next();
    }
    iterator.status()?;
    Ok(())
}

struct MetadataWriter<'a> {
    store: &'a RocksStore,
    puts: BTreeMap<Vec<u8>, Vec<u8>>,
    deletes: BTreeSet<Vec<u8>>,
    bytes: usize,
    batches: u64,
}

impl<'a> MetadataWriter<'a> {
    fn new(store: &'a RocksStore) -> Self {
        Self {
            store,
            puts: BTreeMap::new(),
            deletes: BTreeSet::new(),
            bytes: 0,
            batches: 0,
        }
    }

    fn put(&mut self, key: Vec<u8>, value: Vec<u8>) -> StoreResult<()> {
        self.bytes = self
            .bytes
            .saturating_add(key.len().saturating_add(value.len()));
        self.puts.insert(key, value);
        if self.bytes >= MAX_METADATA_BATCH_BYTES {
            self.flush()?;
        }
        Ok(())
    }

    fn delete(&mut self, key: Vec<u8>) {
        self.bytes = self.bytes.saturating_add(key.len());
        self.deletes.insert(key);
    }

    fn flush(&mut self) -> StoreResult<()> {
        if self.puts.is_empty() && self.deletes.is_empty() {
            return Ok(());
        }
        let transaction = self.store.transaction();
        for key in &self.deletes {
            transaction.delete(key)?;
        }
        for (key, value) in &self.puts {
            transaction.put(key, value)?;
        }
        transaction.commit()?;
        self.puts.clear();
        self.deletes.clear();
        self.bytes = 0;
        self.batches += 1;
        Ok(())
    }
}

fn elapsed_ns(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
}
