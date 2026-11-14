//! Typed desktop commands over the active core index. Blocking calls in this
//! module belong on controller workers, never in a GPUI render callback.

use std::time::Duration;
use objekte_cli::storage_view::format_index_error;

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
    let index = session.repository.index();
    index
        .stop_periodic_reconciliation()
        .map_err(lifecycle_error)?;
    index.stop_sync_cache().map_err(lifecycle_error)?;
    index.stop_sync_storage().map_err(lifecycle_error)
}

pub fn shutdown(session: &OpenedSession) -> Result<(), BackendError> {
    session
        .repository
        .index()
        .shutdown()
        .map_err(lifecycle_error)?;
    Ok(())
}

/// Quiesce every mutation producer before persisting the final accepted
/// managed generation. A failed flush leaves the index quiesced for Retry or
/// Cancel close, which resumes the prior checking policy.
pub fn shutdown_with_storage_flush(session: &OpenedSession) -> Result<(), BackendError> {
    let index = session.repository.index();
    index.quiesce().map_err(lifecycle_error)?;
    index.flush_storage().map_err(|error| BackendError::new(
        BackendErrorKind::Open, format_index_error(&error)))?;
    shutdown(session)
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
    if session
        .repository
        .index()
        .storage_status()
        .map_err(lifecycle_error)?
        .managed_directory
        .is_some()
    {
        session
            .repository
            .index()
            .set_sync_storage(true)
            .map_err(lifecycle_error)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "index_lifecycle.test.rs"]
mod tests;
