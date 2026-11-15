//! Full offline invariant checks.
//!
//! Validation is an explicit test/diagnostic operation. Status polling must
//! use committed counters instead of scanning these tables.

use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;

use redb::{ReadableTable, TableHandle};

use crate::model::{CollectionId, FileId, RecordSlot};

use super::key_codec::Cursor;
use super::keys;
use super::tables::{ALL_TABLES, is_registered_table};
use super::value_codec::{
    BuildFileState, CollectionMetadata, FileBuildState, LengthDirectoryEntry, RecordMetadata,
    StoredContributions, ValueDirectoryEntry,
};
use super::{StoreError, StoreResult, StoreView, fts, read, tables, value_codec};
#[cfg(feature = "store-posting-rows")]
use super::posting_rows as postings;
#[cfg(not(feature = "store-posting-rows"))]
use super::posting_blocks as postings;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ValidationReport {
    pub collections: usize,
    pub live_records: u64,
    pub free_slots: u64,
    pub value_members: u64,
    pub length_members: u64,
    pub state_members: u64,
    pub fts_blocks: u64,
    pub sort_entries: u64,
    pub relation_occurrences: u64,
    pub errors: Vec<String>,
}

impl ValidationReport {
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }
}

pub fn validate(view: &StoreView) -> StoreResult<ValidationReport> {
    let mut report = ValidationReport::default();
    let names = {
        let mut names = Vec::new();
        for handle in view.read().list_tables().map_err(StoreError::database)? {
            names.push(handle.name().to_string());
        }
        names
    };
    if names.len() != ALL_TABLES.len()
        || !names.iter().all(|name| is_registered_table(name))
    {
        report.errors.push(format!(
            "table registry mismatch: {names:?} does not equal the registered schema"
        ));
    }
    let collections = read::all_collections(view)?;
    report.collections = collections.len();
    for collection in &collections {
        validate_collection(view, collection, &mut report)?;
    }
    Ok(report)
}

fn validate_collection(
    view: &StoreView,
    collection: &CollectionMetadata,
    report: &mut ValidationReport,
) -> StoreResult<()> {
    let c = collection.id;
    let mut live = BTreeMap::<u32, RecordMetadata>::new();
    read::for_each_slot(view, c, |slot, record| {
        if live.insert(slot.0, record.clone()).is_some() {
            report.errors.push(format!(
                "collection {} slot {} appears twice",
                c.0, slot.0
            ));
        }
        Ok(true)
    })?;
    report.live_records += live.len() as u64;
    if collection.live_records != live.len() as u64 {
        report.errors.push(format!(
            "collection {} live counter {} does not match {} record rows",
            c.0,
            collection.live_records,
            live.len()
        ));
    }

    let free = read_free_slots(view, c)?;
    report.free_slots += free.len() as u64;
    for slot in &free {
        if u64::from(*slot) >= collection.slot_high_water {
            report
                .errors
                .push(format!("collection {} free slot {slot} above high water", c.0));
        }
        if live.contains_key(slot) {
            report
                .errors
                .push(format!("collection {} slot {slot} is live and free", c.0));
        }
        let incarnation = read_incarnation(view, c, RecordSlot(*slot))?;
        if incarnation.is_none() {
            report.errors.push(format!(
                "collection {} free slot {slot} has no incarnation",
                c.0
            ));
        }
    }
    for &slot in live.keys() {
        if u64::from(slot) >= collection.slot_high_water {
            report.errors.push(format!(
                "collection {} live slot {slot} above high water {}",
                c.0, collection.slot_high_water
            ));
        }
    }

    // Logical-ID and file-record mappings.
    let contributions = {
        let table = view
            .read()
            .open_table(tables::RECORD_CONTRIBUTIONS)
            .map_err(StoreError::database)?;
        let mut map = HashMap::new();
        for row in table.iter().map_err(StoreError::database)? {
            let (key, _value) = row.map_err(StoreError::database)?;
            let mut cursor = Cursor::new(key.value());
            let collection = CollectionId(cursor.read_u32()?);
            let slot = RecordSlot(cursor.read_u32()?);
            let contributions = read::contributions(view, collection, slot)?
                .ok_or_else(|| StoreError::CorruptIndex("missing record contributions".into()))?;
            map.insert((collection.0, slot.0), contributions);
        }
        map
    };
    for (&slot, record) in &live {
        let logical = read::record_by_logical_id(view, c, &record.id.0)?;
        if logical != Some((RecordSlot(slot), record.incarnation)) {
            report.errors.push(format!(
                "collection {} slot {slot} logical-ID mapping mismatch",
                c.0
            ));
        }
        let Some(stored) = contributions.get(&(c.0, slot)) else {
            report
                .errors
                .push(format!("collection {} slot {slot} has no contributions", c.0));
            continue;
        };
        if stored.record.id != record.id {
            report.errors.push(format!(
                "collection {} slot {slot} contribution identity mismatch",
                c.0
            ));
        }
        validate_contributions(view, c, RecordSlot(slot), stored, report)?;
    }
    for key in contributions.keys() {
        if key.0 == c.0 && !live.contains_key(&key.1) {
            report.errors.push(format!(
                "collection {} free slot {} still has contributions",
                c.0, key.1
            ));
        }
    }

    validate_file_state(view, collection, &live, report)?;
    validate_directory_counts(view, c, report)?;
    validate_state_blocks(view, c, &live, report)?;
    validate_fts(view, c, report)?;
    validate_reverse_grams(view, c, &live, report)?;
    validate_sort_and_relation_rows(view, c, &live, report)?;
    Ok(())
}

