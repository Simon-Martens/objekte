//! Native text and full-text postings remain useful in a selected scope.

use super::review_stage1::JsonAliasProvider;
use objekte_core::{
    CacheMode, Direction, JsonQuery, JsonQueryBackend, Options, Repository, Sort, TextMode,
    TextQuery,
};
use std::fs;

fn fixture(full_text: bool) -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let mixed = temp.path().join("data/mixed");
    let native = temp.path().join("data/native");
    for root in [&mixed, &native] {
        fs::create_dir_all(root.join(".editor")).unwrap();
        let schema = if full_text {
            r#"{"type":"object","properties":{"body":{"type":"string","x-full-text":"plain"},"year":{"type":"integer","index":true}}}"#
        } else {
            r#"{"type":"object","properties":{"body":{"type":"string"},"year":{"type":"integer","index":true}}}"#
        };
        fs::write(root.join(".editor/schema.json"), schema).unwrap();
        for index in 0..96 {
            let body = if index == 95 {
                "raretoken"
            } else {
                "ordinary filler"
            };
            let year = if index == 95 { 1848 } else { 1850 };
            fs::write(
                root.join(format!("n{index:03}.json")),
                format!(r#"{{"body":"{body}","year":{year}}}"#),
            )
            .unwrap();
        }
    }
    let custom = if full_text {
        r#"{"body":"raretoken custom","year":1848}"#
    } else {
        r#"{"body":"ordinary","note":"raretoken custom","year":1848}"#
    };
    fs::write(mixed.join("z.md"), custom).unwrap();
    (temp, mixed)
}

fn indexed(root: &std::path::Path, mixed: &std::path::Path) -> Repository {
    Repository::builder(root)
        .options(Options {
            query_cache_bytes: 0,
            ..Default::default()
        })
        .register_source(JsonAliasProvider::new(mixed.to_path_buf()))
        .unwrap()
        .open()
        .unwrap()
}

fn direct(root: &std::path::Path, mixed: &std::path::Path) -> Repository {
    Repository::builder(root)
        .options(Options {
            cache: CacheMode::Disabled,
            query_cache_bytes: 0,
            ..Default::default()
        })
        .register_source(JsonAliasProvider::new(mixed.to_path_buf()))
        .unwrap()
        .open()
        .unwrap()
}

#[test]
fn native_text_remains_selective_in_mixed_scope() {
    let (temp, mixed) = fixture(false);
    let mut accelerated = indexed(temp.path(), &mixed);
    accelerated.warm_bigram_index("mixed").unwrap();
    accelerated.warm_bigram_index("native").unwrap();
    let mut oracle = direct(temp.path(), &mixed);
    let scope = Some(vec!["mixed".into(), "native".into()]);
    for (pattern, mode, selective) in [
        ("raretoken", TextMode::Word, true),
        ("raretoken", TextMode::Regex, true),
        (".*", TextMode::Regex, false),
    ] {
        let request = TextQuery {
            pattern: pattern.into(),
            mode,
            collections: scope.clone(),
            count: true,
            limit: 250,
            ..Default::default()
        };
        let indexed = accelerated.search_text(&request).unwrap();
        let exact = oracle.search_text(&request).unwrap();
        assert_eq!(indexed.total, exact.total, "{pattern}");
        assert_eq!(indexed.truncated, exact.truncated, "{pattern}");
        assert_eq!(
            indexed
                .hits
                .iter()
                .map(|hit| (&hit.record.collection, &hit.record.id))
                .collect::<Vec<_>>(),
            exact
                .hits
                .iter()
                .map(|hit| (&hit.record.collection, &hit.record.id))
                .collect::<Vec<_>>(),
            "{pattern}"
        );
        if selective {
            assert!(
                indexed.metrics.selected_candidate_visits < 12,
                "{pattern}: native nonmatches should not be decoded: {:?}",
                indexed.metrics
            );
            assert!(
                indexed
                    .query_plan
                    .iter()
                    .any(|step| step.contains("native-text-seed"))
            );
        } else {
            assert!(
                indexed.metrics.selected_candidate_visits >= 193,
                "a regex without a necessary literal must scan its local domain"
            );
        }
    }
    let fields = TextQuery {
        pattern: "raretoken".into(),
        fields: Some(vec!["body".into(), "note".into()]),
        collections: scope,
        count: true,
        limit: 250,
        ..Default::default()
    };
    let indexed = accelerated.search_text(&fields).unwrap();
    let exact = oracle.search_text(&fields).unwrap();
    assert_eq!(indexed.total, exact.total);
    assert_eq!(
        indexed
            .hits
            .iter()
            .map(|hit| (&hit.record.collection, &hit.record.id))
            .collect::<Vec<_>>(),
        exact
            .hits
            .iter()
            .map(|hit| (&hit.record.collection, &hit.record.id))
            .collect::<Vec<_>>()
    );
    assert!(indexed.metrics.selected_candidate_visits < 12);
}

