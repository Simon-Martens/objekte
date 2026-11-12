use super::*;
use crate::model::LogicalType;
use crate::query::{FtsField, FtsQuery, Query, QueryOptions, parse_filter};
use crate::reference::{CustomBinding, FieldDefinition, Transform};
use crate::repository::{CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode};
use crate::source::SourceFormat;
use crate::workload::ReturnMode;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

#[test]
fn extended_bounds_compose_exact_field_states_with_uncovered_slots() {
    use crate::build::ExtendedBatch;
    use crate::field_state::{FieldStateColumnBuilder, classify_value};
    use crate::model::CollectionId;

    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::JsonLines);
    definition.schema.register_field(FieldDefinition {
        path: vec!["x".into()],
        kind: LogicalType::String,
        required: false,
        nullable: true,
        indexed: true,
        multi: false,
        element_nullable: vec![],
    }).unwrap();
    let mut column = FieldStateColumnBuilder::new();
    column.cover_slots(&[0, 1, 2].into_iter().collect());
    column.insert_mask(0, classify_value(&Value::String("value".into())).unwrap());
    column.insert_mask(1, classify_value(&Value::Null).unwrap());
    let batch = Arc::new(ExtendedBatch {
        collection: CollectionId(0),
        base_slot: 10,
        record_count: 3,
        records: Vec::new(),
        logical_covered: [0, 1, 2].into_iter().collect(),
        postings: crate::posting::PostingCollection { fields: Default::default() },
        custom_postings: Default::default(),
        field_states: [(vec!["x".into()], column.finish())].into(),
        source_files: Vec::new(),
    });
    let batches = [batch];
    let input: RoaringBitmap = [10, 11, 12, 13].into_iter().collect();
    let check = |filter: &str, may: &[u32], must: &[u32]| {
        let bound = crate::reference::bind_filter(
            &parse_filter(filter).unwrap(), &definition.schema,
        ).unwrap();
        let bounds = extended_filter_bounds(&batches, &bound, &input);
        assert_eq!(bounds.may.iter().collect::<Vec<_>>(), may, "{filter} may");
        assert_eq!(bounds.must.iter().collect::<Vec<_>>(), must, "{filter} must");
    };
    check("x:string = true", &[10, 13], &[10]);
    check("!(x:string = true)", &[11, 12, 13], &[11, 12]);
    check("x:string = true || x:null = true", &[10, 11, 13], &[10, 11]);
    check("x:string = true && x:null = true", &[13], &[]);
}

#[test]
fn randomized_partial_state_publications_bound_source_truth() {
    use crate::build::ExtendedBatch;
    use crate::field_state::{FieldStateColumnBuilder, classify_selected};
    use crate::inventory::{ByteRange, DiagnosticRef, FileEntry, FileStatus, RecordClass, RecordEntry, SourceRevision};
    use crate::model::{CollectionId, FileId, RecordId, RecordSlot};
    use crate::reference::{RecordContext, Schema, eval_filter};
    use std::collections::HashSet;

    let variants = [
        serde_json::json!({}),
        serde_json::json!({"x":null}),
        serde_json::json!({"x":""}),
        serde_json::json!({"x":"text"}),
        serde_json::json!({"x":0}),
        serde_json::json!({"x":-3}),
        serde_json::json!({"x":false}),
        serde_json::json!({"x":true}),
        serde_json::json!({"x":[]}),
        serde_json::json!({"x":[null,"text"]}),
        serde_json::json!({"x":{}}),
        serde_json::json!({"x":{"nested":1}}),
    ];
    let mut seed = 0x9e37_79b9u32;
    let records = (0..64).map(|_| {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        Value::from_json(variants[(seed as usize) % variants.len()].clone()).unwrap()
    }).collect::<Vec<_>>();
    let schema = Schema::default();
    let filters = [
        "x:? = true", "x:string = true", "x:number = true", "x:array = true",
        "x:object = true", "x:null = true", "x:exists = false",
        "!(x:string = true)", "x:string = true || x:number = true",
        "x:? = true && !(x:array = true)",
        "!(x:null = true || x:string = true)",
    ].map(|source| {
        (source, crate::reference::bind_filter(&parse_filter(source).unwrap(), &schema).unwrap())
    });
    let revision = SourceRevision { modified_ns: 0, size: 1 };
    let file = FileEntry {
        id: FileId(0), relative_path: "data.json".into(), revision,
        created_ns: None, collections: vec![CollectionId(0)],
        status: FileStatus::Valid, diagnostics: vec![], omitted_diagnostics: 0,
    };
    let mut truths = Vec::new();
    for (source, bound) in &filters {
        let mut truth = RoaringBitmap::new();
        for (slot, value) in records.iter().enumerate() {
            let record = RecordEntry {
                slot: RecordSlot(slot as u32), id: RecordId(format!("r{slot}")),
                file: FileId(0), range: ByteRange { start: 0, end: 1 }, revision,
                class: RecordClass::Logical, diagnostic_ref: DiagnosticRef(0),
            };
            let context = RecordContext {
                value, source_bytes: None, record: &record, file: &file,
                collection_name: "data", repository_root: std::path::Path::new("/tmp"),
            };
            if eval_filter(bound, &context, &mut HashSet::new()).unwrap() {
                truth.insert(slot as u32);
            }
        }
        truths.push((source, truth));
    }
    let domain: RoaringBitmap = (0..64).collect();
    let order = [7, 1, 5, 0, 3, 6, 2, 4];
    let mut batches = Vec::new();
    for published in 0..=order.len() {
        for ((source, bound), (_, truth)) in filters.iter().zip(&truths) {
            let bounds = extended_filter_bounds(&batches, bound, &domain);
            assert!(bounds.must.is_subset(truth), "false positive proof: {source}, {published}");
            assert!(truth.is_subset(&bounds.may), "false negative candidate: {source}, {published}");
            if published == order.len() {
                assert_eq!(bounds.may, *truth, "full may: {source}");
                assert_eq!(bounds.must, *truth, "full must: {source}");
            }
        }
        if published == order.len() { break; }
        let batch_index = order[published];
        let mut column = FieldStateColumnBuilder::new();
        column.cover_slots(&(0..8).collect());
        for local in 0..8 {
            let value = &records[batch_index * 8 + local];
            let Value::Object(map) = value else { unreachable!() };
            let selected = map.get("x").map(|value| vec![value]);
            let mask = classify_selected(selected.as_deref()).unwrap();
            if mask != 0 { column.insert_mask(local as u32, mask); }
        }
        let batch = ExtendedBatch {
            collection: CollectionId(0), base_slot: (batch_index * 8) as u32,
            record_count: 8, records: Vec::new(),
            logical_covered: (0..8).collect(),
            postings: crate::posting::PostingCollection { fields: Default::default() },
            custom_postings: Default::default(),
            field_states: [(vec!["x".into()], column.finish())].into(),
            source_files: Vec::new(),
        };
        batches.push(Arc::new(batch));
    }
}