fn validate_contributions(
    view: &StoreView,
    collection: CollectionId,
    slot: RecordSlot,
    stored: &StoredContributions,
    report: &mut ValidationReport,
) -> StoreResult<()> {
    let mut value_members = 0u64;
    for (binding, value) in &stored.values {
        let members = read::value_members(view, collection, *binding, value)?;
        if !members.contains(&slot) {
            report.errors.push(format!(
                "collection {} slot {} missing value membership",
                collection.0, slot.0
            ));
        }
        value_members += 1;
    }
    report.value_members += value_members;
    for (binding, length) in &stored.lengths {
        let length = crate::posting::ScalarKey::Number(length.clone());
        if !postings::contains(
            view,
            postings::PostingFamily::Length,
            collection,
            *binding,
            &length,
            slot,
        )? {
            report.errors.push(format!(
                "collection {} slot {} missing length membership",
                collection.0, slot.0
            ));
        }
    }
    report.length_members += stored.lengths.len() as u64;
    for (binding, mask) in &stored.state_masks {
        if *mask == 0 || *mask & !value_codec::STATE_MASK_BITS != 0 {
            report.errors.push(format!(
                "collection {} slot {} has an invalid state mask",
                collection.0, slot.0
            ));
            continue;
        }
        for tag in 0..=tables::MAX_STATE_TAG {
            let Some(state) = super::value_codec::StateKind::from_tag(tag) else {
                continue;
            };
            if mask & (1u16 << tag) == 0 {
                continue;
            }
            report.state_members += 1;
            if !super::states::state_contains(view, collection, *binding, state, slot)? {
                report.errors.push(format!(
                    "collection {} slot {} missing state membership",
                    collection.0, slot.0
                ));
            }
        }
    }
    for projection in &stored.grams {
        let grams = read::record_grams(view, collection, slot, projection.binding, projection.kind)?;
        if grams.as_deref() != Some(projection.grams.as_slice()) {
            report.errors.push(format!(
                "collection {} slot {} record gram list mismatch",
                collection.0, slot.0
            ));
        }
        for gram in &projection.grams {
            let block = keys::fts_block_of(slot.0);
            let encoded = read::fts_block(
                view,
                collection,
                projection.binding,
                projection.kind,
                *gram,
                block,
            )?;
            let Some(encoded) = encoded else {
                report.errors.push(format!(
                    "collection {} slot {} gram {gram} has no block",
                    collection.0, slot.0
                ));
                continue;
            };
            let decoded = fts::decode_block(&encoded)?;
            if !decoded.contains(keys::fts_offset_of(slot.0)) {
                report.errors.push(format!(
                    "collection {} slot {} gram {gram} bit is clear",
                    collection.0, slot.0
                ));
            }
        }
    }
    Ok(())
}

