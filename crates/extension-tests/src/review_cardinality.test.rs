//! Scalar cardinality must remain observable after candidate planning.

use super::review_findings::mixed_repo;
use super::review_stage1::JsonAliasProvider;
use objekte_core::{CacheMode, JsonQuery, JsonQueryBackend, Options, Repository, TextQuery};
use std::fs;

#[test]
fn custom_array_error_survives_indexed_scalar_and_text_pruning() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("data/mixed");
    fs::create_dir_all(directory.join(".editor")).unwrap();
    fs::write(directory.join(".editor/schema.json"),
        r#"{"type":"object","properties":{"year":{"type":"integer","index":true},"body":{"type":"string","x-full-text":"plain"}}}"#).unwrap();
    fs::write(
        directory.join("good.json"),
        r#"{"year":1848,"body":"rare target"}"#,
    )
    .unwrap();
    fs::write(
        directory.join("bad.md"),
        r#"{"year":[1850,1851],"body":"ordinary"}"#,
    )
    .unwrap();
    let mut indexed = mixed_repo(temp.path(), &directory);
    indexed.warm_bigram_index("mixed").unwrap();
    let scalar = JsonQuery {
        filter: "year = 1848".into(),
        collections: Some(vec!["mixed".into()]),
        count: true,
        ..Default::default()
    };
    let indexed_error = indexed.query_json(&scalar).unwrap_err().to_string();
    let mut direct = Repository::builder(temp.path())
        .options(Options {
            cache: CacheMode::Disabled,
            query_cache_bytes: 0,
            ..Default::default()
        })
        .register_source(JsonAliasProvider::new(directory))
        .unwrap()
        .open()
        .unwrap();
    let direct_error = direct
        .query_json(&JsonQuery {
            backend: JsonQueryBackend::Direct,
            ..scalar
        })
        .unwrap_err()
        .to_string();
    assert!(direct_error.contains("multiple values"), "{direct_error}");
    assert!(indexed_error.contains("multiple values"), "{indexed_error}");
    let text = TextQuery {
        pattern: "rare target".into(),
        filter: Some("year = 1848".into()),
        fields: Some(vec!["body".into()]),
        collections: Some(vec!["mixed".into()]),
        count: true,
        ..Default::default()
    };
    indexed.warm_text_index("mixed").unwrap();
    let text_error = indexed.search_text(&text).unwrap_err().to_string();
    assert!(text_error.contains("multiple values"), "{text_error}");
}

