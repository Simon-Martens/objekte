//! Explicit compaction and rebuild scheduling.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::lifecycle::{Index, IndexError};

pub mod compaction;
pub mod rebuild;

pub use rebuild::RebuildReport;

#[cfg(test)]
pub(crate) struct MaintenanceWorkerEntryHook {
    pub(crate) reached: std::sync::mpsc::Sender<MaintenanceOperation>,
    pub(crate) release: Mutex<std::sync::mpsc::Receiver<()>>,
}

/// The kind of a long-running index maintenance task.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaintenanceOperation {
    Compact,
    Rebuild,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaintenancePhase {
    Pending,
    Preparing,
    Publishing,
    Complete,
    Cancelled,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MaintenanceProgress {
    pub operation: MaintenanceOperation,
    pub phase: MaintenancePhase,
    pub attempt: usize,
    pub elapsed: Duration,
}

#[derive(Clone, Debug)]
pub enum MaintenanceReport {
    Compact(compaction::CompactionReport),
    Rebuild(RebuildReport),
}

pub(crate) struct MaintenanceJob {
    pub(crate) operation: MaintenanceOperation,
    pub(crate) started: Instant,
    pub(crate) cancelled: AtomicBool,
    pub(crate) progress: Mutex<MaintenanceProgress>,
    pub(crate) result: Mutex<Option<Result<MaintenanceReport, IndexError>>>,
    pub(crate) complete: Condvar,
    pub(crate) worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}

/// One observable compaction or rebuild. Cancellation prevents publication
/// and leaves the current view available.
#[derive(Clone)]
pub struct MaintenanceTask(pub(crate) Arc<MaintenanceJob>);

impl MaintenanceTask {
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
    }

    pub fn progress(&self) -> Result<MaintenanceProgress, IndexError> {
        let mut progress = *self
            .0
            .progress
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?;
        if !matches!(
            progress.phase,
            MaintenancePhase::Complete | MaintenancePhase::Cancelled | MaintenancePhase::Failed
        ) {
            progress.elapsed = self.0.started.elapsed();
        }
        Ok(progress)
    }

    pub fn is_complete(&self) -> Result<bool, IndexError> {
        Ok(self
            .0
            .result
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?
            .is_some())
    }

    pub fn wait(&self) -> Result<MaintenanceReport, IndexError> {
        let mut result = self.0.result.lock().map_err(|_| IndexError::LockPoisoned)?;
        while result.is_none() {
            result = self
                .0
                .complete
                .wait(result)
                .map_err(|_| IndexError::LockPoisoned)?;
        }
        let outcome = result.as_ref().cloned().ok_or(IndexError::LockPoisoned)?;
        drop(result);
        if let Some(worker) = self
            .0
            .worker
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?
            .take()
        {
            worker.join().map_err(|_| {
                IndexError::InvalidPublication("maintenance worker panicked".into())
            })?;
        }
        outcome
    }
}

impl MaintenanceJob {
    pub(crate) fn set_progress(&self, phase: MaintenancePhase, attempt: usize, elapsed: Duration) {
        if let Ok(mut progress) = self.progress.lock() {
            progress.phase = phase;
            progress.attempt = attempt;
            progress.elapsed = elapsed;
        }
    }
}

#[cfg(test)]
mod tests;
impl Index {
    /// Run one source-free compaction on a worker while pinned queries use
    /// their current view. Concurrent requests for the same operation share
    /// its handle; a request for the other operation returns the existing
    /// busy error immediately.
    pub fn start_compaction(&self) -> Result<MaintenanceTask, IndexError> {
        self.start_maintenance(MaintenanceOperation::Compact)
    }

    /// Reconstruct the index from the current explicit configuration on a
    /// worker. Cancellation leaves the published view unchanged.
    pub fn start_rebuild(&self) -> Result<MaintenanceTask, IndexError> {
        self.start_maintenance(MaintenanceOperation::Rebuild)
    }

