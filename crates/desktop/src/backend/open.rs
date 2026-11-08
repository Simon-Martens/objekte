//! One retained repository per desktop Open operation.

use std::path::PathBuf;
use std::sync::Arc;

use objekte_cli::arguments::{Arguments, ResolvedPaths};
use objekte_cli::loader::{
    configure_definitions, open_repository, repository_options, snapshot_policy,
};
use objekte_search_core::repository::{Repository, StartupStats};
use objekte_search_core::storage::SnapshotSaveStats;
use objekte_search_core::streaming::CancellationToken;

use super::{BackendError, BackendErrorKind};


/// CLI-compatible filesystem and index choices for one desktop Open action.
#[derive(Clone, Debug)]
pub struct OpenSpec {
    pub args: Arguments,
    pub cwd: PathBuf,
}


/// Observed opening work and configuration, including explicit index writes.
#[derive(Clone, Debug)]
pub struct OpenDiagnostics {
    pub open_ns: u128,
    pub startup: StartupStats,
    pub snapshot_save: Option<SnapshotSaveStats>,
    pub schema_files: Vec<PathBuf>,
    pub schema_fields: usize,
    pub ignored_repository_metadata: bool,
}


/// Immutable repository handle and CLI opening context reused across searches.
#[derive(Clone)]
pub struct OpenedSession {
    pub repository: Arc<Repository>,
    pub args: Arguments,
    pub paths: ResolvedPaths,
    pub collections: Vec<String>,
    pub single_collection: bool,
    pub diagnostics: OpenDiagnostics,
}


/// Perform filesystem discovery, indexing, and optional snapshot writing off the UI thread.
pub fn open(
    spec: &OpenSpec,
    cancellation: &CancellationToken,
) -> Result<OpenedSession, BackendError> {
    if cancellation.is_cancelled() {
        return Err(BackendError::cancelled());
    }
    spec.args
        .validate_open_options()
        .map_err(|message| BackendError::new(BackendErrorKind::Options, message))?;
    let paths = spec
        .args
        .resolve_paths(&spec.cwd)
        .map_err(|message| BackendError::new(BackendErrorKind::Options, message))?;
    let configured = configure_definitions(&spec.args, &paths)
        .map_err(|message| BackendError::new(BackendErrorKind::Options, message))?;
    if cancellation.is_cancelled() {
        return Err(BackendError::cancelled());
    }
    let options = repository_options(&spec.args);
    let policy = snapshot_policy(&spec.args, &paths);
    let (repository, open_ns, snapshot_save) = open_repository(
        &paths.root,
        configured.definitions,
        options,
        policy,
        spec.args.write_index.is_some(),
    )
    .map_err(|message| BackendError::new(BackendErrorKind::Open, message))?;
    if cancellation.is_cancelled() {
        return Err(BackendError::cancelled());
    }
    let startup = repository.startup().clone();
    let collections = repository
        .collections()
        .iter()
        .map(|collection| collection.definition.name.clone())
        .collect();
    Ok(OpenedSession {
        repository: Arc::new(repository),
        args: spec.args.clone(),
        paths,
        collections,
        single_collection: !spec.args.multiple_collections(),
        diagnostics: OpenDiagnostics {
            open_ns,
            startup,
            snapshot_save,
            schema_files: configured.schema_files,
            schema_fields: configured.schema_fields,
            ignored_repository_metadata: configured.ignored_repository_metadata,
        },
    })
}

#[cfg(test)]
#[path = "open.test.rs"]
mod tests;
