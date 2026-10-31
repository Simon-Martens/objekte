mod support;

use objekte_core::{CacheMode, Freshness, JsonQuery, JsonQueryBackend, Repository};
use serde_json::json;
use std::fs;
use support::Fixture;


fn fixture() -> Fixture {
    let fixture = Fixture::new();
    for entry in fs::read_dir(fixture.root.join("data/books")).unwrap() {
        fs::remove_file(entry.unwrap().path()).unwrap();
    }
    for (id, value) in [
        (
            "a",
            json!({"title":"Alpha","detail":{"rank":2,"active":true},"payload":null}),
        ),
        (
            "b",
            json!({"title":"A%","detail":{"rank":null,"active":false},"payload":null}),
        ),
        (
            "c",
            json!({"title":"Beta","detail":{"active":null},"payload":null}),
        ),
        ("d", json!({"payload":null})),
        (
            "e",
            json!({"title":"Able","detail":{"rank":4,"active":false},"payload":null}),
        ),
    ] {
        fixture.write(&format!("data/books/{id}.json"), &value);
    }
    fs::write(fixture.root.join("data/books/bad.json"), b"invalid JSON").unwrap();
    fixture
}

fn query(filter: &str) -> JsonQuery {
    JsonQuery {
        filter: filter.into(),
        collections: Some(vec!["books".into()]),
        count: true,
        limit: 100,
        ..Default::default()
    }
}

#[test]
fn runtime_postings_cover_nested_missing_null_ranges_and_booleans() {
    let fixture = fixture();
    let mut repository = fixture.repo(CacheMode::Memory);
    let before = repository
        .query_json(&query("missing(detail.rank)"))
        .unwrap();
    assert_eq!(before.total, Some(2));
    assert!(before.metrics.grep_scans > 0);

    repository.build_posting("books", "detail.rank").unwrap();
    repository.build_posting("books", "detail.active").unwrap();
    for (filter, expected) in [
        ("missing(detail.rank)", vec!["c", "d"]),
        ("exists(detail.rank)", vec!["a", "b", "e"]),
        ("detail.rank = null", vec!["b"]),
        ("detail.rank = 2", vec!["a"]),
        ("detail.rank < 4", vec!["a"]),
        ("detail.rank <= 2", vec!["a"]),
        ("detail.rank > 2", vec!["e"]),
        ("detail.rank >= 3", vec!["e"]),
        ("3 < detail.rank", vec!["e"]),
        ("2 >= detail.rank", vec!["a"]),
        ("detail.rank != 2", vec!["b", "e"]),
        ("detail.rank != null", vec!["a", "e"]),
        ("exists(detail.rank) && detail.rank >= 2", vec!["a", "e"]),
        (
            "detail.rank = 2 || missing(detail.rank)",
            vec!["a", "c", "d"],
        ),
        ("detail.active = true", vec!["a"]),
        ("detail.active = false", vec!["b", "e"]),
        ("detail.active = null", vec!["c"]),
        ("detail.active != true", vec!["b", "c", "e"]),
        ("exists(detail.active)", vec!["a", "b", "c", "e"]),
        ("missing(detail.active)", vec!["d"]),
    ] {
        let indexed = repository.query_json(&query(filter)).unwrap();
        let direct = repository
            .query_json(&JsonQuery {
                backend: JsonQueryBackend::Direct,
                ..query(filter)
            })
            .unwrap();
        let ids = indexed
            .records
            .iter()
            .map(|record| record.id.as_str())
            .collect::<Vec<_>>();
        let direct_ids = direct
            .records
            .iter()
            .map(|record| record.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, expected, "{filter}");
        assert_eq!(ids, direct_ids, "{filter}");
        assert_eq!(indexed.total, direct.total, "{filter}");
        assert_eq!(indexed.metrics.grep_scans, 0, "{filter}");
    }
}

#[test]
fn runtime_string_posting_keeps_percent_literal_for_equality_and_indexes_prefixes() {
    let fixture = fixture();
    let mut repository = fixture.repo(CacheMode::Memory);
    repository.build_posting("books", "title").unwrap();
    for (filter, expected) in [
        ("title = 'A%'", vec!["b"]),
        ("'A%' = title", vec!["b"]),
        ("title ~ 'A%'", vec!["a", "b", "e"]),
        ("title ~ 'A%%'", vec!["a", "b", "e"]),
        ("title < 'Beta'", vec!["a", "b", "e"]),
        ("missing(title)", vec!["d"]),
    ] {
        let result = repository.query_json(&query(filter)).unwrap();
        let ids = result
            .records
            .iter()
            .map(|record| record.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, expected, "{filter}");
        assert_eq!(result.metrics.grep_scans, 0, "{filter}");
    }

    fixture.edit("data/books/a.json", |record| {
        record["title"] = json!("Zenith")
    });
    repository.refresh("books").unwrap();
    let stale = repository.query_json(&query("title ~ 'A%'")).unwrap();
    assert_eq!(stale.total, Some(2));
    assert!(stale.metrics.grep_scans > 0);
    repository.build_posting("books", "title").unwrap();
    assert_eq!(
        repository.query_json(&query("title ~ 'A%'")).unwrap().total,
        Some(2)
    );
    repository.rebuild_bigram_index("books").unwrap();
    let rebuilt = repository.query_json(&query("title ~ 'A%'")).unwrap();
    assert_eq!(rebuilt.total, Some(2));
    assert_eq!(rebuilt.metrics.grep_scans, 0);
}

