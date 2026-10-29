use super::*;

fn repository(root: &Path, format: &str, id: &str) {
    fs::create_dir_all(root.join(".editor/collections/items")).unwrap();
    fs::create_dir_all(root.join("data/items")).unwrap();
    fs::write(
        root.join(".editor/repository.json"),
        r#"{"collections":{"items":".editor/collections/items/settings.json"}}"#,
    )
    .unwrap();
    fs::write(root.join(".editor/collections/items/settings.json"), format!(r#"{{"type":"data","path":"data/items","format":"{format}","id":"{id}","content":{{"body":"body"}}}}"#)).unwrap();
}

#[test]
fn build_index_refresh_one_shard_and_detect_external_changes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repository");
    let cache_root = temp.path().join("cache");
    repository(&root, "json", "filename");
    let a = root.join("data/items/a.json");
    let b = root.join("data/items/b.json");
    fs::write(&a, r#"{"name":"A"}"#).unwrap();
    fs::write(&b, r#"{"name":"B"}"#).unwrap();
    let cache = build_collection(&root, "items", &cache_root).unwrap();
    assert_eq!(cache.index.records.len(), 2);
    let a_location = &cache.index.records["a"];
    let a_shard = fs::read_to_string(cache.shard_path(a_location.shard as usize)).unwrap();
    assert!(a_shard[a_location.offset as usize..].starts_with("{\"id\":\"a\","));
    assert_eq!(
        open_generation(&root, "items", &cache_root)
            .unwrap()
            .generation,
        cache.generation
    );
    assert_eq!(cache.read_record("a").unwrap().unwrap().data["name"], "A");
    assert!(
        detect_changes(&root, "items", &cache_root)
            .unwrap()
            .is_empty()
    );
    fs::write(&a, r#"{"name":"Changed A"}"#).unwrap();
    assert_eq!(
        detect_changes(&root, "items", &cache_root).unwrap(),
        vec!["data/items/a.json"]
    );
    refresh_record(&root, "items", &cache_root, "a").unwrap();
    let updated = open_collection(&root, "items", &cache_root).unwrap();
    assert_eq!(
        updated.read_record("a").unwrap().unwrap().data["name"],
        "Changed A"
    );
    assert_eq!(cache.read_record("a").unwrap().unwrap().data["name"], "A");
    assert!(
        detect_changes(&root, "items", &cache_root)
            .unwrap()
            .is_empty()
    );
    fs::remove_file(&b).unwrap();
    refresh_record(&root, "items", &cache_root, "b").unwrap();
    let updated = open_collection(&root, "items", &cache_root).unwrap();
    assert!(updated.read_record("b").unwrap().is_none());
}

#[test]
fn markdown_frontmatter_is_queryable_json() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repository");
    repository(&root, "markdown", "frontmatter.id");
    fs::write(
        root.join("data/items/article.md"),
        "---\nid: welcome\ntitle: Hello\n---\n# Body\n",
    )
    .unwrap();
    let cache = build_collection(&root, "items", &temp.path().join("cache")).unwrap();
    let row = cache.read_record("welcome").unwrap().unwrap();
    assert_eq!(row.data["title"], "Hello");
    assert_eq!(row.data["body"], "# Body\n");
}

#[test]
fn changed_row_updates_offsets_of_later_rows_in_same_shard() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repository");
    repository(&root, "json", "filename");
    let first = "a";
    let second = (0..1000)
        .map(|n| format!("item{n}"))
        .find(|id| shard(id) == shard(first))
        .unwrap();
    fs::write(root.join("data/items/a.json"), r#"{"value":"short"}"#).unwrap();
    fs::write(
        root.join(format!("data/items/{second}.json")),
        r#"{"value":"second"}"#,
    )
    .unwrap();
    let cache_root = temp.path().join("cache");
    let original = build_collection(&root, "items", &cache_root).unwrap();
    let original_second_offset = original.index.records[&second].offset;
    fs::write(
        root.join("data/items/a.json"),
        format!(r#"{{"value":"{}"}}"#, "long".repeat(100)),
    )
    .unwrap();
    refresh_record(&root, "items", &cache_root, first).unwrap();
    let updated = open_collection(&root, "items", &cache_root).unwrap();
    assert!(updated.index.records[&second].offset > original_second_offset);
    assert_eq!(
        updated.read_record(&second).unwrap().unwrap().data["value"],
        "second"
    );
}

#[test]
fn failed_rebuild_keeps_last_published_generation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repository");
    repository(&root, "json", "filename");
    let path = root.join("data/items/a.json");
    fs::write(&path, r#"{"value":"valid"}"#).unwrap();
    let cache_root = temp.path().join("cache");
    let first = build_collection(&root, "items", &cache_root).unwrap();
    fs::write(&path, "{invalid").unwrap();
    assert!(build_collection(&root, "items", &cache_root).is_err());
    let published = open_collection(&root, "items", &cache_root).unwrap();
    assert_eq!(published.generation, first.generation);
    assert_eq!(
        published.read_record("a").unwrap().unwrap().data["value"],
        "valid"
    );
}
