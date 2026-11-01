//! Covered scalar counts and necessary top-level presence filtering.

mod support;

use objekte_core::{CacheMode, JsonQuery, JsonQueryBackend};
use serde_json::json;
use std::{fs::{self, File, FileTimes}, path::Path, time::{Duration, UNIX_EPOCH}};
use support::Fixture;

fn fixture() -> Fixture {
    let fixture = Fixture::new();
    let books = fixture.root.join("data/books");
    for entry in fs::read_dir(&books).unwrap() {
        fs::remove_file(entry.unwrap().path()).unwrap();
    }
    for (id, value) in [
        (
            "a",
            json!({"score":2,"flag":true,"title":"keep","payload":null}),
        ),
        (
            "b",
            json!({"score":3,"flag":false,"title":"skip","payload":null}),
        ),
        ("c", json!({"score":null,"title":"keep","payload":null})),
        ("d", json!({"payload":null})),
    ] {
        fixture.write(&format!("data/books/{id}.json"), &value);
    }
    fs::write(books.join("e.json"), b"not JSON").unwrap();
    for (id, seconds) in [("a", 500), ("b", 400), ("c", 300), ("d", 200), ("e", 100)] {
        set_modified(&books.join(format!("{id}.json")), seconds);
    }
    fixture
}

fn set_modified(path: &Path, seconds: u64) {
    File::options().write(true).open(path).unwrap().set_times(
        FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(seconds))
    ).unwrap();
}

#[test]
fn string_postings_cover_comparisons_when_opted_in() {
    let fixture = fixture();
    fixture.edit(".editor/books/schema.json", |schema| {
        schema["properties"]["title"]["index"] = json!(true);
    });
    let mut repository = fixture.repo(CacheMode::Memory);
    for filter in [
        "title = 'keep'",
        "title != 'skip'",
        "title < 'skip'",
        "title >= 'skip'",
        "title ~ 'k%'",
        "missing(title)",
        "exists(title)",
    ] {
        let indexed = repository.query_json(&query(filter)).unwrap();
        let direct = repository
            .query_json(&JsonQuery {
                backend: JsonQueryBackend::Direct,
                ..query(filter)
            })
            .unwrap();
        assert_eq!(indexed.total, direct.total, "{filter}");
        assert_eq!(
            indexed
                .records
                .iter()
                .map(|record| &record.id)
                .collect::<Vec<_>>(),
            direct
                .records
                .iter()
                .map(|record| &record.id)
                .collect::<Vec<_>>(),
            "{filter}"
        );
        assert_eq!(indexed.metrics.grep_scans, 0, "{filter}");
    }
}

