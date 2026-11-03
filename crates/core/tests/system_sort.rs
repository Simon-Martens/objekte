use objekte_core::{
    CacheMode, JsonQuery, JsonQueryBackend, JsonQueryResult, Options, RecordStatus, Repository,
    TextQuery, parse_sort_terms,
};
use serde_json::{Value, json};
use std::{
    fs::{self, File, FileTimes},
    path::Path,
    time::{Duration, UNIX_EPOCH},
};


fn write(root: &Path, id: &str, value: Value, modified: u64) {
    let folder = root.join("data/items");
    fs::create_dir_all(&folder).unwrap();
    let path = folder.join(format!("{id}.json"));
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(modified)))
        .unwrap();
}

/// Four records with distinct sizes and times; `d` violates the schema.
fn fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let folder = root.join(".objekte/collections");
    fs::create_dir_all(&folder).unwrap();
    fs::write(
        folder.join("items.schema.json"),
        serde_json::to_vec_pretty(&json!({
            "type":"object",
            "required":["score"],
            "properties":{
                "title":{"type":"string","x-full-text":"markdown"},
                "score":{"type":"integer"},
                "pad":{"type":"string"}
            },
            "x-objekte":{"name":"items","path":"data/items","type":"data","format":"json","id":"filename"}
        }))
        .unwrap(),
    )
    .unwrap();
    // Sizes grow b < c < a < d; times grow c < a < d < b.
    write(
        root,
        "a",
        json!({"title":"**Zeta**","score":1,"pad":"xxxxxxxxxx"}),
        1002,
    );
    write(root, "b", json!({"title":"_alpha_","score":2}), 1004);
    write(
        root,
        "c",
        json!({"title":"Beta","score":3,"pad":"xxx"}),
        1001,
    );
    write(
        root,
        "d",
        json!({"title":"Delta","score":"bad","pad":"xxxxxxxxxxxxxxxxxxxx"}),
        1003,
    );
    temp
}

fn repo(root: &Path, cache: CacheMode) -> Repository {
    let mut options = Options::new();
    options.cache = cache;
    Repository::load_repository(root, options).unwrap()
}

fn query(repository: &mut Repository, filter: &str, backend: JsonQueryBackend) -> JsonQueryResult {
    repository
        .query_json(&JsonQuery {
            filter: filter.into(),
            collections: Some(vec!["items".into()]),
            backend,
            count: true,
            limit: 10,
            ..Default::default()
        })
        .unwrap()
}

fn ids(result: &JsonQueryResult) -> Vec<&str> {
    result
        .records
        .iter()
        .map(|record| record.id.as_str())
        .collect()
}

/// Run against the indexed and direct backends; both must agree.
fn sorted(root: &Path, filter: &str) -> Vec<String> {
    let mut indexed = repo(root, CacheMode::Memory);
    let mut direct = repo(root, CacheMode::Disabled);
    let a = query(&mut indexed, filter, JsonQueryBackend::Auto);
    let b = query(&mut direct, filter, JsonQueryBackend::Direct);
    assert_eq!(ids(&a), ids(&b), "{filter}");
    assert_eq!(a.total, Some(a.records.len()), "{filter}");
    ids(&a).into_iter().map(str::to_owned).collect()
}


#[test]
fn file_metadata_fields_sort_in_both_directions() {
    let fixture = fixture();
    let root = fixture.path();
    for (filter, expected) in [
        ("exists(title) SORT BY @size", ["b", "c", "a", "d"]),
        ("exists(title) SORT BY @size DESC", ["d", "a", "c", "b"]),
        ("exists(title) SORT BY @modified ASC", ["c", "a", "d", "b"]),
        ("exists(title) SORT BY @filename DESC", ["d", "c", "b", "a"]),
        ("exists(title) SORT BY @path", ["a", "b", "c", "d"]),
        ("exists(title) SORT BY @id DESC", ["d", "c", "b", "a"]),
        // Metadata in the filter takes the materialized-record path.
        ("@size > 0 SORT BY @size DESC", ["d", "a", "c", "b"]),
    ] {
        assert_eq!(sorted(root, filter), expected, "{filter}");
    }
}

#[test]
fn modified_descending_is_the_unsorted_default_order() {
    let fixture = fixture();
    let mut repository = repo(fixture.path(), CacheMode::Memory);
    let sorted = query(
        &mut repository,
        "exists(title) SORT BY @modified DESC",
        JsonQueryBackend::Auto,
    );
    let default = query(&mut repository, "exists(title)", JsonQueryBackend::Auto);
    assert_eq!(ids(&sorted), ["b", "d", "a", "c"]);
    assert_eq!(ids(&sorted), ids(&default));
    assert!(
        !sorted
            .query_plan
            .iter()
            .any(|step| step.contains("sorted page"))
    );
}

