use crate::{
    Collection, Error, Metrics, RecordRelation, Result, config::digest, error::io, source::Stamp,
};
use rusqlite::{
    Connection, OptionalExtension, params,
    trace::{TraceEvent, TraceEventCodes},
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::{self, File, OpenOptions},
    io::Write,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
const FORMAT: &str = "6";
pub(crate) struct Store {
    pub db: Connection,
    pub index_cache: crate::json_filter::indexes::IndexCache,
    _lock: File,
    default_cache_size: i64,
    directory: std::path::PathBuf,
    root: std::path::PathBuf,
    identity: Option<(u64, u64)>,
    trace_active: Arc<AtomicBool>,
    trace_installed: bool,
}
#[derive(Debug, Clone)]
pub(crate) struct State {
    pub state: String,
    pub schema: String,
    pub mapping: String,
    pub index: String,
    pub revision: i64,
    pub manifest: String,
    pub records: usize,
    pub view: String,
}
#[derive(Debug, Clone)]
pub(crate) struct Cached {
    pub digest: String,
    pub stamp: Stamp,
}
pub(crate) fn record_table(collection: &str) -> String {
    format!("records_{}", digest(collection.as_bytes()))
}
fn index_prefix(c: &Collection) -> String {
    format!("j_{}", digest(c.name.as_bytes()))
}
pub(crate) fn scalar_query_index(c: &Collection, field: &str) -> String {
    let i = c.indexes.iter().position(|f| f.name == field).unwrap();
    format!("{}_{i}_query", index_prefix(c))
}
pub(crate) fn ordered_view_index(c: &Collection, sorts: &[crate::Sort]) -> Option<String> {
    let prefix = index_prefix(c);
    match sorts {
        [id] if id.field == "@id" => Some(format!("sqlite_autoindex_{}_1", record_table(&c.name))),
        [field, id] if id.field == "@id" && id.direction == crate::Direction::Asc => {
            let i = c.indexes.iter().position(|f| f.name == field.field)?;
            let direction = if field.direction == crate::Direction::Asc {
                "asc"
            } else {
                "desc"
            };
            Some(format!("{prefix}_{i}_{direction}"))
        }
        _ => None,
    }
}

impl Store {
    pub fn open(
        directory: &Path,
        root: &Path,
        cache_mib: Option<u32>,
        sql_diagnostics: bool,
    ) -> Result<Self> {
        fs::create_dir_all(directory).map_err(|e| io(directory, e))?;
        let lock_path = directory.join("lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|e| io(&lock_path, e))?;
        fs2::FileExt::lock_exclusive(&lock).map_err(|e| io(&lock_path, e))?;
        let mut db = Connection::open(directory.join("cache.sqlite"))?;
        let trace_active = Arc::new(AtomicBool::new(sql_diagnostics));
        if sql_diagnostics {
            install_sql_trace(&mut db, trace_active.clone())?;
        }
        crate::json_filter::register(&db)?;
        let index_cache = crate::json_filter::indexes::register(&db)?;
        db.busy_timeout(Duration::from_secs(5))?;
        let default_cache_size = db.pragma_query_value(None, "cache_size", |r| r.get(0))?;
        if let Some(mib) = cache_mib {
            // Negative cache_size values are kibibytes, independent of page size.
            // Apply on every connection: this setting is not persisted in the DB.
            db.pragma_update(None, "cache_size", -i64::from(mib) * 1024)?;
        }
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);")?;
        let identity = root.to_str().unwrap();
        let existing: Option<String> = db
            .query_row("SELECT value FROM meta WHERE key='repository'", [], |r| {
                r.get(0)
            })
            .optional()?;
        if existing.as_deref().is_some_and(|s| s != identity) {
            return Err(Error::Cache("repository identity mismatch".into()));
        }
        let version: Option<String> = db
            .query_row("SELECT value FROM meta WHERE key='format'", [], |r| {
                r.get(0)
            })
            .optional()?;
        if version.as_deref().is_some_and(|s| s != FORMAT) {
            let mut stmt = db
                .prepare("SELECT name FROM sqlite_master WHERE type='table' AND (name GLOB 'v_*' OR name GLOB 'records_*')")?;
            let names = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            drop(stmt);
            let tx = db.unchecked_transaction()?;
            for name in names {
                if name
                    .strip_prefix("v_")
                    .or_else(|| name.strip_prefix("records_"))
                    .is_some_and(|suffix| {
                        suffix.len() == 64 && suffix.bytes().all(|b| b.is_ascii_hexdigit())
                    })
                {
                    tx.execute_batch(&format!("DROP TABLE {name}"))?;
                }
            }
            tx.execute_batch("DROP TABLE IF EXISTS edges; DROP TABLE IF EXISTS record_validation; DROP TABLE IF EXISTS records; DROP TABLE IF EXISTS collections;")?;
            tx.commit()?;
        }
        db.execute_batch("CREATE TABLE IF NOT EXISTS inputs(path TEXT PRIMARY KEY,digest TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS input_collections(path TEXT NOT NULL,collection TEXT NOT NULL,PRIMARY KEY(path,collection)) WITHOUT ROWID;
          CREATE TABLE IF NOT EXISTS collections(name TEXT PRIMARY KEY,state TEXT NOT NULL,schema_hash TEXT NOT NULL DEFAULT '',mapping_hash TEXT NOT NULL DEFAULT '',index_hash TEXT NOT NULL DEFAULT '',revision INTEGER NOT NULL DEFAULT 0,manifest_hash TEXT NOT NULL DEFAULT '',record_count INTEGER NOT NULL DEFAULT 0,view_hash TEXT NOT NULL DEFAULT '');
          CREATE TABLE IF NOT EXISTS record_validation(collection TEXT NOT NULL,id TEXT NOT NULL,parse_status TEXT NOT NULL,validation_state TEXT NOT NULL,validation_schema_hash TEXT NOT NULL,issues TEXT NOT NULL,PRIMARY KEY(collection,id)) WITHOUT ROWID;
          CREATE TABLE IF NOT EXISTS edges(source_collection TEXT NOT NULL,source_id TEXT NOT NULL,location TEXT NOT NULL,target_collection TEXT NOT NULL,target_id TEXT NOT NULL,PRIMARY KEY(source_collection,source_id,location,target_collection,target_id));
          CREATE INDEX IF NOT EXISTS validation_pending ON record_validation(collection,id) WHERE validation_state='pending';
          CREATE INDEX IF NOT EXISTS edge_target ON edges(target_collection,target_id,source_collection,source_id);")?;
        let tx = db.unchecked_transaction()?;
        for (key, value) in [
            ("repository", identity),
            ("format", FORMAT),
            ("hash", "sha256"),
        ] {
            tx.execute("INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value WHERE meta.value<>excluded.value",params![key,value])?;
        }
        tx.commit()?;
        let metadata =
            fs::metadata(directory.join("cache.sqlite")).map_err(|e| io(directory, e))?;
        Ok(Self {
            directory: directory.into(),
            root: root.into(),
            identity: cache_identity(&metadata),
            db,
            index_cache,
            _lock: lock,
            default_cache_size,
            trace_active,
            trace_installed: sql_diagnostics,
        })
    }
    fn set_sql_trace(&mut self, enabled: bool) -> Result<()> {
        if enabled && !self.trace_installed {
            install_sql_trace(&mut self.db, self.trace_active.clone())?;
            self.trace_installed = true;
        }
        self.trace_active.store(enabled, Ordering::Relaxed);
        Ok(())
    }
    pub fn state(&self, name: &str) -> Result<Option<State>> {
        Ok(self.db.query_row("SELECT state,schema_hash,mapping_hash,index_hash,revision,manifest_hash,record_count,view_hash FROM collections WHERE name=?1",[name],|r|Ok(State{state:r.get(0)?,schema:r.get(1)?,mapping:r.get(2)?,index:r.get(3)?,revision:r.get(4)?,manifest:r.get(5)?,records:r.get::<_,i64>(6)? as usize,view:r.get(7)?})).optional()?)
    }
    pub fn manifest(&self, name: &str) -> Result<BTreeMap<String, Cached>> {
        let mut stmt = self.db.prepare(&format!(
            "SELECT id,digest,size,modified,identity FROM {} WHERE collection=?1",
            record_table(name)
        ))?;
        Ok(stmt
            .query_map([name], |r| {
                Ok((
                    r.get(0)?,
                    Cached {
                        digest: r.get(1)?,
                        stamp: Stamp {
                            size: r.get::<_, i64>(2)? as u64,
                            modified: r.get(3)?,
                            identity: r.get(4)?,
                        },
                    },
                ))
            })?
            .collect::<std::result::Result<_, _>>()?)
    }
    pub fn dirty(&self, name: &str) -> Result<()> {
        self.db.execute_batch(&format!("CREATE TABLE IF NOT EXISTS {}(collection TEXT NOT NULL,id TEXT NOT NULL,path TEXT NOT NULL,size INTEGER NOT NULL,modified TEXT NOT NULL,identity TEXT NOT NULL,digest TEXT NOT NULL,json BLOB NOT NULL,revision INTEGER NOT NULL,PRIMARY KEY(collection,id))",record_table(name)))?;
        self.db.execute("INSERT INTO collections(name,state) VALUES(?1,'dirty') ON CONFLICT(name) DO UPDATE SET state='dirty'",[name])?;
        Ok(())
    }
    pub fn has_indexes(&self, c: &Collection) -> Result<bool> {
        Ok(self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name GLOB ?1)",
            [format!("{}_*", index_prefix(c))],
            |r| r.get(0),
        )?)
    }
    pub fn has_requested_indexes(&self, names: &[String]) -> Result<bool> {
        for name in names {
            if !index_exists(&self.db, name)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

fn install_sql_trace(db: &mut Connection, active: Arc<AtomicBool>) -> Result<()> {
    let mut started: HashMap<String, Instant> = HashMap::new();
    db.trace_v2(
        TraceEventCodes::SQLITE_TRACE_STMT | TraceEventCodes::SQLITE_TRACE_PROFILE,
        Some(move |event: TraceEvent<'_>| {
            if !active.load(Ordering::Relaxed) {
                return;
            }
            match event {
                TraceEvent::Stmt(statement, _) => {
                    started
                        .entry(statement.sql().into_owned())
                        .or_insert_with(Instant::now);
                }
                TraceEvent::Profile(statement, sqlite_duration) => {
                    let sql = statement.sql().into_owned();
                    let elapsed = started
                        .remove(&sql)
                        .map_or(sqlite_duration, |start| start.elapsed());
                    let mut stderr = std::io::stderr().lock();
                    let _ = writeln!(
                        stderr,
                        "{}",
                        serde_json::json!({
                            "type": "sql",
                            "sql": sql,
                            "duration_ms": elapsed.as_secs_f64() * 1000.0,
                        })
                    );
                }
                _ => {}
            }
        }),
    )?;
    Ok(())
}
pub(crate) fn drop_indexes(db: &Connection, c: &Collection) -> Result<()> {
    let prefix = index_prefix(c);
    let mut stmt =
        db.prepare("SELECT name FROM sqlite_master WHERE type='index' AND name GLOB ?1")?;
    let names = stmt
        .query_map([format!("{prefix}_*")], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(stmt);
    for name in names {
        if name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            db.execute_batch(&format!("DROP INDEX {name}"))?;
        }
    }
    Ok(())
}
pub(crate) fn create_indexes(
    db: &Connection,
    c: &Collection,
    requested: &[String],
    metrics: &mut Metrics,
) -> Result<()> {
    let table = record_table(&c.name);
    use crate::json_filter::indexes::{quote, view_expr};
    let prefix = index_prefix(c);
    let scope = format!(" WHERE collection={}", quote(&c.name));
    // One compact covering index supplies all configured view keys for compound
    // orders/filters without reparsing full JSON or adding stored table columns.
    let fields = c
        .indexes
        .iter()
        .filter(|f| f.name != "@id")
        .map(|f| {
            format!(
                ",{},{}",
                view_expr(c, &f.name, "", true),
                view_expr(c, &f.name, "", false)
            )
        })
        .collect::<String>();
    if requested.contains(&format!("{prefix}_identity"))
        && !index_exists(db, &format!("{prefix}_identity"))?
    {
        db.execute_batch(&format!(
            "CREATE INDEX {prefix}_identity ON {table}(id{fields}){scope}"
        ))?;
        metrics.index_builds += 1;
    }
    for (i, f) in c.indexes.iter().enumerate() {
        if f.name == "@id" {
            continue;
        }
        let state = view_expr(c, &f.name, "", true);
        let value = view_expr(c, &f.name, "", false);
        for (suffix, direction) in [("asc", "ASC"), ("desc", "DESC")] {
            if !requested.contains(&format!("{prefix}_{i}_{suffix}"))
                || index_exists(db, &format!("{prefix}_{i}_{suffix}"))?
            {
                continue;
            }
            db.execute_batch(&format!("CREATE INDEX {prefix}_{i}_{suffix} ON {table}({state} ASC,{value} {direction},id ASC){scope}"))?;
            metrics.index_builds += 1;
        }
    }
    // Count source records indexed, not separate materialized field-table rows.
    metrics.index_rows_written += db.query_row(
        &format!("SELECT count(*) FROM {table} WHERE collection=?1"),
        [&c.name],
        |r| r.get::<_, i64>(0),
    )? as u64;
    Ok(())
}
pub(crate) fn analyze_indexes(db: &Connection, c: &Collection, requested: &[String]) -> Result<()> {
    for name in requested {
        db.execute_batch(&format!("ANALYZE {name}"))?;
    }
    db.execute_batch(&format!(
        "ANALYZE sqlite_autoindex_{}_1",
        record_table(&c.name)
    ))?;
    Ok(())
}
fn index_exists(db: &Connection, name: &str) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name=?1)",
        [name],
        |r| r.get(0),
    )?)
}
pub(crate) fn page_view_index(c: &Collection, plan: &crate::query::Plan) -> String {
    if plan.filters.is_empty() {
        ordered_view_index(c, &plan.sorts)
    } else {
        None
    }
    .unwrap_or_else(|| covering_view_index(c))
}
pub(crate) fn requested_view_indexes(c: &Collection, plan: &crate::query::Plan) -> Vec<String> {
    let mut names = vec![page_view_index(c, plan)];
    if plan.count {
        let count = filter_view_index(c, &plan.filters);
        if !names.contains(&count) {
            names.push(count);
        }
    }
    names.retain(|name| name.starts_with("j_"));
    names
}
pub(crate) fn requested_view_fields(c: &Collection, names: &[String]) -> BTreeSet<String> {
    let prefix = index_prefix(c);
    c.indexes
        .iter()
        .enumerate()
        .filter(|(i, f)| {
            f.name != "@id"
                && (names.contains(&covering_view_index(c))
                    || ["asc", "desc"]
                        .iter()
                        .any(|suffix| names.contains(&format!("{prefix}_{i}_{suffix}"))))
        })
        .map(|(_, f)| f.name.clone())
        .collect()
}
pub(crate) fn covering_view_index(c: &Collection) -> String {
    format!("{}_identity", index_prefix(c))
}
pub(crate) fn filter_view_index(c: &Collection, filters: &[crate::Filter]) -> String {
    if filters.is_empty() || filters.iter().all(|f| f.field == "@id") {
        return format!("sqlite_autoindex_{}_1", record_table(&c.name));
    }
    if let Some(first) = filters.first()
        && first.field != "@id"
        && filters.iter().all(|f| f.field == first.field)
    {
        let i = c
            .indexes
            .iter()
            .position(|f| f.name == first.field)
            .unwrap();
        return format!("{}_{i}_asc", index_prefix(c));
    }
    covering_view_index(c)
}
pub(crate) fn put_edges(
    db: &Connection,
    c: &str,
    id: &str,
    edges: &[RecordRelation],
    metrics: &mut Metrics,
) -> Result<()> {
    let t = Instant::now();
    let mut stmt=db.prepare_cached("SELECT location,target_collection,target_id FROM edges WHERE source_collection=?1 AND source_id=?2")?;
    let old = stmt
        .query_map(params![c, id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<std::result::Result<BTreeSet<_>, _>>()?;
    let new = edges
        .iter()
        .map(|e| {
            (
                e.location.clone(),
                e.target_collection.clone(),
                e.target_id.clone(),
            )
        })
        .collect::<BTreeSet<_>>();
    for (location, target, target_id) in old.difference(&new) {
        db.prepare_cached("DELETE FROM edges WHERE source_collection=?1 AND source_id=?2 AND location=?3 AND target_collection=?4 AND target_id=?5")?.execute(params![c,id,location,target,target_id])?;
        metrics.edges_removed += 1;
    }
    for (location, target, target_id) in new.difference(&old) {
        db.prepare_cached("INSERT INTO edges VALUES(?1,?2,?3,?4,?5)")?
            .execute(params![c, id, location, target, target_id])?;
        metrics.edges_added += 1;
    }
    metrics.relation_write_nanos += t.elapsed().as_nanos() as u64;
    Ok(())
}
pub(crate) fn delete_record(
    db: &Connection,
    c: &Collection,
    id: &str,
    metrics: &mut Metrics,
) -> Result<()> {
    db.prepare_cached("DELETE FROM record_validation WHERE collection=?1 AND id=?2")?
        .execute(params![c.name, id])?;
    metrics.rows_deleted += db
        .prepare_cached(&format!(
            "DELETE FROM {} WHERE collection=?1 AND id=?2",
            record_table(&c.name)
        ))?
        .execute(params![c.name, id])? as u64;
    metrics.edges_removed += db
        .prepare_cached("DELETE FROM edges WHERE source_collection=?1 AND source_id=?2")?
        .execute(params![c.name, id])? as u64;
    Ok(())
}

// All foreground operations and validator results use this writer. The process-wide
// registry also shares it between Repository handles. The file lock covers each
// operation, not the idle lifetime of a repository, and coordinates other processes.
pub(crate) struct Writer(std::sync::Mutex<Store>);
pub(crate) struct WriterGuard<'a>(std::sync::MutexGuard<'a, Store>);
impl std::ops::Deref for WriterGuard<'_> {
    type Target = Store;
    fn deref(&self) -> &Store {
        &self.0
    }
}
impl Drop for WriterGuard<'_> {
    fn drop(&mut self) {
        self.0.trace_active.store(false, Ordering::Relaxed);
        let _ = fs2::FileExt::unlock(&self.0._lock);
    }
}
impl Writer {
    pub fn open(
        directory: &Path,
        root: &Path,
        budget: Option<u32>,
        sql_diagnostics: bool,
    ) -> Result<std::sync::Arc<Self>> {
        use std::sync::{Arc, Mutex, OnceLock, Weak};
        static WRITERS: OnceLock<Mutex<BTreeMap<std::path::PathBuf, Weak<Writer>>>> =
            OnceLock::new();
        let mut writers = WRITERS
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| Error::Cache("writer registry poisoned".into()))?;
        writers.retain(|_, w| w.strong_count() > 0);
        if let Some(writer) = writers.get(directory).and_then(Weak::upgrade) {
            return Ok(writer);
        }
        let store = Store::open(directory, root, budget, sql_diagnostics)?;
        fs2::FileExt::unlock(&store._lock).map_err(|e| io(directory, e))?;
        let writer = Arc::new(Self(Mutex::new(store)));
        writers.insert(directory.to_owned(), Arc::downgrade(&writer));
        Ok(writer)
    }
    pub fn enter(&self, budget: Option<u32>, sql_diagnostics: bool) -> Result<WriterGuard<'_>> {
        self.enter_inner(budget, sql_diagnostics, true)
    }
    pub fn enter_existing(
        &self,
        budget: Option<u32>,
        sql_diagnostics: bool,
    ) -> Result<WriterGuard<'_>> {
        self.enter_inner(budget, sql_diagnostics, false)
    }
    fn enter_inner(
        &self,
        budget: Option<u32>,
        sql_diagnostics: bool,
        rebuild: bool,
    ) -> Result<WriterGuard<'_>> {
        let store = self
            .0
            .lock()
            .map_err(|_| Error::Cache("cache writer poisoned".into()))?;
        fs2::FileExt::lock_exclusive(&store._lock).map_err(|e| io("cache lock", e))?;
        let mut guard = WriterGuard(store);
        guard.0.set_sql_trace(sql_diagnostics)?;
        let metadata = fs::metadata(guard.directory.join("cache.sqlite"));
        let replaced = metadata.as_ref().is_err()
            || metadata
                .as_ref()
                .is_ok_and(|m| cache_identity(m) != guard.identity);
        if metadata.as_ref().is_ok_and(|m| m.len() < 100) {
            return Err(Error::Cache("truncated SQLite cache".into()));
        }
        if !replaced {
            let identity: String =
                guard
                    .db
                    .query_row("SELECT value FROM meta WHERE key='repository'", [], |r| {
                        r.get(0)
                    })?;
            if identity != guard.root.to_string_lossy() {
                return Err(Error::Cache("repository identity mismatch".into()));
            }
        }
        let obsolete = if replaced {
            false
        } else {
            guard
                .db
                .query_row("SELECT value FROM meta WHERE key='format'", [], |r| {
                    r.get::<_, String>(0)
                })?
                != FORMAT
        };
        if replaced || obsolete {
            if !rebuild {
                return Err(Error::Cache(
                    "cache replaced during background validation; reopen repository".into(),
                ));
            }
            fs2::FileExt::unlock(&guard._lock).map_err(|e| io(&guard.directory, e))?;
            let replacement = Store::open(&guard.directory, &guard.root, budget, sql_diagnostics)?;
            *guard.0 = replacement;
        }
        guard.db.pragma_update(
            None,
            "cache_size",
            budget.map_or(guard.default_cache_size, |m| -i64::from(m) * 1024),
        )?;
        Ok(guard)
    }
}