#[test]
fn unindexed_scalar_queries_fall_back_to_source_verification() {
    let fixture = fixture();
    fixture.edit(".editor/books/schema.json", |schema| {
        schema["properties"]["score"]
            .as_object_mut()
            .unwrap()
            .remove("index");
    });
    let mut repository = fixture.repo(CacheMode::Memory);
    let indexed = repository.query_json(&query("score >= 2")).unwrap();
    let direct = repository
        .query_json(&JsonQuery {
            backend: JsonQueryBackend::Direct,
            ..query("score >= 2")
        })
        .unwrap();
    assert_eq!(indexed.total, direct.total);
    assert_eq!(
        indexed
            .records
            .iter()
            .map(|record| &record.id)
            .collect::<Vec<_>>(),
        direct
            .records
            .iter()
            .map(|record| &record.id)
            .collect::<Vec<_>>()
    );
    assert!(indexed.metrics.grep_scans > 0);
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
fn covered_counts_page_without_parsing_non_page_matches() {
    let fixture = fixture();
    let mut repository = fixture.repo(CacheMode::Memory);
    let page = repository
        .query_json(&JsonQuery {
            offset: 1,
            limit: 1,
            ..query("exists(score)")
        })
        .unwrap();
    assert_eq!(page.total, Some(3));
    assert_eq!(page.records[0].id, "b");
    assert_eq!(page.metrics.source_reads, 1);
    assert_eq!(page.metrics.parses, 1);
    assert_eq!(page.metrics.grep_scans, 0);
    assert!(
        page.query_plan
            .iter()
            .any(|step| step.contains("exact scalar"))
    );

    let missing = repository.query_json(&query("missing(score)")).unwrap();
    assert_eq!(missing.total, Some(1));
    assert_eq!(missing.records[0].id, "d");
    assert_eq!(missing.metrics.grep_scans, 0);

    for (filter, ids) in [
        ("score >= 2 && flag = true", vec!["a"]),
        ("score = 2 || flag = false", vec!["a", "b"]),
        ("exists(score) && score <= 3", vec!["a", "b"]),
    ] {
        let result = repository.query_json(&query(filter)).unwrap();
        assert_eq!(result.total, Some(ids.len()), "{filter}");
        assert_eq!(
            result
                .records
                .iter()
                .map(|record| record.id.as_str())
                .collect::<Vec<_>>(),
            ids,
            "{filter}"
        );
        assert_eq!(result.metrics.grep_scans, 0, "{filter}");
    }
}

#[test]
fn presence_narrows_negation_and_refresh_updates_bits() {
    let fixture = fixture();
    let mut repository = fixture.repo(CacheMode::Memory);
    let negative = repository.query_json(&query("title !~ 'skip'")).unwrap();
    assert_eq!(negative.total, Some(2));
    assert_eq!(
        negative
            .records
            .iter()
            .map(|record| record.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "c"]
    );
    assert_eq!(negative.metrics.grep_scans, 3);

    fixture.write("data/books/b.json", &json!({"title":"skip","payload":null}));
    fixture.write("data/books/f.json", &json!({"score":4,"payload":null}));
    set_modified(&fixture.root.join("data/books/f.json"), 600);
    repository.refresh("books").unwrap();
    let present = repository.query_json(&query("exists(score)")).unwrap();
    assert_eq!(present.total, Some(3));
    assert_eq!(
        present
            .records
            .iter()
            .map(|record| record.id.as_str())
            .collect::<Vec<_>>(),
        ["f", "a", "c"]
    );
    assert_eq!(present.metrics.grep_scans, 3);
}

#[test]
fn covered_count_uses_snapshot_until_explicit_refresh() {
    let fixture = fixture();
    let mut repository = fixture.repo(CacheMode::Memory);
    repository.query_json(&query("score = 2")).unwrap();
    fixture.write(
        "data/books/a.json",
        &json!({"score":222,"title":"keep","payload":null}),
    );
    let snapshot = repository
        .query_json(&JsonQuery {
            offset: 1,
            limit: 1,
            ..query("score = 2")
        })
        .unwrap();
    assert_eq!(snapshot.total, Some(1));
    assert!(snapshot.records.is_empty());
    assert_eq!(snapshot.metrics.grep_scans, 0);
    assert_eq!(snapshot.metrics.parses, 0);

    repository.refresh("books").unwrap();
    assert_eq!(
        repository.query_json(&query("score = 2")).unwrap().total,
        Some(0)
    );
    assert_eq!(
        repository.query_json(&query("score = 222")).unwrap().total,
        Some(1)
    );
}

#[test]
fn typed_index_covers_null_inequality_and_every_numeric_bound() {
    let fixture = fixture();
    fixture.write("data/books/f.json", &json!({"score":"wrong type"}));
    fixture.write("data/books/g.json", &json!({"score":{"nested":2}}));
    set_modified(&fixture.root.join("data/books/f.json"), 50);
    set_modified(&fixture.root.join("data/books/g.json"), 40);
    let mut repository = fixture.repo(CacheMode::Memory);

    for (filter, count, first_id) in [
        ("score = null", 1, Some("c")),
        ("score != null", 3, Some("a")),
        ("score = 2", 1, Some("a")),
        ("score != 2", 3, Some("b")),
        ("score < 3", 1, Some("a")),
        ("score <= 2", 1, Some("a")),
        ("score > 2", 1, Some("b")),
        ("score >= 2", 2, Some("a")),
        ("score = 2 || score = null", 2, Some("a")),
        ("score != 2 && flag = false", 1, Some("b")),
        ("flag != true", 1, Some("b")),
    ] {
        let request = JsonQuery {
            limit: 1,
            ..query(filter)
        };
        let indexed = repository.query_json(&request).unwrap();
        let direct = repository
            .query_json(&JsonQuery {
                backend: JsonQueryBackend::Direct,
                ..request
            })
            .unwrap();
        assert_eq!(indexed.total, Some(count), "{filter}");
        assert_eq!(indexed.total, direct.total, "{filter}");
        assert_eq!(
            indexed.records.first().map(|r| r.id.as_str()),
            first_id,
            "{filter}"
        );
        assert_eq!(
            indexed.records.first().map(|r| r.id.as_str()),
            direct.records.first().map(|r| r.id.as_str()),
            "{filter}"
        );
        assert_eq!(indexed.metrics.grep_scans, 0, "{filter}");
        assert!(
            indexed
                .query_plan
                .iter()
                .any(|step| step.contains("exact scalar")),
            "{filter}"
        );
    }
}

#[test]
fn null_and_inequality_still_detect_scalar_array_errors() {
    let fixture = fixture();
    fixture.write("data/books/f.json", &json!({"score":[2,3]}));
    let mut repository = fixture.repo(CacheMode::Memory);

    for filter in ["score = null", "score != 2"] {
        let error = repository.query_json(&query(filter)).unwrap_err();
        assert!(
            error.to_string().contains("multiple values"),
            "{filter}: {error}"
        );
    }
}
