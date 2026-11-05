//! Candidate coverage across native and registered source partitions.

use super::review_stage1::JsonAliasProvider;
use objekte_core::{
    CacheMode, Direction, JsonQuery, JsonQueryBackend, Options, Repository, Sort, TextQuery,
};
use std::fs;

#[test]
fn rare_native_posting_survives_one_custom_record_and_another_collection() {
    let temp = tempfile::tempdir().unwrap();
    let mixed = temp.path().join("data/mixed");
    let native = temp.path().join("data/native");
    fs::create_dir_all(mixed.join(".editor")).unwrap();
    fs::create_dir_all(native.join(".editor")).unwrap();
    let schema = r#"{"type":"object","properties":{"year":{"type":"integer","index":true},"body":{"type":"string"}}}"#;
    fs::write(mixed.join(".editor/schema.json"), schema).unwrap();
    fs::write(native.join(".editor/schema.json"), schema).unwrap();
    for (root, count) in [(&mixed, 96), (&native, 96)] {
        for index in 0..count {
            let year = if index == 95 { 1848 } else { 1850 };
            fs::write(
                root.join(format!("n{index:03}.json")),
                format!(r#"{{"year":{year},"body":"common target"}}"#),
            )
            .unwrap();
        }
    }
    fs::write(
        mixed.join("z.md"),
        r#"{"year":1848,"body":"custom target"}"#,
    )
    .unwrap();
    let mut repo = Repository::builder(temp.path())
        .register_source(JsonAliasProvider::new(mixed.clone()))
        .unwrap()
        .open()
        .unwrap();
    let request = JsonQuery {
        filter: "year = 1848 && body ~ 'target'".into(),
        collections: Some(vec!["mixed".into(), "native".into()]),
        sort: vec![Sort {
            field: "@id".into(),
            direction: Direction::Asc,
        }],
        limit: 20,
        count: true,
        ..Default::default()
    };
    let accelerated = repo.query_json(&request).unwrap();
    let mut exact = Repository::builder(temp.path())
        .options(Options {
            query_cache_bytes: 0,
            ..Default::default()
        })
        .register_source(JsonAliasProvider::new(mixed))
        .unwrap()
        .open()
        .unwrap();
    let direct = exact
        .query_json(&JsonQuery {
            backend: JsonQueryBackend::Direct,
            ..request.clone()
        })
        .unwrap();
    assert_eq!(accelerated.total, Some(3));
    assert_eq!(
        accelerated
            .records
            .iter()
            .map(|r| (&r.collection, &r.id))
            .collect::<Vec<_>>(),
        direct
            .records
            .iter()
            .map(|r| (&r.collection, &r.id))
            .collect::<Vec<_>>()
    );
    assert!(
        accelerated
            .query_plan
            .iter()
            .any(|step| step.contains("native-posting"))
    );
    assert!(
        accelerated.metrics.source_reads < 20,
        "selective query decoded too many sources"
    );
    assert!(direct.metrics.source_reads >= 193);

    // An unsupported OR branch broadens the necessary domain. The native
    // posting may not silently discard records matching that branch.
    let disjunction = JsonQuery {
        filter: "year = 1848 || body = 'common target'".into(),
        limit: 250,
        ..request.clone()
    };
    let indexed_or = repo.query_json(&disjunction).unwrap();
    let direct_or = exact
        .query_json(&JsonQuery {
            backend: JsonQueryBackend::Direct,
            ..disjunction
        })
        .unwrap();
    assert_eq!(indexed_or.total, direct_or.total);
    assert_eq!(
        indexed_or
            .records
            .iter()
            .map(|r| (&r.collection, &r.id))
            .collect::<Vec<_>>(),
        direct_or
            .records
            .iter()
            .map(|r| (&r.collection, &r.id))
            .collect::<Vec<_>>()
    );

    let all = JsonQuery {
        filter: "exists(year)".into(),
        sort: vec![Sort {
            field: "@id".into(),
            direction: Direction::Asc,
        }],
        collections: Some(vec!["mixed".into(), "native".into()]),
        count: true,
        limit: 3,
        ..Default::default()
    };
    repo.query_json(&all).unwrap();
    let middle = repo.query_json(&JsonQuery { offset: 150, ..all }).unwrap();
    assert_eq!(middle.records.len(), 3);
    assert_eq!(middle.metrics.selected_candidate_visits, 0);
    assert_eq!(middle.metrics.selected_descriptor_visits, 3);
    assert_eq!(middle.metrics.source_reads, 3);
    assert!(
        middle
            .query_plan
            .iter()
            .any(|step| step.contains("ordered-cache"))
    );

    repo.warm_bigram_index("mixed").unwrap();
    let field_query = JsonQuery {
        filter: "year = 1848".into(),
        collections: Some(vec!["mixed".into()]),
        limit: 20,
        count: true,
        ..Default::default()
    };
    let indexed_custom = repo.query_json(&field_query).unwrap();
    assert_eq!(indexed_custom.total, Some(2));
    assert!(
        indexed_custom
            .query_plan
            .iter()
            .any(|step| step.contains("selected-scalar-posting"))
    );
    assert!(indexed_custom.metrics.source_reads < 20);
    repo.save_bigram_cache("mixed").unwrap();
    drop(repo);
    let mut reopened = Repository::builder(temp.path())
        .register_source(JsonAliasProvider::new(temp.path().join("data/mixed")))
        .unwrap()
        .open()
        .unwrap();
    let restored = reopened.warm_bigram_index("mixed").unwrap();
    assert_eq!(
        restored.source_reads, 0,
        "compatible selected field posting should restore"
    );
    let after_reopen = reopened.query_json(&field_query).unwrap();
    assert_eq!(after_reopen.total, Some(2));
    assert!(
        after_reopen
            .query_plan
            .iter()
            .any(|step| step.contains("selected-scalar-posting"))
    );
}

#[test]
fn text_and_scalar_terms_choose_opposite_selective_domains() {
    let temp = tempfile::tempdir().unwrap();
    let mixed = temp.path().join("data/mixed");
    fs::create_dir_all(mixed.join(".editor")).unwrap();
    fs::write(mixed.join(".editor/schema.json"), r#"{"type":"object","properties":{"year":{"type":"integer","index":true},"body":{"type":"string","x-full-text":"plain"},"other":{"type":"string","x-full-text":"plain"}}}"#).unwrap();
    for index in 0..96 {
        let year = if index == 95 { 1848 } else { 1850 };
        let other = if index == 94 { "needle" } else { "quiet" };
        fs::write(
            mixed.join(format!("n{index:03}.json")),
            format!(r#"{{"year":{year},"body":"common text","other":"{other}"}}"#),
        )
        .unwrap();
    }
    fs::write(
        mixed.join("z.md"),
        r#"{"year":1848,"body":"common text","other":"quiet"}"#,
    )
    .unwrap();
    let mut repo = Repository::builder(temp.path())
        .register_source(JsonAliasProvider::new(mixed.clone()))
        .unwrap()
        .open()
        .unwrap();
    repo.warm_bigram_index("mixed").unwrap();
    let query = |pattern: &str, filter: &str| TextQuery {
        pattern: pattern.into(),
        filter: Some(filter.into()),
        fields: Some(vec!["body".into(), "other".into()]),
        collections: Some(vec!["mixed".into()]),
        count: true,
        limit: 20,
        ..Default::default()
    };
    let rare_scalar = repo.search_text(&query("common", "year = 1848")).unwrap();
    let unfiltered = repo
        .search_text(&TextQuery {
            filter: None,
            ..query("needle", "year = 1850")
        })
        .unwrap();
    assert!(unfiltered.metrics.source_reads < 20);
    let rare_text = repo.search_text(&query("needle", "year = 1850")).unwrap();
    assert_eq!(
        rare_scalar.total,
        Some(2),
        "reads={} plan={:?}",
        rare_scalar.metrics.source_reads,
        rare_scalar.query_plan
    );
    assert_eq!(rare_text.total, Some(1));
    assert!(rare_scalar.metrics.source_reads < 20);
    assert!(
        rare_text.metrics.source_reads < 20,
        "reads={} plan={:?}",
        rare_text.metrics.source_reads,
        rare_text.query_plan
    );
    assert!(
        rare_scalar
            .query_plan
            .iter()
            .any(|step| step.contains("index-candidates"))
    );
    assert!(
        rare_text
            .query_plan
            .iter()
            .any(|step| step.contains("index-candidates"))
    );
    assert!(
        rare_scalar
            .query_plan
            .iter()
            .any(|step| step.contains("scalar-seed"))
    );
    assert!(
        rare_text
            .query_plan
            .iter()
            .any(|step| step.contains("text-seed"))
    );
    let mut exact = Repository::builder(temp.path())
        .options(Options {
            cache: CacheMode::Disabled,
            query_cache_bytes: 0,
            ..Default::default()
        })
        .register_source(JsonAliasProvider::new(mixed))
        .unwrap()
        .open()
        .unwrap();
    for (pattern, filter, indexed) in [
        ("common", "year = 1848", rare_scalar),
        ("needle", "year = 1850", rare_text),
    ] {
        let direct = exact.search_text(&query(pattern, filter)).unwrap();
        assert_eq!(indexed.total, direct.total);
        assert_eq!(
            indexed
                .hits
                .iter()
                .map(|hit| &hit.record.id)
                .collect::<Vec<_>>(),
            direct
                .hits
                .iter()
                .map(|hit| &hit.record.id)
                .collect::<Vec<_>>()
        );
    }
}
