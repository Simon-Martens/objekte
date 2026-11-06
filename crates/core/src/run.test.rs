use super::*;
use crate::model::LogicalType;
use crate::query::{QueryOptions, parse_filter};
use crate::reference::{FieldDefinition, Schema};
use crate::repository::{CollectionDefinition, IdentityPolicy};
use crate::source::SourceFormat;

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
        assert_eq!(single.count, 512);
        assert_eq!(single.hits, parallel.hits);
        assert_eq!(single.count, plain.count);
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
            assert_eq!(analyzed.count, result.count);
            assert_eq!(trace[0].actual.as_ref().unwrap().files_opened, 0);
            assert_eq!(trace[0].actual.as_ref().unwrap().index_proven_matches, 3);
        }
        assert_eq!(result.count, 3);
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
    assert_eq!(hit.count, 3);
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
    assert_eq!(empty.count, 0);
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
    assert_eq!(result.count, 2);
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
    assert_eq!(optimized.count, 1);
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
    assert_eq!(optimized.count, 1);
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
        assert_eq!(optimized.count, 1);
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
        assert_eq!(optimized.count, expected);
        assert_eq!(optimized.count, reference.count);
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
        optimized.count,
        repo.search_reference(&query).unwrap().count
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
    assert_eq!(optimized.count, 1);
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
