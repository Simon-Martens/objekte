//! Timed exact finalization for the RocksDB prototype.

use std::collections::{BTreeMap, HashSet};
use std::time::Instant;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::model::{FieldPath, PathRoot};

use super::bitmap_block::decode_block;
use super::key_codec::Cursor;
use super::metadata::{MetadataWriteStats, MetadataWriter};
use super::namespace::Namespace;
use super::value_codec::{
    BuildFileState, BuildPhase, CollectionMetadata, FieldCoverage, FieldDescriptor, FileMetadata,
    LengthDirectoryEntry, RecordMetadata, StateKind, StoredBinding, ValueDirectoryEntry,
};
use super::{RocksStore, StoreError, StoreResult, keys, value_codec};

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
    pub coverage_delta_rows: u64,
    pub coverage_delta_bytes: u64,
    pub admission_receipts: u64,
    pub metadata_batches: u64,
    pub metadata: MetadataWriteStats,
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
    let mut initial_collection_metadata = BTreeMap::<u32, CollectionMetadata>::new();
    let mut binding_rows = BTreeMap::<(u32, u32), StoredBinding>::new();
    let mut structured_bindings = BTreeMap::<(u32, u32), BindingIdentityView>::new();
    let mut coverage_flags = BTreeMap::<(u32, u32), FieldCoverage>::new();
    let mut declared_covered = BTreeMap::<(u32, u32), u64>::new();
    let mut named_members = BTreeMap::<(u32, u32), HashSet<u32>>::new();
    let mut value_cardinality = BTreeMap::<(u32, u32), u64>::new();
    let mut length_cardinality = BTreeMap::<(u32, u32), u64>::new();
    let mut pending_count = 0u64;
    let mut free_slot_count = 0u64;
    let mut admission_receipts = 0u64;
    let mut coverage_delta_rows = 0u64;
    let mut coverage_delta_bytes = 0u64;

    scan_namespace(&view, Namespace::Collections, |_logical, bytes| {
        let metadata: CollectionMetadata = value_codec::decode(bytes, "collection metadata")?;
        let scan = collections
            .get_mut(metadata.id.0 as usize)
            .ok_or_else(|| StoreError::CorruptIndex("collection ID is too large".into()))?;
        scan.discovered_files = metadata.discovered_files;
        initial_collection_metadata.insert(metadata.id.0, metadata);
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
            coverage_flags.insert(
                (*collection, *binding),
                FieldCoverage {
                    kind: stored.descriptor.kind.clone(),
                    multi: stored.descriptor.multi,
                    values_indexed: true,
                    value_complete: true,
                    length_complete: true,
                    empty_string_complete: true,
                    covered_records: 0,
                    inference_status: value_codec::INFERENCE_STATUS_NOT_APPLICABLE,
                    value_cardinality: 0,
                    length_cardinality: 0,
                },
            );
        }
    }

    scan_namespace(&view, Namespace::FieldCoverageDeltas, |logical, bytes| {
        coverage_delta_rows = coverage_delta_rows.saturating_add(1);
        coverage_delta_bytes = coverage_delta_bytes
            .saturating_add(logical.len() as u64)
            .saturating_add(bytes.len() as u64);
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
        let aggregate = coverage_flags
            .entry((collection, binding))
            .or_insert_with(|| delta.coverage.clone());
        aggregate.value_complete &= delta.coverage.value_complete;
        aggregate.length_complete &= delta.coverage.length_complete;
        aggregate.empty_string_complete &= delta.coverage.empty_string_complete;
        Ok(())
    })?;
    scan_namespace(&view, Namespace::BuildReceipts, |_logical, bytes| {
        let receipt: value_codec::BuildReceipt = value_codec::decode(bytes, "build receipt")?;
        for (collection, binding, coverage) in &receipt.coverage {
            let aggregate = coverage_flags
                .entry((collection.0, *binding))
                .or_insert_with(|| coverage.clone());
            aggregate.value_complete &= coverage.value_complete;
            aggregate.length_complete &= coverage.length_complete;
            aggregate.empty_string_complete &= coverage.empty_string_complete;
        }
        admission_receipts = admission_receipts.saturating_add(1);
        Ok(())
    })?;
    let coverage_row_upper_bound =
        admission_receipts.saturating_mul(structured_bindings.len() as u64);
    if coverage_delta_rows > coverage_row_upper_bound {
        return Err(StoreError::CorruptIndex(format!(
            "{coverage_delta_rows} coverage deltas exceed {admission_receipts} receipts times {} bindings",
            structured_bindings.len()
        )));
    }

    scan_namespace(&view, Namespace::PendingFiles, |_logical, _bytes| {
        pending_count = pending_count.saturating_add(1);
        Ok(())
    })?;
    scan_namespace(&view, Namespace::FreeSlots, |_logical, _bytes| {
        free_slot_count = free_slot_count.saturating_add(1);
        Ok(())
    })?;

    let mut files_iterator = view.namespace_prefix(Namespace::Files, b"")?;
    let mut states_iterator = view.namespace_prefix(Namespace::FileBuildState, b"")?;
    files_iterator.seek_to_first();
    states_iterator.seek_to_first();
    while files_iterator.valid() {
        let file_key = files_iterator
            .key()
            .ok_or_else(|| StoreError::CorruptIndex("missing file metadata key".into()))?;
        let mut cursor = Cursor::new(&file_key[1..]);
        let file = cursor.read_u32()?;
        let metadata: FileMetadata = value_codec::decode(
            files_iterator
                .value()
                .ok_or_else(|| StoreError::CorruptIndex("missing file metadata value".into()))?,
            "file metadata",
        )?;
        if metadata.entry.id.0 != file {
            return Err(StoreError::CorruptIndex(
                "file metadata key/value mismatch".into(),
            ));
        }
        if !states_iterator.valid() {
            return Err(StoreError::CorruptIndex(format!(
                "file {file} has no build state"
            )));
        }
        let state_key = states_iterator
            .key()
            .ok_or_else(|| StoreError::CorruptIndex("missing file build state key".into()))?;
        let mut state_cursor = Cursor::new(&state_key[1..]);
        let state_file = state_cursor.read_u32()?;
        if state_file != file {
            return Err(StoreError::CorruptIndex(format!(
                "file inventory order and build state order differ at {file} and {state_file}"
            )));
        }
        let state: value_codec::FileBuildState = value_codec::decode(
            states_iterator
                .value()
                .ok_or_else(|| StoreError::CorruptIndex("missing file build state value".into()))?,
            "file build state",
        )?;
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
            match state.state {
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
        files_iterator.next();
        states_iterator.next();
    }
    files_iterator.status()?;
    states_iterator.status()?;
    if states_iterator.valid() {
        return Err(StoreError::CorruptIndex(
            "file build state has rows without file metadata".into(),
        ));
    }
    drop(files_iterator);
    drop(states_iterator);

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

    scan_namespace(&view, Namespace::DomainMembers, |logical, _bytes| {
        let mut cursor = Cursor::new(logical);
        let collection = cursor.read_u32()?;
        let domain = match cursor.read_u8()? {
            value_codec::DOMAIN_LOGICAL => value_codec::DomainKind::Logical,
            value_codec::DOMAIN_RAW_FALLBACK => value_codec::DomainKind::RawFallback,
            value_codec::DOMAIN_EMPTY_ARRAY => value_codec::DomainKind::EmptyArray,
            tag => {
                return Err(StoreError::CorruptIndex(format!(
                    "unknown domain tag {tag}"
                )));
            }
        };
        if domain == value_codec::DomainKind::Logical {
            let scan = collections
                .get_mut(collection as usize)
                .ok_or_else(|| StoreError::CorruptIndex("record collection is unknown".into()))?;
            scan.logical_records = scan.logical_records.saturating_add(1);
            for ((binding_collection, binding), identity) in &structured_bindings {
                if *binding_collection == collection
                    && matches!(identity, BindingIdentityView::Declared(_))
                {
                    *declared_covered
                        .entry((*binding_collection, *binding))
                        .or_default() += 1;
                }
            }
        }
        Ok(())
    })?;

    let write_started = Instant::now();
    let mut writer = MetadataWriter::new(store);
    let mut value_directory_rows = 0u64;
    let mut current_value = None::<(u32, u32, crate::posting::ScalarKey)>;
    let mut current_value_cardinality = 0u64;
    scan_namespace(&view, Namespace::ValueBlocks, |logical, bytes| {
        let mut cursor = Cursor::new(logical);
        let collection = cursor.read_u32()?;
        let binding = cursor.read_u32()?;
        let value = cursor.read_scalar()?;
        let block_number = cursor.read_u32()?;
        if !cursor.is_empty() {
            return Err(StoreError::CorruptIndex(
                "value block key has trailing bytes".into(),
            ));
        }
        let block = decode_block(bytes)?;
        record_named_members(
            &mut named_members,
            &structured_bindings,
            collection,
            binding,
            block_number,
            &block,
        );
        let next = (collection, binding, value);
        if current_value.as_ref() != Some(&next) {
            if let Some(previous) = current_value.replace(next.clone()) {
                writer.put(
                    Namespace::ValueDirectory.physical(&keys::value_directory_key(
                        previous.0,
                        previous.1,
                        &previous.2,
                    )?),
                    value_codec::encode(&ValueDirectoryEntry {
                        cardinality: current_value_cardinality,
                        representative: previous.2,
                    })?,
                )?;
                value_directory_rows = value_directory_rows.saturating_add(1);
                *value_cardinality
                    .entry((previous.0, previous.1))
                    .or_default() += 1;
                current_value_cardinality = 0;
            }
        }
        current_value_cardinality =
            current_value_cardinality.saturating_add(u64::from(block.cardinality()));
        Ok(())
    })?;
    if let Some(previous) = current_value {
        writer.put(
            Namespace::ValueDirectory.physical(&keys::value_directory_key(
                previous.0,
                previous.1,
                &previous.2,
            )?),
            value_codec::encode(&ValueDirectoryEntry {
                cardinality: current_value_cardinality,
                representative: previous.2,
            })?,
        )?;
        value_directory_rows = value_directory_rows.saturating_add(1);
        *value_cardinality
            .entry((previous.0, previous.1))
            .or_default() += 1;
    }

    let mut length_directory_rows = 0u64;
    let mut current_length = None::<(u32, u32, crate::posting::ScalarKey)>;
    let mut current_length_cardinality = 0u64;
    scan_namespace(&view, Namespace::LengthBlocks, |logical, bytes| {
        let mut cursor = Cursor::new(logical);
        let collection = cursor.read_u32()?;
        let binding = cursor.read_u32()?;
        let value = cursor.read_scalar()?;
        let block_number = cursor.read_u32()?;
        if !cursor.is_empty() {
            return Err(StoreError::CorruptIndex(
                "length block key has trailing bytes".into(),
            ));
        }
        let block = decode_block(bytes)?;
        record_named_members(
            &mut named_members,
            &structured_bindings,
            collection,
            binding,
            block_number,
            &block,
        );
        let next = (collection, binding, value);
        if current_length.as_ref() != Some(&next) {
            if let Some(previous) = current_length.replace(next.clone()) {
                writer.put(
                    Namespace::LengthDirectory.physical(&keys::length_directory_key(
                        previous.0,
                        previous.1,
                        &previous.2,
                    )?),
                    value_codec::encode(&LengthDirectoryEntry {
                        cardinality: current_length_cardinality,
                    })?,
                )?;
                length_directory_rows = length_directory_rows.saturating_add(1);
                *length_cardinality
                    .entry((previous.0, previous.1))
                    .or_default() += 1;
                current_length_cardinality = 0;
            }
        }
        current_length_cardinality =
            current_length_cardinality.saturating_add(u64::from(block.cardinality()));
        Ok(())
    })?;
    if let Some(previous) = current_length {
        writer.put(
            Namespace::LengthDirectory.physical(&keys::length_directory_key(
                previous.0,
                previous.1,
                &previous.2,
            )?),
            value_codec::encode(&LengthDirectoryEntry {
                cardinality: current_length_cardinality,
            })?,
        )?;
        length_directory_rows = length_directory_rows.saturating_add(1);
        *length_cardinality
            .entry((previous.0, previous.1))
            .or_default() += 1;
    }

    let mut state_statistic_rows = 0u64;
    let mut current_state = None::<(u32, u32, StateKind)>;
    let mut current_state_cardinality = 0u64;
    scan_namespace(&view, Namespace::StateBlocks, |logical, bytes| {
        let mut cursor = Cursor::new(logical);
        let collection = cursor.read_u32()?;
        let binding = cursor.read_u32()?;
        let state = StateKind::from_tag(cursor.read_u8()?)
            .ok_or_else(|| StoreError::CorruptIndex("unknown state key tag".into()))?;
        let block_number = cursor.read_u32()?;
        if !cursor.is_empty() {
            return Err(StoreError::CorruptIndex(
                "state block key has trailing bytes".into(),
            ));
        }
        let block = decode_block(bytes)?;
        record_named_members(
            &mut named_members,
            &structured_bindings,
            collection,
            binding,
            block_number,
            &block,
        );
        let next = (collection, binding, state);
        if current_state.as_ref() != Some(&next) {
            if let Some(previous) = current_state.replace(next) {
                writer.put(
                    Namespace::StateStatistics
                        .physical(&keys::state_statistics(previous.0, previous.1, previous.2)),
                    value_codec::encode(&current_state_cardinality)?,
                )?;
                state_statistic_rows = state_statistic_rows.saturating_add(1);
                current_state_cardinality = 0;
            }
        }
        current_state_cardinality =
            current_state_cardinality.saturating_add(u64::from(block.cardinality()));
        Ok(())
    })?;
    if let Some(previous) = current_state {
        writer.put(
            Namespace::StateStatistics
                .physical(&keys::state_statistics(previous.0, previous.1, previous.2)),
            value_codec::encode(&current_state_cardinality)?,
        )?;
        state_statistic_rows = state_statistic_rows.saturating_add(1);
    }

    let mut fts_statistic_rows = 0u64;
    let mut current_fts = None::<(u32, u32, value_codec::GramKind, u64)>;
    let mut current_fts_cardinality = 0u64;
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
        let _block_number = cursor.read_u32()?;
        if !cursor.is_empty() {
            return Err(StoreError::CorruptIndex(
                "FTS block key has trailing bytes".into(),
            ));
        }
        let block = decode_block(bytes)?;
        let next = (collection, binding, kind, gram);
        if current_fts.as_ref() != Some(&next) {
            if let Some(previous) = current_fts.replace(next) {
                writer.put(
                    Namespace::FtsGramStatistics.physical(&keys::fts_statistics_key(
                        previous.0, previous.1, previous.2, previous.3,
                    )),
                    value_codec::encode(&current_fts_cardinality)?,
                )?;
                fts_statistic_rows = fts_statistic_rows.saturating_add(1);
                current_fts_cardinality = 0;
            }
        }
        current_fts_cardinality =
            current_fts_cardinality.saturating_add(u64::from(block.cardinality()));
        Ok(())
    })?;
    if let Some(previous) = current_fts {
        writer.put(
            Namespace::FtsGramStatistics.physical(&keys::fts_statistics_key(
                previous.0, previous.1, previous.2, previous.3,
            )),
            value_codec::encode(&current_fts_cardinality)?,
        )?;
        fts_statistic_rows = fts_statistic_rows.saturating_add(1);
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

    for ((collection, binding), identity) in &structured_bindings {
        let flags = coverage_flags
            .get(&(*collection, *binding))
            .ok_or_else(|| {
                StoreError::CorruptIndex(format!(
                    "collection {collection} binding {binding} has no coverage metadata"
                ))
            })?
            .clone();
        let values = value_cardinality
            .get(&(*collection, *binding))
            .copied()
            .unwrap_or(0);
        let lengths = length_cardinality
            .get(&(*collection, *binding))
            .copied()
            .unwrap_or(0);
        let covered_records = match identity {
            BindingIdentityView::Declared(_) => declared_covered
                .get(&(*collection, *binding))
                .copied()
                .unwrap_or(0),
            BindingIdentityView::Named(_) => named_members
                .get(&(*collection, *binding))
                .map_or(0, |members| members.len() as u64),
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
        let BindingIdentityView::Declared(path) = identity else {
            continue;
        };
        let path = FieldPath {
            root: PathRoot::Data,
            segments: path.clone(),
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
        let mut metadata = initial_collection_metadata
            .remove(&(index as u32))
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

    writer.flush()?;
    let metadata = writer.stats();
    let metadata_batches = metadata.batches;
    transition_phase(store, BuildPhase::Finalizing, BuildPhase::Complete)?;
    let metadata_write_ns = elapsed_ns(write_started);

    Ok(FinalizationReport {
        scan_ns,
        metadata_write_ns,
        records: collections
            .iter()
            .map(|collection| collection.records)
            .sum(),
        value_directory_rows,
        length_directory_rows,
        state_statistic_rows,
        fts_statistic_rows,
        field_rows: structured_bindings.len() as u64,
        coverage_rows: structured_bindings.len() as u64,
        coverage_delta_rows,
        coverage_delta_bytes,
        admission_receipts,
        metadata_batches,
        metadata,
    })
}

fn record_named_members(
    members: &mut BTreeMap<(u32, u32), HashSet<u32>>,
    identities: &BTreeMap<(u32, u32), BindingIdentityView>,
    collection: u32,
    binding: u32,
    block_number: u32,
    block: &super::bitmap_block::Block,
) {
    if matches!(
        identities.get(&(collection, binding)),
        Some(BindingIdentityView::Named(_))
    ) {
        let entries = members.entry((collection, binding)).or_default();
        block.for_each(|offset| {
            entries.insert((block_number << super::bitmap_block::BLOCK_SHIFT) | u32::from(offset));
        });
    }
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

fn elapsed_ns(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
}
