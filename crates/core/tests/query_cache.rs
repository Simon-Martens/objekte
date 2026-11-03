use objekte_core::{
    CacheMode, Freshness, JsonQuery, JsonQueryBackend, Options, Repository, TextMode, TextQuery,
};
use serde_json::json;
use std::fs;

fn fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let schema = temp.path().join(".objekte/collections/books.schema.json");
    fs::create_dir_all(schema.parent().unwrap()).unwrap();
    fs::write(
        schema,
        json!({
            "type":"object",
            "properties":{
                "year":{"type":"integer","index":true},
                "title":{"type":"string","x-sort-index":true},
                "annotation":{"type":"string","x-full-text":"plain"},
                "city":{"type":"string"},
                "name":{"type":"string"}
            },
            "x-objekte":{"name":"books","path":"data/books","type":"data","format":"json","id":"filename"}
        })
        .to_string(),
    )
    .unwrap();
    let data = temp.path().join("data/books");
    fs::create_dir_all(&data).unwrap();
    for index in 0..48 {
        let value = json!({
            "year": if index < 24 { 1820 } else { 1830 },
            "title":format!("Title {:02}", 47 - index),
            "annotation":if index % 3 == 0 { "Goethe visited" } else { "Other author" },
            "city":if index % 2 == 0 { "Berlin" } else { "Leipzig" },
            "name":if index % 4 == 0 { "Müller" } else { "Schmidt" }
        });
        fs::write(data.join(format!("{index:03}.json")), value.to_string()).unwrap();
    }
    temp
}

fn repo(root: &std::path::Path, cache: CacheMode, budget: usize) -> Repository {
    let mut options = Options::new();
    options.cache = cache;
    options.query_cache_bytes = budget;
    Repository::load_repository(root, options).unwrap()
}

fn query(filter: &str, offset: usize, sort: &str) -> JsonQuery {
    JsonQuery {
        filter: format!("{filter}{sort}"),
        collections: Some(vec!["books".into()]),
        count: true,
        limit: 3,
        offset,
        ..Default::default()
    }
}

fn ids(result: &objekte_core::JsonQueryResult) -> Vec<&str> {
    result
        .records
        .iter()
        .map(|record| record.id.as_str())
        .collect()
}

#[test]
fn complete_membership_pagination_and_sort_reuse_match_direct_execution() {
    let fixture = fixture();
    let mut cached = repo(fixture.path(), CacheMode::Memory, 64 * 1024 * 1024);
    let mut direct = repo(fixture.path(), CacheMode::Disabled, 0);
    let filter = "annotation:fts ~ 'Goethe'";
    for (offset, sort) in [
        (0, ""),
        (3, ""),
        (12, ""),
        (3, ""),
        (0, " SORT BY title ASC"),
        (6, " SORT BY title ASC"),
        (0, " SORT BY title DESC"),
        (6, " SORT BY title DESC"),
        (0, " SORT BY @id ASC"),
        (6, " SORT BY @id DESC"),
    ] {
        let request = query(filter, offset, sort);
        let got = cached.query_json(&request).unwrap();
        let expected = direct
            .query_json(&JsonQuery {
                backend: JsonQueryBackend::Direct,
                ..request
            })
            .unwrap();
        assert_eq!(ids(&got), ids(&expected), "{sort} at {offset}");
        assert_eq!(got.total, expected.total);
        assert_eq!(got.truncated, expected.truncated);
        if offset > 0 || !sort.is_empty() {
            assert!(got.metrics.predicate_cache_hits > 0);
        }
    }
    assert!(cached.query_cache_bytes() > 0);
    assert!(cached.query_cache_bytes() <= 64 * 1024 * 1024);
}

