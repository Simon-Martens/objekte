//! Repository API, runtime options, metrics, and cache coordination.
use crate::{
    Collection, ConfigurationChanges, Error, PathStep, RecordState, RecordStatus, RepositoryModel,
    Result, ValidationMetrics, ViewRequest, ViewResult,
    config::{digest, fingerprint},
    config_manifest::ConfigurationSnapshot,
    error::{io, record},
    query::Plan,
    schema::escape,
    source::{Source, Stamp},
    validation::Document,
};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    time::Instant,
};

mod coverage;
mod json_query;
mod reconciliation;
mod relations;
mod segment_cache;
mod source_records;
mod validation_ops;
mod views;


#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheMode {
    Disabled,
    Memory,
    Segments,
}


#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    MetadataOnOpen,
    MetadataOnRequest,
    HashOnOpen,
    HashOnRequest,
}


// INFO: Runtime cache, freshness, and worker settings.
#[derive(Debug, Clone, Serialize)]
pub struct Options {
    pub data_dir: PathBuf,
    pub cache: CacheMode,
    pub freshness: Freshness,
    pub namespace: String,
    /// Compatibility option from the former cache; segment work uses Rayon threads.
    pub ingest_workers: usize,
    /// Compatibility option; validation now runs on demand.
    pub background_workers: usize,
}


impl Options {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            cache: CacheMode::Memory,
            freshness: Freshness::MetadataOnOpen,
            namespace: "default".into(),
            background_workers: std::thread::available_parallelism()
                .map_or(1, |n| (n.get() / 4).max(1)),
            ingest_workers: std::thread::available_parallelism()
                .map_or(1, |n| (n.get() / 2).max(1)),
        }
    }
}


macro_rules! metrics {
    ($($field:ident),* $(,)?) => {
        // INFO: Cumulative operation counters and nanosecond timings.
        #[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
        pub struct Metrics {
            $(pub $field: u64,)*
        }


        impl Metrics {
            pub fn since(&self, before: &Self) -> Self {
                Self {
                    $($field: self.$field.saturating_sub(before.$field),)*
                }
            }

            fn merge(&mut self, other: &Self) {
                $(self.$field = self.$field.saturating_add(other.$field);)*
            }

        }
    };
}

metrics!(
    enumerations,
    source_reads,
    source_bytes,
    hashes,
    parses,
    validations,
    rows_inserted,
    rows_updated,
    rows_deleted,
    rows_reused,
    edges_added,
    edges_removed,
    index_builds,
    index_rows_written,
    reconciliations,
    manifest_hits,
    manifest_loads,
    metadata_reused,
    fingerprint_nanos,
    reload_nanos,
    store_open_nanos,
    configuration_nanos,
    filter_compile_nanos,
    sync_nanos,
    inventory_nanos,
    manifest_nanos,
    source_check_nanos,
    read_nanos,
    hash_nanos,
    parse_nanos,
    json_decode_nanos,
    validation_nanos,
    validation_wait_nanos,
    parallel_prepare_nanos,
    record_write_nanos,
    relation_extract_nanos,
    relation_write_nanos,
    index_nanos,
    commit_nanos,
    query_nanos,
    grep_scans,
    grep_candidates,
    grep_bytes,
    grep_nanos,
    bigram_open_nanos,
    bigram_refresh_nanos,
    bigram_candidates_nanos,
    bigram_file_open_nanos,
    bigram_read_nanos,
    bigram_regex_nanos,
    bigram_materialize_nanos,
    segment_sync_nanos,
    segment_metadata_nanos,
    segment_snapshot_nanos,
    segment_index_nanos,
    segment_verify_nanos,
    segment_materialize_nanos
);


// INFO: Record identity, source path, JSON, digest, validation state, and raw bytes. Derived from source files.
#[derive(Debug, Clone, Serialize)]
pub struct Record {
    pub collection: String,
    pub id: String,
    pub path: String,
    pub hash: String,
    pub data: Value,
    pub state: RecordState,
    /// Exact source bytes for the raw editor when JSON cannot be parsed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Vec<u8>>,
}


