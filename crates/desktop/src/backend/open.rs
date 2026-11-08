//! One retained repository per desktop Open operation.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use objekte_cli::arguments::{Arguments, ResolvedPaths};
use objekte_cli::loader::{
    configure_definitions, open_repository, repository_options, snapshot_policy,
};
use objekte_search_core::lifecycle::Index;
use objekte_search_core::lifecycle_caller::IndexedRepository;
use objekte_search_core::lifecycle_repository::{
    CacheFailurePolicy, CacheOpenPolicy, RepositoryOpenOptions,
};
use objekte_search_core::model::{FieldPath, PathRoot};
use objekte_search_core::repository::{CollectionDefinition, RepositoryOptions, SnapshotMode, StartupStats};
use objekte_search_core::repository_configuration::RepositoryConfiguration;
use objekte_search_core::storage::SnapshotSaveStats;
use objekte_search_core::streaming::CancellationToken;

use super::{BackendError, BackendErrorKind};


/// CLI-compatible filesystem and index choices for one desktop Open action.
#[derive(Clone, Debug)]
pub struct OpenSpec {
    pub args: Arguments,
    pub cwd: PathBuf,
    /// Configured-repository override for Build in memory and explicit cache
    /// choices. `None` retains the CLI-compatible convenience policy.
    pub configured_cache_policy: Option<CacheOpenPolicy>,
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

type BackgroundSnapshotSave = Arc<Mutex<Option<Result<Option<SnapshotSaveStats>, String>>>>;

/// Resolved, immutable inputs needed to validate a replacement cache without
/// rereading a different schema or changing the source configuration.
#[derive(Clone, Debug)]
pub struct SessionConfiguration {
    pub definitions: Arc<Vec<CollectionDefinition>>,
    pub options: RepositoryOptions,
    pub configured_repository: bool,
    pub cache_load: Option<PathBuf>,
    pub cache_save: Option<PathBuf>,
    pub configuration_identity: [u8; 32],
    pub protected_files: Arc<Vec<PathBuf>>,
}


/// Immutable repository handle and CLI opening context reused across searches.
#[derive(Clone)]
pub struct OpenedSession {
    pub repository: Arc<IndexedRepository>,
    pub configuration: Arc<SessionConfiguration>,
    pub args: Arguments,
    pub paths: ResolvedPaths,
    pub collections: Vec<String>,
    pub single_collection: bool,
    pub diagnostics: OpenDiagnostics,
    /// Declared schema literals, parsed once on the opening worker.
    pub declared_values: Arc<HashMap<(String, String), Vec<String>>>,
    pub(super) background_snapshot_save: Option<BackgroundSnapshotSave>,
}

impl OpenedSession {
    /// Record the first explicit destination selected by a manual session.
    /// Core flush chooses that same path when no cache target existed yet.
    pub fn with_first_save_target(&self, destination: PathBuf) -> Self {
        if self.configuration.cache_save.is_some() {
            return self.clone();
        }
        let mut changed = self.clone();
        let mut configuration = (*self.configuration).clone();
        configuration.cache_save = Some(destination);
        changed.configuration = Arc::new(configuration);
        changed
    }

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

