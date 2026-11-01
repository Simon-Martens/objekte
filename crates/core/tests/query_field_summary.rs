//! Collection-level key rejection and the per-file raw-key fallback.

use objekte_core::{JsonQuery, Repository};
use std::{fs, path::Path};

fn write(root: &Path, collection: &str, id: &str, json: &str) {
    let directory = root.join("data").join(collection);
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join(format!("{id}.json")), json).unwrap();
}

fn query(repo: &mut Repository, filter: &str) -> objekte_core::JsonQueryResult {
    repo.query_json(&JsonQuery {
        filter: filter.into(),
        count: true,
        limit: 100,
        ..Default::default()
    })
    .unwrap()
}

#[test]
fn absent_collection_keys_are_skipped_but_escaped_and_refreshed_keys_match() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for id in 0..100 {
        write(root, "absent", &format!("{id:03}"), r#"{"other":true}"#);
    }
    write(root, "plain", "a", r#"{"flag":true}"#);
    write(root, "escaped", "a", r#"{"fl\u0061g":true}"#);

    let mut repo = Repository::open(root).unwrap();
    repo.warm_all_bigram_indexes().unwrap();
    let hits = query(&mut repo, "flag = true");
    assert_eq!(hits.total, Some(2));
    assert_eq!(hits.metrics.grep_scans, 2);

    // Missing and OR queries still include collections without that key.
    assert_eq!(query(&mut repo, "missing(flag)").total, Some(100));
    assert_eq!(
        query(&mut repo, "flag = true || other = true").total,
        Some(102)
    );

    write(root, "absent", "new", r#"{"flag":true}"#);
    repo.refresh("absent").unwrap();
    assert_eq!(query(&mut repo, "flag = true").total, Some(3));
    repo.rebuild_bigram_index("absent").unwrap();
    let rebuilt = query(&mut repo, "flag = true");
    assert_eq!(rebuilt.total, Some(3));
    assert_eq!(rebuilt.metrics.grep_scans, 103);
    assert!(rebuilt.metrics.parses <= 6, "{:#?}", rebuilt.metrics);
}

#[test]
fn array_cardinality_errors_are_not_hidden_by_key_rejection() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "first", "a", r#"{"flag":true}"#);
    write(root, "second", "a", r#"{"flag":[true,false]}"#);
    let mut repo = Repository::open(root).unwrap();
    let error = repo
        .query_json(&JsonQuery {
            filter: "flag = true".into(),
            count: true,
            ..Default::default()
        })
        .unwrap_err();
    assert!(error.to_string().contains("multiple values"), "{error}");
}
