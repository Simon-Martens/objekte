//! Observable, cancellable handles for background storage operations.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use crate::lifecycle::{Index, IndexError};

use super::store::{SaveOptions, SavePhase, SaveProgress, SaveReport};

/// Mutable progress shared by a storage worker.
#[derive(Clone, Debug)]
pub struct StorageProgress {
    pub phase: SavePhase,
    pub objects_total: u64,
    pub objects_completed: u64,
    pub bytes_written: u64,
    pub bytes_reused: u64,
    pub elapsed: std::time::Duration,
}

impl Default for StorageProgress {
    fn default() -> Self {
        Self {
            phase: SavePhase::Preparing,
            objects_total: 0,
            objects_completed: 0,
            bytes_written: 0,
            bytes_reused: 0,
            elapsed: std::time::Duration::ZERO,
        }
    }
}

/// Handle for one background save.
#[derive(Clone)]
pub struct StorageTask(Arc<StorageJob>);

struct StorageJob {
    cancelled: AtomicBool,
    progress: Mutex<StorageProgress>,
    result: Mutex<Option<Result<SaveReport, IndexError>>>,
    complete: Condvar,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl StorageTask {
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }

    pub fn is_complete(&self) -> Result<bool, IndexError> {
        Ok(self
            .0
            .result
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?
            .is_some())
    }

    pub fn progress(&self) -> Result<StorageProgress, IndexError> {
        Ok(self
            .0
            .progress
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?
            .clone())
    }

    pub fn wait(&self) -> Result<SaveReport, IndexError> {
        let mut guard = self
            .0
            .result
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?;
        while guard.is_none() {
            guard = self
                .0
                .complete
                .wait(guard)
                .map_err(|_| IndexError::LockPoisoned)?;
        }
        let result = guard
            .as_ref()
            .cloned()
            .ok_or(IndexError::LockPoisoned)?;
        drop(guard);
        if let Some(worker) = self
            .0
            .worker
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?
            .take()
        {
            worker
                .join()
                .map_err(|_| IndexError::InvalidPublication("storage worker panicked".into()))?;
        }
        result
    }
}

/// Start one save on a dedicated worker. The current view is pinned by
/// `Index::save_to` when the worker begins.
pub(crate) fn start_save_task(
    index: Index,
    directory: std::path::PathBuf,
    options: SaveOptions,
) -> Result<StorageTask, IndexError> {
    let job = Arc::new(StorageJob {
        cancelled: AtomicBool::new(false),
        progress: Mutex::new(StorageProgress::default()),
        result: Mutex::new(None),
        complete: Condvar::new(),
        worker: Mutex::new(None),
    });
    let worker_job = Arc::clone(&job);
    let worker = std::thread::Builder::new()
        .name("objekte-save".into())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                index.save_to_with_control(
                    &directory,
                    options,
                    || worker_job.cancelled.load(Ordering::Acquire),
                    |progress: &SaveProgress| {
                        if let Ok(mut current) = worker_job.progress.lock() {
                            *current = StorageProgress {
                                phase: progress.phase,
                                objects_total: progress.objects_total,
                                objects_completed: progress.objects_completed,
                                bytes_written: progress.bytes_written,
                                bytes_reused: progress.bytes_reused,
                                elapsed: progress.elapsed,
                            };
                        }
                    },
                )
            }))
            .unwrap_or_else(|_| {
                Err(IndexError::InvalidPublication(
                    "storage worker panicked".into(),
                ))
            });
            let _ = index.update_storage_status(|status| status.active_save_task = false);
            if let Ok(mut result) = worker_job.result.lock() {
                *result = Some(outcome);
                worker_job.complete.notify_all();
            }
        })
        .map_err(|error| IndexError::Io(error.to_string()))?;
    *job.worker
        .lock()
        .map_err(|_| IndexError::LockPoisoned)? = Some(worker);
    Ok(StorageTask(job))
}

/// Observable storage progress with no active worker.
pub fn phase_label(phase: SavePhase) -> &'static str {
    match phase {
        SavePhase::Preparing => "preparing",
        SavePhase::WritingObjects => "writing objects",
        SavePhase::WritingCatalogue => "writing catalogue",
        SavePhase::WritingManifest => "writing manifest",
        SavePhase::Committing => "committing",
        SavePhase::CleaningUp => "cleaning up",
        SavePhase::Complete => "complete",
        SavePhase::Cancelled => "cancelled",
        SavePhase::Failed => "failed",
    }
}
