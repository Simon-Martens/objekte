use super::*;
use serde_json::{Value, json};
use std::{fs, path::Path};
fn write(root: &Path, path: &str, v: Value) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
}
fn fixture() -> (tempfile::TempDir, Editor) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let definitions = [
        (
            "places",
            json!({"type":"object","required":["name"],"properties":{"name":{"type":"string"}}}),
            json!({"label":"Places","display":"name"}),
        ),
        (
            "groups",
            json!({"type":"object","required":["name","place"],"properties":{"name":{"type":"string"},"place":{"type":"string","x-relation":{"collection":"places"}}}}),
            json!({"label":"Groups","display":"name"}),
        ),
        (
            "records",
            json!({"type":"object","required":["name"],"properties":{"name":{"type":"string","minLength":1},"place":{"type":"string","x-relation":{"collection":"places"},"x-derived":{"via":"group","field":"place","fallback":true}},"group":{"type":"string","x-relation":{"collection":"groups"}},"asset":{"type":"string","x-content":"media","x-media":{"collection":"assets"}},"source":{"type":"string","x-ownership":{"kind":"imported","editable":false}},"flag":{"type":"boolean","default":false},"count":{"type":"integer"},"date":{"type":"string","format":"date"},"time":{"type":"string","format":"date-time"},"body":{"type":"string","x-content":"markdown"},"nested":{"type":"array","items":{"type":"array","items":{"type":"integer"}}}},"additionalProperties":false}),
            json!({"label":"Records","display":"name","search":["name","place"],"ordering":{"by":"manual","unordered":"last"},"categories":[{"field":"place","label":"Places","collection":"places"},{"field":"group","label":"Groups","collection":"groups","parentField":"place"}],"fields":{"/properties/source":{"x-visible":false}}}),
        ),
    ];
    let mut registrations = json!({});
    for (name, schema, ui) in definitions {
        let prefix = format!(".editor/{name}");
        write(root, &format!("{prefix}/schema.json"), schema);
        write(root, &format!("{prefix}/ui.json"), ui);
        write(
            root,
            &format!("{prefix}/settings.json"),
            json!({"type":"data","path":format!("data/{name}"),"format":"json","id":"filename","schema":format!("{prefix}/schema.json"),"ui":format!("{prefix}/ui.json"),"operations":["read","create","update","rename","delete"]}),
        );
        fs::create_dir_all(root.join(format!("data/{name}"))).unwrap();
        registrations[name] = json!(format!("{prefix}/settings.json"));
    }
    for name in ["assets", "downloads"] {
        fs::create_dir(root.join(name)).unwrap();
        write(
            root,
            &format!(".editor/{name}/settings.json"),
            json!({"type":"media","path":name,"ui":format!(".editor/{name}/ui.json"),"operations":["read","upload","mkdir","move","delete"]}),
        );
        write(
            root,
            &format!(".editor/{name}/ui.json"),
            json!({"label":name}),
        );
        registrations[name] = json!(format!(".editor/{name}/settings.json"));
    }
    write(
        root,
        ".editor/repository.json",
        json!({"version":2,"ui":".editor/ui.json","settings":"settings.json","collections":registrations,"documents":{"settings":{"schema":".editor/settings.schema.json","files":["settings.json"]}}}),
    );
    write(
        root,
        ".editor/ui.json",
        json!({"collections":{},"defaults":{}}),
    );
    write(root, "settings.json", json!({}));
    write(
        root,
        ".editor/settings.schema.json",
        json!({"type":"object"}),
    );
    let editor = Editor::open(root).unwrap();
    (temp, editor)
}
#[test]
fn independent_collections_derive_relationships_and_enforce_ownership() {
    let (t, e) = fixture();
    e.save("places", "a", json!({"name":"A"}), None).unwrap();
    e.save("places", "b", json!({"name":"B"}), None).unwrap();
    let group = e
        .save("groups", "g", json!({"name":"Group","place":"a"}), None)
        .unwrap();
    write(
        t.path(),
        "data/records/one.json",
        json!({"name":"One","group":"g","place":"a","source":"imported"}),
    );
    let doc = e.get("records", "one").unwrap();
    assert_eq!(e.view_value("records", &doc.value).unwrap()["place"], "a");
    e.save(
        "groups",
        "g",
        json!({"name":"Group","place":"b"}),
        Some(&group.revision),
    )
    .unwrap();
    assert_eq!(e.view_value("records", &doc.value).unwrap()["place"], "b");
    // Unchanged legacy duplicates must not protect the former derived place.
    assert!(e.references_to("places", "a").unwrap().is_empty());
    assert_eq!(e.references_to("places", "b").unwrap().len(), 2);
    e.save(
        "records",
        "direct",
        json!({"name":"Direct fallback", "place":"a"}),
        None,
    )
    .unwrap();
    assert_eq!(e.references_to("places", "a").unwrap().len(), 1);
    let saved = e
        .save("records", "one", doc.value.clone(), Some(&doc.revision))
        .unwrap();
    assert!(saved.value.get("place").is_none());
    assert_eq!(
        e.list_in_category("records", "", &["b".into(), "g".into()], 0, 10)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        e.categories("records")
            .unwrap()
            .iter()
            .find(|n| n.value == "b")
            .unwrap()
            .count,
        1
    );
    let mut tamper = saved.value.clone();
    tamper["source"] = json!("changed");
    assert!(matches!(
        e.save("records", "one", tamper, Some(&saved.revision)),
        Err(Error::Validation(_))
    ));
    let mut raw = saved.value.clone();
    raw.as_object_mut().unwrap().remove("source");
    assert!(
        e.save("records", "one", raw, Some(&saved.revision))
            .is_err()
    );
    let initial = e.initial_value("records", &json!({})).unwrap();
    assert_eq!(initial, json!({"flag":false}));
    assert!(initial.get("count").is_none());
    let d = e.draft("records", &saved.value).unwrap();
    assert!(
        d.fields
            .iter()
            .find(|f| f.key == "place")
            .unwrap()
            .read_only
    );
}
#[test]
fn media_collections_track_only_explicit_fields_in_their_own_collection() {
    let (_temp, e) = fixture();
    let a = e.for_media("assets").unwrap();
    let b = e.for_media("downloads").unwrap();
    a.media_upload("same.png", b"a", 100).unwrap();
    b.media_upload("same.png", b"b", 100).unwrap();
    a.media_upload("link.png", b"c", 100).unwrap();
    e.save("records","r",json!({"name":"Record","asset":"same.png","body":"![Image](/media/assets/file?path=link.png)"}),None).unwrap();
    assert_eq!(a.media_references("same.png").unwrap().len(), 1);
    assert!(b.media_references("same.png").unwrap().is_empty());
    b.media_delete("same.png").unwrap();
    a.media_delete("link.png").unwrap();
    assert!(a.media_delete("same.png").is_err());
    assert!(a.media_move("same.png", "renamed.png").is_err());
    assert!(a.media_read("../settings.json").is_err());
}
#[test]
fn formats_manual_order_and_typed_nested_array_drafts() {
    let (_temp, e) = fixture();
    assert!(
        !e.validate("records", &json!({"name":"X","date":"2026-02-30"}))
            .unwrap()
            .is_empty()
    );
    assert!(
        !e.validate("records", &json!({"name":"X","time":"2026-10-22T12:00:00"}))
            .unwrap()
            .is_empty()
    );
    let timestamp = "2026-10-22T12:00:00.123+02:00";
    let d = e
        .save(
            "records",
            "z",
            json!({"name":"Z","time":timestamp,"_editor":{"manual_order":-1}}),
            None,
        )
        .unwrap();
    assert_eq!(d.value["time"], timestamp);
    e.save("records", "a", json!({"name":"A"}), None).unwrap();
    assert_eq!(e.list("records", "", 0, 10).unwrap()[0].id, "z");
    let mut draft = e
        .draft("records", &json!({"name":"X","nested":[[1,2]]}))
        .unwrap();
    let array = draft.fields.iter().find(|f| f.key == "nested").unwrap();
    let nested = array.items[0].id.clone();
    draft
        .array_action(
            &e.config().collection("records").unwrap().editor_schema,
            &nested,
            "add",
            None,
        )
        .unwrap();
    assert!(draft.value().is_err());
}
#[test]
fn markdown_link_insertion_handles_unicode_and_does_not_execute_source() {
    assert_eq!(
        insert_markdown_link(
            "A😀B",
            1,
            3,
            "/collections/places/records/a",
            "Place",
            false
        )
        .unwrap(),
        "A[Place](/collections/places/records/a)B"
    );
    assert!(insert_markdown_link("hello", 0, 5, "javascript:alert(1)", "x", false).is_err());
    assert!(insert_markdown_link("😀", 1, 2, "/x", "x", false).is_err());
}
#[test]
fn committed_schema_revision_is_exact_and_not_a_file_revision() {
    let (t, e) = fixture();
    fn git(root: &Path, args: &[&str]) -> std::process::Output {
        let o = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        o
    }
    git(t.path(), &["init", "-q"]);
    git(t.path(), &["add", ".editor"]);
    git(
        t.path(),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.test",
            "commit",
            "-qm",
            "schema",
        ],
    );
    let hash = String::from_utf8(git(t.path(), &["rev-parse", "HEAD"]).stdout)
        .unwrap()
        .trim()
        .to_owned();
    assert_eq!(
        e.schema_revision("records").unwrap().as_deref(),
        Some(hash.as_str())
    );
    let doc = e
        .save("records", "new", json!({"name":"New"}), None)
        .unwrap();
    assert_eq!(doc.value["_editor"]["schema_revision"], hash);
    assert_ne!(doc.revision, hash);
    let path = t.path().join(".editor/records/schema.json");
    let mut bytes = fs::read(&path).unwrap();
    bytes.push(b'\n');
    fs::write(path, bytes).unwrap();
    assert!(e.schema_revision("records").unwrap().is_none());
    assert_eq!(
        e.get("records", "new").unwrap().value["_editor"]["schema_revision"],
        hash
    );
    let saved = e
        .save("records", "new", doc.value, Some(&doc.revision))
        .unwrap();
    assert!(saved.value.pointer("/_editor/schema_revision").is_none());
}

