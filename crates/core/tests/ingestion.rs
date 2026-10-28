mod support;
use objekte_core::{CacheMode, Freshness, Repository};
use rusqlite::Connection;
use serde_json::json;
use std::fs;
use support::*;

fn seed(f: &Fixture) {
    for i in 0..600 {
        f.write(
            &format!("data/books/r{i:04}.json"),
            &json!({
                "title": format!("Record {i}"), "score": i % 13, "payload": null,
                "person": "p1", "links": [{"person": "p2"}, {"person": "p2"}]
            }),
        );
    }
}

fn database(repo: &Repository) -> Connection {
    Connection::open(repo.cache_directory().unwrap().join("cache.sqlite")).unwrap()
}

fn edges(repo: &Repository) -> Vec<(String, String, String, String)> {
    database(repo).prepare(
        "SELECT source_id,location,target_collection,target_id FROM edges ORDER BY source_id,location,target_collection,target_id"
    ).unwrap().query_map([], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))
        .unwrap().collect::<Result<_,_>>().unwrap()
}

#[test]
fn bounded_parallel_batches_match_serial_rows_edges_and_updates() {
    let f = Fixture::new();
    seed(&f);
    // Exceeds the per-batch source-byte budget and must still make progress.
    f.edit("data/books/r0300.json", |v| {
        v["payload"] = json!("x".repeat(9 * 1024 * 1024))
    });
    let expected = f
        .repo(CacheMode::Disabled)
        .open_view("books", &all())
        .unwrap();
    let mut expected_edges = None;
    for workers in [1, 2, 4] {
        let mut options = f.options(CacheMode::Sqlite);
        options.ingest_workers = workers;
        options.namespace = format!("workers-{workers}");
        let mut repo = Repository::load_repository(&f.root, options.clone()).unwrap();
        let view = repo.open_view("books", &all()).unwrap();
        assert_eq!(view.rows, expected.rows);
        assert_eq!(view.view_hashes, expected.view_hashes);
        assert_eq!(view.metrics.source_reads, 607);
        assert_eq!(view.metrics.validations, 607);
        assert_eq!(view.metrics.rows_inserted, 607);
        let actual_edges = edges(&repo);
        if let Some(expected_edges) = &expected_edges {
            assert_eq!(&actual_edges, expected_edges);
        } else {
            expected_edges = Some(actual_edges);
        }
        assert_eq!(
            database(&repo)
                .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        let warm = repo.open_view("books", &all()).unwrap();
        assert_eq!(warm.rows, expected.rows);
        assert_eq!(warm.metrics.source_reads, 0);
        options.freshness = Freshness::HashOnOpen;
        let strict = Repository::load_repository(&f.root, options)
            .unwrap()
            .open_view("books", &all())
            .unwrap();
        assert_eq!(strict.rows, expected.rows);
        assert_eq!(strict.metrics.hashes, 607);
        assert_eq!(strict.metrics.parses, 0);

        f.edit("data/books/r0400.json", |v| {
            v["score"] = json!(-20);
            v["person"] = json!("p2");
        });
        let update = repo.refresh("books").unwrap();
        assert_eq!(update.rows_updated, 1);
        assert_eq!(update.validations, 1);
        assert_eq!(update.edges_added, 1);
        assert_eq!(update.edges_removed, 1);
        assert_eq!(
            repo.open_view("books", &all()).unwrap().rows,
            f.repo(CacheMode::Disabled)
                .open_view("books", &all())
                .unwrap()
                .rows
        );
        f.edit("data/books/r0400.json", |v| {
            v["score"] = json!(400 % 13);
            v["person"] = json!("p1");
        });
    }
}

#[test]
fn parallel_errors_are_ordered_and_roll_back_earlier_batches_then_recover() {
    let f = Fixture::new();
    seed(&f);
    let original = f.read("data/books/r0300.json");
    f.edit("data/books/r0300.json", |v| {
        v["score"] = json!("wrong type")
    });
    fs::write(f.root.join("data/books/r0301.json"), "malformed JSON").unwrap();
    let mut first_error = None;
    for workers in [1, 2, 4] {
        let mut options = f.options(CacheMode::Sqlite);
        options.ingest_workers = workers;
        options.namespace = format!("error-{workers}");
        let mut repo = Repository::load_repository(&f.root, options).unwrap();
        let error = repo.open_view("books", &all()).unwrap_err().to_string();
        assert!(error.contains("r0300.json#/score"), "{error}");
        if let Some(first_error) = &first_error {
            assert_eq!(&error, first_error);
        } else {
            first_error = Some(error);
        }
        assert_eq!(repo.metrics().rows_inserted, 0);
        assert_eq!(repo.metrics().edges_added, 0);
        assert_eq!(
            database(&repo)
                .query_row("SELECT count(*) FROM records", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            database(&repo)
                .query_row("SELECT count(*) FROM edges", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert!(
            repo.cache_status()
                .unwrap()
                .collections
                .iter()
                .any(|c| c.collection == "books" && c.state == "dirty")
        );
    }
    f.write("data/books/r0300.json", &original);
    f.write("data/books/r0301.json", &json!({"payload": null}));
    let mut options = f.options(CacheMode::Sqlite);
    options.namespace = "error-4".into();
    options.ingest_workers = 4;
    let result = Repository::load_repository(&f.root, options)
        .unwrap()
        .open_view("books", &all())
        .unwrap();
    assert_eq!(result.total, Some(607));
    assert_eq!(result.metrics.rows_inserted, 607);
}
