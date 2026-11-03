use objekte_core::{
    CacheMode, JsonQuery, JsonQueryBackend, JsonQueryResult, Options, Repository, SourceChange,
    TextQuery,
};
use serde_json::{Value, json};
use std::{fs, path::Path};


fn write(root: &Path, relative: &str, value: Value) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
}

fn schema(root: &Path, name: &str, properties: Value) {
    write(
        root,
        &format!(".objekte/collections/{name}.schema.json"),
        json!({
            "type":"object",
            "properties": properties,
            "x-objekte":{"name":name,"path":format!("data/{name}"),"type":"data","format":"json","id":"filename"}
        }),
    );
}

/// `books` links to `people` (indexed scalar, array, and multi-hop) and to
/// `publishers` (unindexed). `loose` has no schema, so nothing is followed.
fn fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    schema(
        root,
        "books",
        json!({
            "title":{"type":"string"},
            "year":{"type":"integer","index":true},
            "author":{"type":"string","index":true,"x-relation":{"collection":"people"}},
            "editors":{"type":"array","items":{"type":"string","x-relation":{"collection":"people"}}},
            "publisher":{"type":"string","x-relation":{"collection":"publishers"}}
        }),
    );
    schema(
        root,
        "people",
        json!({
            "name":{"type":"string"},
            "born":{"type":"integer"},
            "since":{"type":"string"},
            "bio":{"type":"string","x-full-text":"markdown"},
            "aliases":{"type":"array","items":{"type":"string"}},
            "city":{"type":"string","x-relation":{"collection":"places"}}
        }),
    );
    schema(root, "places", json!({"name":{"type":"string"}}));
    schema(root, "publishers", json!({"name":{"type":"string"}}));
    for (id, record) in [
        (
            "ada",
            json!({"name":"Ada","born":1815,"since":"1990-05-01","bio":"**Count** ess","aliases":["A"],"city":"london"}),
        ),
        (
            "bert",
            json!({"name":"Bert","born":1790,"since":"2001-01-01","bio":"Plain","aliases":["B","Bee"],"city":"berlin"}),
        ),
        (
            "cleo",
            json!({"name":"cleo","bio":"_Zeta_","city":"berlin"}),
        ),
    ] {
        write(root, &format!("data/people/{id}.json"), record);
    }
    write(root, "data/places/london.json", json!({"name":"London"}));
    write(root, "data/places/berlin.json", json!({"name":"Berlin"}));
    write(
        root,
        "data/publishers/acme.json",
        json!({"name":"Acme Press"}),
    );
    write(
        root,
        "data/publishers/zed.json",
        json!({"name":"Zed Books"}),
    );
    for (id, record) in [
        (
            "b1",
            json!({"title":"One","year":1840,"author":"ada","editors":["bert"],"publisher":"acme"}),
        ),
        (
            "b2",
            json!({"title":"Two","year":1800,"author":"bert","editors":["ada","cleo"],"publisher":"zed"}),
        ),
        (
            "b3",
            json!({"title":"Three","year":1900,"author":"cleo","editors":[],"publisher":"acme"}),
        ),
        ("b4", json!({"title":"Four","year":1850,"author":"ghost"})),
        // A declared relation follows IDs only, not this invalid object.
        (
            "b5",
            json!({"title":"Five","year":1860,"author":{"name":"Inline","born":1700}}),
        ),
        ("b6", json!({"title":"Six","year":1870})),
    ] {
        write(root, &format!("data/books/{id}.json"), record);
    }
    write(root, "data/loose/l1.json", json!({"author":{"name":"Ada"}}));
    write(root, "data/loose/l2.json", json!({"author":"ada"}));
    temp
}

fn repo(root: &Path, cache: CacheMode) -> Repository {
    let mut options = Options::new();
    options.cache = cache;
    Repository::load_repository(root, options).unwrap()
}

fn run(
    repository: &mut Repository,
    filter: &str,
    scope: Option<&str>,
    backend: JsonQueryBackend,
) -> JsonQueryResult {
    repository
        .query_json(&JsonQuery {
            filter: filter.into(),
            collections: scope.map(|name| vec![name.into()]),
            backend,
            count: true,
            limit: 50,
            ..Default::default()
        })
        .unwrap_or_else(|error| panic!("{filter}: {error}"))
}

fn ids(result: &JsonQueryResult) -> Vec<String> {
    result
        .records
        .iter()
        .map(|record| format!("{}/{}", record.collection, record.id))
        .collect()
}

