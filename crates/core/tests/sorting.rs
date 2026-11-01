use objekte_core::{
    CacheMode, Direction, JsonQuery, JsonQueryBackend, Options, Repository, Sort, TextQuery,
    split_sort_clause,
};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn write(root: &Path, collection: &str, id: &str, value: Value) {
    let folder = root.join("data").join(collection);
    fs::create_dir_all(&folder).unwrap();
    fs::write(
        folder.join(format!("{id}.json")),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
}

fn schema(root: &Path, name: &str, language: Option<&str>) {
    let folder = root.join(".objekte/collections");
    fs::create_dir_all(&folder).unwrap();
    let mut value = json!({
        "type":"object",
        "properties": {
            "title":{"type":"string","x-sort-index":true},
            "genre":{"type":"string","enum":["Ära","ära","Adel","Zebra"],"x-sort-index":true},
            "rank":{"type":"integer","index":true},
            "meta":{"type":"object","properties":{"label":{"type":"string"}}},
            "names":{"type":"array","items":{"type":"string"},"x-sort-reduce":"min"},
            "text":{"type":"string"}
        },
        "x-sort-indexes":[[
            {"field":"rank","direction":"desc"},
            {"field":"title","direction":"asc"}
        ]],
        "x-objekte":{"name":name,"path":format!("data/{name}"),"type":"data","format":"json","id":"filename"}
    });
    if let Some(language) = language {
        value["x-language"] = json!(language);
    }
    if name == "de" {
        value["properties"]["genre"]["x-language"] = json!("sv");
    }
    fs::write(
        folder.join(format!("{name}.schema.json")),
        serde_json::to_vec_pretty(&value).unwrap(),
    )
    .unwrap();
}

fn fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    schema(root, "de", Some("de"));
    schema(root, "en", None);
    schema(root, "sv", Some("sv"));
    for (id, title, rank, label, names) in [
        ("01", "äpfel", 1, "z", vec!["Z", "Ä"]),
        ("02", "Apfel", 2, "y", vec!["B", "A"]),
        ("03", "ÄPFEL", 1, "x", vec!["ä", "a"]),
        ("04", "Aal", 2, "w", vec!["C"]),
        ("05", "aal", 2, "v", vec!["C"]),
        ("06", "Auto", 1, "u", vec!["C"]),
        ("07", "Bär", 3, "t", vec!["C"]),
        ("08", "Ähre", 1, "s", vec!["C"]),
    ] {
        let genre = match id {
            "01" | "02" => "Ära",
            "03" => "ära",
            "04" => "Adel",
            _ => "Zebra",
        };
        write(
            root,
            "de",
            id,
            json!({"title":title,"genre":genre,"rank":rank,
            "meta":{"label":label},"names":names,"text":"token"}),
        );
    }
    for collection in ["en", "sv"] {
        for (id, title) in [("01", "Zebra"), ("02", "Ära"), ("03", "Adel")] {
            write(
                root,
                collection,
                id,
                json!({"title":title,"rank":1,"text":"token"}),
            );
        }
    }
    temp
}