#[test]
fn custom_markdown_mapping_operations_and_view_labels() {
    let (t, _) = fixture();
    let settings_path = t.path().join(".editor/records/settings.json");
    let mut settings: Value = serde_json::from_slice(&fs::read(&settings_path).unwrap()).unwrap();
    settings["format"] = json!("markdown");
    settings["id"] = json!("frontmatter.id");
    settings["content"] = json!({"body":"text","frontmatter":"*"});
    settings["operations"] = json!(["read", "create", "update"]);
    fs::write(settings_path, settings.to_string()).unwrap();
    write(
        t.path(),
        ".editor/records/schema.json",
        json!({"type":"object","required":["id","name","text"],"properties":{"id":{"type":"string"},"name":{"type":"string"},"text":{"type":"string"}}}),
    );
    write(
        t.path(),
        ".editor/records/ui.json",
        json!({"display":"name","labels":{"list":["name"],"reference":["name","id"]}}),
    );
    let e = Editor::open(t.path()).unwrap();
    let doc = e
        .save(
            "records",
            "one",
            json!({"name":"**Name**","text":"Body\n\nnext"}),
            None,
        )
        .unwrap();
    assert_eq!(doc.value["id"], "one");
    assert_eq!(
        e.get("records", "one").unwrap().value["text"],
        "Body\n\nnext"
    );
    assert_eq!(
        e.label("records", &doc.value, "one", "reference").unwrap(),
        "Name · one"
    );
    assert!(
        !fs::read_to_string(t.path().join("data/records/one.md"))
            .unwrap()
            .contains("text:")
    );
    assert!(e.delete("records", "one", &doc.revision).is_err());
    assert!(
        e.rename("records", "one", "two", doc.value, &doc.revision)
            .is_err()
    );
}

