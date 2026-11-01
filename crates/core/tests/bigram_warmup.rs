mod support;

use objekte_core::{CacheMode, JsonQuery, Repository, ViewRequest};
use serde_json::json;
use std::{
    fs::{self, File, FileTimes},
    time::{Duration, UNIX_EPOCH},
};
use support::Fixture;

#[test]
fn saved_secondary_indexes_restore_typed_queries_and_ordered_views() {
    let f = Fixture::new();
    f.repo(CacheMode::Memory)
        .save_bigram_cache("books")
        .unwrap();
    let mut restored = f.repo(CacheMode::Memory);
    restored.warm_bigram_index("books").unwrap();
    assert_eq!(restored.metrics().bigram_snapshot_loads, 1);
    assert_eq!(restored.metrics().secondary_snapshot_loads, 1);

    let query = JsonQuery {
        filter: "score >= 2".into(),
        collections: Some(vec!["books".into()]),
        count: true,
        ..Default::default()
    };
    let expected = f.repo(CacheMode::Disabled).query_json(&query).unwrap();
    let actual = restored.query_json(&query).unwrap();
    assert_eq!(actual.total, expected.total);
    assert_eq!(
        actual
            .records
            .iter()
            .map(|record| &record.id)
            .collect::<Vec<_>>(),
        expected
            .records
            .iter()
            .map(|record| &record.id)
            .collect::<Vec<_>>()
    );

    let request = ViewRequest {
        count: true,
        ..Default::default()
    };
    let expected = f
        .repo(CacheMode::Disabled)
        .open_view("books", &request)
        .unwrap();
    let actual = restored.open_view("books", &request).unwrap();
    assert_eq!(actual.rows, expected.rows);
    assert_eq!(actual.total, expected.total);
}

#[test]
fn schema_change_rebuilds_only_secondary_cache_once() {
    let f = Fixture::new();
    f.repo(CacheMode::Memory)
        .save_bigram_cache("books")
        .unwrap();
    f.edit(".editor/books/schema.json", |schema| {
        schema["properties"]["title"]["index"] = json!(true);
    });
    let mut rebuilt = f.repo(CacheMode::Memory);
    rebuilt.warm_bigram_index("books").unwrap();
    assert_eq!(rebuilt.metrics().bigram_snapshot_loads, 1);
    assert_eq!(rebuilt.metrics().secondary_snapshot_loads, 0);

    let mut restored = f.repo(CacheMode::Memory);
    restored.warm_bigram_index("books").unwrap();
    assert_eq!(restored.metrics().secondary_snapshot_loads, 1);
    let query = JsonQuery {
        filter: "title = 'Atlas'".into(),
        collections: Some(vec!["books".into()]),
        count: true,
        ..Default::default()
    };
    let expected = f.repo(CacheMode::Disabled).query_json(&query).unwrap();
    assert_eq!(restored.query_json(&query).unwrap().total, expected.total);
}

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
    for (id, seconds) in [("a", 200), ("z", 300)] {
        let path = f.root.join(format!("data/books/{id}.json"));
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(seconds)))
            .unwrap();
    }
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
        ["z", "a"]
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
