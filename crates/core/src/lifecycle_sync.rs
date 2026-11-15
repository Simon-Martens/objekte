//! Loading, attaching and observing a persistent index store.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::index_store::{IndexStore, StoreOpenError};
use crate::inventory::FileVersions;
use crate::lifecycle::{Index, IndexError, IndexRevision, PublishedIndex};
use crate::lifecycle_catalog::FileCatalog;
use crate::lifecycle_store::{CheckpointReport, StoreOptions, StoreRebuildReason};
use crate::repository::{
    CollectionDefinition, Repository, RepositoryOptions,
};
use crate::source::SourceRegistry;

/// Result of trying to load an index from a store file.
pub enum StoreLoad {
    /// Loaded; reconciliation has been started.
    Loaded(Index),
    /// The file is missing or cannot be used for this configuration. Build
    /// from source, then call `attach_store`.
    NotLoaded(StoreRebuildReason),
    /// The store cannot be used at all right now (for example it is locked
    /// by another process). Build from source and stay in memory.
    Unavailable(String),
}

/// Cheap description of an attached store.
#[derive(Clone, Debug)]
pub struct StoreStatus {
    pub path: PathBuf,
    pub published_revision: u64,
    pub checkpoint_revision: Option<u64>,
    pub last_checkpoint: Option<SystemTime>,
    pub last_error: Option<String>,
    pub sync_running: bool,
    pub file_bytes: Option<u64>,
}

pub const DEFAULT_STORE_SYNC_INTERVAL: Duration = Duration::from_secs(10);

fn store_error(error: impl std::fmt::Display) -> IndexError {
    IndexError::Io(format!("index store: {error}"))
}

/// The configuration identity the store will be validated against.
pub fn configuration_identity_for(
    root: &Path,
    definitions: &[CollectionDefinition],
    options: RepositoryOptions,
    sources: &SourceRegistry,
) -> Result<[u8; 32], IndexError> {
    let definitions =
        crate::repository_configuration::prepare_source_definitions(root, definitions)
            .map_err(IndexError::IncompatibleConfiguration)?;
    crate::configuration::configuration_hash(&definitions, options, sources)
        .map_err(IndexError::IncompatibleConfiguration)
}

fn is_repository_directory(path: &Path) -> bool {
    path.parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == ".index")
        && path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .is_some_and(|name| name == ".objekte")
}

fn helper_files(path: &Path, root: &Path) -> Result<(), String> {
    let Some(directory) = path.parent() else {
        return Ok(());
    };
    if is_repository_directory(path) {
        let ignore = directory.join(".gitignore");
        if !ignore.exists() {
            std::fs::write(ignore, b"*\n").map_err(|error| error.to_string())?;
        }
    } else {
        let origin = directory.join("origin.txt");
        if !origin.exists() {
            let mut text = root.to_string_lossy().into_owned();
            text.push('\n');
            std::fs::write(origin, text).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn is_store_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".redb") else {
        return false;
    };
    stem.len() == 16 && stem.chars().all(|character| character.is_ascii_hexdigit())
}

fn prune(path: &Path) {
    let Some(directory) = path.parent() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut candidates = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if !is_store_name(&name) {
                return None;
            }
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            Some((entry.path(), modified))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then_with(|| right.0.cmp(&left.0))
    });
    let mut kept_new = false;
    let mut kept = 0usize;
    for (candidate, _) in candidates {
        if candidate == path && !kept_new {
            kept_new = true;
            kept += 1;
            continue;
        }
        if kept < 3 {
            kept += 1;
            continue;
        }
        let _ = std::fs::remove_file(candidate);
    }
}

