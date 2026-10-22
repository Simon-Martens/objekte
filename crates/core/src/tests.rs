use super::*;
use serde_json::json;
use std::fs;
use tempfile::TempDir;

fn fixture() -> (TempDir, Editor) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for dir in [
        ".editor",
        "data/records",
        "data/people",
        "data/articles",
        "media",
    ] {
        fs::create_dir_all(root.join(dir)).unwrap();
    }
    let schema = json!({
      "$schema":"https://json-schema.org/draft/2020-12/schema",
      "x-project":{"media":"media"},
      "oneOf":[
        {"title":"Person","x-collection":{"name":"people","label":"People","path":"data/people","format":"json","id":"filename","display":"name","search":["name"]},
         "type":"object","required":["kind","name"],"properties":{"kind":{"const":"person"},"name":{"type":"string"}}},
        {"title":"Record","x-collection":{"name":"records","label":"Records","path":"data/records","format":"json","id":"filename","display":"name","search":["name"]},
         "type":"object","required":["kind","name","nested"],"properties":{
          "kind":{"const":"record"},"name":{"type":"string"},"nothing":{"type":"null"},"integer":{"type":"integer"},
          "number":{"type":"number"},"flag":{"type":"boolean"},"nested":{"$ref":"#/$defs/nested"},
          "place":{"type":"string","x-editor":"autocomplete"},"group":{"type":"string","x-editor":"autocomplete-select","x-autocomplete":{"scope":["place"]}},
          "owner":{"type":"string","x-editor":"relation","x-relation":{"collection":"people"}},
          "asset":{"type":"string","x-editor":"media"},
          "choice":{"oneOf":[{"type":"string"},{"type":"integer"}]}
         },"additionalProperties":true}
      ],
      "$defs":{"nested":{"type":"array","items":{"type":"object","properties":{"v":{"type":["string","null"]}}}}}
    });
    fs::write(
        root.join(".editor/schema.json"),
        serde_json::to_vec(&schema).unwrap(),
    )
    .unwrap();
    let editor = Editor::open(root).unwrap();
    (temp, editor)
}

