//! Incremental selected postings and generation-bound page work.

use super::review_stage1::JsonAliasProvider;
use super::*;
use objekte_core::{CacheMode, JsonQuery, JsonQueryBackend, Options, Repository, TextQuery};
use std::fs;

#[test]
fn one_edit_reuses_text_and_typed_postings_without_full_decode() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("data/archive");
    fs::create_dir_all(directory.join(".editor")).unwrap();
    fs::write(directory.join(".editor/schema.json"),
        r#"{"type":"object","properties":{"year":{"type":"integer","index":true},"body":{"type":"string","x-full-text":"plain"},"publication_date":{"type":"string"}}}"#).unwrap();
    for slot in 0..64 {
        fs::write(
            directory.join(format!("n{slot:03}.md")),
            r#"{"year":1850,"body":"ordinary text","publication_date":"1850"}"#,
        )
        .unwrap();
    }
    let mut repo = Repository::builder(temp.path())
        .register_source(JsonAliasProvider::new(directory.clone()))
        .unwrap()
        .register_extractor(PublicationYearExtractor::default())
        .unwrap()
        .open()
        .unwrap();
    repo.warm_bigram_index("archive").unwrap();
    repo.build_selected_derived_posting("archive", "example:publication_year")
        .unwrap();
    fs::write(
        directory.join("n063.md"),
        r#"{"year":1848,"body":"revisionneedle text","publication_date":"circa 1848"}"#,
    )
    .unwrap();
    let changed = repo.refresh("archive").unwrap();
    assert_eq!(
        changed.source_reads, 1,
        "one changed source feeds all active projections"
    );
    assert_eq!(changed.parses, 1);
    let query = JsonQuery {
        filter: "year = 1848".into(),
        collections: Some(vec!["archive".into()]),
        count: true,
        limit: 20,
        ..Default::default()
    };
    let result = repo.query_json(&query).unwrap();
    assert_eq!(result.total, Some(1));
    assert_eq!(result.records[0].id, "n063");
    assert!(
        result
            .query_plan
            .iter()
            .any(|step| step.contains("selected-scalar-posting"))
    );
    assert!(result.metrics.source_reads <= 2);
    assert_eq!(
        result.metrics.hashes, 1,
        "only the returned record needs a content digest"
    );
    let text = TextQuery {
        pattern: "revisionneedle".into(),
        fields: Some(vec!["body".into()]),
        collections: Some(vec!["archive".into()]),
        count: true,
        ..Default::default()
    };
    let hit = repo.search_text(&text).unwrap();
    assert_eq!(hit.total, Some(1));
    assert_eq!(hit.hits[0].record.id, "n063");
    assert!(hit.metrics.source_reads < 10);
    assert_eq!(
        repo.derived_value_counts("archive", "example:publication_year")
            .unwrap()
            .into_iter()
            .find(|(year, _)| year == &TypedValue::Number("1848".into()))
            .map(|(_, count)| count),
        Some(1)
    );

    fs::remove_file(directory.join("n000.md")).unwrap();
    fs::rename(directory.join("n001.md"), directory.join("z001.md")).unwrap();
    let changed = repo.refresh("archive").unwrap();
    assert!(
        changed.source_reads <= 1,
        "delete and rename must not decode unchanged sources"
    );
    let all = repo
        .query_json(&JsonQuery {
            filter: "exists(year)".into(),
            collections: Some(vec!["archive".into()]),
            count: true,
            limit: 100,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(all.total, Some(63));
    assert!(all.records.iter().any(|record| record.id == "z001"));
    assert!(
        !all.records
            .iter()
            .any(|record| record.id == "n000" || record.id == "n001")
    );
    repo.save_bigram_cache("archive").unwrap();
    drop(repo);

    let mut restored = Repository::builder(temp.path())
        .register_source(JsonAliasProvider::new(directory.clone()))
        .unwrap()
        .register_extractor(PublicationYearExtractor::default())
        .unwrap()
        .open()
        .unwrap();
    let warm = restored.warm_bigram_index("archive").unwrap();
    assert_eq!(
        warm.source_reads, 0,
        "compatible overlay snapshot restores without source decoding"
    );
    let restored_result = restored.query_json(&query).unwrap();
    assert_eq!(restored_result.total, Some(1));
    assert_eq!(restored.search_text(&text).unwrap().total, Some(1));
    let mut oracle = Repository::builder(temp.path())
        .options(Options {
            cache: CacheMode::Disabled,
            query_cache_bytes: 0,
            ..Default::default()
        })
        .register_source(JsonAliasProvider::new(directory))
        .unwrap()
        .register_extractor(PublicationYearExtractor::default())
        .unwrap()
        .open()
        .unwrap();
    let direct = oracle
        .query_json(&JsonQuery {
            backend: JsonQueryBackend::Direct,
            ..query
        })
        .unwrap();
    assert_eq!(
        restored_result
            .records
            .iter()
            .map(|r| &r.id)
            .collect::<Vec<_>>(),
        direct.records.iter().map(|r| &r.id).collect::<Vec<_>>()
    );

    // A syntactically valid but incorrect base-slot map must be rejected.
    drop(restored);
    let cache_dir = temp.path().join(".objekte/.cache");
    let companion = fs::read_dir(cache_dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .find(|path| {
            if !path.to_string_lossy().ends_with(".selected-meta") {
                return false;
            }
            let Ok(bytes) = fs::read(path) else {
                return false;
            };
            let Ok(meta) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                return false;
            };
            meta["key"]["path"] == "body" && meta["base_to_current"].is_array()
        })
        .unwrap();
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(&companion).unwrap()).unwrap();
    let map = metadata["base_to_current"].as_array_mut().unwrap();
    let live = map
        .iter()
        .enumerate()
        .filter(|(_, slot)| !slot.is_null())
        .map(|(index, _)| index)
        .take(2)
        .collect::<Vec<_>>();
    map.swap(live[0], live[1]);
    fs::write(&companion, serde_json::to_vec(&metadata).unwrap()).unwrap();
    let mut invalid = Repository::builder(temp.path())
        .register_source(JsonAliasProvider::new(temp.path().join("data/archive")))
        .unwrap()
        .register_extractor(PublicationYearExtractor::default())
        .unwrap()
        .open()
        .unwrap();
    let rebuilt = invalid.warm_bigram_index("archive").unwrap();
    assert!(
        rebuilt.source_reads >= 63,
        "invalid slot map must rebuild, not restore"
    );
    assert_eq!(
        invalid
            .query_json(&JsonQuery {
                filter: "year = 1848".into(),
                collections: Some(vec!["archive".into()]),
                count: true,
                ..Default::default()
            })
            .unwrap()
            .total,
        Some(1)
    );
}

#[test]
fn selected_stream_callback_stops_reads_and_does_not_publish_order() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("data/archive");
    fs::create_dir_all(directory.join(".editor")).unwrap();
    fs::write(
        directory.join(".editor/schema.json"),
        r#"{"type":"object","properties":{"body":{"type":"string","x-full-text":"plain"}}}"#,
    )
    .unwrap();
    for slot in 0..48 {
        fs::write(
            directory.join(format!("n{slot:03}.md")),
            r#"{"body":"common target"}"#,
        )
        .unwrap();
    }
    let mut repo = Repository::builder(temp.path())
        .options(Options {
            stream_query_cache: true,
            ..Default::default()
        })
        .register_source(JsonAliasProvider::new(directory))
        .unwrap()
        .open()
        .unwrap();
    repo.warm_bigram_index("archive").unwrap();
    let json = JsonQuery {
        filter: "exists(body)".into(),
        collections: Some(vec!["archive".into()]),
        limit: 5,
        ..Default::default()
    };
    let reads_before = repo.metrics().source_reads;
    let result = repo.query_json_stream(&json, |_, _| Err(Error::Request("stop".into())));
    assert!(matches!(result, Err(Error::Request(message)) if message == "stop"));
    assert!(repo.metrics().source_reads - reads_before <= 2);
    let next = repo.query_json(&json).unwrap();
    assert!(
        !next
            .query_plan
            .iter()
            .any(|step| step.contains("ordered-cache"))
    );

    let text = TextQuery {
        pattern: "target".into(),
        fields: Some(vec!["body".into()]),
        collections: Some(vec!["archive".into()]),
        limit: 5,
        ..Default::default()
    };
    let reads_before = repo.metrics().source_reads;
    let result = repo.search_text_stream(&text, |_, _| Err(Error::Request("stop".into())));
    assert!(matches!(result, Err(Error::Request(message)) if message == "stop"));
    assert!(repo.metrics().source_reads - reads_before <= 2);
    let next = repo.search_text(&text).unwrap();
    assert!(
        !next
            .query_plan
            .iter()
            .any(|step| step.contains("ordered-cache"))
    );
}
