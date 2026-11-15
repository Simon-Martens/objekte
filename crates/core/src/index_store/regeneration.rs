//! Atomic deletion and recreation of every application table.
//!
//! Clears and regenerations share one implementation. Old read transactions
//! keep their previous table roots through redb MVCC until released; the latest
//! schema contains only the new generation.

use redb::{ReadableDatabase, ReadableTable, ReadableTableMetadata, TableHandle, WriteTransaction};

use super::tables::{ALL_TABLES, STORE_METADATA, is_registered_table};
use super::value_codec::{CollectionMetadata, ConstructionState, StoreMetadata};
use super::{IndexStore, StoreError, StoreResult, read, tables, write};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResetReport {
    pub old_revision: u64,
    pub new_revision: u64,
    pub old_build_epoch: u64,
    pub new_build_epoch: u64,
    pub deleted_tables: usize,
    pub recreated_tables: usize,
    /// `(collection id, old generation, new generation)`.
    pub generations: Vec<(u32, u64, u64)>,
}

pub fn reset_tables(
    tx: &WriteTransaction,
    metadata: &mut StoreMetadata,
    state: ConstructionState,
) -> StoreResult<ResetReport> {
    let old_revision = metadata.revision;
    let old_build_epoch = metadata.build_epoch;

    let collections = {
        let table = tx.open_table(tables::COLLECTIONS).map_err(write::map_table)?;
        let mut collections = Vec::new();
        for row in table.iter().map_err(write::map_storage)? {
            let (_, value) = row.map_err(write::map_storage)?;
            collections.push(super::value_codec::decode::<CollectionMetadata>(
                value.value(),
                "collection",
            )?);
        }
        collections
    };

    let names = {
        let mut names = Vec::new();
        for handle in tx.list_tables().map_err(write::map_storage)? {
            names.push(handle.name().to_string());
        }
        names
    };
    for name in &names {
        if !is_registered_table(name) {
            return Err(StoreError::CorruptIndex(format!(
                "unregistered table {name} in the index database"
            )));
        }
    }
    if names.len() != ALL_TABLES.len() {
        let missing = ALL_TABLES
            .iter()
            .filter(|(name, _)| !names.iter().any(|observed| observed == name))
            .map(|(name, _)| *name)
            .collect::<Vec<_>>();
        return Err(StoreError::CorruptIndex(format!(
            "missing registered tables: {}",
            missing.join(", ")
        )));
    }

    // Close every table guard before deleting the tables.
    for (_, definition) in ALL_TABLES {
        tx.delete_table(*definition).map_err(write::map_table)?;
    }
    for (_, definition) in ALL_TABLES {
        tx.open_table(*definition).map_err(write::map_table)?;
    }

    let mut generations = Vec::with_capacity(collections.len());
    for mut collection in collections {
        let old_generation = collection.generation;
        collection.generation = collection
            .generation
            .checked_add(1)
            .ok_or_else(|| StoreError::State("collection generation overflow".into()))?;
        collection.slot_high_water = 0;
        collection.live_records = 0;
        collection.discovered_files = 0;
        collection.pending_files = 0;
        collection.indexed_files = 0;
        collection.unavailable_files = 0;
        collection.processed_files = 0;
        collection.next_binding_id = 0;
        generations.push((collection.id.0, old_generation, collection.generation));
        write::put_collection(tx, &collection)?;
    }

    metadata.revision = metadata
        .revision
        .checked_add(1)
        .ok_or_else(|| StoreError::State("publication revision overflow".into()))?;
    metadata.build_epoch = metadata
        .build_epoch
        .checked_add(1)
        .ok_or_else(|| StoreError::State("build epoch overflow".into()))?;
    metadata.construction = state;
    metadata.catalog_complete = state == ConstructionState::Empty;
    metadata.next_file_id = 0;
    write::put_metadata(tx, metadata)?;

    Ok(ResetReport {
        old_revision,
        new_revision: metadata.revision,
        old_build_epoch,
        new_build_epoch: metadata.build_epoch,
        deleted_tables: names.len(),
        recreated_tables: ALL_TABLES.len(),
        generations,
    })
}

impl IndexStore {
    /// Delete and recreate every application table in one commit. The latest
    /// view is deliberately empty; no source file is read or deleted.
    pub fn clear_index(&self) -> StoreResult<ResetReport> {
        let tx = self.begin_write()?;
        let mut metadata = read::metadata_from_write(&tx)?;
        let report = reset_tables(&tx, &mut metadata, ConstructionState::Empty)?;
        tx.commit().map_err(StoreError::database)?;
        Ok(report)
    }

    /// Reset to a discovering generation. The caller then runs ordinary
    /// metadata discovery and batched admission.
    pub fn reset_for_regeneration(&self) -> StoreResult<ResetReport> {
        let tx = self.begin_write()?;
        let mut metadata = read::metadata_from_write(&tx)?;
        let report = reset_tables(&tx, &mut metadata, ConstructionState::Discovering)?;
        tx.commit().map_err(StoreError::database)?;
        Ok(report)
    }

    pub fn table_names(&self) -> StoreResult<Vec<String>> {
        let read = self
            .database()
            .begin_read()
            .map_err(StoreError::database)?;
        let mut names = Vec::new();
        for handle in read.list_tables().map_err(StoreError::database)? {
            names.push(handle.name().to_string());
        }
        Ok(names)
    }
}

/// Verify that the current schema is exactly the registry. Used by validation
/// and by reset before deletion.
pub fn verify_registry(tx: &WriteTransaction) -> StoreResult<bool> {
    let mut names = Vec::new();
    for handle in tx.list_tables().map_err(write::map_storage)? {
        names.push(handle.name().to_string());
    }
    Ok(names.len() == ALL_TABLES.len()
        && names.iter().all(|name| is_registered_table(name))
        && ALL_TABLES.iter().all(|(name, _)| names.iter().any(|seen| seen == name)))
}

pub fn table_row_counts(tx: &WriteTransaction) -> StoreResult<Vec<(String, u64)>> {
    let mut counts = Vec::new();
    for (name, definition) in ALL_TABLES {
        let table = tx.open_table(*definition).map_err(write::map_table)?;
        counts.push(((*name).to_string(), table.len().map_err(write::map_storage)?));
    }
    Ok(counts)
}

pub fn store_metadata(tx: &WriteTransaction) -> StoreResult<StoreMetadata> {
    let table = tx.open_table(STORE_METADATA).map_err(write::map_table)?;
    let value = table
        .get(tables::meta::FORMAT)
        .map_err(write::map_storage)?
        .ok_or_else(|| StoreError::CorruptIndex("missing store metadata".into()))?;
    super::value_codec::decode(value.value(), "store metadata")
}
