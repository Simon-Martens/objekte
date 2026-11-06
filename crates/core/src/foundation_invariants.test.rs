//! Small independent-answer fixtures for mixed logical and unavailable records.

use std::collections::HashSet;

use crate::inventory::{FileStatus, RecordClass};
use crate::model::{FieldPath, LogicalType};
use crate::query::{Query, QueryOptions, parse_filter};
use crate::reference::{FieldDefinition, Schema};
use crate::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
};
use crate::source::DiagnosticCategory;
use crate::source::SourceFormat;
use crate::workload::{BigramAlphabet, ReturnMode, SortField};

fn definition() -> CollectionDefinition {
    let mut schema = Schema::default();
    for (name, kind, multi) in [
        ("title", LogicalType::String, false),
        (
            "tags",
            LogicalType::Array(Box::new(LogicalType::String)),
            true,
        ),
    ] {
        schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required: false,
                nullable: true,
                indexed: true,
                multi,
                element_nullable: vec![],
            })
            .unwrap();
    }
    CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::Json,
        identity: IdentityPolicy::Filename,
        discovery: Default::default(),
        schema,
        sort_fields: vec![FieldPath::parse("title").unwrap()],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    }
}

fn query(filter: &str) -> Query {
    Query {
        collections: vec!["data".into()],
        filter: Some(parse_filter(filter).unwrap()),
        fts: None,
        options: QueryOptions {
            sort: vec![],
            limit: 20,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    }
}

fn assert_ids(repo: &Repository, filter: &str, expected: &[&str]) {
    let request = query(filter);
    let optimized = repo.search(&request).unwrap();
    let reference = repo.search_reference(&request).unwrap();
    let actual = optimized
        .hits
        .iter()
        .map(|hit| hit.file_path.file_stem().unwrap().to_str().unwrap())
        .collect::<HashSet<_>>();
    assert_eq!(
        actual,
        expected.iter().copied().collect(),
        "filter={filter}"
    );
    assert_eq!(optimized.count, Some(expected.len()), "filter={filter}");
    assert_eq!(optimized.hits, reference.hits, "filter={filter}");
    assert_eq!(optimized.counters.files_opened, 0, "filter={filter}");
    assert_eq!(optimized.counters.record_parses, 0, "filter={filter}");
}

#[test]
fn boolean_truth_evidence_sort_and_restore_use_independent_answers() {
    let root =
        std::env::temp_dir().join(format!("objekte-foundation-truth-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, contents) in [
        ("x", r#"{"title":"x","tags":["a","b"]}"#),
        ("y", r#"{"title":"y","tags":["b"]}"#),
        ("missing", r#"{}"#),
        ("null", r#"{"title":null,"tags":null}"#),
        ("broken", r#"{"title":"needle""#),
    ] {
        std::fs::write(root.join(format!("data/{name}.json")), contents).unwrap();
    }
    let answers: &[(&str, &[&str])] = &[
        ("title = 'x'", &["x"]),
        ("!(title = 'x')", &["y", "missing", "null"]),
        ("title:exists = false", &["missing"]),
        ("title:null = true", &["null"]),
        ("@record.class = 1", &["broken"]),
        ("@record.class = 1 || title = 'x'", &["broken", "x"]),
        ("@record.class = 1 && !(title = 'x')", &[]),
        ("@file.status = 1 && title = 'x'", &[]),
        ("@file.status = 1 || title = 'x'", &["broken", "x"]),
        ("tags ?= 'b'", &["x", "y"]),
        ("!(tags ?= 'b')", &["missing", "null"]),
        ("tags:exists = false", &["missing"]),
    ];
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let mut built_diagnostic = None;
        let options = RepositoryOptions {
            bigram_alphabet: alphabet,
            snapshot_mode: SnapshotMode::ReadWrite,
            verified_cache_bytes: 0,
            ..Default::default()
        };
        for opening in 0..2 {
            let repo = Repository::open_with_options(&root, vec![definition()], options).unwrap();
            assert_eq!(repo.startup().restored, opening == 1);
            for budget in [0, 1 << 20] {
                repo.configure_verified_cache(budget).unwrap();
                for (filter, expected) in answers {
                    assert_ids(&repo, filter, expected);
                }
                let left = repo.search(&query("title = 'x' || tags ?= 'b'")).unwrap();
                let right = repo.search(&query("tags ?= 'b' || title = 'x'")).unwrap();
                assert_eq!(left.hits, right.hits);
                assert_eq!(
                    left.hits
                        .iter()
                        .find(|hit| hit.record_id == "x")
                        .unwrap()
                        .matched_fields,
                    ["tags", "title"]
                );
                let left = repo.search(&query("title = 'x' && tags ?= 'b'")).unwrap();
                let right = repo.search(&query("tags ?= 'b' && title = 'x'")).unwrap();
                assert_eq!(left.hits, right.hits);
            }
            let mut overflow = query("title = 'x'");
            overflow.options.limit = usize::MAX;
            overflow.options.offset = 1;
            assert!(repo.search(&overflow).unwrap_err().contains("overflow"));
            let all = Query {
                filter: None,
                options: QueryOptions {
                    sort: vec![SortField {
                        field: "title".into(),
                        descending: false,
                    }],
                    count: true,
                    ..query("title = 'x'").options
                },
                ..query("title = 'x'")
            };
            let full = repo.search(&all).unwrap();
            assert_eq!(
                full.hits
                    .iter()
                    .map(|hit| hit.file_path.file_stem().unwrap().to_str().unwrap())
                    .collect::<Vec<_>>(),
                ["x", "y", "null", "missing", "broken"]
            );
            for page_size in [1, 2, 3] {
                let mut concatenated = Vec::new();
                for offset in (0..5).step_by(page_size) {
                    let mut page = all.clone();
                    page.options.offset = offset;
                    page.options.limit = page_size;
                    page.options.count = false;
                    let result = repo.search(&page).unwrap();
                    assert!(result.count.is_none());
                    concatenated.extend(result.hits);
                }
                assert_eq!(concatenated, full.hits);
            }
            assert_eq!(
                repo.files()
                    .files
                    .iter()
                    .filter(|file| file.status == FileStatus::Broken)
                    .count(),
                1
            );
            let bad = repo.collections()[0]
                .generation
                .records
                .iter()
                .find(|record| record.class == RecordClass::RawFallback)
                .unwrap();
            let issue = repo
                .files()
                .diagnostic(bad.file, bad.diagnostic_ref)
                .expect("fallback record must resolve its own diagnostic");
            assert_eq!(issue.category, DiagnosticCategory::Syntax);
            assert_eq!(issue.code, "json.parse");
            assert_eq!(issue.record, Some(bad.slot));
            assert_eq!(issue.source_id, "json");
            if let Some(expected) = &built_diagnostic {
                assert_eq!(
                    issue, expected,
                    "restore changed diagnostic identity or contents"
                );
            } else {
                built_diagnostic = Some(issue.clone());
            }
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
