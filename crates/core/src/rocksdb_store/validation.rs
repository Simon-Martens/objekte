//! Independent invariant and reverse-membership checks for the prototype.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use super::bitmap_block::{Block, decode_block};
use super::key_codec::Cursor;
use super::namespace::Namespace;
use super::value_codec::{BuildPhase, CollectionMetadata, StoredContributions};
use super::{RocksStore, StoreError, StoreResult, keys, value_codec};

#[derive(Clone, Debug, Default, Serialize)]
pub struct ValidationReport {
    pub collections: u64,
    pub records: u64,
    pub files: u64,
    pub receipts: u64,
    pub value_members: u64,
    pub length_members: u64,
    pub state_members: u64,
    pub gram_memberships: u64,
    pub sort_entries: u64,
    pub relation_occurrences: u64,
    pub value_directory_rows: u64,
    pub length_directory_rows: u64,
    pub coverage_rows: u64,
    pub errors: Vec<String>,
}

impl ValidationReport {
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    fn error(&mut self, error: impl Into<String>) {
        if self.errors.len() < 64 {
            self.errors.push(error.into());
        }
    }
}

pub fn validate(store: &RocksStore) -> StoreResult<ValidationReport> {
    let view = store.view()?;
    if view.header().phase != BuildPhase::Complete {
        return Err(StoreError::NotFinalized(format!(
            "phase is {:?}",
            view.header().phase
        )));
    }
    let mut report = ValidationReport::default();
    let mut collection_counts = BTreeMap::<u32, (u64, u64)>::new();
    let mut value_members = BTreeMap::<Vec<u8>, Block>::new();
    let mut length_members = BTreeMap::<Vec<u8>, Block>::new();
    let mut state_members = BTreeMap::<Vec<u8>, Block>::new();
    let mut fts_members = BTreeMap::<Vec<u8>, Block>::new();
    let mut record_grams = BTreeMap::<Vec<u8>, Vec<u64>>::new();
    let mut record_grams_by_slot =
        BTreeMap::<(u32, u32), Vec<(u32, value_codec::GramKind, Vec<u64>)>>::new();
    let mut file_records = BTreeSet::new();
    let mut logical_ids = BTreeMap::<Vec<u8>, Vec<u8>>::new();
    let mut domain_members = BTreeSet::new();
    let mut outgoing = BTreeSet::new();
    let mut incoming = BTreeSet::new();

    scan(&view, Namespace::Collections, |_logical, bytes| {
        let metadata: CollectionMetadata = value_codec::decode(bytes, "collection")?;
        report.collections = report.collections.saturating_add(1);
        collection_counts.insert(metadata.id.0, (0, 0));
        Ok(())
    })?;
    scan(&view, Namespace::Files, |_logical, _bytes| {
        report.files = report.files.saturating_add(1);
        Ok(())
    })?;
    scan(&view, Namespace::BuildReceipts, |_logical, _bytes| {
        report.receipts = report.receipts.saturating_add(1);
        Ok(())
    })?;
    scan(&view, Namespace::PendingFiles, |_logical, _bytes| {
        report.error("pending file membership remains in a Complete store");
        Ok(())
    })?;
    scan(&view, Namespace::FileBuildState, |_logical, bytes| {
        let state: value_codec::FileBuildState = value_codec::decode(bytes, "file build state")?;
        if state.state == value_codec::BuildFileState::Pending {
            report.error("file build state remains Pending in a Complete store");
        }
        Ok(())
    })?;
    scan(&view, Namespace::ValueBlocks, |logical, bytes| {
        let mut cursor = Cursor::new(logical);
        let collection = cursor.read_u32()?;
        let binding = cursor.read_u32()?;
        let value = cursor.read_scalar()?;
        let block = cursor.read_u32()?;
        if !cursor.is_empty() {
            return Err(StoreError::CorruptIndex(
                "value block key has trailing bytes".into(),
            ));
        }
        value_members.insert(
            keys::value_block(collection, binding, &value, block)?,
            decode_block(bytes)?,
        );
        Ok(())
    })?;
    scan(&view, Namespace::LengthBlocks, |logical, bytes| {
        let mut cursor = Cursor::new(logical);
        let collection = cursor.read_u32()?;
        let binding = cursor.read_u32()?;
        let value = cursor.read_scalar()?;
        let block = cursor.read_u32()?;
        if !cursor.is_empty() {
            return Err(StoreError::CorruptIndex(
                "length block key has trailing bytes".into(),
            ));
        }
        length_members.insert(
            keys::length_block(collection, binding, &value, block)?,
            decode_block(bytes)?,
        );
        Ok(())
    })?;
    scan(&view, Namespace::StateBlocks, |logical, bytes| {
        let mut cursor = Cursor::new(logical);
        let collection = cursor.read_u32()?;
        let binding = cursor.read_u32()?;
        let state = value_codec::StateKind::from_tag(cursor.read_u8()?)
            .ok_or_else(|| StoreError::CorruptIndex("unknown state tag".into()))?;
        let block = cursor.read_u32()?;
        if !cursor.is_empty() {
            return Err(StoreError::CorruptIndex(
                "state block key has trailing bytes".into(),
            ));
        }
        state_members.insert(
            keys::state_block(collection, binding, state, block),
            decode_block(bytes)?,
        );
        Ok(())
    })?;
    scan(&view, Namespace::FtsBlocks, |logical, bytes| {
        let mut cursor = Cursor::new(logical);
        let collection = cursor.read_u32()?;
        let binding = cursor.read_u32()?;
        let kind = match cursor.read_u8()? {
            value_codec::GRAM_NORMAL => value_codec::GramKind::Normal,
            value_codec::GRAM_SKIP => value_codec::GramKind::Skip,
            tag => {
                return Err(StoreError::CorruptIndex(format!("unknown gram kind {tag}")));
            }
        };
        let gram = cursor.read_u64()?;
        let block = cursor.read_u32()?;
        if !cursor.is_empty() {
            return Err(StoreError::CorruptIndex(
                "FTS block key has trailing bytes".into(),
            ));
        }
        fts_members.insert(
            keys::fts_key(collection, binding, kind, gram, block),
            decode_block(bytes)?,
        );
        Ok(())
    })?;
    scan(&view, Namespace::FileRecords, |logical, _bytes| {
        file_records.insert(logical.to_vec());
        Ok(())
    })?;
    scan(&view, Namespace::LogicalIds, |logical, bytes| {
        logical_ids.insert(logical.to_vec(), bytes.to_vec());
        Ok(())
    })?;
    scan(&view, Namespace::DomainMembers, |logical, _bytes| {
        domain_members.insert(logical.to_vec());
        Ok(())
    })?;
    scan(&view, Namespace::RecordGrams, |logical, bytes| {
        let grams = value_codec::decode_gram_list(bytes)?;
        record_grams.insert(logical.to_vec(), grams.clone());
        let mut cursor = Cursor::new(logical);
        let collection = cursor.read_u32()?;
        let slot = cursor.read_u32()?;
        let binding = cursor.read_u32()?;
        let kind = match cursor.read_u8()? {
            value_codec::GRAM_NORMAL => value_codec::GramKind::Normal,
            value_codec::GRAM_SKIP => value_codec::GramKind::Skip,
            tag => {
                return Err(StoreError::CorruptIndex(format!("unknown gram kind {tag}")));
            }
        };
        record_grams_by_slot
            .entry((collection, slot))
            .or_default()
            .push((binding, kind, grams));
        Ok(())
    })?;
    scan(&view, Namespace::RelationOutgoing, |logical, _bytes| {
        outgoing.insert(logical.to_vec());
        Ok(())
    })?;
    scan(&view, Namespace::RelationIncoming, |logical, _bytes| {
        incoming.insert(logical.to_vec());
        Ok(())
    })?;

    scan(&view, Namespace::Records, |logical, bytes| {
        let mut cursor = Cursor::new(logical);
        let collection = cursor.read_u32()?;
        let slot = cursor.read_u32()?;
        let record: value_codec::RecordMetadata = value_codec::decode(bytes, "record metadata")?;
        let contribution_bytes = view
            .get(
                Namespace::RecordContributions,
                &keys::record(collection, slot),
            )?
            .ok_or_else(|| {
                StoreError::CorruptIndex(format!(
                    "collection {collection} slot {slot} has no reverse contribution"
                ))
            })?;
        let contribution =
            super::contribution_codec::decode(&contribution_bytes)?.into_contributions(Vec::new());
        if contribution.record != record {
            report.error(format!(
                "collection {collection} slot {slot} contribution record mismatch"
            ));
        }
        validate_contribution(
            collection,
            slot,
            &contribution,
            &value_members,
            &length_members,
            &state_members,
            &fts_members,
            &record_grams,
            &record_grams_by_slot,
            &file_records,
            &logical_ids,
            &domain_members,
            &outgoing,
            &incoming,
            &mut report,
        )?;
        let counts = collection_counts
            .get_mut(&collection)
            .ok_or_else(|| StoreError::CorruptIndex("record collection is unknown".into()))?;
        counts.0 = counts.0.saturating_add(1);
        counts.1 = counts.1.max(u64::from(slot) + 1);
        report.records = report.records.saturating_add(1);
        Ok(())
    })?;

    let mut value_directory = BTreeMap::<(u32, u32, crate::posting::ScalarKey), u64>::new();
    for (logical, block) in &value_members {
        let mut cursor = Cursor::new(logical);
        let collection = cursor.read_u32()?;
        let binding = cursor.read_u32()?;
        let value = cursor.read_scalar()?;
        let _block = cursor.read_u32()?;
        *value_directory
            .entry((collection, binding, value))
            .or_default() += u64::from(block.cardinality());
    }
    for (key, expected) in &value_directory {
        let bytes = view
            .get(
                Namespace::ValueDirectory,
                &keys::value_directory_key(key.0, key.1, &key.2)?,
            )?
            .ok_or_else(|| {
                StoreError::CorruptIndex(format!(
                    "missing value directory row for collection {} binding {}",
                    key.0, key.1
                ))
            })?;
        let entry: value_codec::ValueDirectoryEntry =
            value_codec::decode(&bytes, "value directory")?;
        if entry.cardinality != *expected || entry.representative != key.2 {
            report.error(format!(
                "value directory cardinality mismatch for collection {} binding {}",
                key.0, key.1
            ));
        }
        report.value_directory_rows = report.value_directory_rows.saturating_add(1);
    }
    let mut length_directory = BTreeMap::<(u32, u32, crate::posting::ScalarKey), u64>::new();
    for (logical, block) in &length_members {
        let mut cursor = Cursor::new(logical);
        let collection = cursor.read_u32()?;
        let binding = cursor.read_u32()?;
        let value = cursor.read_scalar()?;
        let _block = cursor.read_u32()?;
        *length_directory
            .entry((collection, binding, value))
            .or_default() += u64::from(block.cardinality());
    }
    for (key, expected) in &length_directory {
        let bytes = view
            .get(
                Namespace::LengthDirectory,
                &keys::length_directory_key(key.0, key.1, &key.2)?,
            )?
            .ok_or_else(|| {
                StoreError::CorruptIndex(format!(
                    "missing length directory row for collection {} binding {}",
                    key.0, key.1
                ))
            })?;
        let entry: value_codec::LengthDirectoryEntry =
            value_codec::decode(&bytes, "length directory")?;
        if entry.cardinality != *expected {
            report.error(format!(
                "length directory cardinality mismatch for collection {} binding {}",
                key.0, key.1
            ));
        }
        report.length_directory_rows = report.length_directory_rows.saturating_add(1);
    }
    let mut state_statistics = BTreeMap::<(u32, u32, value_codec::StateKind), u64>::new();
    for (logical, block) in &state_members {
        let mut cursor = Cursor::new(logical);
        let collection = cursor.read_u32()?;
        let binding = cursor.read_u32()?;
        let state = value_codec::StateKind::from_tag(cursor.read_u8()?)
            .ok_or_else(|| StoreError::CorruptIndex("unknown state tag".into()))?;
        let _block = cursor.read_u32()?;
        *state_statistics
            .entry((collection, binding, state))
            .or_default() += u64::from(block.cardinality());
    }
    for (key, expected) in &state_statistics {
        let bytes = view
            .get(
                Namespace::StateStatistics,
                &keys::state_statistics(key.0, key.1, key.2),
            )?
            .ok_or_else(|| StoreError::CorruptIndex("missing state statistic".into()))?;
        let actual: u64 = value_codec::decode(&bytes, "state statistic")?;
        if actual != *expected {
            report.error("state statistic cardinality mismatch");
        }
    }
    let mut fts_statistics = BTreeMap::<(u32, u32, value_codec::GramKind, u64), u64>::new();
    for (logical, block) in &fts_members {
        let mut cursor = Cursor::new(logical);
        let collection = cursor.read_u32()?;
        let binding = cursor.read_u32()?;
        let kind = match cursor.read_u8()? {
            value_codec::GRAM_NORMAL => value_codec::GramKind::Normal,
            value_codec::GRAM_SKIP => value_codec::GramKind::Skip,
            tag => {
                return Err(StoreError::CorruptIndex(format!("unknown gram kind {tag}")));
            }
        };
        let gram = cursor.read_u64()?;
        let _block = cursor.read_u32()?;
        *fts_statistics
            .entry((collection, binding, kind, gram))
            .or_default() += u64::from(block.cardinality());
    }
    for (key, expected) in &fts_statistics {
        let bytes = view
            .get(
                Namespace::FtsGramStatistics,
                &keys::fts_statistics_key(key.0, key.1, key.2, key.3),
            )?
            .ok_or_else(|| StoreError::CorruptIndex("missing FTS statistic".into()))?;
        let actual: u64 = value_codec::decode(&bytes, "FTS statistic")?;
        if actual != *expected {
            report.error("FTS statistic cardinality mismatch");
        }
    }
    scan(&view, Namespace::FieldCoverage, |_logical, _bytes| {
        report.coverage_rows = report.coverage_rows.saturating_add(1);
        Ok(())
    })?;

    for (collection, (records, high_water)) in collection_counts {
        let bytes = view
            .get(Namespace::Collections, &keys::collection(collection))?
            .ok_or_else(|| StoreError::CorruptIndex("collection metadata missing".into()))?;
        let metadata: CollectionMetadata = value_codec::decode(&bytes, "collection metadata")?;
        if metadata.live_records != records || metadata.slot_high_water != high_water {
            report.error(format!(
                "collection {collection} counters do not match completed records"
            ));
        }
        if metadata.pending_files != 0 {
            report.error(format!(
                "collection {collection} reports pending files after completion"
            ));
        }
    }
    Ok(report)
}

