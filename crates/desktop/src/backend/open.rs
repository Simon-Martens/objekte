//! One retained repository per desktop Open operation.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use objekte_cli::arguments::{Arguments, ResolvedPaths};
use objekte_cli::storage_view::format_index_error;
use objekte_cli::loader::{configure_definitions, repository_options};
use objekte_search_core::index_storage::{IndexStorageLocation, IndexStoragePolicy};
use objekte_search_core::lifecycle::{Index, InitialBuildTask};
use objekte_search_core::lifecycle_caller::IndexedRepository;
use objekte_search_core::model::{FieldPath, PathRoot};
use objekte_search_core::repository::StartupStats;
use objekte_search_core::source::SourceRegistry;
use objekte_search_core::streaming::CancellationToken;

use super::{BackendError, BackendErrorKind};

/// CLI-compatible filesystem and index choices for one desktop Open action.
#[derive(Clone, Debug)]
pub struct OpenSpec {
    pub args: Arguments,
    pub cwd: PathBuf,
    /// Shared core policy for desktop opening; `None` is memory only.
    pub storage_policy: Option<IndexStoragePolicy>,
}

/// Observed opening work and configuration.
#[derive(Clone, Debug)]
pub struct OpenDiagnostics {
    pub open_ns: u128,
    pub startup: StartupStats,
    pub schema_files: Vec<PathBuf>,
    pub schema_fields: usize,
    pub ignored_repository_metadata: bool,
}

/// Resolved identity and opening mode of the active session.
#[derive(Clone, Debug)]
pub struct SessionConfiguration {
    pub configured_repository: bool,
    pub configuration_identity: [u8; 32],
}

/// Immutable repository handle and CLI opening context reused across searches.
#[derive(Clone)]
pub struct OpenedSession {
    pub repository: Arc<IndexedRepository>,
    pub storage_policy: IndexStoragePolicy,
    pub configuration: Arc<SessionConfiguration>,
    pub args: Arguments,
    pub paths: ResolvedPaths,
    pub collections: Vec<String>,
    pub single_collection: bool,
    pub diagnostics: OpenDiagnostics,
    /// Declared schema literals, parsed once on the opening worker.
    pub declared_values: Arc<HashMap<(String, String), Vec<String>>>,
}

impl OpenedSession {
    /// Rebind the compatibility metadata view after a rebuild or compaction
    /// publishes a new base. The live Index and session identity stay intact.
    pub fn refresh_base_metadata(&self) -> Result<Self, BackendError> {
        let mut refreshed = self.clone();
        refreshed.repository = Arc::new(
            IndexedRepository::new(self.repository.index().clone())
                .map_err(|error| BackendError::new(BackendErrorKind::Open, error.to_string()))?,
        );
        Ok(refreshed)
    }

}

pub fn open(
    spec: &OpenSpec,
    cancellation: &CancellationToken,
) -> Result<OpenedSession, BackendError> {
    open_shared_storage(
        spec,
        cancellation,
        spec.storage_policy.clone().unwrap_or_default(),
        false,
    )
    .map(|(session, _)| session)
}

/// Return a searchable catalogue while a fresh index continues building.
/// A restored managed generation returns a completed session immediately.
pub fn open_incremental(
    spec: &OpenSpec,
    cancellation: &CancellationToken,
) -> Result<(OpenedSession, Option<InitialBuildTask>), BackendError> {
    open_shared_storage(
        spec,
        cancellation,
        spec.storage_policy.clone().unwrap_or_default(),
        true,
    )
}

