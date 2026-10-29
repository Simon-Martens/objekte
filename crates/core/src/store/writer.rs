//! Process-wide writer registry and cross-process file lock.

use super::*;

// INFO: Foreground operations and validator results share a process-wide writer.
// The file lock covers each operation, including across processes, but is released
// while a repository handle is idle.


// INFO: Shared, mutex-protected SQLite store for serialized foreground writes.
pub(crate) struct Writer(std::sync::Mutex<Store>);


// INFO: Held writer operation that releases SQL tracing and the file lock on drop.
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
        // INFO: A cache file can be replaced outside this process while a handle
        // remains alive. Compare file identity before using the open connection.
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
                != migrations::FORMAT
        };
        if obsolete {
            return Err(Error::Cache(
                "incompatible cache format; use --fresh-cache or remove its cache directory".into(),
            ));
        }
        if replaced {
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


pub(super) fn cache_identity(metadata: &fs::Metadata) -> Option<(u64, u64)> {
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