fn cache_identity(metadata: &fs::Metadata) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some((metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        metadata
            .created()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|t| (t.as_secs(), u64::from(t.subsec_nanos())))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn background_writer_does_not_recreate_a_deleted_cache() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let writer = super::Writer::open(&cache, temp.path(), Some(64), false).unwrap();
        std::fs::remove_dir_all(&cache).unwrap();
        assert!(writer.enter_existing(Some(64), false).is_err());
        assert!(!cache.exists());
        assert!(writer.enter(Some(64), false).is_ok());
        assert!(cache.exists());
    }
    use super::*;

    #[test]
    fn page_cache_budget_is_connection_local_and_keeps_wal_and_durability() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let default: i64 = Connection::open_in_memory()
            .unwrap()
            .query_row("PRAGMA cache_size", [], |r| r.get(0))
            .unwrap();
        for budget in [None, Some(1), Some(16), Some(64), None] {
            let store = Store::open(&cache, temp.path(), budget, false).unwrap();
            assert_eq!(
                store
                    .db
                    .query_row("PRAGMA cache_size", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                budget.map_or(default, |mib| -i64::from(mib) * 1024)
            );
            assert_eq!(
                store
                    .db
                    .query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))
                    .unwrap(),
                "wal"
            );
            assert_eq!(
                store
                    .db
                    .query_row("PRAGMA synchronous", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                2
            );
        }
    }
}
