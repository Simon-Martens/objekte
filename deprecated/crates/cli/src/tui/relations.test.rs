use super::*;
use std::fs;

#[test]
fn backlink_filters_find_declared_scalar_and_array_references() {
    assert_eq!(
        filter_path(&[PathStep::Field("a/b~c".into()), PathStep::Each]),
        "[\"a/b~c\"][*]"
    );
    let temp = tempfile::tempdir().unwrap();
    let notes = temp.path().join("data/notes");
    let books = temp.path().join("data/books");
    fs::create_dir_all(notes.join(".editor")).unwrap();
    fs::create_dir_all(&books).unwrap();
    fs::write(
        notes.join(".editor/schema.json"),
        r#"{"type":"object","properties":{"book":{"type":"string","x-relation":{"collection":"books"}},"links":{"type":"array","items":{"type":"object","properties":{"person":{"type":"string","x-relation":{"collection":"books"}}}}}}}"#,
    )
    .unwrap();
    fs::write(
        notes.join("a.json"),
        r#"{"book":"b","links":[{"person":"b"}]}"#,
    )
    .unwrap();
    fs::write(notes.join("c.json"), r#"{"book":"other"}"#).unwrap();
    fs::write(books.join("b.json"), r#"{"title":"B"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let requests = backlink_requests(&repo, "books", "b");
    assert_eq!(requests.len(), 1);
    assert!(requests[0].filter.contains("[\"book\"] ?= $target"));
    assert!(
        requests[0]
            .filter
            .contains("[\"links\"][*][\"person\"] ?= $target")
    );
    let mut records = Vec::new();
    repo.query_json_stream(&requests[0], |_, record| {
        records.push(record.clone());
        Ok(())
    })
    .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].id, "a");
    let matching = repo
        .references_in_record(&records[0])
        .unwrap()
        .into_iter()
        .filter(|edge| edge.target_collection == "books" && edge.target_id == "b")
        .collect::<Vec<_>>();
    assert_eq!(matching.len(), 2);
}
