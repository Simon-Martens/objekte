//! Typed desktop commands over the active core index. Blocking calls in this
//! module belong on controller workers, never in a GPUI render callback.

use std::time::Duration;

use objekte_search_core::lifecycle::{
    IndexStatus, MaintenanceOperation, MaintenanceTask, ReconciliationTask,
};

use super::open::OpenedSession;
use super::{BackendError, BackendErrorKind};

fn lifecycle_error(error: impl std::fmt::Display) -> BackendError {
    BackendError::new(BackendErrorKind::Open, error.to_string())
}

pub fn status(session: &OpenedSession) -> Result<IndexStatus, BackendError> {
    session.repository.index().status().map_err(lifecycle_error)
}

pub fn current_reconciliation(
    session: &OpenedSession,
) -> Result<Option<ReconciliationTask>, BackendError> {
    session
        .repository
        .index()
        .current_reconciliation_task()
        .map_err(lifecycle_error)
}

pub fn check_for_changes(session: &OpenedSession) -> Result<ReconciliationTask, BackendError> {
    session
        .repository
        .index()
        .start_reconcile()
        .map_err(lifecycle_error)
}

pub fn start_maintenance(
    session: &OpenedSession,
    operation: MaintenanceOperation,
) -> Result<MaintenanceTask, BackendError> {
    let index = session.repository.index();
    match operation {
        MaintenanceOperation::Rebuild => index.start_rebuild(),
        MaintenanceOperation::Compact => index.start_compaction(),
    }
    .map_err(lifecycle_error)
}

pub fn set_periodic_check(
    session: &OpenedSession,
    interval: Option<Duration>,
) -> Result<(), BackendError> {
    match interval {
        Some(interval) => session
            .repository
            .index()
            .start_periodic_reconciliation(interval),
        None => session.repository.index().stop_periodic_reconciliation(),
    }
    .map_err(lifecycle_error)
}

pub fn stop_automation(session: &OpenedSession) -> Result<(), BackendError> {
    session
        .repository
        .index()
        .stop_periodic_reconciliation()
        .map_err(lifecycle_error)
}

pub fn shutdown(session: &OpenedSession) -> Result<(), BackendError> {
    session
        .repository
        .index()
        .shutdown()
        .map_err(lifecycle_error)?;
    Ok(())
}

pub fn resume_after_cancelled_close(
    session: &OpenedSession,
    check_interval: Option<Duration>,
) -> Result<(), BackendError> {
    session
        .repository
        .index()
        .resume_after_quiesce()
        .map_err(lifecycle_error)?;
    set_periodic_check(session, check_interval)?;
    Ok(())
}

#[cfg(test)]
#[path = "index_lifecycle.test.rs"]
mod tests;
