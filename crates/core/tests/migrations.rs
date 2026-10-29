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
fn startup_migrations_create_schema_once() {
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
fn a_cache_without_migration_history_is_rejected_without_data_loss() {
    let f = Fixture::new();
    f.repo(CacheMode::Sqlite)
        .open_view("books", &all())
        .unwrap();
    let db = database(&f);
    db.execute_batch("DROP TABLE migrations").unwrap();
    drop(db);

    assert!(
        f.repo(CacheMode::Sqlite)
            .open_view("books", &all())
            .is_err()
    );
    let db = database(&f);
    assert_eq!(
        db.query_row("SELECT count(*) FROM collections", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn an_untracked_table_is_not_adopted_as_a_new_cache() {
    let f = Fixture::new();
    let directory = f.repo(CacheMode::Sqlite).cache_directory().unwrap();
    fs::create_dir_all(&directory).unwrap();
    let db = Connection::open(directory.join("cache.sqlite")).unwrap();
    db.execute_batch(
        "CREATE TABLE migrations(version INTEGER PRIMARY KEY,name TEXT NOT NULL,applied_at TEXT NOT NULL);
         CREATE TABLE legacy_records(id TEXT PRIMARY KEY);",
    )
    .unwrap();
    drop(db);

    assert!(
        f.repo(CacheMode::Sqlite)
            .open_view("books", &all())
            .is_err()
    );
    assert_eq!(
        database(&f)
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name='legacy_records'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

#[test]
fn failed_migration_keeps_history_pending_and_retries_on_next_open() {
    let f = Fixture::new();
    let repo = f.repo(CacheMode::Sqlite);
    let directory = repo.cache_directory().unwrap();
    fs::create_dir_all(&directory).unwrap();
    let db = Connection::open(directory.join("cache.sqlite")).unwrap();
    db.execute_batch(
        "CREATE TABLE migrations(version INTEGER PRIMARY KEY,name TEXT NOT NULL,applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);
         INSERT INTO migrations(version,name) VALUES(1,'repository_metadata');",
    )
    .unwrap();
    db.execute_batch(include_str!(
        "../src/store/migrations/001_repository_metadata.sql"
    ))
    .unwrap();
    db.execute(
        "INSERT INTO meta(key,value) VALUES('repository',?1)",
        [f.root.to_str().unwrap()],
    )
    .unwrap();
    db.execute("INSERT INTO meta(key,value) VALUES('format','7')", [])
        .unwrap();
    db.execute_batch("CREATE TABLE records(unexpected TEXT)")
        .unwrap();
    drop(db);
    assert!(
        f.repo(CacheMode::Sqlite)
            .open_view("books", &all())
            .is_err()
    );
    let db = database(&f);
    assert_eq!(applied(&db), [(1, "repository_metadata".into())]);
    db.execute_batch("DROP TABLE records").unwrap();
    drop(db);
    let recovered = f
        .repo(CacheMode::Sqlite)
        .open_view("books", &all())
        .unwrap();
    assert_eq!(recovered.total, Some(7));
    assert_eq!(applied(&database(&f)).len(), 2);
}


#[test]
fn previous_disposable_cache_format_is_rebuilt() {
    let f = Fixture::new();
    let mut repo = f.repo(CacheMode::Sqlite);
    repo.open_view("books", &all()).unwrap();
    let directory = repo.cache_directory().unwrap();
    drop(repo);

    let db = Connection::open(directory.join("cache.sqlite")).unwrap();
    db.execute("UPDATE meta SET value='6' WHERE key='format'", []).unwrap();
    db.execute_batch("CREATE TABLE obsolete_marker(value TEXT)").unwrap();
    drop(db);

    let mut repo = f.repo(CacheMode::Sqlite);
    let view = repo.open_view("books", &all()).unwrap();
    assert_eq!(view.total, Some(7));
    assert_eq!(view.metrics.rows_inserted, 7);
    let db = Connection::open(directory.join("cache.sqlite")).unwrap();
    let obsolete: i64 = db.query_row(
        "SELECT count(*) FROM sqlite_master WHERE name='obsolete_marker'",
        [],
        |row| row.get(0),
    ).unwrap();
    assert_eq!(obsolete, 0);
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
