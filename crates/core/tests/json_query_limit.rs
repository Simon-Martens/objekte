//! Early page termination for the parallel in-memory candidate scanner.

mod support;

use objekte_core::{CacheMode, JsonQuery};
use serde_json::json;
use std::fs;
use support::Fixture;


#[test]
fn uncounted_page_stops_after_a_parallel_batch_but_count_scans_all() {
    let fixture = Fixture::new();
    let books = fixture.root.join("data/books");
    for entry in fs::read_dir(&books).unwrap() {
        fs::remove_file(entry.unwrap().path()).unwrap();
    }
    for number in 0..800 {
        fs::write(
            books.join(format!("r{number:03}.json")),
            serde_json::to_vec(&json!({"title":"hit","payload":null})).unwrap(),
        )
        .unwrap();
    }

    let mut repository = fixture.repo(CacheMode::Memory);
    let request = JsonQuery {
        filter: "title = 'hit'".into(),
        collections: Some(vec!["books".into()]),
        offset: 5,
        limit: 3,
        count: false,
        ..Default::default()
    };
    let page = repository.query_json(&request).unwrap();
    assert_eq!(page.total, None);
    assert_eq!(
        page.records
            .iter()
            .map(|record| record.id.as_str())
            .collect::<Vec<_>>(),
        ["r005", "r006", "r007"]
    );
    assert!(page.metrics.grep_scans >= 8);
    assert!(page.metrics.grep_scans < 800);

    let later_page = repository
        .query_json(&JsonQuery {
            offset: 510,
            ..request.clone()
        })
        .unwrap();
    assert_eq!(later_page.total, None);
    assert_eq!(
        later_page
            .records
            .iter()
            .map(|record| record.id.as_str())
            .collect::<Vec<_>>(),
        ["r510", "r511", "r512"]
    );

    let counted = repository
        .query_json(&JsonQuery {
            count: true,
            ..request.clone()
        })
        .unwrap();
    assert_eq!(counted.total, Some(800));
    assert_eq!(counted.metrics.grep_scans, 800);

    // A schema-invalid array later in ID order must still raise the global
    // scalar-cardinality error, even for an uncounted first page.
    fs::write(
        books.join("r800.json"),
        serde_json::to_vec(&json!({"title":["hit","other"],"payload":null})).unwrap(),
    )
    .unwrap();
    repository.refresh("books").unwrap();
    assert!(
        repository
            .query_json(&request)
            .unwrap_err()
            .to_string()
            .contains("multiple values")
    );
}