#[test]
fn residual_and_late_collection_arrays_are_checked_before_pruning() {
    let temp = tempfile::tempdir().unwrap();
    let mixed = temp.path().join("data/zeta");
    let native = temp.path().join("data/alpha");
    fs::create_dir_all(mixed.join(".editor")).unwrap();
    fs::create_dir_all(native.join(".editor")).unwrap();
    let schema = r#"{"type":"object","properties":{"year":{"type":"integer","index":true},"body":{"type":"string","x-full-text":"plain"},"misc":{}}}"#;
    for root in [&mixed, &native] {
        fs::write(root.join(".editor/schema.json"), schema).unwrap();
    }
    fs::write(
        native.join("a.json"),
        r#"{"year":1848,"misc":1,"body":"rare target"}"#,
    )
    .unwrap();
    for index in 0..48 {
        fs::write(
            mixed.join(format!("n{index:03}.json")),
            format!(r#"{{"year":1850,"misc":1,"body":"ordinary {index}"}}"#),
        )
        .unwrap();
    }
    fs::write(
        mixed.join("z.md"),
        r#"{"year":1850,"misc":[2,3],"nested":{"value":[4,5]},"body":"ordinary"}"#,
    )
    .unwrap();
    let mut indexed = mixed_repo(temp.path(), &mixed);
    indexed.warm_bigram_index("zeta").unwrap();
    indexed.warm_text_index("zeta").unwrap();
    let mut direct = Repository::builder(temp.path())
        .options(Options {
            cache: CacheMode::Disabled,
            query_cache_bytes: 0,
            ..Default::default()
        })
        .register_source(JsonAliasProvider::new(mixed))
        .unwrap()
        .open()
        .unwrap();
    for (filter, scope, limit) in [
        ("year = 1848 && misc = 1", vec!["zeta"], 10),
        ("year = 1848 && misc = 1", vec!["alpha", "zeta"], 1),
        ("exists(misc) || misc = 1", vec!["zeta"], 1),
        ("year = 1848 && nested.value = 1", vec!["zeta"], 1),
    ] {
        let query = JsonQuery {
            filter: filter.into(),
            collections: Some(scope.into_iter().map(str::to_owned).collect()),
            limit,
            count: false,
            ..Default::default()
        };
        let expected = direct
            .query_json(&JsonQuery {
                backend: JsonQueryBackend::Direct,
                ..query.clone()
            })
            .unwrap_err()
            .to_string();
        let actual = indexed.query_json(&query).unwrap_err().to_string();
        assert!(expected.contains("multiple values"), "{filter}: {expected}");
        assert!(actual.contains("multiple values"), "{filter}: {actual}");
    }
    let text = TextQuery {
        pattern: "rare target".into(),
        filter: Some("misc = 1".into()),
        fields: Some(vec!["body".into()]),
        collections: Some(vec!["alpha".into(), "zeta".into()]),
        limit: 1,
        ..Default::default()
    };
    let expected = direct.search_text(&text).unwrap_err().to_string();
    let actual = indexed.search_text(&text).unwrap_err().to_string();
    assert!(expected.contains("multiple values"), "{expected}");
    assert!(actual.contains("multiple values"), "{actual}");
}

#[test]
fn scalar_to_array_refresh_and_restore_revoke_safety() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("data/mixed");
    fs::create_dir_all(directory.join(".editor")).unwrap();
    fs::write(
        directory.join(".editor/schema.json"),
        r#"{"type":"object","properties":{"year":{"type":"integer","index":true}}}"#,
    )
    .unwrap();
    fs::write(directory.join("a.json"), r#"{"year":1848}"#).unwrap();
    for index in 0..64 {
        fs::write(
            directory.join(format!("n{index:03}.json")),
            r#"{"year":1850}"#,
        )
        .unwrap();
    }
    fs::write(directory.join("z.md"), r#"{"year":1850}"#).unwrap();
    let query = JsonQuery {
        filter: "year = 1848".into(),
        collections: Some(vec!["mixed".into()]),
        count: true,
        ..Default::default()
    };
    let mut indexed = Repository::builder(temp.path())
        .register_source(JsonAliasProvider::new(directory.clone()))
        .unwrap()
        .open()
        .unwrap();
    indexed.warm_bigram_index("mixed").unwrap();
    let safe = indexed.query_json(&query).unwrap();
    assert_eq!(safe.total, Some(1));
    assert!(
        safe.metrics.source_reads < 10,
        "safe indexed data should remain selective"
    );
    fs::write(directory.join("empty.md"), r#"{"year":[]}"#).unwrap();
    fs::write(directory.join("one.md"), r#"{"year":[1850]}"#).unwrap();
    fs::write(directory.join("z.md"), r#"{"year":[1850,1851]}"#).unwrap();
    indexed.refresh("mixed").unwrap();
    assert!(
        indexed
            .query_json(&query)
            .unwrap_err()
            .to_string()
            .contains("multiple values")
    );
    indexed.save_bigram_cache("mixed").unwrap();
    drop(indexed);
    let mut reopened = Repository::builder(temp.path())
        .register_source(JsonAliasProvider::new(directory))
        .unwrap()
        .open()
        .unwrap();
    assert!(
        reopened
            .query_json(&query)
            .unwrap_err()
            .to_string()
            .contains("multiple values")
    );
    let mut direct = Repository::builder(temp.path())
        .options(Options {
            cache: CacheMode::Disabled,
            query_cache_bytes: 0,
            ..Default::default()
        })
        .register_source(JsonAliasProvider::new(temp.path().join("data/mixed")))
        .unwrap()
        .open()
        .unwrap();
    for filter in [
        "year ?= 1850",
        "exists(year)",
        "year[0] = 1850",
        "year:length = 2",
    ] {
        let request = JsonQuery {
            filter: filter.into(),
            collections: Some(vec!["mixed".into()]),
            count: true,
            ..Default::default()
        };
        let legitimate = reopened.query_json(&request).unwrap();
        let oracle = direct
            .query_json(&JsonQuery {
                backend: JsonQueryBackend::Direct,
                ..request
            })
            .unwrap();
        assert_eq!(legitimate.total, oracle.total, "{filter}");
        assert_eq!(
            legitimate
                .records
                .iter()
                .map(|record| &record.id)
                .collect::<Vec<_>>(),
            oracle
                .records
                .iter()
                .map(|record| &record.id)
                .collect::<Vec<_>>(),
            "{filter}"
        );
    }
}
