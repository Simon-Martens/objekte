use objekte_core::{
    CacheMode, JsonFilter, JsonQuery, JsonQueryBackend, Options, RecordStatus, Repository,
    TextQuery,
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs::{self, File, FileTimes},
    path::Path,
    time::{Duration, UNIX_EPOCH},
};

fn fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join(".objekte/collections")).unwrap();
    fs::create_dir_all(root.join("data/books")).unwrap();
    fs::write(root.join(".objekte/collections/books.schema.json"), json!({
        "type":"object",
        "properties":{"x":{"type":"integer"},"body":{"type":"string"}},
        "x-objekte":{"name":"books","path":"data/books","type":"data","format":"json","id":"filename"}
    }).to_string()).unwrap();
    for (id, text, seconds) in [
        (
            "may",
            json!({"x":1,"@size":7,"body":format!("needle {}", "x".repeat(2000))}).to_string(),
            1_778_803_200,
        ),
        (
            "june",
            json!({"x":"bad","body":"needle"}).to_string(),
            1_780_272_000,
        ),
        ("broken", "{broken needle".to_owned(), 1_777_507_200),
    ] {
        let path = root.join(format!("data/books/{id}.json"));
        fs::write(&path, text).unwrap();
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(seconds)))
            .unwrap();
    }
    temp
}

fn repo(root: &Path, cache: CacheMode) -> Repository {
    let mut options = Options::new();
    options.cache = cache;
    Repository::load_repository(root, options).unwrap()
}

fn query(filter: &str, backend: JsonQueryBackend) -> JsonQuery {
    JsonQuery {
        filter: filter.into(),
        collections: Some(vec!["books".into()]),
        backend,
        count: true,
        limit: 20,
        ..Default::default()
    }
}

#[test]
fn metadata_filters_match_across_all_backends() {
    let fixture = fixture();
    let created_available = fs::metadata(fixture.path().join("data/books/may.json"))
        .unwrap()
        .created()
        .is_ok();
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("@id = 'june'", vec!["june"]),
        ("@filename ~ 'may%'", vec!["may"]),
        ("@path ~ 'data/books/%'", vec!["broken", "june", "may"]),
        ("@size > 1KB", vec!["may"]),
        ("@size >= 1.5KB", vec!["may"]),
        ("@size < 1KiB", vec!["broken", "june"]),
        ("@size < 1.5MB", vec!["broken", "june", "may"]),
        ("@modified = 2026-05", vec!["may"]),
        ("@modified < 2026-05", vec!["broken"]),
        ("@modified <= 2026-05", vec!["broken", "may"]),
        ("@modified > 2026-05", vec!["june"]),
        ("2026-05 < @modified", vec!["june"]),
        ("@modified >= 2026-05", vec!["june", "may"]),
        ("@modified = 2026-05-15T00:00Z", vec!["may"]),
        ("@modified = 2026-05-15T01:00+01:00", vec!["may"]),
        ("@status = 'valid'", vec!["may"]),
        ("@status = 'invalid'", vec!["june"]),
        ("@status = 'incorrect_json'", vec!["broken"]),
        ("@status = 'pending'", vec![]),
        ("@modified != null", vec!["broken", "june", "may"]),
        (
            "@status != 'incorrect_json' && @size >= 1B",
            vec!["june", "may"],
        ),
        ("x = 1 && @modified = 2026-05", vec!["may"]),
        ("@status = 'incorrect_json' || x = 1", vec!["broken", "may"]),
        ("['@size'] = 7", vec!["may"]),
        (
            "@created = null",
            if created_available {
                vec![]
            } else {
                vec!["broken", "june", "may"]
            },
        ),
        (
            "@created != null",
            if created_available {
                vec!["broken", "june", "may"]
            } else {
                vec![]
            },
        ),
        (
            "@created < 9999-12",
            if created_available {
                vec!["broken", "june", "may"]
            } else {
                vec![]
            },
        ),
    ];
    for (filter, expected) in cases {
        for (backend, cache) in [
            (JsonQueryBackend::Direct, CacheMode::Disabled),
            (JsonQueryBackend::Grep, CacheMode::Disabled),
            (JsonQueryBackend::Bigram, CacheMode::Disabled),
            (JsonQueryBackend::Bigram, CacheMode::Memory),
        ] {
            let mut repo = repo(fixture.path(), cache);
            let result = repo.query_json(&query(filter, backend)).unwrap();
            let mut ids = result
                .records
                .iter()
                .map(|record| record.id.as_str())
                .collect::<Vec<_>>();
            ids.sort_unstable();
            assert_eq!(ids, expected, "{filter} {backend:?} {cache:?}");
            assert_eq!(result.total, Some(expected.len()), "{filter} {backend:?}");
            if filter == "@status = 'incorrect_json'" {
                assert_eq!(result.records[0].state.status, RecordStatus::IncorrectJson);
                assert!(result.records[0].raw.is_some());
            }
        }
    }
}

