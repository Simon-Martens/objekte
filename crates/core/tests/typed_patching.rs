use objekte_core::{
    CacheMode, JsonQuery, JsonQueryBackend, JsonQueryResult, Options, Repository, SourceChange,
};
use serde_json::{Value, json};
use std::{fs, path::Path};


const FILES: usize = 200;


fn record(index: usize) -> Value {
    json!({
        "year": 1800 + index % 50,
        "flag": index.is_multiple_of(3),
        "tags": [format!("t{}", index % 7)],
        "meta": {"rank": index % 10}
    })
}

fn write(root: &Path, id: &str, value: &Value) {
    fs::write(
        root.join(format!("data/books/{id}.json")),
        value.to_string(),
    )
    .unwrap();
}

/// Indexed `year` and `flag`, plus runtime postings for `tags[*]` and `meta.rank`.
fn fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join(".objekte/collections")).unwrap();
    fs::create_dir_all(root.join("data/books")).unwrap();
    fs::write(
        root.join(".objekte/collections/books.schema.json"),
        json!({
            "type":"object",
            "properties":{
                "year":{"type":"integer","index":true},
                "flag":{"type":"boolean","index":true},
                "tags":{"type":"array","items":{"type":"string"}},
                "meta":{"type":"object","properties":{"rank":{"type":"integer"}}}
            },
            "x-index-paths":["tags[*]","meta.rank"],
            "x-objekte":{"name":"books","path":"data/books","type":"data","format":"json","id":"filename"}
        })
        .to_string(),
    )
    .unwrap();
    for index in 0..FILES {
        write(root, &format!("b{index:03}"), &record(index));
    }
    temp
}

fn repo(root: &Path, cache: CacheMode) -> Repository {
    let mut options = Options::new();
    options.cache = cache;
    Repository::load_repository(root, options).unwrap()
}

fn query(
    repository: &mut Repository,
    filter: &str,
    backend: JsonQueryBackend,
    limit: usize,
) -> JsonQueryResult {
    repository
        .query_json(&JsonQuery {
            filter: filter.into(),
            collections: Some(vec!["books".into()]),
            backend,
            count: true,
            limit,
            ..Default::default()
        })
        .unwrap_or_else(|error| panic!("{filter}: {error}"))
}

fn ids(result: &JsonQueryResult) -> Vec<String> {
    let mut ids = result
        .records
        .iter()
        .map(|record| record.id.clone())
        .collect::<Vec<_>>();
    ids.sort();
    ids
}

const FILTERS: [&str; 14] = [
    "year = 1801",
    "year > 1840",
    "year >= 1810 && year < 1812",
    "year = 1900",
    "missing(year)",
    "exists(year)",
    "flag = true",
    "flag = false && year = 1803",
    "tags ?= 't3'",
    "tags ?= 'new'",
    "exists(tags)",
    "meta.rank = 9",
    "meta.rank > 7",
    "missing(meta.rank)",
];

/// Every filter matches a direct scan of the current files.
fn assert_matches_direct(repository: &mut Repository, root: &Path) {
    let mut direct = repo(root, CacheMode::Disabled);
    for filter in FILTERS {
        let fast = query(repository, filter, JsonQueryBackend::Auto, 500);
        let slow = query(&mut direct, filter, JsonQueryBackend::Direct, 500);
        assert_eq!(fast.total, slow.total, "{filter}");
        assert_eq!(ids(&fast), ids(&slow), "{filter}");
    }
}

/// Counted single-page queries that an exact index answers read only the page.
fn assert_exact(repository: &mut Repository) {
    for filter in [
        "year > 1840",
        "flag = true",
        "missing(year)",
        "exists(meta.rank)",
        "meta.rank = 9",
    ] {
        let result = query(repository, filter, JsonQueryBackend::Auto, 1);
        assert!(
            result.metrics.source_reads <= 1,
            "{filter} read {} files: {:?}",
            result.metrics.source_reads,
            result.query_plan
        );
    }
}

/// Edits of every kind: changed values, a removed field, repeated and new
/// array values, a malformed file, and with `structural`, a deletion and a
/// new file. A slot-table change forces a full rebuild before saving.
fn edit(root: &Path, structural: bool) -> Vec<SourceChange> {
    write(
        root,
        "b001",
        &json!({"year": 1900, "flag": false, "tags": ["new", "new", "t3"], "meta": {"rank": 9}}),
    );
    write(root, "b002", &json!({"flag": true, "tags": [], "meta": {}}));
    write(
        root,
        "b003",
        &json!({"year": 1841, "flag": true, "tags": "new", "meta": {"rank": 8}}),
    );
    fs::write(root.join("data/books/b004.json"), "{\"year\": 1801,").unwrap();
    let mut ids = vec!["b001", "b002", "b003", "b004"];
    if structural {
        fs::remove_file(root.join("data/books/b005.json")).unwrap();
        write(
            root,
            "b999",
            &json!({"year": 1801, "flag": true, "tags": ["t3"], "meta": {"rank": 9}}),
        );
        ids.extend(["b005", "b999"]);
    }
    ids.into_iter()
        .map(|id| SourceChange::Changed(format!("data/books/{id}.json").into()))
        .collect()
}


#[test]
fn refreshed_files_are_patched_into_typed_and_runtime_postings() {
    let fixture = fixture();
    let root = fixture.path();
    let mut repository = repo(root, CacheMode::Memory);
    assert_matches_direct(&mut repository, root);
    assert_exact(&mut repository);
    edit(root, true);
    repository.refresh("books").unwrap();
    assert_matches_direct(&mut repository, root);
    // Exact counts and runtime postings stay in use after edits.
    assert_exact(&mut repository);
    // A second edit of an already changed file moves it again.
    write(root, "b001", &record(1));
    repository.refresh("books").unwrap();
    assert_matches_direct(&mut repository, root);
    assert_exact(&mut repository);
}

#[test]
fn reported_changes_patch_postings_too() {
    let fixture = fixture();
    let root = fixture.path();
    let mut repository = repo(root, CacheMode::Memory);
    assert_matches_direct(&mut repository, root);
    let changes = edit(root, true);
    repository.apply_source_changes(&changes).unwrap();
    assert_matches_direct(&mut repository, root);
    assert_exact(&mut repository);
}

#[test]
fn patched_postings_survive_a_saved_cache() {
    let fixture = fixture();
    let root = fixture.path();
    let mut repository = repo(root, CacheMode::Memory);
    repository.save_bigram_cache("books").unwrap();
    edit(root, false);
    // The change API rewrites the existing cache with patched postings.
    let report = repository.diff("books").unwrap();
    assert!(report.saved_cache.is_some());
    let mut reopened = repo(root, CacheMode::Memory);
    assert_matches_direct(&mut reopened, root);
    assert_exact(&mut reopened);
    assert_eq!(reopened.metrics().secondary_snapshot_loads, 1);
}

#[test]
fn a_new_handle_diffing_a_saved_cache_patches_postings() {
    let fixture = fixture();
    let root = fixture.path();
    repo(root, CacheMode::Memory)
        .save_bigram_cache("books")
        .unwrap();
    edit(root, false);
    let mut repository = repo(root, CacheMode::Memory);
    let report = repository.diff("books").unwrap();
    assert_eq!(report.changes.len(), 4);
    assert_matches_direct(&mut repository, root);
    assert_exact(&mut repository);
}
