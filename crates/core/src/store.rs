use crate::{
    Collection, Error, Metrics, RecordRelation, Result, config::digest, error::io, query::scalar,
    source::Stamp,
};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    path::Path,
    time::{Duration, Instant},
};
const FORMAT: &str = "2";
pub(crate) struct Store {
    pub db: Connection,
    _lock: File,
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
pub(crate) fn table(c: &Collection) -> String {
    format!("v_{}", digest(c.name.as_bytes()))
}
impl Store {
    pub fn open(directory: &Path, root: &Path) -> Result<Self> {
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
        let db = Connection::open(directory.join("cache.sqlite"))?;
        db.busy_timeout(Duration::from_secs(5))?;
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
                .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name GLOB 'v_*'")?;
            let names = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            drop(stmt);
            let tx = db.unchecked_transaction()?;
            for name in names {
                if name.len() == 66 && name[2..].bytes().all(|b| b.is_ascii_hexdigit()) {
                    tx.execute_batch(&format!("DROP TABLE {name}"))?;
                }
            }
            tx.execute_batch("DROP TABLE IF EXISTS edges; DROP TABLE IF EXISTS records; DROP TABLE IF EXISTS collections;")?;
            tx.commit()?;
        }
        db.execute_batch("CREATE TABLE IF NOT EXISTS collections(name TEXT PRIMARY KEY,state TEXT NOT NULL,schema_hash TEXT NOT NULL DEFAULT '',mapping_hash TEXT NOT NULL DEFAULT '',index_hash TEXT NOT NULL DEFAULT '',revision INTEGER NOT NULL DEFAULT 0,manifest_hash TEXT NOT NULL DEFAULT '',record_count INTEGER NOT NULL DEFAULT 0,view_hash TEXT NOT NULL DEFAULT '');
          CREATE TABLE IF NOT EXISTS records(collection TEXT NOT NULL,id TEXT NOT NULL,path TEXT NOT NULL,size INTEGER NOT NULL,modified TEXT NOT NULL,identity TEXT NOT NULL,digest TEXT NOT NULL,json TEXT NOT NULL,revision INTEGER NOT NULL,PRIMARY KEY(collection,id));
          CREATE TABLE IF NOT EXISTS edges(source_collection TEXT NOT NULL,source_id TEXT NOT NULL,location TEXT NOT NULL,target_collection TEXT NOT NULL,target_id TEXT NOT NULL,PRIMARY KEY(source_collection,source_id,location,target_collection,target_id));
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
        Ok(Self { db, _lock: lock })
    }
    pub fn state(&self, name: &str) -> Result<Option<State>> {
        Ok(self.db.query_row("SELECT state,schema_hash,mapping_hash,index_hash,revision,manifest_hash,record_count,view_hash FROM collections WHERE name=?1",[name],|r|Ok(State{state:r.get(0)?,schema:r.get(1)?,mapping:r.get(2)?,index:r.get(3)?,revision:r.get(4)?,manifest:r.get(5)?,records:r.get::<_,i64>(6)? as usize,view:r.get(7)?})).optional()?)
    }
    pub fn manifest(&self, name: &str) -> Result<BTreeMap<String, Cached>> {
        let mut stmt = self
            .db
            .prepare("SELECT id,digest,size,modified,identity FROM records WHERE collection=?1")?;
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
        self.db.execute("INSERT INTO collections(name,state) VALUES(?1,'dirty') ON CONFLICT(name) DO UPDATE SET state='dirty'",[name])?;
        Ok(())
    }
    pub fn table_exists(&self, c: &Collection) -> Result<bool> {
        Ok(self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [table(c)],
            |r| r.get(0),
        )?)
    }
}
pub(crate) fn create_values(db: &Connection, c: &Collection, metrics: &mut Metrics) -> Result<()> {
    let table = table(c);
    db.execute_batch(&format!("DROP TABLE IF EXISTS {table}"))?;
    let columns = c
        .indexes
        .iter()
        .enumerate()
        .filter(|(_, f)| f.name != "@id")
        .map(|(i, _)| format!(",f{i}_s INTEGER NOT NULL,f{i}_v"))
        .collect::<String>();
    db.execute_batch(&format!(
        "CREATE TABLE {table}(id TEXT PRIMARY KEY{columns})"
    ))?;
    metrics.index_builds += 1;
    Ok(())
}
pub(crate) fn create_indexes(db: &Connection, c: &Collection, metrics: &mut Metrics) -> Result<()> {
    let table = table(c);
    for (i, f) in c.indexes.iter().enumerate() {
        if f.name == "@id" {
            continue;
        }
        for (suffix, direction) in [("asc", "ASC"), ("desc", "DESC")] {
            db.execute_batch(&format!(
                "CREATE INDEX {table}_{i}_{suffix} ON {table}(f{i}_s ASC,f{i}_v {direction},id ASC)"
            ))?;
            metrics.index_builds += 1;
        }
    }
    Ok(())
}
pub(crate) fn put_values(
    db: &Connection,
    c: &Collection,
    id: &str,
    value: &Value,
    metrics: &mut Metrics,
) -> Result<()> {
    let t = Instant::now();
    let mut values = vec![rusqlite::types::Value::Text(id.into())];
    for field in &c.indexes {
        if field.name != "@id" {
            let s = scalar(id, value, &field.name);
            values.push(s.state().into());
            values.push(s.sql());
        }
    }
    let params = vec!["?"; values.len()].join(",");
    db.prepare_cached(&format!(
        "INSERT OR REPLACE INTO {} VALUES({params})",
        table(c)
    ))?
    .execute(params_from_iter(values))?;
    metrics.index_rows_written += 1;
    metrics.index_row_write_nanos += t.elapsed().as_nanos() as u64;
    Ok(())
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
    has_values: bool,
    metrics: &mut Metrics,
) -> Result<()> {
    metrics.rows_deleted += db
        .prepare_cached("DELETE FROM records WHERE collection=?1 AND id=?2")?
        .execute(params![c.name, id])? as u64;
    metrics.edges_removed += db
        .prepare_cached("DELETE FROM edges WHERE source_collection=?1 AND source_id=?2")?
        .execute(params![c.name, id])? as u64;
    if has_values {
        db.prepare_cached(&format!("DELETE FROM {} WHERE id=?1", table(c)))?
            .execute([id])?;
    }
    Ok(())
}
