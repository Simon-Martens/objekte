use crate::{
    Collection, Error, PathStep, RepositoryModel, Result, ViewRequest, ViewResult,
    config::{digest, fingerprint},
    error::{io, record},
    query::Plan,
    schema::escape,
    source::{Source, Stamp},
    store::{self, Store},
};
use rusqlite::{OptionalExtension, params, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    time::Instant,
};

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
}
impl Options {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            cache: CacheMode::Sqlite,
            freshness: Freshness::MetadataOnOpen,
            namespace: "default".into(),
        }
    }
}
macro_rules! metrics {($($f:ident),*)=>{
    #[derive(Debug,Default,Clone,PartialEq,Eq,Serialize)]pub struct Metrics{$(pub $f:u64,)*}
    impl Metrics {pub fn since(&self,before:&Self)->Self{Self{$($f:self.$f.saturating_sub(before.$f),)*}}}
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
    sync_nanos,
    inventory_nanos,
    manifest_nanos,
    source_check_nanos,
    read_nanos,
    hash_nanos,
    parse_nanos,
    index_nanos,
    query_nanos
);
#[derive(Debug, Clone, Serialize)]
pub struct Record {
    pub collection: String,
    pub id: String,
    pub path: String,
    pub hash: String,
    pub data: Value,
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
    pub records: usize,
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
}
struct Raw {
    json: String,
    hash: String,
    stamp: Stamp,
}
pub struct Repository {
    model: RepositoryModel,
    options: Options,
    metrics: Metrics,
    verified: BTreeMap<String, i64>,
    memory_edges: BTreeMap<(String, String), Vec<RecordRelation>>,
    memory_coverage: BTreeMap<String, String>,
}
impl Repository {
    /// Load configuration and schemas only. Does not enumerate records or create cache files.
    pub fn load_repository(root: impl AsRef<Path>, options: Options) -> Result<Self> {
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
            self.model = model;
            self.verified.clear();
            self.memory_edges.clear();
            self.memory_coverage.clear();
            self.cache_directory()?;
        }
        self.metrics.reload_nanos += nanos(t);
        Ok(())
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
    fn store(&mut self) -> Result<Store> {
        let t = Instant::now();
        let s = Store::open(&self.cache_directory()?, &self.model.root)?;
        s.db.execute_batch(
            "CREATE TABLE IF NOT EXISTS inputs(path TEXT PRIMARY KEY,digest TEXT NOT NULL)",
        )?;
        let tx = s.db.unchecked_transaction()?;
        for (p, h) in &self.model.files {
            tx.execute(
                "INSERT INTO inputs(path,digest) VALUES(?1,?2) ON CONFLICT(path) DO UPDATE SET digest=excluded.digest WHERE inputs.digest<>excluded.digest",
                params![p.to_string_lossy(), h],
            )?;
        }
        tx.commit()?;
        self.metrics.store_open_nanos += nanos(t);
        Ok(s)
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
        let path = source.path.join(format!("{id}.json"));
        for _ in 0..3 {
            let t = Instant::now();
            let mut file = source.file(id)?;
            let before = Stamp::from_metadata(&file.metadata().map_err(|e| io(&path, e))?, &path)?;
            if expected.is_some_and(|s| s != &before) {
                return Err(Error::Unstable(path));
            }
            self.metrics.source_check_nanos += nanos(t);
            let t = Instant::now();
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).map_err(|e| io(&path, e))?;
            self.metrics.read_nanos += nanos(t);
            self.metrics.source_reads += 1;
            self.metrics.source_bytes += bytes.len() as u64;
            let t = Instant::now();
            let after = Stamp::from_metadata(&file.metadata().map_err(|e| io(&path, e))?, &path)?;
            self.metrics.source_check_nanos += nanos(t);
            if before != after || bytes.len() as u64 != after.size {
                continue;
            }
            let t = Instant::now();
            let hash = digest(&bytes);
            self.metrics.hash_nanos += nanos(t);
            self.metrics.hashes += 1;
            let json = String::from_utf8(bytes).map_err(|e| record(&path, "", e))?;
            return Ok(Raw {
                json,
                hash,
                stamp: after,
            });
        }
        Err(Error::Unstable(path))
    }
    fn parse(&mut self, c: &Collection, id: &str, json: &str, validate: bool) -> Result<Value> {
        let path = c.path.join(format!("{id}.json"));
        let t = Instant::now();
        self.metrics.parses += 1;
        let value: Value = serde_json::from_str(json).map_err(|e| record(&path, "", e))?;
        if validate {
            self.metrics.validations += 1;
            c.schema.validate(&value, &path)?;
        }
        self.metrics.parse_nanos += nanos(t);
        Ok(value)
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
                validate_id(&target_id)
                    .map_err(|e| record(&c.path.join(format!("{id}.json")), &location, e))?;
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
                    let s = self.store()?;
                    s.dirty(collection)?;
                    let tx = s.db.unchecked_transaction()?;
                    store::delete_record(&tx, &c, id, s.table_exists(&c)?, &mut self.metrics)?;
                    tx.commit()?;
                }
                return Ok(None);
            }
            Err(e) => {
                if self.options.cache == CacheMode::Sqlite
                    && let Ok(s) = self.store()
                {
                    let _ = s.dirty(collection);
                }
                return Err(e);
            }
        };
        let value = match self.parse(&c, id, &raw.json, true) {
            Ok(v) => v,
            Err(e) => {
                if self.options.cache == CacheMode::Sqlite
                    && let Ok(s) = self.store()
                {
                    let _ = s.dirty(collection);
                }
                return Err(e);
            }
        };
        let edges = self.extract(&c, id, &value)?;
        if self.options.cache == CacheMode::Sqlite {
            let s = self.store()?;
            let old_state = s.state(collection)?;
            s.dirty(collection)?;
            let tx = s.db.unchecked_transaction()?;
            self.put_record(
                &tx,
                &c,
                id,
                &raw,
                &value,
                old_state
                    .as_ref()
                    .is_some_and(|v| v.index == c.index_hash && v.schema == c.schema_hash)
                    && s.table_exists(&c)?,
            )?;
            if old_state.as_ref().is_none_or(|s| s.state == "partial") {
                tx.execute(
                    "UPDATE collections SET state='partial' WHERE name=?1",
                    [collection],
                )?;
            }
            tx.commit()?;
        }
        self.memory_edges
            .insert((collection.into(), id.into()), edges);
        Ok(Some(Record {
            collection: collection.into(),
            id: id.into(),
            path: self.path(&c, id),
            hash: raw.hash,
            data: value,
        }))
    }
    fn put_record(
        &mut self,
        db: &rusqlite::Connection,
        c: &Collection,
        id: &str,
        raw: &Raw,
        value: &Value,
        values: bool,
    ) -> Result<()> {
        let exists: Option<String> = db
            .query_row(
                "SELECT digest FROM records WHERE collection=?1 AND id=?2",
                params![c.name, id],
                |r| r.get(0),
            )
            .optional()?;
        if exists.as_ref() != Some(&raw.hash) {
            db.prepare_cached("INSERT INTO records(collection,id,path,size,modified,identity,digest,json,revision) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,1) ON CONFLICT(collection,id) DO UPDATE SET path=excluded.path,size=excluded.size,modified=excluded.modified,identity=excluded.identity,digest=excluded.digest,json=excluded.json,revision=records.revision+1")?.execute(params![c.name,id,self.path(c,id),raw.stamp.size as i64,raw.stamp.modified,raw.stamp.identity,raw.hash,raw.json])?;
            if exists.is_some() {
                self.metrics.rows_updated += 1
            } else {
                self.metrics.rows_inserted += 1
            }
        } else {
            db.prepare_cached(
                "UPDATE records SET size=?3,modified=?4,identity=?5 WHERE collection=?1 AND id=?2",
            )?
            .execute(params![
                c.name,
                id,
                raw.stamp.size as i64,
                raw.stamp.modified,
                raw.stamp.identity
            ])?;
            self.metrics.rows_reused += 1;
        }
        let edges = self.extract(c, id, value)?;
        store::put_edges(db, &c.name, id, &edges, &mut self.metrics)?;
        if values {
            store::put_values(db, c, id, value, &mut self.metrics)?;
        }
        Ok(())
    }
    fn sync(
        &mut self,
        c: &Collection,
        s: &Store,
        request_indexes: bool,
        force: bool,
    ) -> Result<()> {
        let before = self.metrics.clone();
        let t = Instant::now();
        let result = self.sync_inner(c, s, request_indexes, force);
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
        request_indexes: bool,
        force: bool,
    ) -> Result<()> {
        let previous = s.state(&c.name)?;
        let has_table = s.table_exists(c)?;
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
                    && (!request_indexes || (has_table && p.index == c.index_hash))
                    && p.view == c.view_hashes.combined
                    && self.verified.get(&c.name) == Some(&p.revision)
            })
        {
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
        let use_values = request_indexes || has_table;
        let rebuild = use_values
            && (!has_table
                || mapping_changed
                || previous.as_ref().is_none_or(|p| p.index != c.index_hash));
        let complete = previous.as_ref().is_some_and(|p| p.state == "current");
        if metadata_mode
            && complete
            && !schema_changed
            && !rebuild
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
                store::delete_record(&tx, c, id, has_table, &mut self.metrics)?;
            }
        }
        if rebuild {
            let t = Instant::now();
            store::create_values(&tx, c, &mut self.metrics)?;
            self.metrics.index_nanos += nanos(t);
        }
        for (id, stamp) in &files {
            let old = if mapping_changed {
                None
            } else {
                manifest.get(id)
            };
            let candidate = !metadata_mode || !complete || old.is_none_or(|v| &v.stamp != stamp);
            if candidate {
                let raw = self.raw_from(&source, id, Some(stamp))?;
                let changed = old.is_none_or(|v| v.digest != raw.hash);
                if changed || schema_changed {
                    let value = self.parse(c, id, &raw.json, true)?;
                    self.put_record(&tx, c, id, &raw, &value, use_values)?;
                } else {
                    self.metrics.rows_reused += 1;
                    if old.is_some_and(|v| v.stamp != raw.stamp) {
                        tx.prepare_cached("UPDATE records SET size=?3,modified=?4,identity=?5 WHERE collection=?1 AND id=?2")?
                            .execute(params![c.name,id,raw.stamp.size as i64,raw.stamp.modified,raw.stamp.identity])?;
                    }
                    if rebuild {
                        let value = self.parse(c, id, &raw.json, false)?;
                        store::put_values(&tx, c, id, &value, &mut self.metrics)?;
                    }
                }
            } else {
                self.metrics.metadata_reused += 1;
                if schema_changed || rebuild {
                    // Cached bytes correspond to the unchanged manifest entry. Schema/index
                    // changes need revalidation/derivation, not another source-file read.
                    let json: String = tx.query_row(
                        "SELECT json FROM records WHERE collection=?1 AND id=?2",
                        params![c.name, id],
                        |r| r.get(0),
                    )?;
                    let value = self.parse(c, id, &json, schema_changed)?;
                    if schema_changed {
                        let raw = Raw {
                            json,
                            hash: old.unwrap().digest.clone(),
                            stamp: stamp.clone(),
                        };
                        self.put_record(&tx, c, id, &raw, &value, use_values)?;
                    } else {
                        self.metrics.rows_reused += 1;
                        store::put_values(&tx, c, id, &value, &mut self.metrics)?;
                    }
                } else {
                    self.metrics.rows_reused += 1;
                }
            }
        }
        if !mapping_changed {
            for id in manifest.keys().filter(|id| !files.contains_key(*id)) {
                store::delete_record(&tx, c, id, use_values, &mut self.metrics)?;
            }
        }
        source.finish()?;
        // Strict mode retains a final all-file stability check. Metadata mode enumerates
        // once, with pinned-directory and before/after descriptor checks for read candidates.
        if !metadata_mode && self.inventory(c)?.1 != files {
            return Err(Error::Unstable(c.path.clone()));
        }
        if rebuild {
            let t = Instant::now();
            store::create_indexes(&tx, c, &mut self.metrics)?;
            self.metrics.index_nanos += nanos(t);
        }
        let changed = schema_changed
            || rebuild
            || self.metrics.rows_inserted != before.rows_inserted
            || self.metrics.rows_updated != before.rows_updated
            || self.metrics.rows_deleted != before.rows_deleted;
        let revision = previous.as_ref().map_or(0, |p| p.revision) + i64::from(changed);
        tx.execute("UPDATE collections SET state='current',schema_hash=?2,mapping_hash=?3,index_hash=?4,revision=?5,manifest_hash=?6,record_count=?7,view_hash=?8 WHERE name=?1",params![c.name,c.schema_hash,c.mapping_hash,if use_values{c.index_hash.as_str()}else{""},revision,manifest_hash,files.len() as i64,c.view_hashes.combined])?;
        tx.commit()?;
        self.verified.insert(c.name.clone(), revision);
        self.metrics.reconciliations += 1;
        Ok(())
    }
    pub fn refresh(&mut self, collection: &str) -> Result<Metrics> {
        self.reload()?;
        let before = self.metrics.clone();
        let c = self.collection(collection)?;
        if self.options.cache == CacheMode::Sqlite {
            let s = self.store()?;
            self.sync(&c, &s, false, true)?;
        } else {
            self.direct(&c, None)?;
        }
        Ok(self.metrics.since(&before))
    }
    pub fn open_view(&mut self, collection: &str, request: &ViewRequest) -> Result<ViewResult> {
        let before = self.metrics.clone();
        self.reload()?;
        let c = self.collection(collection)?;
        let plan = Plan::compile(&c, request)?;
        let mut result = if self.options.cache == CacheMode::Disabled {
            self.direct(&c, Some(&plan))?.unwrap()
        } else {
            let s = self.store()?;
            self.sync(&c, &s, true, false)?;
            let t = Instant::now();
            let (where_sql, mut values) = plan.sql_where(&c);
            let table = store::table(&c);
            let total = if plan.count {
                Some(s.db.query_row(
                    &format!("SELECT count(*) FROM {table} v{where_sql}"),
                    params_from_iter(values.iter()),
                    |r| r.get::<_, i64>(0),
                )? as usize)
            } else {
                None
            };
            // Choose the page from compact typed keys before fetching JSON payloads.
            // This fixed lowering does not need an adaptive optimizer or result cache.
            let order = plan.sql_order(&c);
            let sql = format!(
                "WITH page AS MATERIALIZED (SELECT v.* FROM {table} v{where_sql} ORDER BY {order} LIMIT ? OFFSET ?) SELECT r.id,r.path,r.json FROM page v JOIN records r ON r.id=v.id AND r.collection=? ORDER BY {order}"
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
                        r.get::<_, String>(2)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let mut out = Vec::with_capacity(rows.len());
            for (id, path, json) in rows {
                let value = serde_json::from_str(&json)
                    .map_err(|e| Error::Cache(format!("invalid cached JSON: {e}")))?;
                out.push(plan.row(&c, &id, path, &value));
            }
            self.metrics.query_nanos += nanos(t);
            ViewResult {
                columns: plan.columns.clone(),
                rows: out,
                total,
                metrics: Metrics::default(),
                query_plan,
                view_hashes: plan.hashes.clone(),
            }
        };
        result.metrics = self.metrics.since(&before);
        Ok(result)
    }
    fn direct(&mut self, c: &Collection, plan: Option<&Plan>) -> Result<Option<ViewResult>> {
        self.memory_coverage.insert(c.name.clone(), "dirty".into());
        let (source, files) = self.inventory(c)?;
        let mut seen = BTreeMap::new();
        let mut rows = Vec::new();
        let mut edges = BTreeMap::new();
        for id in files.keys() {
            let raw = self.raw_from(&source, id, files.get(id))?;
            let data = self.parse(c, id, &raw.json, true)?;
            edges.insert((c.name.clone(), id.clone()), self.extract(c, id, &data)?);
            if let Some(plan) = plan {
                let t = Instant::now();
                if plan.matches(id, &data) {
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
            let rows = rows
                .into_iter()
                .skip(plan.offset)
                .take(plan.limit)
                .map(|(_, r)| r)
                .collect();
            self.metrics.query_nanos += nanos(t);
            Ok(Some(ViewResult {
                columns: plan.columns.clone(),
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
        let mut collections = Vec::new();
        for name in self.model.collections.keys() {
            collections.push(self.coverage(name, store.as_ref())?);
        }
        Ok(CacheStatus {
            directory,
            mode: self.options.cache,
            freshness: self.options.freshness,
            collections,
        })
    }
    fn coverage(&self, name: &str, s: Option<&Store>) -> Result<Coverage> {
        if let Some(s) = s
            && let Some(state) = s.state(name)?
        {
            let records = s.db.query_row(
                "SELECT count(*) FROM records WHERE collection=?1",
                [name],
                |r| r.get::<_, i64>(0),
            )? as usize;
            return Ok(Coverage {
                collection: name.into(),
                verified: state.state == "current"
                    && self.verified.get(name) == Some(&state.revision),
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
            state,
            records: self.memory_edges.keys().filter(|(n, _)| n == name).count(),
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
            let s = self.store()?;
            if complete {
                for source in &sources {
                    let c = self.collection(source)?;
                    self.sync(&c, &s, false, false)?;
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
