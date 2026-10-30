mod support;

use objekte_core::{CacheMode, JsonQuery, Repository};
use serde_json::json;
use std::fs;
use support::Fixture;

#[test]
fn warm_one_then_all_builds_only_missing_collection_indexes() {
    let f = Fixture::new();
    let mut repo = f.repo(CacheMode::Memory);
    let first = repo.warm_bigram_index("books").unwrap();
    assert!(first.bigram_open_nanos > 0);
    assert_eq!(
        repo.warm_bigram_index("books").unwrap().bigram_open_nanos,
        0
    );

    let status = repo.cache_status().unwrap();
    assert_eq!(
        status
            .collections
            .iter()
            .find(|c| c.collection == "books")
            .unwrap()
            .records,
        7
    );
    assert_eq!(
        status
            .collections
            .iter()
            .find(|c| c.collection == "people")
            .unwrap()
            .records,
        0
    );
    assert_eq!(
        status
            .collections
            .iter()
            .find(|c| c.collection == "spare")
            .unwrap()
            .records,
        0
    );

    let warmed = repo.warm_all_bigram_indexes().unwrap();
    assert_eq!(
        warmed.keys().map(String::as_str).collect::<Vec<_>>(),
        ["books", "people", "spare"]
    );
    assert_eq!(warmed["books"].bigram_open_nanos, 0);
    assert!(warmed["people"].bigram_open_nanos > 0);
    assert!(warmed["spare"].bigram_open_nanos > 0);
    assert!(!f.data.exists());
}

#[test]
fn explicit_rebuild_refreshes_membership_and_retains_revisions() {
    let f = Fixture::new();
    let mut repo: Repository = f.repo(CacheMode::Memory);
    repo.warm_bigram_index("books").unwrap();
    let before = repo
        .record_revision("books", "a")
        .unwrap()
        .unwrap()
        .revision;

    f.edit("data/books/a.json", |v| v["title"] = json!("Rebuilt title"));
    let mut added = f.read("data/books/a.json");
    added["title"] = json!("Rebuilt second title");
    f.write("data/books/z.json", &added);
    let rebuilt = repo.rebuild_bigram_index("books").unwrap();
    assert!(rebuilt.bigram_rebuild_nanos > 0);
    assert_eq!(
        repo.record_revision("books", "a")
            .unwrap()
            .unwrap()
            .revision,
        before + 1
    );

    let query = JsonQuery {
        filter: "title ~ 'Rebuilt'".into(),
        collections: Some(vec!["books".into()]),
        count: true,
        ..Default::default()
    };
    let result = repo.query_json(&query).unwrap();
    assert_eq!(result.total, Some(2));
    assert_eq!(
        result
            .records
            .iter()
            .map(|r| r.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "z"]
    );

    fs::remove_file(f.root.join("data/books/z.json")).unwrap();
    let rebuilt = repo.rebuild_all_bigram_indexes().unwrap();
    assert!(rebuilt["books"].bigram_rebuild_nanos > 0);
    assert!(rebuilt["people"].bigram_open_nanos > 0);
    assert!(rebuilt["spare"].bigram_open_nanos > 0);
    assert_eq!(repo.query_json(&query).unwrap().total, Some(1));
    assert!(!repo.record_revision("books", "z").unwrap().unwrap().present);
    assert_eq!(
        repo.record_revision("books", "a")
            .unwrap()
            .unwrap()
            .revision,
        before + 1
    );
}