    #[cfg(test)]
    fn wait_for_maintenance_worker_entry(&self, operation: MaintenanceOperation) {
        let hook = self
            .0
            .maintenance_worker_hook
            .lock()
            .ok()
            .and_then(|hook| hook.clone());
        if let Some(hook) = hook {
            let _ = hook.reached.send(operation);
            if let Ok(release) = hook.release.lock() {
                let _ = release.recv();
            }
        }
    }

    fn start_maintenance(
        &self,
        operation: MaintenanceOperation,
    ) -> Result<MaintenanceTask, IndexError> {
        let mut current = self
            .0
            .maintenance_operation
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?;
        if self.0.quiescing.load(Ordering::Acquire) {
            return Err(IndexError::Cancelled);
        }
        if let Some(job) = current.as_ref() {
            if job
                .result
                .lock()
                .map_err(|_| IndexError::LockPoisoned)?
                .is_none()
            {
                if job.operation == operation {
                    return Ok(MaintenanceTask(Arc::clone(job)));
                }
                return Err(IndexError::ResourceLimit(
                    "another index maintenance task is running".into(),
                ));
            }
        }
        let job = Arc::new(MaintenanceJob {
            operation,
            started: Instant::now(),
            cancelled: AtomicBool::new(false),
            progress: Mutex::new(MaintenanceProgress {
                operation,
                phase: MaintenancePhase::Pending,
                attempt: 0,
                elapsed: Duration::ZERO,
            }),
            result: Mutex::new(None),
            complete: Condvar::new(),
            worker: Mutex::new(None),
        });
        let owner = self.clone();
        let worker_job = Arc::clone(&job);
        let worker = std::thread::Builder::new()
            .name(
                match operation {
                    MaintenanceOperation::Compact => "objekte-compact",
                    MaintenanceOperation::Rebuild => "objekte-rebuild",
                }
                .into(),
            )
            .spawn(move || {
                #[cfg(test)]
                owner.wait_for_maintenance_worker_entry(operation);
                let outcome =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match operation {
                        MaintenanceOperation::Compact => owner
                            .compact_with_control(
                                || worker_job.cancelled.load(Ordering::Acquire),
                                |phase, attempt| {
                                    worker_job.set_progress(
                                        phase,
                                        attempt,
                                        worker_job.started.elapsed(),
                                    )
                                },
                            )
                            .map(MaintenanceReport::Compact),
                        MaintenanceOperation::Rebuild => owner
                            .rebuild_with_control(
                                || worker_job.cancelled.load(Ordering::Acquire),
                                |phase, attempt| {
                                    worker_job.set_progress(
                                        phase,
                                        attempt,
                                        worker_job.started.elapsed(),
                                    )
                                },
                                |_| {},
                            )
                            .map(MaintenanceReport::Rebuild),
                    }))
                    .unwrap_or_else(|_| {
                        Err(IndexError::InvalidPublication(
                            "maintenance worker panicked".into(),
                        ))
                    });
                let phase = match &outcome {
                    Ok(_) => MaintenancePhase::Complete,
                    Err(IndexError::Cancelled) => MaintenancePhase::Cancelled,
                    Err(_) => MaintenancePhase::Failed,
                };
                let attempt = worker_job
                    .progress
                    .lock()
                    .ok()
                    .map(|progress| progress.attempt)
                    .unwrap_or(0);
                worker_job.set_progress(phase, attempt, worker_job.started.elapsed());
                if let Ok(mut result) = worker_job.result.lock() {
                    *result = Some(outcome);
                    worker_job.complete.notify_all();
                }
            })
            .map_err(|error| IndexError::Io(error.to_string()))?;
        *job.worker.lock().map_err(|_| IndexError::LockPoisoned)? = Some(worker);
        *current = Some(Arc::clone(&job));
        Ok(MaintenanceTask(job))
    }
}
