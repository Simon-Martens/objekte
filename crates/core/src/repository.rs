//! Repository API, runtime options, metrics, and in-memory index coordination.
use crate::{
    Collection, ConfigurationChanges, Error, JsonPath, PathStep, RecordState, RecordStatus,
    RepositoryModel, Result, ValidationMetrics, ViewRequest, ViewResult,
    bigram::{BigramEncoding, BigramSnapshot},
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
mod derived_query;
mod extension_derived_index;
mod extension_index;
mod extension_index_snapshot;
mod extension_projection;
mod extension_query;
mod extension_sources;
mod inference;
mod json_query;
mod ordered_result_cache;
mod predicate_cache;
mod reconciliation;
mod relations;
mod selected_lifecycle;
mod source_changes;
mod source_records;
mod text_query;
mod validation_ops;
mod value_counts;

pub use derived_query::{
    DerivedBackend, DerivedPredicate, DerivedQuery, DerivedQueryResult, DerivedSort,
};
pub use extension_projection::SelectedIndexRequest;
pub use extension_query::{ExtensionQueryResult, ExtensionQueryRow};
pub use extension_sources::DecodedLeaf;
pub use value_counts::ValueCount;
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
    pub bigram_encoding: BigramEncoding,
    /// Maximum number of matches accessible across all pages of a query.
    pub total_limit: usize,
    /// Process-local verified-query cache budget in bytes; zero disables it.
    pub query_cache_bytes: usize,
    /// Allow streamed searches to reuse verified results without rechecking
    /// non-page source files. Intended for interactive UIs with reconciliation.
    pub stream_query_cache: bool,
}