#[test]
fn collection_path_selects_overlapping_schema_without_kind_field() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for dir in [".editor", "data/people", "data/places", "media"] {
        fs::create_dir_all(root.join(dir)).unwrap();
    }
    let schema = json!({
        "$schema":"https://json-schema.org/draft/2020-12/schema",
        "x-project":{"media":"media"},
        "anyOf":[
            {"type":"object", "required":["name"], "properties":{"name":{"type":"string"}}, "additionalProperties":false,
             "x-collection":{"name":"people","label":"People","path":"data/people","format":"json","id":"filename","display":"name"}},
            {"type":"object", "required":["name"], "properties":{"name":{"type":"string"}}, "additionalProperties":false,
             "x-collection":{"name":"places","label":"Places","path":"data/places","format":"json","id":"filename","display":"name"}}
        ]
    });
    fs::write(
        root.join(".editor/schema.json"),
        serde_json::to_vec(&schema).unwrap(),
    )
    .unwrap();
    let editor = Editor::open(root).unwrap();
    let value = json!({"name":"Berlin"});
    editor
        .save("places", "berlin", value.clone(), None)
        .unwrap();
    assert_eq!(editor.get("places", "berlin").unwrap().value, value);
    assert!(matches!(
        editor.get("people", "berlin"),
        Err(Error::NotFound(_))
    ));
    assert!(
        !editor
            .validate("places", &json!({"name":42}))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn json_types_refs_relations_and_revisions() {
    let (_temp, editor) = fixture();
    let person = json!({"kind":"person","name":"Ada"});
    editor.save("people", "ada", person, None).unwrap();
    let value = json!({"kind":"record","name":"Record","nothing":null,"integer":1,"number":1.25,"flag":false,"nested":[{"v":null},{"v":"text"}],"owner":"ada","choice":12,"extra":{"a":[null,true,{}]}});
    let first = editor.save("records", "one", value.clone(), None).unwrap();
    assert_eq!(
        editor
            .resolve_relation("people", "ada", None)
            .unwrap()
            .label,
        "Ada"
    );
    assert_eq!(
        editor
            .search_relation("people", "Ada", None, None, 0, 10)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        editor.references_to("people", "ada").unwrap()[0].path,
        "/owner"
    );
    let person_revision = editor.get("people", "ada").unwrap().revision;
    assert!(matches!(
        editor.delete("people", "ada", &person_revision),
        Err(Error::Conflict(_))
    ));
    assert_eq!(editor.get("records", "one").unwrap().value, value);
    assert!(editor.save("records", "one", value.clone(), None).is_err());
    let changed = json!({"kind":"record","name":"Changed","nested":[]});
    editor
        .save("records", "one", changed.clone(), Some(&first.revision))
        .unwrap();
    assert!(matches!(
        editor.save("records", "one", value, Some(&first.revision)),
        Err(Error::Conflict(_))
    ));
    assert_eq!(editor.get("records", "one").unwrap().value, changed);
    editor.delete("people", "ada", &person_revision).unwrap();
    assert_eq!(editor.list("records", "Changed", 0, 10).unwrap().len(), 1);
}

#[test]
fn validation_reports_paths_and_missing_differs_from_null() {
    let (_temp, editor) = fixture();
    let invalid =
        json!({"kind":"record","name":"x","nested":[{"v":9}],"owner":"missing","choice":true});
    let issues = editor.validate("records", &invalid).unwrap();
    assert!(issues.iter().any(|i| i.path == "/nested/0/v"));
    assert!(issues.iter().any(|i| i.path == "/owner"));
    assert!(issues.iter().any(|i| i.path == "/choice"));
    let with_missing = json!({"kind":"record","name":"x","nested":[]});
    assert!(
        editor
            .validate("records", &with_missing)
            .unwrap()
            .is_empty()
    );
    let with_null = json!({"kind":"record","name":"x","nested":[],"integer":null});
    assert!(
        editor
            .validate("records", &with_null)
            .unwrap()
            .iter()
            .any(|i| i.path == "/integer")
    );
}

#[test]
fn record_ids_can_be_set_checked_and_generated() {
    let (_temp, editor) = fixture();
    let value = json!({"kind":"record","name":"A good record","nested":[]});
    assert_eq!(
        editor.generate_id("records", &value).unwrap(),
        "a-good-record"
    );
    assert!(
        editor
            .validate_new_id("records", "custom-id")
            .unwrap()
            .is_empty()
    );
    editor
        .save("records", "a-good-record", value.clone(), None)
        .unwrap();
    assert!(matches!(
        editor.save("records", "a-good-record", value.clone(), None),
        Err(Error::Validation(issues)) if issues.iter().any(|issue| issue.path == "/id")
    ));
    assert_eq!(
        editor.generate_id("records", &value).unwrap(),
        "a-good-record-2"
    );
    assert_eq!(
        editor.validate_new_id("records", "a-good-record").unwrap()[0].path,
        "/id"
    );
    assert_eq!(
        editor.validate_new_id("records", "bad/id").unwrap()[0].path,
        "/id"
    );
    editor.save("records", "custom-id", value, None).unwrap();
    assert_eq!(editor.get("records", "custom-id").unwrap().id, "custom-id");
}

#[test]
fn display_fields_build_labels_from_metadata_and_markdown() {
    let (temp, _) = fixture();
    let schema_path = temp.path().join(".editor/schema.json");
    let mut schema: serde_json::Value =
        serde_json::from_slice(&fs::read(&schema_path).unwrap()).unwrap();
    schema["oneOf"][1]["x-collection"]["displayFields"] = json!(["integer", "name"]);
    fs::write(&schema_path, serde_json::to_vec(&schema).unwrap()).unwrap();
    let editor = Editor::open(temp.path()).unwrap();
    editor
        .save(
            "records",
            "one",
            json!({"kind":"record","name":"**A title** with [a link](https://example.org)","integer":1776,"nested":[]}),
            None,
        )
        .unwrap();
    assert_eq!(
        editor.list("records", "", 0, 10).unwrap()[0].label,
        "1776 · A title with a link"
    );
}

#[test]
fn renaming_checks_revision_uniqueness_and_incoming_references() {
    let (_temp, editor) = fixture();
    let person = editor
        .save("people", "ada", json!({"kind":"person","name":"Ada"}), None)
        .unwrap();
    let record = editor
        .save(
            "records",
            "one",
            json!({"kind":"record","name":"One","nested":[],"owner":"ada"}),
            None,
        )
        .unwrap();
    editor
        .save(
            "records",
            "occupied",
            json!({"kind":"record","name":"Occupied","nested":[]}),
            None,
        )
        .unwrap();
    assert!(matches!(
        editor.rename(
            "people",
            "ada",
            "ada-new",
            person.value.clone(),
            &person.revision
        ),
        Err(Error::Conflict(_))
    ));
    assert!(matches!(
        editor.rename("records", "one", "two", record.value.clone(), "stale"),
        Err(Error::Conflict(_))
    ));
    assert!(matches!(
        editor.rename("records", "one", "occupied", record.value.clone(), &record.revision),
        Err(Error::Validation(issues)) if issues.iter().any(|issue| issue.path == "/id")
    ));
    let renamed = editor
        .rename("records", "one", "two", record.value, &record.revision)
        .unwrap();
    assert_eq!(renamed.id, "two");
    assert!(matches!(
        editor.get("records", "one"),
        Err(Error::NotFound(_))
    ));
    assert_eq!(
        editor.get("records", "two").unwrap().revision,
        renamed.revision
    );
    editor
        .save(
            "records",
            "two",
            json!({"kind":"record","name":"One","nested":[]}),
            Some(&renamed.revision),
        )
        .unwrap();
    assert!(matches!(
        editor.rename("people", "ada", "two", person.value, &person.revision),
        Ok(_)
    ));
}

#[test]
fn field_values_are_distinct_scoped_and_refreshed_after_writes() {
    let (_temp, editor) = fixture();
    let empty = std::collections::BTreeMap::new();
    let berlin = std::collections::BTreeMap::from([("place".into(), "Berlin".into())]);
    let paris = std::collections::BTreeMap::from([("place".into(), "Paris".into())]);
    assert!(
        editor
            .field_values("records", "place", &empty, "", 50)
            .is_ok()
    );
    let first = editor
        .save(
            "records",
            "one",
            json!({"kind":"record","name":"One","nested":[],"place":"Berlin","group":"A"}),
            None,
        )
        .unwrap();
    editor
        .save(
            "records",
            "two",
            json!({"kind":"record","name":"Two","nested":[],"place":"Berlin","group":"A"}),
            None,
        )
        .unwrap();
    editor
        .save(
            "records",
            "three",
            json!({"kind":"record","name":"Three","nested":[],"place":"Paris","group":"B"}),
            None,
        )
        .unwrap();
    assert_eq!(
        editor
            .field_values("records", "group", &empty, "", 50)
            .unwrap(),
        ["A", "B"]
    );
    assert_eq!(
        editor
            .field_values("records", "group", &berlin, "", 50)
            .unwrap(),
        ["A"]
    );
    assert_eq!(
        editor
            .field_values("records", "group", &paris, "", 50)
            .unwrap(),
        ["B"]
    );
    assert_eq!(
        editor
            .field_values("records", "group", &empty, "b", 50)
            .unwrap(),
        ["B"]
    );
    editor
        .save(
            "records",
            "one",
            json!({"kind":"record","name":"One","nested":[],"place":"Berlin","group":"C"}),
            Some(&first.revision),
        )
        .unwrap();
    assert_eq!(
        editor
            .field_values("records", "group", &berlin, "", 50)
            .unwrap(),
        ["A", "C"]
    );
    let renamed = editor.get("records", "two").unwrap();
    editor.delete("records", "two", &renamed.revision).unwrap();
    assert_eq!(
        editor
            .field_values("records", "group", &berlin, "", 50)
            .unwrap(),
        ["C"]
    );
}

#[test]
fn categories_filter_records_and_inline_markdown_is_one_line() {
    let (temp, _) = fixture();
    let schema_path = temp.path().join(".editor/schema.json");
    let mut schema: serde_json::Value =
        serde_json::from_slice(&fs::read(&schema_path).unwrap()).unwrap();
    let branch = &mut schema["oneOf"][1];
    branch["x-collection"]["categories"] = json!([
        {"field":"place","label":"Places"},
        {"field":"group","label":"Groups"}
    ]);
    branch["x-collection"]["fieldOrder"] = json!(["name", "place", "group", "note"]);
    branch["x-collection"]["categoryEntries"] = json!([
        {"path":["Berlin"],"label":"Berlin City"},
        {"path":["Berlin","A"],"order":3},
        {"path":["Berlin","B"],"order":1},
        {"path":["Berlin","Empty"],"description":"Context only","order":2}
    ]);
    branch["properties"]["place"] = json!({"type":"string"});
    branch["properties"]["group"] = json!({"type":"string"});
    branch["properties"]["note"] = json!({"type":"string","x-editor":"inline-markdown"});
    fs::write(&schema_path, serde_json::to_vec(&schema).unwrap()).unwrap();
    let editor = Editor::open(temp.path()).unwrap();
    for (id, place, group) in [
        ("one", "Berlin", Some("A")),
        ("two", "Berlin", Some("B")),
        ("three", "Bonn", None),
    ] {
        let mut value =
            json!({"kind":"record","name":id,"nested":[],"place":place,"note":"*short*"});
        if let Some(group) = group {
            value["group"] = json!(group);
        }
        editor.save("records", id, value, None).unwrap();
    }
    let tree = editor.categories("records").unwrap();
    assert_eq!(tree.len(), 2);
    assert_eq!(tree[0].label, "Berlin City");
    assert_eq!(tree[0].count, 2);
    assert_eq!(tree[0].children.len(), 3);
    assert_eq!(
        tree[0]
            .children
            .iter()
            .map(|node| node.value.as_str())
            .collect::<Vec<_>>(),
        ["B", "Empty", "A"]
    );
    assert_eq!(tree[0].children[0].order, Some(1));
    assert!(tree[0].children.iter().any(|node| node.value == "Empty"
        && node.count == 0
        && node.description.as_deref() == Some("Context only")));
    assert_eq!(
        editor
            .list_in_category("records", "", &["Berlin".into(), "A".into()], 0, 10)
            .unwrap()[0]
            .id,
        "one"
    );
    assert!(
        editor
            .list_in_category("records", "", &["Bonn".into()], 0, 10)
            .unwrap()
            .len()
            == 1
    );
    assert!(
        editor
            .list_in_category(
                "records",
                "",
                &["Berlin".into(), "A".into(), "extra".into()],
                0,
                10
            )
            .is_err()
    );
    assert_eq!(
        editor
            .resolve_relation("records", "one", Some("note"))
            .unwrap()
            .label,
        "short"
    );
    let mut bad = editor.get("records", "one").unwrap().value;
    bad["note"] = json!("first\nsecond");
    assert!(
        editor
            .validate("records", &bad)
            .unwrap()
            .iter()
            .any(|issue| issue.path == "/note")
    );
}

#[test]
fn linked_categories_are_records_and_keep_parent_integrity() {
    let (temp, _) = fixture();
    let schema_path = temp.path().join(".editor/schema.json");
    let mut schema: serde_json::Value =
        serde_json::from_slice(&fs::read(&schema_path).unwrap()).unwrap();
    fs::create_dir_all(temp.path().join("data/orte")).unwrap();
    fs::create_dir_all(temp.path().join("data/sammlungen")).unwrap();
    schema["oneOf"].as_array_mut().unwrap().extend([
        json!({
            "title":"Ort", "type":"object", "required":["kind","name"],
            "properties":{"kind":{"const":"ort"},"name":{"type":"string"},"description":{"type":"string"}},
            "x-collection":{"name":"orte","label":"Orte","path":"data/orte","format":"json","id":"filename","display":"name","search":["name"],"showInSidebar":false}
        }),
        json!({
            "title":"Sammlung", "type":"object", "required":["kind","Ort","name"],
            "properties":{"kind":{"const":"sammlung"},"Ort":{"type":"string","x-editor":"relation","x-relation":{"collection":"orte"}},"name":{"type":"string"},"order":{"type":"integer"}},
            "x-collection":{"name":"sammlungen","label":"Sammlungen","path":"data/sammlungen","format":"json","id":"filename","display":"name","search":["name"]}
        }),
    ]);
    let branch = &mut schema["oneOf"][1];
    branch["properties"]["place"] =
        json!({"type":"string","x-editor":"relation","x-relation":{"collection":"orte"}});
    branch["properties"]["group"] =
        json!({"type":"string","x-editor":"relation","x-relation":{"collection":"sammlungen"}});
    branch["x-collection"]["categories"] = json!([
        {"field":"place","label":"Places","collection":"orte","descriptionField":"description"},
        {"field":"group","label":"Groups","collection":"sammlungen","parentField":"Ort","orderField":"order"}
    ]);
    fs::write(&schema_path, serde_json::to_vec(&schema).unwrap()).unwrap();
    let original_schema = fs::read(&schema_path).unwrap();
    let editor = Editor::open(temp.path()).unwrap();
    assert!(!editor.config().collection("orte").unwrap().show_in_sidebar);
    editor
        .save(
            "orte",
            "berlin",
            json!({"kind":"ort","name":"Berlin","description":"Context"}),
            None,
        )
        .unwrap();
    editor
        .save("orte", "paris", json!({"kind":"ort","name":"Paris"}), None)
        .unwrap();
    editor
        .save(
            "sammlungen",
            "late",
            json!({"kind":"sammlung","Ort":"berlin","name":"Later","order":2}),
            None,
        )
        .unwrap();
    editor
        .save(
            "sammlungen",
            "early",
            json!({"kind":"sammlung","Ort":"berlin","name":"Earlier","order":1}),
            None,
        )
        .unwrap();
    let record = json!({"kind":"record","name":"One","nested":[],"place":"berlin","group":"late"});
    editor.save("records", "one", record, None).unwrap();
    let tree = editor.categories("records").unwrap();
    assert_eq!(tree.len(), 2);
    assert_eq!(tree[0].value, "berlin");
    assert_eq!(tree[0].label, "Berlin");
    assert_eq!(tree[0].description.as_deref(), Some("Context"));
    assert_eq!(tree[0].count, 1);
    assert_eq!(
        tree[0]
            .children
            .iter()
            .map(|node| node.value.as_str())
            .collect::<Vec<_>>(),
        ["early", "late"]
    );
    assert_eq!(tree[0].children[0].count, 0);
    assert_eq!(tree[0].children[1].count, 1);
    assert_eq!(tree[1].value, "paris");
    assert_eq!(tree[1].count, 0);
    assert_eq!(
        editor
            .list_in_category("records", "", &["berlin".into(), "late".into()], 0, 10)
            .unwrap()[0]
            .id,
        "one"
    );
    let wrong_parent =
        json!({"kind":"record","name":"Two","nested":[],"place":"paris","group":"late"});
    assert!(
        editor
            .validate("records", &wrong_parent)
            .unwrap()
            .iter()
            .any(|issue| issue.path == "/group")
    );
    let previous = editor.get("sammlungen", "late").unwrap();
    assert!(matches!(
        editor.save(
            "sammlungen",
            "late",
            json!({"kind":"sammlung","Ort":"paris","name":"Later","order":2}),
            Some(&previous.revision)
        ),
        Err(Error::Conflict(_))
    ));
    let changed = editor
        .save(
            "orte",
            "berlin",
            json!({"kind":"ort","name":"Berlin Mitte","description":"New context"}),
            Some(&editor.get("orte", "berlin").unwrap().revision),
        )
        .unwrap();
    assert_eq!(changed.value["name"], "Berlin Mitte");
    assert_eq!(
        editor.categories("records").unwrap()[0].label,
        "Berlin Mitte"
    );
    assert_eq!(fs::read(schema_path).unwrap(), original_schema);
}

#[test]
fn markdown_frontmatter_roundtrip_preserves_body_and_unknown_fields() {
    let (_temp, editor) = fixture();
    let root = editor.config().root.clone();
    let mut schema: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(".editor/schema.json")).unwrap()).unwrap();
    schema["oneOf"].as_array_mut().unwrap().push(json!({
        "title":"Article", "x-collection":{"name":"articles","label":"Articles","path":"data/articles","format":"markdown","id":"frontmatter.id","display":"title","search":["title"]},
        "type":"object","required":["kind","id","body"],
        "properties":{"kind":{"const":"article"},"id":{"type":"string"},"body":{"type":"string"}},"additionalProperties":true
    }));
    fs::write(
        root.join(".editor/schema.json"),
        serde_json::to_vec(&schema).unwrap(),
    )
    .unwrap();
    let editor = Editor::open(&root).unwrap();
    let source = "---\nkind: article\nid: article\nunknown: [1, 2]\n---\n\n# Heading\n\nBody\n";
    fs::write(root.join("data/articles/article.md"), source).unwrap();
    let doc = editor.get("articles", "article").unwrap();
    assert_eq!(doc.value["body"], "\n# Heading\n\nBody\n");
    assert_eq!(doc.value["unknown"], json!([1, 2]));
    let saved = editor
        .save(
            "articles",
            "article",
            doc.value.clone(),
            Some(&doc.revision),
        )
        .unwrap();
    assert_eq!(
        saved.value,
        editor.get("articles", "article").unwrap().value
    );
    assert!(matches!(
        editor.save(
            "articles",
            "article",
            json!({"kind":"article","id":"other","body":"x"}),
            Some(&saved.revision)
        ),
        Err(Error::Validation(_))
    ));
    let renamed = editor
        .rename(
            "articles",
            "article",
            "new-article",
            saved.value,
            &saved.revision,
        )
        .unwrap();
    assert_eq!(renamed.value["id"], "new-article");
    assert!(!root.join("data/articles/article.md").exists());
    assert!(root.join("data/articles/new-article.md").exists());
}

