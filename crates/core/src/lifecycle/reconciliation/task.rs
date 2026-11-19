use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use crate::lifecycle::reconciliation::{
    ReconciliationProgress, ReconciliationReport, ReconciliationState,
};
use crate::lifecycle::{Index, IndexError};

pub(crate) struct MaintenanceState {
    pub(crate) reconciliation: ReconciliationState,
    pub(crate) current: Option<Arc<ReconciliationJob>>,
}

pub(crate) struct ReconciliationJob {
    pub(crate) cancelled: AtomicBool,
    pub(crate) progress: Mutex<ReconciliationProgress>,
    pub(crate) result: Mutex<Option<ReconciliationReport>>,
    pub(crate) complete: Condvar,
    pub(crate) worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}

/// Observable, cancellable handle for one metadata reconciliation pass.
#[derive(Clone)]
pub struct ReconciliationTask(pub(crate) Arc<ReconciliationJob>);

impl ReconciliationTask {
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
    }

    pub fn is_complete(&self) -> Result<bool, IndexError> {
        Ok(self
            .0
            .result
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?
            .is_some())
    }

    pub fn progress(&self) -> Result<ReconciliationProgress, IndexError> {
        Ok(*self
            .0
            .progress
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?)
    }

    pub fn wait(&self) -> Result<ReconciliationReport, IndexError> {
        let mut guard = self.0.result.lock().map_err(|_| IndexError::LockPoisoned)?;
        while guard.is_none() {
            guard = self
                .0
                .complete
                .wait(guard)
                .map_err(|_| IndexError::LockPoisoned)?;
        }
        let result = guard.as_ref().cloned().ok_or(IndexError::LockPoisoned)?;
        drop(guard);
        if let Some(worker) = self
            .0
            .worker
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?
            .take()
        {
            worker.join().map_err(|_| {
                IndexError::InvalidPublication("reconciliation worker panicked".into())
            })?;
        }
        Ok(result)
    }
}

impl Index {
    /// Coalesce concurrent scan requests into one worker. The scan runs
    /// without holding a query or publication lock; waiters receive its
    /// report and can retry a later pass after partial failure.
    pub fn start_reconcile(&self) -> Result<ReconciliationTask, IndexError> {
        let mut maintenance = self
            .0
            .maintenance
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?;
        if self.0.quiescing.load(Ordering::Acquire) {
            return Err(IndexError::Cancelled);
        }
        if let Some(job) = &maintenance.current {
            if job
                .result
                .lock()
                .map_err(|_| IndexError::LockPoisoned)?
                .is_none()
            {
                return Ok(ReconciliationTask(Arc::clone(job)));
            }
        }
        let job = Arc::new(ReconciliationJob {
            cancelled: AtomicBool::new(false),
            progress: Mutex::new(ReconciliationProgress::default()),
            result: Mutex::new(None),
            complete: Condvar::new(),
            worker: Mutex::new(None),
        });
        let owner = self.clone();
        let worker_job = Arc::clone(&job);
        let worker = std::thread::Builder::new()
            .name("objekte-reconcile".into())
            .spawn(move || {
                if let Ok(mut maintenance) = owner.0.maintenance.lock() {
                    maintenance.reconciliation = ReconciliationState::Running;
                }
                let report = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    owner.reconcile_pass_with_progress(
                        || worker_job.cancelled.load(Ordering::Acquire),
                        |progress| {
                            if let Ok(mut observed) = worker_job.progress.lock() {
                                *observed = ReconciliationProgress::from(progress);
                            }
                        },
                    )
                }))
                .unwrap_or_else(|_| ReconciliationReport {
                    issues: vec!["reconciliation worker panicked".into()],
                    ..ReconciliationReport::default()
                });
                if let Ok(mut observed) = worker_job.progress.lock() {
                    *observed = ReconciliationProgress::from(&report);
                }
                let state = if report.cancelled {
                    ReconciliationState::Cancelled
                } else if report.issues.is_empty() && report.omitted_issues == 0 {
                    ReconciliationState::Complete
                } else {
                    ReconciliationState::Partial
                };
                if let Ok(mut maintenance) = owner.0.maintenance.lock() {
                    maintenance.reconciliation = state;
                }
                if let Ok(mut result) = worker_job.result.lock() {
                    *result = Some(report);
                    worker_job.complete.notify_all();
                }
            })
            .map_err(|error| IndexError::Io(error.to_string()))?;
        *job.worker.lock().map_err(|_| IndexError::LockPoisoned)? = Some(worker);
        maintenance.reconciliation = ReconciliationState::Pending;
        maintenance.current = Some(Arc::clone(&job));
        Ok(ReconciliationTask(job))
    }

    /// Observe the most recently started reconciliation without scheduling a
    /// new pass. The returned handle remains valid after the pass completes;
    /// callers can inspect progress and retrieve its bounded outcome.
    pub fn current_reconciliation_task(&self) -> Result<Option<ReconciliationTask>, IndexError> {
        Ok(self
            .0
            .maintenance
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?
            .current
            .as_ref()
            .map(|job| ReconciliationTask(Arc::clone(job))))
    }

    pub fn wait_for_reconciliation(&self) -> Result<Option<ReconciliationReport>, IndexError> {
        let current = self
            .0
            .maintenance
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?
            .current
            .clone();
        current
            .map(|job| ReconciliationTask(job).wait())
            .transpose()
    }
}
