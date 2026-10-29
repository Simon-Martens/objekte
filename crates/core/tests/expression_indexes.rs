mod support;
use objekte_core::{CacheMode, JsonQuery, Repository, register_cache_sqlite_functions};
use serde_json::json;
use std::fs;
use support::*;

fn query(text: &str) -> JsonQuery {
    JsonQuery {
        filter: text.into(),
        collections: Some(vec!["books".into()]),
        limit: 1000,
        count: true,
        ..Default::default()
    }
}
fn database(repo: &Repository) -> rusqlite::Connection {
    let db =
        rusqlite::Connection::open(repo.cache_directory().unwrap().join("cache.sqlite")).unwrap();
    register_cache_sqlite_functions(&db).unwrap();
    db
}
fn ids(repo: &mut Repository, text: &str) -> Vec<String> {
    let result = repo.query_json(&query(text)).unwrap();
    assert_eq!(result.total, Some(result.records.len()));
    result.records.into_iter().map(|r| r.id).collect()
}
fn fixture() -> Fixture {
    let f = Fixture::new();
    for e in fs::read_dir(f.root.join("data/books")).unwrap() {
        fs::remove_file(e.unwrap().path()).unwrap();
    }
    for (id, raw) in [
        ("a", r#"{"score":1800,"title":"alpha"}"#),
        ("b", r#"{"score":"1800","title":"beta"}"#),
        ("c", r#"{"score":true,"title":"gamma"}"#),
        ("d", r#"{"score":null}"#),
        ("e", "{}"),
        ("f", r#"{"score":9007199254740993}"#),
        ("g", r#"{"score":1e500}"#),
        ("h", r#"{"score":0.123456789012345678901234567891}"#),
        ("i", r#"{"score":-100,"score":1801}"#),
        ("j", r#"{"score":[1802]}"#),
        ("k", "{broken"),
    ] {
        fs::write(f.root.join(format!("data/books/{id}.json")), raw).unwrap();
    }
    f
}

#[test]
fn scalar_query_indexes_are_lazy_and_survive_reopen() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    r.open_view("books", &Default::default()).unwrap();
    let count = |r: &Repository| {
        database(r)
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='index' AND name GLOB 'j_*_query'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
    };
    assert_eq!(count(&r), 0);
    for text in [
        "exists(score)",
        "score ?= 2",
        "score:each >= 0",
        "title:lower = 'alpha'",
    ] {
        assert_eq!(r.query_json(&query(text)).unwrap().metrics.index_builds, 0);
        assert_eq!(count(&r), 0);
    }
    let first = r.query_json(&query("score >= 0")).unwrap();
    assert_eq!(first.metrics.index_builds, 1);
    assert_eq!(first.metrics.source_reads, 0);
    assert_eq!(count(&r), 1);
    drop(r);
    let mut r = f.repo(CacheMode::Sqlite);
    let next = r.query_json(&query("score >= 0")).unwrap();
    assert_eq!(next.metrics.index_builds, 0);
    assert_eq!(first.total, next.total);
    assert_eq!(count(&r), 1);
    assert_eq!(
        r.query_json(&query("title = 'alpha'"))
            .unwrap()
            .metrics
            .index_builds,
        1
    );
    assert_eq!(count(&r), 2);
}

#[test]
fn queries_seek_json_expression_indexes_without_stored_field_columns() {
    let f = fixture();
    let mut r = f.repo(CacheMode::Sqlite);
    let initial = r
        .query_json(&query("score >= 1800 && score < 1900"))
        .unwrap();
    assert_eq!(initial.total, Some(3));
    assert!(initial.metrics.index_builds > 0);
    let result = r
        .query_json(&query("score >= 1800 && score < 1900"))
        .unwrap();
    assert_eq!(result.metrics.source_reads, 0);
    assert_eq!(result.metrics.enumerations, 0);
    assert!(
        result
            .query_plan
            .iter()
            .any(|line| line.contains("SEARCH r USING INDEX j_")
                && line.contains("_query")
                && line.contains("<expr>")),
        "{:?}",
        result.query_plan
    );
    assert!(
        !result
            .query_plan
            .iter()
            .any(|line| line.starts_with("SCAN r")),
        "An indexed small page must not scan raw records during its payload join: {:?}",
        result.query_plan
    );
    let view = r.open_view("books", &Default::default()).unwrap();
    assert!(
        view.query_plan
            .iter()
            .any(|line| line.contains("SCAN v USING INDEX j_") && line.contains("_asc")),
        "{:?}",
        view.query_plan
    );
    let db = database(&r);
    let tables = db
        .prepare("SELECT name FROM sqlite_master WHERE type='table'")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(!tables.iter().any(|t| t.starts_with("v_")), "{tables:?}");
    let columns = db
        .prepare("PRAGMA table_xinfo(records)")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        columns,
        [
            "collection",
            "id",
            "path",
            "size",
            "modified",
            "identity",
            "digest",
            "json",
            "revision"
        ]
    );
    let definitions = db
        .prepare("SELECT sql FROM sqlite_master WHERE type='index' AND name GLOB 'j_*'")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(
        definitions
            .iter()
            .any(|sql| sql.contains("objekte_json_key(json,"))
    );
    assert!(definitions.iter().all(|sql| sql.contains("ON records(")));
    assert_eq!(
        db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    let mut direct = f.repo(CacheMode::Disabled);
    for expression in [
        "score=1800",
        "score='1800'",
        "score=true",
        "score=null",
        "score!=1800",
        "score>9007199254740992",
        "score=9007199254740993",
        "score=1e500",
        "score > 0.123456789012345678901234567890 && score < 1",
        "1800 <= score",
        "score=1800 || title='beta'",
        "score=1800 && title ~ 'alp%'",
        "score<true",
        "score ?= 1802",
        "score:each >= 1800",
        "score ?!= 1800",
    ] {
        assert_eq!(
            ids(&mut r, expression),
            ids(&mut direct, expression),
            "{expression}"
        );
    }
    assert_eq!(ids(&mut r, "score = 9007199254740993"), ["f"]);
    assert_eq!(ids(&mut r, "score=1800"), ["a"]);
    assert_eq!(ids(&mut r, "score='1800'"), ["b"]);
    // Scalar ambiguity must still be detected even in records an index would skip.
    f.write("data/books/z.json", &json!({"score":[1,2]}));
    r.refresh("books").unwrap();
    let mut q = query("score >= 1800");
    q.limit = 1;
    assert!(
        r.query_json(&q)
            .unwrap_err()
            .to_string()
            .contains("multiple values")
    );
}

#[test]
fn expression_indexes_follow_edits_deletes_restarts_and_reconfiguration() {
    let f = fixture();
    let mut r = f.repo(CacheMode::Sqlite);
    assert_eq!(ids(&mut r, "score=1800"), ["a"]);
    f.write("data/books/a.json", &json!({"score":"1800"}));
    fs::remove_file(f.root.join("data/books/i.json")).unwrap();
    f.write("data/books/new.json", &json!({"score":1800}));
    let delta = r.refresh("books").unwrap();
    assert_eq!(delta.rows_updated, 1);
    assert_eq!(delta.rows_inserted, 1);
    assert_eq!(delta.rows_deleted, 1);
    assert_eq!(ids(&mut r, "score=1800"), ["new"]);
    assert_eq!(ids(&mut r, "score='1800'"), ["a", "b"]);
    drop(r);
    let mut r = f.repo(CacheMode::Sqlite);
    assert_eq!(ids(&mut r, "score=1800"), ["new"]);
    assert_eq!(
        database(&r)
            .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    f.edit(".editor/books/ui.json", |v| {
        v["list"]["sortable"]
            .as_array_mut()
            .unwrap()
            .push(json!("type"))
    });
    let next = r.open_view("books", &all()).unwrap();
    assert_eq!(next.metrics.source_reads, 0);
    assert!(next.metrics.index_builds > 0);
    assert_eq!(ids(&mut r, "score=1800"), ["new"]);
    let db = database(&r);
    let legacy_table = format!("v_{}", "0".repeat(64));
    db.execute_batch(&format!("CREATE TABLE {legacy_table}(id TEXT PRIMARY KEY, f0_s INTEGER, f0_v); INSERT INTO {legacy_table} VALUES('old',0,1800);")).unwrap();
    db.execute("UPDATE meta SET value='4' WHERE key='format'", [])
        .unwrap();
    drop(db);
    drop(r);
    let mut r = f.repo(CacheMode::Sqlite);
    assert_eq!(ids(&mut r, "score=1800"), ["new"]);
    assert_eq!(
        database(&r)
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name=?1",
                [&legacy_table],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        database(&r)
            .query_row("SELECT value FROM meta WHERE key='format'", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "5"
    );
}

#[test]
fn compound_views_read_covering_index_keys_without_json_functions() {
    use objekte_core::{Direction, Filter, Operator, Sort, ViewRequest};
    let f = Fixture::new();
    let mut cached = f.repo(CacheMode::Sqlite);
    let mut direct = f.repo(CacheMode::Disabled);
    for request in [
        ViewRequest {
            sort: Some(Sort {
                field: "score".into(),
                direction: Direction::Asc,
            }),
            sub_sorts: Some(vec![Sort {
                field: "title".into(),
                direction: Direction::Desc,
            }]),
            count: true,
            ..Default::default()
        },
        ViewRequest {
            filters: vec![Filter {
                field: "score".into(),
                operator: Operator::Ge,
                value: json!(2),
            }],
            sort: Some(Sort {
                field: "title".into(),
                direction: Direction::Asc,
            }),
            count: true,
            ..Default::default()
        },
    ] {
        let actual = cached.open_view("books", &request).unwrap();
        let expected = direct.open_view("books", &request).unwrap();
        assert_eq!(actual.rows, expected.rows);
        assert_eq!(actual.total, expected.total);
        assert!(
            actual
                .query_plan
                .iter()
                .any(|p| p.contains("USING INDEX j_") && p.contains("_identity")),
            "{:?}",
            actual.query_plan
        );
    }
    let db = database(&cached);
    let name: String = db
        .query_row(
            "SELECT name FROM sqlite_master WHERE type='index' AND name GLOB 'j_*_identity'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    // Reading these keys must not call the extractors, even for secondary sorts.
    // Failure functions make a hidden fallback to JSON parsing a hard test error.
    for function in ["objekte_view_state", "objekte_view_value"] {
        db.create_scalar_function(
            function,
            2,
            rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
            |_| -> rusqlite::Result<i64> { Err(rusqlite::Error::InvalidQuery) },
        )
        .unwrap();
    }
    let sql = format!(
        "SELECT id, objekte_view_value(json,'[\"score\",\"number\"]') FROM records INDEXED BY {name} WHERE collection='books' ORDER BY objekte_view_state(json,'[\"score\",\"number\"]'),objekte_view_value(json,'[\"score\",\"number\"]'),objekte_view_state(json,'[\"title\",\"string\"]'),objekte_view_value(json,'[\"title\",\"string\"]') DESC,id LIMIT 50"
    );
    let ids = db
        .prepare(&sql)
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(ids.len(), 7);
}

#[test]
fn configured_quoted_paths_and_comparison_values_remain_sql_data() {
    let f = fixture();
    let field = "odd.'\"; --\nkey";
    let value = "x'); DROP TABLE records; --";
    f.edit(".editor/books/schema.json", |v| {
        v["properties"][field] = json!({"type":"string"})
    });
    f.edit(".editor/books/ui.json", |v| {
        v["list"]["sortable"]
            .as_array_mut()
            .unwrap()
            .push(json!(field))
    });
    f.write("data/books/a.json", &json!({field:value}));
    let mut q = query(&format!(
        "[{}] = $value",
        serde_json::to_string(field).unwrap()
    ));
    q.parameters.insert("value".into(), json!(value));
    let mut r = f.repo(CacheMode::Sqlite);
    let result = r.query_json(&q).unwrap();
    assert_eq!(result.total, Some(1));
    assert_eq!(result.records[0].id, "a");
    assert!(!result.sql.unwrap().contains(value));
    assert!(result.bindings.iter().any(|v| v == &format!("S{value}")));
    assert_eq!(
        database(&r)
            .query_row(
                "SELECT count(*) FROM records WHERE collection='books'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        11
    );
}
