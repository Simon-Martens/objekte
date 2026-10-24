use super::*;

fn write(root: &std::path::Path, path: &str, value: serde_json::Value) {
    fs::write(root.join(path), serde_json::to_vec_pretty(&value).unwrap()).unwrap();
}

#[test]
fn rename_updates_nested_self_mapped_settings_and_markdown_references_once() {
    let temp = split_fixture();
    let root = temp.path();
    let schema_path = root.join(".editor/objects.schema.json");
    let mut schema: serde_json::Value =
        serde_json::from_slice(&fs::read(&schema_path).unwrap()).unwrap();
    let relation = json!({"type":"string","x-relation":{"collection":"people"}});
    schema["anyOf"][0]["properties"]["self"] = relation.clone();
    schema["anyOf"][1]["properties"]["owners"] = json!({"type":"array","items":relation});
    schema["anyOf"][1]["properties"]["nested"] =
        json!({"type":"object","properties":{"a/b~c":relation}});
    schema["anyOf"][1]["properties"]["foreign"] =
        json!({"type":"string","x-relation":{"collection":"notes"}});
    write(root, ".editor/objects.schema.json", schema);
    write(
        root,
        ".editor/settings.schema.json",
        json!({"type":"object","properties":{"title":{"type":"string"},"featured":relation}}),
    );
    write(
        root,
        "settings.json",
        json!({"title":"ada","featured":"ada"}),
    );
    write(root, "links.json", json!(["ada", "ada"]));
    add_document_mapping(
        root,
        "links",
        json!({"type":"array","items":relation}),
        &["links.json"],
    );
    write(
        root,
        "data/notes/source.json",
        json!({"text":"ada","owner":"ada"}),
    );
    // The same reference is covered by both a collection and a document schema.
    add_document_mapping(
        root,
        "source",
        json!({"type":"object","properties":{"owner":relation}}),
        &["data/notes/source.json"],
    );
    let editor = Editor::open(root).unwrap();
    let person = editor
        .save("people", "ada", json!({"name":"Ada"}), None)
        .unwrap();
    let person = editor
        .save(
            "people",
            "ada",
            json!({"name":"Ada","self":"ada"}),
            Some(&person.revision),
        )
        .unwrap();
    editor
        .save("notes", "ada", json!({"text":"Another collection"}), None)
        .unwrap();
    let previous = editor.get("notes", "source").unwrap();
    let body = "[Ada](/collections/people/records/ada \"Title\") [Ada][p] and [again][p].\n\n[p]: <#/collections/people/ada> 'kept title'\n\n`[code](/collections/people/records/ada)`\n";
    let source = editor.save("notes", "source", json!({"text":"ada","owner":"ada","owners":["ada","ada"],"nested":{"a/b~c":"ada"},"foreign":"ada","body":body}), Some(&previous.revision)).unwrap();
    let result = editor
        .rename_with_references(
            "people",
            "ada",
            "Renée 2",
            json!({"name":"Renée","self":"ada"}),
            &person.revision,
        )
        .unwrap();
    assert_eq!(result.references_renamed, 11);
    assert!(editor.get("people", "ada").is_err());
    assert_eq!(result.document.value["self"], "Renée 2");
    let updated = editor.get("notes", "source").unwrap();
    assert_eq!(updated.value["owner"], "Renée 2");
    assert_eq!(updated.value["owners"], json!(["Renée 2", "Renée 2"]));
    assert_eq!(updated.value["nested"]["a/b~c"], "Renée 2");
    assert_eq!(updated.value["foreign"], "ada");
    assert_eq!(updated.value["text"], "ada");
    assert_eq!(
        updated.value["body"],
        body.replacen("records/ada", "records/Ren%C3%A9e%202", 1)
            .replace("people/ada>", "people/Ren%C3%A9e%202>")
    );
    assert_eq!(
        editor.settings().unwrap().value,
        json!({"title":"ada","featured":"Renée 2"})
    );
    assert_eq!(
        editor.json_document("links", "links.json").unwrap().value,
        json!(["Renée 2", "Renée 2"])
    );
    assert!(editor.references_to("people", "ada").unwrap().is_empty());
    assert!(
        editor
            .save("notes", "source", source.value, Some(&source.revision))
            .is_err()
    );
    // Automatic reference edits are persisted, not merely overlaid in the original Editor.
    assert_eq!(
        Editor::open(root)
            .unwrap()
            .get("notes", "source")
            .unwrap()
            .value,
        updated.value
    );
}

