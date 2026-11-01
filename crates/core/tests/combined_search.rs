use objekte_core::{CacheMode, Options, Repository, TextMode, TextQuery};
use serde_json::json;
use std::{collections::BTreeMap, fs, path::Path};

fn fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let schema = root.join(".objekte/collections/books.schema.json");
    fs::create_dir_all(schema.parent().unwrap()).unwrap();
    fs::write(schema, json!({
        "type":"object",
        "properties":{
            "year":{"type":"integer","index":true},
            "title":{"type":"string","x-sort-index":true},
            "body":{"type":"string","x-full-text":"markdown"},
            "secret":{"type":"string","exclude_fts":true}
        },
        "x-objekte":{"name":"books","path":"data/books","type":"data","format":"json","id":"filename"}
    }).to_string()).unwrap();
    let docs = root.join("data/books");
    fs::create_dir_all(&docs).unwrap();
    for (id, content) in [
        (
            "a",
            json!({"year":1800,"title":"Zebra","body":"**Water** falls","secret":"match"})
                .to_string(),
        ),
        (
            "b",
            json!({"year":1900,"title":"Äpfel","body":"Water river"}).to_string(),
        ),
        (
            "c",
            json!({"year":1800,"title":"Apple","body":"Dry land"}).to_string(),
        ),
        (
            "d",
            json!({"year":1800,"title":"Auto","body":"water lake"}).to_string(),
        ),
        ("e", "{\"year\":1800,\"body\":\"Water\"".to_owned()),
    ] {
        fs::write(docs.join(format!("{id}.json")), content).unwrap();
    }
    temp
}

fn open(path: &Path, cache: CacheMode) -> Repository {
    let mut options = Options::new();
    options.cache = cache;
    Repository::load_repository(path, options).unwrap()
}

fn ids(repo: &mut Repository, query: &TextQuery) -> Vec<String> {
    repo.search_text(query)
        .unwrap()
        .hits
        .into_iter()
        .map(|h| h.record.id)
        .collect()
}

#[test]
fn filter_runs_before_text_with_cache_and_direct_scan() {
    let temp = fixture();
    for cache in [CacheMode::Memory, CacheMode::Disabled] {
        let mut repo = open(temp.path(), cache);
        let mut query = TextQuery {
            pattern: "Water".into(),
            filter: Some("year = $year".into()),
            parameters: BTreeMap::from([("year".into(), json!(1800))]),
            collections: Some(vec!["books".into()]),
            count: true,
            ..Default::default()
        };
        let result = repo.search_text(&query).unwrap();
        assert_eq!(result.total, Some(2));
        assert_eq!(
            result
                .hits
                .iter()
                .map(|hit| hit.record.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "d"]
        );
        assert!(
            result
                .query_plan
                .iter()
                .any(|step| step.contains("JSON filter"))
        );
        query.mode = TextMode::RegexClean;
        query.pattern = "(?i)water.*".into();
        assert_eq!(ids(&mut repo, &query), ["a", "d"]);
        query.pattern = "Water SORT BY title DESC".into();
        assert_eq!(ids(&mut repo, &query), ["a", "d"]);
        query.pattern = "Water".into();
        query.mode = TextMode::Word;
        query.offset = 1;
        query.limit = 1;
        let result = repo.search_text(&query).unwrap();
        assert_eq!(result.total, Some(2));
        assert_eq!(result.hits[0].record.id, "d");
        let mut streamed = Vec::new();
        let summary = repo
            .search_text_stream(&query, |_, hit| {
                streamed.push(hit.record.id.clone());
                Ok(())
            })
            .unwrap();
        assert_eq!(streamed, ["d"]);
        assert_eq!(summary.total, Some(2));
        if cache == CacheMode::Memory {
            repo.save_bigram_cache("books").unwrap();
            let mut restored = open(temp.path(), CacheMode::Memory);
            let result = restored.search_text(&query).unwrap();
            assert_eq!(result.total, Some(2));
            assert_eq!(result.hits[0].record.id, "d");
            assert_eq!(restored.metrics().bigram_snapshot_loads, 1);
        }
    }
}

#[test]
fn combined_filter_validates_input_and_keeps_excluded_fields_filterable() {
    let temp = fixture();
    let mut repo = open(temp.path(), CacheMode::Memory);
    let mut query = TextQuery {
        pattern: "Water".into(),
        filter: Some("secret = 'match'".into()),
        ..Default::default()
    };
    assert_eq!(ids(&mut repo, &query), ["a"]);
    query.pattern = "match".into();
    assert!(ids(&mut repo, &query).is_empty());
    query.pattern = "Water".into();
    query.filter = Some(" ".into());
    assert!(repo.search_text(&query).is_err());
    query.filter = Some("year = $missing".into());
    assert!(repo.search_text(&query).is_err());
    query.filter = None;
    query.parameters.insert("year".into(), json!(1800));
    assert!(repo.search_text(&query).is_err());
}

#[test]
fn selective_json_index_intersects_broad_full_text_candidates() {
    let temp = fixture();
    let docs = temp.path().join("data/books");
    for index in 0..4100 {
        fs::write(
            docs.join(format!("x{index:04}.json")),
            r#"{"year":1900,"body":"Water"}"#,
        )
        .unwrap();
    }
    let mut repo = open(temp.path(), CacheMode::Memory);
    let query = TextQuery {
        pattern: "Water".into(),
        filter: Some("year = 1800".into()),
        count: true,
        ..Default::default()
    };
    let result = repo.search_text(&query).unwrap();
    assert_eq!(result.total, Some(2));
    assert!(
        result
            .query_plan
            .iter()
            .any(|step| step.contains("JSON filter index candidates"))
    );
    assert!(
        result.metrics.fts_candidates <= 3,
        "{}",
        result.metrics.fts_candidates
    );
    assert_eq!(
        ids(&mut open(temp.path(), CacheMode::Disabled), &query),
        ["a", "d"]
    );
}
