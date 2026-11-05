//! Regressions for Direct backend eligibility and native candidate translation.

use super::review_stage1::JsonAliasProvider;
use objekte_core::{CacheMode, Direction, JsonQuery, JsonQueryBackend, Options, Repository, Sort};
use std::{fs, path::Path};

fn fixture(count: usize) -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let mixed = temp.path().join("data/mixed");
    fs::create_dir_all(mixed.join(".editor")).unwrap();
    fs::write(
        mixed.join(".editor/schema.json"),
        r#"{"type":"object","properties":{"body":{"type":"string","index":true,"x-full-text":"plain"},"year":{"type":"integer","index":true}}}"#,
    )
    .unwrap();
    for index in 0..count {
        let year = if index % 3 == 2 { 1850 } else { 1848 };
        let body = if index == 0 || index == count / 2 || index + 1 == count {
            "common raretokenXYZ"
        } else {
            "common filler"
        };
        fs::write(
            mixed.join(format!("n{index:05}.json")),
            format!(r#"{{"year":{year},"body":"{body}"}}"#),
        )
        .unwrap();
    }
    fs::write(
        mixed.join("z.md"),
        r#"{"year":1848,"body":"common raretokenXYZ custom"}"#,
    )
    .unwrap();
    (temp, mixed)
}

fn open(root: &Path, mixed: &Path, cache: CacheMode) -> Repository {
    Repository::builder(root)
        .options(Options {
            cache,
            query_cache_bytes: 0,
            ..Default::default()
        })
        .register_source(JsonAliasProvider::new(mixed.to_path_buf()))
        .unwrap()
        .open()
        .unwrap()
}

fn query(filter: &str, backend: JsonQueryBackend, offset: usize, limit: usize) -> JsonQuery {
    JsonQuery {
        filter: filter.into(),
        backend,
        collections: Some(vec!["mixed".into()]),
        sort: vec![Sort {
            field: "@id".into(),
            direction: Direction::Asc,
        }],
        offset,
        limit,
        count: true,
        ..Default::default()
    }
}

fn ids(result: &objekte_core::JsonQueryResult) -> Vec<(&str, &str)> {
    result
        .records
        .iter()
        .map(|record| (record.collection.as_str(), record.id.as_str()))
        .collect()
}

#[test]
fn warmed_selected_scalar_fallback_does_not_prune_direct_fts_query() {
    let (temp, mixed) = fixture(96);
    let mut indexed = open(temp.path(), &mixed, CacheMode::Memory);
    indexed.warm_bigram_index("mixed").unwrap();
    let mut oracle = open(temp.path(), &mixed, CacheMode::Disabled);
    let direct_request = query(
        "body:fts ~ 'common' && year = 1848",
        JsonQueryBackend::Direct,
        0,
        100,
    );
    let direct = indexed.query_json(&direct_request).unwrap();
    let expected = oracle.query_json(&direct_request).unwrap();
    assert_eq!(direct.total, expected.total);
    assert_eq!(direct.truncated, expected.truncated);
    assert_eq!(ids(&direct), ids(&expected));
    assert_eq!(direct.metrics.selected_candidate_visits, 97);
    assert_eq!(direct.metrics.selected_candidate_bitsets, 0);
    assert_eq!(direct.metrics.native_translation_candidates, 0);
    for backend in [JsonQueryBackend::Auto, JsonQueryBackend::Bigram] {
        let accelerated = indexed
            .query_json(&JsonQuery {
                backend,
                ..direct_request.clone()
            })
            .unwrap();
        assert_eq!(accelerated.total, expected.total);
        assert_eq!(accelerated.truncated, expected.truncated);
        assert_eq!(ids(&accelerated), ids(&expected));
        assert!(accelerated.metrics.selected_candidate_visits < 97);
        assert!(
            accelerated
                .query_plan
                .iter()
                .any(|step| step.contains("selected-scalar-posting"))
        );
    }

    fs::write(
        mixed.join("n00002.json"),
        r#"{"year":[1850,1851],"body":"common filler"}"#,
    )
    .unwrap();
    indexed.refresh("mixed").unwrap();
    oracle.refresh("mixed").unwrap();
    let invalid = indexed.query_json(&direct_request).unwrap_err().to_string();
    let expected_error = oracle.query_json(&direct_request).unwrap_err().to_string();
    assert_eq!(invalid, expected_error);
}

#[test]
fn dense_fts_intersection_translates_members_without_native_order_walk() {
    for count in [128, 1024] {
        let (temp, mixed) = fixture(count);
        let mut indexed = open(temp.path(), &mixed, CacheMode::Memory);
        indexed.warm_bigram_index("mixed").unwrap();
        let mut oracle = open(temp.path(), &mixed, CacheMode::Disabled);
        let request = query("body:fts ~ 'raretokenXYZ'", JsonQueryBackend::Auto, 0, 2);
        // First query initializes the generation-bound native slot map.
        indexed.query_json(&request).unwrap();
        for offset in [0, 2, 4] {
            let page = JsonQuery {
                offset,
                ..request.clone()
            };
            let result = indexed.query_json(&page).unwrap();
            let exact = oracle
                .query_json(&JsonQuery {
                    backend: JsonQueryBackend::Direct,
                    ..page
                })
                .unwrap();
            assert_eq!(result.total, exact.total);
            assert_eq!(result.truncated, exact.truncated);
            assert_eq!(ids(&result), ids(&exact));
            assert_eq!(result.metrics.native_translation_order_entries, 0);
            assert!(result.metrics.native_translation_bitmap_words <= count.div_ceil(64) as u64);
            assert_eq!(result.metrics.native_translation_candidates, 3);
            assert_eq!(result.metrics.selected_candidate_visits, 4);
        }

        let empty = query("body:fts ~ 'nevertermABC'", JsonQueryBackend::Auto, 0, 2);
        let result = indexed.query_json(&empty).unwrap();
        let exact = oracle
            .query_json(&JsonQuery {
                backend: JsonQueryBackend::Direct,
                ..empty
            })
            .unwrap();
        assert_eq!(result.total, exact.total);
        assert_eq!(ids(&result), ids(&exact));
        assert_eq!(result.metrics.native_translation_order_entries, 0);
        assert_eq!(result.metrics.native_translation_candidates, 0);

        if count == 128 {
            fs::write(
                mixed.join("a.json"),
                r#"{"year":1850,"body":"common filler"}"#,
            )
            .unwrap();
            indexed.refresh("mixed").unwrap();
            indexed.query_json(&request).unwrap();
            indexed.save_bigram_cache("mixed").unwrap();
            let after = indexed.query_json(&request).unwrap();
            let exact = oracle
                .query_json(&JsonQuery {
                    backend: JsonQueryBackend::Direct,
                    ..request.clone()
                })
                .unwrap();
            assert_eq!(after.total, exact.total);
            assert_eq!(ids(&after), ids(&exact));
            assert_eq!(after.metrics.native_translation_order_entries, 0);
            assert_eq!(after.metrics.native_translation_candidates, 3);
        }
    }
}