#[test]
fn media_operations_and_paths() {
    let (_temp, editor) = fixture();
    editor.media_mkdir("images").unwrap();
    editor.media_upload("images/a.png", b"abc", 10).unwrap();
    assert_eq!(editor.media_read("images/a.png").unwrap(), b"abc");
    assert!(
        editor
            .media_upload("images/too-big", b"12345678901", 10)
            .is_err()
    );
    editor.media_move("images/a.png", "images/b.png").unwrap();
    assert_eq!(editor.media_list("images", 0, 10).unwrap()[0].name, "b.png");
    editor.media_delete("images/b.png").unwrap();
    editor.media_delete("images").unwrap();
    assert!(editor.media_mkdir("../outside").is_err());
    assert!(editor.media_read("../outside").is_err());
    assert!(safe_name("a/b").is_err());
}

#[test]
fn media_search_and_markdown_references() {
    let (_temp, editor) = fixture();
    editor.media_mkdir("images").unwrap();
    editor.media_upload("images/a b.png", b"image", 10).unwrap();
    editor.media_upload("images/other.pdf", b"pdf", 10).unwrap();
    let value = json!({
        "kind":"record", "name":"Linked", "nested":[], "asset":"images/other.pdf",
        "comments":[
            "See [scan](/api/media/file?path=images%2Fa%20b.png) and ![image](/api/media/file?path=images%2Fa%20b.png)",
            "[external](https://example.com/image.png)"
        ]
    });
    editor.save("records", "linked", value, None).unwrap();
    let found = editor.media_search("A B", 0, 10).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].path, "images/a b.png");
    let references = editor.media_references("images/a b.png").unwrap();
    assert_eq!(references.len(), 1);
    assert_eq!(references[0].record_id, "linked");
    assert_eq!(references[0].path, "/comments/0");
    assert!(matches!(
        editor.media_delete("images/a b.png"),
        Err(Error::Conflict(_))
    ));
    assert!(matches!(
        editor.media_move("images", "renamed"),
        Err(Error::Conflict(_))
    ));
    let direct = editor.media_references("images/other.pdf").unwrap();
    assert_eq!(direct.len(), 1);
    assert_eq!(direct[0].path, "/asset");
}