#[test]
fn list_queries_count_sort_filter_and_explain_search_matches() {
    let (t, e) = fixture();
    let path = t.path().join(".editor/records/ui.json");
    let mut ui: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    ui["list"] = json!({"filters":["count","flag","name"]});
    ui["search"] = json!(["name", "body"]);
    write(t.path(), ".editor/records/ui.json", ui);
    e.save(
        "records",
        "a",
        json!({"name":"Alpha","count":10,"flag":false,"body":"Hidden needle in body"}),
        None,
    )
    .unwrap();
    e.save(
        "records",
        "b",
        json!({"name":"Beta","count":2,"flag":true}),
        None,
    )
    .unwrap();
    e.save("records", "c", json!({"name":"Gamma"}), None)
        .unwrap();
    let e = Editor::open(t.path()).unwrap();
    let mut query = ListRequest {
        limit: 1,
        sort: "count".into(),
        ..Default::default()
    };
    let page = e.query_collection("records", &query).unwrap();
    assert_eq!(page.total, 3);
    assert_eq!(page.matched, 3);
    assert_eq!(page.rows[0].id, "b");
    query.query = "NEEDLE".into();
    let page = e.query_collection("records", &query).unwrap();
    assert_eq!(page.matched, 1);
    assert_eq!(page.rows[0].matches[0].field, "body");
    query.query.clear();
    query.filters.insert("count.min".into(), "5".into());
    assert_eq!(
        e.query_collection("records", &query).unwrap().rows[0].id,
        "a"
    );
    query.filters.clear();
    query.filters.insert("flag".into(), "false".into());
    assert_eq!(e.query_collection("records", &query).unwrap().matched, 1);
    query.filters.insert("source".into(), "secret".into());
    assert!(e.query_collection("records", &query).is_err());
}
#[test]
fn replacing_media_preserves_paths_checks_revision_and_requires_permission() {
    let (t, e) = fixture();
    let media = e.for_media("assets").unwrap();
    media.media_upload("file.txt", b"old", 50).unwrap();
    let revision = media.media_revision("file.txt").unwrap();
    assert!(
        media
            .media_replace("file.txt", b"new", &revision, 50)
            .is_err()
    );
    let path = t.path().join(".editor/assets/settings.json");
    let mut settings: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    settings["operations"] = json!(["read", "upload", "replace", "copy"]);
    write(t.path(), ".editor/assets/settings.json", settings);
    let media = Editor::open(t.path()).unwrap().for_media("assets").unwrap();
    media
        .media_replace("file.txt", b"new", &revision, 50)
        .unwrap();
    assert_eq!(media.media_read("file.txt").unwrap(), b"new");
    assert!(
        media
            .media_replace("file.txt", b"stale", &revision, 50)
            .is_err()
    );
    media.media_copy("file.txt", "copy.txt").unwrap();
    assert_eq!(media.media_read("copy.txt").unwrap(), b"new");
    assert!(media.media_copy("file.txt", "copy.txt").is_err());
    assert!(
        media
            .media_replace("../settings.json", b"{}", "x", 50)
            .is_err()
    );
}