/// Current or deleted source-record revision retained by the selected cache.
#[derive(Debug, Clone, Serialize)]
pub struct RecordRevision {
    pub collection: String,
    pub id: String,
    pub revision: u64,
    pub changed_at_nanos: u128,
    pub present: bool,
}


#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetState {
    Unknown,
    Present,
    Missing,
}


// INFO: A source-to-target edge. Derived from source files. Target state is resolved on read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordRelation {
    pub source_collection: String,
    pub source_id: String,
    pub location: String,
    pub target_collection: String,
    pub target_id: String,
    pub target_state: TargetState,
}


// INFO: Cache completeness, configuration versions, and validation counts for a collection.
#[derive(Debug, Clone, Serialize)]
pub struct Coverage {
    pub collection: String,
    pub state: String,
    pub verified: bool,
    /// Whether existing derived data uses the currently loaded configuration.
    /// This does not assert source freshness or completed record validation.
    pub configuration_current: bool,
    pub applied_schema_hash: Option<String>,
    pub expected_schema_hash: String,
    pub records: usize,
    /// Counts by validation state; empty when no persistent coverage exists.
    pub validation: BTreeMap<String, usize>,
    pub revision: i64,
    pub manifest_hash: Option<String>,
    pub view_hash: Option<String>,
}


// INFO: Reference edges accompanied by collection coverage information.
#[derive(Debug, Clone, Serialize)]
pub struct References {
    pub edges: Vec<RecordRelation>,
    pub coverage: Vec<Coverage>,
}


// INFO: Cache location, policies, collection coverage, and configuration changes.
#[derive(Debug, Clone, Serialize)]
pub struct CacheStatus {
    pub directory: PathBuf,
    pub mode: CacheMode,
    pub freshness: Freshness,
    pub collections: Vec<Coverage>,
    pub configuration_changes: Option<ConfigurationChanges>,
}


// INFO: Stable source bytes, content digest, and descriptor stamp.
struct Raw {
    json: Vec<u8>,
    hash: String,
    stamp: Stamp,
}


// INFO: Live handle for configuration, cache writes, validation, metrics, and coverage.
pub struct Repository {
    model: RepositoryModel,
    options: Options,
    metrics: Metrics,
    verified: BTreeMap<String, i64>,
    memory_edges: BTreeMap<(String, String), Vec<RecordRelation>>,
    memory_coverage: BTreeMap<String, String>,
    segment_memory: Mutex<SegmentMemory>,
    file_pickers: BTreeMap<String, crate::file_picker::FilePicker>,
    observed_configuration: ConfigurationSnapshot,
    configuration_changes: Option<ConfigurationChanges>,
}


type SegmentMemory = BTreeMap<String, (u64, Arc<Vec<segment_cache::Shard>>)>;