#[test]
fn conjunctions_reuse_complete_and_partial_truth_then_refresh_invalidates() {
    let fixture = fixture();
    let mut cached = repo(fixture.path(), CacheMode::Memory, 64 * 1024 * 1024);
    let mut direct = repo(fixture.path(), CacheMode::Disabled, 0);
    let expressions = [
        "year = 1820",
        "year = 1820 && annotation:fts ~ 'Goethe'",
        "year = 1820 && annotation:fts ~ 'Goethe' && city = 'Berlin'",
        "year = 1820 && annotation:fts ~ 'Goethe' && name = 'Müller'",
        "annotation:fts ~ 'Goethe' && name = 'Müller'",
        "annotation:fts ~ 'Goethe' || name = 'Müller'",
        "annotation:fts ~ 'Goethe' && year = 1820",
    ];
    let mut reuse = 0;
    for expression in expressions {
        let request = query(expression, 0, "");
        let got = cached.query_json(&request).unwrap();
        let expected = direct
            .query_json(&JsonQuery {
                backend: JsonQueryBackend::Direct,
                ..request
            })
            .unwrap();
        assert_eq!(ids(&got), ids(&expected), "{expression}");
        assert_eq!(got.total, expected.total);
        if expression.contains("city = 'Berlin'") {
            assert!(got.metrics.predicate_cache_hits > 0);
        }
        if expression == "annotation:fts ~ 'Goethe' && name = 'Müller'" {
            assert!(got.metrics.partial_evaluation_hits > 0);
        }
        if expression == "annotation:fts ~ 'Goethe' || name = 'Müller'" {
            assert!(got.metrics.partial_evaluation_hits > 0);
        }
        reuse += got.metrics.predicate_cache_hits + got.metrics.partial_evaluation_hits;
    }
    assert!(reuse > 0);
    let path = fixture.path().join("data/books/000.json");
    fs::write(
        &path,
        json!({"year":1820,"title":"Changed","annotation":"Other","city":"Berlin","name":"Müller"})
            .to_string(),
    )
    .unwrap();
    cached.refresh("books").unwrap();
    let request = query("annotation:fts ~ 'Goethe'", 0, "");
    let got = cached.query_json(&request).unwrap();
    let expected = direct
        .query_json(&JsonQuery {
            backend: JsonQueryBackend::Direct,
            ..request
        })
        .unwrap();
    assert_eq!(got.total, expected.total);
    assert_eq!(ids(&got), ids(&expected));
}

#[test]
fn complete_or_branches_form_a_complete_cached_union() {
    let fixture = fixture();
    let mut cached = repo(fixture.path(), CacheMode::Memory, 64 * 1024 * 1024);
    let mut direct = repo(fixture.path(), CacheMode::Disabled, 0);
    for expression in [
        "annotation:fts ~ 'Goethe'",
        "name = 'Müller'",
        "annotation:fts ~ 'Goethe' || name = 'Müller'",
    ] {
        let request = query(expression, 0, "");
        let got = cached.query_json(&request).unwrap();
        let expected = direct
            .query_json(&JsonQuery {
                backend: JsonQueryBackend::Direct,
                ..request
            })
            .unwrap();
        assert_eq!(ids(&got), ids(&expected));
        assert_eq!(got.total, expected.total);
        if expression.contains("||") {
            assert!(got.metrics.predicate_cache_hits > 0);
            assert_eq!(got.metrics.grep_scans, 0);
        }
    }
}

#[test]
fn cached_conjunction_and_exact_posting_need_only_page_reads() {
    let fixture = fixture();
    let schema_path = fixture
        .path()
        .join(".objekte/collections/books.schema.json");
    let mut schema: serde_json::Value =
        serde_json::from_slice(&fs::read(&schema_path).unwrap()).unwrap();
    schema["properties"]["city"]["index"] = json!(true);
    fs::write(&schema_path, schema.to_string()).unwrap();
    let mut cached = repo(fixture.path(), CacheMode::Memory, 64 * 1024 * 1024);
    let mut direct = repo(fixture.path(), CacheMode::Disabled, 0);
    let base = query("year = 1820 && annotation:fts ~ 'Goethe'", 0, "");
    cached.query_json(&base).unwrap();
    let refined = query(
        "year = 1820 && annotation:fts ~ 'Goethe' && city = 'Berlin'",
        0,
        " SORT BY title ASC",
    );
    let result = cached.query_json(&refined).unwrap();
    let expected = direct
        .query_json(&JsonQuery {
            backend: JsonQueryBackend::Direct,
            ..refined.clone()
        })
        .unwrap();
    assert_eq!(ids(&result), ids(&expected));
    assert_eq!(result.total, expected.total);
    assert_eq!(result.metrics.grep_scans, 0);
    assert!(result.metrics.predicate_cache_hits > 0);
    let next_page = cached
        .query_json(&JsonQuery {
            offset: 3,
            ..refined
        })
        .unwrap();
    assert!(next_page.metrics.ordered_result_hits > 0);
}

#[test]
fn zero_budget_skips_verification_cache() {
    let fixture = fixture();
    let mut repo = repo(fixture.path(), CacheMode::Memory, 0);
    let request = query("annotation:fts ~ 'Goethe'", 0, "");
    repo.query_json(&request).unwrap();
    let again = repo.query_json(&request).unwrap();
    assert_eq!(repo.query_cache_bytes(), 0);
    assert_eq!(again.metrics.predicate_cache_hits, 0);
}

