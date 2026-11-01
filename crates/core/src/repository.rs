//! Repository API, runtime options, metrics, and in-memory index coordination.
use crate::{
    Collection, ConfigurationChanges, Error, JsonPath, PathStep, RecordState, RecordStatus,
    RepositoryModel, Result, ValidationMetrics, ViewRequest, ViewResult,
    bigram::BigramSnapshot,
    config::{Discovery, digest},
    config_snapshot::ConfigurationSnapshot,
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
    path::{Path, PathBuf},
    time::Instant,
};

mod coverage;
mod inference;
mod json_query;
mod reconciliation;
mod relations;
mod source_records;
mod text_query;
mod validation_ops;
mod views;


#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheMode {
    Disabled,
    Memory,
}


#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    MetadataOnOpen,
    MetadataOnRequest,
    HashOnRequest,
}


// INFO: In-memory picker and freshness settings.
#[derive(Debug, Clone, Serialize)]
pub struct Options {
    pub cache: CacheMode,
    pub freshness: Freshness,
}


impl Options {
    pub fn new() -> Self {
        Self {
            cache: CacheMode::Memory,
            freshness: Freshness::MetadataOnOpen,
        }
    }
}


impl Default for Options {
    fn default() -> Self {
        Self::new()
    }
}


// Propagate field attributes through arithmetic so feature-gated counters
// disappear from Metrics and its operations together.
macro_rules! metrics {
    ($($(#[$field_attr:meta])* $field:ident),* $(,)?) => {
        // INFO: Cumulative operation counters and nanosecond timings.
        #[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
        pub struct Metrics {
            $($(#[$field_attr])* pub $field: u64,)*
        }


        impl Metrics {
            pub fn since(&self, before: &Self) -> Self {
                let mut difference = Self::default();
                $($(#[$field_attr])* {
                    difference.$field = self.$field.saturating_sub(before.$field);
                })*
                difference
            }

            pub(crate) fn add_from(&mut self, other: &Self) {
                $($(#[$field_attr])* {
                    self.$field = self.$field.saturating_add(other.$field);
                })*
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
    reload_nanos,
    filter_compile_nanos,
    inventory_nanos,
    source_check_nanos,
    read_nanos,
    hash_nanos,
    parse_nanos,
    json_decode_nanos,
    validation_nanos,
    validation_wait_nanos,
    query_nanos,
    grep_scans,
    grep_candidates,
    grep_bytes,
    grep_nanos,
    bigram_open_nanos,
    bigram_snapshot_loads,
    bigram_rebuild_nanos,
    #[cfg(feature = "index-timings")]
    bigram_inventory_nanos,
    #[cfg(feature = "index-timings")]
    bigram_slots_nanos,
    #[cfg(feature = "index-timings")]
    bigram_scan_nanos,
    #[cfg(feature = "index-timings")]
    bigram_project_worker_nanos,
    #[cfg(feature = "index-timings")]
    bigram_add_worker_nanos,
    #[cfg(feature = "index-timings")]
    bigram_compress_nanos,
    #[cfg(feature = "index-timings")]
    bigram_typed_nanos,
    #[cfg(feature = "index-timings")]
    bigram_view_nanos,
    bigram_refresh_nanos,
    bigram_candidates_nanos,
    bigram_file_open_nanos,
    bigram_read_nanos,
    bigram_regex_nanos,
    bigram_materialize_nanos,
    fts_open_nanos,
    fts_candidates
);


// INFO: Record identity, source path, JSON, digest, validation state, and raw bytes. Derived from source files.
#[derive(Debug, Clone, Serialize)]
pub struct Record {
    pub collection: String,
    pub id: String,
    pub path: String,
    pub size_bytes: u64,
    /// Signed Unix nanoseconds as a decimal string, preserving pre-epoch times.
    pub modified_unix_nanos: String,
    pub hash: String,
    pub data: Value,
    pub state: RecordState,
    /// Exact source bytes for the raw editor when JSON cannot be parsed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Vec<u8>>,
}


/// Current or deleted source-record revision retained by the repository handle.
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


// INFO: In-memory collection coverage and validation counts.
#[derive(Debug, Clone, Serialize)]
pub struct Coverage {
    pub collection: String,
    pub state: String,
    pub verified: bool,
    /// Whether existing derived data uses the currently loaded configuration.
    /// This does not assert source freshness or completed record validation.
    pub configuration_current: bool,
    pub expected_schema_hash: Option<String>,
    pub records: usize,
}


// INFO: Reference edges accompanied by collection coverage information.
#[derive(Debug, Clone, Serialize)]
pub struct References {
    pub edges: Vec<RecordRelation>,
    pub coverage: Vec<Coverage>,
}


// INFO: Picker policy, collection coverage, and configuration changes.
#[derive(Debug, Clone, Serialize)]
pub struct CacheStatus {
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


// INFO: Live handle for configuration, in-memory indexes, metrics, and coverage.
pub struct Repository {
    model: RepositoryModel,
    discovery: Option<Discovery>,
    options: Options,
    metrics: Metrics,
    memory_edges: BTreeMap<(String, String), Vec<RecordRelation>>,
    memory_coverage: BTreeMap<String, String>,
    file_pickers: BTreeMap<String, crate::file_picker::FilePicker>,
    requested_postings: BTreeMap<String, BTreeSet<JsonPath>>,
    auto_postings: BTreeMap<String, BTreeSet<JsonPath>>,
    observed_configuration: ConfigurationSnapshot,
    configuration_changes: Option<ConfigurationChanges>,
}


/// Code-level repository opening with optional schema bases.
pub struct RepositoryBuilder {
    root: std::path::PathBuf,
    options: Options,
    discovery: Discovery,
}


impl RepositoryBuilder {
    /// Set cache and freshness policies before opening.
    pub fn options(mut self, options: Options) -> Self {
        self.options = options;
        self
    }

    /// Discover collections below this root instead of the default `data` folder.
    pub fn data_root(mut self, path: impl AsRef<Path>) -> Self {
        self.discovery.data_root = Some(path.as_ref().to_owned());
        self
    }

    /// Ignore conventional schema files; explicit `schema` calls still apply.
    pub fn without_default_schemas(mut self) -> Self {
        self.discovery.use_default_schemas = false;
        self
    }

    /// Attach a schema to a folder relative to the data root. Its entire
    /// subtree becomes one collection. Schema paths are relative to the repo.
    pub fn schema(mut self, base: impl AsRef<Path>, path: impl AsRef<Path>) -> Self {
        self.discovery
            .schemas
            .push((base.as_ref().to_owned(), path.as_ref().to_owned()));
        self
    }

    /// Open the repository using discovered collection boundaries.
    pub fn open(self) -> Result<Repository> {
        let model = RepositoryModel::discover(&self.root, &self.discovery)?;
        Ok(Repository::from_model(
            model,
            self.options,
            Some(self.discovery),
        ))
    }
}


impl Repository {
    /// Discover populated JSON folders as collections using the default layout.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        Self::builder(root).open()
    }

    /// Discover collections and attach one explicitly supplied schema base.
    pub fn open_with_schema(
        root: impl AsRef<Path>,
        base: impl AsRef<Path>,
        schema: impl AsRef<Path>,
    ) -> Result<Self> {
        Self::builder(root).schema(base, schema).open()
    }

    /// Configure discovery and optional per-collection schemas in code.
    pub fn builder(root: impl AsRef<Path>) -> RepositoryBuilder {
        RepositoryBuilder {
            root: root.as_ref().to_owned(),
            options: Options::new(),
            discovery: Discovery::default(),
        }
    }

    /// Use the legacy manifest when present; otherwise discover JSON folders.
    /// Discovery lists filenames but does not parse records or build indexes.
    pub fn load_repository(root: impl AsRef<Path>, options: Options) -> Result<Self> {
        if root.as_ref().join(".editor/repository.json").is_file()
            && !root.as_ref().join(".objekte/collections").is_dir()
            && !root.as_ref().join(".objekte/views").is_dir()
        {
            return Ok(Self::from_model(
                RepositoryModel::load(root.as_ref())?,
                options,
                None,
            ));
        }
        let discovery = Discovery::default();
        let model = RepositoryModel::discover(root.as_ref(), &discovery)?;
        Ok(Self::from_model(model, options, Some(discovery)))
    }

    fn from_model(model: RepositoryModel, options: Options, discovery: Option<Discovery>) -> Self {
        Self {
            observed_configuration: ConfigurationSnapshot::from_model(&model),
            configuration_changes: None,
            model,
            discovery,
            options,
            metrics: Metrics::default(),
            memory_edges: BTreeMap::new(),
            memory_coverage: BTreeMap::new(),
            file_pickers: BTreeMap::new(),
            requested_postings: BTreeMap::new(),
            auto_postings: BTreeMap::new(),
        }
    }

    pub fn model(&self) -> &RepositoryModel {
        &self.model
    }

    /// Repository-owned path for one collection's bigram snapshot.
    pub fn bigram_cache_path(&self, collection: &str) -> Result<PathBuf> {
        self.collection(collection)?;
        use sha2::{Digest, Sha256};
        let name = format!("{:x}", Sha256::digest(collection.as_bytes()));
        Ok(self
            .model
            .root
            .join(".objekte/.cache")
            .join(format!("bigram-{name}.bin")))
    }

    /// Copy parsed configuration into an independent handle with no indexes.
    /// A UI can warm this handle on another thread and adopt it when ready.
    pub fn fork_unindexed(&self) -> Self {
        Self::from_model(
            self.model.clone(),
            self.options.clone(),
            self.discovery.clone(),
        )
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
            let model = if let Some(settings) = &self.discovery {
                RepositoryModel::discover(&self.model.root, settings)?
            } else {
                RepositoryModel::load(&self.model.root)?
            };
            self.replace_model(model);
        }
        self.metrics.reload_nanos += nanos(t);
        Ok(())
    }

    fn replace_model(&mut self, model: RepositoryModel) {
        let previous = std::mem::replace(&mut self.model, model);
        let unchanged = previous
            .collections
            .iter()
            .filter_map(|(name, old)| {
                self.model.collections.get(name).and_then(|new| {
                    (old.path == new.path
                        && old.recursive == new.recursive
                        && old.schema_hash == new.schema_hash
                        && old.mapping_hash == new.mapping_hash
                        && old.index_hash == new.index_hash
                        && old.view_hashes == new.view_hashes)
                        .then(|| name.clone())
                })
            })
            .collect::<BTreeSet<_>>();
        // A newly discovered sibling does not invalidate a warm collection.
        self.memory_edges
            .retain(|(name, _), _| unchanged.contains(name));
        self.memory_coverage
            .retain(|name, _| unchanged.contains(name));
        self.file_pickers.retain(|name, _| unchanged.contains(name));
        self.requested_postings
            .retain(|name, _| self.model.collections.contains_key(name));
        self.auto_postings
            .retain(|name, _| self.model.collections.contains_key(name));
    }

    /// Most recent successful configuration comparison in this handle.
    pub fn configuration_changes(&self) -> Option<&ConfigurationChanges> {
        self.configuration_changes.as_ref()
    }

    /// Observe configuration changes without enumerating or opening source records.
    pub fn refresh_configuration(&mut self) -> Result<ConfigurationChanges> {
        self.reload()?;
        let current = if let Some(settings) = &self.discovery {
            RepositoryModel::discover(&self.model.root, settings)?
        } else {
            RepositoryModel::load(&self.model.root)?
        };
        if current.collections.keys().collect::<Vec<_>>()
            != self.model.collections.keys().collect::<Vec<_>>()
            || current.files != self.model.files
        {
            self.replace_model(current);
        }
        let changes = self.observed_configuration.compare(&self.model);
        self.observed_configuration = ConfigurationSnapshot::from_model(&self.model);
        self.configuration_changes = Some(changes);
        Ok(self.configuration_changes.clone().unwrap())
    }
}


pub(crate) fn validate_id(id: &str) -> Result<()> {
    if id.contains(['\\', '\0'])
        || id
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        Err(Error::Request(format!("invalid record ID {id:?}")))
    } else {
        Ok(())
    }
}

fn nanos(t: Instant) -> u64 {
    t.elapsed().as_nanos().min(u64::MAX as u128) as u64
}