impl Options {
    pub fn new() -> Self {
        Self {
            cache: CacheMode::Memory,
            freshness: Freshness::MetadataOnOpen,
            bigram_encoding: BigramEncoding::Ascii,
            total_limit: 10_000,
            query_cache_bytes: 64 * 1024 * 1024,
            stream_query_cache: false,
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
    selected_candidate_visits,
    selected_descriptor_visits,
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
    secondary_snapshot_loads,
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
    bigram_rendered_compress_nanos,
    #[cfg(feature = "index-timings")]
    bigram_field_compress_nanos,
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
    fts_candidates,
    predicate_cache_hits,
    ordered_result_hits,
    partial_evaluation_hits,
    cache_evictions
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


/// A source edit the application reports after changing files itself.
/// Paths are absolute or relative to the repository root and end in `.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceChange {
    Created(PathBuf),
    Changed(PathBuf),
    Deleted(PathBuf),
    Moved { from: PathBuf, to: PathBuf },
}


/// Record IDs that a refresh created, changed, or deleted in one collection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceChanges {
    pub created: Vec<String>,
    pub changed: Vec<String>,
    pub deleted: Vec<String>,
}


impl SourceChanges {
    pub fn len(&self) -> usize {
        self.created.len() + self.changed.len() + self.deleted.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}


/// The outcome of `diff` or `apply_source_changes` for one collection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionChanges {
    #[serde(flatten)]
    pub changes: SourceChanges,
    /// False when this handle had no index for the collection yet. Its next
    /// load diffs any disk cache against the source files.
    pub indexed: bool,
    /// The disk cache rewritten after the changes, if one exists.
    pub saved_cache: Option<PathBuf>,
}


/// A source inventory collected separately from a repository's live index.
pub struct CollectionInventory {
    collection: String,
    path: PathBuf,
    recursive: bool,
    files: BTreeMap<String, Stamp>,
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
    /// Effective bigram encoding for each collection, including schema overrides.
    pub bigram_encodings: BTreeMap<String, BigramEncoding>,
    pub collections: Vec<Coverage>,
    pub configuration_changes: Option<ConfigurationChanges>,
}


// INFO: Stable source bytes, content digest, and descriptor stamp.
struct Raw {
    json: Vec<u8>,
    hash: String,
    stamp: Stamp,
    created: Option<i128>,
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
    query_cache: predicate_cache::VerifiedPredicateCache,
    extensions: Option<std::sync::Arc<crate::extensions::Extensions>>,
    custom_source_collections: BTreeSet<String>,
    /// Frozen selected descriptors for custom collections under MetadataOnOpen.
    custom_source_snapshot: BTreeMap<String, CustomSourceSnapshot>,
    extension_text_indexes:
        BTreeMap<extension_index::RepresentationIndexKey, extension_index::RepresentationIndex>,
    selected_derived_indexes: BTreeMap<
        extension_derived_index::SelectedDerivedKey,
        extension_derived_index::SelectedDerivedPosting,
    >,
    invalid_selected_collections: BTreeSet<String>,
}


#[derive(Clone)]
struct CustomSourceSnapshot {
    sources: std::sync::Arc<Vec<crate::extensions::SourceDescriptor>>,
    /// Frozen routing fact for this exact inventory generation.
    has_custom: bool,
    native_count: usize,
    custom_slots: std::sync::Arc<Vec<u32>>,
    /// Native picker slots are mapped once to this selected generation.
    native_slot_map: std::sync::Arc<std::sync::OnceLock<Option<Vec<Option<u32>>>>>,
    /// Default source order and its inverse, built once for this generation.
    default_order: std::sync::Arc<Vec<u32>>,
    default_rank: std::sync::Arc<Vec<u32>>,
    fingerprint: String,
    source_generation: String,
    cacheable: bool,
    definitions: Vec<crate::extensions::Descriptor>,
}


type PendingSelectedOrder = (ordered_result_cache::OrderedResultKey, Vec<u32>);


impl CustomSourceSnapshot {
    fn new(
        collection: &Collection,
        sources: Vec<crate::extensions::SourceDescriptor>,
        extensions: &crate::extensions::Extensions,
    ) -> Result<Self> {
        if sources.len() > u32::MAX as usize {
            return Err(Error::Request(
                "selected source table exceeds 2^32 slots".into(),
            ));
        }
        let (fingerprint, cacheable, definitions) =
            extensions.selection_dependencies(collection, &sources)?;
        let source_generation = crate::config::fingerprint("selected-sources/v1", &sources);
        let has_custom = sources
            .iter()
            .any(|source| source.provider != crate::source::FILESYSTEM_JSON_IDENTITY);
        let custom_slots = sources
            .iter()
            .enumerate()
            .filter(|(_, source)| source.provider != crate::source::FILESYSTEM_JSON_IDENTITY)
            .map(|(slot, _)| slot as u32)
            .collect::<Vec<_>>();
        let native_count = sources.len() - custom_slots.len();
        let mut default_order = (0..sources.len() as u32).collect::<Vec<_>>();
        default_order.sort_unstable_by(|&left, &right| {
            let a = &sources[left as usize];
            let b = &sources[right as usize];
            let modified = |source: &crate::extensions::SourceDescriptor| {
                source
                    .metadata
                    .get("modified_unix_nanos")
                    .and_then(|value| value.parse::<i128>().ok())
                    .unwrap_or(0)
            };
            modified(b)
                .cmp(&modified(a))
                .then_with(|| a.record_id.cmp(&b.record_id))
        });
        let mut default_rank = vec![0_u32; sources.len()];
        for (rank, &slot) in default_order.iter().enumerate() {
            default_rank[slot as usize] = rank as u32;
        }
        Ok(Self {
            sources: std::sync::Arc::new(sources),
            has_custom,
            native_count,
            custom_slots: std::sync::Arc::new(custom_slots),
            native_slot_map: std::sync::Arc::new(std::sync::OnceLock::new()),
            default_order: std::sync::Arc::new(default_order),
            default_rank: std::sync::Arc::new(default_rank),
            fingerprint,
            source_generation,
            cacheable,
            definitions,
        })
    }