#[test]
fn refresh_and_reload_invalidate_only_changed_collection_results() {
    let fixture = fixture();
    let mut cached = repo(fixture.path(), CacheMode::Memory, 64 * 1024 * 1024);
    let request = query("annotation:fts ~ 'Goethe'", 0, " SORT BY title ASC");
    cached.query_json(&request).unwrap();
    assert!(
        cached
            .query_json(&request)
            .unwrap()
            .metrics
            .ordered_result_hits
            > 0
    );

    let data = fixture.path().join("data/books");
    fs::remove_file(data.join("000.json")).unwrap();
    fs::write(
        data.join("099.json"),
        json!({
            "year": 1820, "title": "A new title", "annotation": "Goethe visited",
            "city": "Berlin", "name": "Müller"
        })
        .to_string(),
    )
    .unwrap();
    cached.refresh("books").unwrap();
    let changed = cached.query_json(&request).unwrap();
    assert_eq!(changed.metrics.ordered_result_hits, 0);
    assert_eq!(changed.metrics.predicate_cache_hits, 0);
    let mut direct = repo(fixture.path(), CacheMode::Disabled, 0);
    let expected = direct
        .query_json(&JsonQuery {
            backend: JsonQueryBackend::Direct,
            ..request.clone()
        })
        .unwrap();
    assert_eq!(ids(&changed), ids(&expected));
    assert_eq!(changed.total, expected.total);

    let schema_path = fixture
        .path()
        .join(".objekte/collections/books.schema.json");
    let mut schema: serde_json::Value =
        serde_json::from_slice(&fs::read(&schema_path).unwrap()).unwrap();
    schema["properties"]["city"]["index"] = json!(true);
    fs::write(&schema_path, schema.to_string()).unwrap();
    cached.reload().unwrap();
    let reloaded = cached.query_json(&request).unwrap();
    assert_eq!(reloaded.metrics.ordered_result_hits, 0);
    assert_eq!(ids(&reloaded), ids(&expected));
}

#[test]
fn metadata_on_request_refreshes_before_cache_lookup() {
    let fixture = fixture();
    let mut options = Options::new();
    options.freshness = Freshness::MetadataOnRequest;
    let mut cached = Repository::load_repository(fixture.path(), options).unwrap();
    let request = query("annotation:fts ~ 'Goethe'", 0, "");
    let before = cached.query_json(&request).unwrap();
    assert!(
        cached
            .query_json(&request)
            .unwrap()
            .metrics
            .predicate_cache_hits
            > 0
    );
    fs::write(
        fixture.path().join("data/books/000.json"),
        json!({
            "year": 1820, "title": "Changed", "annotation": "Other author",
            "city": "Berlin", "name": "Müller"
        })
        .to_string(),
    )
    .unwrap();
    let after = cached.query_json(&request).unwrap();
    assert_eq!(after.total, Some(before.total.unwrap() - 1));
    assert_eq!(after.metrics.predicate_cache_hits, 0);
}

#[test]
fn cardinality_errors_and_hash_freshness_never_use_cached_truth() {
    let fixture = fixture();
    let request = query("city = 'Berlin'", 0, "");
    let mut cached = repo(fixture.path(), CacheMode::Memory, 64 * 1024 * 1024);
    cached.query_json(&request).unwrap();
    assert!(
        cached
            .query_json(&request)
            .unwrap()
            .metrics
            .predicate_cache_hits
            > 0
    );
    fs::write(
        fixture.path().join("data/books/000.json"),
        json!({
            "year": 1820, "title": "Changed", "annotation": "Goethe visited",
            "city": ["Berlin", "Leipzig"], "name": "Müller"
        })
        .to_string(),
    )
    .unwrap();
    cached.refresh("books").unwrap();
    assert!(cached.query_json(&request).is_err());

    let mut options = Options::new();
    options.freshness = Freshness::HashOnRequest;
    let mut hash_repo = Repository::load_repository(fixture.path(), options).unwrap();
    let safe_request = query("annotation:fts ~ 'Goethe'", 0, "");
    hash_repo.query_json(&safe_request).unwrap();
    let again = hash_repo.query_json(&safe_request).unwrap();
    assert_eq!(again.metrics.predicate_cache_hits, 0);
    assert_eq!(hash_repo.query_cache_bytes(), 0);
}

#[test]
fn an_early_page_only_reuses_its_proven_order_prefix() {
    let fixture = fixture();
    let mut repo = repo(fixture.path(), CacheMode::Memory, 64 * 1024 * 1024);
    let mut request = query("annotation:fts ~ 'Goethe'", 0, "");
    request.count = false;
    let first = repo.query_json(&request).unwrap();
    let again = repo.query_json(&request).unwrap();
    assert_eq!(ids(&first), ids(&again));
    assert_eq!(again.total, None);
    assert!(again.metrics.ordered_result_hits > 0);
    request.offset = 12;
    let deep = repo.query_json(&request).unwrap();
    assert_eq!(deep.records.len(), 3);
}

