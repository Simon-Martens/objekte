//! Covered scalar counts and necessary top-level presence filtering.

mod support;

use objekte_core::{CacheMode, JsonQuery};
use serde_json::json;
use std::fs;
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
    repository.refresh("books").unwrap();
    let present = repository.query_json(&query("exists(score)")).unwrap();
    assert_eq!(present.total, Some(3));
    assert_eq!(
        present
            .records
            .iter()
            .map(|record| record.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "c", "f"]
    );
    assert_eq!(present.metrics.grep_scans, 3);
}

#[test]
fn covered_count_checks_metadata_of_skipped_matches() {
    let fixture = fixture();
    let mut repository = fixture.repo(CacheMode::Memory);
    repository.query_json(&query("exists(score)")).unwrap();
    fixture.write(
        "data/books/a.json",
        &json!({"score":222,"title":"keep","payload":null}),
    );
    let stale = repository.query_json(&JsonQuery {
        offset: 1,
        limit: 1,
        ..query("exists(score)")
    });
    assert!(stale.unwrap_err().to_string().contains("changed"));
}
