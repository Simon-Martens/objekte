//! One cancellable initial construction and its ordered, coalesced publication
//! cursor. The task retains only the latest pinned view, not an event queue.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use crate::lifecycle::{Index, IndexError, IndexView};
use crate::repository::{CollectionDefinition, Repository, RepositoryOptions, SnapshotMode};
use crate::source::SourceRegistry;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitialBuildPhase {
    Discovering,
    Building,
    Complete,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitialBuildStatus {
    pub phase: InitialBuildPhase,
    pub discovered_files: usize,
    pub catalogue_complete: bool,
    pub processed_files: usize,
    pub processed_records: u64,
    pub searchable_batches: u64,
    pub construction_complete: bool,
    pub error: Option<String>,
    pub catalogue_elapsed_ns: Option<u64>,
    pub first_batch_elapsed_ns: Option<u64>,
    pub completion_elapsed_ns: Option<u64>,
}

impl Default for InitialBuildStatus {
    fn default() -> Self {
        Self {
            phase: InitialBuildPhase::Discovering,
            discovered_files: 0,
            catalogue_complete: false,
            processed_files: 0,
            processed_records: 0,
            searchable_batches: 0,
            construction_complete: false,
            error: None,
            catalogue_elapsed_ns: None,
            first_batch_elapsed_ns: None,
            completion_elapsed_ns: None,
        }
    }
}

#[derive(Clone)]
pub struct InitialBuildSnapshot {
    /// Monotonic event cursor. A waiter checks it before sleeping, so no
    /// publication can be lost between obtaining a view and subscribing.
    pub cursor: u64,
    pub status: InitialBuildStatus,
    pub view: Option<IndexView>,
}

struct InitialState {
    cursor: u64,
    status: InitialBuildStatus,
    index: Option<Index>,
    view: Option<IndexView>,
    terminal_error: Option<IndexError>,
}

struct InitialOpening {
    started: Instant,
    cancelled: AtomicBool,
    state: Mutex<InitialState>,
    changed: Condvar,
}

#[derive(Clone)]
pub struct InitialBuildTask(Arc<InitialOpening>);

impl InitialBuildTask {
    fn new() -> Self {
        Self(Arc::new(InitialOpening {
            started: Instant::now(),
            cancelled: AtomicBool::new(false),
            state: Mutex::new(InitialState {
                cursor: 0,
                status: InitialBuildStatus::default(),
                index: None,
                view: None,
                terminal_error: None,
            }),
            changed: Condvar::new(),
        }))
    }

    /// Wrap a completed cache restore in the same client opening vocabulary.
    pub fn from_ready(index: Index) -> Result<Self, IndexError> {
        let task = Self::new();
        task.publish(index.clone(), true)?;
        Ok(task)
    }

    fn publish(&self, index: Index, complete: bool) -> Result<(), IndexError> {
        let view = index.view()?;
        let status = index.status()?;
        let mut state = self.0.state.lock().map_err(|_| IndexError::LockPoisoned)?;
        state.status.catalogue_complete = true;
        let elapsed_ns = self.0.started.elapsed().as_nanos() as u64;
        state.status.catalogue_elapsed_ns.get_or_insert(elapsed_ns);
        state.status.discovered_files = status.indexed_files;
        state.status.processed_files = status.processed_files;
        state.status.processed_records = status.live_records;
        state.status.searchable_batches = view
            .0
            .segments
            .iter()
            .map(|group| {
                group
                    .iter()
                    .filter(|segment| segment.active.len() > 0)
                    .count() as u64
            })
            .sum();
        if state.status.searchable_batches > 0 {
            state
                .status
                .first_batch_elapsed_ns
                .get_or_insert(elapsed_ns);
        }
        if complete {
            state.status.completion_elapsed_ns = Some(elapsed_ns);
        }
        state.status.construction_complete = complete;
        state.status.phase = if complete {
            InitialBuildPhase::Complete
        } else {
            InitialBuildPhase::Building
        };
        state.index = Some(index);
        state.view = Some(view);
        state.cursor = state
            .cursor
            .checked_add(1)
            .ok_or(IndexError::RevisionOverflow)?;
        self.0.changed.notify_all();
        Ok(())
    }

    fn finish_error(&self, error: IndexError) {
        if let Ok(mut state) = self.0.state.lock() {
            state.status.phase = if error == IndexError::Cancelled {
                InitialBuildPhase::Cancelled
            } else {
                InitialBuildPhase::Failed
            };
            state.status.error = Some(error.to_string());
            state.terminal_error = Some(error);
            state.cursor = state.cursor.saturating_add(1);
            self.0.changed.notify_all();
        }
    }

    pub fn status(&self) -> Result<InitialBuildStatus, IndexError> {
        Ok(self
            .0
            .state
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?
            .status
            .clone())
    }

    /// The index remains available after a terminal build failure; its last
    /// pinned view is still explicitly incomplete for query and catalogue UI.
    pub fn try_index(&self) -> Result<Option<Index>, IndexError> {
        Ok(self
            .0
            .state
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?
            .index
            .clone())
    }

    pub fn snapshot(&self) -> Result<InitialBuildSnapshot, IndexError> {
        let state = self.0.state.lock().map_err(|_| IndexError::LockPoisoned)?;
        Ok(InitialBuildSnapshot {
            cursor: state.cursor,
            status: state.status.clone(),
            view: state.view.clone(),
        })
    }

    /// Return the newest state after `cursor`. Intermediate progress may
    /// coalesce, but the current revision and terminal outcome cannot be lost.
    pub fn wait_next(&self, cursor: u64) -> Result<InitialBuildSnapshot, IndexError> {
        let mut state = self.0.state.lock().map_err(|_| IndexError::LockPoisoned)?;
        if cursor > state.cursor {
            return Err(IndexError::InvalidPublication(
                "opening cursor is from the future".into(),
            ));
        }
        while state.cursor == cursor
            && !matches!(
                state.status.phase,
                InitialBuildPhase::Complete
                    | InitialBuildPhase::Cancelled
                    | InitialBuildPhase::Failed
            )
        {
            state = self
                .0
                .changed
                .wait(state)
                .map_err(|_| IndexError::LockPoisoned)?;
        }
        Ok(InitialBuildSnapshot {
            cursor: state.cursor,
            status: state.status.clone(),
            view: state.view.clone(),
        })
    }

    pub fn wait_catalogue(&self) -> Result<Index, IndexError> {
        let mut state = self.0.state.lock().map_err(|_| IndexError::LockPoisoned)?;
        loop {
            if let Some(index) = &state.index {
                return Ok(index.clone());
            }
            if let Some(error) = &state.terminal_error {
                return Err(error.clone());
            }
            state = self
                .0
                .changed
                .wait(state)
                .map_err(|_| IndexError::LockPoisoned)?;
        }
    }

    pub fn wait_complete(&self) -> Result<Index, IndexError> {
        let mut state = self.0.state.lock().map_err(|_| IndexError::LockPoisoned)?;
        loop {
            if state.status.construction_complete {
                return state.index.clone().ok_or(IndexError::LockPoisoned);
            }
            if let Some(error) = &state.terminal_error {
                return Err(error.clone());
            }
            state = self
                .0
                .changed
                .wait(state)
                .map_err(|_| IndexError::LockPoisoned)?;
        }
    }

    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
    }
}