#[test]
fn metadata_filters_keep_sorted_and_streamed_results_consistent() {
    let fixture = fixture();
    for backend in [
        JsonQueryBackend::Direct,
        JsonQueryBackend::Grep,
        JsonQueryBackend::Bigram,
    ] {
        let mut repo = repo(fixture.path(), CacheMode::Memory);
        let query = query("@status = 'incorrect_json' || x = 1 SORT BY x ASC", backend);
        let result = repo.query_json(&query).unwrap();
        let mut streamed = Vec::new();
        let summary = repo
            .query_json_stream(&query, |_, record| {
                streamed.push((record.id.clone(), record.state.status));
                Ok(())
            })
            .unwrap();
        assert_eq!(summary.total, Some(2));
        assert_eq!(
            streamed,
            result
                .records
                .iter()
                .map(|record| (record.id.clone(), record.state.status))
                .collect::<Vec<_>>()
        );
        assert!(
            streamed
                .iter()
                .any(|(id, status)| id == "broken" && *status == RecordStatus::IncorrectJson)
        );
    }
}

#[test]
fn metadata_filter_combines_with_text_and_rejects_bad_literals() {
    let fixture = fixture();
    let mut repo = repo(fixture.path(), CacheMode::Memory);
    let result = repo
        .search_text(&TextQuery {
            pattern: "needle".into(),
            filter: Some("@status = 'incorrect_json'".into()),
            collections: Some(vec!["books".into()]),
            count: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].record.id, "broken");
    for filter in [
        "@size > 1XB",
        "@size > 1.1KiB",
        "@modified = 2026-13",
        "@modified = 2023-02-29",
        "@created > 2026-05-01T12:00",
        "x = 1KB",
        "x = 2026-05",
    ] {
        assert!(
            JsonFilter::compile(filter, &BTreeMap::new()).is_err(),
            "{filter}"
        );
    }
    assert!(
        JsonFilter::compile("@size > 1KB", &BTreeMap::new())
            .unwrap()
            .matches(&json!({}))
            .is_err()
    );
}

#[test]
fn refresh_updates_size_and_modification_metadata() {
    let fixture = fixture();
    let mut repo = repo(fixture.path(), CacheMode::Memory);
    assert_eq!(
        repo.query_json(&query(
            "@size > 1KB && @modified = 2026-05",
            JsonQueryBackend::Bigram
        ))
        .unwrap()
        .total,
        Some(1)
    );
    let path = fixture.path().join("data/books/may.json");
    fs::write(&path, r#"{"x":1,"body":"short"}"#).unwrap();
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(1_780_272_000)))
        .unwrap();
    repo.refresh("books").unwrap();
    assert_eq!(
        repo.query_json(&query("@size > 1KB", JsonQueryBackend::Bigram))
            .unwrap()
            .total,
        Some(0)
    );
    assert_eq!(
        repo.query_json(&query("@modified = 2026-05", JsonQueryBackend::Bigram))
            .unwrap()
            .total,
        Some(0)
    );
    assert_eq!(
        repo.query_json(&query("@modified = 2026-06", JsonQueryBackend::Bigram))
            .unwrap()
            .total,
        Some(2)
    );
}

#[test]
fn filename_uses_basename_for_nested_ids_and_status_handles_schema_free_files() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join(".objekte/collections")).unwrap();
    fs::create_dir_all(root.join("data/books/part")).unwrap();
    fs::create_dir_all(root.join("data/notes")).unwrap();
    fs::write(
        root.join(".objekte/collections/books.schema.json"),
        json!({
            "type":"object",
            "x-objekte":{"name":"books","path":"data/books","type":"data","format":"json","id":"filename"}
        })
        .to_string(),
    )
    .unwrap();
    fs::write(root.join("data/books/part/a.json"), "{}").unwrap();
    fs::write(root.join("data/notes/n.json"), "{}").unwrap();
    for backend in [
        JsonQueryBackend::Direct,
        JsonQueryBackend::Grep,
        JsonQueryBackend::Bigram,
    ] {
        let mut repo = repo(root, CacheMode::Memory);
        let nested = JsonQuery {
            filter: "@id = 'part/a' && @filename = 'a.json' && @path = 'data/books/part/a.json'"
                .into(),
            collections: Some(vec!["books".into()]),
            backend,
            count: true,
            ..Default::default()
        };
        let found = repo.query_json(&nested).unwrap();
        assert_eq!(found.total, Some(1), "{backend:?}");
        assert_eq!(found.records[0].id, "part/a");
        let schema_free = JsonQuery {
            filter: "@status = 'no_schema'".into(),
            collections: Some(vec!["notes".into()]),
            backend,
            count: true,
            ..Default::default()
        };
        let found = repo.query_json(&schema_free).unwrap();
        assert_eq!(found.total, Some(1), "{backend:?}");
        assert_eq!(found.records[0].state.status, RecordStatus::NoSchema);
    }
}
