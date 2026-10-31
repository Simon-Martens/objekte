//! Collection discovery, optional schemas, and nested record ownership.

mod support;

use objekte_core::{
    Collection, JsonQuery, JsonQueryBackend, RecordStatus, Repository, ViewRequest,
};
use serde_json::json;
use std::{fs, path::Path};

fn write(root: &Path, relative: &str, value: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, value).unwrap();
}

fn request(filter: &str, collection: Option<&str>) -> JsonQuery {
    JsonQuery {
        filter: filter.into(),
        collections: collection.map(|name| vec![name.into()]),
        count: true,
        limit: 100,
        ..Default::default()
    }
}

#[test]
fn folders_with_direct_json_are_independent_collections_without_schemas() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "data/books/a.json", r#"{"title":"Moon"}"#);
    write(root, "data/books/part/a.json", r#"{"title":"Moon"}"#);
    write(root, "data/notes/n.json", r#"{"title":"Sun"}"#);
    write(root, "data/broken/b.json", "not JSON");
    fs::create_dir_all(root.join("data/empty")).unwrap();
    write(root, "settings.json", r#"{"title":"should be ignored"}"#);

    let mut repo = Repository::open(root).unwrap();
    assert_eq!(
        repo.model()
            .collections
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["books", "books/part", "broken", "notes"]
    );
    assert!(
        repo.model()
            .collections
            .values()
            .all(|c| c.schema_path.is_none() && c.indexes.is_empty() && !c.recursive)
    );
    assert_eq!(
        repo.query_json(&request("title = 'Moon'", None))
            .unwrap()
            .total,
        Some(2)
    );
    assert_eq!(
        repo.query_json(&request("title = 'Moon'", Some("books")))
            .unwrap()
            .total,
        Some(1)
    );
    repo.build_posting("books", "title").unwrap();
    let posted = repo
        .query_json(&request("title = 'Moon'", Some("books")))
        .unwrap();
    assert_eq!(posted.total, Some(1));
    assert_eq!(posted.metrics.grep_scans, 0);
    repo.build_posting("books", "unknown").unwrap();
    let missing = repo
        .query_json(&request("missing(unknown)", Some("books")))
        .unwrap();
    assert_eq!(missing.total, Some(1));
    assert_eq!(missing.metrics.grep_scans, 0);
    assert!(repo.get_record("books", "part/a").is_err());
    let record = repo.get_record("books/part", "a").unwrap().unwrap();
    assert_eq!(record.state.status, RecordStatus::NoSchema);
    let malformed = repo.get_record("broken", "b").unwrap().unwrap();
    assert_eq!(malformed.state.status, RecordStatus::IncorrectJson);
    assert_eq!(repo.validate_collection("books").unwrap().validated, 0);
    let page = repo.open_view("books", &ViewRequest::default()).unwrap();
    assert_eq!(page.columns, ["@id"]);
    assert_eq!(page.states["a"].status, RecordStatus::NoSchema);
}

#[test]
fn direct_json_in_data_root_has_its_own_collection() {
    let temp = tempfile::tempdir().unwrap();
    write(temp.path(), "data/root.json", r#"{"title":"Root"}"#);
    write(temp.path(), "data/books/a.json", r#"{"title":"Book"}"#);
    let mut repo = Repository::open(temp.path()).unwrap();
    assert_eq!(
        repo.model()
            .collections
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["@root", "books"]
    );
    assert_eq!(
        repo.query_json(&request("title = 'Root'", Some("@root")))
            .unwrap()
            .total,
        Some(1)
    );
}

#[test]
fn schema_free_top_level_filter_uses_bigram_candidates() {
    let temp = tempfile::tempdir().unwrap();
    for id in 0..12 {
        write(
            temp.path(),
            &format!("data/books/{id}.json"),
            if id == 7 {
                r#"{"title":"Starlight"}"#
            } else {
                r#"{"title":"Earth"}"#
            },
        );
    }
    let mut repo = Repository::open(temp.path()).unwrap();
    let result = repo
        .query_json(&request("title ~ 'Starlight'", Some("books")))
        .unwrap();
    assert_eq!(result.total, Some(1));
    assert_eq!(result.records[0].id, "7");
    assert!(result.metrics.grep_scans < 12, "{:#?}", result.metrics);
    write(
        temp.path(),
        "data/books/12.json",
        r#"{"title":["Moon","Sun"]}"#,
    );
    repo.refresh("books").unwrap();
    let error = repo
        .query_json(&request("title ~ 'Starlight'", Some("books")))
        .unwrap_err();
    assert!(error.to_string().contains("multiple values"), "{error}");
}

#[test]
fn default_schema_claims_all_descendants_with_relative_record_ids() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "data/books/a.json", r#"{"title":"A","year":1800}"#);
    write(
        root,
        "data/books/part/a.json",
        r#"{"title":"B","year":1801}"#,
    );
    write(root, "data/books/part/deep/b.json", r#"{"year":1802}"#);
    write(root, "data/notes/n.json", r#"{"title":"N"}"#);
    write(
        root,
        ".editor/collections/books/schema.json",
        &json!({
            "type":"object",
            "properties":{"title":{"type":"string"},"year":{"type":"integer"}},
            "required":["title"]
        })
        .to_string(),
    );

    let mut repo = Repository::open(root).unwrap();
    let explicitly_opened = Repository::open_with_schema(
        root,
        "books",
        root.join(".editor/collections/books/schema.json"),
    )
    .unwrap();
    assert!(explicitly_opened.model().collections["books"].recursive);
    assert_eq!(
        repo.model()
            .collections
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["books", "notes"]
    );
    assert!(repo.model().collections["books"].recursive);
    assert_eq!(repo.model().collections["books"].indexes.len(), 0);
    let result = repo
        .query_json(&request("year >= 1800", Some("books")))
        .unwrap();
    assert_eq!(result.total, Some(3));
    assert_eq!(
        result
            .records
            .iter()
            .map(|r| r.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "part/a", "part/deep/b"]
    );
    assert_eq!(result.records[1].path, "data/books/part/a.json");
    assert_eq!(result.records[0].state.status, RecordStatus::Valid);
    assert_eq!(result.records[2].state.status, RecordStatus::Invalid);
    let direct = repo
        .query_json(&JsonQuery {
            backend: JsonQueryBackend::Direct,
            ..request("year >= 1800", Some("books"))
        })
        .unwrap();
    assert_eq!(
        direct.records.iter().map(|r| &r.id).collect::<Vec<_>>(),
        result.records.iter().map(|r| &r.id).collect::<Vec<_>>()
    );
    let grep = repo
        .query_json(&JsonQuery {
            backend: JsonQueryBackend::Grep,
            ..request("year >= 1800", Some("books"))
        })
        .unwrap();
    assert_eq!(
        grep.records.iter().map(|r| &r.id).collect::<Vec<_>>(),
        result.records.iter().map(|r| &r.id).collect::<Vec<_>>()
    );

    repo.build_posting("books", "year").unwrap();
    let indexed = repo
        .query_json(&request("year >= 1801", Some("books")))
        .unwrap();
    assert_eq!(indexed.total, Some(2));
    assert_eq!(indexed.metrics.grep_scans, 0);
    assert_eq!(
        repo.get_record("books", "part/a").unwrap().unwrap().id,
        "part/a"
    );

    write(
        root,
        "data/books/part/new.json",
        r#"{"title":"C","year":1803}"#,
    );
    repo.refresh("books").unwrap();
    assert_eq!(
        repo.query_json(&request("year >= 1801", Some("books")))
            .unwrap()
            .total,
        Some(3)
    );
    repo.build_posting("books", "year").unwrap();
    repo.rebuild_bigram_index("books").unwrap();
    let rebuilt = repo
        .query_json(&request("year >= 1801", Some("books")))
        .unwrap();
    assert_eq!(rebuilt.total, Some(3));
    assert_eq!(rebuilt.metrics.grep_scans, 0);
}

#[test]
fn local_default_schema_groups_descendants_and_overrides_shared_default() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "data/books/part/a.json", r#"{"title":"A"}"#);
    write(
        root,
        ".editor/collections/books/schema.json",
        r#"{"type":"object","required":["shared"]}"#,
    );
    write(
        root,
        "data/books/.editor/schema.json",
        r#"{"type":"object","properties":{"title":{"type":"string"}},"required":["title"]}"#,
    );
    let mut repo = Repository::open(root).unwrap();
    assert_eq!(repo.model().collections.len(), 1);
    assert_eq!(
        repo.model().collections["books"].path,
        root.join("data/books")
    );
    assert_eq!(
        repo.model().collections["books"].schema_path,
        Some(root.join("data/books/.editor/schema.json"))
    );
    let result = repo
        .query_json(&request("title = 'A'", Some("books")))
        .unwrap();
    assert_eq!(result.total, Some(1));
    assert_eq!(result.records[0].state.status, RecordStatus::Valid);
}

#[test]
fn default_schemas_can_be_disabled_without_disabling_explicit_schemas() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "data/books/a.json", "{}");
    write(root, "data/books/part/b.json", "{}");
    write(root, "data/notes/n.json", "{}");
    write(
        root,
        "data/books/.editor/schema.json",
        r#"{"type":"object"}"#,
    );
    write(
        root,
        ".editor/collections/notes/schema.json",
        r#"{"type":"object"}"#,
    );
    let plain = Repository::builder(root)
        .without_default_schemas()
        .open()
        .unwrap();
    assert_eq!(
        plain
            .model()
            .collections
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["books", "books/part", "notes"]
    );
    assert!(
        plain
            .model()
            .collections
            .values()
            .all(|c| c.schema_path.is_none())
    );

    let explicit = Repository::builder(root)
        .without_default_schemas()
        .schema("notes", ".editor/collections/notes/schema.json")
        .open()
        .unwrap();
    assert!(explicit.model().collections["notes"].schema_path.is_some());
    assert!(explicit.model().collections["books"].schema_path.is_none());
}

#[test]
fn code_supplied_schema_and_data_root_override_defaults() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "library/volumes/a.json", r#"{"rank":2}"#);
    write(root, "library/volumes/part/a.json", r#"{"rank":3}"#);
    write(root, "library/other/o.json", r#"{"rank":9}"#);
    let schema = temp.path().join("external-schema.json");
    fs::write(
        &schema,
        r#"{"type":"object","properties":{"rank":{"type":"integer","index":true}}}"#,
    )
    .unwrap();
    let mut repo = Repository::builder(root)
        .data_root("library")
        .schema("volumes", &schema)
        .open()
        .unwrap();
    assert_eq!(
        repo.model()
            .collections
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["other", "volumes"]
    );
    assert!(repo.model().collections["volumes"].schema_path.is_some());
    assert_eq!(repo.model().collections["volumes"].indexes[0].name, "rank");
    let indexed = repo
        .query_json(&request("rank >= 2", Some("volumes")))
        .unwrap();
    assert_eq!(indexed.total, Some(2));
    assert_eq!(indexed.metrics.grep_scans, 0);
    assert_eq!(indexed.records[1].id, "part/a");
    assert_eq!(repo.model().collections["other"].schema_path, None);
}

#[test]
fn overlapping_schema_bases_and_empty_collections_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "data/books/part/a.json", "{}");
    fs::create_dir_all(root.join("data/empty")).unwrap();
    write(root, "schema.json", r#"{"type":"object"}"#);
    let overlapping = Repository::builder(root)
        .schema("books", "schema.json")
        .schema("books/part", "schema.json")
        .open()
        .err()
        .unwrap()
        .to_string();
    assert!(overlapping.contains("overlap"), "{overlapping}");
    let duplicate = Repository::builder(root)
        .schema("books", "schema.json")
        .schema("books", "schema.json")
        .open()
        .err()
        .unwrap()
        .to_string();
    assert!(duplicate.contains("overlap"), "{duplicate}");
    let empty = Repository::builder(root)
        .schema("empty", "schema.json")
        .open()
        .err()
        .unwrap()
        .to_string();
    assert!(empty.contains("must contain a JSON file"), "{empty}");
    write(root, "data/books/schema.json", r#"{"type":"object"}"#);
    let misplaced = Repository::builder(root)
        .schema("books", "data/books/schema.json")
        .open()
        .err()
        .unwrap()
        .to_string();
    assert!(
        misplaced.contains("outside the JSON data tree"),
        "{misplaced}"
    );
}

#[test]
fn explicit_configuration_refresh_discovers_new_populated_folders() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "data/books/a.json", "{}");
    let mut repo = Repository::open(root).unwrap();
    repo.warm_bigram_index("books").unwrap();
    write(root, "data/notes/n.json", "{}");
    let changes = repo.refresh_configuration().unwrap();
    assert_eq!(changes.affected_collections, ["notes"]);
    assert!(repo.model().collections.contains_key("notes"));
    assert_eq!(
        repo.warm_bigram_index("books").unwrap().bigram_open_nanos,
        0
    );
    assert_eq!(
        repo.query_json(&request("missing(title)", None))
            .unwrap()
            .total,
        Some(2)
    );
    fs::remove_file(root.join("data/notes/n.json")).unwrap();
    let removed = repo.refresh_configuration().unwrap();
    assert_eq!(removed.affected_collections, ["notes"]);
    assert!(!repo.model().collections.contains_key("notes"));
}

#[test]
fn adding_and_removing_default_schema_changes_collection_boundaries() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "data/books/a.json", "{}");
    write(root, "data/books/part/b.json", "{}");
    let mut repo = Repository::open(root).unwrap();
    assert!(repo.model().collections.contains_key("books/part"));
    write(
        root,
        ".editor/collections/books/schema.json",
        r#"{"type":"object"}"#,
    );
    let added = repo.refresh_configuration().unwrap();
    assert!(added.affected_collections.contains(&"books".into()));
    assert!(added.affected_collections.contains(&"books/part".into()));
    assert!(!repo.model().collections.contains_key("books/part"));
    assert_eq!(
        repo.query_json(&request("missing(x)", Some("books")))
            .unwrap()
            .total,
        Some(2)
    );
    fs::remove_file(root.join(".editor/collections/books/schema.json")).unwrap();
    let removed = repo.refresh_configuration().unwrap();
    assert!(removed.affected_collections.contains(&"books/part".into()));
    assert!(repo.model().collections.contains_key("books/part"));
}

