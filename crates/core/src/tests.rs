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
fn categories_filter_records_and_inline_presentation_does_not_constrain_data() {
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
    let mut multiline = editor.get("records", "one").unwrap().value;
    multiline["note"] = json!("first\nsecond");
    assert!(editor.validate("records", &multiline).unwrap().is_empty());
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
fn media_search_tracks_explicit_references_only() {
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
    assert!(references.is_empty());
    editor.media_delete("images/a b.png").unwrap();
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

fn split_fixture() -> TempDir {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for dir in [".editor", "data/people", "data/notes", "media"] {
        fs::create_dir_all(root.join(dir)).unwrap();
    }
    let files = [
        (
            ".editor/repository.json",
            json!({"objects":".editor/objects.schema.json","ui":".editor/ui.json","media":"media","collections":{
            "people":{"path":"data/people","format":"json","id":"filename"},
            "notes":{"path":"data/notes","format":"json","id":"filename"}},
            "settings":"settings.json","documents":{"editor-settings":{"schema":".editor/settings.schema.json","files":["settings.json"]}}}),
        ),
        (
            ".editor/objects.schema.json",
            json!({"anyOf":[
                {"x-collection":"people","type":"object","required":["name"],"properties":{"name":{"type":"string"}}},
                {"x-collection":"notes","type":"object","required":["text"],"properties":{
                    "text":{"type":"string","x-content":"inline-markdown"},
                    "owner":{"type":"string","x-relation":{"collection":"people"}}}}
            ]}),
        ),
        (
            ".editor/ui.json",
            json!({"collections":{"notes":{"label":"Notes","display":"text","fields":{"/properties/text":{"title":"Text","x-editor":"inline-markdown"}}},"people":{"display":"name"}}}),
        ),
        (
            ".editor/settings.schema.json",
            json!({"type":"object","required":["title"],"properties":{"title":{"type":"string"}},"additionalProperties":false}),
        ),
        ("settings.json", json!({"title":"Example"})),
    ];
    for (path, value) in files {
        fs::write(root.join(path), serde_json::to_vec(&value).unwrap()).unwrap();
    }
    temp
}

#[test]
fn split_configuration_keeps_data_rules_independent_of_ui() {
    let temp = split_fixture();
    let editor = Editor::open(temp.path()).unwrap();
    let collection = editor.config().collection("notes").unwrap();
    assert!(
        collection
            .schema_value
            .pointer("/properties/text/x-editor")
            .is_none()
    );
    assert_eq!(
        collection
            .editor_schema
            .pointer("/properties/text/title")
            .unwrap(),
        "Text"
    );
    assert!(
        !editor
            .validate("notes", &json!({"text":"two\nlines"}))
            .unwrap()
            .is_empty()
    );
    assert!(
        !editor
            .validate("notes", &json!({"text":"ok","owner":"missing"}))
            .unwrap()
            .is_empty()
    );
    fs::write(temp.path().join(".editor/ui.json"), r#"{"collections":{}}"#).unwrap();
    let headless = Editor::open(temp.path()).unwrap();
    assert!(
        !headless
            .validate("notes", &json!({"text":"two\nlines"}))
            .unwrap()
            .is_empty()
    );
    assert!(
        !headless
            .validate("notes", &json!({"text":"ok","owner":"missing"}))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn split_configuration_rejects_invalid_ui_and_storage() {
    let temp = split_fixture();
    let ui = temp.path().join(".editor/ui.json");
    for value in [
        json!({"collections":{"missing":{}}}),
        json!({"collections":{"notes":{"fields":{"/properties/missing":{"title":"Missing"}}}}}),
        json!({"collections":{"notes":{"fields":{"/properties/text":{"type":"number"}}}}}),
    ] {
        fs::write(&ui, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(Editor::open(temp.path()).is_err());
    }
    fs::write(&ui, r#"{"collections":{}}"#).unwrap();
    let path = temp.path().join(".editor/repository.json");
    let mut repo: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    repo["settings"] = json!(".editor/objects.schema.json");
    fs::write(&path, serde_json::to_vec(&repo).unwrap()).unwrap();
    assert!(Editor::open(temp.path()).is_err());
}

#[test]
fn editor_instance_settings_validate_and_detect_stale_writes() {
    let temp = split_fixture();
    let editor = Editor::open(temp.path()).unwrap();
    let original = editor.settings().unwrap();
    assert!(matches!(
        editor.save_settings(json!({"title":42}), &original.revision),
        Err(Error::Validation(_))
    ));
    assert_eq!(editor.settings().unwrap().revision, original.revision);
    let saved = editor
        .save_settings(json!({"title":"New title"}), &original.revision)
        .unwrap();
    assert_eq!(editor.settings().unwrap().value, saved.value);
    assert!(matches!(
        editor.save_settings(json!({"title":"Stale"}), &original.revision),
        Err(Error::Conflict(_))
    ));
    fs::write(
        temp.path().join("settings.json"),
        r#"{"title":"External edit"}"#,
    )
    .unwrap();
    assert!(matches!(
        editor.save_settings(json!({"title":"Overwrite"}), &saved.revision),
        Err(Error::Conflict(_))
    ));
}

#[test]
fn bundled_projects_load_split_configuration() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    for name in ["project", "lenz-archiv"] {
        let editor = Editor::open(root.join(name)).unwrap();
        assert!(editor.settings_schema().is_ok());
        assert!(
            editor
                .validate_settings(&editor.settings().unwrap().value)
                .unwrap()
                .is_empty()
        );
        for name in editor.config().collections.keys() {
            assert!(editor.list(name, "", 0, 1).is_ok());
        }
    }
}

#[test]
fn split_hierarchy_constraints_survive_removing_sidebar_tree() {
    let temp = split_fixture();
    let root = temp.path();
    fs::create_dir(root.join("data/groups")).unwrap();
    let manifest = root.join(".editor/repository.json");
    let mut repo: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    repo["collections"]["groups"] = json!({"path":"data/groups","format":"json","id":"filename"});
    fs::write(manifest, serde_json::to_vec(&repo).unwrap()).unwrap();
    let schema_path = root.join(".editor/objects.schema.json");
    let mut schema: serde_json::Value =
        serde_json::from_slice(&fs::read(&schema_path).unwrap()).unwrap();
    schema["anyOf"].as_array_mut().unwrap().push(
        json!({"x-collection":"groups","type":"object","properties":{
            "parent":{"type":"string","x-relation":{"collection":"people"}}
        }}),
    );
    schema["anyOf"][1]["properties"]["group"] =
        json!({"type":"string","x-relation":{"collection":"groups"}});
    schema["anyOf"][1]["x-categoryConstraints"] = json!([
        {"field":"owner","label":"Owner","collection":"people"},
        {"field":"group","label":"Group","collection":"groups","parentField":"parent"}
    ]);
    fs::write(schema_path, serde_json::to_vec(&schema).unwrap()).unwrap();
    fs::write(root.join(".editor/ui.json"), r#"{"collections":{}}"#).unwrap();
    let editor = Editor::open(root).unwrap();
    editor
        .save("people", "a", json!({"name":"A"}), None)
        .unwrap();
    editor
        .save("people", "b", json!({"name":"B"}), None)
        .unwrap();
    let group = editor
        .save("groups", "g", json!({"parent":"a"}), None)
        .unwrap();
    assert!(
        !editor
            .validate("notes", &json!({"text":"Note","owner":"b","group":"g"}))
            .unwrap()
            .is_empty()
    );
    editor
        .save(
            "notes",
            "n",
            json!({"text":"Note","owner":"a","group":"g"}),
            None,
        )
        .unwrap();
    assert!(matches!(
        editor.save("groups", "g", json!({"parent":"b"}), Some(&group.revision)),
        Err(Error::Conflict(_))
    ));
}

fn add_document_mapping(
    root: &std::path::Path,
    name: &str,
    schema: serde_json::Value,
    files: &[&str],
) {
    let schema_path = format!(".editor/{name}.schema.json");
    fs::write(
        root.join(&schema_path),
        serde_json::to_vec(&schema).unwrap(),
    )
    .unwrap();
    let manifest = root.join(".editor/repository.json");
    let mut repo: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    repo["documents"][name] = json!({"schema":schema_path,"files":files});
    fs::write(manifest, serde_json::to_vec(&repo).unwrap()).unwrap();
}

#[test]
fn schemas_apply_to_one_or_multiple_json_files_at_arbitrary_locations() {
    let temp = split_fixture();
    let root = temp.path();
    fs::create_dir_all(root.join("config/nested")).unwrap();
    fs::write(root.join("config/nested/a.json"), "[1,2]").unwrap();
    fs::write(root.join(".editor/b.json"), "[3]").unwrap();
    add_document_mapping(
        root,
        "lists",
        json!({"type":"array","items":{"$ref":"#/$defs/item"},"$defs":{"item":{"type":"integer","minimum":0}}}),
        &["config/nested/a.json", ".editor/b.json"],
    );
    let editor = Editor::open(root).unwrap();
    assert_eq!(editor.document_files("lists").unwrap().len(), 2);
    for file in editor.document_files("lists").unwrap() {
        let original = editor.json_document("lists", file).unwrap();
        assert!(matches!(
            editor.save_json_document("lists", file, json!([-1]), &original.revision),
            Err(Error::Validation(_))
        ));
        let saved = editor
            .save_json_document("lists", file, json!([4]), &original.revision)
            .unwrap();
        assert_eq!(saved.value, json!([4]));
        assert!(matches!(
            editor.save_json_document("lists", file, json!([5]), &original.revision),
            Err(Error::Conflict(_))
        ));
    }
    assert!(editor.json_document("lists", "settings.json").is_err());
    assert!(editor.json_document("lists", "../outside.json").is_err());
    let current = editor
        .json_document("editor-settings", "settings.json")
        .unwrap();
    editor
        .save_json_document(
            "editor-settings",
            "settings.json",
            json!({"title":"Editor name"}),
            &current.revision,
        )
        .unwrap();
    assert_eq!(
        editor.settings().unwrap().value,
        json!({"title":"Editor name"})
    );
}

#[test]
fn document_mappings_reject_duplicate_protected_and_escaping_paths() {
    for file in [
        "settings.json",
        ".editor/repository.json",
        "../outside.json",
    ] {
        let temp = split_fixture();
        add_document_mapping(temp.path(), "invalid", json!({}), &[file]);
        assert!(Editor::open(temp.path()).is_err());
    }
}

#[test]
fn mapped_collection_files_keep_record_validation_and_reference_guards() {
    let temp = split_fixture();
    let root = temp.path();
    fs::write(root.join("data/people/a.json"), r#"{"name":"Alice"}"#).unwrap();
    add_document_mapping(
        root,
        "person",
        json!({"properties":{"name":{"minLength":3}}}),
        &["data/people/a.json"],
    );
    let editor = Editor::open(root).unwrap();
    let original = editor
        .json_document("person", "data/people/a.json")
        .unwrap();
    assert!(matches!(
        editor.save_json_document(
            "person",
            "data/people/a.json",
            json!({}),
            &original.revision
        ),
        Err(Error::Validation(_))
    ));
    assert!(matches!(
        editor.save("people", "a", json!({"name":"A"}), Some(&original.revision)),
        Err(Error::Validation(_))
    ));
    editor
        .save_json_document(
            "person",
            "data/people/a.json",
            json!({"name":"Anne"}),
            &original.revision,
        )
        .unwrap();
    let current = editor.get("people", "a").unwrap();
    assert_eq!(current.value, json!({"name":"Anne"}));
    assert!(matches!(
        editor.delete("people", "a", &current.revision),
        Err(Error::Conflict(_))
    ));
    assert!(matches!(
        editor.rename("people", "a", "b", current.value, &current.revision),
        Err(Error::Conflict(_))
    ));
}