/// Matching `collection/id`s, checked against a direct scan without indexes.
fn matching(root: &Path, filter: &str, scope: Option<&str>) -> Vec<String> {
    let sorted = format!("{filter} SORT BY @id");
    let mut indexed = repo(root, CacheMode::Memory);
    let mut direct = repo(root, CacheMode::Disabled);
    let fast = run(&mut indexed, &sorted, scope, JsonQueryBackend::Auto);
    let slow = run(&mut direct, &sorted, scope, JsonQueryBackend::Direct);
    assert_eq!(ids(&fast), ids(&slow), "{filter}");
    assert_eq!(fast.total, Some(fast.records.len()), "{filter}");
    // The unsorted indexed path must agree with the sorted one.
    let mut unsorted = ids(&run(&mut indexed, filter, scope, JsonQueryBackend::Auto));
    unsorted.sort();
    assert_eq!(unsorted, ids(&fast), "{filter}");
    ids(&fast)
}

fn books(names: &[&str]) -> Vec<String> {
    names.iter().map(|id| format!("books/{id}")).collect()
}


#[test]
fn comparisons_follow_declared_relations() {
    let fixture = fixture();
    let root = fixture.path();
    for (filter, expected) in [
        ("author.name = 'Ada'", books(&["b1"])),
        ("author.name ~ :ignorecase('%e%')", books(&["b2", "b3"])),
        ("author.born >= 1800", books(&["b1"])),
        ("author.since = 1990-05", books(&["b1"])),
        ("author.since < 2000-01", books(&["b1"])),
        // A bare year is a number, not a date literal.
        ("author.since < 2000", books(&[])),
        // Dangling IDs and records without a relation have no linked value.
        ("author.name != 'Ada'", books(&["b2", "b3"])),
        // The relation field itself still compares the stored ID.
        ("author = 'ada'", books(&["b1"])),
        ("author.city.name = 'Berlin'", books(&["b2", "b3"])),
        ("publisher.name ~ '%Press%'", books(&["b1", "b3"])),
        ("author.name = 'Ada' || year = 1900", books(&["b1", "b3"])),
        ("author.name = 'Inline'", books(&[])),
        ("author.name = 'Inline' || title = 'Five'", books(&["b5"])),
        ("author.city.name = 'Berlin' && year < 1850", books(&["b2"])),
    ] {
        assert_eq!(matching(root, filter, Some("books")), expected, "{filter}");
    }
}

#[test]
fn arrays_presence_and_field_comparisons_follow_relations() {
    let fixture = fixture();
    let root = fixture.path();
    for (filter, expected) in [
        ("editors.name ?= 'Ada'", books(&["b2"])),
        ("editors.name ?= 'Bert'", books(&["b1"])),
        // Cleo has no `born`, so Ada's 1815 is b2's only value.
        ("editors.born:each > 1800", books(&["b2"])),
        ("editors.born:each < 1800", books(&["b1"])),
        ("editors.city.name ?= 'Berlin'", books(&["b1", "b2"])),
        ("exists(author.born)", books(&["b1", "b2"])),
        ("missing(author.born)", books(&["b3", "b4", "b5", "b6"])),
        ("exists(editors.name)", books(&["b1", "b2"])),
        ("year > author.born", books(&["b1", "b2"])),
        (
            "author.born < year && author.city.name = 'London'",
            books(&["b1"]),
        ),
        ("author.aliases ?= 'Bee'", books(&["b2"])),
    ] {
        assert_eq!(matching(root, filter, Some("books")), expected, "{filter}");
    }
}

#[test]
fn rendered_text_and_cardinality_rules_apply_in_linked_records() {
    let fixture = fixture();
    let root = fixture.path();
    // `bio` is Markdown in `people`, so `:fts` renders it there.
    assert_eq!(
        matching(root, "author.bio:fts ~ '%Count ess%'", Some("books")),
        books(&["b1"])
    );
    assert_eq!(
        matching(root, "author.bio ~ '%Count ess%'", Some("books")),
        books(&[])
    );
    assert_eq!(
        matching(root, "author.bio:fts = 'Zeta'", Some("books")),
        books(&["b3"])
    );
    // Several linked values need `?` or `:each`, as for embedded arrays.
    for filter in ["editors.name = 'Ada'", "author.aliases = 'B'"] {
        for (cache, backend) in [
            (CacheMode::Memory, JsonQueryBackend::Auto),
            (CacheMode::Disabled, JsonQueryBackend::Direct),
        ] {
            let error = repo(root, cache)
                .query_json(&JsonQuery {
                    filter: filter.into(),
                    collections: Some(vec!["books".into()]),
                    backend,
                    count: true,
                    ..Default::default()
                })
                .unwrap_err();
            assert!(
                error.to_string().contains("multiple values"),
                "{filter}: {error}"
            );
        }
    }
}

#[test]
fn collections_without_a_schema_use_ordinary_traversal() {
    let fixture = fixture();
    let root = fixture.path();
    // `loose` has no schema: its string ID is not followed.
    assert_eq!(
        matching(root, "author.name = 'Ada'", Some("loose")),
        ["loose/l1"]
    );
    assert_eq!(
        matching(root, "author = 'ada'", Some("loose")),
        ["loose/l2"]
    );
    // An unscoped query decides per collection.
    assert_eq!(
        matching(root, "author.name = 'Ada'", None),
        ["books/b1", "loose/l1"]
    );
}

