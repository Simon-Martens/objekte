//! Schema-aware `:fts` modifiers in structural JSON filters.

use objekte_core::{JsonQuery, JsonQueryBackend, Repository};
use serde_json::json;
use std::{fs, path::Path};

fn write(root: &Path, path: &str, bytes: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
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