#[test]
fn unsafe_multi_value_path_keeps_scalar_cardinality_error() {
    let fixture = fixture();
    fixture.write(
        "data/books/c.json",
        &json!({"detail":[{"rank":1},{"rank":2}],"payload":null}),
    );
    let mut repository = fixture.repo(CacheMode::Memory);
    repository.build_posting("books", "detail.rank").unwrap();
    let error = repository
        .query_json(&query("detail.rank = 2"))
        .unwrap_err();
    assert!(error.to_string().contains("multiple values"), "{error}");
    let missing = repository
        .query_json(&query("missing(detail.rank)"))
        .unwrap();
    assert_eq!(missing.total, Some(1));
    assert_eq!(missing.metrics.grep_scans, 0);
}

#[test]
fn all_collection_posting_api_covers_unscoped_missing_query() {
    let fixture = fixture();
    let mut repository = fixture.repo(CacheMode::Memory);
    repository.build_posting_all("title").unwrap();
    let request = JsonQuery {
        filter: "missing(title)".into(),
        count: true,
        limit: 100,
        ..Default::default()
    };
    let indexed = repository.query_json(&request).unwrap();
    let direct = repository
        .query_json(&JsonQuery {
            backend: JsonQueryBackend::Direct,
            ..request
        })
        .unwrap();
    assert_eq!(indexed.total, direct.total);
    assert_eq!(
        indexed
            .records
            .iter()
            .map(|record| (&record.collection, &record.id))
            .collect::<Vec<_>>(),
        direct
            .records
            .iter()
            .map(|record| (&record.collection, &record.id))
            .collect::<Vec<_>>()
    );
    assert_eq!(indexed.metrics.grep_scans, 0);
}

#[test]
fn runtime_posting_accepts_quoted_keys_and_array_indexes() {
    let fixture = fixture();
    fixture.edit("data/books/a.json", |record| {
        record["a.b"] = json!({"items":[{"rank":2}]});
    });
    let mut repository = fixture.repo(CacheMode::Memory);
    repository
        .build_posting("books", r#"["a.b"].items[0].rank"#)
        .unwrap();
    for (filter, count, first) in [
        (r#"["a.b"].items[0].rank = 2"#, 1, "a"),
        (r#"exists(["a.b"].items[0].rank)"#, 1, "a"),
        (r#"missing(["a.b"].items[0].rank)"#, 4, "b"),
    ] {
        let result = repository.query_json(&query(filter)).unwrap();
        assert_eq!(result.total, Some(count), "{filter}");
        assert_eq!(result.records[0].id, first, "{filter}");
        assert_eq!(result.metrics.grep_scans, 0, "{filter}");
    }
}

#[test]
fn rebuilding_an_existing_schema_posting_replaces_old_values() {
    let fixture = fixture();
    fixture.edit("data/books/a.json", |record| record["score"] = json!(2));
    let mut repository = fixture.repo(CacheMode::Memory);
    repository.build_posting("books", "score").unwrap();
    assert_eq!(
        repository.query_json(&query("score = 2")).unwrap().total,
        Some(1)
    );

    fixture.edit("data/books/a.json", |record| record["score"] = json!(3));
    repository.build_posting("books", "score").unwrap();
    assert_eq!(
        repository.query_json(&query("score = 2")).unwrap().total,
        Some(0)
    );
    assert_eq!(
        repository.query_json(&query("score = 3")).unwrap().total,
        Some(1)
    );
    repository.rebuild_bigram_index("books").unwrap();
    let rebuilt = repository.query_json(&query("score = 3")).unwrap();
    assert_eq!(rebuilt.total, Some(1));
    assert_eq!(rebuilt.metrics.grep_scans, 0);
}

#[test]
fn requested_posting_survives_hash_on_request_picker_replacement() {
    let fixture = fixture();
    let mut options = fixture.options(CacheMode::Memory);
    options.freshness = Freshness::HashOnRequest;
    let mut repository = Repository::load_repository(&fixture.root, options).unwrap();
    repository.build_posting("books", "detail.rank").unwrap();
    for _ in 0..2 {
        let result = repository.query_json(&query("detail.rank >= 2")).unwrap();
        assert_eq!(result.total, Some(2));
        assert_eq!(result.metrics.grep_scans, 0);
    }
}