    fn native_slot_map(&self, picker: &crate::file_picker::FilePicker) -> Option<&[Option<u32>]> {
        self.native_slot_map
            .get_or_init(|| {
                let mut map = vec![None; picker.slot_count()];
                let mut found = 0;
                for &slot in picker.sorted_slots() {
                    let (id, stamp) = picker.view_record(slot);
                    let index = self
                        .sources
                        .binary_search_by(|source| source.record_id.as_str().cmp(id))
                        .ok()?;
                    let source = &self.sources[index];
                    if source.provider != crate::source::FILESYSTEM_JSON_IDENTITY
                        || source.revision != crate::config::fingerprint("source-stamp/v1", stamp)
                    {
                        return None;
                    }
                    map[slot] = Some(index as u32);
                    found += 1;
                }
                (found == self.native_count).then_some(map)
            })
            .as_deref()
    }

    /// Enumerate only candidate slots where a posting is selective. The
    /// default order is generation-bound, so a warm query never sorts the
    /// complete inventory again.
    fn scan_slots(&self, candidates: Option<&[u64]>, default_order: bool) -> SelectedScanSlots<'_> {
        let Some(bits) = candidates else {
            return if default_order {
                SelectedScanSlots::Default(self.default_order.iter())
            } else {
                SelectedScanSlots::All(0..self.sources.len())
            };
        };
        let mut slots = Vec::new();
        for (word_index, &value) in bits.iter().enumerate() {
            let mut word = value;
            while word != 0 {
                let bit = word.trailing_zeros() as usize;
                let slot = word_index * 64 + bit;
                if slot < self.sources.len() {
                    slots.push(slot as u32);
                }
                word &= word - 1;
            }
        }
        if default_order {
            slots.sort_unstable_by_key(|&slot| self.default_rank[slot as usize]);
        }
        SelectedScanSlots::Candidates(slots.into_iter())
    }
}


enum SelectedScanSlots<'a> {
    Default(std::slice::Iter<'a, u32>),
    All(std::ops::Range<usize>),
    Candidates(std::vec::IntoIter<u32>),
}


impl Iterator for SelectedScanSlots<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Default(slots) => slots.next().map(|slot| *slot as usize),
            Self::All(slots) => slots.next(),
            Self::Candidates(slots) => slots.next().map(|slot| slot as usize),
        }
    }
}


/// Code-level repository opening with optional schema bases.
pub struct RepositoryBuilder {
    root: std::path::PathBuf,
    options: Options,
    discovery: Discovery,
    extensions: crate::extensions::Extensions,
}


impl RepositoryBuilder {
    /// Register a statically linked source provider before opening.
    pub fn register_source(
        mut self,
        source: impl crate::extensions::SourceProvider + 'static,
    ) -> Result<Self> {
        self.extensions.register_source(source)?;
        Ok(self)
    }

    /// Register a statically linked format decoder before opening.
    pub fn register_format(
        mut self,
        format: impl crate::extensions::FormatDecoder + 'static,
    ) -> Result<Self> {
        self.extensions.register_format(format)?;
        Ok(self)
    }

    /// Register a text representation before opening.
    pub fn register_representation(
        mut self,
        representation: impl crate::extensions::TextRepresentation + 'static,
    ) -> Result<Self> {
        self.extensions.register_representation(representation)?;
        Ok(self)
    }

    /// Register a derived typed-value extractor before opening.
    pub fn register_extractor(
        mut self,
        extractor: impl crate::extensions::ValueExtractor + 'static,
    ) -> Result<Self> {
        self.extensions.register_extractor(extractor)?;
        Ok(self)
    }

    /// Register an exact operator before opening.
    pub fn register_operator(
        mut self,
        operator: impl crate::extensions::QueryOperator + 'static,
    ) -> Result<Self> {
        self.extensions.register_operator(operator)?;
        Ok(self)
    }