impl Repository {
    /// Load configuration and schemas only. Does not enumerate records or create cache files.
    pub fn load_repository(root: impl AsRef<Path>, options: Options) -> Result<Self> {
        if options.ingest_workers == 0 || options.background_workers == 0 {
            return Err(Error::Request("worker counts must be at least 1".into()));
        }
        if options.namespace.is_empty()
            || !options
                .namespace
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(Error::Request(
                "cache namespace must contain only letters, digits, '-' or '_'".into(),
            ));
        }
        let model = RepositoryModel::load(root.as_ref())?;
        let repo = Self {
            observed_configuration: ConfigurationSnapshot::from_model(&model),
            configuration_changes: None,
            model,
            options,
            metrics: Metrics::default(),
            verified: BTreeMap::new(),
            memory_edges: BTreeMap::new(),
            memory_coverage: BTreeMap::new(),
            segment_memory: Mutex::new(BTreeMap::new()),
            file_pickers: BTreeMap::new(),
        };
        repo.cache_directory()?;
        Ok(repo)
    }

    pub fn model(&self) -> &RepositoryModel {
        &self.model
    }

    pub fn metrics(&self) -> &Metrics {
        &self.metrics
    }

    pub fn reset_metrics(&mut self) {
        self.metrics = Metrics::default()
    }

    pub fn reload(&mut self) -> Result<()> {
        let t = Instant::now();
        if self.model.changed()? {
            let model = RepositoryModel::load(&self.model.root)?;
            let previous = std::mem::replace(&mut self.model, model);
            if let Err(error) = self.cache_directory() {
                self.model = previous;
                return Err(error);
            }
            let unchanged = |name: &str| {
                previous
                    .collections
                    .get(name)
                    .zip(self.model.collections.get(name))
                    .is_some_and(|(old, new)| {
                        old.schema_hash == new.schema_hash
                            && old.mapping_hash == new.mapping_hash
                            && old.index_hash == new.index_hash
                            && old.view_hashes == new.view_hashes
                    })
            };
            self.verified.retain(|name, _| unchanged(name));
            self.memory_edges.retain(|(name, _), _| unchanged(name));
            self.memory_coverage.retain(|name, _| unchanged(name));
            self.file_pickers.clear();
        }
        self.metrics.reload_nanos += nanos(t);
        Ok(())
    }

    /// Most recent successful manifest comparison, not a durable event log.
    pub fn configuration_changes(&self) -> Option<&ConfigurationChanges> {
        self.configuration_changes.as_ref()
    }

    /// Observe configuration changes without enumerating or opening source records.
    /// With segments this creates/updates the persisted manifest. Disabled caching
    /// compares with this handle's previous snapshot and writes no application data.
    pub fn refresh_configuration(&mut self) -> Result<ConfigurationChanges> {
        self.reload()?;
        if self.options.cache == CacheMode::Segments {
            self.segment_config_refresh()?;
        } else {
            let changes = self.observed_configuration.compare(&self.model);
            self.observed_configuration = ConfigurationSnapshot::from_model(&self.model);
            self.configuration_changes = Some(changes);
        }
        Ok(self.configuration_changes.clone().unwrap())
    }

    pub fn cache_directory(&self) -> Result<PathBuf> {
        let base = future_path(&self.options.data_dir)?;
        let path = base
            .join("repositories")
            .join(digest(self.model.root.to_str().unwrap().as_bytes()))
            .join(&self.options.namespace);
        let path = future_path(&path)?;
        if path.starts_with(self.model.root.join(".editor"))
            || self
                .model
                .collections
                .values()
                .any(|c| path.starts_with(&c.path) || c.path.starts_with(&path))
        {
            return Err(Error::Request(
                "application cache must be outside source collections and .editor".into(),
            ));
        }
        Ok(path)
    }
}


pub(crate) fn validate_id(id: &str) -> Result<()> {
    if id.is_empty() || id == "." || id == ".." || id.contains(['/', '\\', '\0']) {
        Err(Error::Request(format!("invalid record ID {id:?}")))
    } else {
        Ok(())
    }
}

fn nanos(t: Instant) -> u64 {
    t.elapsed().as_nanos().min(u64::MAX as u128) as u64
}

fn future_path(path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir().map_err(|e| io(path, e))?.join(path)
    };
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(Error::Request(
            "application data path cannot contain '..'".into(),
        ));
    }
    let mut ancestor = path.as_path();
    let mut parts = Vec::new();
    while !ancestor.exists() {
        parts.push(
            ancestor
                .file_name()
                .ok_or_else(|| Error::Request("invalid application data path".into()))?
                .to_owned(),
        );
        ancestor = ancestor
            .parent()
            .ok_or_else(|| Error::Request("invalid application data path".into()))?;
    }
    let mut out = fs::canonicalize(ancestor).map_err(|e| io(ancestor, e))?;
    for part in parts.into_iter().rev() {
        out.push(part)
    }
    Ok(out)
}


impl Metrics {
    fn rollback_writes(&mut self, before: &Self) {
        self.rows_inserted = before.rows_inserted;
        self.rows_updated = before.rows_updated;
        self.rows_deleted = before.rows_deleted;
        self.rows_reused = before.rows_reused;
        self.metadata_reused = before.metadata_reused;
        self.edges_added = before.edges_added;
        self.edges_removed = before.edges_removed;
        self.index_builds = before.index_builds;
        self.index_rows_written = before.index_rows_written;
    }
}
