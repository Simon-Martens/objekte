//! Explicit persistent index opening and checkpointing.

use std::path::PathBuf;
use std::time::Duration;

use crate::index_store::{IndexStore, StoreOpenError};
use crate::inventory::FileVersions;
use crate::lifecycle::{Index, IndexError, IndexRevision, PublishedIndex};
use crate::lifecycle_catalog::FileCatalog;
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
        let definitions = crate::repository_configuration::prepare_source_definitions(
            &root,
            &definitions,
        )
        .map_err(IndexError::IncompatibleConfiguration)?;
        let configuration = crate::configuration::configuration_hash(
            &definitions,
            options,
            &sources,
        )
        .map_err(IndexError::IncompatibleConfiguration)?;
        let path = store
            .path
            .clone()
            .unwrap_or_else(|| root.join(".objekte/.index/index.redb"));
        match IndexStore::open(&path, store.durable) {
            Ok(index_store) => match index_store.validate(
                &configuration,
                crate::posting_map::POSTING_CHUNK_BITS,
            ) {
                Ok(()) => match assemble_loaded_index(
                    index_store,
                    root.clone(),
                    definitions.clone(),
                    options,
                    sources.clone(),
                ) {
                    Ok(index) => Ok((index, StoreOpenOutcome::Loaded)),
                    Err(_error) => {
                        let _ = std::fs::remove_file(&path);
                        Self::rebuild_with_store(
                            root,
                            definitions,
                            options,
                            sources,
                            path,
                            store.durable,
                            StoreRebuildReason::Damaged("load failed".into()),
                        )
                    }
                },
                Err(reason) => {
                    let _ = std::fs::remove_file(&path);
                    Self::rebuild_with_store(
                        root, definitions, options, sources, path, store.durable, reason,
                    )
                }
            },
            Err(StoreOpenError::Locked(error)) => {
                let index = Self::build_with_sources(root, definitions, options, sources)?;
                Ok((index, StoreOpenOutcome::WithoutStore(error)))
            }
            Err(StoreOpenError::Damaged(error)) => {
                let _ = std::fs::remove_file(&path);
                Self::rebuild_with_store(
                    root,
                    definitions,
                    options,
                    sources,
                    path,
                    store.durable,
                    StoreRebuildReason::Damaged(error),
                )
            }
        }
    }

    fn rebuild_with_store(
        root: PathBuf,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        sources: SourceRegistry,
        path: PathBuf,
        durable: bool,
        reason: StoreRebuildReason,
    ) -> Result<(Index, StoreOpenOutcome), IndexError> {
        let index = Self::build_with_sources(
            root,
            definitions,
            options,
            sources,
        )?;
        let mut store = match IndexStore::open(&path, durable) {
            Ok(store) => store,
            Err(StoreOpenError::Damaged(error)) | Err(StoreOpenError::Locked(error)) => {
                return Ok((index, StoreOpenOutcome::WithoutStore(error)));
            }
        };
        let view = index.view()?;
        store
            .checkpoint(&view.0, view.revision().0)
            .map_err(store_error)?;
        *index
            .0
            .store
            .lock()
            .map_err(|_| IndexError::LockPoisoned)? = Some(store);
        Ok((index, StoreOpenOutcome::Rebuilt(reason)))
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
        store
            .checkpoint(&view.0, view.revision().0)
            .map_err(store_error)
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

pub(crate) fn assemble_loaded_index(
    store: IndexStore,
    root: PathBuf,
    definitions: Vec<CollectionDefinition>,
    options: RepositoryOptions,
    sources: SourceRegistry,
) -> Result<Index, String> {
    let contents = store.load_contents()?;
    let mut repository =
        Repository::empty_for_configuration(root, definitions, options, &sources)?;
    for (number, summary) in contents.global_inference.iter().enumerate() {
        if number < repository.collections().len() {
            repository.set_global_inference(number, summary.clone())?;
        }
    }
    let repository = std::sync::Arc::new(repository);
    let instance_id = crate::repository::next_repository_instance();
    let mut published = PublishedIndex::from_repository(
        instance_id,
        IndexRevision(contents.revision),
        std::sync::Arc::clone(&repository),
    )
    .map_err(|error| error.to_string())?;
    published.catalog = FileCatalog::restored(
        std::sync::Arc::clone(&repository),
        contents.catalog,
    )?;
    published.file_versions = FileVersions::initial(published.catalog.allocated_files());
    published.next_segment_ordinal = contents.next_segment_ordinal;
    for (ordinal, shard, active) in contents.shards {
        published = published
            .with_restored_segment(shard, active, ordinal)
            .map_err(|error| error.to_string())?;
    }
    let index = Index::from_published(published);
    *index
        .0
        .store
        .lock()
        .map_err(|_| "index store: lock poisoned".to_string())? = Some(store);
    index.start_reconcile().map_err(|error| error.to_string())?;
    Ok(index)
}

#[cfg(test)]
#[path = "lifecycle_store.test.rs"]
mod tests;