#[test]
fn rename_failures_leave_all_original_files_unchanged() {
    let temp = split_fixture();
    let root = temp.path();
    let editor = Editor::open(root).unwrap();
    let person = editor
        .save("people", "ada", json!({"name":"Ada"}), None)
        .unwrap();
    editor
        .save("notes", "one", json!({"text":"First","owner":"ada"}), None)
        .unwrap();
    editor
        .save("notes", "two", json!({"text":"Second","owner":"ada"}), None)
        .unwrap();
    let original: Vec<_> = [
        "data/people/ada.json",
        "data/notes/one.json",
        "data/notes/two.json",
    ]
    .into_iter()
    .map(|path| (path, fs::read(root.join(path)).unwrap()))
    .collect();
    for invalid in [json!({"name":42}), json!({"name":"Ada"})] {
        let expected = if invalid["name"].is_string() {
            "stale"
        } else {
            &person.revision
        };
        assert!(
            editor
                .rename("people", "ada", "new", invalid, expected)
                .is_err()
        );
        for (path, bytes) in &original {
            assert_eq!(&fs::read(root.join(path)).unwrap(), bytes);
        }
        assert!(!root.join("data/people/new.json").exists());
    }
    let path = root.join(".editor/objects.schema.json");
    let mut schema: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    schema["anyOf"][1]["properties"]["owner"]["pattern"] = json!("^ada$");
    write(root, ".editor/objects.schema.json", schema.clone());
    let editor = Editor::open(root).unwrap();
    assert!(matches!(
        editor.rename(
            "people",
            "ada",
            "new",
            person.value.clone(),
            &person.revision
        ),
        Err(Error::Validation(_))
    ));
    for (path, bytes) in &original {
        assert_eq!(&fs::read(root.join(path)).unwrap(), bytes);
    }
    assert!(!root.join("data/people/new.json").exists());
    schema["anyOf"][1]["properties"]["owner"]
        .as_object_mut()
        .unwrap()
        .remove("pattern");
    schema["anyOf"][1]["properties"]["owner"]["x-ownership"] = json!("immutable");
    write(root, ".editor/objects.schema.json", schema);
    let editor = Editor::open(root).unwrap();
    assert!(
        editor
            .rename("people", "ada", "new", person.value, &person.revision)
            .is_err()
    );
    for (path, bytes) in &original {
        assert_eq!(&fs::read(root.join(path)).unwrap(), bytes);
    }
}

#[test]
fn frontmatter_rename_updates_identity_links_and_references() {
    let temp = split_fixture();
    let root = temp.path();
    let mut config: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(".editor/repository.json")).unwrap()).unwrap();
    config["collections"]["notes"]["format"] = json!("markdown");
    config["collections"]["notes"]["id"] = json!("frontmatter.slug");
    write(root, ".editor/repository.json", config);
    write(
        root,
        ".editor/ui.json",
        json!({"collections":{"notes":{"display":"slug"},"people":{"display":"name"}}}),
    );
    let mut schema: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(".editor/objects.schema.json")).unwrap())
            .unwrap();
    schema["anyOf"][1] = json!({"x-collection":"notes","type":"object","required":["slug","body"],"properties":{"slug":{"type":"string"},"body":{"type":"string"}}});
    schema["anyOf"][0]["properties"]["note"] =
        json!({"type":"string","x-relation":{"collection":"notes"}});
    write(root, ".editor/objects.schema.json", schema);
    fs::write(
        root.join("data/notes/custom-filename.md"),
        "---\nslug: old\nextra: keep\n---\n[Self](/collections/notes/records/old)\n",
    )
    .unwrap();
    let editor = Editor::open(root).unwrap();
    let note = editor.get("notes", "old").unwrap();
    editor
        .save(
            "people",
            "person",
            json!({"name":"Reader","note":"old"}),
            None,
        )
        .unwrap();
    let result = editor
        .rename_with_references("notes", "old", "new", note.value, &note.revision)
        .unwrap();
    assert_eq!(result.references_renamed, 2);
    assert_eq!(result.document.value["slug"], "new");
    assert_eq!(result.document.value["extra"], "keep");
    assert_eq!(
        result.document.value["body"],
        "[Self](/collections/notes/records/new)\n"
    );
    assert!(!root.join("data/notes/custom-filename.md").exists());
    assert!(root.join("data/notes/new.md").exists());
    assert_eq!(editor.get("people", "person").unwrap().value["note"], "new");
}
