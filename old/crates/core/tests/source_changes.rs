use objekte_core::{
    CacheMode, JsonQuery, JsonQueryBackend, Options, Repository, SourceChange, SourceChanges,
    TextQuery, bigram::BigramSnapshot,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};


fn write(root: &Path, relative: &str, value: Value) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
}

/// `books` has a schema base, so nested IDs belong to it; `notes` is schema-free.
fn fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        ".objekte/collections/books.schema.json",
        json!({
            "type":"object",
            "properties":{
                "title":{"type":"string","x-sort-index":true},
                "year":{"type":"integer","index":true}
            },
            "x-objekte":{"name":"books","path":"data/books","type":"data","format":"json","id":"filename"}
        }),
    );
    write(
        root,
        "data/books/a.json",
        json!({"title":"Alpha","year":1800}),
    );
    write(
        root,
        "data/books/b.json",
        json!({"title":"Beta","year":1801}),
    );
    write(
        root,
        "data/books/c.json",
        json!({"title":"Gamma","year":1802}),
    );
    write(
        root,
        "data/books/part/d.json",
        json!({"title":"Delta","year":1803}),
    );
    write(root, "data/notes/n1.json", json!({"text":"first"}));
    temp
}

fn repo(root: &Path, cache: CacheMode) -> Repository {
    let mut options = Options::new();
    options.cache = cache;
    Repository::load_repository(root, options).unwrap()
}

