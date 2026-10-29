use crate::{
    Collection, ConfigurationChanges, Error, PathStep, RecordState, RecordStatus, RepositoryModel,
    Result, ValidationMetrics, ValidationMode, ViewRequest, ViewResult,
    config::{digest, fingerprint},
    config_manifest::ConfigurationSnapshot,
    error::{io, record},
    query::Plan,
    schema::escape,
    source::{Source, Stamp},
    store::{self, Store, Writer},
    validation::{Document, Validator},
};
use rayon::prelude::*;
use rusqlite::{OptionalExtension, params, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Instant,
};
mod json_query;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheMode {
    Disabled,
    Sqlite,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    MetadataOnOpen,
    MetadataOnRequest,
    HashOnOpen,
    HashOnRequest,
}
#[derive(Debug, Clone, Serialize)]
pub struct Options {
    pub data_dir: PathBuf,
    pub cache: CacheMode,
    pub freshness: Freshness,
    pub namespace: String,
    /// Suggested SQLite page-cache budget per connection. None uses SQLite's default.
    pub sqlite_cache_mib: Option<u32>,
    /// Workers for reading, hashing, and parsing an empty collection cache.
    /// SQLite writes remain ordered on one connection. One disables parallel preparation.
    pub ingest_workers: usize,
    pub background_workers: usize,
    pub validation: ValidationMode,
    /// Emit one timed SQL statement per line to stderr while the SQLite cache is used.
    pub sql_diagnostics: bool,
}
impl Options {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            cache: CacheMode::Sqlite,
            freshness: Freshness::MetadataOnOpen,
            namespace: "default".into(),
            sqlite_cache_mib: Some(64),
            background_workers: std::thread::available_parallelism()
                .map_or(1, |n| (n.get() / 4).max(1)),
            validation: ValidationMode::Background,
            sql_diagnostics: false,
            ingest_workers: std::thread::available_parallelism()
                .map_or(1, |n| (n.get() / 2).max(1)),
        }
    }
}
macro_rules! metrics {($($f:ident),*)=>{
    #[derive(Debug,Default,Clone,PartialEq,Eq,Serialize)]pub struct Metrics{$(pub $f:u64,)*}
    impl Metrics {
        pub fn since(&self,before:&Self)->Self{Self{$($f:self.$f.saturating_sub(before.$f),)*}}
        fn merge(&mut self, other: &Self) { $(self.$f = self.$f.saturating_add(other.$f);)* }
    }
}}
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
    parallel_prepare_nanos,
    record_write_nanos,
    relation_extract_nanos,
    relation_write_nanos,
    index_nanos,
    commit_nanos,
    query_nanos
);
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetState {
    Unknown,
    Present,
    Missing,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordRelation {
    pub source_collection: String,
    pub source_id: String,
    pub location: String,
    pub target_collection: String,
    pub target_id: String,
    pub target_state: TargetState,
}
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
#[derive(Debug, Clone, Serialize)]
pub struct References {
    pub edges: Vec<RecordRelation>,
    pub coverage: Vec<Coverage>,
}
#[derive(Debug, Clone, Serialize)]
pub struct CacheStatus {
    pub directory: PathBuf,
    pub mode: CacheMode,
    pub freshness: Freshness,
    pub collections: Vec<Coverage>,
    pub configuration_changes: Option<ConfigurationChanges>,
}
struct Raw {
    json: Vec<u8>,
    hash: String,
    stamp: Stamp,
}
pub struct Repository {
    writer: Option<Arc<Writer>>,
    validator: Option<Validator>,
    model: RepositoryModel,
    options: Options,
    metrics: Metrics,
    verified: BTreeMap<String, i64>,
    memory_edges: BTreeMap<(String, String), Vec<RecordRelation>>,
    memory_coverage: BTreeMap<String, String>,
    observed_configuration: ConfigurationSnapshot,
    configuration_changes: Option<ConfigurationChanges>,
}
impl Repository {
    /// Load configuration and schemas only. Does not enumerate records or create cache files.
    pub fn load_repository(root: impl AsRef<Path>, options: Options) -> Result<Self> {
        if options.ingest_workers == 0 || options.background_workers == 0 {
            return Err(Error::Request("worker counts must be at least 1".into()));
        }
        if options
            .sqlite_cache_mib
            .is_some_and(|mib| mib == 0 || mib > (i32::MAX as u32) / 1024)
        {
            return Err(Error::Request(
                "sqlite_cache_mib must be between 1 and 2097151, or None for SQLite's default"
                    .into(),
            ));
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
            writer: None,
            validator: None,
            observed_configuration: ConfigurationSnapshot::from_model(&model),
            configuration_changes: None,
            model,
            options,
            metrics: Metrics::default(),
            verified: BTreeMap::new(),
            memory_edges: BTreeMap::new(),
            memory_coverage: BTreeMap::new(),
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
        }
        self.metrics.reload_nanos += nanos(t);
        Ok(())
    }
    /// Most recent successful manifest comparison, not a durable event log.
    pub fn configuration_changes(&self) -> Option<&ConfigurationChanges> {
        self.configuration_changes.as_ref()
    }
    /// Observe configuration changes without enumerating or opening source records.
    /// With SQLite this creates/updates the persisted manifest. Disabled caching
    /// compares with this handle's previous snapshot and writes no application data.
    pub fn refresh_configuration(&mut self) -> Result<ConfigurationChanges> {
        self.reload()?;
        if self.options.cache == CacheMode::Sqlite {
            self.store()?;
        } else {
            let changes = self
                .observed_configuration
                .compare(&self.model, &BTreeSet::new());
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
    fn store(&mut self) -> Result<Arc<Writer>> {
        let t = Instant::now();
        let writer = match &self.writer {
            Some(w) => w.clone(),
            None => {
                let w = Writer::open(
                    &self.cache_directory()?,
                    &self.model.root,
                    self.options.sqlite_cache_mib,
                    self.options.sql_diagnostics,
                )?;
                self.writer = Some(w.clone());
                w
            }
        };
        let s = writer.enter(self.options.sqlite_cache_mib, self.options.sql_diagnostics)?;
        self.metrics.store_open_nanos += nanos(t);
        let t = Instant::now();
        let result = s.reconcile_configuration(&self.model);
        self.metrics.configuration_nanos += nanos(t);
        self.configuration_changes = Some(result?);
        drop(s);
        Ok(writer)
    }
    fn collection(&self, name: &str) -> Result<Collection> {
        Ok(self.model.collection(name)?.clone())
    }
    fn inventory(&mut self, c: &Collection) -> Result<(Source, BTreeMap<String, Stamp>)> {
        let t = Instant::now();
        let source = Source::open(&c.path)?;
        self.metrics.enumerations += 1;
        let files = source.inventory()?;
        self.metrics.inventory_nanos += nanos(t);
        Ok((source, files))
    }
    fn check_source(&self, c: &Collection) -> Result<()> {
        let actual = fs::canonicalize(&c.path).map_err(|e| io(&c.path, e))?;
        if actual != c.path || !actual.starts_with(&self.model.root) {
            return Err(record(
                &c.path,
                "",
                "source directory identity changed; reload repository",
            ));
        }
        Ok(())
    }
    fn raw(&mut self, c: &Collection, id: &str) -> Result<Raw> {
        let t = Instant::now();
        let source = Source::open(&c.path)?;
        self.metrics.source_check_nanos += nanos(t);
        let raw = self.raw_from(&source, id, None)?;
        source.finish()?;
        Ok(raw)
    }
    fn raw_from(&mut self, source: &Source, id: &str, expected: Option<&Stamp>) -> Result<Raw> {
        Self::read_raw(&mut self.metrics, source, id, expected)
    }
    fn read_raw(
        metrics: &mut Metrics,
        source: &Source,
        id: &str,
        expected: Option<&Stamp>,
    ) -> Result<Raw> {
        let path = source.path.join(format!("{id}.json"));
        for _ in 0..3 {
            let t = Instant::now();
            let mut file = source.file(id)?;
            let before = Stamp::from_metadata(&file.metadata().map_err(|e| io(&path, e))?, &path)?;
            if expected.is_some_and(|s| s != &before) {
                return Err(Error::Unstable(path));
            }
            metrics.source_check_nanos += nanos(t);
            let t = Instant::now();
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).map_err(|e| io(&path, e))?;
            metrics.read_nanos += nanos(t);
            metrics.source_reads += 1;
            metrics.source_bytes += bytes.len() as u64;
            let t = Instant::now();
            let after = Stamp::from_metadata(&file.metadata().map_err(|e| io(&path, e))?, &path)?;
            metrics.source_check_nanos += nanos(t);
            if before != after || bytes.len() as u64 != after.size {
                continue;
            }
            let t = Instant::now();
            let hash = digest(&bytes);
            metrics.hash_nanos += nanos(t);
            metrics.hashes += 1;
            let json = bytes;
            return Ok(Raw {
                json,
                hash,
                stamp: after,
            });
        }
        Err(Error::Unstable(path))
    }
    fn parse(&mut self, c: &Collection, id: &str, json: &[u8], validate: bool) -> Document {
        Self::parse_value(&mut self.metrics, c, id, json, validate)
    }
    fn parse_value(
        metrics: &mut Metrics,
        c: &Collection,
        _id: &str,
        json: &[u8],
        validate: bool,
    ) -> Document {
        let t = Instant::now();
        metrics.parses += 1;
        let mut doc = Document::parse(json);
        metrics.json_decode_nanos += nanos(t);
        if validate && doc.state.status != RecordStatus::IncorrectJson {
            let t = Instant::now();
            doc.validate(c);
            metrics.validations += 1;
            metrics.validation_nanos += nanos(t);
        }
        metrics.parse_nanos += nanos(t);
        doc
    }
    fn schedule_validation(&mut self, c: &Collection) -> Result<()> {
        if self.options.cache == CacheMode::Disabled
            || self.options.validation == ValidationMode::Manual
        {
            return Ok(());
        }
        self.enqueue_validation(c)
    }
    fn enqueue_validation(&mut self, c: &Collection) -> Result<()> {
        if self.validator.is_none() {
            let writer = match &self.writer {
                Some(writer) => writer.clone(),
                None => self.store()?,
            };
            self.validator = Some(Validator::new(
                writer,
                self.options.background_workers,
                self.options.sqlite_cache_mib,
                self.options.sql_diagnostics,
            )?);
        }
        self.validator.as_ref().unwrap().schedule(c.clone());
        Ok(())
    }
    /// Wait for already scheduled validation. Does not refresh source files.
    pub fn wait_for_validation(&self) -> Result<ValidationMetrics> {
        self.validator
            .as_ref()
            .map_or_else(|| Ok(ValidationMetrics::default()), Validator::wait)
    }
    pub fn validation_metrics(&self) -> ValidationMetrics {
        self.validator
            .as_ref()
            .map_or_else(ValidationMetrics::default, Validator::metrics)
    }
    /// Coalesced collection notifications published after validation commits.
    pub fn poll_validation_updates(&self) -> Result<Vec<String>> {
        self.validator
            .as_ref()
            .map_or_else(|| Ok(Vec::new()), Validator::updates)
    }
    /// Reconcile one collection and explicitly validate all pending cached records.
    pub fn validate_collection(&mut self, name: &str) -> Result<ValidationMetrics> {
        self.refresh(name)?;
        if self.options.cache == CacheMode::Sqlite {
            let c = self.collection(name)?;
            self.enqueue_validation(&c)?;
        }
        self.wait_for_validation()
    }
    /// Reconcile and validate every registered collection, including unopened ones.
    /// Uses the same validation policy and cumulative worker metrics as validating
    /// individual collections. With caching disabled, validation runs inline.
    pub fn validate_all(&mut self) -> Result<ValidationMetrics> {
        self.reload()?;
        let names = self.model.collections.keys().cloned().collect::<Vec<_>>();
        for name in names {
            self.validate_collection(&name)?;
        }
        self.wait_for_validation()
    }
    fn path(&self, c: &Collection, id: &str) -> String {
        c.path
            .join(format!("{id}.json"))
            .strip_prefix(&self.model.root)
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }
    fn extract(&self, c: &Collection, id: &str, data: &Value) -> Result<Vec<RecordRelation>> {
        fn visit(value: &Value, path: &[PathStep], at: &str, out: &mut Vec<(String, String)>) {
            match path.split_first() {
                None => {
                    if let Some(id) = value.as_str().filter(|s| !s.is_empty()) {
                        out.push((at.into(), id.into()))
                    }
                }
                Some((PathStep::Field(name), rest)) => {
                    if let Some(v) = value.get(name) {
                        visit(v, rest, &format!("{at}/{}", escape(name)), out)
                    }
                }
                Some((PathStep::Each, rest)) => {
                    if let Some(values) = value.as_array() {
                        for (i, v) in values.iter().enumerate() {
                            visit(v, rest, &format!("{at}/{i}"), out)
                        }
                    }
                }
            }
        }
        let mut out = Vec::new();
        for edge in self.model.graph.edges.iter().filter(|e| e.source == c.name) {
            let mut values = Vec::new();
            visit(data, &edge.path, "", &mut values);
            for (location, target_id) in values {
                if validate_id(&target_id).is_err() {
                    continue;
                }
                out.push(RecordRelation {
                    source_collection: c.name.clone(),
                    source_id: id.into(),
                    location,
                    target_collection: edge.target.clone(),
                    target_id,
                    target_state: TargetState::Unknown,
                });
            }
        }
        out.sort_by(|a, b| {
            (&a.location, &a.target_collection, &a.target_id).cmp(&(
                &b.location,
                &b.target_collection,
                &b.target_id,
            ))
        });
        Ok(out)
    }
    /// Reads the authoritative file, even when a persistent cache exists.
    pub fn get_record(&mut self, collection: &str, id: &str) -> Result<Option<Record>> {
        let before = self.metrics.clone();
        let result = self.get_record_inner(collection, id);
        if result.is_err() {
            self.metrics.rollback_writes(&before);
        }
        result
    }
    fn get_record_inner(&mut self, collection: &str, id: &str) -> Result<Option<Record>> {
        self.reload()?;
        let c = self.collection(collection)?;
        validate_id(id)?;
        self.verified.remove(collection);
        self.memory_coverage
            .insert(collection.into(), "partial".into());
        let result = self.raw(&c, id);
        let raw = match result {
            Ok(v) => v,
            Err(Error::Io { path, source })
                if source.kind() == std::io::ErrorKind::NotFound
                    && path == c.path.join(format!("{id}.json")) =>
            {
                self.memory_edges.remove(&(collection.into(), id.into()));
                if self.options.cache == CacheMode::Sqlite {
                    let writer = self.store()?;
                    let s = writer
                        .enter(self.options.sqlite_cache_mib, self.options.sql_diagnostics)?;
                    s.dirty(collection)?;
                    let tx = s.db.unchecked_transaction()?;
                    store::delete_record(&tx, &c, id, &mut self.metrics)?;
                    tx.commit()?;
                }
                return Ok(None);
            }
            Err(e) => {
                if self.options.cache == CacheMode::Sqlite
                    && let Ok(writer) = self.store()
                    && let Ok(s) =
                        writer.enter(self.options.sqlite_cache_mib, self.options.sql_diagnostics)
                {
                    let _ = s.dirty(collection);
                }
                return Err(e);
            }
        };
        let mut value = self.parse(&c, id, &raw.json, self.options.cache == CacheMode::Disabled);
        let edges = self.extract(&c, id, &value)?;
        if self.options.cache == CacheMode::Sqlite {
            let writer = self.store()?;
            let s = writer.enter(self.options.sqlite_cache_mib, self.options.sql_diagnostics)?;
            let old_state = s.state(collection)?;
            s.dirty(collection)?;
            let tx = s.db.unchecked_transaction()?;
            self.put_record(
                &tx,
                &c,
                id,
                &raw,
                &value,
                (old_state
                    .as_ref()
                    .is_some_and(|v| v.index == c.index_hash && v.schema == c.schema_hash)
                    && s.has_indexes(&c)?)
                .then_some(&s.index_cache),
            )?;
            if old_state.as_ref().is_none_or(|s| s.state == "partial") {
                tx.execute(
                    "UPDATE collections SET state='partial' WHERE name=?1",
                    [collection],
                )?;
            }
            tx.commit()?;
            value.state = cached_state(&s.db, collection, id)?;
        }
        self.schedule_validation(&c)?;
        self.memory_edges
            .insert((collection.into(), id.into()), edges);
        Ok(Some(Record {
            collection: collection.into(),
            id: id.into(),
            path: self.path(&c, id),
            hash: raw.hash,
            raw: (value.state.status == RecordStatus::IncorrectJson).then_some(raw.json),
            state: value.state,
            data: value.data,
        }))
    }
    fn ingest_empty(
        &mut self,
        db: &rusqlite::Connection,
        c: &Collection,
        source: &Source,
        files: &BTreeMap<String, Stamp>,
        indexed: Option<&crate::json_filter::indexes::IndexCache>,
    ) -> Result<()> {
        let t = Instant::now();
        let pool = rayon::ThreadPoolBuilder::new()
            // Reserve sufficient virtual stack for Sonic's bounded nesting in debug builds.
            .stack_size(16 * 1024 * 1024)
            .num_threads(self.options.ingest_workers.min(files.len()))
            .build()
            .map_err(|e| Error::Cache(format!("create ingestion workers: {e}")))?;
        self.metrics.parallel_prepare_nanos += nanos(t);
        let mut remaining = files.iter().peekable();
        while remaining.peek().is_some() {
            // Bound retained documents by both count and source-byte size. One
            // oversized record is processed alone; this is not a total RSS limit.
            let batch = next_ingest_batch(&mut remaining);
            let t = Instant::now();
            let prepared = pool.install(|| {
                batch
                    .par_iter()
                    .map(|(id, stamp)| {
                        let mut metrics = Metrics::default();
                        let result = (|| {
                            let raw = Self::read_raw(&mut metrics, source, id, Some(stamp))?;
                            let value = Self::parse_value(&mut metrics, c, id, &raw.json, false);
                            Ok((raw, value))
                        })();
                        (metrics, result)
                    })
                    .collect::<Vec<(Metrics, Result<(Raw, Document)>)>>()
            });
            self.metrics.parallel_prepare_nanos += nanos(t);
            // Account for all worker attempts, including those after the first
            // ordered error. Only this thread touches SQLite, in manifest order.
            for (metrics, _) in &prepared {
                self.metrics.merge(metrics);
            }
            for ((id, _), (_, result)) in batch.into_iter().zip(prepared) {
                let (raw, value) = result?;
                self.put_record(db, c, id, &raw, &value, indexed)?;
            }
        }
        Ok(())
    }
    fn put_record(
        &mut self,
        db: &rusqlite::Connection,
        c: &Collection,
        id: &str,
        raw: &Raw,
        value: &Document,
        indexed: Option<&crate::json_filter::indexes::IndexCache>,
    ) -> Result<()> {
        let t = Instant::now();
        let table = store::record_table(&c.name);
        let exists: Option<String> = db
            .prepare_cached(&format!(
                "SELECT digest FROM {table} WHERE collection=?1 AND id=?2"
            ))?
            .query_row(params![c.name, id], |r| r.get(0))
            .optional()?;
        if exists.as_ref() != Some(&raw.hash) {
            let maintained = if let Some(index_cache) = indexed {
                index_cache.prime(
                    &raw.json,
                    (value.state.status != RecordStatus::IncorrectJson).then_some(&value.data),
                )?
            } else {
                false
            };
            db.prepare_cached(&format!("INSERT INTO {table}(collection,id,path,size,modified,identity,digest,json,revision) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,1) ON CONFLICT(collection,id) DO UPDATE SET path=excluded.path,size=excluded.size,modified=excluded.modified,identity=excluded.identity,digest=excluded.digest,json=excluded.json,revision={table}.revision+1"))?.execute(params![c.name,id,self.path(c,id),raw.stamp.size as i64,raw.stamp.modified,raw.stamp.identity,raw.hash,raw.json])?;
            db.prepare_cached("INSERT INTO record_validation(collection,id,parse_status,validation_state,validation_schema_hash,issues) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(collection,id) DO UPDATE SET parse_status=excluded.parse_status,validation_state=excluded.validation_state,validation_schema_hash=excluded.validation_schema_hash,issues=excluded.issues")?.execute(params![c.name,id,if value.state.status == RecordStatus::IncorrectJson {"incorrect_json"} else {"parsed"},value.state.status.text(),c.schema_hash,serde_json::to_string(&value.state.issues).unwrap()])?;
            if maintained {
                self.metrics.index_rows_written += 1;
            }
            if exists.is_some() {
                self.metrics.rows_updated += 1
            } else {
                self.metrics.rows_inserted += 1
            }
        } else {
            db.prepare_cached(&format!(
                "UPDATE {table} SET size=?3,modified=?4,identity=?5 WHERE collection=?1 AND id=?2"
            ))?
            .execute(params![
                c.name,
                id,
                raw.stamp.size as i64,
                raw.stamp.modified,
                raw.stamp.identity
            ])?;
            // A schema change invalidates diagnostics even if source bytes did not change.
            db.execute("UPDATE record_validation SET validation_state=?3,validation_schema_hash=?4,issues=?5 WHERE collection=?1 AND id=?2 AND validation_schema_hash<>?4", params![c.name,id,value.state.status.text(),c.schema_hash,serde_json::to_string(&value.state.issues).unwrap()])?;
            self.metrics.rows_reused += 1;
        }
        self.metrics.record_write_nanos += nanos(t);
        let t = Instant::now();
        let edges = self.extract(c, id, value)?;
        self.metrics.relation_extract_nanos += nanos(t);
        store::put_edges(db, &c.name, id, &edges, &mut self.metrics)?;
        Ok(())
    }
    fn sync(
        &mut self,
        c: &Collection,
        s: &Store,
        requested_indexes: &[String],
        force: bool,
    ) -> Result<()> {
        let before = self.metrics.clone();
        let t = Instant::now();
        let result = self.sync_inner(c, s, requested_indexes, force);
        self.metrics.sync_nanos += nanos(t);
        if result.is_err() {
            self.verified.remove(&c.name);
            let _ = s.dirty(&c.name);
            self.metrics.rollback_writes(&before);
        }
        result
    }
    fn sync_inner(
        &mut self,
        c: &Collection,
        s: &Store,
        requested_indexes: &[String],
        force: bool,
    ) -> Result<()> {
        let table = store::record_table(&c.name);
        let previous = s.state(&c.name)?;
        let has_indexes = s.has_indexes(c)?;
        let requested_ready = s.has_requested_indexes(requested_indexes)?;
        let on_open = matches!(
            self.options.freshness,
            Freshness::MetadataOnOpen | Freshness::HashOnOpen
        );
        if !force
            && on_open
            && previous.as_ref().is_some_and(|p| {
                p.state == "current"
                    && p.schema == c.schema_hash
                    && p.mapping == c.mapping_hash
                    && (!has_indexes || p.index == c.index_hash)
                    && p.view == c.view_hashes.combined
                    && self.verified.get(&c.name) == Some(&p.revision)
            })
        {
            if !requested_ready {
                let t = Instant::now();
                let tx = s.db.unchecked_transaction()?;
                let _build = s.index_cache.build(c, requested_indexes, false)?;
                if requested_indexes.len() > 1 {
                    s.index_cache.freeze()?;
                }
                store::create_indexes(&tx, c, requested_indexes, &mut self.metrics)?;
                store::analyze_indexes(&tx, c, requested_indexes)?;
                tx.execute(
                    "UPDATE collections SET index_hash=?2 WHERE name=?1",
                    params![c.name, c.index_hash],
                )?;
                tx.commit()?;
                self.metrics.index_nanos += nanos(t);
            }
            return Ok(());
        }
        self.verified.remove(&c.name);
        let (source, files) = self.inventory(c)?;
        let t = Instant::now();
        let manifest_hash = fingerprint("source-manifest/v2", &files);
        self.metrics.fingerprint_nanos += nanos(t);
        let metadata_mode = matches!(
            self.options.freshness,
            Freshness::MetadataOnOpen | Freshness::MetadataOnRequest
        );
        let mapping_changed = previous
            .as_ref()
            .is_some_and(|p| !p.mapping.is_empty() && p.mapping != c.mapping_hash);
        let schema_changed =
            previous.as_ref().is_none_or(|p| p.schema != c.schema_hash) || mapping_changed;
        let use_indexes = !requested_indexes.is_empty() || has_indexes;
        let rebuild = use_indexes
            && (!has_indexes
                || mapping_changed
                || previous.as_ref().is_none_or(|p| p.index != c.index_hash));
        let build_indexes = !requested_indexes.is_empty() && (rebuild || !requested_ready);
        let complete = previous.as_ref().is_some_and(|p| p.state == "current");
        if metadata_mode
            && complete
            && !schema_changed
            && !rebuild
            && !build_indexes
            && previous
                .as_ref()
                .is_some_and(|p| p.manifest == manifest_hash && p.records == files.len())
        {
            // Unchanged metadata: do not load the per-record DB manifest or source bytes.
            let previous = previous.as_ref().unwrap();
            source.finish()?;
            if previous.view != c.view_hashes.combined {
                s.db.execute(
                    "UPDATE collections SET view_hash=?2 WHERE name=?1",
                    params![c.name, c.view_hashes.combined],
                )?;
            }
            self.metrics.manifest_hits += 1;
            self.metrics.metadata_reused += files.len() as u64;
            self.metrics.rows_reused += files.len() as u64;
            self.metrics.reconciliations += 1;
            self.verified.insert(c.name.clone(), previous.revision);
            return Ok(());
        }
        s.dirty(&c.name)?;
        let t = Instant::now();
        let manifest = s.manifest(&c.name)?;
        self.metrics.manifest_nanos += nanos(t);
        self.metrics.manifest_loads += 1;
        let before = self.metrics.clone();
        let tx = s.db.unchecked_transaction()?;
        if mapping_changed {
            for id in manifest.keys() {
                store::delete_record(&tx, c, id, &mut self.metrics)?;
            }
        }
        if rebuild {
            let t = Instant::now();
            store::drop_indexes(&tx, c)?;
            self.metrics.index_nanos += nanos(t);
        }
        let incremental = build_indexes && manifest.is_empty();
        if incremental {
            let t = Instant::now();
            store::create_indexes(&tx, c, requested_indexes, &mut self.metrics)?;
            self.metrics.index_nanos += nanos(t);
        }
        let _build = if build_indexes {
            Some(s.index_cache.build(c, requested_indexes, incremental)?)
        } else {
            None
        };
        let indexed = (use_indexes && (!rebuild || build_indexes)).then_some(&s.index_cache);
        if manifest.is_empty() && self.options.ingest_workers > 1 && files.len() > 1 {
            self.ingest_empty(&tx, c, &source, &files, indexed)?;
        } else {
            for (id, stamp) in &files {
                let old = if mapping_changed {
                    None
                } else {
                    manifest.get(id)
                };
                let candidate =
                    !metadata_mode || !complete || old.is_none_or(|v| &v.stamp != stamp);
                if candidate {
                    let raw = self.raw_from(&source, id, Some(stamp))?;
                    let changed = old.is_none_or(|v| v.digest != raw.hash);
                    if changed || schema_changed {
                        let value = self.parse(c, id, &raw.json, false);
                        self.put_record(&tx, c, id, &raw, &value, indexed)?;
                    } else {
                        self.metrics.rows_reused += 1;
                        if old.is_some_and(|v| v.stamp != raw.stamp) {
                            tx.prepare_cached(&format!("UPDATE {table} SET size=?3,modified=?4,identity=?5 WHERE collection=?1 AND id=?2"))?
                            .execute(params![c.name,id,raw.stamp.size as i64,raw.stamp.modified,raw.stamp.identity])?;
                        }
                    }
                } else {
                    self.metrics.metadata_reused += 1;
                    if schema_changed {
                        // Cached bytes correspond to the unchanged manifest entry. Schema
                        // changes need revalidation, not another source-file read.
                        let json: Vec<u8> = tx.query_row(
                            &format!("SELECT json FROM {table} WHERE collection=?1 AND id=?2"),
                            params![c.name, id],
                            |r| Ok(r.get_ref(0)?.as_bytes()?.to_vec()),
                        )?;
                        let value = self.parse(c, id, &json, false);
                        let raw = Raw {
                            json,
                            hash: old.unwrap().digest.clone(),
                            stamp: stamp.clone(),
                        };
                        self.put_record(&tx, c, id, &raw, &value, indexed)?;
                    } else {
                        self.metrics.rows_reused += 1;
                    }
                }
            }
        }
        if !mapping_changed {
            for id in manifest.keys().filter(|id| !files.contains_key(*id)) {
                store::delete_record(&tx, c, id, &mut self.metrics)?;
            }
        }
        source.finish()?;
        // Strict mode retains a final all-file stability check. Metadata mode enumerates
        // once, with pinned-directory and before/after descriptor checks for read candidates.
        if !metadata_mode && self.inventory(c)?.1 != files {
            return Err(Error::Unstable(c.path.clone()));
        }
        if build_indexes {
            let t = Instant::now();
            if !incremental {
                if requested_indexes.len() > 1 {
                    s.index_cache.freeze()?;
                }
                store::create_indexes(&tx, c, requested_indexes, &mut self.metrics)?;
            }
            store::analyze_indexes(&tx, c, requested_indexes)?;
            self.metrics.index_nanos += nanos(t);
        }
        let changed = schema_changed
            || rebuild
            || self.metrics.rows_inserted != before.rows_inserted
            || self.metrics.rows_updated != before.rows_updated
            || self.metrics.rows_deleted != before.rows_deleted;
        let revision = previous.as_ref().map_or(0, |p| p.revision) + i64::from(changed);
        let t = Instant::now();
        tx.execute("UPDATE collections SET state='current',schema_hash=?2,mapping_hash=?3,index_hash=?4,revision=?5,manifest_hash=?6,record_count=?7,view_hash=?8 WHERE name=?1",params![c.name,c.schema_hash,c.mapping_hash,if use_indexes{c.index_hash.as_str()}else{""},revision,manifest_hash,files.len() as i64,c.view_hashes.combined])?;
        tx.commit()?;
        self.metrics.commit_nanos += nanos(t);
        self.verified.insert(c.name.clone(), revision);
        self.metrics.reconciliations += 1;
        Ok(())
    }
    pub fn refresh(&mut self, collection: &str) -> Result<Metrics> {
        self.reload()?;
        let before = self.metrics.clone();
        let c = self.collection(collection)?;
        if self.options.cache == CacheMode::Sqlite {
            let writer = self.store()?;
            let s = writer.enter(self.options.sqlite_cache_mib, self.options.sql_diagnostics)?;
            self.sync(&c, &s, &[], true)?;
        } else {
            self.direct(&c, None)?;
        }
        self.schedule_validation(&c)?;
        Ok(self.metrics.since(&before))
    }
    pub fn open_view(&mut self, collection: &str, request: &ViewRequest) -> Result<ViewResult> {
        let before = self.metrics.clone();
        self.reload()?;
        let c = self.collection(collection)?;
        let plan = Plan::compile(&c, request)?;
        let table = store::record_table(&c.name);
        let mut result = if self.options.cache == CacheMode::Disabled {
            self.direct(&c, Some(&plan))?.unwrap()
        } else {
            let writer = self.store()?;
            let s = writer.enter(self.options.sqlite_cache_mib, self.options.sql_diagnostics)?;
            self.sync(&c, &s, &store::requested_view_indexes(&c, &plan), false)?;
            let t = Instant::now();
            let (where_sql, mut values) = plan.sql_where(&c);
            let total = if plan.count {
                let index = store::filter_view_index(&c, &plan.filters);
                Some(s.db.query_row(
                    &format!("SELECT count(*) FROM {table} v INDEXED BY {index}{where_sql}"),
                    params_from_iter(values.iter()),
                    |r| r.get::<_, i64>(0),
                )? as usize)
            } else {
                None
            };
            // Choose the page from compact typed keys before fetching JSON payloads.
            // This fixed lowering does not need an adaptive optimizer or result cache.
            let order = plan.sql_order(&c);
            let (keys, page_order) = plan.sql_page_keys(&c);
            // Use the requested order index, or the covering keys for compound
            // views. Payloads are read only for the selected page.
            let index = store::page_view_index(&c, &plan);
            let access = format!(" INDEXED BY {index}");
            let sql = format!(
                "WITH page AS MATERIALIZED (SELECT v.id{keys} FROM {table} v{access}{where_sql} ORDER BY {order} LIMIT ? OFFSET ?) SELECT r.id,r.path,r.json,h.validation_state,h.issues FROM page p CROSS JOIN {table} r ON r.id=p.id AND r.collection=? CROSS JOIN record_validation h ON h.collection=r.collection AND h.id=r.id ORDER BY {page_order}"
            );
            values.push((plan.limit as i64).into());
            values.push((plan.offset as i64).into());
            values.push(collection.to_owned().into());
            let mut explain = s.db.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
            let query_plan = explain
                .query_map(params_from_iter(values.iter()), |r| r.get::<_, String>(3))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let mut stmt = s.db.prepare(&sql)?;
            let rows = stmt
                .query_map(params_from_iter(values.iter()), |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get_ref(2)?.as_bytes()?.to_vec(),
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let mut out = Vec::with_capacity(rows.len());
            let fields = plan
                .columns
                .iter()
                .map(String::as_str)
                .chain(std::iter::once(c.display.as_str()))
                .filter(|field| *field != "@id")
                .collect();
            let mut states = BTreeMap::new();
            for (id, path, bytes, status, issues) in rows {
                let state = RecordState::read(&status, &issues)?;
                let value = if state.status == RecordStatus::IncorrectJson {
                    Value::Null
                } else {
                    let json =
                        std::str::from_utf8(&bytes).map_err(|e| Error::Cache(e.to_string()))?;
                    if json.trim_start().starts_with('{') {
                        crate::json::project(json, &fields)
                    } else {
                        crate::json::parse(json)
                    }
                    .map_err(|e| Error::Cache(format!("invalid cached JSON: {e}")))?
                };
                states.insert(id.clone(), state);
                out.push(plan.row(&c, &id, path, &value));
            }
            self.metrics.query_nanos += nanos(t);
            ViewResult {
                columns: plan.columns.clone(),
                rows: out,
                states,
                total,
                metrics: Metrics::default(),
                query_plan,
                view_hashes: plan.hashes.clone(),
            }
        };
        self.schedule_validation(&c)?;
        result.metrics = self.metrics.since(&before);
        Ok(result)
    }
    fn direct(&mut self, c: &Collection, plan: Option<&Plan>) -> Result<Option<ViewResult>> {
        self.memory_coverage.insert(c.name.clone(), "dirty".into());
        let (source, files) = self.inventory(c)?;
        let mut seen = BTreeMap::new();
        let mut rows = Vec::new();
        let mut states = BTreeMap::new();
        let mut edges = BTreeMap::new();
        for id in files.keys() {
            let raw = self.raw_from(&source, id, files.get(id))?;
            let data = self.parse(c, id, &raw.json, true);
            edges.insert((c.name.clone(), id.clone()), self.extract(c, id, &data)?);
            if let Some(plan) = plan {
                let t = Instant::now();
                if plan.matches(id, &data) {
                    states.insert(id.clone(), data.state.clone());
                    rows.push((
                        plan.keys(id, &data),
                        plan.row(c, id, self.path(c, id), &data),
                    ));
                }
                self.metrics.query_nanos += nanos(t);
            }
            seen.insert(id.clone(), raw.stamp);
        }
        source.finish()?;
        if self.inventory(c)?.1 != seen {
            return Err(Error::Unstable(c.path.clone()));
        }
        self.memory_edges.retain(|(name, _), _| name != &c.name);
        self.memory_edges.extend(edges);
        self.memory_coverage
            .insert(c.name.clone(), "current".into());
        if let Some(plan) = plan {
            let t = Instant::now();
            rows.sort_by(|a, b| plan.compare(a, b));
            let total = plan.count.then_some(rows.len());
            let rows: Vec<_> = rows
                .into_iter()
                .skip(plan.offset)
                .take(plan.limit)
                .map(|(_, r)| r)
                .collect();
            self.metrics.query_nanos += nanos(t);
            Ok(Some(ViewResult {
                columns: plan.columns.clone(),
                states: rows
                    .iter()
                    .map(|r| (r.id.clone(), states.remove(&r.id).unwrap()))
                    .collect(),
                rows,
                total,
                metrics: Metrics::default(),
                query_plan: vec!["direct source scan".into()],
                view_hashes: plan.hashes.clone(),
            }))
        } else {
            Ok(None)
        }
    }
    pub fn cache_status(&mut self) -> Result<CacheStatus> {
        self.reload()?;
        let directory = self.cache_directory()?;
        let store =
            if self.options.cache == CacheMode::Sqlite && directory.join("cache.sqlite").exists() {
                Some(self.store()?)
            } else {
                None
            };
        let guard = store
            .as_ref()
            .map(|w| w.enter(self.options.sqlite_cache_mib, self.options.sql_diagnostics))
            .transpose()?;
        let mut collections = Vec::new();
        for name in self.model.collections.keys() {
            collections.push(self.coverage(name, guard.as_deref())?);
        }
        Ok(CacheStatus {
            directory,
            mode: self.options.cache,
            freshness: self.options.freshness,
            collections,
            configuration_changes: store.as_ref().and(self.configuration_changes.clone()),
        })
    }
    fn coverage(&self, name: &str, s: Option<&Store>) -> Result<Coverage> {
        if let Some(s) = s
            && let Some(state) = s.state(name)?
        {
            let c = self.model.collection(name)?;
            let configuration_current = state.schema == c.schema_hash
                && state.mapping == c.mapping_hash
                && (state.index.is_empty() || state.index == c.index_hash)
                && state.view == c.view_hashes.combined;
            let records = s.db.query_row(
                &format!(
                    "SELECT count(*) FROM {} WHERE collection=?1",
                    store::record_table(name)
                ),
                [name],
                |r| r.get::<_, i64>(0),
            )? as usize;
            let validation = s.db.prepare("SELECT validation_state,count(*) FROM record_validation WHERE collection=?1 GROUP BY validation_state")?
                .query_map([name], |r| Ok((r.get(0)?,r.get::<_, i64>(1)? as usize)))?
                .collect::<std::result::Result<BTreeMap<_,_>,_>>()?;
            return Ok(Coverage {
                validation,
                collection: name.into(),
                verified: state.state == "current"
                    && configuration_current
                    && self.verified.get(name) == Some(&state.revision),
                configuration_current,
                applied_schema_hash: (!state.schema.is_empty()).then_some(state.schema),
                expected_schema_hash: c.schema_hash.clone(),
                state: state.state,
                records,
                revision: state.revision,
                manifest_hash: (!state.manifest.is_empty()).then_some(state.manifest),
                view_hash: (!state.view.is_empty()).then_some(state.view),
            });
        }
        let state = self
            .memory_coverage
            .get(name)
            .cloned()
            .unwrap_or_else(|| "absent".into());
        Ok(Coverage {
            collection: name.into(),
            verified: state == "current",
            configuration_current: state == "current",
            applied_schema_hash: None,
            expected_schema_hash: self.model.collection(name)?.schema_hash.clone(),
            state,
            records: self.memory_edges.keys().filter(|(n, _)| n == name).count(),
            validation: BTreeMap::new(),
            revision: 0,
            manifest_hash: None,
            view_hash: None,
        })
    }
    pub fn references_from(
        &mut self,
        collection: &str,
        id: &str,
        check_targets: bool,
    ) -> Result<References> {
        self.get_record(collection, id)?;
        let mut edges = self
            .memory_edges
            .get(&(collection.into(), id.into()))
            .cloned()
            .unwrap_or_default();
        if check_targets {
            self.resolve_targets(&mut edges)?;
        }
        let coverage = self
            .cache_status()?
            .collections
            .into_iter()
            .filter(|c| c.collection == collection)
            .collect();
        Ok(References { edges, coverage })
    }
    pub fn references_to(
        &mut self,
        collection: &str,
        id: &str,
        complete: bool,
        check_targets: bool,
    ) -> Result<References> {
        self.reload()?;
        self.model.collection(collection)?;
        validate_id(id)?;
        let sources = self
            .model
            .graph
            .edges
            .iter()
            .filter(|e| e.target == collection)
            .map(|e| e.source.clone())
            .collect::<BTreeSet<_>>();
        let mut edges = Vec::new();
        let mut coverage = Vec::new();
        if self.options.cache == CacheMode::Sqlite {
            let writer = self.store()?;
            let s = writer.enter(self.options.sqlite_cache_mib, self.options.sql_diagnostics)?;
            if complete {
                for source in &sources {
                    let c = self.collection(source)?;
                    self.sync(&c, &s, &[], false)?;
                }
            }
            for source in &sources {
                let mut stmt=s.db.prepare("SELECT source_collection,source_id,location,target_collection,target_id FROM edges WHERE target_collection=?1 AND target_id=?2 AND source_collection=?3 ORDER BY source_collection,source_id,location")?;
                edges.extend(
                    stmt.query_map(params![collection, id, source], |r| {
                        Ok(RecordRelation {
                            source_collection: r.get(0)?,
                            source_id: r.get(1)?,
                            location: r.get(2)?,
                            target_collection: r.get(3)?,
                            target_id: r.get(4)?,
                            target_state: TargetState::Unknown,
                        })
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?,
                );
                coverage.push(self.coverage(source, Some(&s))?);
            }
        } else {
            if complete {
                for source in &sources {
                    let c = self.collection(source)?;
                    self.direct(&c, None)?;
                }
            }
            for source in &sources {
                coverage.push(self.coverage(source, None)?);
            }
            edges.extend(
                self.memory_edges
                    .values()
                    .flatten()
                    .filter(|e| e.target_collection == collection && e.target_id == id)
                    .cloned(),
            );
        }
        if complete {
            for source in &sources {
                let c = self.collection(source)?;
                self.schedule_validation(&c)?;
            }
        }
        if check_targets {
            self.resolve_targets(&mut edges)?;
        }
        Ok(References { edges, coverage })
    }
    fn resolve_targets(&self, edges: &mut [RecordRelation]) -> Result<()> {
        let mut known = BTreeMap::new();
        for edge in edges {
            let key = (edge.target_collection.clone(), edge.target_id.clone());
            edge.target_state = if let Some(state) = known.get(&key) {
                *state
            } else {
                let c = self.model.collection(&edge.target_collection)?;
                self.check_source(c)?;
                let p = c.path.join(format!("{}.json", edge.target_id));
                let state = match fs::symlink_metadata(&p) {
                    Ok(m) if m.file_type().is_file() => TargetState::Present,
                    Ok(_) => return Err(record(&p, "", "reference target must be a regular file")),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => TargetState::Missing,
                    Err(e) => return Err(io(&p, e)),
                };
                known.insert(key, state);
                state
            };
        }
        Ok(())
    }
}
pub(crate) fn validate_id(id: &str) -> Result<()> {
    if id.is_empty() || id == "." || id == ".." || id.contains(['/', '\\', '\0']) {
        Err(Error::Request(format!("invalid record ID {id:?}")))
    } else {
        Ok(())
    }
}
const MAX_INGEST_BATCH_RECORDS: usize = 256;
const MAX_INGEST_BATCH_BYTES: u64 = 8 * 1024 * 1024;

fn next_ingest_batch<'a, I>(remaining: &mut std::iter::Peekable<I>) -> Vec<(&'a String, &'a Stamp)>
where
    I: Iterator<Item = (&'a String, &'a Stamp)>,
{
    let mut batch = Vec::new();
    let mut bytes = 0_u64;
    while let Some((_, stamp)) = remaining.peek() {
        if !batch.is_empty()
            && (batch.len() >= MAX_INGEST_BATCH_RECORDS
                || bytes.saturating_add(stamp.size) > MAX_INGEST_BATCH_BYTES)
        {
            break;
        }
        bytes = bytes.saturating_add(stamp.size);
        batch.push(remaining.next().unwrap());
    }
    batch
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

fn cached_state(db: &rusqlite::Connection, collection: &str, id: &str) -> Result<RecordState> {
    let (status, issues): (String, String) = db.query_row(
        "SELECT validation_state,issues FROM record_validation WHERE collection=?1 AND id=?2",
        params![collection, id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    RecordState::read(&status, &issues)
}

#[cfg(test)]
mod batch_tests {
    use super::*;

    fn partitions(sizes: impl IntoIterator<Item = u64>) -> Vec<(usize, u64)> {
        let files = sizes
            .into_iter()
            .enumerate()
            .map(|(i, size)| {
                (
                    format!("r{i:04}"),
                    Stamp {
                        size,
                        modified: String::new(),
                        identity: String::new(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut remaining = files.iter().peekable();
        let mut batches = Vec::new();
        while remaining.peek().is_some() {
            let batch = next_ingest_batch(&mut remaining);
            let bytes = batch.iter().map(|(_, stamp)| stamp.size).sum();
            assert!(batch.len() <= 256);
            assert!(bytes <= 8 * 1024 * 1024 || batch.len() == 1);
            batches.push((batch.len(), bytes));
        }
        batches
    }

    #[test]
    fn preparation_batches_respect_record_and_byte_bounds() {
        assert_eq!(
            partitions(std::iter::repeat_n(1, 600)),
            vec![(256, 256), (256, 256), (88, 88)]
        );
        assert_eq!(
            partitions([4 * 1024 * 1024, 4 * 1024 * 1024, 1]),
            vec![(2, 8 * 1024 * 1024), (1, 1)]
        );
        assert_eq!(
            partitions([9 * 1024 * 1024, 1]),
            vec![(1, 9 * 1024 * 1024), (1, 1)]
        );
    }
}