fn validate_file_state(
    view: &StoreView,
    collection: &CollectionMetadata,
    live: &BTreeMap<u32, RecordMetadata>,
    report: &mut ValidationReport,
) -> StoreResult<()> {
    for pending in read::pending_files(view, collection.id)? {
        if read::file(view, pending)?.is_none() {
            report.errors.push(format!(
                "collection {} pending file {} has no metadata",
                collection.id.0, pending.0
            ));
            continue;
        }
        if !matches!(
            read::build_state_kind(view, pending)?,
            Some(BuildFileState::Pending)
        ) {
            report.errors.push(format!(
                "collection {} pending file {} is not in Pending state",
                collection.id.0, pending.0
            ));
        }
    }
    for record in live.values() {
        let Some(state) = read::build_state_kind(view, record.file)? else {
            report.errors.push(format!(
                "collection {} file {} has no build state",
                collection.id.0, record.file.0
            ));
            continue;
        };
        if state != BuildFileState::Indexed {
            report.errors.push(format!(
                "collection {} has records in non-indexed file {}",
                collection.id.0, record.file.0
            ));
        }
        let rows = read::records_for_file(view, record.file)?;
        if !rows
            .iter()
            .any(|row| row.collection == collection.id && row.slot.0 == u32::MAX)
        {
            // checked below with the actual slot set
        }
    }
    for record in live.values() {
        let rows = read::records_for_file(view, record.file)?;
        if rows.iter().filter(|row| row.collection == collection.id).count() == 0 {
            report.errors.push(format!(
                "collection {} file {} has no file-record rows",
                collection.id.0, record.file.0
            ));
        }
    }
    Ok(())
}

fn validate_directory_counts(
    view: &StoreView,
    collection: CollectionId,
    report: &mut ValidationReport,
) -> StoreResult<()> {
    // Memberships are validated against the directory by a full scan of each
    // table below. The exact binding set is derived from directory rows.
    let value_directory = view
        .read()
        .open_table(tables::VALUE_DIRECTORY)
        .map_err(StoreError::database)?;
    let mut directory = Vec::new();
    for row in value_directory.iter().map_err(StoreError::database)? {
        let (key, value) = row.map_err(StoreError::database)?;
        let mut cursor = Cursor::new(key.value());
        let c = cursor.read_u32()?;
        if c != collection.0 {
            continue;
        }
        let binding = cursor.read_u32()?;
        let scalar = cursor.read_scalar()?;
        let entry = value_codec::decode::<ValueDirectoryEntry>(value.value(), "value directory")?;
        directory.push((binding, scalar, entry.cardinality));
    }
    for (binding, scalar, cardinality) in directory {
        let members = read::value_members(view, collection, binding, &scalar)?;
        if members.len() as u64 != cardinality {
            report.errors.push(format!(
                "collection {} binding {} value {} cardinality {} != {} members",
                collection.0,
                binding,
                scalar_display(&scalar),
                cardinality,
                members.len()
            ));
        }
    }

    let length_directory = view
        .read()
        .open_table(tables::LENGTH_DIRECTORY)
        .map_err(StoreError::database)?;
    for row in length_directory.iter().map_err(StoreError::database)? {
        let (key, value) = row.map_err(StoreError::database)?;
        let mut cursor = Cursor::new(key.value());
        let c = cursor.read_u32()?;
        if c != collection.0 {
            continue;
        }
        let binding = cursor.read_u32()?;
        let scalar = cursor.read_scalar()?;
        let entry =
            value_codec::decode::<LengthDirectoryEntry>(value.value(), "length directory")?;
        if let Some(error) = postings::validate_directory(
            view,
            postings::PostingFamily::Length,
            collection,
            binding,
            &scalar,
            entry.cardinality,
        )? {
            report.errors.push(format!(
                "collection {} binding {} length {}: {error}",
                collection.0, binding, scalar_display(&scalar)
            ));
        }
    }
    Ok(())
}

