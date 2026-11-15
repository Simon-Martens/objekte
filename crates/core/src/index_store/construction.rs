//! Construction catalog, admission state and progress counters.
//!
//! Discovery writes file/path metadata and pending membership in bounded
//! transactions. A file becomes searchable through the source evaluator as
//! soon as the catalog is complete, even while zero files are indexed. Content
//! admission atomically moves one file from pending to indexed.

use redb::WriteTransaction;

use crate::inventory::FileEntry;
use crate::model::{CollectionId, FileId};

use super::value_codec::{
    BuildFileState, CollectionMetadata, ConstructionState, FileBuildState, StoreMetadata,
};
use super::{StoreError, StoreResult, write};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConstructionCounters {
    pub discovered_files: u64,
    pub pending_files: u64,
    pub indexed_files: u64,
    pub unavailable_files: u64,
    pub processed_files: u64,
    pub live_records: u64,
}

impl ConstructionCounters {
    pub fn from_collection(collection: &CollectionMetadata) -> Self {
        Self {
            discovered_files: collection.discovered_files,
            pending_files: collection.pending_files,
            indexed_files: collection.indexed_files,
            unavailable_files: collection.unavailable_files,
            processed_files: collection.processed_files,
            live_records: collection.live_records,
        }
    }

    pub fn apply_to(&self, collection: &mut CollectionMetadata) {
        collection.discovered_files = self.discovered_files;
        collection.pending_files = self.pending_files;
        collection.indexed_files = self.indexed_files;
        collection.unavailable_files = self.unavailable_files;
        collection.processed_files = self.processed_files;
        collection.live_records = self.live_records;
    }

    pub fn observe_discovered(&mut self, count: u64) {
        self.discovered_files = self.discovered_files.saturating_add(count);
        self.pending_files = self.pending_files.saturating_add(count);
    }
}

pub fn set_construction(
    tx: &WriteTransaction,
    metadata: &mut StoreMetadata,
    state: ConstructionState,
    catalog_complete: bool,
) -> StoreResult<()> {
    metadata.construction = state;
    metadata.catalog_complete = catalog_complete;
    if matches!(
        state,
        ConstructionState::Empty | ConstructionState::Discovering | ConstructionState::Building
    ) {
        metadata.build_epoch = metadata
            .build_epoch
            .checked_add(1)
            .ok_or_else(|| StoreError::State("build epoch overflow".into()))?;
    }
    metadata.revision = metadata
        .revision
        .checked_add(1)
        .ok_or_else(|| StoreError::State("publication revision overflow".into()))?;
    write::put_metadata(tx, metadata)
}

pub fn check_epoch(metadata: &StoreMetadata, epoch: u64) -> StoreResult<()> {
    if metadata.build_epoch != epoch {
        return Err(StoreError::StaleHandle(format!(
            "build epoch {epoch} is not the current epoch {}",
            metadata.build_epoch
        )));
    }
    Ok(())
}

/// Record one discovered source file and its pending membership.
pub fn discover_file(
    tx: &WriteTransaction,
    collection: &mut CollectionMetadata,
    entry: &FileEntry,
    initial_version: u64,
    epoch: u64,
    revision: u64,
) -> StoreResult<()> {
    write::put_file(tx, entry, initial_version, None)?;
    write::put_build_state(
        tx,
        entry.id,
        &FileBuildState {
            state: BuildFileState::Pending,
            expected_file_version: initial_version,
            build_epoch: epoch,
            last_admission_revision: revision,
            error: None,
        },
    )?;
    write::add_pending_file(tx, collection.id, entry.id)?;
    collection.discovered_files = collection
        .discovered_files
        .checked_add(1)
        .ok_or_else(|| StoreError::State("discovered file count overflow".into()))?;
    collection.pending_files = collection
        .pending_files
        .checked_add(1)
        .ok_or_else(|| StoreError::State("pending file count overflow".into()))?;
    Ok(())
}

/// Atomically move one file from pending to indexed. Record rows, slots and
/// every index family must already have been written in this transaction.
pub fn mark_indexed(
    tx: &WriteTransaction,
    collection: &mut CollectionMetadata,
    file: FileId,
    file_version: u64,
    epoch: u64,
    revision: u64,
) -> StoreResult<()> {
    write::remove_pending_file(tx, collection.id, file)?;
    write::put_build_state(
        tx,
        file,
        &FileBuildState {
            state: BuildFileState::Indexed,
            expected_file_version: file_version,
            build_epoch: epoch,
            last_admission_revision: revision,
            error: None,
        },
    )?;
    collection.pending_files = collection
        .pending_files
        .checked_sub(1)
        .ok_or_else(|| StoreError::State("pending file count underflow".into()))?;
    collection.indexed_files = collection
        .indexed_files
        .checked_add(1)
        .ok_or_else(|| StoreError::State("indexed file count overflow".into()))?;
    collection.processed_files = collection
        .processed_files
        .checked_add(1)
        .ok_or_else(|| StoreError::State("processed file count overflow".into()))?;
    Ok(())
}

pub fn mark_unavailable(
    tx: &WriteTransaction,
    collection: &mut CollectionMetadata,
    file: FileId,
    file_version: u64,
    epoch: u64,
    revision: u64,
    error: String,
) -> StoreResult<()> {
    write::remove_pending_file(tx, collection.id, file)?;
    write::put_build_state(
        tx,
        file,
        &FileBuildState {
            state: BuildFileState::Unavailable,
            expected_file_version: file_version,
            build_epoch: epoch,
            last_admission_revision: revision,
            error: Some(error),
        },
    )?;
    collection.pending_files = collection
        .pending_files
        .checked_sub(1)
        .ok_or_else(|| StoreError::State("pending file count underflow".into()))?;
    collection.unavailable_files = collection
        .unavailable_files
        .checked_add(1)
        .ok_or_else(|| StoreError::State("unavailable file count overflow".into()))?;
    collection.processed_files = collection
        .processed_files
        .checked_add(1)
        .ok_or_else(|| StoreError::State("processed file count overflow".into()))?;
    Ok(())
}

/// Remove all pending memberships for a collection without touching records.
pub fn drain_pending(tx: &WriteTransaction, collection: CollectionId) -> StoreResult<u64> {
    let files = super::read::pending_files_for_write(tx, collection)?;
    for file in &files {
        write::remove_pending_file(tx, collection, *file)?;
    }
    Ok(files.len() as u64)
}
