//! Full-text markup search and separate-index lifecycle.

use objekte_core::{CacheMode, Options, Repository, TextMode, TextQuery};
use serde_json::json;
use std::{fs, path::Path};

fn write(root: &Path, relative: &str, value: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, value).unwrap();
}

fn schema(root: &Path) {
    write(
        root,
        "data/books/.editor/schema.json",
        &json!({
            "type":"object",
            "properties":{
                "md":{"type":"string","x-full-text":"markdown"},
                "html":{"type":"string","x-full-text":"html"},
                "xml":{"type":"string","x-full-text":"xml"},
                "plain":{"type":"string","x-full-text":"plain"},
                "notes":{"type":"array","items":{"type":"object","properties":{
                    "body":{"type":"string","x-full-text":"markdown"}
                }}}
            }
        })
        .to_string(),
    );
}

fn query(pattern: &str, mode: TextMode) -> TextQuery {
    TextQuery {
        pattern: pattern.into(),
        mode,
        collections: Some(vec!["books".into()]),
        limit: 100,
        count: true,
        ..Default::default()
    }
}

#[test]
fn visible_words_and_regex_remove_markup_and_normalize_whitespace() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    schema(root);
    write(
        root,
        "data/books/a.json",
        r#"{"md":"**He**lp\n  the \\*world\\*"}"#,
    );
    write(
        root,
        "data/books/b.json",
        r#"{"html":"<p>He<b>lp</b> &amp; world</p><script>secret</script>"}"#,
    );
    write(
        root,
        "data/books/c.json",
        r#"{"xml":"<root><p>He<em>lp</em></p></root>"}"#,
    );
    write(root, "data/books/d.json", r#"{"plain":"Helper\tworld"}"#);
    write(
        root,
        "data/books/e.json",
        r#"{"notes":[{"body":"**He**lp"}]}"#,
    );
    write(root, "data/books/f.json", r#"{"md":"Straße"}"#);
    write(root, "data/books/g.json", r#"{"md":"STRASSE"}"#);
    let mut repo = Repository::open(root).unwrap();

    let result = repo.search_text(&query("Help", TextMode::Word)).unwrap();
    assert_eq!(result.total, Some(4), "{:#?}", result.metrics);
    assert_eq!(
        result
            .hits
            .iter()
            .map(|hit| hit.record.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "c", "e"]
    );
    assert_eq!(result.hits[4 - 1].fields, ["notes[].body"]);
    assert_eq!(
        repo.search_text(&query("secret", TextMode::Word))
            .unwrap()
            .total,
        Some(0)
    );
    assert_eq!(
        repo.search_text(&query("Help +the +\\*world\\*", TextMode::RegexClean))
            .unwrap()
            .hits
            .iter()
            .map(|hit| hit.record.id.as_str())
            .collect::<Vec<_>>(),
        ["a"]
    );
    assert_eq!(
        repo.search_text(&query("Help & world", TextMode::RegexClean))
            .unwrap()
            .hits[0]
            .record
            .id,
        "b"
    );
    assert_eq!(
        repo.search_text(&query("strasse", TextMode::Word))
            .unwrap()
            .hits
            .iter()
            .map(|hit| hit.record.id.as_str())
            .collect::<Vec<_>>(),
        ["f", "g"]
    );
    let mut sensitive = query("strasse", TextMode::Word);
    sensitive.case_sensitive = true;
    assert_eq!(repo.search_text(&sensitive).unwrap().total, Some(0));
}

#[test]
fn raw_regex_can_match_tags_while_clean_regex_cannot() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    schema(root);
    write(
        root,
        "data/books/a.json",
        r#"{"html":"<p>He<b>lp</b></p>"}"#,
    );
    write(root, "data/books/b.json", r#"{"md":"**He**lp"}"#);
    let mut repo = Repository::open(root).unwrap();
    assert_eq!(
        repo.search_text(&query(r"<b>lp</b>", TextMode::RegexRaw))
            .unwrap()
            .hits[0]
            .record
            .id,
        "a"
    );
    assert_eq!(
        repo.search_text(&query(r"<B>LP</B>", TextMode::RegexRaw))
            .unwrap()
            .total,
        Some(1)
    );
    assert_eq!(
        repo.search_text(&query(r"<b>lp</b>", TextMode::RegexClean))
            .unwrap()
            .total,
        Some(0)
    );
    let mut markdown = query(r"\*\*He\*\*lp", TextMode::RegexRaw);
    markdown.fields = Some(vec!["md".into()]);
    assert_eq!(repo.search_text(&markdown).unwrap().hits[0].record.id, "b");
    let mut wrong_field = query("Help", TextMode::Word);
    wrong_field.fields = Some(vec!["md".into()]);
    assert_eq!(
        repo.search_text(&wrong_field)
            .unwrap()
            .hits
            .iter()
            .map(|hit| hit.record.id.as_str())
            .collect::<Vec<_>>(),
        ["b"]
    );
}

#[test]
fn pagination_and_rebuild_follow_source_changes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    schema(root);
    for id in ["a", "b", "c", "d"] {
        write(
            root,
            &format!("data/books/{id}.json"),
            r#"{"md":"**Help**"}"#,
        );
    }
    let mut repo = Repository::open(root).unwrap();
    assert!(repo.warm_text_index("books").unwrap().fts_open_nanos > 0);
    assert_eq!(repo.warm_text_index("books").unwrap().fts_open_nanos, 0);
    let mut page = query("Help", TextMode::Word);
    page.offset = 1;
    page.limit = 2;
    let result = repo.search_text(&page).unwrap();
    assert_eq!(result.total, Some(4), "{:#?}", result.metrics);
    assert_eq!(
        result
            .hits
            .iter()
            .map(|hit| hit.record.id.as_str())
            .collect::<Vec<_>>(),
        ["b", "c"]
    );

    write(root, "data/books/e.json", r#"{"md":"**He**lp"}"#);
    repo.refresh("books").unwrap();
    assert_eq!(
        repo.search_text(&query("Help", TextMode::Word))
            .unwrap()
            .total,
        Some(5)
    );
    repo.rebuild_text_index("books").unwrap();
    assert_eq!(
        repo.search_text(&query("Help", TextMode::Word))
            .unwrap()
            .total,
        Some(5)
    );
    fs::remove_file(root.join("data/books/a.json")).unwrap();
    repo.refresh("books").unwrap();
    assert_eq!(
        repo.search_text(&query("Help", TextMode::Word))
            .unwrap()
            .total,
        Some(4)
    );
    repo.rebuild_bigram_index("books").unwrap();
    assert_eq!(
        repo.search_text(&query("Help", TextMode::Word))
            .unwrap()
            .total,
        Some(4)
    );
}

#[test]
fn streamed_hits_match_collected_hits_and_late_errors_invalidate_them() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    schema(root);
    for id in 0..300 {
        write(
            root,
            &format!("data/books/a{id:03}.json"),
            r#"{"md":"**Help**"}"#,
        );
    }
    write(root, "data/books/zbad.json", r#"{"md":"**Help**"}"#);
    let mut repo = Repository::open(root).unwrap();
    let mut request = query("Help", TextMode::Word);
    request.limit = 2;
    request.offset = 1;
    let collected = repo.search_text(&request).unwrap();
    let mut streamed = Vec::new();
    let summary = repo
        .search_text_stream(&request, |ordinal, hit| {
            streamed.push((ordinal, hit.record.id.clone()));
            Ok(())
        })
        .unwrap();
    assert_eq!(summary.total, collected.total);
    assert_eq!(summary.returned, 2);
    assert_eq!(streamed, [(1, "a001".into()), (2, "a002".into())]);

    request.offset = 0;
    let mut seen = Vec::new();
    let error = repo
        .search_text_stream(&request, |ordinal, hit| {
            seen.push((ordinal, hit.record.id.clone()));
            if ordinal == 1 {
                write(root, "data/books/zbad.json", r#"{"md":"changed"}"#);
            }
            Ok(())
        })
        .unwrap_err();
    assert_eq!(seen, [(1, "a000".into()), (2, "a001".into())]);
    assert!(error.to_string().contains("source changed"), "{error}");
}

#[test]
fn schema_annotation_is_optional_and_checked() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "data/books/a.json", r#"{"md":"**Help**"}"#);
    let mut plain = Repository::open(root).unwrap();
    assert!(plain.warm_text_index("books").is_ok());
    assert_eq!(
        plain
            .search_text(&query("Help", TextMode::Word))
            .unwrap()
            .total,
        Some(1)
    );
    write(
        root,
        "data/books/.editor/schema.json",
        r#"{"type":"object","properties":{"md":{"type":"number","x-full-text":"markdown"}}}"#,
    );
    assert!(Repository::open(root).is_err());
}

#[test]
fn every_leaf_is_searchable_unless_a_schema_field_excludes_it() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "data/books/.editor/schema.json",
        &json!({"type":"object","properties":{
            "formatted":{"type":"string","x-full-text":"markdown"},
            "plain":{"type":"string"},
            "secret":{"type":"string","exclude_fts":true},
            "group":{"type":"object","exclude_fts":true,"properties":{
                "child":{"type":"string"}
            }},
            "notes":{"type":"array","items":{"type":"object","properties":{
                "body":{"type":"string"},
                "hidden":{"type":"string","exclude_fts":true}
            }}}
        }})
        .to_string(),
    );
    write(
        root,
        "data/books/a.json",
        r#"{"formatted":"**He**lp","plain":"plain token","secret":"secret token","group":{"child":"group token"},"unknown":"unknown token","nested":{"body":"nested token"},"notes":[{"body":"note token","hidden":"hidden token"}],"year":1822,"active":true,"nil":null}"#,
    );
    let mut repo = Repository::open(root).unwrap();
    repo.warm_text_index("books").unwrap();
    let cases = [
        ("Help", "formatted"),
        ("plain", "plain"),
        ("unknown", "unknown"),
        ("nested", "nested.body"),
        ("note", "notes[].body"),
        ("1822", "year"),
        ("true", "active"),
        ("null", "nil"),
    ];
    for (word, field) in cases {
        let result = repo.search_text(&query(word, TextMode::Word)).unwrap();
        assert_eq!(result.total, Some(1), "{word}");
        assert_eq!(result.hits[0].fields, [field], "{word}");
    }
    for word in ["secret", "group", "hidden"] {
        assert_eq!(
            repo.search_text(&query(word, TextMode::Word))
                .unwrap()
                .total,
            Some(0)
        );
        assert_eq!(
            repo.search_text(&query(word, TextMode::RegexRaw))
                .unwrap()
                .total,
            Some(0)
        );
    }
    let mut selected = query("unknown", TextMode::Word);
    selected.fields = Some(vec!["unknown".into()]);
    assert_eq!(repo.search_text(&selected).unwrap().total, Some(1));
    selected.fields = Some(vec!["plain".into()]);
    assert_eq!(repo.search_text(&selected).unwrap().total, Some(0));
}

