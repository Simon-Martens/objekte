mod support;

use objekte_core::{CacheMode, JsonQuery, RecordStatus};
use rusqlite::Connection;
use serde_json::json;
use std::fs;
use support::*;


fn query(filter: &str) -> JsonQuery {
    JsonQuery {
        filter: filter.into(),
        collections: Some(vec!["books".into()]),
        count: true,
        ..Default::default()
    }
}


#[test]
fn query_validates_pending_rows_and_keeps_type_safe_invalid_matches() {
    let f = Fixture::new();
    for entry in fs::read_dir(f.root.join("data/books")).unwrap() {
        fs::remove_file(entry.unwrap().path()).unwrap();
    }
    f.write("data/books/valid.json", &json!({"payload":null,"score":2,"title":"foo"}));
    f.write("data/books/invalid.json", &json!({"score":2,"title":"food"}));
    f.write("data/books/wrong_type.json", &json!({"payload":null,"score":"2","title":"foo"}));
    f.write("data/books/missing.json", &json!({"payload":null,"title":"foo"}));
    fs::write(f.root.join("data/books/broken.json"), "{broken").unwrap();

    let mut repo = f.repo(CacheMode::Sqlite);
    let result = repo.query_json(&query("score = 2 && title ~ 'foo'")).unwrap();
    assert_eq!(result.total, Some(2));
    let states = result.records.iter().map(|r| (r.id.as_str(), r.state.status)).collect::<Vec<_>>();
    assert_eq!(states, [("invalid", RecordStatus::Invalid), ("valid", RecordStatus::Valid)]);

    let db = Connection::open(repo.cache_directory().unwrap().join("cache.sqlite")).unwrap();
    let pending: i64 = db.query_row(
        "SELECT count(*) FROM records WHERE collection='books' AND validation_state='pending'",
        [],
        |row| row.get(0),
    ).unwrap();
    assert_eq!(pending, 0);
    let tables = db.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'record%'")
        .unwrap().query_map([], |row| row.get::<_, String>(0)).unwrap()
        .collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(tables, ["records"]);
}


#[test]
fn explicit_query_scope_rejects_undeclared_or_wrongly_typed_fields() {
    let f = Fixture::new();
    let mut repo = f.repo(CacheMode::Sqlite);
    for filter in ["unknown = 2", "score = '2'", "score ~ '2'"] {
        assert!(repo.query_json(&query(filter)).is_err(), "{filter}");
    }
    assert!(!f.data.exists());
}


#[test]
fn global_query_validates_collections_outside_the_filter_schema() {
    let f = Fixture::new();
    let mut repo = f.repo(CacheMode::Sqlite);
    repo.query_json(&JsonQuery {
        filter: "score = 2".into(),
        count: true,
        ..Default::default()
    }).unwrap();

    let db = Connection::open(repo.cache_directory().unwrap().join("cache.sqlite")).unwrap();
    let pending: i64 = db.query_row(
        "SELECT count(*) FROM records WHERE validation_state='pending'",
        [],
        |row| row.get(0),
    ).unwrap();
    assert_eq!(pending, 0);
    let collections: i64 = db.query_row(
        "SELECT count(DISTINCT collection) FROM records",
        [],
        |row| row.get(0),
    ).unwrap();
    assert_eq!(collections, 3);
}
