//! Transaction-backed relation adjacency.
//!
//! Outgoing occurrences are keyed by source occurrence ordinal so repeated
//! references remain distinct. Incoming rows are keyed by the logical target
//! identity, never by a reusable target slot. Targets are resolved through the
//! snapshot's `logical_ids` table at query time.

use redb::WriteTransaction;

use crate::model::{CollectionId, RecordSlot};

use super::key_codec::{Cursor, KeyWriter};
use super::keys;
use super::tables;
use super::value_codec::{RelationOccurrence, RelationStateKind};
use super::{StoreError, StoreResult, write};

pub fn insert_relation_occurrences(
    tx: &WriteTransaction,
    collection: CollectionId,
    slot: RecordSlot,
    occurrences: &[RelationOccurrence],
) -> StoreResult<()> {
    let mut outgoing = tx
        .open_table(tables::RELATION_OUTGOING)
        .map_err(write::map_table)?;
    let mut incoming = tx
        .open_table(tables::RELATION_INCOMING)
        .map_err(write::map_table)?;
    for occurrence in occurrences {
        let key = keys::relation_outgoing(
            collection.0,
            occurrence.declaration_ordinal,
            slot.0,
            occurrence.occurrence_ordinal,
        );
        let value = encode_outgoing(occurrence)?;
        outgoing
            .insert(key.as_slice(), value.as_slice())
            .map_err(write::map_storage)?;
        let incoming_key = keys::relation_incoming(
            occurrence.target_collection.0,
            &occurrence.target_id,
            collection.0,
            occurrence.declaration_ordinal,
            slot.0,
            occurrence.occurrence_ordinal,
        );
        let incoming_value = encode_positions(&occurrence.positions);
        incoming
            .insert(incoming_key.as_slice(), incoming_value.as_slice())
            .map_err(write::map_storage)?;
    }
    Ok(())
}

pub fn remove_relation_occurrences(
    tx: &WriteTransaction,
    collection: CollectionId,
    slot: RecordSlot,
    occurrences: &[RelationOccurrence],
) -> StoreResult<()> {
    let mut outgoing = tx
        .open_table(tables::RELATION_OUTGOING)
        .map_err(write::map_table)?;
    let mut incoming = tx
        .open_table(tables::RELATION_INCOMING)
        .map_err(write::map_table)?;
    for occurrence in occurrences {
        let key = keys::relation_outgoing(
            collection.0,
            occurrence.declaration_ordinal,
            slot.0,
            occurrence.occurrence_ordinal,
        );
        outgoing.remove(key.as_slice()).map_err(write::map_storage)?;
        let incoming_key = keys::relation_incoming(
            occurrence.target_collection.0,
            &occurrence.target_id,
            collection.0,
            occurrence.declaration_ordinal,
            slot.0,
            occurrence.occurrence_ordinal,
        );
        incoming
            .remove(incoming_key.as_slice())
            .map_err(write::map_storage)?;
    }
    Ok(())
}

/// Decode one outgoing value into logical target identity and positions.
pub fn decode_outgoing(bytes: &[u8]) -> StoreResult<(CollectionId, String, Vec<u32>)> {
    let mut cursor = Cursor::new(bytes);
    let collection = CollectionId(cursor.read_u32()?);
    let target = cursor.read_str()?;
    let positions = decode_positions(&mut cursor)?;
    if !cursor.is_empty() {
        return Err(StoreError::CorruptIndex(
            "trailing bytes in outgoing relation row".into(),
        ));
    }
    Ok((collection, target, positions))
}

/// Decode one incoming value into occurrence positions.
pub fn decode_incoming(bytes: &[u8]) -> StoreResult<Vec<u32>> {
    let mut cursor = Cursor::new(bytes);
    let positions = decode_positions(&mut cursor)?;
    if !cursor.is_empty() {
        return Err(StoreError::CorruptIndex(
            "trailing bytes in incoming relation row".into(),
        ));
    }
    Ok(positions)
}

fn encode_outgoing(occurrence: &RelationOccurrence) -> StoreResult<Vec<u8>> {
    let mut writer = KeyWriter::with_capacity(8 + occurrence.target_id.len() + 8);
    writer.push_u32(occurrence.target_collection.0);
    writer.push_str(&occurrence.target_id);
    append_positions(&mut writer, &occurrence.positions);
    Ok(writer.into_bytes())
}

fn encode_positions(positions: &[u32]) -> Vec<u8> {
    let mut writer = KeyWriter::with_capacity(4 + positions.len() * 4);
    append_positions(&mut writer, positions);
    writer.into_bytes()
}

fn append_positions(writer: &mut KeyWriter, positions: &[u32]) {
    let count = u32::try_from(positions.len()).expect("relation position count fits u32");
    writer.push_u32(count);
    for position in positions {
        writer.push_u32(*position);
    }
}

fn decode_positions(cursor: &mut Cursor<'_>) -> StoreResult<Vec<u32>> {
    let count = cursor.read_u32()?;
    let mut positions = Vec::with_capacity(count.min(1 << 20) as usize);
    for _ in 0..count {
        positions.push(cursor.read_u32()?);
    }
    Ok(positions)
}

pub fn insert_relation_state(
    tx: &WriteTransaction,
    collection: CollectionId,
    declaration: u32,
    state: RelationStateKind,
    slot: RecordSlot,
) -> StoreResult<()> {
    write::put_relation_state(tx, collection, declaration, state, slot)
}

pub fn remove_relation_state(
    tx: &WriteTransaction,
    collection: CollectionId,
    declaration: u32,
    state: RelationStateKind,
    slot: RecordSlot,
) -> StoreResult<()> {
    write::remove_relation_state(tx, collection, declaration, state, slot)
}

/// Current state membership for one declaration.
pub fn state_slots(
    view: &super::StoreView,
    collection: CollectionId,
    declaration: u32,
    state: RelationStateKind,
) -> StoreResult<Vec<RecordSlot>> {
    let prefix = keys::relation_state_prefix(collection.0, declaration, state);
    let successor = super::key_codec::prefix_successor(&prefix);
    let upper = successor
        .as_deref()
        .map(std::ops::Bound::Excluded)
        .unwrap_or(std::ops::Bound::Unbounded);
    let table = view
        .read()
        .open_table(tables::RELATION_STATES)
        .map_err(write::map_table)?;
    let mut slots = Vec::new();
    for row in table
        .range::<&[u8]>((std::ops::Bound::Included(prefix.as_slice()), upper))
        .map_err(write::map_storage)?
    {
        let (key, _) = row.map_err(write::map_storage)?;
        let mut cursor = Cursor::new(key.value());
        let _collection = cursor.read_u32()?;
        let _declaration = cursor.read_u32()?;
        let _state = cursor.read_u8()?;
        slots.push(RecordSlot(cursor.read_u32()?));
    }
    Ok(slots)
}

/// Count occurrence rows for one declaration without materializing positions.
pub fn occurrence_count(
    view: &super::StoreView,
    collection: CollectionId,
    declaration: u32,
) -> StoreResult<u64> {
    Ok(super::read::outgoing_relations(view, collection, declaration)?.len() as u64)
}