#[test]
fn native_json_full_text_remains_selective_in_mixed_scope() {
    let (temp, mixed) = fixture(true);
    let mut accelerated = indexed(temp.path(), &mixed);
    accelerated.warm_bigram_index("mixed").unwrap();
    accelerated.warm_bigram_index("native").unwrap();
    let mut oracle = direct(temp.path(), &mixed);
    let scope = Some(vec!["mixed".into(), "native".into()]);
    for (filter, selective) in [
        ("body:fts ~ 'raretoken'", true),
        ("body:fts ~ 'raretoken' && year = 1848", true),
        ("year >= 1800 && body:fts ~ 'raretoken'", true),
        ("body:fts ~ 'raretoken' || year = 1850", false),
    ] {
        let request = JsonQuery {
            filter: filter.into(),
            collections: scope.clone(),
            count: true,
            limit: 250,
            ..Default::default()
        };
        let indexed = accelerated.query_json(&request).unwrap();
        let exact = oracle
            .query_json(&JsonQuery {
                backend: JsonQueryBackend::Direct,
                ..request
            })
            .unwrap();
        assert_eq!(indexed.total, exact.total, "{filter}");
        assert_eq!(indexed.truncated, exact.truncated, "{filter}");
        assert_eq!(
            indexed
                .records
                .iter()
                .map(|record| (&record.collection, &record.id))
                .collect::<Vec<_>>(),
            exact
                .records
                .iter()
                .map(|record| (&record.collection, &record.id))
                .collect::<Vec<_>>(),
            "{filter}"
        );
        if selective {
            assert!(
                indexed.metrics.selected_candidate_visits < 12,
                "{filter}: native full-text posting should narrow work: {:?}",
                indexed.metrics
            );
        }
        if filter == "body:fts ~ 'raretoken'" || filter.starts_with("year >=") {
            assert!(
                indexed
                    .query_plan
                    .iter()
                    .any(|step| step.contains("native-full-text"))
            );
        }
    }
}

