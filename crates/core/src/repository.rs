//! Repository API, runtime options, metrics, and in-memory index coordination.
use crate::{
    Collection, ConfigurationChanges, Error, PathStep, RecordState, RecordStatus, RepositoryModel,
    Result, ValidationMetrics, ViewRequest, ViewResult,
    config::digest,
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
    path::Path,
    time::Instant,
};

mod coverage;
mod json_query;
mod reconciliation;
mod relations;
mod source_records;
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
    /// Infer typed postings for observed JSON paths in addition to schema fields.
    pub infer_field_types: bool,
}


impl Options {
    pub fn new() -> Self {
        Self {
            cache: CacheMode::Memory,
            freshness: Freshness::MetadataOnOpen,
            infer_field_types: false,
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
    bigram_materialize_nanos
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
    pub expected_schema_hash: String,
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
    options: Options,
    metrics: Metrics,
    memory_edges: BTreeMap<(String, String), Vec<RecordRelation>>,
    memory_coverage: BTreeMap<String, String>,
    file_pickers: BTreeMap<String, crate::file_picker::FilePicker>,
    observed_configuration: ConfigurationSnapshot,
    configuration_changes: Option<ConfigurationChanges>,
}


impl Repository {
    /// Load configuration and schemas only. Does not enumerate records.
    pub fn load_repository(root: impl AsRef<Path>, options: Options) -> Result<Self> {
        let model = RepositoryModel::load(root.as_ref())?;
        Ok(Self {
            observed_configuration: ConfigurationSnapshot::from_model(&model),
            configuration_changes: None,
            model,
            options,
            metrics: Metrics::default(),
            memory_edges: BTreeMap::new(),
            memory_coverage: BTreeMap::new(),
            file_pickers: BTreeMap::new(),
        })
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
            self.memory_edges.retain(|(name, _), _| unchanged(name));
            self.memory_coverage.retain(|name, _| unchanged(name));
            self.file_pickers.clear();
        }
        self.metrics.reload_nanos += nanos(t);
        Ok(())
    }

    /// Most recent successful configuration comparison in this handle.
    pub fn configuration_changes(&self) -> Option<&ConfigurationChanges> {
        self.configuration_changes.as_ref()
    }

    /// Observe configuration changes without enumerating or opening source records.
    pub fn refresh_configuration(&mut self) -> Result<ConfigurationChanges> {
        self.reload()?;
        let changes = self.observed_configuration.compare(&self.model);
        self.observed_configuration = ConfigurationSnapshot::from_model(&self.model);
        self.configuration_changes = Some(changes);
        Ok(self.configuration_changes.clone().unwrap())
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
