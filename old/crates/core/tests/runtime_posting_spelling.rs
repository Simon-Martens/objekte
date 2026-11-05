use objekte_core::{CacheMode, JsonQuery, JsonQueryBackend, JsonQueryResult, Options, Repository};
use serde_json::{Value, json};
use std::{fs, path::Path};


fn write(root: &Path, id: &str, value: &Value) {
    fs::write(
        root.join(format!("data/books/{id}.json")),
        value.to_string(),
    )
    .unwrap();
}

/// Runtime postings declared with `[*]`; all files hold arrays there.
fn fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join(".objekte/collections")).unwrap();
    fs::create_dir_all(root.join("data/books")).unwrap();
    fs::write(
        root.join(".objekte/collections/books.schema.json"),
        json!({
            "type":"object",
            "x-index-paths":["tags[*]","people[*].role"],
            "x-objekte":{"name":"books","path":"data/books","type":"data","format":"json","id":"filename"}
        })
        .to_string(),
    )
    .unwrap();
    for index in 0..60 {
        write(
            root,
            &format!("b{index:02}"),
            &json!({
                "tags": [format!("t{}", index % 5), "all"],
                "people": [{"role": if index % 4 == 0 { "editor" } else { "author" }}],
            }),
        );
    }
    write(root, "empty", &json!({"tags": [], "people": []}));
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
            collections: Some(vec!["books".into()]),
            backend,
            count: true,
            limit: 1,
            ..Default::default()
        })
        .unwrap_or_else(|error| panic!("{filter}: {error}"))
}

const FILTERS: [&str; 8] = [
    "tags ?= 't3'",
    "tags[*] ?= 't3'",
    "tags ?= 'all'",
    "tags ?~ 't%'",
    "people.role ?= 'editor'",
    "people[*].role ?= 'editor'",
    "exists(tags)",
    "exists(tags[*])",
];

/// Totals agree with a direct scan; returns the files each query read.
fn reads(repository: &mut Repository, root: &Path) -> Vec<(String, u64)> {
    let mut direct = repo(root, CacheMode::Disabled);
    FILTERS
        .iter()
        .map(|filter| {
            let fast = query(repository, filter, JsonQueryBackend::Auto);
            let slow = query(&mut direct, filter, JsonQueryBackend::Direct);
            assert_eq!(fast.total, slow.total, "{filter}");
            ((*filter).to_owned(), fast.metrics.source_reads)
        })
        .collect()
}

fn read_count(reads: &[(String, u64)], filter: &str) -> u64 {
    reads.iter().find(|(name, _)| name == filter).unwrap().1
}


#[test]
fn implicit_spelling_uses_an_equivalent_star_posting() {
    let fixture = fixture();
    let root = fixture.path();
    let mut repository = repo(root, CacheMode::Memory);
    let reads = reads(&mut repository, root);
    for filter in [
        "tags ?= 't3'",
        "tags[*] ?= 't3'",
        "tags ?= 'all'",
        "people.role ?= 'editor'",
        "people[*].role ?= 'editor'",
        "exists(tags[*])",
    ] {
        assert!(read_count(&reads, filter) <= 1, "{filter}: {reads:?}");
    }
    // `exists(tags)` also holds for an empty array, which `tags[*]` lacks.
    assert_eq!(
        query(&mut repository, "exists(tags)", JsonQueryBackend::Auto).total,
        Some(61)
    );
    assert_eq!(
        query(&mut repository, "exists(tags[*])", JsonQueryBackend::Auto).total,
        Some(60)
    );
}

#[test]
fn a_non_array_value_disables_the_alias_but_not_correctness() {
    let fixture = fixture();
    let root = fixture.path();
    // A plain string is reached by `tags` but not by `tags[*]`, and an
    // object by `people.role` but not by `people[*].role`.
    write(
        root,
        "plain",
        &json!({"tags": "t3", "people": {"role": "editor"}}),
    );
    let mut repository = repo(root, CacheMode::Memory);
    let reads = reads(&mut repository, root);
    assert!(read_count(&reads, "tags[*] ?= 't3'") <= 1, "{reads:?}");
    assert!(read_count(&reads, "tags ?= 't3'") > 1, "{reads:?}");
    assert!(
        read_count(&reads, "people.role ?= 'editor'") > 1,
        "{reads:?}"
    );
}

#[test]
fn edits_keep_equivalence_current() {
    let fixture = fixture();
    let root = fixture.path();
    let mut repository = repo(root, CacheMode::Memory);
    reads(&mut repository, root);
    // An array edit keeps the alias.
    write(
        root,
        "b01",
        &json!({"tags": ["t3", "new"], "people": [{"role": "editor"}]}),
    );
    repository.refresh("books").unwrap();
    let after = reads(&mut repository, root);
    assert!(read_count(&after, "tags ?= 't3'") <= 1, "{after:?}");
    // A scalar edit breaks equivalence; results stay correct.
    write(
        root,
        "b02",
        &json!({"tags": "t3", "people": [{"role": "author"}]}),
    );
    repository.refresh("books").unwrap();
    let broken = reads(&mut repository, root);
    assert!(read_count(&broken, "tags ?= 't3'") > 1, "{broken:?}");
    assert!(read_count(&broken, "tags[*] ?= 't3'") <= 1, "{broken:?}");
    // Once the file is an array again, a rebuild restores the alias.
    write(
        root,
        "b02",
        &json!({"tags": ["t3"], "people": [{"role": "author"}]}),
    );
    repository.rebuild_bigram_index("books").unwrap();
    let rebuilt = reads(&mut repository, root);
    assert!(read_count(&rebuilt, "tags ?= 't3'") <= 1, "{rebuilt:?}");
}