#[test]
fn schema_free_collections_can_warm_and_search_every_field() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "data/books/a.json",
        r#"{"title":"Moon","extra":{"note":"Schiller"}}"#,
    );
    write(root, "data/people/b.json", r#"{"name":"Goethe"}"#);
    let mut repo = Repository::open(root).unwrap();
    let warmed = repo.warm_all_text_indexes().unwrap();
    assert_eq!(warmed.len(), 2);
    assert_eq!(
        repo.search_text(&query("Schiller", TextMode::Word))
            .unwrap()
            .hits[0]
            .fields,
        ["extra.note"]
    );
    let all = TextQuery {
        pattern: "Goethe".into(),
        count: true,
        ..Default::default()
    };
    assert_eq!(
        repo.search_text(&all).unwrap().hits[0].record.collection,
        "people"
    );
    assert_eq!(repo.rebuild_all_text_indexes().unwrap().len(), 2);
    let mut options = Options::new();
    options.cache = CacheMode::Disabled;
    let mut direct = Repository::builder(root).options(options).open().unwrap();
    for pattern in ["Moon", "Schiller", "Goethe"] {
        let mut request = query(pattern, TextMode::Word);
        request.collections = None;
        assert_eq!(
            repo.search_text(&request).unwrap().total,
            direct.search_text(&request).unwrap().total,
            "{pattern}"
        );
    }
}