#[test]
fn markdown_record_links_are_incoming_references() {
    let (_temp, editor) = fixture();
    editor
        .save("people", "ada", json!({"kind":"person","name":"Ada"}), None)
        .unwrap();
    editor
        .save(
            "records",
            "linked",
            json!({
                "kind":"record", "name":"Linked", "nested":[],
                "comments":["See [Ada](#/collections/people/ada)."]
            }),
            None,
        )
        .unwrap();
    let references = editor.references_to("people", "ada").unwrap();
    assert_eq!(references.len(), 1);
    assert_eq!(references[0].record_id, "linked");
    assert_eq!(references[0].path, "/comments/0");
}

#[cfg(unix)]
#[test]
fn symlinks_cannot_escape_collection_or_media() {
    use std::os::unix::fs::symlink;
    let (temp, editor) = fixture();
    let outside = temp
        .path()
        .parent()
        .unwrap()
        .join("outside-editor-test.json");
    fs::write(&outside, b"{}").unwrap();
    symlink(&outside, temp.path().join("data/records/escape.json")).unwrap();
    symlink(&outside, temp.path().join("media/escape.json")).unwrap();
    assert!(matches!(
        editor.get("records", "escape"),
        Err(Error::InvalidPath(_))
    ));
    assert!(matches!(
        editor.media_read("escape.json"),
        Err(Error::InvalidPath(_))
    ));
    fs::remove_file(outside).unwrap();
}
