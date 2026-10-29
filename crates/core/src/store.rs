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

mod indexes;
mod migrations;
mod records;
mod trace;
mod writer;

#[cfg(test)]
pub(crate) use indexes::covering_view_index;
pub(crate) use indexes::{
    analyze_indexes, create_indexes, drop_indexes, filter_view_index, page_view_index,
    requested_view_fields, requested_view_indexes, scalar_query_index,
};
pub(crate) use records::{delete_record, put_edges};
use trace::install_sql_trace;
pub(crate) use writer::Writer;
use writer::cache_identity;


// INFO: Open SQLite connection, index cache, lock, identity, and tracing state.
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


// INFO: Collection cache state and applied configuration hashes. Saved in Cache DB.
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


// INFO: Record digest and source stamp from the cache manifest. Saved in Cache DB.
#[derive(Debug, Clone)]
pub(crate) struct Cached {
    pub digest: String,
    pub stamp: Stamp,
}


pub(crate) fn record_table() -> &'static str {
    "records"
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
            // INFO: Negative cache_size values are kibibytes, independent of page size.
            // Apply on every connection because this setting is not persisted in the DB.
            db.pragma_update(None, "cache_size", -i64::from(mib) * 1024)?;
        }
        migrations::prepare(&db, root)?;
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
        Ok(self.db.query_row(
            "SELECT state,schema_hash,mapping_hash,index_hash,revision,manifest_hash,record_count,view_hash FROM collections WHERE name=?1",
            [name],
            |row| Ok(State {
                state: row.get(0)?,
                schema: row.get(1)?,
                mapping: row.get(2)?,
                index: row.get(3)?,
                revision: row.get(4)?,
                manifest: row.get(5)?,
                records: row.get::<_, i64>(6)? as usize,
                view: row.get(7)?,
            }),
        ).optional()?)
    }

    pub fn manifest(&self, name: &str) -> Result<BTreeMap<String, Cached>> {
        let mut stmt = self.db.prepare(&format!(
            "SELECT id,digest,size,modified,identity FROM {} WHERE collection=?1",
            record_table()
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
        self.db.execute(
            "INSERT INTO collections(name,state) VALUES(?1,'dirty')
             ON CONFLICT(name) DO UPDATE SET state='dirty'",
            [name],
        )?;
        Ok(())
    }
}


#[cfg(test)]
#[path = "store.tests.rs"]
mod tests;
