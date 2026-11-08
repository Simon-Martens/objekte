//! One retained repository per desktop Open operation.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use objekte_cli::arguments::{Arguments, ResolvedPaths};
use objekte_cli::loader::{
    configure_definitions, open_repository, repository_options, snapshot_policy,
};
use objekte_search_core::lifecycle_caller::IndexedRepository;
use objekte_search_core::model::{FieldPath, PathRoot};
use objekte_search_core::repository::StartupStats;
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
    pub repository: Arc<IndexedRepository>,
    pub args: Arguments,
    pub paths: ResolvedPaths,
    pub collections: Vec<String>,
    pub single_collection: bool,
    pub diagnostics: OpenDiagnostics,
    /// Declared schema literals, parsed once on the opening worker.
    pub declared_values: Arc<HashMap<(String, String), Vec<String>>>,
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
        spec.args.repo.is_some(),
    )
    .map_err(|message| BackendError::new(BackendErrorKind::Open, message))?;
    if cancellation.is_cancelled() {
        return Err(BackendError::cancelled());
    }
    let startup = repository.startup().clone();
    let collections: Vec<String> = repository
        .collections()
        .iter()
        .map(|collection| collection.definition.name.clone())
        .collect();
    let declared_values = Arc::new(read_declared_values(&configured.schema_files, &collections));
    Ok(OpenedSession {
        repository: Arc::new(repository),
        args: spec.args.clone(),
        paths,
        collections,
        single_collection: !spec.args.multiple_collections(),
        declared_values,
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