#[test]
fn english_swedish_and_enum_field_override_use_distinct_collations() {
    let fixture = fixture();
    let mut repository = repo(fixture.path(), CacheMode::Memory, 100);
    for (collection, expected) in [
        ("en", vec!["en/03", "en/02", "en/01"]),
        ("sv", vec!["sv/03", "sv/01", "sv/02"]),
    ] {
        let result = repository
            .query_json(&JsonQuery {
                filter: "rank >= 0 SORT BY title".into(),
                collections: Some(vec![collection.into()]),
                count: true,
                limit: 100,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(ids(&result), expected);
    }
    let result = repository
        .query_json(&JsonQuery {
            filter: "rank >= 0 SORT BY genre".into(),
            collections: Some(vec!["de".into()]),
            count: true,
            limit: 100,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(&ids(&result)[..4], &["de/04", "de/05", "de/06", "de/07"]);
    assert_eq!(&ids(&result)[4..], &["de/08", "de/01", "de/02", "de/03"]);
}

fn repo(root: &Path, cache: CacheMode, cap: usize) -> Repository {
    let mut options = Options::new();
    options.cache = cache;
    options.total_limit = cap;
    Repository::load_repository(root, options).unwrap()
}

fn ids(result: &objekte_core::JsonQueryResult) -> Vec<String> {
    result
        .records
        .iter()
        .map(|record| format!("{}/{}", record.collection, record.id))
        .collect()
}

#[test]
fn sort_clause_parser_preserves_literals_and_accepts_programmatic_terms() {
    let (filter, sorts) =
        split_sort_clause("title ~ \"Ä SORT BY B\" && rank > 0 SORT BY title DESC && rank")
            .unwrap();
    assert_eq!(filter, "title ~ \"Ä SORT BY B\" && rank > 0");
    assert_eq!(
        sorts,
        vec![
            Sort {
                field: "title".into(),
                direction: Direction::Desc
            },
            Sort {
                field: "rank".into(),
                direction: Direction::Asc
            }
        ]
    );
    assert!(split_sort_clause("rank > 0 SORT BY title && title").is_err());
    assert_eq!(
        split_sort_clause("rank > 0 SORT BY ['a&&b'] DESC && title")
            .unwrap()
            .1[0]
            .field,
        "['a&&b']"
    );
    let fixture = fixture();
    let mut repository = repo(fixture.path(), CacheMode::Memory, 100);
    let result = repository
        .query_json(&JsonQuery {
            filter: "rank >= 0".into(),
            sort: sorts[..1].to_vec(),
            collections: Some(vec!["de".into()]),
            limit: 100,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(result.records[0].id, "07");
}

#[test]
fn german_case_and_umlauts_match_direct_and_indexed_sorts() {
    let fixture = fixture();
    let mut indexed = repo(fixture.path(), CacheMode::Memory, 100);
    let mut direct = repo(fixture.path(), CacheMode::Disabled, 100);
    let query = JsonQuery {
        filter: "rank >= 0 SORT BY title ASC".into(),
        collections: Some(vec!["de".into()]),
        count: true,
        limit: 100,
        ..Default::default()
    };
    let got = indexed.query_json(&query).unwrap();
    let expected = direct
        .query_json(&JsonQuery {
            backend: JsonQueryBackend::Direct,
            ..query
        })
        .unwrap();
    assert_eq!(ids(&got), ids(&expected));
    assert_eq!(
        ids(&got),
        [
            "de/04", "de/05", "de/08", "de/02", "de/01", "de/03", "de/06", "de/07"
        ]
    );
    assert!(
        got.query_plan
            .iter()
            .any(|step| step.contains("ordered sort index"))
    );
    assert!(
        got.query_plan
            .iter()
            .any(|step| step.contains("direct verified page"))
    );
}

#[test]
fn composite_desc_and_unindexed_nested_sort_agree_with_direct() {
    let fixture = fixture();
    let mut indexed = repo(fixture.path(), CacheMode::Memory, 100);
    let mut direct = repo(fixture.path(), CacheMode::Disabled, 100);
    for filter in [
        "rank >= 0 SORT BY rank DESC && title ASC",
        "rank >= 0 SORT BY title DESC",
        "rank >= 0 SORT BY meta.label DESC",
        "rank >= 0 SORT BY names ASC",
    ] {
        let request = JsonQuery {
            filter: filter.into(),
            collections: Some(vec!["de".into()]),
            count: true,
            limit: 100,
            ..Default::default()
        };
        let got = indexed.query_json(&request).unwrap();
        let expected = direct
            .query_json(&JsonQuery {
                backend: JsonQueryBackend::Direct,
                ..request
            })
            .unwrap();
        assert_eq!(ids(&got), ids(&expected), "{filter}");
        if filter.ends_with("names ASC") {
            assert_eq!(
                ids(&got),
                [
                    "de/02", "de/03", "de/01", "de/04", "de/05", "de/06", "de/07", "de/08"
                ]
            );
        }
        if filter.contains("title DESC") || filter.contains("rank DESC && title ASC") {
            assert!(
                got.query_plan
                    .iter()
                    .any(|step| step.contains("ordered sort index"))
            );
        }
    }
}

#[test]
fn text_sort_stream_updates_and_collection_order() {
    let fixture = fixture();
    let mut repository = repo(fixture.path(), CacheMode::Memory, 100);
    let mut direct = repo(fixture.path(), CacheMode::Disabled, 100);
    let request = TextQuery {
        pattern: "token".into(),
        sort: vec![Sort {
            field: "title".into(),
            direction: Direction::Asc,
        }],
        count: true,
        limit: 4,
        ..Default::default()
    };
    let mut pages = Vec::new();
    let summary = repository
        .search_text_stream_updates(&request, |page| {
            pages.push(
                page.iter()
                    .map(|hit| format!("{}/{}", hit.record.collection, hit.record.id))
                    .collect::<Vec<_>>(),
            );
            Ok(())
        })
        .unwrap();
    let final_page = repository.search_text(&request).unwrap();
    let direct_page = direct.search_text(&request).unwrap();
    assert_eq!(
        final_page
            .hits
            .iter()
            .map(|hit| (&hit.record.collection, &hit.record.id))
            .collect::<Vec<_>>(),
        direct_page
            .hits
            .iter()
            .map(|hit| (&hit.record.collection, &hit.record.id))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        pages.last().unwrap(),
        &final_page
            .hits
            .iter()
            .map(|hit| format!("{}/{}", hit.record.collection, hit.record.id))
            .collect::<Vec<_>>()
    );
    assert_eq!(summary.total, Some(14));
    assert!(
        final_page
            .hits
            .iter()
            .all(|hit| hit.record.collection == "de")
    );
    assert!(pages.len() > 1);
}

#[test]
fn unindexed_sort_stream_replaces_provisional_pages() {
    let fixture = fixture();
    let mut repository = repo(fixture.path(), CacheMode::Memory, 100);
    let request = JsonQuery {
        filter: "rank >= 0 SORT BY meta.label ASC".into(),
        collections: Some(vec!["de".into()]),
        count: true,
        limit: 3,
        ..Default::default()
    };
    let mut pages = Vec::new();
    let summary = repository
        .query_json_stream_updates(&request, |page| {
            pages.push(
                page.iter()
                    .map(|record| record.id.clone())
                    .collect::<Vec<_>>(),
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(summary.total, Some(8));
    assert_eq!(pages.last().unwrap(), &vec!["08", "07", "06"]);
    assert_ne!(pages.first(), pages.last());
    assert!(pages.iter().all(|page| page.len() <= 3));
}

#[test]
fn unindexed_bigram_sort_reads_each_match_once() {
    let fixture = fixture();
    let mut repository = repo(fixture.path(), CacheMode::Memory, 100);
    repository.warm_bigram_index("de").unwrap();
    repository.reset_metrics();
    let result = repository
        .query_json(&JsonQuery {
            filter: "rank >= 0 SORT BY meta.label ASC".into(),
            collections: Some(vec!["de".into()]),
            count: true,
            limit: 3,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(result.total, Some(8));
    assert_eq!(result.metrics.source_reads, 8);
    assert_eq!(result.metrics.parses, 8);
    assert_eq!(
        result
            .records
            .iter()
            .map(|record| record.id.as_str())
            .collect::<Vec<_>>(),
        ["08", "07", "06"]
    );
    assert!(result.records.iter().all(|record| !record.hash.is_empty()));
}

#[test]
fn single_pass_sorted_records_match_direct_records() {
    let fixture = fixture();
    let mut indexed = repo(fixture.path(), CacheMode::Memory, 100);
    let mut direct = repo(fixture.path(), CacheMode::Disabled, 100);
    for filter in [
        "rank >= 0 SORT BY meta.label ASC",
        "rank >= 0 SORT BY title DESC",
        "rank >= 0 SORT BY rank DESC && title ASC",
    ] {
        let request = JsonQuery {
            filter: filter.into(),
            collections: Some(vec!["de".into()]),
            count: true,
            limit: 4,
            offset: 2,
            ..Default::default()
        };
        let fast = indexed.query_json(&request).unwrap();
        let plain = direct
            .query_json(&JsonQuery {
                backend: JsonQueryBackend::Direct,
                ..request
            })
            .unwrap();
        assert_eq!(fast.total, plain.total, "{filter}");
        assert_eq!(
            serde_json::to_value(fast.records).unwrap(),
            serde_json::to_value(plain.records).unwrap(),
            "{filter}"
        );
    }
}

#[test]
fn structured_fts_sort_and_result_cap_work_together() {
    let fixture = fixture();
    let mut repository = repo(fixture.path(), CacheMode::Memory, 3);
    let request = TextQuery {
        pattern: "token".into(),
        sort: vec![Sort {
            field: "title".into(),
            direction: Direction::Desc,
        }],
        collections: Some(vec!["de".into()]),
        count: true,
        limit: 2,
        ..Default::default()
    };
    let result = repository.search_text(&request).unwrap();
    assert_eq!(result.total, Some(3));
    assert!(result.truncated);
    assert_eq!(result.hits.len(), 2);
    assert_eq!(result.hits[0].record.id, "07");
    assert!(
        result
            .query_plan
            .iter()
            .any(|step| step.contains("direct verified page"))
    );
}

#[test]
fn indexed_pages_match_direct_across_locales_and_mixed_search() {
    let fixture = fixture();
    let mut indexed = repo(fixture.path(), CacheMode::Memory, 5);
    let mut direct = repo(fixture.path(), CacheMode::Disabled, 5);
    for (collection, sort) in [
        ("de", "title ASC"),
        ("de", "title DESC"),
        ("de", "rank DESC && title ASC"),
        ("de", "genre ASC"),
        ("en", "title ASC"),
        ("sv", "title ASC"),
    ] {
        for (count, offset) in [(false, 0), (true, 2), (false, 4)] {
            let query = JsonQuery {
                filter: format!("rank >= 0 SORT BY {sort}"),
                collections: Some(vec![collection.into()]),
                count,
                limit: 2,
                offset,
                ..Default::default()
            };
            let fast = indexed.query_json(&query).unwrap();
            let plain = direct
                .query_json(&JsonQuery {
                    backend: JsonQueryBackend::Direct,
                    ..query
                })
                .unwrap();
            assert_eq!(
                ids(&fast),
                ids(&plain),
                "{collection} {sort} {count} {offset}"
            );
            assert_eq!(fast.total, plain.total);
            assert_eq!(fast.truncated, plain.truncated);
            assert!(
                fast.query_plan
                    .iter()
                    .any(|step| step.contains("direct verified page"))
            );
        }
    }

    let request = TextQuery {
        pattern: "token".into(),
        filter: Some("rank >= 1 SORT BY title ASC".into()),
        collections: Some(vec!["de".into()]),
        count: true,
        limit: 2,
        offset: 2,
        ..Default::default()
    };
    let fast = indexed.search_text(&request).unwrap();
    let plain = direct.search_text(&request).unwrap();
    assert_eq!(
        fast.hits
            .iter()
            .map(|hit| hit.record.id.as_str())
            .collect::<Vec<_>>(),
        plain
            .hits
            .iter()
            .map(|hit| hit.record.id.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(fast.total, plain.total);
    assert_eq!(fast.truncated, plain.truncated);
    assert!(
        fast.query_plan
            .iter()
            .any(|step| step.contains("direct verified page"))
    );
}

#[test]
fn repository_total_limit_clamps_pages_and_reports_truncation() {
    let fixture = fixture();
    let mut invalid = Options::new();
    invalid.total_limit = 0;
    assert!(Repository::load_repository(fixture.path(), invalid).is_err());
    let mut repository = repo(fixture.path(), CacheMode::Memory, 3);
    for filter in ["rank >= 0", "rank >= 0 SORT BY title ASC"] {
        let first = repository
            .query_json(&JsonQuery {
                filter: filter.into(),
                collections: Some(vec!["de".into()]),
                count: true,
                limit: 2,
                ..Default::default()
            })
            .unwrap();
        let second = repository
            .query_json(&JsonQuery {
                filter: filter.into(),
                collections: Some(vec!["de".into()]),
                count: true,
                limit: 2,
                offset: 2,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(first.total, Some(3));
        assert!(first.truncated);
        assert_eq!(first.records.len(), 2);
        assert_eq!(second.records.len(), 1);
    }
}

#[test]
fn restored_sort_index_and_refresh_follow_source() {
    let fixture = fixture();
    let mut repository = repo(fixture.path(), CacheMode::Memory, 100);
    let query = JsonQuery {
        filter: "rank >= 0 SORT BY title ASC".into(),
        collections: Some(vec!["de".into()]),
        count: true,
        limit: 100,
        ..Default::default()
    };
    let before = ids(&repository.query_json(&query).unwrap());
    repository.save_bigram_cache("de").unwrap();
    drop(repository);
    let mut restored = repo(fixture.path(), CacheMode::Memory, 100);
    let loaded = restored.query_json(&query).unwrap();
    assert_eq!(ids(&loaded), before);
    assert!(loaded.metrics.secondary_snapshot_loads > 0);
    write(
        fixture.path(),
        "de",
        "07",
        json!({"title":"Aa","rank":3,"text":"token"}),
    );
    restored.refresh("de").unwrap();
    assert_eq!(ids(&restored.query_json(&query).unwrap())[0], "de/07");
}

#[test]
fn changing_root_language_rebuilds_saved_sort_keys() {
    let fixture = fixture();
    let query = JsonQuery {
        filter: "rank >= 0 SORT BY title ASC".into(),
        collections: Some(vec!["de".into()]),
        count: true,
        limit: 8,
        ..Default::default()
    };
    let mut original = repo(fixture.path(), CacheMode::Memory, 100);
    original.query_json(&query).unwrap();
    original.save_bigram_cache("de").unwrap();
    drop(original);

    let path = fixture.path().join(".objekte/collections/de.schema.json");
    let mut schema: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    schema["x-language"] = json!("sv");
    fs::write(path, serde_json::to_vec(&schema).unwrap()).unwrap();

    let mut restored = repo(fixture.path(), CacheMode::Memory, 100);
    let mut direct = repo(fixture.path(), CacheMode::Disabled, 100);
    let fast = restored.query_json(&query).unwrap();
    let plain = direct
        .query_json(&JsonQuery {
            backend: JsonQueryBackend::Direct,
            ..query
        })
        .unwrap();
    assert_eq!(ids(&fast), ids(&plain));
    assert!(
        fast.query_plan
            .iter()
            .any(|step| step.contains("direct verified page"))
    );
}

#[test]
fn german_single_letters_case_ties_nulls_and_pages() {
    let fixture = tempfile::tempdir().unwrap();
    schema(fixture.path(), "de", Some("de"));
    for (id, title) in [
        ("01", Some("ä")),
        ("02", Some("A")),
        ("03", Some("a")),
        ("04", Some("Ä")),
        ("05", Some("B")),
        ("06", Some("b")),
        ("07", None),
    ] {
        let value = match title {
            Some(title) => json!({"title":title,"rank":1}),
            None => json!({"rank":1}),
        };
        write(fixture.path(), "de", id, value);
    }
    write(fixture.path(), "de", "08", json!({"title":null,"rank":1}));
    let mut indexed = repo(fixture.path(), CacheMode::Memory, 100);
    let mut direct = repo(fixture.path(), CacheMode::Disabled, 100);
    for (direction, expected) in [
        ("ASC", vec!["02", "03", "01", "04", "05", "06", "08", "07"]),
        ("DESC", vec!["05", "06", "01", "04", "02", "03", "08", "07"]),
    ] {
        let filter = format!("rank >= 0 SORT BY title {direction}");
        let whole = indexed
            .query_json(&JsonQuery {
                filter: filter.clone(),
                collections: Some(vec!["de".into()]),
                limit: 100,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            whole
                .records
                .iter()
                .map(|record| record.id.as_str())
                .collect::<Vec<_>>(),
            expected
        );
        for offset in [0, 2, 4, 6] {
            let request = JsonQuery {
                filter: filter.clone(),
                collections: Some(vec!["de".into()]),
                limit: 2,
                offset,
                ..Default::default()
            };
            let page = indexed.query_json(&request).unwrap();
            let plain = direct
                .query_json(&JsonQuery {
                    backend: JsonQueryBackend::Direct,
                    ..request
                })
                .unwrap();
            assert_eq!(ids(&page), ids(&plain));
            assert_eq!(
                page.records
                    .iter()
                    .map(|record| record.id.as_str())
                    .collect::<Vec<_>>(),
                &expected[offset..(offset + 2)]
            );
        }
    }
}

#[test]
fn invalid_sort_schema_declarations_fail_on_open() {
    for change in [
        ("/x-language", json!("not_a_locale")),
        ("/properties/names/x-sort-index", json!(true)),
        ("/properties/title/x-sort-reduce", json!("min")),
        (
            "/x-sort-indexes",
            json!([[{"field":"unknown"},{"field":"title"}]]),
        ),
    ] {
        let fixture = tempfile::tempdir().unwrap();
        schema(fixture.path(), "de", Some("de"));
        let path = fixture.path().join(".objekte/collections/de.schema.json");
        let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let (parent, key) = change.0.rsplit_once('/').unwrap();
        value
            .pointer_mut(parent)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(key.into(), change.1);
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(
            Repository::load_repository(fixture.path(), Options::new()).is_err(),
            "{}",
            change.0
        );
    }
}

#[test]
fn indexed_quoted_field_with_separator_sorts_correctly() {
    let fixture = tempfile::tempdir().unwrap();
    schema(fixture.path(), "de", Some("de"));
    let path = fixture.path().join(".objekte/collections/de.schema.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["properties"]["a&&b"] = json!({"type":"string","x-sort-index":true});
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    write(fixture.path(), "de", "01", json!({"rank":1,"a&&b":"Z"}));
    write(fixture.path(), "de", "02", json!({"rank":1,"a&&b":"A"}));
    let mut repository = repo(fixture.path(), CacheMode::Memory, 100);
    let result = repository
        .query_json(&JsonQuery {
            filter: "rank >= 0 SORT BY [\"a&&b\"]".into(),
            collections: Some(vec!["de".into()]),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(ids(&result), ["de/02", "de/01"]);
    assert!(
        result
            .query_plan
            .iter()
            .any(|step| step.contains("ordered sort index"))
    );
}
