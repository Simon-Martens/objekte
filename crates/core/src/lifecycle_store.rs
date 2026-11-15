//! Explicit persistent index opening and checkpointing.

use std::path::PathBuf;
use std::time::Duration;

use crate::index_store::{IndexStore, StoreOpenError};
use crate::lifecycle::{Index, IndexError};
use crate::repository::{CollectionDefinition, Repository, RepositoryOptions};
use crate::source::SourceRegistry;

/// Where and how the index store is kept.
#[derive(Clone, Debug)]
pub struct StoreOptions {
    /// `None` means `<root>/.objekte/.index/index.redb`.
    pub path: Option<PathBuf>,
    /// `false` skips fsync on commit (tests and benchmarks).
    pub durable: bool,
}

/// What `open_persistent` did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreOpenOutcome {
    Loaded,
    Rebuilt(StoreRebuildReason),
    WithoutStore(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreRebuildReason {
    Missing,
    FormatVersion,
    Configuration,
    Damaged(String),
}

/// Counters of one checkpoint.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CheckpointReport {
    pub revision: u64,
    pub shards_written: u64,
    pub shards_deleted: u64,
    pub main_shards_updated: u64,
    pub chunks_written: u64,
    pub chunks_deleted: u64,
    pub rows_written: u64,
    pub bytes_written: u64,
    pub duration: Duration,
}

fn store_error(error: impl std::fmt::Display) -> IndexError {
    IndexError::Io(format!("index store: {error}"))
}

impl Index {
    pub fn open_persistent(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        sources: SourceRegistry,
        store: StoreOptions,
    ) -> Result<(Index, StoreOpenOutcome), IndexError> {
        let root = root.into();
        let path = store
            .path
            .clone()
            .unwrap_or_else(|| root.join(".objekte/.index/index.redb"));
        match Self::load_from_store(
            root.clone(),
            definitions.clone(),
            options,
            sources.clone(),
            store.clone(),
        )? {
            crate::lifecycle_sync::StoreLoad::Loaded(index) => {
                Ok((index, StoreOpenOutcome::Loaded))
            }
            crate::lifecycle_sync::StoreLoad::NotLoaded(reason) => {
                let index = Self::build_with_sources(root, definitions, options, sources)?;
                index.attach_store(StoreOptions {
                    path: Some(path),
                    durable: store.durable,
                })?;
                Ok((index, StoreOpenOutcome::Rebuilt(reason)))
            }
            crate::lifecycle_sync::StoreLoad::Unavailable(error) => {
                let index = Self::build_with_sources(root, definitions, options, sources)?;
                Ok((index, StoreOpenOutcome::WithoutStore(error)))
            }
        }
    }

    /// Write everything that changed since the last checkpoint.
    pub fn checkpoint(&self) -> Result<CheckpointReport, IndexError> {
        let view = self.view()?;
        if !view.initial_construction_complete() {
            return Err(IndexError::InvalidPublication(
                "initial construction is not complete".into(),
            ));
        }
        let mut guard = self
            .0
            .store
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?;
        let store = guard
            .as_mut()
            .ok_or_else(|| IndexError::Io("index store: no store is attached".into()))?;
        let report = store
            .checkpoint(&view.0, view.revision().0)
            .map_err(store_error)?;
        drop(guard);
        if let Ok(mut error) = self.0.store_sync_error.lock() {
            *error = None;
        }
        Ok(report)
    }

    /// True when this index was opened with a usable store.
    pub fn has_store(&self) -> bool {
        self.0
            .store
            .lock()
            .map(|store| store.is_some())
            .unwrap_or(false)
    }
}

#[cfg(test)]
#[path = "lifecycle_store.test.rs"]
mod tests;
