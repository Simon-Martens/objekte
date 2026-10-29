mod support;

use objekte_core::CacheMode;
use rusqlite::Connection;
use std::fs;
use support::*;

fn database(f: &Fixture) -> Connection {
    let repo = f.repo(CacheMode::Sqlite);
    Connection::open(repo.cache_directory().unwrap().join("cache.sqlite")).unwrap()
}

fn applied(db: &Connection) -> Vec<(i64, String)> {
    db.prepare("SELECT version,name FROM migrations ORDER BY version")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn startup_migrations_create_schema_once_and_adopt_existing_cache() {
    let f = Fixture::new();
    let first = f
        .repo(CacheMode::Sqlite)
        .open_view("books", &all())
        .unwrap();
    assert_eq!(first.total, Some(7));
    let db = database(&f);
    assert_eq!(
        applied(&db),
        [
            (1, "repository_metadata".into()),
            (2, "records_and_indexes".into())
        ]
    );
    // INFO: format-6 caches from before migration tracking have populated records
    // but no history table. Reopening must adopt their schema without data loss.
    db.execute_batch("DROP TABLE migrations").unwrap();
    drop(db);
    let reopened = f
        .repo(CacheMode::Sqlite)
        .open_view("books", &all())
        .unwrap();
    assert_eq!(reopened.total, Some(7));
    assert_eq!(reopened.metrics.rows_inserted, 0);
    let db = database(&f);
    assert_eq!(
        applied(&db),
        [
            (1, "repository_metadata".into()),
            (2, "records_and_indexes".into())
        ]
    );
    db.execute(
        "UPDATE migrations SET applied_at='sentinel' WHERE version=1",
        [],
    )
    .unwrap();
    drop(db);
    let warm = f
        .repo(CacheMode::Sqlite)
        .open_view("books", &all())
        .unwrap();
    assert_eq!(warm.metrics.rows_inserted, 0);
    assert_eq!(warm.metrics.parses, 0);
    assert_eq!(
        database(&f)
            .query_row(
                "SELECT applied_at FROM migrations WHERE version=1",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "sentinel"
    );
}

#[test]
fn failed_migration_keeps_history_pending_and_retries_on_next_open() {
    let f = Fixture::new();
    let repo = f.repo(CacheMode::Sqlite);
    let directory = repo.cache_directory().unwrap();
    fs::create_dir_all(&directory).unwrap();
    let db = Connection::open(directory.join("cache.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE record_validation(unexpected TEXT)")
        .unwrap();
    drop(db);
    assert!(
        f.repo(CacheMode::Sqlite)
            .open_view("books", &all())
            .is_err()
    );
    let db = database(&f);
    assert_eq!(applied(&db), [(1, "repository_metadata".into())]);
    db.execute_batch("DROP TABLE record_validation").unwrap();
    drop(db);
    let recovered = f
        .repo(CacheMode::Sqlite)
        .open_view("books", &all())
        .unwrap();
    assert_eq!(recovered.total, Some(7));
    assert_eq!(applied(&database(&f)).len(), 2);
}

#[test]
fn newer_migration_history_is_rejected_without_rebuilding_the_cache() {
    let f = Fixture::new();
    f.repo(CacheMode::Sqlite)
        .open_view("books", &all())
        .unwrap();
    let db = database(&f);
    db.execute(
        "INSERT INTO migrations(version,name) VALUES(99,'future_schema')",
        [],
    )
    .unwrap();
    drop(db);

    assert!(
        f.repo(CacheMode::Sqlite)
            .open_view("books", &all())
            .is_err()
    );
    let db = database(&f);
    assert_eq!(applied(&db).last(), Some(&(99, "future_schema".into())));
}
