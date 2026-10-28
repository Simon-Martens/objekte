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
        assert_eq!(view.metrics.validations, 0);
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
        assert_eq!(update.validations, 0);
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
fn parallel_content_errors_are_retained_across_batches_then_recover() {
    let f = Fixture::new();
    seed(&f);
    f.edit("data/books/r0300.json", |v| {
        v["score"] = json!("wrong type")
    });
    fs::write(f.root.join("data/books/r0301.json"), "malformed JSON").unwrap();
    for workers in [1, 2, 4] {
        let mut options = f.options(CacheMode::Sqlite);
        options.ingest_workers = workers;
        options.namespace = format!("error-{workers}");
        let mut repo = Repository::load_repository(&f.root, options).unwrap();
        let view = repo.open_view("books", &all()).unwrap();
        assert_eq!(view.total, Some(607));
        assert_eq!(
            view.states["r0301"].status,
            objekte_core::RecordStatus::IncorrectJson
        );
        assert_eq!(
            view.states["r0300"].status,
            objekte_core::RecordStatus::Pending
        );
        let validated = repo.validate_collection("books").unwrap();
        assert_eq!(validated.applied, 606);
        let view = repo.open_view("books", &all()).unwrap();
        assert_eq!(
            view.states["r0300"].status,
            objekte_core::RecordStatus::Invalid
        );
        assert!(
            view.states["r0300"]
                .issues
                .iter()
                .any(|i| i.location == "/score")
        );
        assert_eq!(
            repo.cache_status()
                .unwrap()
                .collections
                .iter()
                .find(|c| c.collection == "books")
                .unwrap()
                .state,
            "current"
        );
    }
    f.write("data/books/r0300.json", &json!({"payload":null,"score":4}));
    f.write("data/books/r0301.json", &json!({"payload":null}));
    let mut options = f.options(CacheMode::Sqlite);
    options.namespace = "error-4".into();
    let mut repo = Repository::load_repository(&f.root, options).unwrap();
    let result = repo.open_view("books", &all()).unwrap();
    assert_eq!(result.metrics.rows_inserted, 0);
    assert_eq!(result.metrics.rows_updated, 2);
    assert_eq!(repo.validate_collection("books").unwrap().applied, 2);
}