fn open_shared_storage(
    spec: &OpenSpec,
    cancellation: &CancellationToken,
    storage: IndexStoragePolicy,
    incremental: bool,
) -> Result<(OpenedSession, Option<InitialBuildTask>), BackendError> {
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
    let mut options = repository_options(&spec.args);
    options.verified_cache_bytes = 64 * 1024 * 1024;
    let mut presented_storage = storage.clone();
    if let IndexStorageLocation::CustomDirectory(path) = &mut presented_storage.location {
        *path = path.canonicalize().map_err(|error| BackendError::new(
            BackendErrorKind::Options, format!("custom index base {}: {error}", path.display())))?;
    }
    let started = Instant::now();
    let build = Index::start_incremental_build_with_storage(
        &paths.root,
        configured.definitions.clone(),
        options,
        storage,
        SourceRegistry::builtin(),
    )
    .map_err(|error| BackendError::new(BackendErrorKind::Open, format_index_error(&error)))?;
    let index = if incremental {
        loop {
            if cancellation.is_cancelled() {
                build.cancel();
                return Err(BackendError::cancelled());
            }
            let status = build
                .status()
                .map_err(|error| BackendError::new(BackendErrorKind::Open, error.to_string()))?;
            if status.catalogue_complete
                || matches!(status.phase,
                    objekte_search_core::lifecycle::InitialBuildPhase::Cancelled
                    | objekte_search_core::lifecycle::InitialBuildPhase::Failed)
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        build.wait_catalogue()
    } else {
        build.wait_complete()
    }
    .map_err(|error| BackendError::new(BackendErrorKind::Open, format_index_error(&error)))?;
    let task = (incremental && !build.status().map_err(|error| BackendError::new(BackendErrorKind::Open, error.to_string()))?.construction_complete).then_some(build);
    if cancellation.is_cancelled() {
        if let Some(task) = &task {
            task.cancel();
        }
        index
            .shutdown()
            .map_err(|error| BackendError::new(BackendErrorKind::Open, error.to_string()))?;
        return Err(BackendError::cancelled());
    }
    let repository = IndexedRepository::new(index)
        .map_err(|error| BackendError::new(BackendErrorKind::Open, error.to_string()))?;
    let startup = repository.startup().clone();
    let collections = repository
        .collections()
        .iter()
        .map(|collection| collection.definition.name.clone())
        .collect::<Vec<_>>();
    let declared_values = Arc::new(read_declared_values(&configured.schema_files, &collections));
    let configuration_identity = repository
        .index()
        .view()
        .map_err(|error| BackendError::new(BackendErrorKind::Open, error.to_string()))?
        .configuration_identity();
    Ok((OpenedSession {
        repository: Arc::new(repository),
        storage_policy: presented_storage,
        configuration: Arc::new(SessionConfiguration {
            configured_repository: spec.args.repo.is_some(),
            configuration_identity,
        }),
        args: spec.args.clone(),
        paths,
        collections,
        single_collection: !spec.args.multiple_collections(),
        declared_values,
        diagnostics: OpenDiagnostics {
            open_ns: started.elapsed().as_nanos(),
            startup,
            schema_files: configured.schema_files,
            schema_fields: configured.schema_fields,
            ignored_repository_metadata: configured.ignored_repository_metadata,
        },
    }, task))
}

fn read_declared_values(
    schema_files: &[PathBuf],
    collections: &[String],
) -> HashMap<(String, String), Vec<String>> {
    fn visit(
        node: &serde_json::Value,
        path: &mut Vec<String>,
        collection: &str,
        result: &mut HashMap<(String, String), Vec<String>>,
    ) {
        if !path.is_empty() {
            if let Some(values) = node.get("enum").and_then(serde_json::Value::as_array) {
                let key = FieldPath {
                    root: PathRoot::Data,
                    segments: path.clone(),
                }
                .display();
                let choices = result.entry((collection.to_owned(), key)).or_default();
                for value in values.iter().take(256) {
                    if !value.is_null() && !value.is_array() && !value.is_object() {
                        let literal = value.to_string();
                        if !choices.contains(&literal) {
                            choices.push(literal);
                        }
                    }
                }
            }
        }
        if let Some(properties) = node
            .get("properties")
            .and_then(serde_json::Value::as_object)
        {
            for (name, child) in properties {
                path.push(name.clone());
                visit(child, path, collection, result);
                path.pop();
            }
        }
        if let Some(items) = node.get("items") {
            visit(items, path, collection, result);
        }
    }

    let mut result = HashMap::new();
    for file in schema_files {
        let Ok(bytes) = std::fs::read(file) else {
            continue;
        };
        let Ok(schema) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        let collection = schema
            .get("x-objekte")
            .and_then(|config| config.get("name"))
            .and_then(serde_json::Value::as_str)
            .or_else(|| (collections.len() == 1).then(|| collections[0].as_str()));
        if let Some(collection) = collection {
            visit(&schema, &mut Vec::new(), collection, &mut result);
        }
    }
    result
}

#[cfg(test)]
#[path = "open.test.rs"]
mod tests;