    /// Register a query frontend before opening.
    pub fn register_frontend(
        mut self,
        frontend: impl crate::extensions::QueryFrontend + 'static,
    ) -> Result<Self> {
        self.extensions.register_frontend(frontend)?;
        Ok(self)
    }

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
    pub fn open(mut self) -> Result<Repository> {
        if self.options.total_limit == 0 {
            return Err(Error::Request(
                "repository total_limit must be positive".into(),
            ));
        }
        self.extensions.ensure_source_representations()?;
        self.discovery
            .extension_annotations
            .extend(self.extensions.schema_annotations());
        self.discovery.source_providers = self.extensions.source_providers().cloned().collect();
        self.discovery.source_formats = self.extensions.format_identities().cloned().collect();
        let model = RepositoryModel::discover(&self.root, &self.discovery)?;
        if !self.extensions.is_empty() {
            self.extensions.bind_schemas(&model)?;
        }
        let extensions =
            (!self.extensions.is_empty()).then(|| std::sync::Arc::new(self.extensions));
        let mut repository = Repository::from_model_with_extensions(
            model,
            self.options,
            Some(self.discovery),
            extensions,
        );
        if repository
            .extensions
            .as_ref()
            .is_some_and(|extensions| extensions.source_providers().next().is_some())
        {
            for name in repository.model.collections.keys() {
                let applicable = repository.extensions.as_ref().is_some_and(|extensions| {
                    extensions
                        .source_providers()
                        .any(|provider| provider.may_select(name))
                });
                let selected = repository.source_descriptors(name)?;
                if selected
                    .iter()
                    .any(|source| source.provider != "objekte:filesystem")
                {
                    repository.custom_source_collections.insert(name.clone());
                }
                if applicable {
                    repository.custom_source_snapshot.insert(
                        name.clone(),
                        CustomSourceSnapshot::new(
                            repository.model.collection(name)?,
                            selected,
                            repository.extensions.as_deref().unwrap(),
                        )?,
                    );
                }
            }
        }
        Ok(repository)
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
            extensions: crate::extensions::Extensions::default(),
        }
    }

    /// Use the legacy manifest when present; otherwise discover JSON folders.
    /// Discovery lists filenames but does not parse records or build indexes.
    pub fn load_repository(root: impl AsRef<Path>, options: Options) -> Result<Self> {
        if options.total_limit == 0 {
            return Err(Error::Request(
                "repository total_limit must be positive".into(),
            ));
        }
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
        Self::from_model_with_extensions(model, options, discovery, None)
    }

    fn from_model_with_extensions(
        model: RepositoryModel,
        options: Options,
        discovery: Option<Discovery>,
        extensions: Option<std::sync::Arc<crate::extensions::Extensions>>,
    ) -> Self {
        let query_cache = predicate_cache::VerifiedPredicateCache::new(
            if options.cache == CacheMode::Memory && options.freshness != Freshness::HashOnRequest {
                options.query_cache_bytes
            } else {
                0
            },
        );
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
            query_cache,
            extensions,
            custom_source_collections: BTreeSet::new(),
            custom_source_snapshot: BTreeMap::new(),
            extension_text_indexes: BTreeMap::new(),
            selected_derived_indexes: BTreeMap::new(),
            invalid_selected_collections: BTreeSet::new(),
        }
    }

    pub fn model(&self) -> &RepositoryModel {
        &self.model
    }

    /// Repository-owned path for one collection's bigram snapshot.
    pub fn bigram_cache_path(&self, collection: &str) -> Result<PathBuf> {
        self.model.collection(collection)?;
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
        let mut fork = Self::from_model_with_extensions(
            self.model.clone(),
            self.options.clone(),
            self.discovery.clone(),
            self.extensions.clone(),
        );
        fork.custom_source_collections = self.custom_source_collections.clone();
        fork.custom_source_snapshot = self.custom_source_snapshot.clone();
        fork.invalid_selected_collections = self.invalid_selected_collections.clone();
        fork
    }

    /// Immutable, registration-order-independent extension definitions.
    pub fn extensions(&self) -> Option<&crate::extensions::Extensions> {
        self.extensions.as_deref()
    }

    pub fn metrics(&self) -> &Metrics {
        &self.metrics
    }

    /// Allocated bytes estimated for verified predicates and ordered slot lists.
    pub fn query_cache_bytes(&self) -> usize {
        self.query_cache.bytes()
    }

    pub fn reset_metrics(&mut self) {
        self.metrics = Metrics::default()
    }

    /// Explicitly rediscover repository configuration and retain indexes for
    /// collections whose effective configuration has not changed.
    pub fn reload(&mut self) -> Result<()> {
        let t = Instant::now();
        let previous_selected = self
            .custom_source_snapshot
            .iter()
            .map(|(name, snapshot)| {
                (
                    name.clone(),
                    (
                        snapshot.source_generation.clone(),
                        snapshot.fingerprint.clone(),
                    ),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let model = if let Some(settings) = &self.discovery {
            RepositoryModel::discover(&self.model.root, settings)?
        } else {
            RepositoryModel::load(&self.model.root)?
        };
        let next_extensions = if let Some(extensions) = &self.extensions {
            let mut next = (**extensions).clone();
            next.bind_schemas(&model)?;
            Some(std::sync::Arc::new(next))
        } else {
            None
        };
        let mut custom_sources = BTreeSet::new();
        let mut custom_snapshot = BTreeMap::new();
        if next_extensions
            .as_ref()
            .is_some_and(|extensions| extensions.source_providers().next().is_some())
        {
            for (name, collection) in &model.collections {
                let applicable = next_extensions.as_ref().is_some_and(|extensions| {
                    extensions
                        .source_providers()
                        .any(|provider| provider.may_select(name))
                });
                let selected = extension_sources::select_sources(
                    collection,
                    name,
                    next_extensions.as_deref(),
                )?;
                if selected
                    .iter()
                    .any(|source| source.provider != "objekte:filesystem")
                {
                    custom_sources.insert(name.clone());
                }
                if applicable {
                    custom_snapshot.insert(
                        name.clone(),
                        CustomSourceSnapshot::new(
                            collection,
                            selected,
                            next_extensions.as_deref().unwrap(),
                        )?,
                    );
                }
            }
        }
        self.extensions = next_extensions;
        self.replace_model(model);
        self.custom_source_collections = custom_sources;
        self.custom_source_snapshot = custom_snapshot;
        self.invalid_selected_collections.clear();
        for (name, snapshot) in &self.custom_source_snapshot {
            if previous_selected.get(name)
                != Some(&(
                    snapshot.source_generation.clone(),
                    snapshot.fingerprint.clone(),
                ))
            {
                self.query_cache.invalidate(name);
            }
        }
        for name in previous_selected.keys() {
            if !self.custom_source_snapshot.contains_key(name) {
                self.query_cache.invalidate(name);
            }
        }
        self.extension_text_indexes.retain(|key, _| {
            self.custom_source_snapshot
                .get(&key.collection)
                .is_some_and(|snapshot| snapshot.fingerprint == key.selection)
        });
        self.selected_derived_indexes.retain(|key, _| {
            self.custom_source_snapshot
                .get(&key.collection)
                .is_some_and(|snapshot| snapshot.fingerprint == key.selection)
        });
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
        self.extension_text_indexes
            .retain(|key, _| unchanged.contains(&key.collection));
        self.selected_derived_indexes
            .retain(|key, _| unchanged.contains(&key.collection));
        self.requested_postings
            .retain(|name, _| self.model.collections.contains_key(name));
        self.auto_postings
            .retain(|name, _| self.model.collections.contains_key(name));
        for name in previous.collections.keys() {
            if !unchanged.contains(name) {
                self.query_cache.invalidate(name);
            }
        }
    }

    /// Most recent successful configuration comparison in this handle.
    pub fn configuration_changes(&self) -> Option<&ConfigurationChanges> {
        self.configuration_changes.as_ref()
    }

    /// Explicitly reload configuration and report changes since the previous
    /// configuration observation.
    pub fn refresh_configuration(&mut self) -> Result<ConfigurationChanges> {
        self.reload()?;
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