#[test]
fn capped_indexed_sort_prefix_preserves_truncation() {
    let fixture = fixture();
    let mut options = Options::new();
    options.total_limit = 10;
    let mut repo = Repository::load_repository(fixture.path(), options).unwrap();
    let request = query("annotation:fts ~ 'Goethe'", 6, " SORT BY title ASC");
    let first = repo.query_json(&request).unwrap();
    let again = repo.query_json(&request).unwrap();
    assert_eq!(ids(&first), ids(&again));
    assert_eq!(again.total, Some(10));
    assert!(again.truncated);
    assert!(again.metrics.ordered_result_hits > 0);
}

#[test]
fn text_pages_reuse_slots_but_recompute_hit_fields() {
    let fixture = fixture();
    let mut cached = repo(fixture.path(), CacheMode::Memory, 64 * 1024 * 1024);
    let mut direct = repo(fixture.path(), CacheMode::Disabled, 0);
    let mut request = TextQuery {
        pattern: "Goethe".into(),
        collections: Some(vec!["books".into()]),
        count: true,
        limit: 3,
        ..Default::default()
    };
    for offset in [0, 3, 9, 3] {
        request.offset = offset;
        let got = cached.search_text(&request).unwrap();
        let expected = direct.search_text(&request).unwrap();
        assert_eq!(got.total, expected.total);
        assert_eq!(
            got.hits
                .iter()
                .map(|hit| (&hit.record.id, &hit.fields, &hit.matched_paths))
                .collect::<Vec<_>>(),
            expected
                .hits
                .iter()
                .map(|hit| (&hit.record.id, &hit.fields, &hit.matched_paths))
                .collect::<Vec<_>>()
        );
        if offset > 0 {
            assert!(got.metrics.predicate_cache_hits > 0);
        }
    }
    request.offset = 0;
    request.mode = TextMode::WordRaw;
    let raw = cached.search_text(&request).unwrap();
    assert_eq!(raw.metrics.predicate_cache_hits, 0);

    request.mode = TextMode::Word;
    request.count = false;
    request.offset = 0;
    request.pattern = "Goethe visited".into();
    let first = cached.search_text(&request).unwrap();
    let again = cached.search_text(&request).unwrap();
    assert_eq!(
        first
            .hits
            .iter()
            .map(|hit| &hit.record.id)
            .collect::<Vec<_>>(),
        again
            .hits
            .iter()
            .map(|hit| &hit.record.id)
            .collect::<Vec<_>>()
    );
    assert!(again.metrics.ordered_result_hits > 0);

    request.count = true;
    request.pattern = "Goethe".into();
    request.filter = Some("year >= 0 SORT BY title ASC".into());
    let sorted_first = cached.search_text(&request).unwrap();
    let sorted_again = cached.search_text(&request).unwrap();
    let sorted_direct = direct.search_text(&request).unwrap();
    assert_eq!(
        sorted_again
            .hits
            .iter()
            .map(|hit| &hit.record.id)
            .collect::<Vec<_>>(),
        sorted_direct
            .hits
            .iter()
            .map(|hit| &hit.record.id)
            .collect::<Vec<_>>()
    );
    assert_eq!(sorted_first.total, sorted_again.total);
    assert!(sorted_again.metrics.ordered_result_hits > 0);
}

#[test]
fn malformed_text_matches_keep_raw_hit_fields_after_reuse() {
    let fixture = fixture();
    fs::write(
        fixture.path().join("data/books/090.json"),
        br#"{"annotation":"Goethe"#,
    )
    .unwrap();
    let mut cached = repo(fixture.path(), CacheMode::Memory, 64 * 1024 * 1024);
    let mut direct = repo(fixture.path(), CacheMode::Disabled, 0);
    let request = TextQuery {
        pattern: "Goethe".into(),
        collections: Some(vec!["books".into()]),
        count: true,
        limit: 50,
        ..Default::default()
    };
    cached.search_text(&request).unwrap();
    let repeated = cached.search_text(&request).unwrap();
    let expected = direct.search_text(&request).unwrap();
    assert_eq!(repeated.total, expected.total);
    assert_eq!(
        repeated
            .hits
            .iter()
            .map(|hit| (&hit.record.id, &hit.fields))
            .collect::<Vec<_>>(),
        expected
            .hits
            .iter()
            .map(|hit| (&hit.record.id, &hit.fields))
            .collect::<Vec<_>>()
    );
    assert!(
        repeated
            .hits
            .iter()
            .any(|hit| hit.record.id == "090" && hit.fields == ["$raw"])
    );
    assert!(repeated.metrics.ordered_result_hits > 0);
}