impl Index {
    /// Load only. Never parses a source file and never creates a file.
    pub fn load_from_store(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        sources: SourceRegistry,
        store: StoreOptions,
    ) -> Result<StoreLoad, IndexError> {
        let root = root.into();
        let definitions =
            crate::repository_configuration::prepare_source_definitions(&root, &definitions)
                .map_err(IndexError::IncompatibleConfiguration)?;
        let configuration =
            crate::configuration::configuration_hash(&definitions, options, &sources)
                .map_err(IndexError::IncompatibleConfiguration)?;
        let path = store
            .path
            .clone()
            .unwrap_or_else(|| root.join(".objekte/.index/index.redb"));
        if !path.is_file() {
            return Ok(StoreLoad::NotLoaded(StoreRebuildReason::Missing));
        }
        let index_store = match IndexStore::open(&path, store.durable) {
            Ok(store) => store,
            Err(StoreOpenError::Locked(error)) => return Ok(StoreLoad::Unavailable(error)),
            Err(StoreOpenError::Damaged(error)) => {
                return Ok(StoreLoad::NotLoaded(StoreRebuildReason::Damaged(error)))
            }
        };
        if let Err(reason) = index_store.validate(
            &configuration,
            crate::posting_map::POSTING_CHUNK_BITS,
        ) {
            return Ok(StoreLoad::NotLoaded(reason));
        }
        let started = std::time::Instant::now();
        match assemble_loaded_index(index_store, root, definitions, options, sources) {
            Ok(index) => {
                let elapsed = started.elapsed().as_millis() as u64;
                index.update_timings_ms(|timings| {
                    timings.metadata = Some(elapsed);
                    timings.search_available = Some(elapsed);
                    timings.full_available = Some(elapsed);
                });
                Ok(StoreLoad::Loaded(index))
            }
            Err(error) => Ok(StoreLoad::NotLoaded(StoreRebuildReason::Damaged(error))),
        }
    }

    /// Attach a store to a complete live index and write everything.
    pub fn attach_store(&self, store: StoreOptions) -> Result<CheckpointReport, IndexError> {
        let view = self.view()?;
        if !view.initial_construction_complete() {
            return Err(IndexError::InvalidPublication(
                "initial construction is not complete".into(),
            ));
        }
        if self.has_store() {
            return Err(IndexError::Io(
                "index store: a store is already attached".into(),
            ));
        }
        let root = view.repository().root().to_path_buf();
        let path = store
            .path
            .unwrap_or_else(|| root.join(".objekte/.index/index.redb"));
        let _ = std::fs::remove_file(&path);
        let index_store = match IndexStore::open(&path, store.durable) {
            Ok(store) => store,
            Err(StoreOpenError::Locked(error) | StoreOpenError::Damaged(error)) => {
                return Err(store_error(error))
            }
        };
        helper_files(&path, &root).map_err(store_error)?;
        let report = {
            let mut guard = self
                .0
                .store
                .lock()
                .map_err(|_| IndexError::LockPoisoned)?;
            *guard = Some(index_store);
            self.checkpoint()?
        };
        prune(&path);
        Ok(report)
    }

    /// Stop using the store. The file stays on disk.
    pub fn detach_store(&self) -> Result<(), IndexError> {
        let mut store = self
            .0
            .store
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?;
        *store = None;
        Ok(())
    }

    pub fn store_status(&self) -> Result<Option<StoreStatus>, IndexError> {
        let view = self.view()?;
        let store = self
            .0
            .store
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?;
        let Some(store) = store.as_ref() else {
            return Ok(None);
        };
        let file_bytes = std::fs::metadata(&store.path)
            .ok()
            .map(|metadata| metadata.len());
        Ok(Some(StoreStatus {
            path: store.path.clone(),
            published_revision: view.revision().0,
            checkpoint_revision: Some(store.revision()),
            last_checkpoint: store.last_checkpoint(),
            last_error: self
                .0
                .store_sync_error
                .lock()
                .map_err(|_| IndexError::LockPoisoned)?
                .clone(),
            sync_running: self
                .0
                .store_sync
                .lock()
                .map_err(|_| IndexError::LockPoisoned)?
                .is_some(),
            file_bytes,
        }))
    }

    /// `checkpoint()` only if the published revision differs from the last
    /// checkpointed one. `Ok(None)` when there is no store or nothing changed.
    pub fn checkpoint_if_changed(&self) -> Result<Option<CheckpointReport>, IndexError> {
        let view = self.view()?;
        if !view.initial_construction_complete() {
            return Ok(None);
        }
        {
            let store = self
                .0
                .store
                .lock()
                .map_err(|_| IndexError::LockPoisoned)?;
            let Some(store) = store.as_ref() else {
                return Ok(None);
            };
            if store.revision() == view.revision().0 {
                return Ok(None);
            }
        }
        self.checkpoint().map(Some)
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
    let mut store = store;
    store.install_loaded(&contents.shards, &contents.catalog);
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
    for (ordinal, _store_id, shard, active) in contents.shards {
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