#[test]
fn markup_custom_fallback_does_not_use_json_raw_bigrams() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("data/mixed");
    fs::create_dir_all(directory.join(".editor")).unwrap();
    fs::write(
        directory.join(".editor/schema.json"),
        r#"{"type":"object","properties":{"body":{"type":"string","x-full-text":"markdown"}}}"#,
    )
    .unwrap();
    for index in 0..64 {
        let body = if index == 63 {
            "foo *bar* baz"
        } else {
            "ordinary filler"
        };
        fs::write(
            directory.join(format!("n{index:03}.json")),
            format!(r#"{{"body":"{body}"}}"#),
        )
        .unwrap();
    }
    fs::write(directory.join("z.md"), r#"{"body":"foo *bar* baz"}"#).unwrap();
    let mut accelerated = indexed(temp.path(), &directory);
    accelerated.warm_bigram_index("mixed").unwrap();
    let mut oracle = direct(temp.path(), &directory);
    let query = TextQuery {
        pattern: "foo bar baz".into(),
        collections: Some(vec!["mixed".into()]),
        count: true,
        limit: 70,
        ..Default::default()
    };
    let result = accelerated.search_text(&query).unwrap();
    let exact = oracle.search_text(&query).unwrap();
    assert_eq!(result.total, Some(2));
    assert_eq!(
        result
            .hits
            .iter()
            .map(|hit| &hit.record.id)
            .collect::<Vec<_>>(),
        exact
            .hits
            .iter()
            .map(|hit| &hit.record.id)
            .collect::<Vec<_>>()
    );
    assert!(result.metrics.selected_candidate_visits < 8);
}

#[test]
fn mixed_text_and_json_indexes_survive_native_compaction_and_reopen() {
    let (temp, mixed) = fixture(true);
    let mut repo = indexed(temp.path(), &mixed);
    repo.warm_bigram_index("mixed").unwrap();
    let text = TextQuery {
        pattern: "raretoken".into(),
        collections: Some(vec!["mixed".into()]),
        count: true,
        limit: 20,
        ..Default::default()
    };
    let json = JsonQuery {
        filter: "body:fts ~ 'raretoken'".into(),
        collections: Some(vec!["mixed".into()]),
        count: true,
        limit: 20,
        ..Default::default()
    };
    assert_eq!(repo.search_text(&text).unwrap().total, Some(2));
    assert_eq!(repo.query_json(&json).unwrap().total, Some(2));
    fs::write(mixed.join("a.json"), r#"{"body":"ordinary","year":1850}"#).unwrap();
    repo.refresh("mixed").unwrap();
    // These requests initialize native-to-selected translation against the
    // appended, noncompact native slot layout.
    assert_eq!(repo.search_text(&text).unwrap().total, Some(2));
    assert_eq!(repo.query_json(&json).unwrap().total, Some(2));
    repo.save_bigram_cache("mixed").unwrap();
    let mut oracle = direct(temp.path(), &mixed);
    let after_text = repo.search_text(&text).unwrap();
    let after_json = repo.query_json(&json).unwrap();
    let expected_text = oracle.search_text(&text).unwrap();
    let expected_json = oracle
        .query_json(&JsonQuery {
            backend: JsonQueryBackend::Direct,
            ..json.clone()
        })
        .unwrap();
    assert_eq!(
        after_text
            .hits
            .iter()
            .map(|hit| &hit.record.id)
            .collect::<Vec<_>>(),
        expected_text
            .hits
            .iter()
            .map(|hit| &hit.record.id)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        after_json
            .records
            .iter()
            .map(|record| &record.id)
            .collect::<Vec<_>>(),
        expected_json
            .records
            .iter()
            .map(|record| &record.id)
            .collect::<Vec<_>>()
    );
    assert!(after_text.metrics.selected_candidate_visits < 8);
    assert!(after_json.metrics.selected_candidate_visits < 8);
    drop(repo);
    let mut reopened = indexed(temp.path(), &mixed);
    let mut oracle = direct(temp.path(), &mixed);
    let text_reopened = reopened.search_text(&text).unwrap();
    let text_exact = oracle.search_text(&text).unwrap();
    assert_eq!(text_reopened.total, text_exact.total);
    assert_eq!(
        text_reopened
            .hits
            .iter()
            .map(|hit| &hit.record.id)
            .collect::<Vec<_>>(),
        text_exact
            .hits
            .iter()
            .map(|hit| &hit.record.id)
            .collect::<Vec<_>>()
    );
    let json_reopened = reopened.query_json(&json).unwrap();
    let json_exact = oracle
        .query_json(&JsonQuery {
            backend: JsonQueryBackend::Direct,
            ..json
        })
        .unwrap();
    assert_eq!(json_reopened.total, json_exact.total);
    assert_eq!(
        json_reopened
            .records
            .iter()
            .map(|record| &record.id)
            .collect::<Vec<_>>(),
        json_exact
            .records
            .iter()
            .map(|record| &record.id)
            .collect::<Vec<_>>()
    );
    assert!(json_reopened.metrics.selected_candidate_visits < 8);
}

#[test]
fn cached_sorted_page_crosses_native_custom_and_collection_boundaries() {
    let (temp, mixed) = fixture(false);
    let mut cached = Repository::builder(temp.path())
        .register_source(JsonAliasProvider::new(mixed.clone()))
        .unwrap()
        .open()
        .unwrap();
    let mut oracle = direct(temp.path(), &mixed);
    let request = JsonQuery {
        filter: "year >= 1800".into(),
        collections: Some(vec!["mixed".into(), "native".into()]),
        sort: vec![Sort {
            field: "@id".into(),
            direction: Direction::Asc,
        }],
        count: true,
        limit: 3,
        ..Default::default()
    };
    let first = cached.query_json(&request).unwrap();
    assert_eq!(first.total, Some(193));
    for offset in [95, 96, 190] {
        let page = JsonQuery {
            offset,
            ..request.clone()
        };
        let result = cached.query_json(&page).unwrap();
        let exact = oracle
            .query_json(&JsonQuery {
                backend: JsonQueryBackend::Direct,
                ..page
            })
            .unwrap();
        assert_eq!(result.total, exact.total);
        assert_eq!(result.truncated, exact.truncated);
        assert_eq!(
            result
                .records
                .iter()
                .map(|record| (&record.collection, &record.id))
                .collect::<Vec<_>>(),
            exact
                .records
                .iter()
                .map(|record| (&record.collection, &record.id))
                .collect::<Vec<_>>()
        );
        assert!(result.metrics.ordered_result_hits > 0);
        assert_eq!(result.metrics.selected_candidate_visits, 0);
        assert_eq!(
            result.metrics.selected_descriptor_visits,
            result.records.len() as u64
        );
        assert_eq!(result.metrics.source_reads, result.records.len() as u64);
    }
}
