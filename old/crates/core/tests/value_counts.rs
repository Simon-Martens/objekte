use objekte_core::{Repository, SourceChange, ValueCount};
use serde_json::{Value, json};
use std::{fs, path::Path};


fn write(root: &Path, relative: &str, value: Value) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
}

/// `books` indexes `kind` and `year` as fields and `tags[*]` as a path;
/// `title` is not indexed.
fn fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        ".objekte/collections/books.schema.json",
        json!({
            "type":"object",
            "properties":{
                "title":{"type":"string"},
                "kind":{"type":["string","null"],"index":true},
                "year":{"type":"integer","index":true},
                "tags":{"type":"array","items":{"type":"string"}}
            },
            "x-index-paths":["tags[*]"],
            "x-objekte":{"name":"books","path":"data/books","type":"data","format":"json","id":"filename"}
        }),
    );
    for (id, kind, year, tags) in [
        ("a", json!("poem"), 1800, json!(["x", "x", "y"])),
        ("b", json!("prose"), 1800, json!(["x"])),
        ("c", json!("poem"), 1900, json!([])),
        ("d", json!(null), 1850, json!(["z"])),
        ("e", json!("drama"), 1900, json!(["y"])),
    ] {
        write(
            root,
            &format!("data/books/{id}.json"),
            json!({"title": format!("Book {id}"), "kind": kind, "year": year, "tags": tags}),
        );
    }
    temp
}

fn counts(values: &[ValueCount]) -> Vec<(Value, usize)> {
    values
        .iter()
        .map(|count| (count.value.clone(), count.records))
        .collect()
}

fn indexed(root: &Path) -> Repository {
    let mut repository = Repository::open(root).unwrap();
    repository.diff("books").unwrap();
    repository
}


#[test]
fn indexed_fields_count_records_most_first_and_ties_in_value_order() {
    let temp = fixture();
    let repository = indexed(temp.path());
    let kinds = repository
        .value_counts("books", "kind", 10)
        .unwrap()
        .unwrap();
    assert_eq!(
        counts(&kinds),
        [(json!("poem"), 2), (json!("drama"), 1), (json!("prose"), 1)],
        "null is left out"
    );
    let years = repository
        .value_counts("books", "year", 2)
        .unwrap()
        .unwrap();
    assert_eq!(counts(&years), [(json!(1800), 2), (json!(1900), 2)]);
}

#[test]
fn array_paths_count_each_record_once_and_accept_the_implicit_spelling() {
    let temp = fixture();
    let repository = indexed(temp.path());
    let tags = repository
        .value_counts("books", "tags[*]", 10)
        .unwrap()
        .unwrap();
    assert_eq!(
        counts(&tags),
        [(json!("x"), 2), (json!("y"), 2), (json!("z"), 1)]
    );
    let implicit = repository
        .value_counts("books", "tags", 10)
        .unwrap()
        .unwrap();
    assert_eq!(counts(&implicit), counts(&tags));
}

#[test]
fn unindexed_paths_and_unbuilt_indexes_give_none() {
    let temp = fixture();
    let unbuilt = Repository::open(temp.path()).unwrap();
    assert!(unbuilt.value_counts("books", "kind", 10).unwrap().is_none());
    let repository = indexed(temp.path());
    assert!(
        repository
            .value_counts("books", "title", 10)
            .unwrap()
            .is_none()
    );
    assert!(
        repository
            .value_counts("books", "missing.path", 10)
            .unwrap()
            .is_none()
    );
    assert!(repository.value_counts("nope", "kind", 10).is_err());
    assert!(repository.value_counts("books", "kind ==", 10).is_err());
}

#[test]
fn counts_follow_reported_changes() {
    let temp = fixture();
    let mut repository = indexed(temp.path());
    write(
        temp.path(),
        "data/books/b.json",
        json!({"title":"Book b","kind":"poem","year":1800,"tags":["y"]}),
    );
    fs::remove_file(temp.path().join("data/books/e.json")).unwrap();
    repository
        .apply_source_changes(&[
            SourceChange::Changed(temp.path().join("data/books/b.json")),
            SourceChange::Deleted(temp.path().join("data/books/e.json")),
        ])
        .unwrap();
    let kinds = repository
        .value_counts("books", "kind", 10)
        .unwrap()
        .unwrap();
    assert_eq!(counts(&kinds), [(json!("poem"), 3)]);
    let tags = repository
        .value_counts("books", "tags[*]", 10)
        .unwrap()
        .unwrap();
    assert_eq!(
        counts(&tags),
        [(json!("y"), 2), (json!("x"), 1), (json!("z"), 1)]
    );
}