fn hierarchy_fixture() -> (tempfile::TempDir, Editor) {
    let (t, _) = fixture();
    for (name, children) in [
        ("places", vec![("groups", "groups"), ("records", "records")]),
        ("groups", vec![("records", "records")]),
        ("records", vec![]),
    ] {
        let mut schema = json!({"type":"object","required":["name"],"properties":{"name":{"type":"string","minLength":1}},"additionalProperties":false});
        for (field, target) in children {
            schema["properties"][field] = json!({"type":"array","uniqueItems":true,"x-children":true,"items":{"type":"string","x-relation":{"collection":target}}});
        }
        write(t.path(), &format!(".editor/{name}/schema.json"), schema);
        write(
            t.path(),
            &format!(".editor/{name}/ui.json"),
            json!({"display":"name","label":name}),
        );
    }
    let e = Editor::open(t.path()).unwrap();
    (t, e)
}
#[test]
fn parent_owned_lists_support_direct_grouped_and_shared_records() {
    let (_t, e) = hierarchy_fixture();
    let p = e
        .save("places", "p", json!({"name":"Österreich"}), None)
        .unwrap();
    e.create_child(
        "places",
        "p",
        "groups",
        &p.revision,
        "g",
        json!({"name":"Collection"}),
    )
    .unwrap();
    let g = e.get("groups", "g").unwrap();
    e.create_child(
        "groups",
        "g",
        "records",
        &g.revision,
        "r",
        json!({"name":"Édition"}),
    )
    .unwrap();
    let p = e.get("places", "p").unwrap();
    e.set_child_membership("places", "p", "records", "r", true, &p.revision)
        .unwrap();
    assert_eq!(e.parent_links("records", "r").unwrap().len(), 2);
    let tree = e.hierarchy("places").unwrap();
    assert_eq!(tree[0].children.len(), 2);
    assert!(tree[0].children[0].branch);
    assert_eq!(tree[0].children[0].children[0].id, "r");
    let parent = e.get("places", "p").unwrap();
    let draft = e.draft("places", &parent.value).unwrap();
    assert!(
        draft
            .fields
            .iter()
            .filter(|f| f.key != "name" && f.key != "_editor")
            .all(|f| f.hidden)
    );
    assert_eq!(draft.value().unwrap(), parent.value);
    assert!(
        e.delete("records", "r", &e.get("records", "r").unwrap().revision)
            .is_err()
    );
    assert!(
        e.set_child_membership("places", "p", "records", "r", false, &p.revision)
            .is_err()
    );
    e.set_child_membership("places", "p", "records", "r", false, &parent.revision)
        .unwrap();
    assert_eq!(e.parent_links("records", "r").unwrap().len(), 1);
    assert!(e.get("records", "r").is_ok());
    let page = e
        .query_collection(
            "records",
            &ListRequest {
                ids: Some(e.child_ids("places", "p", "records").unwrap()),
                limit: 20,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(page.total, 0);
    // Removing the final placement leaves a valid, independently browsable record.
    let record = e.get("records", "r").unwrap();
    let group = e.get("groups", "g").unwrap();
    e.set_child_membership("groups", "g", "records", "r", false, &group.revision)
        .unwrap();
    assert!(e.parent_links("records", "r").unwrap().is_empty());
    assert_eq!(e.get("records", "r").unwrap().value, record.value);
    assert!(e.validate("records", &record.value).unwrap().is_empty());
    let page = e
        .query_collection("records", &ListRequest::default())
        .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.rows[0].id, "r");
}
#[test]
fn child_creation_checks_revision_permissions_and_rolls_back_failed_attachments() {
    let (t, e) = hierarchy_fixture();
    let p = e.save("places", "p", json!({"name":"P"}), None).unwrap();
    assert!(
        e.create_child(
            "places",
            "p",
            "records",
            "stale",
            "new",
            json!({"name":"New"})
        )
        .is_err()
    );
    assert!(e.get("records", "new").is_err());
    let path = t.path().join(".editor/places/schema.json");
    let mut schema: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    schema["properties"]["records"]["maxItems"] = json!(0);
    write(t.path(), ".editor/places/schema.json", schema);
    let e = Editor::open(t.path()).unwrap();
    assert!(
        e.create_child(
            "places",
            "p",
            "records",
            &p.revision,
            "new",
            json!({"name":"New"})
        )
        .is_err()
    );
    assert!(e.get("records", "new").is_err());
    assert_eq!(e.get("places", "p").unwrap().revision, p.revision);
    let path = t.path().join(".editor/places/settings.json");
    let mut config: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    config["operations"] = json!(["read"]);
    write(t.path(), ".editor/places/settings.json", config);
    let e = Editor::open(t.path()).unwrap();
    assert!(
        e.create_child(
            "places",
            "p",
            "groups",
            &p.revision,
            "g",
            json!({"name":"Group"})
        )
        .is_err()
    );
    assert!(e.get("groups", "g").is_err());
}
#[test]
fn hierarchy_schemas_reject_cycles_and_duplicate_child_references() {
    let (t, e) = hierarchy_fixture();
    e.save("records", "r", json!({"name":"R"}), None).unwrap();
    assert!(
        e.save("places", "p", json!({"name":"P","records":["r","r"]}), None)
            .is_err()
    );
    assert!(
        e.save(
            "places",
            "p",
            json!({"name":"P","records":["missing"]}),
            None
        )
        .is_err()
    );
    write(
        t.path(),
        ".editor/records/schema.json",
        json!({"type":"object","properties":{"name":{"type":"string"},"parents":{"type":"array","uniqueItems":true,"x-children":true,"items":{"type":"string","x-relation":{"collection":"places"}}}}}),
    );
    assert!(Editor::open(t.path()).is_err());
}
#[test]
fn unicode_collection_sort_is_accent_aware_in_both_directions() {
    let (_t, e) = hierarchy_fixture();
    for (id, name) in [
        ("z", "Zürich"),
        ("o", "Österreich"),
        ("b", "Berlin"),
        ("a", "Ägypten"),
        ("e", "Édition"),
    ] {
        e.save("records", id, json!({"name":name}), None).unwrap();
    }
    for key in ["_label", "name"] {
        let mut request = ListRequest {
            sort: key.into(),
            limit: 20,
            ..Default::default()
        };
        let ids = |r: &ListRequest| {
            e.query_collection("records", r)
                .unwrap()
                .rows
                .into_iter()
                .map(|r| r.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&request), vec!["a", "b", "e", "o", "z"]);
        request.descending = true;
        assert_eq!(ids(&request), vec!["z", "o", "e", "b", "a"]);
    }
}
#[test]
fn migrated_lenz_records_and_existing_parent_links_validate() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/lenz-archiv");
    let e = Editor::open(root).unwrap();
    for name in ["orte", "sammlungen", "handschriften"] {
        let rows: Vec<_> = (0..10000)
            .step_by(100)
            .map(|offset| e.list(name, "", offset, 100).unwrap())
            .take_while(|p| !p.is_empty())
            .flatten()
            .collect();
        for row in rows {
            let doc = e.get(name, &row.id).unwrap();
            assert!(
                e.validate(name, &doc.value).unwrap().is_empty(),
                "{name}/{}",
                row.id
            );
            assert!(doc.value.get("Ort").is_none() && doc.value.get("Sammlung").is_none());
            if name != "handschriften" {
                for id in e.child_ids(name, &row.id, "handschriften").unwrap() {
                    assert!(
                        e.get("handschriften", &id).is_ok(),
                        "{name}/{} links to missing manuscript {id}",
                        row.id
                    );
                }
            }
        }
    }
}

#[test]
fn missing_child_files_do_not_break_navigation_and_membership_can_be_repaired() {
    let (t, e) = hierarchy_fixture();
    e.save("records", "r", json!({"name":"Record"}), None)
        .unwrap();
    let parent = e
        .save("places", "p", json!({"name":"Place","records":["r"]}), None)
        .unwrap();
    fs::remove_file(t.path().join("data/records/r.json")).unwrap();
    assert!(e.hierarchy("places").unwrap()[0].children.is_empty());
    assert!(!e.validate("places", &parent.value).unwrap().is_empty());
    e.set_child_membership("places", "p", "records", "r", false, &parent.revision)
        .unwrap();
    assert!(e.child_ids("places", "p", "records").unwrap().is_empty());
}
