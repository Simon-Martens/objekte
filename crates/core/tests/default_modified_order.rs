use objekte_core::{
    CacheMode, Direction, JsonQuery, JsonQueryBackend, Options, Repository, Sort, TextQuery,
};
use serde_json::json;
use std::{
    fs::{self, File, FileTimes},
    path::Path,
    time::{Duration, UNIX_EPOCH},
};

fn schema(root: &Path, name: &str) {
    let folder = root.join(".objekte/collections");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join(format!("{name}.schema.json")), serde_json::to_vec(&json!({
        "type":"object",
        "properties":{"rank":{"type":"integer","index":true},"text":{"type":"string"}},
        "x-objekte":{"name":name,"path":format!("data/{name}"),"type":"data","format":"json","id":"filename"}
    })).unwrap()).unwrap();
}

#[test]
fn sparse_exact_matches_and_mixed_search_keep_newest_first_stream_order() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    schema(root, "books");
    for number in 0..40 {
        write(root, "books", &format!("ordinary{number:02}"), 100 + number);
    }
    for (id, time) in [("old", 200), ("new", 400), ("middle", 300)] {
        write(root, "books", id, time);
        let path = root.join(format!("data/books/{id}.json"));
        fs::write(&path, br#"{"rank":2,"text":"needle"}"#).unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(time)))
            .unwrap();
    }
    let expected = ["new", "middle", "old"];
    let mut repo = Repository::open(root).unwrap();
    let query = JsonQuery {
        filter: "rank = 2".into(),
        collections: Some(vec!["books".into()]),
        count: true,
        ..Default::default()
    };
    let result = repo.query_json(&query).unwrap();
    assert_eq!(result.metrics.grep_scans, 0);
    assert_eq!(
        result
            .records
            .iter()
            .map(|record| record.id.as_str())
            .collect::<Vec<_>>(),
        expected
    );
    let mut streamed = Vec::new();
    repo.query_json_stream(&query, |_, record| {
        streamed.push(record.id.clone());
        Ok(())
    })
    .unwrap();
    assert_eq!(streamed, expected);

    let mixed = TextQuery {
        pattern: "needle".into(),
        filter: Some("rank = 2".into()),
        collections: Some(vec!["books".into()]),
        count: true,
        ..Default::default()
    };
    let hits = repo.search_text(&mixed).unwrap();
    assert_eq!(
        hits.hits
            .iter()
            .map(|hit| hit.record.id.as_str())
            .collect::<Vec<_>>(),
        expected
    );
    let mut hits_streamed = Vec::new();
    repo.search_text_stream(&mixed, |_, hit| {
        hits_streamed.push(hit.record.id.clone());
        Ok(())
    })
    .unwrap();
    assert_eq!(hits_streamed, expected);

    let mut options = Options::new();
    options.cache = CacheMode::Disabled;
    let mut direct = Repository::load_repository(root, options).unwrap();
    let direct_hits = direct.search_text(&mixed).unwrap();
    assert_eq!(
        direct_hits
            .hits
            .iter()
            .map(|hit| hit.record.id.as_str())
            .collect::<Vec<_>>(),
        expected
    );
}

fn write(root: &Path, collection: &str, id: &str, seconds: u64) {
    let folder = root.join("data").join(collection);
    fs::create_dir_all(&folder).unwrap();
    let path = folder.join(format!("{id}.json"));
    fs::write(&path, br#"{"rank":1,"text":"needle"}"#).unwrap();
    let time = UNIX_EPOCH + Duration::from_secs(seconds);
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(time))
        .unwrap();
}

fn json_ids(
    repo: &mut Repository,
    backend: JsonQueryBackend,
    offset: usize,
    limit: usize,
) -> Vec<String> {
    repo.query_json(&JsonQuery {
        filter: "rank = 1".into(),
        backend,
        offset,
        limit,
        ..Default::default()
    })
    .unwrap()
    .records
    .into_iter()
    .map(|record| format!("{}/{}", record.collection, record.id))
    .collect()
}