#[test]
fn duplicate_keys_use_the_last_value_in_index_and_verifier() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "data/books/a.json",
        r#"{"title":"discarded","title":"retained"}"#,
    );
    let mut repo = Repository::open(root).unwrap();
    assert_eq!(
        repo.search_text(&query("retained", TextMode::Word))
            .unwrap()
            .total,
        Some(1)
    );
    assert_eq!(
        repo.search_text(&query("discarded", TextMode::Word))
            .unwrap()
            .total,
        Some(0)
    );
}

#[test]
fn exclude_fts_requires_a_boolean() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "data/books/a.json", r#"{"title":"Moon"}"#);
    write(
        root,
        "data/books/.editor/schema.json",
        r#"{"type":"object","properties":{"title":{"type":"string","exclude_fts":"yes"}}}"#,
    );
    assert!(Repository::open(root).is_err());
}

#[test]
fn numeric_index_text_matches_verified_number_spelling() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "data/books/a.json", r#"{"number":1e2}"#);
    let number: serde_json::Value = serde_json::from_str("1e2").unwrap();
    let pattern = regex::escape(&number.as_number().unwrap().to_string());
    let mut repo = Repository::open(root).unwrap();
    assert_eq!(
        repo.search_text(&query(&pattern, TextMode::RegexClean))
            .unwrap()
            .total,
        Some(1)
    );
}