#[test]
fn segment_filter_bounds_keep_unknown_fallback_and_exclude_retired_before_not() {
    use crate::build::{
        FilePreparationBinding, SegmentAdmission, SourceFileInput, prepare_file_build,
    };
    use crate::inventory::{ActiveRecords, SourceRevision};
    use crate::model::{CollectionId, FileId, RecordSlot};
    use crate::source::SourceRegistry;

    let root = std::env::temp_dir().join(format!(
        "objekte-segment-bounds-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let path = root.join("data/data.jsonl");
    std::fs::write(
        &path,
        b"{\"kind\":\"a\"}\n{\"kind\":\"b\"}\n{bad}\n{\"kind\":\"a\"}\n",
    )
    .unwrap();
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::JsonLines);
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["kind".into()],
            kind: LogicalType::String,
            required: false,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let source = SourceRegistry::builtin()
        .resolve(&SourceFormat::JsonLines)
        .unwrap();
    let inference = crate::inference::InferenceConfig::default();
    let alphabet = crate::workload::BigramAlphabet::Ascii;
    let prepared = prepare_file_build(
        &root,
        SourceFileInput {
            id: FileId(200),
            relative_path: "data/data.jsonl".into(),
            revision: SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
            created_ns: None,
        },
        &[FilePreparationBinding {
            collection: CollectionId(0),
            definition: &definition,
            source: &source,
        }],
        alphabet,
        inference,
        1,
    )
    .unwrap();
    let mut admission = SegmentAdmission::new(CollectionId(0), &source);
    admission
        .admit(
            &prepared.source,
            prepared.collections.into_iter().next().unwrap(),
        )
        .unwrap();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let segment = admission
        .seal()
        .into_indexed(&definition, alphabet, inference, 1, &pool)
        .unwrap();
    let repo = Repository::open_with_options(
        &root, vec![definition.clone()], RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    ).unwrap();
    let collection = &repo.collections()[0];
    assert_eq!(segment.records.len(), 4);
    let active = ActiveRecords::all(4)
        .unwrap()
        .retired(&[RecordSlot(3)])
        .unwrap();
    let positive =
        crate::reference::bind_filter(&parse_filter("kind = 'a'").unwrap(), &definition.schema)
            .unwrap();
    let bounds = segment_filter_bounds(&segment, &active, &positive, collection).unwrap();
    assert_eq!(bounds.may.iter().collect::<Vec<_>>(), [0, 2]);
    assert_eq!(bounds.must.iter().collect::<Vec<_>>(), [0]);
    let negative =
        crate::reference::bind_filter(&parse_filter("!(kind = 'a')").unwrap(), &definition.schema)
            .unwrap();
    let bounds = segment_filter_bounds(&segment, &active, &negative, collection).unwrap();
    assert_eq!(bounds.may.iter().collect::<Vec<_>>(), [1, 2]);
    assert_eq!(bounds.must.iter().collect::<Vec<_>>(), [1]);
    let impossible = crate::reference::bind_filter(
        &parse_filter("kind = 'a' && !(kind = 'a')").unwrap(),
        &definition.schema,
    )
    .unwrap();
    let bounds = segment_filter_bounds(&segment, &active, &impossible, collection).unwrap();
    assert_eq!(bounds.may.iter().collect::<Vec<_>>(), [2]);
    assert!(bounds.must.is_empty());
    let nonzero_start = crate::reference::bind_filter(
        &parse_filter("@record.byte_start > 0").unwrap(),
        &definition.schema,
    )
    .unwrap();
    let bounds = segment_filter_bounds(&segment, &active, &nonzero_start, collection).unwrap();
    assert_eq!(bounds.may.iter().collect::<Vec<_>>(), [1, 2]);
    assert_eq!(bounds.must.iter().collect::<Vec<_>>(), [1, 2]);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_segments_prove_declared_inferred_and_file_size_filters() {
    use crate::inference::{InferenceConfig, InferenceMode};

    let root = std::env::temp_dir().join(format!(
        "objekte-segment-exact-proofs-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for number in 0..24 {
        let mixed_year = if number == 0 {
            serde_json::Value::Null
        } else {
            serde_json::json!(number)
        };
        let payload = serde_json::json!({
            "kind": [if number % 3 == 0 { "Lied" } else { "Buch" }],
            "dynamic": {"type": [if number % 4 == 0 { "Person" } else { "Group" }]},
            "inferred": if number % 5 == 0 { "special" } else { "ordinary" },
            "year": 1700 + number * 10,
            "mixed_year": mixed_year,
            "padding": if number % 2 == 0 { "x".repeat(128) } else { String::new() },
        });
        std::fs::write(
            root.join(format!("data/{number:03}.json")),
            payload.to_string(),
        )
        .unwrap();
    }
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
    definition.identity = IdentityPolicy::Filename;
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["kind".into()],
            kind: LogicalType::Array(Box::new(LogicalType::String)),
            required: true,
            nullable: false,
            indexed: true,
            multi: true,
            element_nullable: vec![],
        })
        .unwrap();
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["year".into()],
            kind: LogicalType::Number,
            required: true,
            nullable: false,
            indexed: false,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["mixed_year".into()],
            kind: LogicalType::Number,
            required: false,
            nullable: true,
            indexed: false,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    definition.sort_fields = vec![
        crate::model::FieldPath::parse("year").unwrap(),
        crate::model::FieldPath::parse("mixed_year").unwrap(),
    ];
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["dynamic".into(), "type".into()],
            kind: LogicalType::Array(Box::new(LogicalType::String)),
            required: true,
            nullable: false,
            indexed: true,
            multi: true,
            element_nullable: vec![],
        })
        .unwrap();
    let mut inference = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        ..Default::default()
    };
    inference.policy.min_nonblank_occurrences = 1;
    inference.policy.min_nonblank_records = 1;
    let options = RepositoryOptions {
        inference,
        snapshot_mode: SnapshotMode::Disabled,
        ..Default::default()
    };
    let static_repo =
        Arc::new(Repository::open_with_options(&root, vec![definition.clone()], options).unwrap());
    let index = crate::lifecycle::Index::build(&root, vec![definition], options).unwrap();
    let view = index.view().unwrap();
    let published = crate::lifecycle::PublishedIndex::from_repository(
        crate::repository::next_repository_instance(),
        crate::lifecycle::IndexRevision::INITIAL,
        Arc::clone(&static_repo),
    )
    .unwrap();
    let partitioned =
        crate::lifecycle_partition::partition_base_with_limits(published, 5, 5).unwrap();
    let partitioned_index = crate::lifecycle::Index::from_published(partitioned);
    let partitioned_view = partitioned_index.view().unwrap();
    assert!(
        partitioned_view
            .segments(crate::model::CollectionId(0))
            .unwrap()
            .len()
            > 1
    );
    assert!(
        view.repository().collections()[0]
            .generation
            .records
            .is_empty()
    );
    for (filter, expected_count) in [
        ("kind ?~ 'Lied'", 8),
        ("dynamic.type ?~ 'Person'", 6),
        ("inferred = 'special'", 5),
        ("@file.size >= 160", 12),
        ("@file.modified >= 2000-01-01", 24),
        ("@record.byte_start = 0", 24),
        ("@record.byte_length > 0", 24),
        ("year >= 1600", 24),
        ("year < 2000", 24),
        ("year >= 1800", 14),
        ("mixed_year >= 0", 23),
    ] {
        let query = Query {
            collections: vec!["data".into()],
            fts: None,
            filter: Some(parse_filter(filter).unwrap()),
            options: QueryOptions {
                sort: vec![],
                limit: 2,
                offset: 1,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence: true,
            },
        };
        let expected = static_repo.search(&query).unwrap();
        for candidate_view in [&view, &partitioned_view] {
            let actual = candidate_view.search(&query).unwrap();
            assert_eq!(actual.count, expected.count, "{filter}");
            assert_eq!(actual.count, Some(expected_count), "{filter}");
            assert_eq!(actual.hits.len(), expected.hits.len(), "{filter}");
            assert_eq!(
                actual
                    .hits
                    .iter()
                    .map(|hit| &hit.record_id)
                    .collect::<Vec<_>>(),
                expected
                    .hits
                    .iter()
                    .map(|hit| &hit.record_id)
                    .collect::<Vec<_>>(),
                "{filter}"
            );
            assert_eq!(actual.counters.scanned_records, 0, "{filter}");
            assert_eq!(actual.counters.record_parses, 0, "{filter}");
            assert_eq!(
                actual.counters.exact_posting_matches, expected_count as u64,
                "{filter}"
            );
        }
    }
    for (filter, expected_count) in [("kind[0] = 'Lied'", 8), ("kind[1] = 'Lied'", 0)] {
        let query = Query {
            collections: vec!["data".into()],
            fts: None,
            filter: Some(parse_filter(filter).unwrap()),
            options: QueryOptions {
                sort: vec![],
                limit: 2,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence: true,
            },
        };
        let expected = static_repo.search(&query).unwrap();
        for candidate_view in [&view, &partitioned_view] {
            let actual = candidate_view.search(&query).unwrap();
            assert_eq!(actual.count, expected.count, "{filter}");
            assert_eq!(actual.count, Some(expected_count), "{filter}");
            assert_eq!(
                actual
                    .hits
                    .iter()
                    .map(|hit| &hit.record_id)
                    .collect::<Vec<_>>(),
                expected
                    .hits
                    .iter()
                    .map(|hit| &hit.record_id)
                    .collect::<Vec<_>>(),
                "{filter}"
            );
            assert_eq!(actual.counters.scanned_records, 8, "{filter}");
        }
    }
    partitioned_index.shutdown().unwrap();
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn named_builtin_folded_filter_keeps_its_compatible_bigram_path() {
    let root = std::env::temp_dir().join(format!("objekte-folded-index-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (id, title) in [
        ("a", "Straße Almanach"),
        ("b", "STRASSE ALMANACH"),
        ("c", "unrelated text"),
    ] {
        std::fs::write(
            root.join(format!("data/{id}.json")),
            serde_json::json!({"title":title}).to_string(),
        )
        .unwrap();
    }
    let mut schema = crate::reference::Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["title".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: false,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    schema
        .register_custom(
            vec!["title".into()],
            CustomBinding {
                name: "folded".into(),
                version: 1,
                transform: Transform::CaseFold,
            },
        )
        .unwrap();
    schema
        .register_custom(
            vec!["title".into()],
            CustomBinding {
                name: "plain".into(),
                version: 1,
                transform: Transform::Identity,
            },
        )
        .unwrap();
    let definition = CollectionDefinition {
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
    };
    let query = Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(parse_filter("title:folded ~ 'STRASSE'").unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    };
    for alphabet in [
        crate::workload::BigramAlphabet::Ascii,
        crate::workload::BigramAlphabet::Unicode,
    ] {
        let options = crate::repository::RepositoryOptions {
            bigram_alphabet: alphabet,
            snapshot_mode: crate::repository::SnapshotMode::IfValid,
            ..Default::default()
        };
        for opening in 0..2 {
            let repo =
                Repository::open_with_options(&root, vec![definition.clone()], options).unwrap();
            assert_eq!(repo.startup().restored, opening == 1);
            if opening == 0 {
                repo.save_default_snapshot_for_test();
            }
            assert_eq!(repo.collections[0].custom_text.len(), 1);
            let result = repo.search(&query).unwrap();
            assert_eq!(result.count, Some(2));
            assert_eq!(
                result
                    .hits
                    .iter()
                    .map(|hit| hit.record_id.as_str())
                    .collect::<Vec<_>>(),
                ["a", "b"]
            );
            assert_eq!(result.hits, repo.search_reference(&query).unwrap().hits);
            assert_eq!(result.counters.candidates, 2);
            assert_eq!(
                repo.explain(&query).unwrap()[0].terms[0].selected.kind,
                AccessPathKind::CustomBigram
            );
            for (filter, expected) in [
                ("title:lower ~ 'STRASSE'", 2),
                ("title:plain ~ 'STRASSE'", 1),
                ("title ~ 'STRASSE'", 1),
            ] {
                let mut request = query.clone();
                request.filter = Some(parse_filter(filter).unwrap());
                assert_eq!(repo.search(&request).unwrap().count, Some(expected));
                assert_eq!(
                    repo.search(&request).unwrap().hits,
                    repo.search_reference(&request).unwrap().hits
                );
                let plan = &repo.explain(&request).unwrap()[0].terms[0];
                assert!(
                    std::iter::once(&plan.selected)
                        .chain(plan.alternatives.iter())
                        .any(|path| matches!(
                            path.kind,
                            AccessPathKind::RawBigram | AccessPathKind::CustomBigram
                        )),
                    "{filter}"
                );
            }
            for selected in [false, true] {
                for (case_sensitivity, expected) in [
                    (crate::query::CaseSensitivity::Sensitive, 1),
                    (crate::query::CaseSensitivity::Insensitive, 2),
                ] {
                    let request = Query {
                        collections: vec!["data".into()],
                        fts: Some(FtsQuery {
                            pattern_parameter: None,
                            mode: crate::query::FtsMode::Literal,
                            case_sensitivity,
                            literal: "STRASSE".into(),
                            fields: selected
                                .then(|| FtsField {
                                    query_path: None,
                                    path: crate::model::FieldPath::parse("title").unwrap(),
                                    binding: None,
                                })
                                .into_iter()
                                .collect(),
                        }),
                        filter: None,
                        options: query.options.clone(),
                    };
                    let result = repo.search(&request).unwrap();
                    assert_eq!(result.count, Some(expected));
                    assert_eq!(result.hits, repo.search_reference(&request).unwrap().hits);
                    assert_eq!(result.counters.candidates, 2);
                }
            }
            repo.configure_verified_cache(1 << 20).unwrap();
            for (literal, case_sensitivity, expected) in [
                ("STRASSE", crate::query::CaseSensitivity::Sensitive, 1),
                ("strasse", crate::query::CaseSensitivity::Insensitive, 2),
                ("Straße", crate::query::CaseSensitivity::Sensitive, 1),
                ("strasse", crate::query::CaseSensitivity::Sensitive, 0),
                ("STRASSE", crate::query::CaseSensitivity::Insensitive, 2),
            ] {
                let mut request = query.clone();
                request.filter = None;
                request.fts = Some(FtsQuery {
                    pattern_parameter: None,
                    mode: crate::query::FtsMode::Literal,
                    case_sensitivity,
                    literal: literal.into(),
                    fields: vec![],
                });
                request.options.limit = 1;
                request.options.offset = usize::from(expected == 2);
                request.options.return_mode = ReturnMode::Bytes;
                let first = repo.search(&request).unwrap();
                let warm = repo.search(&request).unwrap();
                let reference = repo.search_reference(&request).unwrap();
                assert_eq!(first.count, Some(expected));
                assert_eq!(first.hits, reference.hits);
                assert_eq!(warm.hits, reference.hits);
                assert!(warm.counters.verified_result_hit);
                request.options.return_mode = ReturnMode::Logical;
                assert_eq!(
                    repo.search(&request).unwrap().hits,
                    repo.search_reference(&request).unwrap().hits
                );
            }
            let mut combined = query.clone();
            combined.fts = Some(FtsQuery {
                pattern_parameter: None,
                mode: crate::query::FtsMode::Literal,
                case_sensitivity: crate::query::CaseSensitivity::Insensitive,
                literal: "strasse".into(),
                fields: vec![],
            });
            combined.filter = Some(parse_filter("title = 'Straße Almanach'").unwrap());
            assert_eq!(repo.search(&combined).unwrap().count, Some(1));
            assert_eq!(repo.collections[0].custom_text.len(), 1);
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}


#[derive(Debug)]
struct SeparatorOperation {
    preparations: Arc<AtomicUsize>,
    callbacks: Arc<AtomicUsize>,
    mappings: Arc<AtomicUsize>,
    posting: bool,
    bigram: bool,
    complete: bool,
}


fn normalized_equal(input: &[u8], needle: &[u8]) -> bool {
    let mut mapped = input
        .iter()
        .copied()
        .filter(|byte| !matches!(byte, b' ' | b'-' | b'_'))
        .map(|byte| byte.to_ascii_lowercase());
    needle
        .iter()
        .all(|expected| mapped.next() == Some(*expected))
        && mapped.next().is_none()
}

fn normalized_contains(input: &[u8], needle: &[u8]) -> bool {
    (0..=input.len()).any(|start| {
        let mut mapped = input[start..]
            .iter()
            .copied()
            .filter(|byte| !matches!(byte, b' ' | b'-' | b'_'))
            .map(|byte| byte.to_ascii_lowercase());
        needle
            .iter()
            .all(|expected| mapped.next() == Some(*expected))
    })
}


#[derive(Debug)]
struct SeparatorFilter {
    needle: Vec<u8>,
    operator: Operator,
    callbacks: Arc<AtomicUsize>,
}


impl crate::reference::PreparedFilterMatcher for SeparatorFilter {
    fn input(&self) -> crate::reference::ExactInput {
        crate::reference::ExactInput::OriginalRecordBytes
    }

    fn matches(&self, _: Option<&Value>, bytes: Option<&[u8]>) -> Result<bool, String> {
        self.callbacks.fetch_add(1, AtomicOrdering::Relaxed);
        if self.needle == b"fail" {
            return Err("prepared separator matcher failure".into());
        }
        let bytes = bytes.ok_or("separator matcher requires original bytes")?;
        let title = bytes
            .split(|byte| *byte == b'|')
            .next()
            .ok_or("missing title")?;
        Ok(match self.operator {
            Operator::Eq => normalized_equal(title, &self.needle),
            Operator::Contains => normalized_contains(title, &self.needle),
            _ => return Err("unsupported separator operator".into()),
        })
    }

    fn posting_compatible(&self) -> bool {
        true
    }

    fn bigram_compatible(&self) -> bool {
        true
    }
}


#[derive(Debug)]
struct SeparatorText {
    needle: Vec<u8>,
    callbacks: Arc<AtomicUsize>,
}


impl crate::reference::PreparedTextMatcher for SeparatorText {
    fn matches(&self, text: &str) -> Result<bool, String> {
        self.callbacks.fetch_add(1, AtomicOrdering::Relaxed);
        Ok(normalized_contains(text.as_bytes(), &self.needle))
    }

    fn bigram_compatible(&self) -> bool {
        true
    }
}


impl crate::reference::BindingOperation for SeparatorOperation {
    fn accepts(&self, kind: &LogicalType, operator: Operator, _: bool) -> bool {
        *kind == LogicalType::String && matches!(operator, Operator::Eq | Operator::Contains)
    }

    fn prepare_literal(
        &self,
        literal: &crate::query::Literal,
        _: &LogicalType,
    ) -> Result<Value, String> {
        self.preparations.fetch_add(1, AtomicOrdering::Relaxed);
        let crate::query::Literal::String(text) = literal else {
            return Err("expected separator text".into());
        };
        let value = text
            .strip_prefix("sep:")
            .ok_or("separator literal needs sep: prefix")?;
        Ok(Value::String(self.map_text(value)))
    }

    fn prepare_fts_literal(&self, text: &str) -> Result<String, String> {
        self.preparations.fetch_add(1, AtomicOrdering::Relaxed);
        Ok(self.map_text(text))
    }

    fn prepare_filter_matcher(
        &self,
        literal: &Value,
        operator: Operator,
    ) -> Result<Option<Arc<dyn crate::reference::PreparedFilterMatcher>>, String> {
        self.preparations.fetch_add(1, AtomicOrdering::Relaxed);
        let Value::String(text) = literal else {
            return Err("expected prepared separator text".into());
        };
        Ok(Some(Arc::new(SeparatorFilter {
            needle: text.as_bytes().to_vec(),
            operator,
            callbacks: self.callbacks.clone(),
        })))
    }

    fn prepare_text_matcher(
        &self,
        literal: &str,
    ) -> Result<Option<Arc<dyn crate::reference::PreparedTextMatcher>>, String> {
        self.preparations.fetch_add(1, AtomicOrdering::Relaxed);
        Ok(Some(Arc::new(SeparatorText {
            needle: literal.as_bytes().to_vec(),
            callbacks: self.callbacks.clone(),
        })))
    }

    fn map_value(&self, value: &Value) -> Option<Value> {
        let Value::String(text) = value else {
            return None;
        };
        Some(Value::String(self.map_text(text)))
    }

    fn map_text(&self, text: &str) -> String {
        self.mappings.fetch_add(1, AtomicOrdering::Relaxed);
        text.bytes()
            .filter(|byte| !matches!(byte, b' ' | b'-' | b'_'))
            .map(|byte| byte.to_ascii_lowercase() as char)
            .collect()
    }

    fn posting(&self) -> bool {
        self.posting
    }

    fn bigram(&self) -> bool {
        self.bigram
    }

    fn complete_coverage(&self) -> bool {
        self.complete
    }
}


struct SeparatorReader(Arc<AtomicUsize>);


impl crate::source::SourceReader for SeparatorReader {
    fn name(&self) -> &'static str {
        "separator"
    }

    fn logical_text_coverage(&self) -> crate::source::LogicalTextCoverage {
        crate::source::LogicalTextCoverage::AllStringFields
    }

    fn frame(&self, file: &[u8]) -> Result<Vec<crate::inventory::ByteRange>, String> {
        Ok(vec![crate::inventory::ByteRange::checked(
            0,
            file.len() as u64,
            file.len() as u64,
        )?])
    }

    fn parse(&self, bytes: &[u8]) -> Result<crate::source::ExtractedRecord, String> {
        self.0.fetch_add(1, AtomicOrdering::Relaxed);
        let text = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
        let (title, body) = text.split_once('|').ok_or("missing separator")?;
        Ok(crate::source::ExtractedRecord {
            value: Value::Object(std::collections::BTreeMap::from([
                ("title".into(), Value::String(title.into())),
                ("body".into(), Value::String(body.into())),
            ])),
            text: vec![
                crate::source::TextSegment {
                    path: vec!["title".into()],
                    text: title.into(),
                },
                crate::source::TextSegment {
                    path: vec!["body".into()],
                    text: body.into(),
                },
            ],
        })
    }

    fn parse_outcome(
        &self,
        bytes: &[u8],
    ) -> Result<crate::source::ExtractedRecord, crate::source::ReaderFailure> {
        self.parse(bytes).map_err(|message| {
            crate::source::ReaderFailure::Malformed(crate::source::ReaderDiagnostic::new(
                crate::source::DiagnosticCategory::Syntax,
                "separator.parse",
                message,
            ))
        })
    }
}


#[test]
fn prepared_separator_matcher_uses_index_and_borrowed_scan_paths() {
    use crate::query::{FtsField, FtsQuery};
    use crate::reference::{FieldDefinition, RegisteredBinding, Schema};
    use crate::repository::{RepositoryOptions, SnapshotMode};
    use crate::source::{SourceFormat, SourceRegistry};

    let root =
        std::env::temp_dir().join(format!("objekte-separator-matcher-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.sep"), b"A-l_pha|body").unwrap();
    std::fs::write(root.join("data/b.sep"), b"beta|body").unwrap();
    // The raw bytes look like a matching custom literal, but this record has
    // no logical title. Partial custom coverage must scan eligible records
    // without treating the fallback slot as an exact or complementary hit.
    std::fs::write(root.join("data/broken.sep"), b"sep:alpha").unwrap();
    for n in 0..30 {
        std::fs::write(
            root.join(format!("data/z{n:02}.sep")),
            format!("other{n:02}|body"),
        )
        .unwrap();
    }
    let preparations = Arc::new(AtomicUsize::new(0));
    let callbacks = Arc::new(AtomicUsize::new(0));
    let mappings = Arc::new(AtomicUsize::new(0));
    let parses = Arc::new(AtomicUsize::new(0));
    let definition = |posting, bigram, complete| {
        let mut schema = Schema::default();
        schema
            .register_field(FieldDefinition {
                path: vec!["title".into()],
                kind: LogicalType::String,
                required: true,
                nullable: false,
                indexed: false,
                multi: false,
                element_nullable: vec![],
            })
            .unwrap();
        let binding = RegisteredBinding {
            name: "sep".into(),
            version: 1,
            operation: Arc::new(SeparatorOperation {
                preparations: preparations.clone(),
                callbacks: callbacks.clone(),
                mappings: mappings.clone(),
                posting,
                bigram,
                complete,
            }),
        };
        schema
            .register_operation(vec!["title".into()], binding)
            .unwrap();
        schema
            .set_default_binding(vec!["title".into()], "sep")
            .unwrap();
        CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::registered("sep"),
            identity: IdentityPolicy::Filename,
            discovery: Default::default(),
            schema,
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }
    };
    let sources = || {
        let mut registry = SourceRegistry::builtin();
        registry
            .register("sep", 1, "sep", Arc::new(SeparatorReader(parses.clone())))
            .unwrap();
        registry
    };
    let filter_query = |filter: &str| Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(parse_filter(filter).unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    };
    let fts_query = |named: bool| Query {
        collections: vec!["data".into()],
        filter: None,
        fts: Some(FtsQuery {
            pattern_parameter: None,
            mode: crate::query::FtsMode::Literal,
            case_sensitivity: crate::query::CaseSensitivity::Sensitive,
            literal: "alp".into(),
            fields: vec![FtsField {
                query_path: None,
                path: crate::model::FieldPath::parse("title").unwrap(),
                binding: named.then(|| "sep".into()),
            }],
        }),
        options: QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    };
    for alphabet in [
        crate::workload::BigramAlphabet::Ascii,
        crate::workload::BigramAlphabet::Unicode,
    ] {
        for (posting, bigram, complete) in [
            (true, true, true),
            (false, true, true),
            (false, false, true),
            (true, true, false),
        ] {
            let repo = Repository::open_with_sources(
                &root,
                vec![definition(posting, bigram, complete)],
                RepositoryOptions {
                    bigram_alphabet: alphabet,
                    snapshot_mode: SnapshotMode::Disabled,
                    ..Default::default()
                },
                sources(),
            )
            .unwrap();
            assert_eq!(repo.build_failed_parses(), 1);
            let mut unsupported = fts_query(true);
            unsupported.fts.as_mut().unwrap().case_sensitivity =
                crate::query::CaseSensitivity::Insensitive;
            assert!(
                repo.search(&unsupported)
                    .unwrap_err()
                    .contains("does not support insensitive mode")
            );
            assert_eq!(
                repo.collections()[0]
                    .generation
                    .records
                    .iter()
                    .filter(|record| record.class == crate::inventory::RecordClass::RawFallback)
                    .count(),
                1
            );
            for filter in [
                "title = 'sep:alpha'",
                "title:sep = 'sep:alpha'",
                "title:sep ~ 'sep:alp'",
                "title:sep ~ 'sep:al'",
            ] {
                let query = filter_query(filter);
                let before_preparations = preparations.load(AtomicOrdering::Relaxed);
                let before_callbacks = callbacks.load(AtomicOrdering::Relaxed);
                let before_parses = parses.load(AtomicOrdering::Relaxed);
                let before_mappings = mappings.load(AtomicOrdering::Relaxed);
                let result = repo.search(&query).unwrap();
                assert_eq!(
                    result.count,
                    Some(1),
                    "{alphabet:?} {posting} {bigram} {complete} {filter}"
                );
                assert_eq!(result.hits[0].record_id, "a");
                assert_eq!(result.hits[0].matched_fields, ["title"]);
                let selected = repo.explain(&query).unwrap()[0].terms[0].selected.kind;
                if posting && complete && filter.contains(" = ") {
                    assert_eq!(selected, AccessPathKind::Posting);
                } else if bigram && complete && filter.contains("sep:alp") {
                    assert_eq!(selected, AccessPathKind::CustomBigram);
                } else if !bigram || !complete {
                    assert_eq!(selected, AccessPathKind::Scanner);
                }
                assert_eq!(
                    preparations.load(AtomicOrdering::Relaxed) - before_preparations,
                    2
                );
                assert_eq!(mappings.load(AtomicOrdering::Relaxed) - before_mappings, 1);
                if result.counters.record_parses == 0 {
                    assert_eq!(parses.load(AtomicOrdering::Relaxed), before_parses);
                }
                let callback_delta = callbacks.load(AtomicOrdering::Relaxed) - before_callbacks;
                if filter.contains("~") || !posting || !complete {
                    assert!(callback_delta > 0, "prepared callback unused: {filter}");
                    assert_eq!(
                        result.counters.record_parses, 0,
                        "borrowed byte scan decoded: {filter}"
                    );
                } else {
                    assert_eq!(callback_delta, 0, "exact posting still scanned: {filter}");
                    assert_eq!(result.counters.files_opened, 0);
                }
                let second = repo.search(&query).unwrap();
                assert_eq!(second.hits, result.hits);
                assert_eq!(
                    preparations.load(AtomicOrdering::Relaxed) - before_preparations,
                    2
                );
            }
            for named in [false, true] {
                let before_callbacks = callbacks.load(AtomicOrdering::Relaxed);
                let result = repo.search(&fts_query(named)).unwrap();
                assert_eq!(result.count, Some(1));
                assert_eq!(result.hits[0].record_id, "a");
                assert!(callbacks.load(AtomicOrdering::Relaxed) > before_callbacks);
            }
            if !bigram && !posting {
                let error = repo
                    .search(&filter_query("title:sep ~ 'sep:fail'"))
                    .unwrap_err();
                assert!(
                    error.contains("prepared separator matcher failure"),
                    "{error}"
                );
            }
            if bigram && complete {
                let mut logical = filter_query("title:sep ~ 'sep:alp'");
                logical.options.return_mode = ReturnMode::Logical;
                let result = repo.search(&logical).unwrap();
                assert_eq!(result.counters.record_parses, 1);
                assert!(matches!(&result.hits[0].payload,
                    Some(crate::repository::Payload::Logical(Value::Object(fields)))
                    if fields.get("title") == Some(&Value::String("A-l_pha".into()))));
            }
        }
    }
    let options = RepositoryOptions {
        snapshot_mode: SnapshotMode::IfValid,
        ..Default::default()
    };
    let built = Repository::open_with_sources(
        &root,
        vec![definition(true, true, true)],
        options,
        sources(),
    )
    .unwrap();
    assert!(!built.startup().restored);
    built.save_default_snapshot_for_test();
    drop(built);
    let restored = Repository::open_with_sources(
        &root,
        vec![definition(true, true, true)],
        options,
        sources(),
    )
    .unwrap();
    assert!(restored.startup().restored);
    assert_eq!(
        restored
            .search(&filter_query("title:sep ~ 'sep:alp'"))
            .unwrap()
            .hits[0]
            .record_id,
        "a"
    );
    std::fs::remove_dir_all(root).unwrap();
}


#[derive(Debug)]
struct RemoveSpaces;


impl crate::reference::BindingOperation for RemoveSpaces {
    fn accepts(&self, kind: &LogicalType, op: Operator, _: bool) -> bool {
        *kind == LogicalType::String && matches!(op, Operator::Contains | Operator::Eq)
    }

    fn prepare_literal(
        &self,
        literal: &crate::query::Literal,
        _: &LogicalType,
    ) -> Result<Value, String> {
        match literal {
            crate::query::Literal::String(text) => Ok(Value::String(self.map_text(text))),
            _ => Err("expected text".into()),
        }
    }

    fn map_value(&self, value: &Value) -> Option<Value> {
        match value {
            Value::String(text) => Some(Value::String(self.map_text(text))),
            _ => None,
        }
    }

    fn map_text(&self, text: &str) -> String {
        text.chars().filter(|ch| *ch != ' ').collect()
    }

    fn bigram(&self) -> bool {
        true
    }

    fn value_text_equivalent(&self) -> bool {
        true
    }
}


#[test]
fn transformed_raw_index_cannot_prune_identity_field_predicates() {
    use crate::reference::{RegisteredBinding, Schema};
    use crate::repository::{RepositoryOptions, SnapshotMode};
    let root = std::env::temp_dir().join(format!("objekte-raw-compat-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"), r#"{"title":"hello world"}"#).unwrap();
    std::fs::write(root.join("data/b.json"), r#"{"title":"other"}"#).unwrap();
    let definition = || {
        let mut schema = Schema::default();
        schema
            .register_field(FieldDefinition {
                path: vec!["title".into()],
                kind: LogicalType::String,
                required: true,
                nullable: false,
                indexed: false,
                multi: false,
                element_nullable: vec![],
            })
            .unwrap();
        schema
            .set_raw_binding(RegisteredBinding {
                name: "nospace".into(),
                version: 1,
                operation: Arc::new(RemoveSpaces),
            })
            .unwrap();
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
    for alphabet in [
        crate::workload::BigramAlphabet::Ascii,
        crate::workload::BigramAlphabet::Unicode,
    ] {
        let options = RepositoryOptions {
            bigram_alphabet: alphabet,
            snapshot_mode: SnapshotMode::IfValid,
            ..Default::default()
        };
        for open in 0..2 {
            let repo = Repository::open_with_options(&root, vec![definition()], options).unwrap();
            assert_eq!(repo.startup().restored, open == 1);
            if open == 0 {
                repo.save_default_snapshot_for_test();
            }
            for filter in [
                Some("title ~ 'hello world'"),
                Some("title = 'hello world'"),
                None,
            ] {
                let query = Query {
                    collections: vec!["data".into()],
                    filter: filter.map(|s| parse_filter(s).unwrap()),
                    fts: filter.is_none().then(|| FtsQuery {
                        pattern_parameter: None,
                        mode: crate::query::FtsMode::Literal,
                        case_sensitivity: crate::query::CaseSensitivity::Sensitive,
                        literal: "hello world".into(),
                        fields: vec![FtsField {
                            query_path: None,
                            path: crate::model::FieldPath::parse("title").unwrap(),
                            binding: None,
                        }],
                    }),
                    options: QueryOptions {
                        sort: vec![],
                        limit: 10,
                        offset: 0,
                        count: true,
                        return_mode: ReturnMode::Locator,
                        evidence: true,
                    },
                };
                let result = repo.search(&query).unwrap();
                assert_eq!(result.count, Some(1), "{alphabet:?} {filter:?}");
                assert_eq!(result.hits[0].record_id, "a");
                if filter == Some("title ~ 'hello world'") {
                    let plan = repo.explain(&query).unwrap();
                    assert_ne!(plan[0].terms[0].selected.kind, AccessPathKind::RawBigram);
                    assert!(plan[0].terms[0].reason.contains("raw bigram rejected"));
                }
            }
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}


#[derive(Debug)]
struct KeyedOperation {
    posting: bool,
    bigram: bool,
    complete: bool,
}


#[test]
fn malformed_raw_text_uses_original_literal_beside_transformed_valid_text() {
    use crate::query::FtsQuery;
    use crate::reference::RegisteredBinding;
    use crate::repository::{RepositoryOptions, SnapshotMode};
    let root = std::env::temp_dir().join(format!(
        "objekte-fallback-raw-binding-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"), br#"{"title":"HELLO"}"#).unwrap();
    std::fs::write(root.join("data/b.json"), br#"{"title":"HELLO""#).unwrap();
    let mut schema = crate::reference::Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["title".into()],
            kind: LogicalType::String,
            required: false,
            nullable: false,
            indexed: false,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    schema
        .set_raw_binding(RegisteredBinding {
            name: "keyed".into(),
            version: 1,
            operation: Arc::new(KeyedOperation {
                posting: false,
                bigram: true,
                complete: true,
            }),
        })
        .unwrap();
    let definition = CollectionDefinition {
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
    };
    for alphabet in [
        crate::workload::BigramAlphabet::Ascii,
        crate::workload::BigramAlphabet::Unicode,
    ] {
        let options = RepositoryOptions {
            bigram_alphabet: alphabet,
            snapshot_mode: SnapshotMode::IfValid,
            ..Default::default()
        };
        let built =
            Repository::open_with_options(&root, vec![definition.clone()], options).unwrap();
        for (literal, case_sensitivity, expected) in [
            ("HELLO", crate::query::CaseSensitivity::Sensitive, 2),
            ("hello", crate::query::CaseSensitivity::Sensitive, 1),
            ("ell", crate::query::CaseSensitivity::Sensitive, 1),
            ("hello", crate::query::CaseSensitivity::Insensitive, 2),
            ("ell", crate::query::CaseSensitivity::Insensitive, 2),
        ] {
            let query = Query {
                collections: vec!["data".into()],
                filter: None,
                fts: Some(FtsQuery {
                    pattern_parameter: None,
                    mode: crate::query::FtsMode::Literal,
                    case_sensitivity,
                    literal: literal.into(),
                    fields: vec![],
                }),
                options: QueryOptions {
                    sort: vec![],
                    limit: 10,
                    offset: 0,
                    count: true,
                    return_mode: ReturnMode::Locator,
                    evidence: true,
                },
            };
            let actual = built.search(&query).unwrap();
            assert_eq!(actual.count, Some(expected), "{literal}");
            assert_eq!(
                actual.hits,
                built.search_reference(&query).unwrap().hits,
                "{literal}"
            );
            if literal == "HELLO" {
                assert!(
                    actual
                        .hits
                        .iter()
                        .any(|hit| hit.matched_fields == ["@record.raw_text"])
                );
            }
            built.save_default_snapshot_for_test();
            let restored =
                Repository::open_with_options(&root, vec![definition.clone()], options).unwrap();
            assert!(restored.startup().restored);
            assert_eq!(restored.search(&query).unwrap().hits, actual.hits);
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}


impl crate::reference::BindingOperation for KeyedOperation {
    fn accepts(&self, kind: &LogicalType, operator: Operator, _multi: bool) -> bool {
        *kind == LogicalType::String
            && matches!(operator, Operator::Eq | Operator::Ne | Operator::Contains)
    }

    fn prepare_literal(
        &self,
        literal: &crate::query::Literal,
        _kind: &LogicalType,
    ) -> Result<Value, String> {
        match literal {
            crate::query::Literal::Null => Ok(Value::Null),
            crate::query::Literal::String(text) => text
                .strip_prefix("key:")
                .map(|body| Value::String(body.trim().to_lowercase()))
                .ok_or("keyed literal needs key: prefix".into()),
            _ => Err("keyed literal requires text".into()),
        }
    }

    fn map_value(&self, value: &Value) -> Option<Value> {
        match value {
            Value::String(text) => Some(Value::String(self.map_text(text))),
            Value::Null => Some(Value::Null),
            _ => None,
        }
    }

    fn map_text(&self, text: &str) -> String {
        text.trim().to_lowercase()
    }

    fn value_text_equivalent(&self) -> bool {
        true
    }

    fn posting(&self) -> bool {
        self.posting
    }

    fn bigram(&self) -> bool {
        self.bigram
    }

    fn complete_coverage(&self) -> bool {
        self.complete
    }
}


#[test]
fn registered_binding_has_identical_filter_fts_and_restore_semantics() {
    use crate::query::{FtsField, Literal};
    use crate::reference::RegisteredBinding;
    use crate::repository::{RepositoryOptions, SnapshotMode};

    let root = std::env::temp_dir().join(format!("objekte-keyed-binding-{}", std::process::id()));
    let path = root.join("data");
    std::fs::create_dir_all(&path).unwrap();
    for (id, json) in [
        ("a", r#"{"title":"  ALPHA  ","text":"NEEDLE Place"}"#),
        ("b", r#"{"title":"alpha","text":"other"}"#),
        ("c", r#"{"title":"beta","text":"needle!"}"#),
        ("d", r#"{"text":"other"}"#),
        ("e", r#"{"title":"","text":"other"}"#),
    ] {
        std::fs::write(path.join(format!("{id}.json")), json).unwrap();
    }
    let definition = |posting, bigram, complete, version| {
        let mut schema = crate::reference::Schema::default();
        schema
            .register_field(FieldDefinition {
                path: vec!["title".into()],
                kind: LogicalType::String,
                required: false,
                nullable: true,
                indexed: false,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
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
        let binding = RegisteredBinding {
            name: "keyed".into(),
            version,
            operation: Arc::new(KeyedOperation {
                posting,
                bigram,
                complete,
            }),
        };
        schema
            .register_operation(vec!["title".into()], binding.clone())
            .unwrap();
        schema
            .set_default_binding(vec!["title".into()], "keyed")
            .unwrap();
        schema.set_raw_binding(binding).unwrap();
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
    let make_query = |filter: Option<&str>, fts: Option<(&str, Option<&str>)>| Query {
        collections: vec![],
        fts: fts.map(|(literal, selected)| FtsQuery {
            pattern_parameter: None,
            mode: crate::query::FtsMode::Literal,
            case_sensitivity: crate::query::CaseSensitivity::Sensitive,
            literal: literal.into(),
            fields: selected
                .map(|name| {
                    vec![FtsField {
                        query_path: None,
                        path: crate::model::FieldPath::parse("title").unwrap(),
                        binding: Some(name.into()),
                    }]
                })
                .unwrap_or_default(),
        }),
        filter: filter.map(|text| parse_filter(text).unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 20,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    };
    let mut default_fts = make_query(None, Some(("alp", Some("keyed"))));
    default_fts.fts.as_mut().unwrap().fields[0].binding = None;
    let cases = [
        (
            make_query(Some("title:keyed = 'key:ALPHA'"), None),
            vec!["a", "b"],
            vec!["title"],
        ),
        (
            make_query(Some("title = 'key:ALPHA'"), None),
            vec!["a", "b"],
            vec!["title"],
        ),
        (
            make_query(Some("title:keyed != 'key:ALPHA'"), None),
            vec!["c", "e"],
            vec![],
        ),
        (
            make_query(Some("title:keyed ~ 'key:alp'"), None),
            vec!["a", "b"],
            vec!["title"],
        ),
        (
            make_query(Some("title:keyed ~ 'key:al'"), None),
            vec!["a", "b"],
            vec!["title"],
        ),
        (
            make_query(Some("title:keyed ~ 'key:'"), None),
            vec!["a", "b", "c", "e"],
            vec!["title"],
        ),
        (
            make_query(None, Some(("alp", Some("keyed")))),
            vec!["a", "b"],
            vec!["title"],
        ),
        (default_fts, vec!["a", "b"], vec!["title"]),
        (
            make_query(None, Some(("", Some("keyed")))),
            vec!["a", "b", "c", "e"],
            vec!["title"],
        ),
        (
            make_query(None, Some(("needle", None))),
            vec!["a", "c"],
            vec!["text"],
        ),
    ];
    for (posting, bigram, complete) in [
        (true, true, true),
        (false, true, true),
        (false, false, true),
        (true, true, false),
    ] {
        let repo = Repository::open_with_options(
            &root,
            vec![definition(posting, bigram, complete, 1)],
            RepositoryOptions {
                snapshot_mode: SnapshotMode::Disabled,
                ..Default::default()
            },
        )
        .unwrap();
        if posting && complete {
            let mut request = crate::posting_values::PostingValuesRequest::new("data", "title", 10);
            request.prefix = Some("alp".into());
            assert!(
                repo.posting_values(&request)
                    .unwrap_err()
                    .contains("does not preserve prefix")
            );
        }
        for (case_index, (query, expected, evidence)) in cases.iter().enumerate() {
            let result = repo.search(query).unwrap();
            let reference = repo.search_reference(query).unwrap();
            let mut ids = result
                .hits
                .iter()
                .map(|hit| hit.record_id.as_str())
                .collect::<Vec<_>>();
            ids.sort_unstable();
            assert_eq!(
                &ids, expected,
                "posting={posting}, bigram={bigram}, complete={complete}"
            );
            assert_eq!(result.count.unwrap(), reference.count.unwrap());
            assert_eq!(result.hits, reference.hits);
            assert!(
                result
                    .hits
                    .iter()
                    .all(|hit| hit.matched_fields == *evidence)
            );
            if case_index == 0 && posting && complete {
                assert_eq!(result.counters.files_opened, 0);
            }
            if case_index == 6 && bigram && complete {
                assert!(result.counters.candidates < 5);
            }
        }
        let invalid = make_query(Some("title:keyed > 'key:alpha'"), None);
        assert!(
            repo.search(&invalid)
                .unwrap_err()
                .contains("unsupported operation")
        );
        let invalid = make_query(Some("title:keyed = 'alpha'"), None);
        assert!(repo.search(&invalid).unwrap_err().contains("key: prefix"));
        let invalid = make_query(Some("title:unknown = 'key:alpha'"), None);
        assert!(
            repo.search(&invalid)
                .unwrap_err()
                .contains("unknown binding")
        );
        let invalid = make_query(None, Some(("alpha", Some("unknown"))));
        assert!(
            repo.search(&invalid)
                .unwrap_err()
                .contains("unknown FTS binding")
        );
        let _ = Literal::Null;
    }
    let options = RepositoryOptions {
        snapshot_mode: SnapshotMode::IfValid,
        ..Default::default()
    };
    let built =
        Repository::open_with_options(&root, vec![definition(true, true, true, 1)], options)
            .unwrap();
    assert!(!built.startup().restored);
    built.save_default_snapshot_for_test();
    drop(built);
    let restored =
        Repository::open_with_options(&root, vec![definition(true, true, true, 1)], options)
            .unwrap();
    assert!(restored.startup().restored);
    for (query, expected, _) in &cases {
        let mut ids = restored
            .search(query)
            .unwrap()
            .hits
            .into_iter()
            .map(|hit| hit.record_id)
            .collect::<Vec<_>>();
        ids.sort_unstable();
        assert_eq!(
            ids.iter().map(String::as_str).collect::<Vec<_>>(),
            *expected
        );
    }
    drop(restored);
    let changed =
        Repository::open_with_options(&root, vec![definition(true, true, true, 2)], options)
            .unwrap();
    assert!(!changed.startup().restored);
    std::fs::remove_dir_all(root).unwrap();
}

fn plan_expr(
    repo: &Repository,
    collection: &CollectionState,
    expr: &BoundExpr,
    domain: &RoaringBitmap,
) -> Result<Plan, String> {
    plan_expr_restricted(repo, collection, expr, domain, domain, true)
}

#[test]
fn record_id_lookup_respects_case_fold_binding() {
    let root = std::env::temp_dir().join(format!("objekte-id-fold-{}", std::process::id()));
    let path = root.join("data");
    std::fs::create_dir_all(&path).unwrap();
    for id in ["ABC", "abc", "Straße", "STRASSE"] {
        std::fs::write(path.join(format!("{id}.json")), "{}").unwrap();
    }
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::Json,
            identity: IdentityPolicy::Filename,
            discovery: Default::default(),
            schema: Default::default(),
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
    for (filter, expected) in [
        ("@record.id = 'abc'", vec!["abc"]),
        ("@record.id = 'ABC'", vec!["ABC"]),
        ("@record.id:lower = 'abc'", vec!["ABC", "abc"]),
        ("@record.id:lower = 'STRASSE'", vec!["STRASSE", "Straße"]),
    ] {
        let query = Query {
            collections: vec![],
            fts: None,
            filter: Some(parse_filter(filter).unwrap()),
            options: QueryOptions {
                sort: vec![],
                limit: 20,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence: false,
            },
        };
        let optimized = repo.search(&query).unwrap();
        let reference = repo.search_reference(&query).unwrap();
        let mut actual = optimized
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>();
        actual.sort_unstable();
        assert_eq!(actual, expected, "{filter}");
        assert_eq!(
            optimized.count.unwrap(),
            reference.count.unwrap(),
            "{filter}"
        );
        assert_eq!(optimized.hits, reference.hits, "{filter}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn indexed_restriction_precedes_written_first_scan() {
    let root = std::env::temp_dir().join(format!("objekte-executor-{}", std::process::id()));
    let path = root.join("data");
    std::fs::create_dir_all(&path).unwrap();
    for (id, kind, text) in [
        ("a", "wanted", "needle"),
        ("b", "other", "needle"),
        ("c", "other", "nope"),
    ] {
        std::fs::write(
            path.join(format!("{id}.json")),
            serde_json::json!({"kind":kind,"text":text}).to_string(),
        )
        .unwrap();
    }
    let mut schema = crate::reference::Schema::default();
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
    schema
        .register_custom(
            vec!["text".into()],
            CustomBinding {
                name: "folded".into(),
                version: 1,
                transform: Transform::CaseFold,
            },
        )
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
    let domain: RoaringBitmap = (0..3).collect();
    let bound = crate::reference::bind_filter(
        &parse_filter("kind = 'wanted'").unwrap(),
        &repo.collections[0].definition.schema,
    )
    .unwrap();
    let plan = plan_expr(&repo, &repo.collections[0], &bound, &domain).unwrap();
    assert_eq!(plan.choices[0].selected.kind, AccessPathKind::Posting);
    assert!(
        plan.choices[0]
            .alternatives
            .iter()
            .any(|p| p.kind == AccessPathKind::Scanner)
    );
    let bound = crate::reference::bind_filter(
        &parse_filter("kind ~ 'want'").unwrap(),
        &repo.collections[0].definition.schema,
    )
    .unwrap();
    let plan = plan_expr(&repo, &repo.collections[0], &bound, &domain).unwrap();
    assert_eq!(plan.choices[0].selected.kind, AccessPathKind::Posting);
    assert!(plan.exact);
    let options = QueryOptions {
        sort: vec![],
        limit: 50,
        offset: 0,
        count: true,
        return_mode: ReturnMode::Locator,
        evidence: true,
    };
    for expression in [
        "text ~ 'needle' && kind = 'wanted'",
        "kind = 'wanted' && text ~ 'needle'",
    ] {
        let query = Query {
            collections: vec![],
            fts: None,
            filter: Some(parse_filter(expression).unwrap()),
            options: options.clone(),
        };
        let optimized = repo.search(&query).unwrap();
        let reference = repo.search_reference(&query).unwrap();
        assert_eq!(optimized.count.unwrap(), reference.count.unwrap());
        assert_eq!(optimized.hits[0].record_id, "a");
        assert_eq!(optimized.counters.record_parses, 1);
    }
    let query = Query {
        collections: vec![],
        fts: None,
        filter: Some(parse_filter("text ~ 'needle' && kind = 'absent'").unwrap()),
        options,
    };
    let optimized = repo.search(&query).unwrap();
    assert_eq!(optimized.count.unwrap(), 0);
    assert_eq!(optimized.counters.files_opened, 0);
    let identity = Query {
        collections: vec![],
        fts: None,
        filter: Some(parse_filter("@record.id = 'b'").unwrap()),
        options: query.options.clone(),
    };
    let optimized = repo.search(&identity).unwrap();
    assert_eq!(optimized.count.unwrap(), 1);
    assert_eq!(optimized.hits[0].record_id, "b");
    assert_eq!(optimized.counters.files_opened, 0);
    let disjunction = Query {
        collections: vec![],
        fts: None,
        filter: Some(parse_filter("kind = 'wanted' || text ~ 'nope'").unwrap()),
        options: query.options.clone(),
    };
    let optimized = repo.search(&disjunction).unwrap();
    assert_eq!(optimized.count.unwrap(), 2);
    assert_eq!(
        optimized.count.unwrap(),
        repo.search_reference(&disjunction).unwrap().count.unwrap()
    );
    let folded = Query {
        collections: vec![],
        fts: Some(FtsQuery {
            pattern_parameter: None,
            mode: crate::query::FtsMode::Literal,
            case_sensitivity: crate::query::CaseSensitivity::Sensitive,
            literal: "NEEDLE".into(),
            fields: vec![FtsField {
                query_path: None,
                path: crate::model::FieldPath::parse("text").unwrap(),
                binding: Some("folded".into()),
            }],
        }),
        filter: None,
        options: query.options.clone(),
    };
    let optimized = repo.search(&folded).unwrap();
    assert_eq!(optimized.count.unwrap(), 2);
    assert_eq!(optimized.counters.record_parses, 2);
    assert_eq!(
        optimized.count.unwrap(),
        repo.search_reference(&folded).unwrap().count.unwrap()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rare_text_restriction_can_precede_a_broad_posting() {
    let root = std::env::temp_dir().join(format!("objekte-adapt-{}", std::process::id()));
    let path = root.join("data");
    std::fs::create_dir_all(&path).unwrap();
    for i in 0..240 {
        let text = if i == 173 {
            "uniqueneedleword"
        } else {
            "ordinary text"
        };
        std::fs::write(
            path.join(format!("{i:03}.json")),
            serde_json::json!({"kind":"broad","text":text}).to_string(),
        )
        .unwrap();
    }
    let mut schema = crate::reference::Schema::default();
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
    let domain: RoaringBitmap = (0..240).collect();
    let options = QueryOptions {
        sort: vec![],
        limit: 10,
        offset: 0,
        count: true,
        return_mode: ReturnMode::Locator,
        evidence: false,
    };
    for expression in [
        "kind = 'broad' && text ~ 'uniqueneedleword'",
        "text ~ 'uniqueneedleword' && kind = 'broad'",
    ] {
        let bound = crate::reference::bind_filter(
            &parse_filter(expression).unwrap(),
            &repo.collections[0].definition.schema,
        )
        .unwrap();
        let plan = plan_expr(&repo, &repo.collections[0], &bound, &domain).unwrap();
        assert_eq!(plan.choices[0].field, "text");
        assert_eq!(plan.choices[0].selected.kind, AccessPathKind::RawBigram);
        let query = Query {
            collections: vec![],
            fts: None,
            filter: Some(parse_filter(expression).unwrap()),
            options: options.clone(),
        };
        let optimized = repo.search(&query).unwrap();
        let reference = repo.search_reference(&query).unwrap();
        assert_eq!(optimized.count.unwrap(), 1);
        assert_eq!(optimized.hits[0].record_id, "173");
        assert_eq!(optimized.hits[0].record_id, reference.hits[0].record_id);
        assert_eq!(optimized.counters.record_parses, 1);
    }
    for expression in [
        "kind = 'absent' || text ~ 'uniqueneedleword'",
        "text ~ 'uniqueneedleword' || kind = 'absent'",
    ] {
        let query = Query {
            collections: vec![],
            fts: None,
            filter: Some(parse_filter(expression).unwrap()),
            options: options.clone(),
        };
        let optimized = repo.search(&query).unwrap();
        assert_eq!(
            optimized.count.unwrap(),
            repo.search_reference(&query).unwrap().count.unwrap()
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}