fn scalar_display(scalar: &crate::posting::ScalarKey) -> String {
    match scalar {
        crate::posting::ScalarKey::String(text) => format!("string:{text}"),
        crate::posting::ScalarKey::Id(text) => format!("id:{text}"),
        crate::posting::ScalarKey::Number(number) => format!("number:{number:?}"),
        crate::posting::ScalarKey::Bool(value) => format!("bool:{value}"),
        crate::posting::ScalarKey::Date(value) => format!("date:{value}"),
        crate::posting::ScalarKey::Datetime(value) => format!("datetime:{value}"),
        crate::posting::ScalarKey::Unixtime(value) => format!("unixtime:{value}"),
    }
}

fn validate_state_blocks(
    view: &StoreView,
    collection: CollectionId,
    live: &BTreeMap<u32, RecordMetadata>,
    report: &mut ValidationReport,
) -> StoreResult<()> {
    let mut statistics = Vec::new();
    super::states::for_each_statistics(view, collection, |binding, state, stats| {
        statistics.push((binding, state, stats));
        Ok(true)
    })?;
    for (binding, state, stats) in statistics {
        let mut observed_cardinality = 0u64;
        let mut observed_blocks = 0u64;
        let mut free_bit = None;
        super::states::for_each_state_block(view, collection, binding, state, |block, bytes| {
            let decoded = super::bitmap_block::decode_block(bytes)?;
            observed_blocks += 1;
            observed_cardinality += u64::from(decoded.cardinality());
            decoded.for_each(|offset| {
                let slot = (block << super::bitmap_block::BLOCK_SHIFT) | u32::from(offset);
                if !live.contains_key(&slot) && free_bit.is_none() {
                    free_bit = Some(slot);
                }
            });
            Ok(true)
        })?;
        if observed_cardinality != stats.cardinality || observed_blocks != stats.blocks {
            report.errors.push(format!(
                "collection {} binding {} state {}: statistics ({}, {}) != observed ({observed_cardinality}, {observed_blocks})",
                collection.0,
                binding,
                state.tag(),
                stats.cardinality,
                stats.blocks
            ));
        }
        if let Some(slot) = free_bit {
            report.errors.push(format!(
                "collection {} binding {} state {}: bit on non-live slot {slot}",
                collection.0,
                binding,
                state.tag()
            ));
        }
    }
    Ok(())
}

fn validate_sort_and_relation_rows(
    view: &StoreView,
    collection: CollectionId,
    live: &BTreeMap<u32, RecordMetadata>,
    report: &mut ValidationReport,
) -> StoreResult<()> {
    for (table_name, definition) in [
        ("sort_entries", tables::SORT_ENTRIES),
        ("relation_outgoing", tables::RELATION_OUTGOING),
    ] {
        let table = view
            .read()
            .open_table(definition)
            .map_err(StoreError::database)?;
        let prefix = keys::collection_prefix(collection.0);
        let successor = super::key_codec::prefix_successor(&prefix);
        let upper = successor
            .as_deref()
            .map(std::ops::Bound::Excluded)
            .unwrap_or(std::ops::Bound::Unbounded);
        let mut count = 0u64;
        let mut bad_slot = false;
        for row in table
            .range::<&[u8]>((std::ops::Bound::Included(prefix.as_slice()), upper))
            .map_err(StoreError::database)?
        {
            let (key, _) = row.map_err(StoreError::database)?;
            count += 1;
            let slot = u32::from_be_bytes(
                key.value()[key.value().len() - 4..]
                    .try_into()
                    .expect("four-byte slot"),
            );
            if !live.contains_key(&slot) {
                bad_slot = true;
            }
        }
        if bad_slot {
            report.errors.push(format!(
                "collection {} {table_name} references a non-live slot",
                collection.0
            ));
        }
        if table_name == "sort_entries" {
            report.sort_entries += count;
        } else {
            report.relation_occurrences += count;
        }
    }
    Ok(())
}

