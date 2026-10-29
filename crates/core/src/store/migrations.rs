//! Transactional schema setup for the disposable SQLite cache.

use crate::{Error, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

pub(super) const FORMAT: &str = "7";


// INFO: Versioned SQL schema step and its history name.
struct Migration {
    version: i64,
    name: &'static str,
    sql: &'static str,
}


const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "repository_metadata",
        sql: include_str!("migrations/001_repository_metadata.sql"),
    },
    Migration {
        version: 2,
        name: "records_and_indexes",
        sql: include_str!("migrations/002_records_and_indexes.sql"),
    },
];

pub(super) fn prepare(db: &Connection, root: &Path) -> Result<()> {
    // INFO: journal_mode cannot be changed inside a transaction. Do it before migrations.
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")?;

    let tables = db
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let had_meta = tables.iter().any(|name| name == "meta");
    let had_migrations = tables.iter().any(|name| name == "migrations");
    let applied_count: i64 = if had_migrations {
        db.query_row("SELECT count(*) FROM migrations", [], |row| row.get(0))?
    } else {
        0
    };

    // INFO: An untracked pre-release schema cannot be migrated safely. Require
    // an explicit fresh namespace instead of inferring its table layout.
    if (had_meta && (!had_migrations || applied_count == 0))
        || (!had_meta && had_migrations && (applied_count != 0 || tables.len() != 1))
        || (!had_meta && !had_migrations && !tables.is_empty())
    {
        return Err(Error::Cache(
            "unrecognized cache schema; use --fresh-cache or remove its cache directory".into(),
        ));
    }

    let identity = root.to_str().unwrap();
    if had_meta {
        let existing: Option<String> = db
            .query_row("SELECT value FROM meta WHERE key='repository'", [], |row| {
                row.get(0)
            })
            .optional()?;
        if existing.as_deref().is_some_and(|stored| stored != identity) {
            return Err(Error::Cache("repository identity mismatch".into()));
        }
        let format: Option<String> = db
            .query_row("SELECT value FROM meta WHERE key='format'", [], |row| {
                row.get(0)
            })
            .optional()?;
        if format.as_deref() == Some("6") {
            let tx = db.unchecked_transaction()?;
            for name in &tables {
                let quoted = name.replace('"', "\"\"");
                tx.execute_batch(&format!("DROP TABLE \"{quoted}\""))?;
            }
            tx.commit()?;
            return prepare(db, root);
        }
        if format.as_deref().is_some_and(|stored| stored != FORMAT) {
            return Err(Error::Cache("incompatible cache format".into()));
        }
    }

    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS migrations(
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );",
    )?;
    let latest = MIGRATIONS.last().unwrap().version;
    let newest: Option<i64> =
        db.query_row("SELECT max(version) FROM migrations", [], |row| row.get(0))?;
    if newest.is_some_and(|version| version > latest) {
        return Err(Error::Cache(
            "cache migrations are newer than this application".into(),
        ));
    }
    for migration in MIGRATIONS {
        let applied: Option<String> = db
            .query_row(
                "SELECT name FROM migrations WHERE version=?1",
                [migration.version],
                |row| row.get(0),
            )
            .optional()?;
        match applied {
            Some(name) if name == migration.name => continue,
            Some(_) => return Err(Error::Cache("cache migration history mismatch".into())),
            None => {}
        }
        // INFO: The schema change and its history row commit together. A failed
        // migration leaves no marker, so the next open can retry it.
        let tx = db.unchecked_transaction()?;
        tx.execute_batch(migration.sql)?;
        tx.execute(
            "INSERT INTO migrations(version,name) VALUES(?1,?2)",
            params![migration.version, migration.name],
        )?;
        tx.commit()?;
    }

    let tx = db.unchecked_transaction()?;
    for (key, value) in [
        ("repository", identity),
        ("format", FORMAT),
        ("hash", "sha256"),
    ] {
        tx.execute(
            "INSERT INTO meta(key,value) VALUES(?1,?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value
             WHERE meta.value<>excluded.value",
            params![key, value],
        )?;
    }
    tx.commit()?;
    Ok(())
}