fn validate_contribution(
    collection: u32,
    slot: u32,
    contribution: &StoredContributions,
    value_members: &BTreeMap<Vec<u8>, Block>,
    length_members: &BTreeMap<Vec<u8>, Block>,
    state_members: &BTreeMap<Vec<u8>, Block>,
    fts_members: &BTreeMap<Vec<u8>, Block>,
    record_grams: &BTreeMap<Vec<u8>, Vec<u64>>,
    record_grams_by_slot: &BTreeMap<(u32, u32), Vec<(u32, value_codec::GramKind, Vec<u64>)>>,
    file_records: &BTreeSet<Vec<u8>>,
    logical_ids: &BTreeMap<Vec<u8>, Vec<u8>>,
    domain_members: &BTreeSet<Vec<u8>>,
    outgoing: &BTreeSet<Vec<u8>>,
    incoming: &BTreeSet<Vec<u8>>,
    report: &mut ValidationReport,
) -> StoreResult<()> {
    let file_record = keys::file_record(contribution.record.file, collection, slot);
    if !file_records.contains(&file_record) {
        report.error(format!(
            "collection {collection} slot {slot} has no file-record row"
        ));
    }
    let logical = logical_ids.get(&keys::logical_id(collection, &contribution.record.id.0));
    let valid = logical.is_some_and(|bytes| {
        bytes.len() == 12
            && bytes[..4] == slot.to_be_bytes()
            && u64::from_be_bytes(bytes[4..].try_into().expect("eight bytes"))
                == contribution.record.incarnation
    });
    if !valid {
        report.error(format!(
            "collection {collection} slot {slot} logical-ID mapping mismatch"
        ));
    }
    for (binding, value) in &contribution.values {
        report.value_members = report.value_members.saturating_add(1);
        if !contains(
            value_members,
            keys::value_block(collection, *binding, value, slot >> 12)?,
            slot,
        ) {
            report.error(format!(
                "collection {collection} slot {slot} missing value membership"
            ));
        }
    }
    for (binding, length) in &contribution.lengths {
        report.length_members = report.length_members.saturating_add(1);
        if !contains(
            length_members,
            keys::length_block(
                collection,
                *binding,
                &crate::posting::ScalarKey::Number(length.clone()),
                slot >> 12,
            )?,
            slot,
        ) {
            report.error(format!(
                "collection {collection} slot {slot} missing length membership"
            ));
        }
    }
    for (binding, mask) in &contribution.state_masks {
        if *mask == 0 || *mask & !value_codec::STATE_MASK_BITS != 0 {
            report.error(format!(
                "collection {collection} slot {slot} has invalid state mask"
            ));
            continue;
        }
        for tag in 0..=value_codec::STATE_EMPTY_OBJECT {
            if mask & (1 << tag) == 0 {
                continue;
            }
            report.state_members = report.state_members.saturating_add(1);
            let state = value_codec::StateKind::from_tag(tag)
                .ok_or_else(|| StoreError::CorruptIndex("unknown state tag".into()))?;
            if !contains(
                state_members,
                keys::state_block(collection, *binding, state, slot >> 12),
                slot,
            ) {
                report.error(format!(
                    "collection {collection} slot {slot} missing state membership"
                ));
            }
        }
    }
    for domain in &contribution.domains {
        if !domain_members.contains(&keys::domain_member(collection, *domain, slot)) {
            report.error(format!(
                "collection {collection} slot {slot} has no domain membership"
            ));
        }
    }
    for (binding, kind, grams) in record_grams_by_slot
        .get(&(collection, slot))
        .into_iter()
        .flatten()
    {
        report.gram_memberships = report.gram_memberships.saturating_add(grams.len() as u64);
        let expected_key = keys::record_grams(collection, slot, *binding, *kind);
        if record_grams.get(&expected_key) != Some(grams) {
            report.error(format!(
                "collection {collection} slot {slot} reverse gram list mismatch"
            ));
        }
        for gram in grams {
            if !contains(
                fts_members,
                keys::fts_key(collection, *binding, *kind, *gram, slot >> 12),
                slot,
            ) {
                report.error(format!(
                    "collection {collection} slot {slot} gram {gram} bit is clear"
                ));
            }
        }
    }
    report.sort_entries = report
        .sort_entries
        .saturating_add(contribution.sorts.len() as u64);
    report.relation_occurrences = report
        .relation_occurrences
        .saturating_add(contribution.relations.len() as u64);
    for occurrence in &contribution.relations {
        if !outgoing.contains(&keys::relation_outgoing(
            collection,
            occurrence.declaration_ordinal,
            slot,
            occurrence.occurrence_ordinal,
        )) {
            report.error(format!(
                "collection {collection} slot {slot} missing outgoing relation"
            ));
        }
        if !incoming.contains(&keys::relation_incoming(
            occurrence.target_collection.0,
            &occurrence.target_id,
            collection,
            occurrence.declaration_ordinal,
            slot,
            occurrence.occurrence_ordinal,
        )) {
            report.error(format!(
                "collection {collection} slot {slot} missing incoming relation"
            ));
        }
    }
    Ok(())
}

fn contains(members: &BTreeMap<Vec<u8>, Block>, logical: Vec<u8>, slot: u32) -> bool {
    members
        .get(&logical)
        .is_some_and(|block| block.contains((slot & 0x0fff) as u16))
}

fn scan(
    view: &super::StoreView<'_>,
    namespace: Namespace,
    mut visit: impl FnMut(&[u8], &[u8]) -> StoreResult<()>,
) -> StoreResult<()> {
    let mut iterator = view.namespace_prefix(namespace, b"")?;
    iterator.seek_to_first();
    while iterator.valid() {
        let key = iterator
            .key()
            .ok_or_else(|| StoreError::CorruptIndex("missing key".into()))?;
        if super::namespace::namespace_of(key) != Some(namespace) {
            return Err(StoreError::CorruptIndex(
                "iterator crossed namespace boundary".into(),
            ));
        }
        visit(
            &key[1..],
            iterator
                .value()
                .ok_or_else(|| StoreError::CorruptIndex("missing value".into()))?,
        )?;
        iterator.next();
    }
    iterator.status()?;
    Ok(())
}