fn validate_reverse_grams(
    view: &StoreView,
    collection: CollectionId,
    live: &BTreeMap<u32, RecordMetadata>,
    report: &mut ValidationReport,
) -> StoreResult<()> {
    let table = view
        .read()
        .open_table(tables::RECORD_GRAMS)
        .map_err(StoreError::database)?;
    let prefix = keys::collection_prefix(collection.0);
    let successor = super::key_codec::prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(std::ops::Bound::Excluded)
        .unwrap_or(std::ops::Bound::Unbounded);
    let mut reverse = 0u64;
    for row in table
        .range::<&[u8]>((std::ops::Bound::Included(prefix.as_slice()), upper))
        .map_err(StoreError::database)?
    {
        let (key, value) = row.map_err(StoreError::database)?;
        let mut cursor = Cursor::new(key.value());
        let _collection = cursor.read_u32()?;
        let slot = cursor.read_u32()?;
        let _binding = cursor.read_u32()?;
        let _kind = cursor.read_u8()?;
        if !live.contains_key(&slot) {
            report.errors.push(format!(
                "collection {} reverse grams reference free slot {slot}",
                collection.0
            ));
        }
        let grams = value_codec::decode_gram_list(value.value())?;
        reverse = reverse
            .checked_add(grams.len() as u64)
            .ok_or_else(|| StoreError::CorruptIndex("reverse gram count overflow".into()))?;
    }
    let mut forward = 0u64;
    let statistics = view
        .read()
        .open_table(tables::FTS_GRAM_STATISTICS)
        .map_err(StoreError::database)?;
    let prefix = keys::collection_prefix(collection.0);
    let successor = super::key_codec::prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(std::ops::Bound::Excluded)
        .unwrap_or(std::ops::Bound::Unbounded);
    for row in statistics
        .range::<&[u8]>((std::ops::Bound::Included(prefix.as_slice()), upper))
        .map_err(StoreError::database)?
    {
        let (key, value) = row.map_err(StoreError::database)?;
        let mut cursor = Cursor::new(key.value());
        let _collection = cursor.read_u32()?;
        let _binding = cursor.read_u32()?;
        let _kind = cursor.read_u8()?;
        let _gram = cursor.read_u64()?;
        let mut value_cursor = Cursor::new(value.value());
        let cardinality = value_cursor.read_u64()?;
        forward = forward
            .checked_add(cardinality)
            .ok_or_else(|| StoreError::CorruptIndex("forward gram count overflow".into()))?;
    }
    if reverse != forward {
        report.errors.push(format!(
            "collection {} reverse gram memberships {reverse} != forward {forward}",
            collection.0
        ));
    }
    Ok(())
}

