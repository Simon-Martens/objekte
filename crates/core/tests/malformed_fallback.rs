//! Malformed source files remain text-searchable without claiming JSON fields.

use objekte_core::{
    CacheMode, JsonQuery, JsonQueryBackend, Options, RecordStatus, Repository, TextMode, TextQuery,
};
use std::{fs, path::Path, time::UNIX_EPOCH};

fn write(root: &Path, id: &str, bytes: &[u8]) {
    let directory = root.join("data/docs");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join(format!("{id}.json")), bytes).unwrap();
}

fn query(repo: &mut Repository, backend: JsonQueryBackend, filter: &str) -> Vec<String> {
    repo.query_json(&JsonQuery {
        filter: filter.into(),
        collections: Some(vec!["docs".into()]),
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

fn text_query(pattern: &str, mode: TextMode) -> TextQuery {
    TextQuery {
        pattern: pattern.into(),
        mode,
        collections: Some(vec!["docs".into()]),
        count: true,
        limit: 100,
        ..Default::default()
    }
}

#[test]
fn malformed_text_never_matches_filter_queries_and_has_source_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "a_valid", br#"{"note":"Mond","year":1822}"#);
    let broken = br#"{"note":"M\u006fnd", "year":1822, broken"#;
    write(root, "b_broken", broken);

    for backend in [
        JsonQueryBackend::Bigram,
        JsonQueryBackend::Direct,
        JsonQueryBackend::Grep,
    ] {
        let mut repo = Repository::open(root).unwrap();
        assert_eq!(query(&mut repo, backend, "note ~ 'Mond'"), ["a_valid"]);
        assert_eq!(
            query(&mut repo, backend, "note ~ :ignorecase('MOND')"),
            ["a_valid"]
        );
        assert_eq!(query(&mut repo, backend, "note ~ '%'"), ["a_valid"]);
        assert_eq!(
            query(&mut repo, backend, "year = 1822 && note ~ 'Mond'"),
            ["a_valid"]
        );
        assert_eq!(query(&mut repo, backend, "note !~ 'Mond'"), [] as [&str; 0]);
        assert_eq!(
            query(&mut repo, backend, "year = 1822 || note ~ 'Mond'"),
            ["a_valid"]
        );
        assert_eq!(query(&mut repo, backend, "note = 'Mond'"), ["a_valid"]);

        let valid = repo
            .query_json(&JsonQuery {
                filter: "note = 'Mond'".into(),
                collections: Some(vec!["docs".into()]),
                backend,
                ..Default::default()
            })
            .unwrap()
            .records
            .remove(0);
        assert_eq!(
            valid.size_bytes,
            br#"{"note":"Mond","year":1822}"#.len() as u64
        );
        let valid_modified = fs::metadata(root.join(&valid.path))
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            .to_string();
        assert_eq!(valid.modified_unix_nanos, valid_modified);

        let result = repo
            .search_text(&text_query("Mond", TextMode::Word))
            .unwrap();
        assert_eq!(result.total, Some(2));
        let record = &result.hits[1].record;
        assert_eq!(record.id, "b_broken");
        assert_eq!(record.path, "data/docs/b_broken.json");
        assert_eq!(record.size_bytes, broken.len() as u64);
        let modified = fs::metadata(root.join(&record.path))
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            .to_string();
        assert_eq!(record.modified_unix_nanos, modified);
        assert_eq!(record.state.status, RecordStatus::IncorrectJson);
        assert_eq!(record.state.issues[0].code, "json_syntax");
        assert_eq!(record.raw.as_deref(), Some(broken.as_slice()));
    }
}

#[test]
fn eager_full_text_postings_and_streams_include_malformed_files_in_id_order() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let schema = root.join("data/docs/.editor/schema.json");
    fs::create_dir_all(schema.parent().unwrap()).unwrap();
    fs::write(
        schema,
        r#"{"type":"object","properties":{"note":{"type":"string","x-full-text":"markdown"}}}"#,
    )
    .unwrap();
    for number in 0..130 {
        let bytes: &[u8] = if number == 1 {
            br#"{"note":"Mond"}"#
        } else if number == 64 {
            br#"{"note":"M\u006fnd", broken"#
        } else {
            br#"{"note":"Other"}"#
        };
        write(root, &format!("r{number:03}"), bytes);
    }
    let mut repo = Repository::open(root).unwrap();
    repo.warm_bigram_index("docs").unwrap();
    assert!(repo.text_index_bytes("docs").unwrap().unwrap() > 0);
    assert_eq!(
        query(&mut repo, JsonQueryBackend::Bigram, "note:fts ~ 'Mond'"),
        ["r001"]
    );

    let mut records = Vec::new();
    let summary = repo
        .query_json_stream(
            &JsonQuery {
                filter: "note:fts ~ 'Mond'".into(),
                collections: Some(vec!["docs".into()]),
                count: true,
                limit: 100,
                ..Default::default()
            },
            |ordinal, record| {
                records.push((ordinal, record.id.clone(), record.state.status));
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(summary.total, Some(1));
    assert_eq!(records[0], (1, "r001".into(), RecordStatus::Valid));
    assert_eq!(records.len(), 1);

    let mut hits = Vec::new();
    let summary = repo
        .search_text_stream(&text_query("Mond", TextMode::Word), |ordinal, hit| {
            hits.push((ordinal, hit.record.id.clone()));
            Ok(())
        })
        .unwrap();
    assert_eq!(summary.total, Some(2));
    assert_eq!(hits, [(1, "r001".into()), (2, "r064".into())]);

    let raw = repo
        .search_text(&text_query(r"M\\u006fnd", TextMode::RegexRaw))
        .unwrap();
    assert_eq!(raw.total, Some(1));
    assert_eq!(raw.hits[0].record.id, "r064");
    assert!(raw.query_plan[0].contains("full-text bigram"));
}

#[test]
fn full_text_fallback_matches_escaped_text_and_respects_field_scope() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "a_valid", br#"{"note":"Mond"}"#);
    write(root, "b_broken", br#"{"note":"M\u006fnd", broken"#);
    let mut repo = Repository::open(root).unwrap();

    let result = repo
        .search_text(&text_query("Mond", TextMode::Word))
        .unwrap();
    assert_eq!(result.total, Some(2));
    assert_eq!(
        result.hits[1].record.state.status,
        RecordStatus::IncorrectJson
    );
    assert_eq!(result.hits[1].fields, ["$raw"]);
    assert_eq!(result.hits[1].matched_paths, ["$raw"]);
    assert!(result.hits[1].record.raw.is_some());

    let raw = repo
        .search_text(&text_query(r"M\\u006fnd", TextMode::RegexRaw))
        .unwrap();
    assert_eq!(raw.total, Some(1));
    assert_eq!(raw.hits[0].record.id, "b_broken");
    assert!(raw.query_plan[0].contains("normal bigram"));

    let rendered_regex = repo
        .search_text(&text_query("Mond", TextMode::RegexClean))
        .unwrap();
    assert_eq!(rendered_regex.total, Some(2));
    assert_eq!(repo.text_index_bytes("docs").unwrap(), None);
    assert_eq!(repo.warm_text_index("docs").unwrap().fts_open_nanos, 0);
    assert_eq!(repo.text_index_bytes("docs").unwrap(), None);
    assert_eq!(
        query(&mut repo, JsonQueryBackend::Bigram, "note:fts ~ 'Mond'"),
        ["a_valid"]
    );

    let mut scoped = text_query("Mond", TextMode::Word);
    scoped.fields = Some(vec!["note".into()]);
    let scoped = repo.search_text(&scoped).unwrap();
    assert_eq!(scoped.total, Some(1));
    assert_eq!(scoped.hits[0].record.id, "a_valid");

    let mut options = Options::new();
    options.cache = CacheMode::Disabled;
    let mut direct = Repository::builder(root).options(options).open().unwrap();
    for request in [
        text_query("Mond", TextMode::Word),
        text_query("Mond", TextMode::RegexClean),
        text_query(r"M\\u006fnd", TextMode::RegexRaw),
    ] {
        let indexed = repo.search_text(&request).unwrap();
        let scanned = direct.search_text(&request).unwrap();
        assert_eq!(indexed.total, scanned.total);
        assert_eq!(
            indexed
                .hits
                .iter()
                .map(|hit| &hit.record.id)
                .collect::<Vec<_>>(),
            scanned
                .hits
                .iter()
                .map(|hit| &hit.record.id)
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn normal_index_does_not_drop_whitespace_sensitive_text_matches() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "valid", br#"{"note":"Moon\nlight"}"#);
    write(root, "broken", br#"{"note":"Moon\nlight", broken"#);
    let mut repo = Repository::open(root).unwrap();

    for request in [
        text_query("Moon light", TextMode::Word),
        text_query(r"Moon\s+light", TextMode::RegexClean),
        text_query(r"Moon\s+light", TextMode::RegexRaw),
    ] {
        let result = repo.search_text(&request).unwrap();
        assert_eq!(result.total, Some(2), "{}", request.pattern);
        assert_eq!(result.hits[0].record.id, "broken");
        assert_eq!(result.hits[1].record.id, "valid");
    }
    assert_eq!(repo.text_index_bytes("docs").unwrap(), None);
}

#[test]
fn exclusion_without_markup_builds_separate_index_with_malformed_text() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let schema = root.join("data/docs/.editor/schema.json");
    fs::create_dir_all(schema.parent().unwrap()).unwrap();
    fs::write(
        schema,
        r#"{"type":"object","properties":{"hidden":{"type":"string","exclude_fts":true},"public":{"type":"string"}}}"#,
    )
    .unwrap();
    write(root, "valid", br#"{"hidden":"Moon","public":"Star"}"#);
    write(root, "broken", br#"{"hidden":"Moon", broken"#);
    let mut repo = Repository::open(root).unwrap();
    repo.warm_bigram_index("docs").unwrap();
    assert!(repo.text_index_bytes("docs").unwrap().unwrap() > 0);
    let result = repo
        .search_text(&text_query("Moon", TextMode::Word))
        .unwrap();
    assert_eq!(result.total, Some(1));
    assert_eq!(result.hits[0].record.id, "broken");
    assert_eq!(result.hits[0].fields, ["$raw"]);
    assert_eq!(result.hits[0].matched_paths, ["$raw"]);
}

#[test]
fn refresh_and_rebuild_track_malformed_transitions() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "a", br#"{"note":"M\u006fnd", broken"#);
    let mut repo = Repository::open(root).unwrap();
    assert_eq!(
        repo.search_text(&text_query("Mond", TextMode::Word))
            .unwrap()
            .total,
        Some(1)
    );
    assert!(query(&mut repo, JsonQueryBackend::Bigram, "note ~ 'Mond'").is_empty());

    write(root, "a", br#"{"note":"Other"}"#);
    repo.refresh("docs").unwrap();
    assert_eq!(
        repo.search_text(&text_query("Mond", TextMode::Word))
            .unwrap()
            .total,
        Some(0)
    );
    repo.rebuild_bigram_index("docs").unwrap();
    assert!(query(&mut repo, JsonQueryBackend::Bigram, "note ~ 'Mond'").is_empty());

    write(root, "a", br#"{"note":"M\u006fnd", broken again"#);
    repo.refresh("docs").unwrap();
    assert_eq!(
        repo.search_text(&text_query("Mond", TextMode::Word))
            .unwrap()
            .total,
        Some(1)
    );
    repo.rebuild_bigram_index("docs").unwrap();
    assert_eq!(
        repo.search_text(&text_query("Mond", TextMode::Word))
            .unwrap()
            .total,
        Some(1)
    );
}

#[test]
fn exclusion_builds_text_index_and_malformed_raw_text_enters_both_indexes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let schema = root.join("data/docs/.editor/schema.json");
    fs::create_dir_all(schema.parent().unwrap()).unwrap();
    fs::write(
        schema,
        r#"{"type":"object","properties":{"public":{"type":"string","x-full-text":"markdown"},"secret":{"type":"string","exclude_fts":true}}}"#,
    )
    .unwrap();
    write(
        root,
        "bad",
        br#"{"secret":"Forbidden","public":"Visible", broken"#,
    );
    let mut repo = Repository::open(root).unwrap();
    repo.warm_bigram_index("docs").unwrap();
    assert!(repo.text_index_bytes("docs").unwrap().unwrap() > 0);
    assert!(query(&mut repo, JsonQueryBackend::Bigram, "secret ~ 'Forbidden'").is_empty());
    assert_eq!(
        repo.search_text(&text_query("Forbidden", TextMode::Word))
            .unwrap()
            .total,
        Some(1)
    );
    assert_eq!(
        repo.search_text(&text_query("Visible", TextMode::Word))
            .unwrap()
            .total,
        Some(1)
    );
}

#[test]
fn non_utf8_source_keeps_ascii_text_and_exact_raw_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let bytes = b"\xff note Mond";
    write(root, "bad", bytes);
    let mut repo = Repository::open(root).unwrap();
    let result = repo
        .search_text(&text_query("Mond", TextMode::Word))
        .unwrap();
    assert_eq!(result.total, Some(1));
    assert_eq!(
        result.hits[0].record.state.status,
        RecordStatus::IncorrectJson
    );
    assert_eq!(result.hits[0].record.raw.as_deref(), Some(bytes.as_slice()));
}
