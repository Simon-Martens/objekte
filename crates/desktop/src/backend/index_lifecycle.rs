//! Typed desktop commands over the active core index. Blocking calls in this
//! module belong on controller workers, never in a GPUI render callback.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use objekte_search_core::lifecycle::{
    Index, IndexRevision, IndexStatus, MaintenanceOperation, MaintenanceTask, ReconciliationTask,
};
use objekte_search_core::lifecycle_caller::IndexedRepository;
use objekte_search_core::storage::SnapshotSaveStats;
use objekte_search_core::streaming::CancellationToken;

use super::open::OpenedSession;
use super::{BackendError, BackendErrorKind};

#[derive(Clone, Debug)]
pub struct SaveOutcome {
    pub destination: PathBuf,
    pub saved_revision: IndexRevision,
    pub current_status: IndexStatus,
    pub stats: SnapshotSaveStats,
}

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

fn same_existing_destination(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

/// Save a pinned active revision. Replacing another existing destination is
/// permitted only after the UI has obtained its explicit overwrite decision.
pub fn save_index(
    session: &OpenedSession,
    destination: &Path,
    allow_replace: bool,
) -> Result<SaveOutcome, BackendError> {
    if destination.as_os_str().is_empty() {
        return Err(BackendError::new(
            BackendErrorKind::Options,
            "Choose an index destination",
        ));
    }
    let known_selected = session
        .configuration
        .cache_save
        .as_deref()
        .is_some_and(|selected| same_existing_destination(selected, destination));
    match std::fs::symlink_metadata(destination) {
        Ok(_) if !known_selected && !allow_replace => {
            return Err(BackendError::new(
                BackendErrorKind::Options,
                format!(
                    "{} already exists; confirm replacement before saving an index copy",
                    destination.display()
                ),
            ));
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(lifecycle_error(error));
        }
        _ => {}
    }
    let (saved_revision, stats) = session
        .repository
        .index()
        .flush_cache_with_stats(destination)
        .map_err(lifecycle_error)?;
    Ok(SaveOutcome {
        destination: destination.to_path_buf(),
        saved_revision,
        current_status: status(session)?,
        stats,
    })
}

/// Prepare a cache replacement against the active source/schema definition.
/// The caller installs this session only after success and ownership checks.
pub fn load_index(
    session: &OpenedSession,
    cache: &Path,
    cancellation: &CancellationToken,
) -> Result<OpenedSession, BackendError> {
    if cache.as_os_str().is_empty() {
        return Err(BackendError::new(
            BackendErrorKind::Options,
            "Choose an index cache to load",
        ));
    }
    if cancellation.is_cancelled() {
        return Err(BackendError::cancelled());
    }
    let started = Instant::now();
    let index = Index::load_cache(
        &session.paths.root,
        session.configuration.definitions.as_ref().clone(),
        session.configuration.options,
        cache,
    )
    .map_err(lifecycle_error)?;
    index
        .protect_configuration_files(&session.configuration.protected_files)
        .map_err(lifecycle_error)?;
    if cancellation.is_cancelled() {
        index.shutdown().map_err(lifecycle_error)?;
        return Err(BackendError::cancelled());
    }
    let repository = IndexedRepository::new(index).map_err(lifecycle_error)?;
    let identity = repository
        .index()
        .view()
        .map_err(lifecycle_error)?
        .configuration_identity();
    if identity != session.configuration.configuration_identity {
        repository.index().shutdown().map_err(lifecycle_error)?;
        return Err(BackendError::new(
            BackendErrorKind::Open,
            "selected index has a different source configuration",
        ));
    }
    let mut configuration = (*session.configuration).clone();
    configuration.cache_load = Some(cache.to_path_buf());
    configuration.cache_save = Some(cache.to_path_buf());
    let mut diagnostics = session.diagnostics.clone();
    diagnostics.open_ns = started.elapsed().as_nanos();
    diagnostics.startup = repository.startup().clone();
    diagnostics.snapshot_save = None;
    Ok(OpenedSession {
        repository: Arc::new(repository),
        configuration: Arc::new(configuration),
        args: session.args.clone(),
        paths: session.paths.clone(),
        collections: session.collections.clone(),
        single_collection: session.single_collection,
        diagnostics,
        declared_values: Arc::clone(&session.declared_values),
        background_snapshot_save: None,
    })
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

pub fn set_automatic_save(session: &OpenedSession, enabled: bool) -> Result<(), BackendError> {
    if enabled
        && (!session.configuration.configured_repository
            || session.configuration.cache_save.is_none())
    {
        return Err(BackendError::new(
            BackendErrorKind::Options,
            "Automatic saving requires a configured repository and selected index destination",
        ));
    }
    session
        .repository
        .index()
        .set_sync_cache(enabled)
        .map_err(lifecycle_error)
}

pub fn automatic_save_error(session: &OpenedSession) -> Result<Option<String>, BackendError> {
    session
        .repository
        .index()
        .sync_cache_error()
        .map_err(lifecycle_error)
}

pub fn stop_automation(session: &OpenedSession) -> Result<(), BackendError> {
    let index = session.repository.index();
    index
        .stop_periodic_reconciliation()
        .map_err(lifecycle_error)?;
    index.stop_sync_cache().map_err(lifecycle_error)
}

pub fn shutdown(session: &OpenedSession) -> Result<(), BackendError> {
    session
        .repository
        .index()
        .shutdown()
        .map_err(lifecycle_error)?;
    Ok(())
}

#[cfg(test)]
#[path = "index_lifecycle.test.rs"]
mod tests;