impl Index {
    pub fn start_incremental_build(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
    ) -> Result<InitialBuildTask, IndexError> {
        Self::start_incremental_build_with_sources(
            root,
            definitions,
            options,
            SourceRegistry::builtin(),
        )
    }

    pub fn start_incremental_build_with_sources(
        root: impl Into<PathBuf>,
        definitions: Vec<CollectionDefinition>,
        mut options: RepositoryOptions,
        sources: SourceRegistry,
    ) -> Result<InitialBuildTask, IndexError> {
        options.snapshot_mode = SnapshotMode::Disabled;
        options.inference.validate().map_err(IndexError::Build)?;
        crate::relation::validate_definitions(&definitions).map_err(IndexError::Build)?;
        let task = InitialBuildTask::new();
        let worker_task = task.clone();
        let root = root.into();
        std::thread::Builder::new()
            .name("objekte-initial-build".into())
            .spawn(move || {
                let result = Repository::build_incremental_with_sources(
                    root,
                    definitions,
                    options,
                    sources,
                    |index| worker_task.publish(index.clone(), false),
                    |index| worker_task.publish(index.clone(), false),
                    || worker_task.0.cancelled.load(Ordering::Acquire),
                );
                match result {
                    Ok(index) => {
                        if let Err(error) = worker_task.publish(index, true) {
                            worker_task.finish_error(error);
                        }
                    }
                    Err(error) => worker_task.finish_error(error),
                }
            })
            .map_err(|error| IndexError::Build(error.to_string()))?;
        Ok(task)
    }
}

#[cfg(test)]
#[path = "lifecycle_initial.test.rs"]
mod tests;
