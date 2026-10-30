mod support;
use objekte_core::{CacheMode, PathStep, Repository};
use serde_json::json;
use std::fs;
use support::*;

#[test]
fn metadata_is_lazy_and_exposes_exact_indexes_and_cycles() {
    let f = Fixture::new();
    fs::write(f.root.join("data/books/a.json"), "broken").unwrap();
    let mut r = f.repo(CacheMode::Memory);
    assert_eq!(r.metrics().enumerations, 0);
    assert_eq!(r.metrics().source_reads, 0);
    assert!(!f.data.exists());
    let b = &r.model().collections["books"];
    let names = b
        .indexes
        .iter()
        .map(|i| i.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["count", "flag", "score"]);
    assert!(!names.contains(&"payload"));
    assert_eq!(b.page_size, 3);
    assert_eq!(
        r.model().graph.cycles,
        vec![vec!["books".to_owned(), "people".to_owned()]]
    );
    assert!(r.model().graph.edges.iter().any(|e| e.source == "books"
        && e.path
            == vec![
                PathStep::Field("links".into()),
                PathStep::Each,
                PathStep::Field("person".into())
            ]));
    assert!(
        r.cache_status()
            .unwrap()
            .collections
            .iter()
            .all(|c| c.state == "absent")
    );
    assert!(!f.data.exists());
}

#[test]
fn a_record_read_does_not_enumerate_or_touch_other_collections() {
    let f = Fixture::new();
    fs::write(f.root.join("data/spare/alone.json"), "broken").unwrap();
    let mut r = f.repo(CacheMode::Memory);
    let a = r.get_record("books", "a").unwrap().unwrap();
    assert_eq!(a.data["payload"]["html"], "<p>Keep **markdown**</p>");
    assert_eq!(r.metrics().source_reads, 1);
    assert_eq!(r.metrics().enumerations, 0);
    let status = r.cache_status().unwrap();
    let b = status
        .collections
        .iter()
        .find(|c| c.collection == "books")
        .unwrap();
    assert_eq!(b.state, "partial");
    assert!(!b.verified);
    assert_eq!(b.records, 1);
    assert!(
        status
            .collections
            .iter()
            .filter(|c| c.collection != "books")
            .all(|c| c.state == "absent")
    );
    let view = r.open_view("books", &all()).unwrap();
    assert_eq!(view.total, Some(7));
    assert_eq!(view.metrics.source_reads, 7);
}

#[test]
fn unsupported_schema_keywords_fail_at_their_locations() {
    for (key, value) in [
        ("minLength", json!(1)),
        ("$ref", json!("#/$defs/x")),
        ("format", json!("date")),
    ] {
        let f = Fixture::new();
        f.edit(".editor/books/schema.json", |v| {
            v["properties"]["title"][key] = value
        });
        let e = Repository::load_repository(&f.root, f.options(CacheMode::Disabled))
            .err()
            .unwrap()
            .to_string();
        assert!(e.contains(key), "{e}");
        assert!(e.contains("/properties/title"), "{e}");
    }
}

#[test]
fn invalid_configuration_and_ui_are_rejected() {
    let cases = vec![
        (".editor/repository.json", "version", json!(9)),
        (".editor/books/settings.json", "format", json!("markdown")),
        (".editor/books/settings.json", "id", json!("data.id")),
        (".editor/books/ui.json", "pageSize", json!(0)),
        (".editor/books/ui.json", "display", json!("payload")),
        (".editor/books/ui.json", "unsupported", json!(true)),
    ];
    for (p, key, value) in cases {
        let f = Fixture::new();
        f.edit(p, |v| v[key] = value);
        assert!(
            Repository::load_repository(&f.root, f.options(CacheMode::Disabled)).is_err(),
            "{p} {key}"
        );
    }
    for (key, value) in [
        ("columns", json!(["unknown"])),
        ("filters", json!(["payload"])),
        ("sortable", json!(["title"])),
        ("columns", json!(["title", "title"])),
    ] {
        let f = Fixture::new();
        f.edit(".editor/books/ui.json", |v| v["list"][key] = value);
        assert!(Repository::load_repository(&f.root, f.options(CacheMode::Disabled)).is_err());
    }
}

#[test]
fn shape_profile_rejects_bad_types_and_relation_shapes() {
    for value in [
        json!({"type":["string","number"]}),
        json!({"type":"array"}),
        json!({"type":"number","x-relation":{"collection":"people"}}),
        json!({"type":"object","additionalProperties":{"type":"string"}}),
    ] {
        let f = Fixture::new();
        f.edit(".editor/books/schema.json", |v| {
            v["properties"]["person"] = value
        });
        assert!(Repository::load_repository(&f.root, f.options(CacheMode::Disabled)).is_err());
    }
}

#[test]
fn unknown_targets_fail_but_self_references_are_valid() {
    let f = Fixture::new();
    f.edit(".editor/people/schema.json", |v| {
        v["properties"]["favourite"]["x-relation"]["collection"] = json!("missing")
    });
    let err = Repository::load_repository(&f.root, f.options(CacheMode::Disabled))
        .err()
        .unwrap()
        .to_string();
    assert!(err.contains("missing"));
    f.edit(".editor/people/schema.json", |v| {
        v["properties"]["favourite"]["x-relation"]["collection"] = json!("people")
    });
    let r = f.repo(CacheMode::Disabled);
    assert!(r.model().graph.cycles.contains(&vec!["people".into()]));
}

#[test]
fn page_defaults_and_optional_ui_values_are_normalized() {
    let f = Fixture::new();
    f.write(".editor/books/ui.json", &json!({"display":"title"}));
    let r = f.repo(CacheMode::Disabled);
    let c = &r.model().collections["books"];
    assert_eq!(c.page_size, 3);
    assert_eq!(c.list.default_sort.field, "@id");
    assert_eq!(c.list.columns, vec!["title"]);
    f.edit(".editor/books/ui.json", |v| v["pageSize"] = json!(6));
    assert_eq!(
        f.repo(CacheMode::Disabled).model().collections["books"].page_size,
        6
    );
}

#[test]
fn collection_paths_cannot_escape() {
    let f = Fixture::new();
    f.edit(".editor/books/settings.json", |v| {
        v["path"] = json!("../other")
    });
    assert!(Repository::load_repository(&f.root, f.options(CacheMode::Disabled)).is_err());
}

#[test]
fn duplicate_and_overlapping_source_directories_fail() {
    let f = Fixture::new();
    f.edit(".editor/people/settings.json", |v| {
        v["path"] = json!("data/books")
    });
    assert!(Repository::load_repository(&f.root, f.options(CacheMode::Disabled)).is_err());
}

#[test]
fn declared_property_names_are_not_treated_as_keywords() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Disabled);
    assert_eq!(
        r.get_record("books", "a").unwrap().unwrap().data["type"],
        "literal property name"
    );
}
