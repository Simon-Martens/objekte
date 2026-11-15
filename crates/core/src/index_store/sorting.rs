//! Stored configured sort projections.
//!
//! One entry per live record per currently configured order. Both directions
//! are materialized: reversing a forward scan would reverse ties and move the
//! special-value groups incorrectly.

use std::cmp::Ordering;

use redb::WriteTransaction;

use crate::model::{CollectionId, RecordSlot};
use crate::sorting::{SortKey, SortOptions};

use super::key_codec::KeyWriter;
use super::keys;
use super::tables;
use super::value_codec::{SortDefinition, SortProjection};
use super::{StoreError, StoreResult, write};

/// Encode a complete order key. The logical ID and slot are appended by the
/// table key builder, not by this function.
pub fn encode_order_key(projection: &SortProjection, definition: &SortDefinition) -> StoreResult<Vec<u8>> {
    if projection.definition != definition.id {
        return Err(StoreError::CorruptIndex(
            "sort projection belongs to another definition".into(),
        ));
    }
    if projection.key.len() != definition.constituents.len() {
        return Err(StoreError::CorruptIndex(
            "sort projection constituent count mismatch".into(),
        ));
    }
    let mut writer = KeyWriter::with_capacity(32);
    for (key, constituent) in projection.key.iter().zip(&definition.constituents) {
        let descending = constituent.direction == super::value_codec::SortDirection::Descending;
        writer.push_sort_key(key, descending, constituent.options);
    }
    Ok(writer.into_bytes())
}

pub fn insert_sort_projection(
    tx: &WriteTransaction,
    collection: CollectionId,
    slot: RecordSlot,
    logical_id: &str,
    projections: &[SortProjection],
    definitions: &[SortDefinition],
) -> StoreResult<()> {
    let mut table = tx.open_table(tables::SORT_ENTRIES).map_err(write::map_table)?;
    for projection in projections {
        let definition = definitions
            .iter()
            .find(|definition| definition.id == projection.definition)
            .ok_or_else(|| {
                StoreError::State(format!(
                    "missing sort definition {}",
                    projection.definition
                ))
            })?;
        let order = encode_order_key(projection, definition)?;
        let key = keys::sort_entry(collection.0, projection.definition, &order, logical_id, slot.0);
        table
            .insert(key.as_slice(), &[][..])
            .map_err(write::map_storage)?;
    }
    Ok(())
}

pub fn insert_sort_projections<'a>(
    tx: &WriteTransaction,
    collection: CollectionId,
    rows: impl IntoIterator<Item = (RecordSlot, &'a str, &'a [SortProjection])>,
    definitions: &[SortDefinition],
) -> StoreResult<()> {
    let mut table = tx.open_table(tables::SORT_ENTRIES).map_err(write::map_table)?;
    for (slot, logical_id, projections) in rows {
        for projection in projections {
            let definition = definitions
                .iter()
                .find(|definition| definition.id == projection.definition)
                .ok_or_else(|| {
                    StoreError::State(format!(
                        "missing sort definition {}",
                        projection.definition
                    ))
                })?;
            let order = encode_order_key(projection, definition)?;
            let key = keys::sort_entry(
                collection.0,
                projection.definition,
                &order,
                logical_id,
                slot.0,
            );
            table
                .insert(key.as_slice(), &[][..])
                .map_err(write::map_storage)?;
        }
    }
    Ok(())
}

pub fn remove_sort_projection(
    tx: &WriteTransaction,
    collection: CollectionId,
    slot: RecordSlot,
    logical_id: &str,
    projections: &[SortProjection],
    definitions: &[SortDefinition],
) -> StoreResult<()> {
    let mut table = tx.open_table(tables::SORT_ENTRIES).map_err(write::map_table)?;
    for projection in projections {
        let definition = definitions
            .iter()
            .find(|definition| definition.id == projection.definition)
            .ok_or_else(|| {
                StoreError::State(format!(
                    "missing sort definition {}",
                    projection.definition
                ))
            })?;
        let order = encode_order_key(projection, definition)?;
        let key = keys::sort_entry(collection.0, projection.definition, &order, logical_id, slot.0);
        if table
            .remove(key.as_slice())
            .map_err(write::map_storage)?
            .is_none()
        {
            return Err(StoreError::CorruptIndex(format!(
                "missing sort entry for definition {} slot {}",
                projection.definition, slot.0
            )));
        }
    }
    Ok(())
}

/// Compare two encoded order keys bytewise. This is the same comparison redb
/// performs on `sort_entries` tree keys.
pub fn compare_encoded(left: &[u8], right: &[u8]) -> Ordering {
    left.cmp(right)
}

/// Direct comparison helper used by property tests against
/// `SortKey::compare_with`.
pub fn compare_projection(
    left: &SortProjection,
    right: &SortProjection,
    definition: &SortDefinition,
) -> StoreResult<Ordering> {
    let left = encode_order_key(left, definition)?;
    let right = encode_order_key(right, definition)?;
    Ok(compare_encoded(&left, &right))
}

pub fn sort_options_for(definition: &SortDefinition, index: usize) -> Option<SortOptions> {
    definition.constituents.get(index).map(|c| c.options)
}

pub fn first_sort_key(projection: &SortProjection) -> Option<&SortKey> {
    projection.key.first()
}