#[test]
fn sorting_reads_linked_values() {
    let fixture = fixture();
    let root = fixture.path();
    let sorted = |filter: &str| {
        let mut indexed = repo(root, CacheMode::Memory);
        let mut direct = repo(root, CacheMode::Disabled);
        let fast = ids(&run(
            &mut indexed,
            filter,
            Some("books"),
            JsonQueryBackend::Auto,
        ));
        let slow = ids(&run(
            &mut direct,
            filter,
            Some("books"),
            JsonQueryBackend::Direct,
        ));
        assert_eq!(fast, slow, "{filter}");
        fast
    };
    // Case-insensitive collation; missing linked values sort last.
    assert_eq!(
        sorted("exists(title) SORT BY author.name"),
        books(&["b1", "b2", "b3", "b4", "b5", "b6"])
    );
    assert_eq!(
        sorted("exists(title) SORT BY author.city.name DESC && @id"),
        books(&["b1", "b2", "b3", "b4", "b5", "b6"])
    );
    assert_eq!(
        sorted("exists(author.bio) SORT BY author.bio:fts"),
        books(&["b1", "b2", "b3"])
    );
    assert_eq!(
        sorted("exists(title) SORT BY author.born DESC && title"),
        books(&["b1", "b2", "b5", "b4", "b6", "b3"])
    );
    let mut repository = repo(root, CacheMode::Memory);
    let text = repository
        .search_text(&TextQuery {
            pattern: "e".into(),
            collections: Some(vec!["books".into()]),
            filter: Some("exists(author.name) SORT BY author.name DESC".into()),
            count: true,
            limit: 10,
            ..Default::default()
        })
        .unwrap();
    let hits = text
        .hits
        .iter()
        .map(|hit| hit.record.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(hits, ["b3", "b2", "b1"]);
}

#[test]
fn the_linked_collection_is_queried_first_to_narrow_candidates() {
    let fixture = fixture();
    let root = fixture.path();
    let mut repository = repo(root, CacheMode::Memory);
    run(
        &mut repository,
        "exists(title)",
        Some("books"),
        JsonQueryBackend::Auto,
    );
    // Indexed scalar relation: typed postings select the one referring book.
    let result = run(
        &mut repository,
        "author.name = 'Ada'",
        Some("books"),
        JsonQueryBackend::Auto,
    );
    assert_eq!(ids(&result), books(&["b1"]));
    assert!(
        result
            .query_plan
            .iter()
            .any(|step| step == "relation semi-join: 1 linked IDs"),
        "{:?}",
        result.query_plan
    );
    // Three linked people are read, then only the one candidate book.
    assert_eq!(result.metrics.grep_scans, 1, "{:?}", result.metrics);
    // An unindexed relation narrows through the bigram index.
    let result = run(
        &mut repository,
        "publisher.name = 'Zed Books'",
        Some("books"),
        JsonQueryBackend::Auto,
    );
    assert_eq!(ids(&result), books(&["b2"]));
    assert!(result.metrics.grep_scans < 6, "{:?}", result.metrics);
    // No linked match means no source record is read.
    let result = run(
        &mut repository,
        "author.name = 'Nobody'",
        Some("books"),
        JsonQueryBackend::Auto,
    );
    assert_eq!(result.total, Some(0));
    assert_eq!(result.metrics.grep_scans, 0, "{:?}", result.metrics);
}

#[test]
fn linked_changes_are_visible_to_the_next_query() {
    let fixture = fixture();
    let root = fixture.path();
    let mut repository = repo(root, CacheMode::Memory);
    assert_eq!(
        ids(&run(
            &mut repository,
            "author.name = 'Ada'",
            Some("books"),
            JsonQueryBackend::Auto
        )),
        books(&["b1"])
    );
    write(
        root,
        "data/people/bert.json",
        json!({"name":"Ada","born":1790,"city":"berlin"}),
    );
    repository
        .apply_source_changes(&[SourceChange::Changed("data/people/bert.json".into())])
        .unwrap();
    let mut found = ids(&run(
        &mut repository,
        "author.name = 'Ada'",
        Some("books"),
        JsonQueryBackend::Auto,
    ));
    found.sort();
    assert_eq!(found, books(&["b1", "b2"]));
    // A source edit reported through the change API is seen as well.
    write(
        root,
        "data/books/b6.json",
        json!({"title":"Six","year":1870,"author":"ada"}),
    );
    repository
        .apply_source_changes(&[SourceChange::Changed("data/books/b6.json".into())])
        .unwrap();
    let mut found = ids(&run(
        &mut repository,
        "author.name = 'Ada'",
        Some("books"),
        JsonQueryBackend::Auto,
    ));
    found.sort();
    assert_eq!(found, books(&["b1", "b2", "b6"]));
}