    /// Take the result of an explicit write requested while a restored cache
    /// reconciles in the background. `None` means no result is ready.
    pub fn take_background_snapshot_save(
        &self,
    ) -> Option<Result<Option<SnapshotSaveStats>, String>> {
        self.background_snapshot_save
            .as_ref()?
            .lock()
            .ok()?
            .take()
    }
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
    if spec
        .args
        .load_index
        .as_ref()
        .is_some_and(|path| path.trim().is_empty())
        || spec
            .args
            .write_index
            .as_ref()
            .is_some_and(|path| path.trim().is_empty())
    {
        return Err(BackendError::new(
            BackendErrorKind::Options,
            "Choose an explicit index location",
        ));
    }
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
    let cache_directory = policy
        .directory
        .clone()
        .unwrap_or_else(|| paths.root.join(".objekte"));
    let (repository, open_ns, snapshot_save) = if spec.args.repo.is_some() {
        let opened = Instant::now();
        let index = Index::open_configured_repository(
            RepositoryConfiguration {
                root: paths.root.clone(),
                definitions: configured.definitions.clone(),
                schema_files: configured.schema_files.clone(),
                protected_files: configured.protected_files.clone(),
                schema_fields: configured.schema_fields,
            },
            RepositoryOpenOptions {
                index: options,
                cache_directory: Some(cache_directory.clone()),
                cache_open: spec.configured_cache_policy.unwrap_or(match policy.mode {
                    SnapshotMode::Required => CacheOpenPolicy::Required,
                    SnapshotMode::IfValid => CacheOpenPolicy::IfPresent,
                    SnapshotMode::Disabled if spec.args.write_index.is_some() => CacheOpenPolicy::Ignore,
                    SnapshotMode::Disabled => CacheOpenPolicy::IfPresent,
                }),
                cache_failure: if spec.args.write_index.is_some()
                    && policy.mode == SnapshotMode::IfValid
                {
                    CacheFailurePolicy::Rebuild
                } else {
                    CacheFailurePolicy::Report
                },
                sync_cache: false,
            },
        )
        .map_err(|error| BackendError::new(BackendErrorKind::Open, error.to_string()))?;
        let repository = IndexedRepository::new(index)
            .map_err(|error| BackendError::new(BackendErrorKind::Open, error.to_string()))?;
        let open_ns = opened.elapsed().as_nanos();
        let snapshot_save = if spec.args.write_index.is_some() && !repository.startup().restored {
            let cache = objekte_search_core::storage::snapshot_path_in(
                &cache_directory,
                options.bigram_alphabet,
            );
            Some(
                repository
                    .index()
                    .flush_cache_with_stats(&cache)
                    .map_err(|error| BackendError::new(BackendErrorKind::Open, error.to_string()))?
                    .1,
            )
        } else {
            None
        };
        (repository, open_ns, snapshot_save)
    } else {
        open_repository(
            &paths.root,
            &configured,
            options,
            policy,
            spec.args.write_index.is_some(),
            false,
            false,
        )
        .map_err(|message| BackendError::new(BackendErrorKind::Open, message))?
    };
    if cancellation.is_cancelled() {
        return Err(BackendError::cancelled());
    }
    let startup = repository.startup().clone();
    let background_snapshot_save = if spec.args.write_index.is_some() && startup.restored {
        let result = Arc::new(Mutex::new(None));
        let result_for_worker = Arc::clone(&result);
        let index = repository.index().clone();
        let cache = objekte_search_core::storage::snapshot_path_in(
            &cache_directory,
            options.bigram_alphabet,
        );
        std::thread::Builder::new()
            .name("objekte-desktop-cache-save".into())
            .spawn(move || {
                let outcome = (|| -> Result<Option<SnapshotSaveStats>, String> {
                    let report = index
                        .wait_for_reconciliation()
                        .map_err(|error| error.to_string())?
                        .ok_or("restored cache has no reconciliation task")?;
                    if report.cancelled || !report.issues.is_empty() || report.omitted_issues > 0 {
                        return Err(format!(
                            "cache reconciliation incomplete: {} source issues",
                            report.issues.len() + report.omitted_issues
                        ));
                    }
                    if !index.status().map_err(|error| error.to_string())?.dirty() {
                        return Ok(None);
                    }
                    index
                        .flush_cache_with_stats(&cache)
                        .map(|(_, stats)| Some(stats))
                        .map_err(|error| error.to_string())
                })();
                if let Ok(mut result) = result_for_worker.lock() {
                    *result = Some(outcome);
                }
            })
            .map_err(|error| BackendError::new(BackendErrorKind::Open, error.to_string()))?;
        Some(result)
    } else {
        None
    };
    let collections: Vec<String> = repository
        .collections()
        .iter()
        .map(|collection| collection.definition.name.clone())
        .collect();
    let declared_values = Arc::new(read_declared_values(&configured.schema_files, &collections));
    let selected_cache = objekte_search_core::storage::snapshot_path_in(
        &cache_directory,
        options.bigram_alphabet,
    );
    let configured_policy = spec.configured_cache_policy.unwrap_or(CacheOpenPolicy::IfPresent);
    let cache_load = (spec.args.load_index.is_some()
        || (spec.args.repo.is_some() && configured_policy != CacheOpenPolicy::Ignore))
        .then(|| selected_cache.clone());
    let cache_save = (spec.args.write_index.is_some()
        || spec.args.load_index.is_some()
        || (spec.args.repo.is_some() && configured_policy != CacheOpenPolicy::Ignore))
        .then_some(selected_cache);
    let configuration = Arc::new(SessionConfiguration {
        definitions: Arc::new(configured.definitions),
        options,
        configured_repository: spec.args.repo.is_some(),
        cache_load,
        cache_save,
        configuration_identity: repository
            .index()
            .view()
            .map_err(|error| BackendError::new(BackendErrorKind::Open, error.to_string()))?
            .configuration_identity(),
        protected_files: Arc::new(configured.protected_files),
    });
    Ok(OpenedSession {
        repository: Arc::new(repository),
        configuration,
        args: spec.args.clone(),
        paths,
        collections,
        single_collection: !spec.args.multiple_collections(),
        declared_values,
        background_snapshot_save,
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
