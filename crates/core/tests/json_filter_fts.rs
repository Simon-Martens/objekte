//! Schema-aware `:fts` modifiers in structural JSON filters.

use objekte_core::{JsonQuery, JsonQueryBackend, Repository, TextMode, TextQuery};
use serde_json::json;
use std::{
    fs::{self, File, FileTimes},
    path::Path,
    time::{Duration, UNIX_EPOCH},
};

fn write(root: &Path, path: &str, bytes: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn order_by_id(root: &Path, ids: &[&str]) {
    for (rank, id) in ids.iter().enumerate() {
        let path = root.join(format!("data/books/{id}.json"));
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(
                FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(1000 - rank as u64)),
            )
            .unwrap();
    }
}

fn ids(repo: &mut Repository, filter: &str, backend: JsonQueryBackend) -> Vec<String> {
    repo.query_json(&JsonQuery {
        filter: filter.into(),
        collections: Some(vec!["books".into()]),
        backend,
        count: true,
        limit: 100,
        ..Default::default()
    })
    .unwrap()
    .records
    .into_iter()
    .map(|record| record.id)
    .collect()
}

#[test]
fn fts_composes_with_filters_and_keeps_backend_parity() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "data/books/.editor/schema.json",
        &json!({"type":"object","properties":{
            "year":{"type":"number","index":true},
            "annotation":{"type":"string","x-full-text":"markdown"},
            "plain":{"type":"string"},
            "excluded":{"type":"string","x-full-text":"markdown","exclude_fts":true},
            "notes":{"type":"array","items":{"type":"object","properties":{
                "body":{"type":"string","x-full-text":"html"}
            }}}
        }})
        .to_string(),
    );
    write(
        root,
        "data/books/a.json",
        r#"{"year":1822,"annotation":"**Goet**he","plain":"**Goet**he","excluded":"**Goet**he"}"#,
    );
    write(
        root,
        "data/books/b.json",
        r#"{"year":1800,"annotation":"Goethe"}"#,
    );
    write(
        root,
        "data/books/c.json",
        r#"{"year":1822,"annotation":"Goet<b>he</b>"}"#,
    );
    write(
        root,
        "data/books/d.json",
        r#"{"year":1900,"annotation":"Other","notes":[{"body":"<p>Goet<b>he</b></p>"}]}"#,
    );
    order_by_id(root, &["a", "b", "c", "d"]);
    let cases: &[(&str, &[&str])] = &[
        ("year = 1822 && annotation:fts ~ 'Goet'", &["a", "c"]),
        ("annotation:fts ~ 'Goethe'", &["a", "b", "c"]),
        ("annotation ~ 'Goethe'", &["b"]),
        ("annotation:fts = 'Goethe'", &["a", "b", "c"]),
        ("annotation:fts ~ :ignorecase('GOETHE')", &["a", "b", "c"]),
        ("annotation:fts !~ 'Goet'", &["d"]),
        (
            "annotation:fts ~ 'Goethe' || year = 1900",
            &["a", "b", "c", "d"],
        ),
        (
            "(annotation:fts ~ 'Goethe' && year = 1822) || (notes.body:fts ?~ 'Goethe' && year = 1900)",
            &["a", "c", "d"],
        ),
        ("plain:fts ~ 'Goethe'", &[]),
        ("plain:fts ~ 'Goet'", &["a"]),
        ("excluded:fts ~ 'Goethe'", &[]),
        ("excluded:fts ~ 'Goet'", &["a"]),
        ("notes.body:fts ?~ 'Goethe'", &["d"]),
    ];
    for backend in [
        JsonQueryBackend::Bigram,
        JsonQueryBackend::Direct,
        JsonQueryBackend::Grep,
    ] {
        let mut repo = Repository::open(root).unwrap();
        for (filter, expected) in cases {
            assert_eq!(
                ids(&mut repo, filter, backend),
                *expected,
                "{backend:?}: {filter}"
            );
        }
    }
}

