use std::sync::Arc;
use std::sync::atomic::Ordering;

#[cfg(test)]
use crate::lifecycle::maintenance::MaintenanceJob;
use crate::lifecycle::maintenance::MaintenanceTask;
use crate::lifecycle::reconciliation::{ReconciliationReport, ReconciliationTask};
use crate::lifecycle::{Index, IndexError};

#[cfg(test)]
fn observe_quiesce_slot(index: &Index, operation: Option<&Arc<MaintenanceJob>>) {
    let sender = index
        .0
        .quiesce_slot_observation
        .lock()
        .ok()
        .and_then(|sender| sender.clone());
    if let Some(sender) = sender {
        let identity = operation.map(|job| Arc::as_ptr(job) as usize);
        let _ = sender.send(identity);
    }
}
impl Index {
    /// Prevent new data publications, stop recurring work, and wait for every
    /// currently owned mutation worker. The resulting view is stable for a
    /// caller that is closing the index.
    pub fn quiesce(&self) -> Result<Option<ReconciliationReport>, IndexError> {
        {
            let _published = self
                .0
                .published
                .write()
                .map_err(|_| IndexError::LockPoisoned)?;
            self.0.quiescing.store(true, Ordering::Release);
        }
        self.stop_periodic_reconciliation()?;
        let operation = {
            let slot = self
                .0
                .maintenance_operation
                .lock()
                .map_err(|_| IndexError::LockPoisoned)?;
            slot.clone()
        };
        #[cfg(test)]
        observe_quiesce_slot(self, operation.as_ref());
        let operation_error = if let Some(job) = operation.as_ref() {
            let task = MaintenanceTask(Arc::clone(job));
            task.cancel();
            match task.wait() {
                Ok(_) | Err(IndexError::Cancelled) => None,
                Err(error) => Some(error),
            }
        } else {
            None
        };
        if let Some(completed) = operation.as_ref() {
            let mut slot = self
                .0
                .maintenance_operation
                .lock()
                .map_err(|_| IndexError::LockPoisoned)?;
            if slot
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, completed))
            {
                *slot = None;
            }
        }
        let current = self
            .0
            .maintenance
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?
            .current
            .clone();
        let reconciliation = current
            .map(|job| {
                let task = ReconciliationTask(job);
                task.cancel();
                task.wait()
            })
            .transpose()?;
        if let Some(error) = operation_error {
            return Err(error);
        }
        Ok(reconciliation)
    }

    /// Reopen publication after a cancelled close. Callers restore only the
    /// explicit recurring policies they held before quiescing.
    pub fn resume_after_quiesce(&self) -> Result<(), IndexError> {
        let _published = self
            .0
            .published
            .write()
            .map_err(|_| IndexError::LockPoisoned)?;
        self.0.quiescing.store(false, Ordering::Release);
        Ok(())
    }

    /// Stop the lifecycle service without writing a cache.
    pub fn shutdown(&self) -> Result<Option<ReconciliationReport>, IndexError> {
        self.quiesce()
    }
}
