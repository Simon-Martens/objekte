//! Public regressions for native/selected candidate translation and errors.

use super::review_stage1::JsonAliasProvider;
use objekte_core::{JsonQuery, JsonQueryBackend, Options, Repository};
use std::fs;

pub(super) fn mixed_repo(root: &std::path::Path, directory: &std::path::Path) -> Repository {
    Repository::builder(root)
        .options(Options {
            query_cache_bytes: 0,
            ..Default::default()
        })
        .register_source(JsonAliasProvider::new(directory.to_path_buf()))
        .unwrap()
        .open()
        .unwrap()
}

fn ids(result: &objekte_core::JsonQueryResult) -> Vec<(String, String)> {
    result
        .records
        .iter()
        .map(|record| (record.collection.clone(), record.id.clone()))
        .collect()
}

#[test]
fn native_candidates_remap_after_compacting_live_slots() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("data/mixed");
    fs::create_dir_all(directory.join(".editor")).unwrap();
    fs::write(
        directory.join(".editor/schema.json"),
        r#"{"type":"object","properties":{"year":{"type":"integer","index":true}}}"#,
    )
    .unwrap();
    for index in 0..32 {
        let year = if index == 10 { 1848 } else { 1850 };
        fs::write(
            directory.join(format!("n{index:03}.json")),
            format!(r#"{{"year":{year}}}"#),
        )
        .unwrap();
    }
    fs::write(directory.join("z.md"), r#"{"year":1848}"#).unwrap();
    let mut indexed = mixed_repo(temp.path(), &directory);
    let query = JsonQuery {
        filter: "year = 1848".into(),
        collections: Some(vec!["mixed".into()]),
        count: true,
        limit: 40,
        ..Default::default()
    };
    // Build the concrete native picker, then add a record that appends a
    // native slot while sorting before the existing IDs.
    assert_eq!(indexed.query_json(&query).unwrap().total, Some(2));
    fs::write(directory.join("a.json"), r#"{"year":1850}"#).unwrap();
    indexed.refresh("mixed").unwrap();
    let before = indexed.query_json(&query).unwrap();
    assert!(
        before
            .query_plan
            .iter()
            .any(|step| step.contains("native-posting"))
    );
    let mut before_ids = ids(&before);
    before_ids.sort();
    assert_eq!(
        before_ids,
        vec![
            ("mixed".into(), "n010".into()),
            ("mixed".into(), "z".into())
        ]
    );
    indexed.save_bigram_cache("mixed").unwrap();
    let after = indexed.query_json(&query).unwrap();
    let mut direct = mixed_repo(temp.path(), &directory);
    let oracle = direct
        .query_json(&JsonQuery {
            backend: JsonQueryBackend::Direct,
            ..query
        })
        .unwrap();
    assert_eq!(after.total, oracle.total);
    assert_eq!(after.truncated, oracle.truncated);
    assert_eq!(ids(&after), ids(&oracle));

    // A deletion leaves a hole. Explicit reindex compacts it again while the
    // selected source inventory is reused.
    fs::remove_file(directory.join("n002.json")).unwrap();
    indexed.refresh("mixed").unwrap();
    assert_eq!(
        indexed
            .query_json(&JsonQuery {
                filter: "year = 1848".into(),
                collections: Some(vec!["mixed".into()]),
                count: true,
                limit: 40,
                ..Default::default()
            })
            .unwrap()
            .total,
        Some(2)
    );
    indexed.reindex_bigram_cache("mixed").unwrap();
    let after_reindex = indexed
        .query_json(&JsonQuery {
            filter: "year = 1848".into(),
            collections: Some(vec!["mixed".into()]),
            count: true,
            limit: 40,
            ..Default::default()
        })
        .unwrap();
    let mut reopened = mixed_repo(temp.path(), &directory);
    let restored = reopened
        .query_json(&JsonQuery {
            filter: "year = 1848".into(),
            collections: Some(vec!["mixed".into()]),
            count: true,
            limit: 40,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(ids(&after_reindex), ids(&restored));
    assert_eq!(after_reindex.total, restored.total);
}