#[test]
fn fts_is_an_ordinary_operand_without_schema() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "data/books/a.json",
        r#"{"annotation":"**Goet**he","year":1822}"#,
    );
    write(
        root,
        "data/books/b.json",
        r#"{"annotation":"Goethe","year":1822}"#,
    );
    order_by_id(root, &["a", "b"]);
    let mut repo = Repository::open(root).unwrap();
    for backend in [
        JsonQueryBackend::Bigram,
        JsonQueryBackend::Direct,
        JsonQueryBackend::Grep,
    ] {
        assert_eq!(
            ids(
                &mut repo,
                "year = 1822 && annotation:fts ~ 'Goethe'",
                backend
            ),
            ["b"]
        );
        assert_eq!(
            ids(&mut repo, "annotation:fts ~ 'Goet'", backend),
            ["a", "b"]
        );
    }
}

#[test]
fn declared_fields_have_independent_normal_and_rendered_postings() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        ".objekte/collections/books.schema.json",
        &json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "x-full-text": "plain"},
                "body": {"type": "string", "x-full-text": "markdown"},
                "summary": {"type": "string", "index": true, "x-full-text": "plain"},
                "tags": {"type": "array", "items": {"type": "string", "x-full-text": "plain"}},
                "notes": {"type": "array", "items": {"type": "object", "properties": {
                    "line": {"type": "string", "x-full-text": "html"}
                }}},
                "misc": {"type": "string"}
            },
            "x-objekte": {"name": "books", "path": "data/books", "type": "data", "format": "json", "id": "filename"}
        })
        .to_string(),
    );
    for number in 0..128 {
        write(
            root,
            &format!("data/books/d{number:03}.json"),
            &json!({
                "title": if number == 7 { "titleonlyneedle" } else { "ordinary" },
                "body": if number == 8 { "**renderedneedle**" } else { "bodyonlyneedle" },
                "summary": if number == 8 { "ordinary" } else { "renderedneedle" },
                "tags": if number == 11 { vec!["arrayneedle"] } else { Vec::new() },
                "notes": if number == 9 { vec![json!({"line":"<b>htmlneedle</b>"})] } else { Vec::new() },
                "misc": if number == 10 { "miscneedle" } else { "ordinary" }
            })
            .to_string(),
        );
    }
    let mut repo = Repository::open(root).unwrap();
    repo.warm_bigram_index("books").unwrap();
    assert!(repo.text_index_bytes("books").unwrap().unwrap() > 0);

    let cases = [
        ("title ~ 'titleonlyneedle'", vec!["d007".to_owned()]),
        ("title:fts ~ 'titleonlyneedle'", vec!["d007".to_owned()]),
        ("body ~ '**renderedneedle**'", vec!["d008".to_owned()]),
        ("body:fts ~ 'renderedneedle'", vec!["d008".to_owned()]),
        ("summary ~ 'ordinary'", vec!["d008".to_owned()]),
        ("summary:fts ~ 'ordinary'", vec!["d008".to_owned()]),
        ("notes.line ?~ 'htmlneedle'", vec!["d009".to_owned()]),
        ("notes.line:fts ~ 'htmlneedle'", vec!["d009".to_owned()]),
        ("tags ?~ 'arrayneedle'", vec!["d011".to_owned()]),
        ("tags:fts ?~ 'arrayneedle'", vec!["d011".to_owned()]),
        ("misc ~ 'miscneedle'", vec!["d010".to_owned()]),
    ];
    for (filter, expected) in cases {
        let indexed = ids(&mut repo, filter, JsonQueryBackend::Bigram);
        let direct = ids(&mut repo, filter, JsonQueryBackend::Direct);
        assert_eq!(indexed, direct, "{filter}");
        assert_eq!(indexed, expected, "{filter}");
    }
    let title = repo
        .query_json(&JsonQuery {
            filter: "title ~ 'titleonlyneedle'".into(),
            collections: Some(vec!["books".into()]),
            count: true,
            ..Default::default()
        })
        .unwrap();
    assert!(title.metrics.source_reads < 20, "{:?}", title.metrics);
    let rendered = repo
        .query_json(&JsonQuery {
            filter: "body:fts ~ 'renderedneedle'".into(),
            collections: Some(vec!["books".into()]),
            count: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(rendered.records.len(), 1);
    assert!(rendered.metrics.source_reads < 20, "{:?}", rendered.metrics);
    let array = repo
        .query_json(&JsonQuery {
            filter: "tags:fts ?~ 'arrayneedle'".into(),
            collections: Some(vec!["books".into()]),
            count: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(array.records.len(), 1);
    assert!(array.metrics.source_reads < 20, "{:?}", array.metrics);

    let absent_in_field = repo
        .query_json(&JsonQuery {
            filter: "notes.line:fts ?~ 'titleonlyneedle'".into(),
            collections: Some(vec!["books".into()]),
            count: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(absent_in_field.total, Some(0));
    assert!(
        absent_in_field.metrics.source_reads < 20,
        "{:?}",
        absent_in_field.metrics
    );

    let full_text = repo
        .search_text(&TextQuery {
            pattern: "htmlneedle".into(),
            mode: TextMode::Word,
            collections: Some(vec!["books".into()]),
            count: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        full_text
            .hits
            .iter()
            .map(|hit| hit.record.id.as_str())
            .collect::<Vec<_>>(),
        ["d009"]
    );
    assert!(
        full_text.metrics.source_reads < 20,
        "{:?}",
        full_text.metrics
    );
    for (pattern, mode) in [
        ("renderedneedle", TextMode::Word),
        (r"\*\*renderedneedle\*\*", TextMode::RegexRaw),
    ] {
        let selected = repo
            .search_text(&TextQuery {
                pattern: pattern.into(),
                mode,
                fields: Some(vec!["body".into()]),
                collections: Some(vec!["books".into()]),
                count: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            selected
                .hits
                .iter()
                .map(|hit| hit.record.id.as_str())
                .collect::<Vec<_>>(),
            ["d008"]
        );
        assert!(selected.metrics.source_reads < 20, "{:?}", selected.metrics);
    }

    repo.save_bigram_cache("books").unwrap();
    let mut restored = Repository::open(root).unwrap();
    assert_eq!(
        ids(
            &mut restored,
            "title ~ 'titleonlyneedle'",
            JsonQueryBackend::Bigram
        ),
        ["d007"]
    );
    assert_eq!(restored.metrics().bigram_snapshot_loads, 1);
    assert_eq!(restored.metrics().secondary_snapshot_loads, 1);
    assert_eq!(
        ids(
            &mut restored,
            "body:fts ~ 'renderedneedle'",
            JsonQueryBackend::Bigram
        ),
        ["d008"]
    );
    write(
        root,
        "data/books/d007.json",
        r#"{"title":"changedneedle","body":"other"}"#,
    );
    restored.refresh("books").unwrap();
    assert!(
        ids(
            &mut restored,
            "title ~ 'titleonlyneedle'",
            JsonQueryBackend::Bigram
        )
        .is_empty()
    );
    assert_eq!(
        ids(
            &mut restored,
            "title ~ 'changedneedle'",
            JsonQueryBackend::Bigram
        ),
        ["d007"]
    );
    assert_eq!(
        ids(
            &mut restored,
            "title:fts ~ 'changedneedle'",
            JsonQueryBackend::Bigram
        ),
        ["d007"]
    );
    write(
        root,
        "data/books/d128.json",
        r#"{"title":"newfieldneedle","body":"**newrenderneedle**"}"#,
    );
    restored.refresh("books").unwrap();
    assert_eq!(
        ids(
            &mut restored,
            "body:fts ~ 'newrenderneedle'",
            JsonQueryBackend::Bigram
        ),
        ["d128"]
    );
    fs::remove_file(root.join("data/books/d008.json")).unwrap();
    restored.refresh("books").unwrap();
    assert!(
        ids(
            &mut restored,
            "body:fts ~ 'renderedneedle'",
            JsonQueryBackend::Bigram
        )
        .is_empty()
    );
}

#[test]
fn invalid_container_in_declared_string_field_keeps_existential_matches() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        ".objekte/collections/books.schema.json",
        &json!({
            "type": "object",
            "properties": {"title": {"type": "string", "x-full-text": "plain"}},
            "x-objekte": {"name": "books", "path": "data/books", "type": "data", "format": "json", "id": "filename"}
        })
        .to_string(),
    );
    write(
        root,
        "data/books/a.json",
        r#"{"title":["arrayinscalarneedle"]}"#,
    );
    write(root, "data/books/b.json", r#"{"title":"ordinary"}"#);
    let mut repo = Repository::open(root).unwrap();
    for filter in [
        "title:fts ?~ 'arrayinscalarneedle'",
        "title ?~ 'arrayinscalarneedle'",
    ] {
        assert_eq!(ids(&mut repo, filter, JsonQueryBackend::Bigram), ["a"]);
        assert_eq!(ids(&mut repo, filter, JsonQueryBackend::Direct), ["a"]);
    }
}