#[test]
fn status_sort_uses_validation_state_on_every_path() {
    let fixture = fixture();
    let root = fixture.path();
    // "invalid" orders before "valid"; size breaks no ties here, ID does.
    for filter in ["exists(title) SORT BY @status", "@size > 0 SORT BY @status"] {
        assert_eq!(sorted(root, filter), ["d", "a", "b", "c"], "{filter}");
    }
    assert_eq!(
        sorted(root, "exists(title) SORT BY @status DESC && @size DESC"),
        ["a", "c", "b", "d"]
    );
    let mut repository = repo(root, CacheMode::Memory);
    let result = query(
        &mut repository,
        "exists(title) SORT BY @status",
        JsonQueryBackend::Auto,
    );
    assert_eq!(result.records[0].state.status, RecordStatus::Invalid);
}

#[test]
fn created_sort_is_accepted_without_reordering_matches() {
    let fixture = fixture();
    let mut ids = sorted(fixture.path(), "exists(title) SORT BY @created DESC");
    ids.sort();
    assert_eq!(ids, ["a", "b", "c", "d"]);
}

#[test]
fn fts_sort_orders_rendered_markup_text() {
    let fixture = fixture();
    let root = fixture.path();
    assert_eq!(
        sorted(root, "exists(title) SORT BY title:fts"),
        ["b", "c", "d", "a"]
    );
    assert_eq!(
        sorted(root, "exists(title) SORT BY title:fts DESC"),
        ["a", "d", "c", "b"]
    );
    // Markup characters change the order of the stored strings.
    assert_ne!(
        sorted(root, "exists(title) SORT BY title"),
        ["b", "c", "d", "a"]
    );
    // Without a declared format, `:fts` sorts by the stored value.
    assert_eq!(
        sorted(root, "exists(title) SORT BY pad:fts"),
        sorted(root, "exists(title) SORT BY pad")
    );
    let mut repository = repo(root, CacheMode::Memory);
    let result = repository
        .search_text(&TextQuery {
            pattern: "a".into(),
            sort: parse_sort_terms("title:fts").unwrap(),
            collections: Some(vec!["items".into()]),
            count: true,
            limit: 10,
            ..Default::default()
        })
        .unwrap();
    let hits = result
        .hits
        .iter()
        .map(|hit| hit.record.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(hits, ["b", "c", "d", "a"]);
    let result = repository
        .search_text(&TextQuery {
            pattern: "a".into(),
            sort: parse_sort_terms("@size DESC").unwrap(),
            collections: Some(vec!["items".into()]),
            limit: 10,
            ..Default::default()
        })
        .unwrap();
    let hits = result
        .hits
        .iter()
        .map(|hit| hit.record.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(hits, ["d", "a", "c", "b"]);
}

#[test]
fn unknown_system_fields_and_modifiers_are_rejected() {
    assert!(parse_sort_terms("@unknown").is_err());
    assert!(parse_sort_terms("title:lower").is_err());
    assert!(parse_sort_terms("@size && @size DESC").is_err());
    let terms = parse_sort_terms("@status DESC && title:fts").unwrap();
    assert_eq!(terms[0].field, "@status");
    assert_eq!(terms[1].field, "title:fts");
}


#[test]
fn metadata_filtered_sorts_parse_once_and_return_malformed_sources() {
    let fixture = fixture();
    let root = fixture.path();
    let broken = b"{\"title\": \"unterminated".to_vec();
    fs::write(root.join("data/items/e.json"), &broken).unwrap();
    for filter in [
        "@size > 0 SORT BY @size DESC",
        "@size > 0 SORT BY @status && @id",
    ] {
        let mut indexed = repo(root, CacheMode::Memory);
        let mut direct = repo(root, CacheMode::Disabled);
        let fast = query(&mut indexed, filter, JsonQueryBackend::Auto);
        let slow = query(&mut direct, filter, JsonQueryBackend::Direct);
        assert_eq!(ids(&fast), ids(&slow), "{filter}");
        assert_eq!(fast.total, Some(5), "{filter}");
        // Each matching source is parsed once; none is materialized again.
        assert_eq!(fast.metrics.parses, 5, "{filter}");
        for result in [&fast, &slow] {
            let record = result
                .records
                .iter()
                .find(|record| record.id == "e")
                .unwrap();
            assert_eq!(record.state.status, RecordStatus::IncorrectJson);
            assert_eq!(record.raw.as_deref(), Some(broken.as_slice()));
            let valid = result
                .records
                .iter()
                .find(|record| record.id == "a")
                .unwrap();
            assert_eq!(valid.state.status, RecordStatus::Valid);
        }
    }
    let mut indexed = repo(root, CacheMode::Memory);
    let status = query(
        &mut indexed,
        "@size > 0 SORT BY @status && @id",
        JsonQueryBackend::Auto,
    );
    assert_eq!(ids(&status), ["e", "d", "a", "b", "c"]);
    // A size sort validates only retained records, not every match.
    let mut indexed = repo(root, CacheMode::Memory);
    let size = indexed
        .query_json(&JsonQuery {
            filter: "@size > 0 SORT BY @size DESC".into(),
            collections: Some(vec!["items".into()]),
            limit: 1,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(ids(&size), ["d"]);
    assert!(size.metrics.validations < 4, "{}", size.metrics.validations);
}