fn ids(repository: &mut Repository, collection: &str, filter: &str) -> Vec<String> {
    let result = repository
        .query_json(&JsonQuery {
            filter: filter.into(),
            collections: Some(vec![collection.into()]),
            count: true,
            limit: 50,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(result.total, Some(result.records.len()), "{filter}");
    result.records.into_iter().map(|record| record.id).collect()
}

/// Results of a fresh direct scan, which ignores every index.
fn direct(root: &Path, collection: &str, filter: &str) -> Vec<String> {
    let mut repository = repo(root, CacheMode::Disabled);
    let result = repository
        .query_json(&JsonQuery {
            filter: filter.into(),
            collections: Some(vec![collection.into()]),
            backend: JsonQueryBackend::Direct,
            count: true,
            limit: 50,
            ..Default::default()
        })
        .unwrap();
    result.records.into_iter().map(|record| record.id).collect()
}

const FILTERS: [&str; 5] = [
    "year >= 1800 SORT BY title",
    "year = 1900",
    "title ~ '%a%' SORT BY @id",
    "exists(year) SORT BY year DESC",
    "year < 1802 SORT BY @id",
];

fn changes(created: &[&str], changed: &[&str], deleted: &[&str]) -> SourceChanges {
    let owned = |ids: &[&str]| ids.iter().map(|id| (*id).to_owned()).collect();
    SourceChanges {
        created: owned(created),
        changed: owned(changed),
        deleted: owned(deleted),
    }
}


#[test]
fn reported_edits_update_every_index_without_a_directory_scan() {
    let fixture = fixture();
    let root = fixture.path();
    let mut repository = repo(root, CacheMode::Memory);
    for filter in FILTERS {
        assert_eq!(
            ids(&mut repository, "books", filter),
            direct(root, "books", filter)
        );
    }
    write(
        root,
        "data/books/a.json",
        json!({"title":"Aardvark","year":1900}),
    );
    write(
        root,
        "data/books/part/e.json",
        json!({"title":"Epsilon","year":1799}),
    );
    fs::remove_file(root.join("data/books/b.json")).unwrap();
    fs::rename(
        root.join("data/books/c.json"),
        root.join("data/books/part/c2.json"),
    )
    .unwrap();
    let report = repository
        .apply_source_changes(&[
            SourceChange::Changed("data/books/a.json".into()),
            SourceChange::Created(root.join("data/books/part/e.json")),
            SourceChange::Deleted("data/books/b.json".into()),
            SourceChange::Moved {
                from: "data/books/c.json".into(),
                to: "data/books/part/c2.json".into(),
            },
        ])
        .unwrap();
    let books = &report["books"];
    assert!(books.indexed);
    assert_eq!(books.saved_cache, None);
    assert_eq!(
        books.changes,
        changes(&["part/c2", "part/e"], &["a"], &["b", "c"])
    );
    for filter in FILTERS {
        assert_eq!(
            ids(&mut repository, "books", filter),
            direct(root, "books", filter),
            "{filter}"
        );
    }
    assert_eq!(ids(&mut repository, "books", "year = 1900"), ["a"]);
    let text = repository
        .search_text(&TextQuery {
            pattern: "Epsilon".into(),
            collections: Some(vec!["books".into()]),
            count: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(text.total, Some(1));
    let deleted = repository.record_revision("books", "b").unwrap().unwrap();
    assert!(!deleted.present);
    let changed = repository.record_revision("books", "a").unwrap().unwrap();
    assert!(changed.present && changed.revision > 1);
}

#[test]
fn reported_kinds_are_checked_against_the_files() {
    let fixture = fixture();
    let root = fixture.path();
    let mut repository = repo(root, CacheMode::Memory);
    ids(&mut repository, "books", "exists(year)");
    // Nothing happened on disk, so nothing changes, whatever the report says.
    let report = repository
        .apply_source_changes(&[
            SourceChange::Changed("data/books/a.json".into()),
            SourceChange::Deleted("data/books/b.json".into()),
            SourceChange::Created("data/books/missing.json".into()),
        ])
        .unwrap();
    assert_eq!(report["books"].changes, SourceChanges::default());
    // A file reported as changed but actually removed is deleted.
    fs::remove_file(root.join("data/books/a.json")).unwrap();
    let report = repository
        .apply_source_changes(&[SourceChange::Changed("data/books/a.json".into())])
        .unwrap();
    assert_eq!(report["books"].changes, changes(&[], &[], &["a"]));
}

#[test]
fn invalid_paths_are_rejected_before_any_change_is_applied() {
    let fixture = fixture();
    let root = fixture.path();
    let mut repository = repo(root, CacheMode::Memory);
    assert_eq!(
        ids(&mut repository, "books", "year = 1900"),
        Vec::<String>::new()
    );
    write(
        root,
        "data/books/a.json",
        json!({"title":"Alpha","year":1900}),
    );
    for bad in [
        PathBuf::from("data/other/x.json"),
        PathBuf::from("data/books/a.txt"),
        PathBuf::from("data/books/../notes/n1.json"),
        PathBuf::from("data/notes/deeper/n2.json"),
    ] {
        let error = repository
            .apply_source_changes(&[
                SourceChange::Changed("data/books/a.json".into()),
                SourceChange::Created(bad.clone()),
            ])
            .unwrap_err();
        assert!(
            error.to_string().contains("not a JSON record") || error.to_string().contains("ID"),
            "{}: {error}",
            bad.display()
        );
    }
    // The valid change in each rejected batch was not applied.
    assert_eq!(
        ids(&mut repository, "books", "year = 1900"),
        Vec::<String>::new()
    );
}

#[test]
fn diff_reports_and_applies_all_source_changes() {
    let fixture = fixture();
    let root = fixture.path();
    let mut repository = repo(root, CacheMode::Memory);
    ids(&mut repository, "books", "exists(year)");
    ids(&mut repository, "notes", "exists(text)");
    write(
        root,
        "data/books/a.json",
        json!({"title":"Alpha","year":1900}),
    );
    write(
        root,
        "data/books/f.json",
        json!({"title":"Phi","year":1804}),
    );
    fs::remove_file(root.join("data/books/part/d.json")).unwrap();
    write(root, "data/notes/n2.json", json!({"text":"second"}));
    let report = repository.diff_all().unwrap();
    assert_eq!(
        report["books"].changes,
        changes(&["f"], &["a"], &["part/d"])
    );
    assert_eq!(report["notes"].changes, changes(&["n2"], &[], &[]));
    for filter in FILTERS {
        assert_eq!(
            ids(&mut repository, "books", filter),
            direct(root, "books", filter),
            "{filter}"
        );
    }
    assert!(repository.diff("books").unwrap().changes.is_empty());
}

#[test]
fn existing_disk_caches_are_rewritten_and_stay_valid() {
    let fixture = fixture();
    let root = fixture.path();
    let mut repository = repo(root, CacheMode::Memory);
    let cache = repository.save_bigram_cache("books").unwrap();
    write(
        root,
        "data/books/a.json",
        json!({"title":"Alpha","year":1900}),
    );
    write(
        root,
        "data/books/g.json",
        json!({"title":"Gimel","year":1805}),
    );
    let report = repository
        .apply_source_changes(&[
            SourceChange::Changed("data/books/a.json".into()),
            SourceChange::Created("data/books/g.json".into()),
        ])
        .unwrap();
    assert_eq!(
        report["books"].saved_cache.as_deref(),
        Some(cache.as_path())
    );
    // The saved manifest now matches every current source file.
    BigramSnapshot::load_checked(&cache, &root.join("data/books"), true).unwrap();
    let mut reopened = repo(root, CacheMode::Memory);
    assert_eq!(ids(&mut reopened, "books", "year = 1900"), ["a"]);
    assert_eq!(reopened.metrics().bigram_snapshot_loads, 1);
    // A collection without a disk cache is not given one.
    ids(&mut repository, "notes", "exists(text)");
    write(root, "data/notes/n1.json", json!({"text":"edited"}));
    let notes = repository.diff("notes").unwrap();
    assert_eq!(notes.changes, changes(&[], &["n1"], &[]));
    assert_eq!(notes.saved_cache, None);
    assert!(!repository.bigram_cache_path("notes").unwrap().exists());
}

#[test]
fn unindexed_collections_are_left_for_their_next_load() {
    let fixture = fixture();
    let root = fixture.path();
    let mut repository = repo(root, CacheMode::Memory);
    write(
        root,
        "data/books/a.json",
        json!({"title":"Alpha","year":1900}),
    );
    let report = repository
        .apply_source_changes(&[SourceChange::Changed("data/books/a.json".into())])
        .unwrap();
    assert!(!report["books"].indexed);
    assert_eq!(ids(&mut repository, "books", "year = 1900"), ["a"]);
    let mut disabled = repo(root, CacheMode::Disabled);
    let report = disabled.diff_all().unwrap();
    assert!(report.values().all(|outcome| !outcome.indexed));
}

#[cfg(unix)]
#[test]
fn symlinked_and_absolute_paths_resolve_to_the_owning_collection() {
    let fixture = fixture();
    let root = fixture.path();
    let link = tempfile::tempdir().unwrap();
    let alias = link.path().join("alias");
    std::os::unix::fs::symlink(root, &alias).unwrap();
    let mut repository = repo(root, CacheMode::Memory);
    ids(&mut repository, "books", "exists(year)");
    write(
        root,
        "data/books/part/d.json",
        json!({"title":"Delta","year":1900}),
    );
    fs::remove_file(root.join("data/books/b.json")).unwrap();
    let report: BTreeMap<_, _> = repository
        .apply_source_changes(&[
            SourceChange::Changed(alias.join("data/books/part/d.json")),
            SourceChange::Deleted(alias.join("data/books/b.json")),
        ])
        .unwrap();
    assert_eq!(report["books"].changes, changes(&[], &["part/d"], &["b"]));
}


#[test]
fn diff_in_a_new_handle_itemizes_changes_since_the_saved_cache() {
    let fixture = fixture();
    let root = fixture.path();
    repo(root, CacheMode::Memory)
        .save_bigram_cache("books")
        .unwrap();
    // Edits while no process holds the repository open.
    write(
        root,
        "data/books/a.json",
        json!({"title":"Alpha","year":1900}),
    );
    write(
        root,
        "data/books/h.json",
        json!({"title":"Heth","year":1806}),
    );
    fs::remove_file(root.join("data/books/b.json")).unwrap();
    let mut repository = repo(root, CacheMode::Memory);
    let books = repository.diff("books").unwrap();
    assert_eq!(books.changes, changes(&["h"], &["a"], &["b"]));
    assert_eq!(repository.metrics().bigram_snapshot_loads, 1);
    let cache = books.saved_cache.unwrap();
    BigramSnapshot::load_checked(&cache, &root.join("data/books"), true).unwrap();
    for filter in FILTERS {
        assert_eq!(
            ids(&mut repository, "books", filter),
            direct(root, "books", filter),
            "{filter}"
        );
    }
    // Without a saved cache there is nothing to compare against.
    let notes = repository.diff("notes").unwrap();
    assert!(notes.indexed && notes.changes.is_empty());
}