fn text_ids(repo: &mut Repository, offset: usize, limit: usize) -> Vec<String> {
    repo.search_text(&TextQuery {
        pattern: "needle".into(),
        offset,
        limit,
        ..Default::default()
    })
    .unwrap()
    .hits
    .into_iter()
    .map(|hit| format!("{}/{}", hit.record.collection, hit.record.id))
    .collect()
}

#[test]
fn default_order_uses_modified_time_and_id_ties_across_backends() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    schema(root, "alpha");
    schema(root, "beta");
    for (id, time) in [("a", 100), ("b", 300), ("c", 200), ("d", 300)] {
        write(root, "alpha", id, time);
    }
    write(root, "beta", "z", 500);
    let expected = ["alpha/b", "alpha/d", "alpha/c", "alpha/a", "beta/z"];
    let mut repo = Repository::open(root).unwrap();
    for backend in [
        JsonQueryBackend::Auto,
        JsonQueryBackend::Bigram,
        JsonQueryBackend::Direct,
        JsonQueryBackend::Grep,
    ] {
        assert_eq!(json_ids(&mut repo, backend, 0, 10), expected, "{backend:?}");
        assert_eq!(
            json_ids(&mut repo, backend, 1, 2),
            expected[1..3],
            "{backend:?}"
        );
    }
    assert_eq!(text_ids(&mut repo, 0, 10), expected);
    assert_eq!(text_ids(&mut repo, 2, 2), expected[2..4]);
    let explicit = repo
        .query_json(&JsonQuery {
            filter: "rank = 1".into(),
            sort: vec![Sort {
                field: "@id".into(),
                direction: Direction::Asc,
            }],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        explicit
            .records
            .iter()
            .map(|r| format!("{}/{}", r.collection, r.id))
            .collect::<Vec<_>>(),
        ["alpha/a", "alpha/b", "alpha/c", "alpha/d", "beta/z"]
    );
}

#[test]
fn cached_order_restores_and_tracks_updates_without_moving_bigram_slots() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    schema(root, "books");
    for (id, time) in [("a", 100), ("b", 300), ("c", 200)] {
        write(root, "books", id, time);
    }
    let mut initial = Repository::open(root).unwrap();
    assert_eq!(
        json_ids(&mut initial, JsonQueryBackend::Auto, 0, 10),
        ["books/b", "books/c", "books/a"]
    );
    initial.save_bigram_cache("books").unwrap();
    drop(initial);

    let mut restored = Repository::open(root).unwrap();
    restored.warm_bigram_index("books").unwrap();
    assert_eq!(restored.metrics().secondary_snapshot_loads, 1);
    assert_eq!(
        text_ids(&mut restored, 0, 10),
        ["books/b", "books/c", "books/a"]
    );

    write(root, "books", "a", 400);
    restored.refresh("books").unwrap();
    assert_eq!(
        json_ids(&mut restored, JsonQueryBackend::Auto, 0, 10),
        ["books/a", "books/b", "books/c"]
    );
    assert_eq!(
        text_ids(&mut restored, 0, 10),
        ["books/a", "books/b", "books/c"]
    );
    drop(restored);

    let mut reloaded = Repository::open(root).unwrap();
    reloaded.warm_bigram_index("books").unwrap();
    assert_eq!(reloaded.metrics().secondary_snapshot_loads, 1);
    assert_eq!(
        json_ids(&mut reloaded, JsonQueryBackend::Auto, 0, 10),
        ["books/a", "books/b", "books/c"]
    );

    fs::remove_file(root.join("data/books/b.json")).unwrap();
    write(root, "books", "d", 500);
    reloaded.refresh("books").unwrap();
    assert_eq!(
        json_ids(&mut reloaded, JsonQueryBackend::Auto, 0, 10),
        ["books/d", "books/a", "books/c"]
    );
    assert_eq!(
        text_ids(&mut reloaded, 0, 10),
        ["books/d", "books/a", "books/c"]
    );
    drop(reloaded);

    let mut final_open = Repository::open(root).unwrap();
    final_open.warm_bigram_index("books").unwrap();
    assert_eq!(final_open.metrics().secondary_snapshot_loads, 1);
    assert_eq!(
        json_ids(&mut final_open, JsonQueryBackend::Auto, 0, 10),
        ["books/d", "books/a", "books/c"]
    );
}