#[test]
fn case_insensitive_regex_keeps_unicode_folding_candidates() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    schema(root);
    for (id, value) in [("a", "ıx"), ("b", "ſx"), ("c", "Kx"), ("d", "ix")] {
        write(
            root,
            &format!("data/books/{id}.json"),
            &json!({"plain":value}).to_string(),
        );
    }
    let mut repo = Repository::open(root).unwrap();
    for pattern in ["ix", "sx", "kx"] {
        let regex = regex::RegexBuilder::new(pattern)
            .case_insensitive(true)
            .build()
            .unwrap();
        let expected = [("a", "ıx"), ("b", "ſx"), ("c", "Kx"), ("d", "ix")]
            .into_iter()
            .filter(|(_, value)| regex.is_match(value))
            .map(|(id, _)| id)
            .collect::<Vec<_>>();
        let found = repo
            .search_text(&query(pattern, TextMode::RegexClean))
            .unwrap()
            .hits
            .iter()
            .map(|hit| hit.record.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(found, expected, "{pattern}");
    }
}

#[test]
fn disabled_cache_scans_marked_source_fields_directly() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    schema(root);
    write(root, "data/books/a.json", r#"{"md":"**He**lp"}"#);
    write(root, "data/books/b.json", r#"{"md":"Other"}"#);
    let mut options = Options::new();
    options.cache = CacheMode::Disabled;
    let mut repo = Repository::builder(root).options(options).open().unwrap();
    let result = repo.search_text(&query("Help", TextMode::Word)).unwrap();
    assert_eq!(result.total, Some(1));
    assert_eq!(result.hits[0].record.id, "a");
    assert!(result.query_plan[0].contains("direct"));
}
