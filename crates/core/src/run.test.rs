use super::*;
use crate::model::LogicalType;
use crate::query::{QueryOptions, parse_filter};
use crate::reference::{FieldDefinition, Schema};
use crate::repository::{CollectionDefinition, IdentityPolicy};
use crate::source::SourceFormat;

#[test]
fn exact_posting_count_preserves_page_evidence_with_and_without_membership_cache() {
    let root = std::env::temp_dir().join(format!("objekte-proof-page-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for number in 0..100 {
        std::fs::write(
            root.join(format!("data/{number:03}.json")),
            r#"{"kind":"x"}"#,
        )
        .unwrap();
    }
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["kind".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::Json,
            identity: IdentityPolicy::Filename,
            discovery: Default::default(),
            schema,
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }],
        crate::repository::RepositoryOptions {
            snapshot_mode: crate::repository::SnapshotMode::Disabled,
            verified_cache_bytes: 0,
            ..Default::default()
        },
    )
    .unwrap();
    let query = Query {
        collections: vec!["data".into()],
        filter: Some(parse_filter("kind = 'x'").unwrap()),
        fts: None,
        options: QueryOptions {
            sort: vec![],
            limit: 5,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    };
    for budget in [0, 1 << 20] {
        repo.configure_verified_cache(budget).unwrap();
        let result = repo.search(&query).unwrap();
        assert_eq!(result.count, Some(100));
        assert_eq!(result.counters.record_parses, 0);
        assert_eq!(result.counters.distinct_files, 0);
        assert_eq!(
            result
                .hits
                .iter()
                .map(|hit| hit.record_id.as_str())
                .collect::<Vec<_>>(),
            ["000", "001", "002", "003", "004"]
        );
        assert!(result.hits.iter().all(|hit| hit.matched_fields == ["kind"]));
        assert_eq!(result.hits, repo.search_reference(&query).unwrap().hits);
        if budget != 0 {
            let cached = repo.search(&query).unwrap();
            assert!(cached.counters.verified_result_hit);
            assert_eq!(cached.counters.distinct_files, 0);
            assert_eq!(cached.hits, result.hits);
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn physical_file_accounting_deduplicates_shared_collections_and_payload_reads() {
    let root = std::env::temp_dir().join(format!("objekte-shared-files-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/one.json"),
        br#"{"body":"needle","memo":"needle"}"#,
    )
    .unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["body".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    schema
        .register_field(FieldDefinition {
            path: vec!["memo".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: false,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    let definitions = ["a", "b"]
        .into_iter()
        .map(|name| CollectionDefinition {
            name: name.into(),
            source_path: "data".into(),
            format: SourceFormat::Json,
            identity: IdentityPolicy::Filename,
            discovery: Default::default(),
            schema: schema.clone(),
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        })
        .collect();
    let repo = Repository::open(&root, definitions).unwrap();
    let mut query = Query {
        collections: vec![],
        fts: None,
        filter: Some(parse_filter("memo ~ 'needle'").unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 2,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let scanned = repo.search(&query).unwrap();
    assert_eq!(scanned.count, Some(2));
    assert_eq!(scanned.counters.files_opened, 2);
    assert_eq!(scanned.counters.distinct_files, 1);
    query.filter = Some(parse_filter("body = 'needle'").unwrap());
    let exact = repo.search(&query).unwrap();
    assert_eq!(exact.counters.files_opened, 0);
    assert_eq!(exact.counters.distinct_files, 0);
    query.options.return_mode = ReturnMode::Bytes;
    let payload = repo.search(&query).unwrap();
    assert_eq!(payload.count, Some(2));
    assert_eq!(payload.counters.files_opened, 1);
    assert_eq!(payload.counters.distinct_files, 1);
    assert_eq!(payload.hits[0].file_id, payload.hits[1].file_id);
    assert!(payload.hits.iter().all(|hit| matches!(
        &hit.payload,
        Some(Payload::Bytes(bytes)) if bytes == br#"{"body":"needle","memo":"needle"}"#
    )));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn malformed_jsonl_is_searchable_without_inventing_logical_values() {
    use crate::inventory::{FileStatus, RecordClass};
    use crate::query::FtsQuery;
    use crate::repository::{Payload, RepositoryOptions, SnapshotMode};
    let root = std::env::temp_dir().join(format!("objekte-malformed-jsonl-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/mixed.jsonl"),
        b"{\"title\":\"alpha\"}\n{\"title\":\n{\"title\":\"omega\"}\n",
    )
    .unwrap();
    std::fs::write(root.join("data/valid.jsonl"), b"{\"title\":\"beta\"}\n").unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["title".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let definition = CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::JsonLines,
        identity: IdentityPolicy::Generated,
        discovery: Default::default(),
        schema,
        sort_fields: vec![crate::model::FieldPath::parse("@file.status").unwrap()],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    };
    let query = |filter: Option<&str>, fts: Option<FtsQuery>, mode| Query {
        collections: vec!["data".into()],
        filter: filter.map(|filter| parse_filter(filter).unwrap()),
        fts,
        options: QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: mode,
            evidence: true,
        },
    };
    for alphabet in [
        crate::workload::BigramAlphabet::Ascii,
        crate::workload::BigramAlphabet::Unicode,
    ] {
        let options = RepositoryOptions {
            bigram_alphabet: alphabet,
            snapshot_mode: SnapshotMode::ReadWrite,
            verified_cache_bytes: 1024 * 1024,
            ..Default::default()
        };
        let built =
            Repository::open_with_options(&root, vec![definition.clone()], options).unwrap();
        assert_eq!(built.build_failed_parses(), 1);
        assert_eq!(built.files.files[0].status, FileStatus::Broken);
        assert_eq!(built.files.files[0].diagnostics.len(), 1);
        assert_eq!(built.files.files[0].diagnostics[0].line, Some(2));
        assert!(built.files.files[0].diagnostics[0].byte_offset.unwrap() >= 18);
        assert_eq!(built.collections[0].generation.records.len(), 4);
        assert_eq!(built.collections[0].logical_records.len(), 3);
        let all = query(None, None, ReturnMode::Logical);
        let result = built.search(&all).unwrap();
        assert_eq!(result.count, Some(4));
        let bad = result
            .hits
            .iter()
            .find(|hit| hit.record_class == RecordClass::RawFallback)
            .unwrap();
        assert_eq!(bad.file_status, FileStatus::Broken);
        assert!(matches!(
            bad.payload,
            Some(Payload::LogicalUnavailable { .. })
        ));
        assert!(
            built
                .files
                .diagnostic(crate::model::FileId(bad.file_id), bad.diagnostic_ref)
                .is_some()
        );
        assert_eq!(built.search_reference(&all).unwrap().hits, result.hits);
        let cached = built.search(&all).unwrap();
        assert!(cached.counters.verified_result_hit);
        assert_eq!(cached.hits, result.hits);
        let bytes = built
            .search(&query(Some("@record.class = 1"), None, ReturnMode::Bytes))
            .unwrap();
        assert_eq!(bytes.count, Some(1));
        assert!(
            matches!(&bytes.hits[0].payload, Some(Payload::Bytes(value)) if value == b"{\"title\":")
        );
        for (filter, expected) in [
            ("title = 'alpha'", 1),
            ("!(title = 'alpha')", 2),
            ("@file.status = 1", 3),
            ("@file.status = 1 || title = 'x'", 3),
            ("@file.status = 0 || title = 'x'", 1),
            ("@record.class = 1 && title = 'x'", 0),
            ("@record.class = 1 && !(title = 'x')", 0),
        ] {
            let q = query(Some(filter), None, ReturnMode::Locator);
            let actual = built.search(&q).unwrap();
            assert_eq!(actual.count, Some(expected), "{filter}");
            assert_eq!(
                actual.hits,
                built.search_reference(&q).unwrap().hits,
                "{filter}"
            );
        }
        let raw_fts = query(
            None,
            Some(FtsQuery {
                literal: "title".into(),
                fields: vec![],
            }),
            ReturnMode::Locator,
        );
        let raw_result = built.search(&raw_fts).unwrap();
        assert_eq!(raw_result.count, Some(1));
        assert_eq!(raw_result.counters.raw_text_verifications, 1);
        assert_eq!(raw_result.counters.record_parses, 0);
        assert_eq!(raw_result.hits[0].record_class, RecordClass::RawFallback);
        assert_eq!(raw_result.hits[0].matched_fields, ["@record.raw_text"]);
        let selected = query(
            None,
            Some(FtsQuery {
                literal: "title".into(),
                fields: vec![crate::query::FtsField {
                    path: crate::model::FieldPath::parse("title").unwrap(),
                    binding: None,
                }],
            }),
            ReturnMode::Locator,
        );
        assert_eq!(built.search(&selected).unwrap().count, Some(0));
        let mut sorted = all.clone();
        sorted.options.sort = vec![crate::workload::SortField {
            field: "@file.status".into(),
            descending: false,
        }];
        let ordered = built.search(&sorted).unwrap();
        assert_eq!(ordered.hits[0].file_status, FileStatus::Valid);
        assert!(
            ordered.hits[1..]
                .iter()
                .all(|hit| hit.file_status == FileStatus::Broken)
        );
        assert_eq!(ordered.hits, built.search_reference(&sorted).unwrap().hits);
        let restored =
            Repository::open_with_options(&root, vec![definition.clone()], options).unwrap();
        assert!(restored.startup().restored);
        assert_eq!(restored.search(&all).unwrap().hits, result.hits);
        assert_eq!(restored.search(&raw_fts).unwrap().hits, raw_result.hits);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn empty_and_invalid_utf8_json_have_exact_raw_ranges() {
    use crate::repository::{Payload, RepositoryOptions, SnapshotMode};
    let root = std::env::temp_dir().join(format!("objekte-malformed-json-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/empty.json"), b"").unwrap();
    std::fs::write(root.join("data/encoding.json"), b"{\"title\":\"\xff\"}").unwrap();
    std::fs::write(root.join("data/valid.json"), b"{\"title\":\"okay\"}").unwrap();
    let definition = CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::Json,
        identity: IdentityPolicy::Filename,
        discovery: Default::default(),
        schema: Schema::default(),
        sort_fields: vec![],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    };
    let repo = Repository::open_with_options(
        &root,
        vec![definition],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    let query = |filter: &str, mode| Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(parse_filter(filter).unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: mode,
            evidence: false,
        },
    };
    let empty = repo
        .search(&query("@record.byte_length = 0", ReturnMode::Bytes))
        .unwrap();
    assert_eq!(empty.count, Some(1));
    assert!(matches!(&empty.hits[0].payload, Some(Payload::Bytes(bytes)) if bytes.is_empty()));
    assert_eq!(
        empty.hits[0].record_class,
        crate::inventory::RecordClass::RawFallback
    );
    let encoding = repo
        .search(&query("@file.path ~ 'encoding'", ReturnMode::Bytes))
        .unwrap();
    assert_eq!(encoding.count, Some(1));
    assert!(
        matches!(&encoding.hits[0].payload, Some(Payload::Bytes(bytes)) if bytes == b"{\"title\":\"\xff\"}")
    );
    assert!(
        repo.files
            .files
            .iter()
            .find(|file| file.relative_path.ends_with("encoding.json"))
            .unwrap()
            .diagnostics
            .iter()
            .any(|issue| issue.category == crate::source::DiagnosticCategory::Encoding)
    );
    let fts = Query {
        collections: vec!["data".into()],
        filter: None,
        fts: Some(crate::query::FtsQuery {
            literal: "�".into(),
            fields: vec![],
        }),
        options: QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Logical,
            evidence: true,
        },
    };
    let found = repo.search(&fts).unwrap();
    assert_eq!(found.count, Some(1));
    assert_eq!(
        found.hits[0].record_class,
        crate::inventory::RecordClass::RawFallback
    );
    assert!(matches!(
        found.hits[0].payload,
        Some(Payload::LogicalUnavailable { .. })
    ));
    assert_eq!(found.hits, repo.search_reference(&fts).unwrap().hits);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn residual_proofs_skip_indexed_terms_and_preserve_or_evidence() {
    let root = std::env::temp_dir().join(format!("objekte-residual-proofs-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (id, kind, body, other) in [
        ("a", "yes", "needle", "ok"),
        ("b", "yes", "ordinary", "ok"),
        ("c", "no", "needle", "ok"),
        ("d", "no", "ordinary", "ok"),
        ("e", "no", "needle", "blocked"),
    ] {
        std::fs::write(
            root.join(format!("data/{id}.json")),
            format!(r#"{{"kind":"{kind}","body":"{body}","other":"{other}"}}"#),
        )
        .unwrap();
    }
    for n in 0..80 {
        std::fs::write(
            root.join(format!("data/z{n:03}.json")),
            r#"{"kind":"no","body":"ordinary","other":"ok"}"#,
        )
        .unwrap();
    }
    let mut schema = Schema::default();
    for (path, indexed) in [("kind", true), ("body", false), ("other", false)] {
        schema
            .register_field(FieldDefinition {
                path: vec![path.into()],
                kind: LogicalType::String,
                required: true,
                nullable: false,
                indexed,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::Json,
            identity: IdentityPolicy::Filename,
            discovery: Default::default(),
            schema,
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }],
        crate::repository::RepositoryOptions {
            snapshot_mode: crate::repository::SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    let make = |filter: &str, evidence| Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(parse_filter(filter).unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence,
        },
    };
    for (filter, evidence, expected) in [
        ("kind = 'yes' && body ~ 'needle'", false, vec!["a"]),
        (
            "kind = 'yes' || body ~ 'needle'",
            false,
            vec!["a", "b", "c", "e"],
        ),
        (
            "kind = 'yes' || body ~ 'needle'",
            true,
            vec!["a", "b", "c", "e"],
        ),
        ("kind = 'yes' || other != 'ok'", false, vec!["a", "b", "e"]),
        ("kind = 'yes' || other != 'ok'", true, vec!["a", "b", "e"]),
        (
            "(kind = 'yes' || body ~ 'needle') && !(other = 'blocked')",
            false,
            vec!["a", "b", "c"],
        ),
        (
            "(kind = 'yes' || body ~ 'needle') && !(other = 'blocked')",
            true,
            vec!["a", "b", "c"],
        ),
    ] {
        let query = make(filter, evidence);
        let (actual, trace) = repo.explain_analyze(&query).unwrap();
        let reference = repo.search_reference(&query).unwrap();
        assert_eq!(actual.count.unwrap(), expected.len(), "{filter}");
        assert_eq!(
            actual
                .hits
                .iter()
                .map(|h| h.record_id.as_str())
                .collect::<Vec<_>>(),
            expected,
            "{filter}"
        );
        assert_eq!(actual.hits, reference.hits, "{filter}");
        assert_eq!(
            trace[0].actual.as_ref().unwrap().residual_evaluations,
            actual.counters.residual_evaluations
        );
        if filter == "kind = 'yes' && body ~ 'needle'" {
            assert_eq!(actual.counters.residual_evaluations, 2);
            assert_eq!(actual.counters.scanned_records, 2);
        }
        if filter == "kind = 'yes' || body ~ 'needle'" && !evidence {
            assert_eq!(actual.counters.exact_posting_matches, 2);
            assert_eq!(actual.counters.scanned_records, 2);
            assert_eq!(actual.counters.residual_evaluations, 2);
            assert!(trace[0].terms.iter().any(|term| term.field == "body"
                && term.selected.kind == crate::executor::AccessPathKind::RawBigram));
        }
        if filter == "kind = 'yes' || body ~ 'needle'" && evidence {
            assert_eq!(actual.hits[0].matched_fields, ["body", "kind"]);
            assert_eq!(actual.hits[1].matched_fields, ["kind"]);
        }
        if filter == "kind = 'yes' || other != 'ok'" && !evidence {
            assert_eq!(actual.counters.exact_posting_matches, 2);
            assert_eq!(actual.counters.scanned_records, 83);
            assert_eq!(actual.counters.residual_evaluations, 83);
        }
        if filter == "kind = 'yes' || other != 'ok'" && evidence {
            assert_eq!(actual.counters.scanned_records, 85);
            assert_eq!(actual.counters.residual_evaluations, 83);
        }
    }
    assert!(
        repo.search(&make("kind = 'yes' || kind:unknown = 'x'", false))
            .is_err()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn stored_order_pages_stop_globally_and_count_remains_optional() {
    use crate::workload::SortField;

    let root = std::env::temp_dir().join(format!("objekte-page-only-{}", std::process::id()));
    for collection in ["a", "b"] {
        std::fs::create_dir_all(root.join(collection)).unwrap();
        for n in 0..100 {
            std::fs::write(
                root.join(format!("{collection}/{collection}{n:03}.json")),
                format!(r#"{{"sequence":{n},"kind":"yes","body":"needle {n}"}}"#),
            )
            .unwrap();
        }
    }
    let mut schema = Schema::default();
    for (path, kind, indexed) in [
        ("sequence", LogicalType::Number, false),
        ("kind", LogicalType::String, true),
        ("body", LogicalType::String, false),
    ] {
        schema
            .register_field(FieldDefinition {
                path: vec![path.into()],
                kind,
                required: true,
                nullable: false,
                indexed,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    let definitions = ["a", "b"].map(|name| CollectionDefinition {
        name: name.into(),
        source_path: name.into(),
        format: SourceFormat::Json,
        identity: IdentityPolicy::Filename,
        discovery: Default::default(),
        schema: schema.clone(),
        sort_fields: vec![
            crate::model::FieldPath::parse("sequence").unwrap(),
            crate::model::FieldPath::parse("body").unwrap(),
        ],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    });
    let repo = Repository::open_with_options(
        &root,
        definitions.to_vec(),
        crate::repository::RepositoryOptions {
            snapshot_mode: crate::repository::SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    let mut query = Query {
        collections: vec![],
        fts: None,
        filter: Some(parse_filter("kind = 'yes'").unwrap()),
        options: QueryOptions {
            sort: vec![SortField {
                field: "sequence".into(),
                descending: false,
            }],
            limit: 5,
            offset: 7,
            count: false,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    };
    let (page, trace) = repo.explain_analyze(&query).unwrap();
    assert_eq!(page.count, None);
    assert_eq!(
        page.hits
            .iter()
            .map(|h| h.record_id.as_str())
            .collect::<Vec<_>>(),
        ["b003", "a004", "b004", "a005", "b005"]
    );
    assert_eq!(page.counters.exact_posting_matches, 12);
    assert_eq!(page.counters.scanned_records, 0);
    assert!(!page.counters.match_count_complete);
    assert!(
        trace
            .iter()
            .all(|part| part.fts_strategy == "stored order page traversal")
    );
    query.options.count = true;
    let counted = repo.search(&query).unwrap();
    assert_eq!(counted.count, Some(200));
    assert!(counted.counters.match_count_complete);
    assert_eq!(page.hits, counted.hits);
    assert_eq!(counted.counters.exact_posting_matches, 200);

    query.options.count = false;
    for mode in [ReturnMode::Bytes, ReturnMode::Logical] {
        query.options.return_mode = mode.clone();
        let partial = repo.search(&query).unwrap();
        assert_eq!(partial.count, None);
        assert_eq!(partial.counters.files_opened, 5);
        assert_eq!(
            partial.counters.record_parses,
            if mode == ReturnMode::Logical { 5 } else { 0 }
        );
        query.options.count = true;
        let complete = repo.search(&query).unwrap();
        assert_eq!(partial.hits, complete.hits);
        query.options.count = false;
    }

    query.options.return_mode = ReturnMode::Locator;
    query.filter = Some(parse_filter("kind = 'yes' && body ~ 'needle'").unwrap());
    let inexact = repo.search(&query).unwrap();
    assert_eq!(inexact.count, None);
    assert_eq!(inexact.counters.scanned_records, 12);
    query.options.count = true;
    let full = repo.search(&query).unwrap();
    assert_eq!(inexact.hits, full.hits);
    assert_eq!(full.count, Some(200));
    assert!(full.counters.scanned_records >= 200);

    query.options.count = false;
    query.filter = Some(parse_filter("kind = 'no'").unwrap());
    let none = repo.search(&query).unwrap();
    assert_eq!(none.count, None);
    assert!(none.hits.is_empty());
    assert_eq!(none.counters.scanned_records, 0);

    query.filter = Some(parse_filter("kind = 'yes'").unwrap());
    query.options.sort[0].descending = true;
    let (descending, descending_trace) = repo.explain_analyze(&query).unwrap();
    assert!(
        descending_trace
            .iter()
            .all(|part| part.fts_strategy == "stored order page traversal")
    );
    assert_eq!(descending.counters.exact_posting_matches, 12);
    query.options.count = true;
    assert_eq!(descending.hits, repo.search(&query).unwrap().hits);
    query.options.count = false;
    query.options.sort[0].descending = false;
    query.options.sort.clear();
    let unsorted = repo.search(&query).unwrap();
    assert_eq!(unsorted.count, None);
    assert_eq!(
        unsorted
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["a007", "a008", "a009", "a010", "a011"]
    );
    assert_eq!(unsorted.counters.exact_posting_matches, 12);
    query.options.count = true;
    assert_eq!(unsorted.hits, repo.search(&query).unwrap().hits);
    query.options.count = false;
    query.options.sort = vec![SortField {
        field: "sequence".into(),
        descending: false,
    }];
    query.options.offset = 198;
    let deep = repo.search(&query).unwrap();
    assert_eq!(deep.count, None);
    assert_eq!(deep.hits.len(), 2);
    query.options.count = true;
    assert_eq!(deep.hits, repo.search(&query).unwrap().hits);

    query.options.count = false;
    query.options.offset = 7;
    query.options.sort.push(SortField {
        field: "body".into(),
        descending: false,
    });
    let multiple_fields = repo.search(&query).unwrap();
    assert_eq!(multiple_fields.count, None);
    assert!(multiple_fields.counters.match_count_complete);
    query.options.count = true;
    assert_eq!(multiple_fields.hits, repo.search(&query).unwrap().hits);
    query.options.sort.pop();

    query.options.count = false;
    query.options.limit = 0;
    let empty_page = repo.search(&query).unwrap();
    assert_eq!(empty_page.count, None);
    assert!(empty_page.hits.is_empty());
    assert_eq!(empty_page.counters.scanned_records, 0);

    let cached_repo = Repository::open_with_options(
        &root,
        definitions.to_vec(),
        crate::repository::RepositoryOptions {
            snapshot_mode: crate::repository::SnapshotMode::Disabled,
            verified_cache_bytes: 1 << 20,
            ..Default::default()
        },
    )
    .unwrap();
    query.options.count = false;
    query.options.offset = 7;
    query.options.limit = 5;
    let first_page = cached_repo.search(&query).unwrap();
    assert!(!first_page.counters.verified_result_hit);
    query.options.count = true;
    let complete = cached_repo.search(&query).unwrap();
    assert!(!complete.counters.verified_result_hit);
    assert_eq!(complete.count, Some(200));
    query.options.count = false;
    let cached_page = cached_repo.search(&query).unwrap();
    assert!(cached_page.counters.verified_result_hit);
    assert_eq!(cached_page.count, None);
    assert_eq!(cached_page.hits, first_page.hits);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn one_jsonl_stored_order_page_reuses_one_handle_for_deep_residual() {
    use crate::workload::SortField;
    let root = std::env::temp_dir().join(format!("objekte-ordered-jsonl-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let mut data = Vec::new();
    let mut starts = Vec::new();
    for n in 0..6000 {
        starts.push(data.len() as u64);
        data.extend_from_slice(
            format!(r#"{{"sequence":{n},"kind":"yes","body":"needle"}}"#).as_bytes(),
        );
        data.push(b'\n');
    }
    std::fs::write(root.join("data/all.jsonl"), data).unwrap();
    let mut schema = Schema::default();
    for (name, kind, indexed) in [
        ("sequence", LogicalType::Number, false),
        ("kind", LogicalType::String, true),
        ("body", LogicalType::String, false),
    ] {
        schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required: true,
                nullable: false,
                indexed,
                multi: false,
                element_nullable: vec![],
            })
            .unwrap();
    }
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::JsonLines,
            identity: IdentityPolicy::Generated,
            discovery: Default::default(),
            schema,
            sort_fields: vec![crate::model::FieldPath::parse("sequence").unwrap()],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }],
        crate::repository::RepositoryOptions {
            snapshot_mode: crate::repository::SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    let query = |filter: &str, mode, count| Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(parse_filter(filter).unwrap()),
        options: QueryOptions {
            sort: vec![SortField {
                field: "sequence".into(),
                descending: false,
            }],
            limit: 5,
            offset: 1000,
            count,
            return_mode: mode,
            evidence: false,
        },
    };
    let residual = query("body ~ 'needle'", ReturnMode::Locator, false);
    let (result, traces) = repo.explain_analyze(&residual).unwrap();
    assert_eq!(result.count, None);
    assert_eq!(
        result
            .hits
            .iter()
            .map(|hit| hit.byte_start)
            .collect::<Vec<_>>(),
        starts[1000..1005]
    );
    assert_eq!(result.counters.scanned_records, 1005);
    assert_eq!(result.counters.residual_evaluations, 1005);
    assert_eq!(result.counters.record_parses, 1005);
    assert_eq!(result.counters.files_opened, 1);
    assert_eq!(result.counters.distinct_files, 1);
    assert_eq!(traces[0].fts_strategy, "stored order page traversal");
    let exact = repo
        .search(&query("kind = 'yes'", ReturnMode::Logical, false))
        .unwrap();
    assert_eq!(
        exact
            .hits
            .iter()
            .map(|hit| hit.byte_start)
            .collect::<Vec<_>>(),
        starts[1000..1005]
    );
    assert_eq!(exact.counters.files_opened, 1);
    assert_eq!(exact.counters.distinct_files, 1);
    assert_eq!(exact.counters.record_parses, 5);
    repo.configure_verified_cache(4 << 20).unwrap();
    let counted = query("kind = 'yes'", ReturnMode::Logical, true);
    let first = repo.search(&counted).unwrap();
    let cached = repo.search(&counted).unwrap();
    assert_eq!(first.count, Some(6000));
    assert!(cached.counters.verified_result_hit);
    assert_eq!(cached.counters.files_opened, 1);
    assert_eq!(cached.counters.distinct_files, 1);
    assert_eq!(cached.counters.record_parses, 5);
    assert_eq!(cached.hits, first.hits);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn interleaved_stored_order_records_keep_bounded_file_handles() {
    use crate::workload::SortField;
    let root =
        std::env::temp_dir().join(format!("objekte-interleaved-jsonl-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for file in 0..3 {
        let mut data = String::new();
        for row in 0..30 {
            data.push_str(&format!(
                "{{\"sequence\":{},\"body\":\"needle\"}}\n",
                row * 3 + file
            ));
        }
        std::fs::write(root.join(format!("data/{file}.jsonl")), data).unwrap();
    }
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["body".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: false,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    schema
        .register_field(FieldDefinition {
            path: vec!["sequence".into()],
            kind: LogicalType::Number,
            required: true,
            nullable: false,
            indexed: false,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::JsonLines,
            identity: IdentityPolicy::Generated,
            discovery: Default::default(),
            schema,
            sort_fields: vec![crate::model::FieldPath::parse("sequence").unwrap()],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }],
        crate::repository::RepositoryOptions {
            snapshot_mode: crate::repository::SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    let query = Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(parse_filter("body ~ 'needle'").unwrap()),
        options: QueryOptions {
            sort: vec![SortField {
                field: "sequence".into(),
                descending: false,
            }],
            limit: 5,
            offset: 7,
            count: false,
            return_mode: ReturnMode::Logical,
            evidence: false,
        },
    };
    let result = repo.search(&query).unwrap();
    assert_eq!(result.count, None);
    assert_eq!(result.counters.scanned_records, 12);
    assert_eq!(result.counters.files_opened, 3);
    assert_eq!(result.counters.distinct_files, 3);
    assert_eq!(result.counters.record_parses, 12);
    assert_eq!(
        result
            .hits
            .iter()
            .map(|hit| match &hit.payload {
                Some(Payload::Logical(Value::Object(fields))) => match &fields["sequence"] {
                    Value::Number(crate::model::Number::Integer(n)) => *n,
                    _ => panic!("number"),
                },
                _ => panic!("logical payload"),
            })
            .collect::<Vec<_>>(),
        vec![7, 8, 9, 10, 11]
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn large_multi_record_scans_bound_payloads_and_reuse_file_handles() {
    use crate::query::FtsQuery;

    let root = std::env::temp_dir().join(format!("objekte-large-jsonl-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let padding = "x".repeat(64);
    for file in ["left", "right"] {
        let mut body = String::new();
        for n in 0..2000 {
            let rare = if n % 500 == 0 { " rare-token" } else { "" };
            body.push_str(&format!(
                r#"{{"kind":"yes","body":"needle{rare}","padding":"{padding}"}}"#
            ));
            body.push('\n');
        }
        std::fs::write(root.join(format!("data/{file}.jsonl")), body).unwrap();
    }
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::JsonLines,
            identity: IdentityPolicy::Generated,
            discovery: Default::default(),
            schema: Default::default(),
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }],
        crate::repository::RepositoryOptions {
            query_workers: 4,
            snapshot_mode: crate::repository::SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    let mut query = Query {
        collections: vec![],
        fts: Some(FtsQuery {
            literal: "needle".into(),
            fields: vec![],
        }),
        filter: None,
        options: QueryOptions {
            sort: vec![],
            limit: 5,
            offset: 7,
            count: true,
            return_mode: ReturnMode::Logical,
            evidence: true,
        },
    };
    let dense = repo.search(&query).unwrap();
    assert_eq!(dense.count, Some(4000));
    assert_eq!(dense.hits.len(), 5);
    assert_eq!(dense.counters.files_opened, 2);
    assert_eq!(dense.counters.record_parses, 4000);
    assert_eq!(dense.counters.scanned_records, 4000);
    assert!(
        dense
            .hits
            .iter()
            .all(|hit| matches!(hit.payload, Some(Payload::Logical(_))))
    );
    let max_record_len = 128u64;
    assert!(
        dense.counters.peak_retained_payload_bytes <= 3 * 12 * max_record_len,
        "retained {} bytes",
        dense.counters.peak_retained_payload_bytes
    );
    let reference = repo.search_reference(&query).unwrap();
    assert_eq!(dense.hits, reference.hits);

    query.options.return_mode = ReturnMode::Bytes;
    let bytes = repo.search(&query).unwrap();
    assert_eq!(bytes.count, Some(4000));
    assert_eq!(bytes.counters.files_opened, 2);
    assert_eq!(bytes.counters.record_parses, 4000);
    assert!(
        bytes
            .hits
            .iter()
            .all(|hit| matches!(hit.payload, Some(Payload::Bytes(_))))
    );
    assert_eq!(bytes.hits, repo.search_reference(&query).unwrap().hits);

    query.options.return_mode = ReturnMode::Locator;
    let locator = repo.search(&query).unwrap();
    assert_eq!(locator.count, Some(4000));
    assert_eq!(locator.counters.peak_retained_payload_bytes, 0);
    assert!(locator.hits.iter().all(|hit| hit.payload.is_none()));
    query.options.return_mode = ReturnMode::Logical;
    query.options.limit = 0;
    query.options.offset = 0;
    let count_only = repo.search(&query).unwrap();
    assert_eq!(count_only.count, Some(4000));
    assert!(count_only.hits.is_empty());
    assert_eq!(count_only.counters.files_opened, 2);
    assert_eq!(count_only.counters.record_parses, 4000);
    assert_eq!(count_only.counters.peak_retained_payload_bytes, 0);

    query.options.limit = 5;
    query.options.offset = 1000;
    let deep = repo.search(&query).unwrap();
    assert_eq!(deep.count, Some(4000));
    assert_eq!(deep.hits, repo.search_reference(&query).unwrap().hits);
    assert!(deep.counters.peak_retained_payload_bytes > dense.counters.peak_retained_payload_bytes);
    assert!(deep.counters.peak_retained_payload_bytes <= 3 * 1005 * max_record_len);

    query.options.offset = 0;
    query.fts.as_mut().unwrap().literal = "rare-token".into();
    let sparse = repo.search(&query).unwrap();
    assert_eq!(sparse.count, Some(8));
    assert_eq!(sparse.counters.files_opened, 2);
    assert_eq!(sparse.counters.record_parses, 8);
    assert_eq!(sparse.hits, repo.search_reference(&query).unwrap().hits);
    assert!(sparse.counters.bytes_read < repo.collections()[0].stats.total_file_bytes / 10);

    query.fts.as_mut().unwrap().literal = "needle".into();
    repo.configure_verified_cache(1024).unwrap();
    let first = repo.search(&query).unwrap();
    let second = repo.search(&query).unwrap();
    assert!(!first.counters.verified_result_hit && !second.counters.verified_result_hit);
    assert_eq!(first.count, Some(4000));
    repo.configure_verified_cache(4 << 20).unwrap();
    let complete = repo.search(&query).unwrap();
    let cached = repo.search(&query).unwrap();
    assert!(!complete.counters.verified_result_hit);
    assert!(cached.counters.verified_result_hit);
    assert_eq!(complete.hits, cached.hits);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn parallel_file_scan_matches_single_worker_page_and_work() {
    let root = std::env::temp_dir().join(format!("objekte-parallel-scan-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for index in 0..512 {
        std::fs::write(
            root.join("data").join(format!("{index:04}.json")),
            format!(
                "{{\"kind\":\"{}\",\"sequence\":{},\"decoration\":\"complete\"}}",
                if index % 2 == 0 { "yes" } else { "no" },
                index % 300
            ),
        )
        .unwrap();
    }
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["kind".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: false,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    schema
        .register_field(FieldDefinition {
            path: vec!["sequence".into()],
            kind: LogicalType::Number,
            required: true,
            nullable: false,
            indexed: false,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    let definition = CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::Json,
        identity: IdentityPolicy::Filename,
        discovery: Default::default(),
        schema,
        sort_fields: vec![crate::model::FieldPath::parse("sequence").unwrap()],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    };
    let query = Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(parse_filter("(kind = 'yes' || kind = 'no') && !(kind = 'missing')").unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 50,
            offset: 25,
            count: true,
            return_mode: ReturnMode::Logical,
            evidence: true,
        },
    };
    let run = |workers, query: &Query| {
        let repo = Repository::open_with_options(
            &root,
            vec![definition.clone()],
            crate::repository::RepositoryOptions {
                query_workers: workers,
                snapshot_mode: crate::repository::SnapshotMode::Disabled,
                ..Default::default()
            },
        )
        .unwrap();
        repo.search(query).unwrap()
    };
    let mut sorted = query.clone();
    sorted.options.sort = vec![crate::workload::SortField {
        field: "sequence".into(),
        descending: true,
    }];
    for query in [&query, &sorted] {
        let single = run(1, query);
        let parallel = run(8, query);
        let mut without_evidence = query.clone();
        without_evidence.options.evidence = false;
        let plain = run(8, &without_evidence);
        assert_eq!(single.count.unwrap(), 512);
        assert_eq!(single.hits, parallel.hits);
        assert_eq!(single.count.unwrap(), plain.count.unwrap());
        assert_eq!(
            single
                .hits
                .iter()
                .map(|hit| &hit.record_id)
                .collect::<Vec<_>>(),
            plain
                .hits
                .iter()
                .map(|hit| &hit.record_id)
                .collect::<Vec<_>>()
        );
        assert!(plain.hits.iter().all(|hit| hit.matched_fields.is_empty()));
        assert!(single.hits.iter().all(|hit| matches!(
            &hit.payload,
            Some(Payload::Logical(Value::Object(fields))) if fields.iter().any(|(name, _)| name == "decoration")
        )));
        if !query.options.sort.is_empty() {
            assert!(
                single
                    .hits
                    .iter()
                    .any(|hit| hit.record_id.as_str() >= "0256")
            );
        }
        for result in [&single, &parallel] {
            assert_eq!(result.counters.files_opened, 512);
            assert_eq!(result.counters.record_parses, 512);
            assert_eq!(result.counters.scanned_records, 512);
            assert_eq!(result.counters.total_matches, 512);
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn exact_membership_materializes_only_page_payload() {
    let root = std::env::temp_dir().join(format!("objekte-payload-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for id in ["a", "b", "c"] {
        std::fs::write(
            root.join("data").join(format!("{id}.json")),
            format!("{{\"kind\":\"yes\",\"id\":\"{id}\"}}"),
        )
        .unwrap();
    }
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["kind".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    let repo = Repository::open(
        &root,
        vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::Json,
            identity: IdentityPolicy::Filename,
            discovery: Default::default(),
            schema,
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }],
    )
    .unwrap();
    for (mode, parses, reads) in [
        (ReturnMode::Locator, 0, 0),
        (ReturnMode::Bytes, 0, 1),
        (ReturnMode::Logical, 1, 1),
    ] {
        let query = Query {
            collections: vec![],
            fts: None,
            filter: Some(parse_filter("kind = 'yes'").unwrap()),
            options: QueryOptions {
                sort: vec![],
                limit: 1,
                offset: 1,
                count: true,
                return_mode: mode.clone(),
                evidence: true,
            },
        };
        let result = repo.search(&query).unwrap();
        assert_eq!(
            result.counters.prepared_plan_hit,
            mode != ReturnMode::Locator
        );
        assert!(!result.counters.verified_result_hit);
        if mode == ReturnMode::Locator {
            let plan = repo.explain(&query).unwrap();
            assert!(plan[0].actual.is_none());
            assert_eq!(plan[0].terms[0].field, "kind");
            let (analyzed, trace) = repo.explain_analyze(&query).unwrap();
            assert_eq!(analyzed.count.unwrap(), result.count.unwrap());
            assert_eq!(trace[0].actual.as_ref().unwrap().files_opened, 0);
            assert_eq!(trace[0].actual.as_ref().unwrap().index_proven_matches, 3);
        }
        assert_eq!(result.count.unwrap(), 3);
        assert_eq!(result.hits.len(), 1);
        assert_eq!(result.hits[0].record_id, "b");
        assert_eq!(result.counters.record_parses, parses);
        assert_eq!(result.counters.files_opened, reads);
        assert_eq!(result.hits[0].matched_fields, ["kind"]);
        match mode {
            ReturnMode::Locator => assert!(result.hits[0].payload.is_none()),
            ReturnMode::Bytes => assert!(
                matches!(&result.hits[0].payload,Some(Payload::Bytes(b)) if b.starts_with(b"{\"kind\""))
            ),
            ReturnMode::Logical => {
                assert!(matches!(&result.hits[0].payload, Some(Payload::Logical(_))))
            }
        }
    }
    assert_eq!(repo.verified_cache_budget().unwrap(), 0);
    repo.configure_verified_cache(1024 * 1024).unwrap();
    assert_eq!(repo.verified_cache_budget().unwrap(), 1024 * 1024);
    let mut query = Query {
        collections: vec![],
        fts: None,
        filter: Some(parse_filter("kind = 'yes'").unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 1,
            offset: 1,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    };
    let miss = repo.search(&query).unwrap();
    assert!(!miss.counters.verified_result_hit);
    let hit = repo.search(&query).unwrap();
    assert!(hit.counters.verified_result_hit);
    assert_eq!(hit.count.unwrap(), 3);
    assert_eq!(hit.hits[0].record_id, "b");
    assert_eq!(hit.hits[0].matched_fields, ["kind"]);
    assert_eq!(hit.counters.files_opened, 0);
    query.options.offset = 2;
    query.options.return_mode = ReturnMode::Logical;
    let page = repo.search(&query).unwrap();
    assert!(page.counters.verified_result_hit);
    assert_eq!(page.hits[0].record_id, "c");
    assert_eq!(page.counters.record_parses, 1);
    query.filter = Some(parse_filter("kind = 'absent'").unwrap());
    assert!(!repo.search(&query).unwrap().counters.verified_result_hit);
    let empty = repo.search(&query).unwrap();
    assert!(empty.counters.verified_result_hit);
    assert_eq!(empty.count.unwrap(), 0);
    query.filter = Some(parse_filter("kind = 'yes'").unwrap());
    query.options.offset = 1;
    query.options.return_mode = ReturnMode::Bytes;
    std::fs::write(
        root.join("data/b.json"),
        b"{\"kind\":\"no\",\"id\":\"b\",\"extra\":true}",
    )
    .unwrap();
    assert!(
        repo.search(&query)
            .unwrap_err()
            .contains("stale source revision")
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn multi_record_scan_opens_file_once_and_parses_each_range_once() {
    let root = std::env::temp_dir().join(format!("objekte-multi-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/multi.jsonl"),
        "{\"name\":\"Ähre\"}\n{\"name\":\"Zug\"}\n",
    )
    .unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["name".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: false,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    let repo = Repository::open(
        &root,
        vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::JsonLines,
            identity: IdentityPolicy::Generated,
            discovery: Default::default(),
            schema,
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }],
    )
    .unwrap();
    assert_eq!(
        repo.collections[0]
            .generation
            .file_records
            .records_for(crate::model::FileId(0))
            .unwrap()
            .len(),
        2
    );
    let query = Query {
        collections: vec![],
        fts: None,
        filter: Some(parse_filter("name:exists = true").unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 2,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Logical,
            evidence: true,
        },
    };
    let result = repo.search(&query).unwrap();
    assert_eq!(result.count.unwrap(), 2);
    assert_eq!(result.counters.files_opened, 1);
    assert_eq!(result.counters.record_parses, 2);
    assert!(
        result
            .hits
            .iter()
            .all(|hit| matches!(hit.payload, Some(Payload::Logical(_))))
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn exact_boolean_evidence_tracks_only_true_branches() {
    let root = std::env::temp_dir().join(format!("objekte-evidence-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/x.json"),
        b"{\"a\":\"yes\",\"b\":\"no\",\"c\":\"yes\"}",
    )
    .unwrap();
    let mut schema = Schema::default();
    for name in ["a", "b", "c"] {
        schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind: LogicalType::String,
                required: true,
                nullable: false,
                indexed: true,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    let repo = Repository::open(
        &root,
        vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::Json,
            identity: IdentityPolicy::Filename,
            discovery: Default::default(),
            schema,
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }],
    )
    .unwrap();
    let query = Query {
        collections: vec![],
        fts: None,
        filter: Some(parse_filter("(a = 'yes' && b = 'absent') || c = 'yes'").unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    };
    let optimized = repo.search(&query).unwrap();
    let reference = repo.search_reference(&query).unwrap();
    assert_eq!(optimized.count.unwrap(), 1);
    assert_eq!(optimized.counters.record_parses, 0);
    assert_eq!(optimized.hits[0].matched_fields, ["c"]);
    assert_eq!(
        optimized.hits[0].matched_fields,
        reference.hits[0].matched_fields
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn typed_temporal_fields_survive_postings_payload_cache_and_restore() {
    let root = std::env::temp_dir().join(format!("objekte-temporal-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/a.json"),
        b"{\"day\":\"2026-01-01\",\"seconds\":1767225600}",
    )
    .unwrap();
    std::fs::write(
        root.join("data/b.json"),
        b"{\"day\":\"2026-02-01\",\"seconds\":1769904000}",
    )
    .unwrap();
    let mut schema = Schema::default();
    for (name, kind) in [
        ("day", LogicalType::Date),
        ("seconds", LogicalType::Unixtime),
    ] {
        schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required: true,
                nullable: false,
                indexed: true,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    let definitions = vec![CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::Json,
        identity: IdentityPolicy::Filename,
        discovery: Default::default(),
        schema,
        sort_fields: vec![crate::model::FieldPath::parse("day").unwrap()],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    }];
    let query = Query {
        collections: vec![],
        fts: None,
        filter: Some(parse_filter("day >= '2026-01-15' && seconds >= 1769904000").unwrap()),
        options: QueryOptions {
            sort: vec![crate::workload::SortField {
                field: "day".into(),
                descending: false,
            }],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Logical,
            evidence: true,
        },
    };
    let repo = Repository::open(&root, definitions.clone()).unwrap();
    let optimized = repo.search(&query).unwrap();
    let reference = repo.search_reference(&query).unwrap();
    assert_eq!(optimized.count.unwrap(), 1);
    assert_eq!(optimized.hits[0].record_id, "b");
    assert_eq!(optimized.hits[0].payload, reference.hits[0].payload);
    assert!(
        matches!(&optimized.hits[0].payload,Some(Payload::Logical(crate::model::Value::Object(fields))) if matches!(fields["day"],crate::model::Value::Date(_)))
    );
    repo.configure_verified_cache(1024 * 1024).unwrap();
    repo.search(&query).unwrap();
    let cached = repo.search(&query).unwrap();
    assert!(cached.counters.verified_result_hit);
    assert_eq!(cached.hits[0].payload, optimized.hits[0].payload);
    let restored = Repository::open(&root, definitions).unwrap();
    assert!(restored.startup.restored);
    assert_eq!(
        restored.search(&query).unwrap().hits[0].payload,
        optimized.hits[0].payload
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn optional_temporal_parent_queries_keep_absent_null_and_present_records() {
    let root = std::env::temp_dir().join(format!("objekte-optional-date-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (id, json) in [
        ("absent", r#"{"note":"keep"}"#),
        ("null", r#"{"note":"keep","details":null}"#),
        (
            "present",
            r#"{"note":"keep","details":{"published":"2026-01-02"}}"#,
        ),
    ] {
        std::fs::write(root.join(format!("data/{id}.json")), json).unwrap();
    }
    let definition = |indexed| {
        let mut schema = Schema::default();
        for (path, kind, required, nullable, index) in [
            (
                vec!["note".into()],
                LogicalType::String,
                true,
                false,
                indexed,
            ),
            (
                vec!["details".into()],
                LogicalType::Object,
                false,
                true,
                false,
            ),
            (
                vec!["details".into(), "published".into()],
                LogicalType::Date,
                true,
                false,
                true,
            ),
        ] {
            schema
                .register_field(FieldDefinition {
                    path,
                    kind,
                    required,
                    nullable,
                    indexed: index,
                    multi: false,
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
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }
    };
    for indexed in [false, true] {
        let options = crate::repository::RepositoryOptions {
            snapshot_mode: crate::repository::SnapshotMode::ReadWrite,
            ..Default::default()
        };
        for open in 0..2 {
            let repo =
                Repository::open_with_options(&root, vec![definition(indexed)], options).unwrap();
            assert_eq!(repo.startup().restored, open == 1);
            for (filter, expected) in [
                ("note ~ 'keep'", vec!["absent", "null", "present"]),
                ("note = 'keep'", vec!["absent", "null", "present"]),
                ("details.published = '2026-01-02'", vec!["present"]),
            ] {
                for mode in [ReturnMode::Locator, ReturnMode::Bytes, ReturnMode::Logical] {
                    let query = Query {
                        collections: vec!["data".into()],
                        fts: None,
                        filter: Some(parse_filter(filter).unwrap()),
                        options: QueryOptions {
                            sort: vec![],
                            limit: 10,
                            offset: 0,
                            count: true,
                            return_mode: mode.clone(),
                            evidence: true,
                        },
                    };
                    let result = repo.search(&query).unwrap();
                    let ids = result
                        .hits
                        .iter()
                        .map(|h| h.record_id.as_str())
                        .collect::<Vec<_>>();
                    assert_eq!(ids, expected, "{indexed} {open} {filter} {mode:?}");
                    assert_eq!(result.count, Some(expected.len()));
                    if mode == ReturnMode::Logical && filter == "note = 'keep'" {
                        assert!(
                            matches!(&result.hits[2].payload, Some(Payload::Logical(Value::Object(fields)))
                            if matches!(&fields["details"], Value::Object(details) if matches!(details["published"], Value::Date(_))))
                        );
                    }
                }
                let count_only = Query {
                    collections: vec!["data".into()],
                    fts: None,
                    filter: Some(parse_filter(filter).unwrap()),
                    options: QueryOptions {
                        sort: vec![],
                        limit: 0,
                        offset: 0,
                        count: true,
                        return_mode: ReturnMode::Locator,
                        evidence: false,
                    },
                };
                assert_eq!(
                    repo.search(&count_only).unwrap().count,
                    Some(expected.len())
                );
            }
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn selected_folded_and_metadata_fts_match_reference() {
    let root = std::env::temp_dir().join(format!("objekte-selected-fts-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"), b"{\"text\":\"Stra\\u00dfe\"}").unwrap();
    std::fs::write(root.join("data/b.json"), b"{\"text\":\"Other\"}").unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["text".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: false,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    let repo = Repository::open(
        &root,
        vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::Json,
            identity: IdentityPolicy::Filename,
            discovery: Default::default(),
            schema,
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }],
    )
    .unwrap();
    let options = QueryOptions {
        sort: vec![],
        limit: 10,
        offset: 0,
        count: true,
        return_mode: ReturnMode::Locator,
        evidence: true,
    };
    for (literal, field) in [("STRASSE", "text:lower"), ("a.json", "@file.path")] {
        let (path, binding) = field
            .rsplit_once(':')
            .map_or((field, None), |(a, b)| (a, Some(b.to_owned())));
        let query = Query {
            collections: vec![],
            filter: None,
            fts: Some(crate::query::FtsQuery {
                literal: literal.into(),
                fields: vec![crate::query::FtsField {
                    path: crate::model::FieldPath::parse(path).unwrap(),
                    binding,
                }],
            }),
            options: options.clone(),
        };
        let optimized = repo.search(&query).unwrap();
        let reference = repo.search_reference(&query).unwrap();
        assert_eq!(optimized.count.unwrap(), 1);
        assert_eq!(optimized.hits[0].record_id, "a");
        if field == "@file.path" {
            assert_eq!(optimized.counters.record_parses, 0);
        }
        assert_eq!(
            optimized.hits[0].matched_fields,
            reference.hits[0].matched_fields
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn uncovered_posting_unindexed_or_negation_and_empty_conjunct_remain_exact() {
    let root = std::env::temp_dir().join(format!("objekte-coverage-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (id, kind, text) in [
        ("a", "yes", "xray"),
        ("b", "no", "other"),
        ("c", "no", "xenon"),
    ] {
        std::fs::write(
            root.join("data").join(format!("{id}.json")),
            serde_json::json!({"kind":kind,"text":text}).to_string(),
        )
        .unwrap();
    }
    let mut schema = Schema::default();
    for (name, indexed) in [("kind", true), ("text", false)] {
        schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind: LogicalType::String,
                required: true,
                nullable: false,
                indexed,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    let mut repo = Repository::open(
        &root,
        vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::Json,
            identity: IdentityPolicy::Filename,
            discovery: Default::default(),
            schema,
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }],
    )
    .unwrap();
    let options = QueryOptions {
        sort: vec![],
        limit: 10,
        offset: 0,
        count: true,
        return_mode: ReturnMode::Locator,
        evidence: true,
    };
    for (expression, expected) in [
        ("kind = 'missing' && text ~ 'x'", 0),
        ("kind = 'missing' || text ~ 'x'", 2),
        ("!(text ~ 'x')", 1),
    ] {
        let query = Query {
            collections: vec![],
            fts: None,
            filter: Some(parse_filter(expression).unwrap()),
            options: options.clone(),
        };
        let optimized = repo.search(&query).unwrap();
        let reference = repo.search_reference(&query).unwrap();
        assert_eq!(optimized.count.unwrap(), expected);
        assert_eq!(optimized.count.unwrap(), reference.count.unwrap());
        assert_eq!(
            optimized
                .hits
                .iter()
                .map(|h| &h.record_id)
                .collect::<Vec<_>>(),
            reference
                .hits
                .iter()
                .map(|h| &h.record_id)
                .collect::<Vec<_>>()
        );
        if expression.starts_with("kind = 'missing' &&") {
            assert_eq!(optimized.counters.files_opened, 0);
        }
    }
    repo.collections[0]
        .postings
        .fields
        .get_mut(&vec!["kind".into()])
        .unwrap()
        .covered_records = 1;
    let query = Query {
        collections: vec![],
        fts: None,
        filter: Some(parse_filter("kind = 'yes'").unwrap()),
        options,
    };
    let optimized = repo.search(&query).unwrap();
    assert_eq!(
        optimized.count.unwrap(),
        repo.search_reference(&query).unwrap().count.unwrap()
    );
    assert!(optimized.counters.record_parses > 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn quoted_data_name_does_not_shadow_metadata() {
    let root = std::env::temp_dir().join(format!("objekte-metadata-name-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"), b"{\"@file\":{\"size\":\"data\"}}").unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["@file".into(), "size".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    let repo = Repository::open(
        &root,
        vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::Json,
            identity: IdentityPolicy::Filename,
            discovery: Default::default(),
            schema,
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }],
    )
    .unwrap();
    let query = Query {
        collections: vec![],
        fts: None,
        filter: Some(parse_filter("[\"@file\"].size = 'data' && @file.size > 0").unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    };
    let optimized = repo.search(&query).unwrap();
    let reference = repo.search_reference(&query).unwrap();
    assert_eq!(optimized.count.unwrap(), 1);
    assert_eq!(optimized.counters.record_parses, 0);
    assert_eq!(
        optimized.hits[0].matched_fields,
        reference.hits[0].matched_fields
    );
    assert!(
        optimized.hits[0]
            .matched_fields
            .contains(&"[\"@file\"].size".to_string())
    );
    assert!(
        optimized.hits[0]
            .matched_fields
            .contains(&"@file.size".to_string())
    );
    std::fs::remove_dir_all(root).unwrap();
}