#[test]
fn legacy_collection_can_omit_schema_and_ui() {
    let fixture = support::Fixture::new();
    fixture.edit(".editor/books/settings.json", |settings| {
        settings.as_object_mut().unwrap().remove("schema");
        settings.as_object_mut().unwrap().remove("ui");
    });
    let mut repo =
        Repository::load_repository(&fixture.root, objekte_core::Options::new()).unwrap();
    assert!(repo.model().collections["books"].schema_path.is_none());
    assert_eq!(
        repo.get_record("books", "a").unwrap().unwrap().state.status,
        RecordStatus::NoSchema
    );
    assert_eq!(
        repo.open_view("books", &ViewRequest::default())
            .unwrap()
            .columns,
        ["@id"]
    );
    write(
        &fixture.root,
        "data/books/.editor/schema.json",
        &fs::read_to_string(fixture.root.join(".editor/books/schema.json")).unwrap(),
    );
    let changes = repo.refresh_configuration().unwrap();
    assert_eq!(changes.affected_collections, ["books"]);
    assert_eq!(
        repo.model().collections["books"].schema_path,
        Some(fixture.root.join("data/books/.editor/schema.json"))
    );
    assert!(repo.model().collections["books"].recursive);
}

#[test]
fn collection_can_open_standalone_with_or_without_schema() {
    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().join("books");
    write(temp.path(), "books/a.json", r#"{"title":"A"}"#);
    write(temp.path(), "books/part/b.json", r#"{"title":"B"}"#);
    let plain = Collection::open(&folder).unwrap();
    assert_eq!(plain.name, "@root");
    assert!(!plain.recursive);
    assert!(plain.schema_path.is_none());
    let schema = temp.path().join("book-schema.json");
    fs::write(
        &schema,
        r#"{"type":"object","properties":{"title":{"type":"string","index":true}}}"#,
    )
    .unwrap();
    let grouped = Collection::open_with_schema(&folder, &schema).unwrap();
    assert!(grouped.recursive);
    assert_eq!(grouped.indexes[0].name, "title");
    fs::remove_file(folder.join("a.json")).unwrap();
    assert!(Collection::open(&folder).is_err());
    assert!(Collection::open_with_schema(&folder, &schema).is_ok());
}

#[test]
fn declared_references_can_target_nested_record_ids() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "data/books/part/a.json", "{}");
    write(root, "data/notes/n.json", r#"{"book":"part/a"}"#);
    write(root, "schemas/books.json", r#"{"type":"object"}"#);
    write(
        root,
        "schemas/notes.json",
        r#"{"type":"object","properties":{"book":{"type":"string","x-relation":{"collection":"books"}}}}"#,
    );
    let mut repo = Repository::builder(root)
        .schema("books", "schemas/books.json")
        .schema("notes", "schemas/notes.json")
        .open()
        .unwrap();
    let from = repo.references_from("notes", "n", true).unwrap();
    assert_eq!(from.edges.len(), 1);
    assert_eq!(from.edges[0].target_id, "part/a");
    assert_eq!(
        from.edges[0].target_state,
        objekte_core::TargetState::Present
    );
    let to = repo.references_to("books", "part/a", true, true).unwrap();
    assert_eq!(to.edges.len(), 1);
    assert_eq!(to.edges[0].source_collection, "notes");
}

#[test]
fn flat_collection_does_not_resolve_a_nested_reference_as_its_record() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "data/books/a.json", "{}");
    write(root, "data/books/part/a.json", "{}");
    write(root, "data/notes/n.json", r#"{"book":"part/a"}"#);
    write(
        root,
        "schemas/notes.json",
        r#"{"type":"object","properties":{"book":{"type":"string","x-relation":{"collection":"books"}}}}"#,
    );
    let mut repo = Repository::builder(root)
        .schema("notes", "schemas/notes.json")
        .open()
        .unwrap();
    let from = repo.references_from("notes", "n", true).unwrap();
    assert_eq!(
        from.edges[0].target_state,
        objekte_core::TargetState::Missing
    );
}

#[test]
fn existing_musenalm_layout_discovers_its_seven_schema_collections() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let repo = Repository::open(root).unwrap();
    assert_eq!(repo.model().collections.len(), 7);
    assert!(
        repo.model()
            .collections
            .values()
            .all(|collection| collection.recursive && collection.schema_path.is_some())
    );
    assert_eq!(
        repo.model().collections["contents"].display,
        "preferred_title"
    );
}