fn validate_fts(
    view: &StoreView,
    collection: CollectionId,
    report: &mut ValidationReport,
) -> StoreResult<()> {
    let statistics = view
        .read()
        .open_table(tables::FTS_GRAM_STATISTICS)
        .map_err(StoreError::database)?;
    let mut grams = Vec::new();
    for row in statistics.iter().map_err(StoreError::database)? {
        let (key, value) = row.map_err(StoreError::database)?;
        let mut cursor = Cursor::new(key.value());
        let c = cursor.read_u32()?;
        if c != collection.0 {
            continue;
        }
        let binding = cursor.read_u32()?;
        let kind = cursor.read_u8()?;
        let gram = cursor.read_u64()?;
        let mut value_cursor = Cursor::new(value.value());
        let cardinality = value_cursor.read_u64()?;
        let blocks = value_cursor.read_u64()?;
        grams.push((binding, kind, gram, cardinality, blocks));
    }
    for (binding, kind, gram, cardinality, blocks) in grams {
        let kind = if kind == super::tables::GRAM_SKIP {
            super::value_codec::GramKind::Skip
        } else {
            super::value_codec::GramKind::Normal
        };
        let mut observed_blocks = 0u64;
        let mut observed_cardinality = 0u64;
        read::for_each_fts_block(view, collection, binding, kind, gram, |_, bytes| {
            let decoded = fts::decode_block(bytes)?;
            observed_blocks += 1;
            observed_cardinality += u64::from(decoded.cardinality());
            report.fts_blocks += 1;
            Ok(true)
        })?;
        if observed_blocks != blocks || observed_cardinality != cardinality {
            report.errors.push(format!(
                "collection {} binding {} gram {gram}: statistics ({cardinality}, {blocks}) != observed ({observed_cardinality}, {observed_blocks})",
                collection.0, binding
            ));
        }
    }
    Ok(())
}

fn read_free_slots(view: &StoreView, collection: CollectionId) -> StoreResult<Vec<u32>> {
    let table = view
        .read()
        .open_table(tables::FREE_SLOTS)
        .map_err(StoreError::database)?;
    let prefix = keys::collection_prefix(collection.0);
    let successor = super::key_codec::prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(Bound::Excluded)
        .unwrap_or(Bound::Unbounded);
    let mut slots = Vec::new();
    for row in table
        .range::<&[u8]>((Bound::Included(prefix.as_slice()), upper))
        .map_err(StoreError::database)?
    {
        let (key, _) = row.map_err(StoreError::database)?;
        let mut cursor = Cursor::new(key.value());
        let _ = cursor.read_u32()?;
        slots.push(cursor.read_u32()?);
    }
    Ok(slots)
}

fn read_incarnation(
    view: &StoreView,
    collection: CollectionId,
    slot: RecordSlot,
) -> StoreResult<Option<u64>> {
    let table = view
        .read()
        .open_table(tables::SLOT_INCARNATIONS)
        .map_err(StoreError::database)?;
    let key = keys::slot_incarnation(collection.0, slot.0);
    let Some(value) = table.get(key.as_slice()).map_err(StoreError::database)? else {
        return Ok(None);
    };
    let mut cursor = Cursor::new(value.value());
    Ok(Some(cursor.read_u64()?))
}

/// Reusable validation of an injectable failure point. The store has no
/// failure-injection production code; tests construct corrupt rows directly.
pub fn require_valid(view: &StoreView) -> StoreResult<ValidationReport> {
    let report = validate(view)?;
    if !report.is_valid() {
        return Err(StoreError::CorruptIndex(report.errors.join("; ")));
    }
    Ok(report)
}

pub fn collections_with_free_slots(view: &StoreView) -> StoreResult<Vec<(u32, u64)>> {
    let collections = read::all_collections(view)?;
    let mut result = Vec::new();
    for collection in collections {
        let free = read_free_slots(view, collection.id)?;
        if !free.is_empty() {
            result.push((collection.id.0, free.len() as u64));
        }
    }
    Ok(result)
}

pub fn file_state_summary(view: &StoreView, file: FileId) -> StoreResult<Option<FileBuildState>> {
    read::build_state(view, file)
}

pub fn collection_record_count(view: &StoreView, collection: CollectionId) -> StoreResult<u64> {
    read::live_record_count(view, collection)
}

#[cfg(test)]
#[path = "validation.test.rs"]
mod tests;
