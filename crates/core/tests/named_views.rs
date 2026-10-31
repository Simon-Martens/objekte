use objekte_core::{Repository, ViewQueryResult, ViewType};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path};

fn write(root: &Path, path: &str, value: Value) {
    let file = root.join(path);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
}

fn fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "data/records/a.json",
        json!({"title":"Alpha","score":1}),
    );
    write(
        root,
        "data/records/b.json",
        json!({"title":"Beta","score":2}),
    );
    write(
        root,
        "data/records/c.json",
        json!({"title":"Gamma","score":3}),
    );
    write(
        root,
        ".objekte/collections/books.schema.json",
        json!({
            "type":"object",
            "properties":{"title":{"type":"string"},"score":{"type":"integer","index":true}},
            "x-objekte":{"name":"books","path":"data/records","type":"data","format":"json","id":"filename"}
        }),
    );
    write(
        root,
        ".objekte/views/books_list.view.json",
        json!({
            "name":"books_list", "type":"list",
            "arguments":{
                "page":{"type":"integer","default":1},
                "minimum":{"type":"integer","default":1},
                "term":{"type":"string","default":"a"}
            },
            "queries":{
                "records":{"type":"json","request":{
                    "filter":"score >= $minimum", "parameters":{"minimum":{"$arg":"minimum"}},
                    "collections":["books"], "limit":1,
                    "offset":{"$page_offset":{"page":{"$arg":"page"},"size":1}}, "count":true
                }},
                "matches":{"type":"text","request":{
                    "pattern":{"$arg":"term"},"collections":["books"],"limit":10,"count":true
                }}
            }
        }),
    );
    temp
}

#[test]
fn schema_mapping_and_independent_view_queries_execute() {
    let temp = fixture();
    let mut repo = Repository::open(temp.path()).unwrap();
    assert_eq!(repo.model().collections.len(), 1);
    assert_eq!(
        repo.model().collections["books"].path,
        temp.path().join("data/records")
    );
    assert_eq!(repo.model().views.len(), 1);
    let result = repo
        .run_view(
            "books_list",
            &BTreeMap::from([("page".into(), json!(2)), ("term".into(), json!("Beta"))]),
        )
        .unwrap();
    assert_eq!(result.kind, ViewType::List);
    let ViewQueryResult::Json(records) = &result.queries["records"] else {
        panic!("JSON result expected")
    };
    assert_eq!(records.total, Some(3));
    assert_eq!(records.records[0].id, "b");
    let ViewQueryResult::Text(matches) = &result.queries["matches"] else {
        panic!("text result expected")
    };
    assert_eq!(matches.total, Some(1));
    assert_eq!(matches.hits[0].record.id, "b");
}

#[test]
fn view_arguments_are_typed_and_native_query_arguments_can_be_overridden() {
    let temp = fixture();
    let mut repo = Repository::open(temp.path()).unwrap();
    assert!(
        repo.run_view("books_list", &BTreeMap::from([("page".into(), json!(0))]))
            .is_err()
    );
    assert!(
        repo.run_view("books_list", &BTreeMap::from([("page".into(), json!("2"))]))
            .is_err()
    );
    assert!(
        repo.run_view(
            "books_list",
            &BTreeMap::from([("unknown".into(), json!(1))])
        )
        .is_err()
    );
    let overrides = BTreeMap::from([(
        "records".into(),
        BTreeMap::from([("limit".into(), json!(2))]),
    )]);
    let result = repo
        .run_view_with_query_arguments("books_list", &BTreeMap::new(), &overrides)
        .unwrap();
    let ViewQueryResult::Json(records) = &result.queries["records"] else {
        panic!("JSON result expected")
    };
    assert_eq!(records.records.len(), 2);
}

#[test]
fn view_file_changes_reload_without_rebuilding_collection_mapping() {
    let temp = fixture();
    let mut repo = Repository::open(temp.path()).unwrap();
    let path = temp.path().join(".objekte/views/books_list.view.json");
    let mut view: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    view["arguments"]["term"]["default"] = json!("Gamma");
    fs::write(&path, serde_json::to_vec_pretty(&view).unwrap()).unwrap();
    let changes = repo.refresh_configuration().unwrap();
    assert_eq!(changes.affected_views, vec!["books_list"]);
    assert!(changes.affected_collections.is_empty());
    let result = repo.run_view("books_list", &BTreeMap::new()).unwrap();
    let ViewQueryResult::Text(matches) = &result.queries["matches"] else {
        panic!("text result expected")
    };
    assert_eq!(matches.hits[0].record.id, "c");
}

#[test]
fn added_single_view_requires_its_argument_and_is_reported_on_refresh() {
    let temp = fixture();
    let mut repo = Repository::open(temp.path()).unwrap();
    write(
        temp.path(),
        ".objekte/views/book_detail.view.json",
        json!({
            "name":"book_detail", "type":"single",
            "arguments":{"score":{"type":"integer"}},
            "queries":{"record":{"type":"json","request":{
                "filter":"score = $score", "parameters":{"score":{"$arg":"score"}},
                "collections":["books"], "limit":1
            }}}
        }),
    );
    let changes = repo.refresh_configuration().unwrap();
    assert_eq!(changes.affected_views, vec!["book_detail"]);
    assert!(repo.run_view("book_detail", &BTreeMap::new()).is_err());
    let result = repo
        .run_view("book_detail", &BTreeMap::from([("score".into(), json!(2))]))
        .unwrap();
    assert_eq!(result.kind, ViewType::Single);
    let ViewQueryResult::Json(record) = &result.queries["record"] else {
        panic!("JSON result expected")
    };
    assert_eq!(record.records[0].id, "b");
}

#[test]
fn required_string_argument_can_supply_a_text_pattern() {
    let temp = fixture();
    write(
        temp.path(),
        ".objekte/views/title_search.view.json",
        json!({
            "name":"title_search", "type":"list",
            "arguments":{"term":{"type":"string"}},
            "queries":{"titles":{"type":"text","request":{
                "pattern":{"$arg":"term"}, "collections":["books"], "count":true
            }}}
        }),
    );
    let mut repo = Repository::open(temp.path()).unwrap();
    let result = repo
        .run_view(
            "title_search",
            &BTreeMap::from([("term".into(), json!("Beta"))]),
        )
        .unwrap();
    let ViewQueryResult::Text(titles) = &result.queries["titles"] else {
        panic!("text result expected")
    };
    assert_eq!(titles.total, Some(1));
    assert_eq!(titles.hits[0].record.id, "b");
}
