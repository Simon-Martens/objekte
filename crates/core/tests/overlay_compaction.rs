use objekte_core::{CacheMode, JsonQuery, JsonQueryBackend, Options, Repository};
use serde_json::json;
use std::{fs, path::Path};


const FILES: usize = 600;


fn write_all(root: &Path) {
    let folder = root.join("data/books");
    fs::create_dir_all(&folder).unwrap();
    for index in 0..FILES {
        fs::write(
            folder.join(format!("b{index:04}.json")),
            json!({"year": index, "flag": index % 2 == 0}).to_string(),
        )
        .unwrap();
    }
}

/// 600 records with indexed `year` and `flag` fields.
fn fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join(".objekte/collections")).unwrap();
    fs::write(
        root.join(".objekte/collections/books.schema.json"),
        json!({
            "type":"object",
            "properties":{"year":{"type":"integer","index":true},"flag":{"type":"boolean","index":true}},
            "x-objekte":{"name":"books","path":"data/books","type":"data","format":"json","id":"filename"}
        })
        .to_string(),
    )
    .unwrap();
    write_all(root);
    temp
}

fn repo(root: &Path) -> Repository {
    let mut options = Options::new();
    options.cache = CacheMode::Memory;
    Repository::load_repository(root, options).unwrap()
}

/// Files read to verify one indexed match.
fn reads_for_one_match(repository: &mut Repository) -> u64 {
    let result = repository
        .query_json(&JsonQuery {
            filter: "year = 7 && flag = false".into(),
            collections: Some(vec!["books".into()]),
            backend: JsonQueryBackend::Bigram,
            count: true,
            limit: 10,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(result.total, Some(1));
    assert_eq!(result.records[0].id, "b0007");
    result.metrics.grep_scans
}


#[test]
fn touching_every_file_rebuilds_instead_of_keeping_a_full_overlay() {
    let fixture = fixture();
    let root = fixture.path();
    let mut repository = repo(root);
    assert!(reads_for_one_match(&mut repository) <= 1);
    // Same content, new stamps: like a checkout or copy of every file.
    write_all(root);
    repository.refresh("books").unwrap();
    assert!(reads_for_one_match(&mut repository) <= 1);
}

#[test]
fn a_saved_cache_with_a_full_overlay_is_rebuilt_on_load() {
    let fixture = fixture();
    let root = fixture.path();
    let cache = repo(root).save_bigram_cache("books").unwrap();
    let saved = fs::metadata(&cache).unwrap().modified().unwrap();
    write_all(root);
    let mut repository = repo(root);
    assert!(reads_for_one_match(&mut repository) <= 1);
    // The repaired index replaced the saved cache, so the next load is clean.
    assert_ne!(fs::metadata(&cache).unwrap().modified().unwrap(), saved);
    let mut reopened = repo(root);
    assert!(reads_for_one_match(&mut reopened) <= 1);
    assert_eq!(reopened.metrics().bigram_snapshot_loads, 1);
}

#[test]
fn a_few_edits_stay_in_the_overlay() {
    let fixture = fixture();
    let root = fixture.path();
    let mut repository = repo(root);
    reads_for_one_match(&mut repository);
    for index in 0..10 {
        fs::write(
            root.join(format!("data/books/b{index:04}.json")),
            json!({"year": index, "flag": index % 2 == 0, "edited": true}).to_string(),
        )
        .unwrap();
    }
    repository.refresh("books").unwrap();
    // The ten overlay slots stay conservative candidates; nothing else is read.
    let reads = reads_for_one_match(&mut repository);
    assert!((1..=11).contains(&reads), "{reads}");
}
