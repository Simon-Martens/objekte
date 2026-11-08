use super::*;
use crate::model::LogicalType;
use crate::query::{QueryOptions, parse_filter};
use crate::reference::{FieldDefinition, Schema};
use crate::repository::{CollectionDefinition, IdentityPolicy};
use crate::source::SourceFormat;

#[test]
fn merged_segment_search_counts_filters_fts_and_pages_in_one_order() {
    use crate::build::{
        FilePreparationBinding, SegmentAdmission, SourceFileInput, prepare_file_build,
    };
    use crate::inventory::SourceRevision;
    use crate::model::{CollectionId, FileId};
    use crate::query::{CaseSensitivity, FtsMode, FtsQuery};
    use crate::repository::{RepositoryOptions, SnapshotMode};

    let root = std::env::temp_dir().join(format!(
        "objekte-segment-merge-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/base.jsonl"),
        b"{\"kind\":\"b\",\"title\":\"beta\"}\n{\"kind\":\"a\",\"title\":\"alpha\"}\n",
    )
    .unwrap();
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::JsonLines);
    for field in ["kind", "title"] {
        definition
            .schema
            .register_field(FieldDefinition {
                path: vec![field.into()],
                kind: LogicalType::String,
                required: true,
                nullable: false,
                indexed: true,
                multi: false,
                element_nullable: vec![],
            })
            .unwrap();
    }
    definition
        .sort_fields
        .push(FieldPath::parse("title").unwrap());
    let repo = Arc::new(
        Repository::open_with_options(
            &root,
            vec![definition],
            RepositoryOptions {
                snapshot_mode: SnapshotMode::Disabled,
                ..Default::default()
            },
        )
        .unwrap(),
    );
    let path = root.join("data/delta.jsonl");
    std::fs::write(
        &path,
        b"{\"kind\":\"a\",\"title\":\"aardvark\"}\n{\"kind\":\"b\",\"title\":\"bravo\"}\n{\"kind\":\"c\",\"title\":\"charlie\"}\n",
    )
    .unwrap();
    let collection = &repo.collections()[0];
    let prepared = prepare_file_build(
        &root,
        SourceFileInput {
            id: FileId(1),
            relative_path: "data/delta.jsonl".into(),
            revision: SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
            created_ns: None,
        },
        &[FilePreparationBinding {
            collection: CollectionId(0),
            definition: &collection.definition,
            source: &collection.source,
        }],
        repo.options.bigram_alphabet,
        repo.options.inference,
        1,
    )
    .unwrap();
    let mut admission = SegmentAdmission::new(CollectionId(0), &collection.source);
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
    let index = admission
        .seal()
        .into_indexed(
            &collection.definition,
            repo.options.bigram_alphabet,
            repo.options.inference,
            1,
            &pool,
        )
        .unwrap();
    let catalog = FileCatalog::from_repository(Arc::clone(&repo))
        .unwrap()
        .appended(index.files[0].entry.clone(), None, None)
        .unwrap();
    let segments = vec![Arc::new(vec![Arc::new(PublishedSegment {
        ordinal: 1,
        active: ActiveRecords::all(index.records.len()).unwrap(),
        index: Arc::new(index),
    })])];
    let base_active = vec![ActiveRecords::all(collection.generation.records.len()).unwrap()];
    let revision = IndexRevision(2);
    let mut query = Query {
        collections: vec!["data".into()],
        filter: Some(parse_filter("!(kind = 'b')").unwrap()),
        fts: None,
        options: QueryOptions {
            sort: vec![SortField::simple("title", false)],
            limit: 1,
            offset: 1,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let result =
        search_segmented(&repo, &query, &base_active, &segments, &catalog, revision).unwrap();
    assert_eq!(result.count, Some(3));
    assert_eq!(
        result.hits[0].file_path,
        std::path::Path::new("data/base.jsonl")
    );
    assert_eq!(result.hits[0].generation, revision.0);
    query.options.offset = 0;
    query.options.limit = 3;
    let all = search_segmented(&repo, &query, &base_active, &segments, &catalog, revision).unwrap();
    let reference = search_segmented_reference(
        &repo, &query, &base_active, &segments, &catalog, revision,
    )
    .unwrap();
    assert_eq!(all.count, reference.count);
    assert_eq!(all.hits, reference.hits);
    assert_eq!(
        all.hits
            .iter()
            .map(|hit| hit.file_path.as_path())
            .collect::<Vec<_>>(),
        [
            std::path::Path::new("data/delta.jsonl"),
            std::path::Path::new("data/base.jsonl"),
            std::path::Path::new("data/delta.jsonl"),
        ]
    );
    query.options.return_mode = ReturnMode::Logical;
    let logical =
        search_segmented(&repo, &query, &base_active, &segments, &catalog, revision).unwrap();
    assert!(
        logical
            .hits
            .iter()
            .all(|hit| matches!(hit.payload, Some(Payload::Logical(_))))
    );
    query.options.return_mode = ReturnMode::Locator;
    query.options.sort.clear();
    query.filter = Some(parse_filter("kind = 'a'").unwrap());
    query.options.limit = 0;
    let counted =
        search_segmented(&repo, &query, &base_active, &segments, &catalog, revision).unwrap();
    assert_eq!(counted.count, Some(2));
    assert_eq!(counted.counters.files_opened, 0);
    query.filter = None;
    query.fts = Some(FtsQuery {
        pattern_parameter: None,
        mode: FtsMode::Literal,
        case_sensitivity: CaseSensitivity::Sensitive,
        literal: "bravo".into(),
        fields: vec![],
    });
    query.options.limit = 10;
    let text =
        search_segmented(&repo, &query, &base_active, &segments, &catalog, revision).unwrap();
    let reference_text = search_segmented_reference(
        &repo, &query, &base_active, &segments, &catalog, revision,
    )
    .unwrap();
    assert_eq!(text.count, reference_text.count);
    assert_eq!(text.hits, reference_text.hits);
    assert_eq!(text.count, Some(1));
    assert_eq!(
        text.hits[0].file_path,
        std::path::Path::new("data/delta.jsonl")
    );
    query.fts = None;
    query.options.sort = vec![SortField::computed(
        crate::workload::SortExpression {
            path: "title".into(),
            binding: Some("lower".into()),
            reducer: None,
        },
        false,
    )];
    query.options.return_mode = ReturnMode::Logical;
    let computed =
        search_segmented(&repo, &query, &base_active, &segments, &catalog, revision).unwrap();
    assert_eq!(computed.count, Some(5));
    assert_eq!(
        computed
            .hits
            .iter()
            .map(|hit| match hit.payload.as_ref().unwrap() {
                Payload::Logical(Value::Object(fields)) => match &fields["title"] {
                    Value::String(title) => title.as_str(),
                    _ => panic!("title is not a string"),
                },
                _ => panic!("logical payload missing"),
            })
            .collect::<Vec<_>>(),
        ["aardvark", "alpha", "beta", "bravo", "charlie"]
    );
    let stream_context = ExecutionContext::new(
        crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
        ExecutionLimits::default(),
        Some(CancellationToken::new()),
    );
    let mut streamed = Vec::new();
    let outcome = search_stream_segmented_with_context(
        &repo,
        &query,
        ResultScope::All { offset: 0 },
        StreamOptions { batch_rows: 2, ..Default::default() },
        &stream_context,
        &base_active,
        &segments,
        &catalog,
        revision,
        &mut |event| {
            if let StreamEvent::OrderedRows { rows, .. } = event {
                streamed.extend_from_slice(rows);
            }
            Ok(StreamControl::Continue)
        },
    )
    .unwrap();
    let StreamOutcome::Complete(summary) = outcome else { panic!("stream incomplete") };
    assert_eq!(summary.count, Some(5));
    assert_eq!(summary.returned_rows, 5);
    assert_eq!(streamed, computed.hits);
    let page_context = ExecutionContext::new(
        crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
        ExecutionLimits::default(),
        Some(CancellationToken::new()),
    );
    let mut replacement = Vec::new();
    let page = search_stream_segmented_with_context(
        &repo, &query, ResultScope::Page { offset: 1, limit: 2 },
        StreamOptions { mode: crate::streaming::StreamMode::PageReplacements, ..Default::default() },
        &page_context, &base_active, &segments, &catalog, revision,
        &mut |event| {
            if let StreamEvent::PageReplacement { rows, .. } = event {
                replacement.extend_from_slice(rows);
            }
            Ok(StreamControl::Continue)
        },
    ).unwrap();
    let StreamOutcome::Complete(page) = page else { panic!("page stream incomplete") };
    assert_eq!(page.count, Some(5));
    assert_eq!(replacement, computed.hits[1..3]);
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let cancelled_context = ExecutionContext::new(
        crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
        ExecutionLimits::default(),
        Some(cancelled),
    );
    assert!(matches!(
        search_stream_segmented_with_context(
            &repo, &query, ResultScope::Page { offset: 0, limit: 1 },
            StreamOptions::default(), &cancelled_context, &base_active, &segments,
            &catalog, revision, &mut |_| Ok(StreamControl::Continue),
        ).unwrap(),
        StreamOutcome::Cancelled(_)
    ));
    let during_context = ExecutionContext::new(
        crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
        ExecutionLimits::default(),
        Some(CancellationToken::new()),
    );
    during_context.cancel_on_check_for_test(8);
    assert!(matches!(
        search_stream_segmented_with_context(
            &repo, &query, ResultScope::All { offset: 0 },
            StreamOptions::default(), &during_context, &base_active, &segments,
            &catalog, revision, &mut |_| Ok(StreamControl::Continue),
        ).unwrap(),
        StreamOutcome::Cancelled(_)
    ));
    let limited_context = ExecutionContext::new(
        crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
        ExecutionLimits { source_record_bytes: 8, ..Default::default() },
        Some(CancellationToken::new()),
    );
    assert!(matches!(
        search_stream_segmented_with_context(
            &repo, &query, ResultScope::Page { offset: 0, limit: 1 },
            StreamOptions::default(), &limited_context, &base_active, &segments,
            &catalog, revision, &mut |_| Ok(StreamControl::Continue),
        ),
        Err(StreamError::Resource { .. })
    ));
    query.options.return_mode = ReturnMode::Locator;
    query.options.sort.clear();
    let mut retired_base = base_active.clone();
    retired_base[0] = retired_base[0]
        .retired(&[crate::model::RecordSlot(0)])
        .unwrap();
    let mut retired_delta = (*segments[0][0]).clone();
    retired_delta.active = retired_delta
        .active
        .retired(&[crate::model::RecordSlot(1)])
        .unwrap();
    let retired_segments = vec![Arc::new(vec![Arc::new(retired_delta)])];
    let live = search_segmented(
        &repo,
        &query,
        &retired_base,
        &retired_segments,
        &catalog,
        IndexRevision(3),
    )
    .unwrap();
    assert_eq!(live.count, Some(3));
    assert!(live.hits.iter().all(
        |hit| hit.file_path != std::path::Path::new("data/base.jsonl") || hit.byte_start != 0
    ));
    assert!(live.hits.iter().all(
        |hit| hit.file_path != std::path::Path::new("data/delta.jsonl")
            || hit.byte_start != retired_segments[0][0].index.records[1].range.start
    ));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn segment_direct_lookup_uses_local_slot_and_published_file() {
    use crate::build::{
        FilePreparationBinding, SegmentAdmission, SourceFileInput, prepare_file_build,
    };
    use crate::inventory::SourceRevision;
    use crate::model::{CollectionId, FileId, RecordSlot};
    use crate::repository::{RepositoryOptions, SnapshotMode};

    let root = std::env::temp_dir().join(format!(
        "objekte-segment-lookup-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/base.jsonl"), b"{\"name\":\"base\"}\n").unwrap();
    let definition = CollectionDefinition::folder("data", "data", SourceFormat::JsonLines);
    let repo = Arc::new(
        Repository::open_with_options(
            &root,
            vec![definition],
            RepositoryOptions {
                snapshot_mode: SnapshotMode::Disabled,
                ..Default::default()
            },
        )
        .unwrap(),
    );
    let path = root.join("data/added.jsonl");
    std::fs::write(&path, b"{\"name\":\"added\"}\n").unwrap();
    let collection = &repo.collections()[0];
    let input = SourceFileInput {
        id: FileId(1),
        relative_path: "data/added.jsonl".into(),
        revision: SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
        created_ns: None,
    };
    let prepared = prepare_file_build(
        &root,
        input,
        &[FilePreparationBinding {
            collection: CollectionId(0),
            definition: &collection.definition,
            source: &collection.source,
        }],
        repo.options.bigram_alphabet,
        repo.options.inference,
        1,
    )
    .unwrap();
    let mut admission = SegmentAdmission::new(CollectionId(0), &collection.source);
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
        .into_indexed(
            &collection.definition,
            repo.options.bigram_alphabet,
            repo.options.inference,
            1,
            &pool,
        )
        .unwrap();
    let catalog = FileCatalog::from_repository(Arc::clone(&repo))
        .unwrap()
        .appended(segment.files[0].entry.clone(), None, None)
        .unwrap();
    let id = segment.records[0].id.0.as_str();
    let active = ActiveRecords::all(segment.records.len()).unwrap();
    let revision = IndexRevision(9);
    let locator = get_segment_record_by_id(
        &repo,
        0,
        &segment,
        &active,
        &catalog,
        revision,
        id,
        ReturnMode::Locator,
    )
    .unwrap()
    .unwrap();
    assert_eq!(locator.file_id, 1);
    assert_eq!(locator.generation, revision.0);
    assert_eq!(locator.file_path, std::path::Path::new("data/added.jsonl"));
    assert_eq!(locator.record_id, id);
    assert!(locator.payload.is_none());
    let bytes = get_segment_record_by_id(
        &repo,
        0,
        &segment,
        &active,
        &catalog,
        revision,
        id,
        ReturnMode::Bytes,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        bytes.payload,
        Some(Payload::Bytes(b"{\"name\":\"added\"}".to_vec()))
    );
    let logical = get_segment_record_by_id(
        &repo,
        0,
        &segment,
        &active,
        &catalog,
        revision,
        id,
        ReturnMode::Logical,
    )
    .unwrap()
    .unwrap();
    assert!(matches!(logical.payload, Some(Payload::Logical(_))));
    let retired = active.retired(&[RecordSlot(0)]).unwrap();
    assert!(
        get_segment_record_by_id(
            &repo,
            0,
            &segment,
            &retired,
            &catalog,
            revision,
            id,
            ReturnMode::Locator
        )
        .unwrap()
        .is_none()
    );
    assert!(
        get_segment_record_by_id(
            &repo,
            0,
            &segment,
            &active,
            &catalog,
            revision,
            "missing",
            ReturnMode::Locator,
        )
        .unwrap()
        .is_none()
    );
    std::fs::write(&path, b"{\"name\":\"different\"}\n").unwrap();
    assert!(
        get_segment_record_by_id(
            &repo,
            0,
            &segment,
            &active,
            &catalog,
            revision,
            id,
            ReturnMode::Bytes
        )
        .is_err()
    );
    let retired_catalog = catalog.retired(FileId(1)).unwrap();
    assert!(
        get_segment_record_by_id(
            &repo,
            0,
            &segment,
            &active,
            &retired_catalog,
            revision,
            id,
            ReturnMode::Locator,
        )
        .is_err()
    );
    std::fs::remove_dir_all(root).unwrap();
}

fn concurrent_test_inferred_column(path: &str) -> SortColumn {
    SortColumn {
        path: FieldPath::parse(path).unwrap(),
        kind: LogicalType::String,
        keys: vec![SortKey::String(vec![1])],
        ascending: vec![crate::model::RecordSlot(0)],
        descending: Vec::new(),
        locale: "de".into(),
        options: SortOptions::default(),
        collation_version: crate::sorting::COLLATION_VERSION.into(),
    }
}

#[test]
fn active_inspection_traces_use_live_domain_and_match_search() {
    let root = std::env::temp_dir().join(format!(
        "objekte-active-trace-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, body) in [
        ("00", r#"{"kind":"yes","body":"needle"}"#),
        ("01", r#"{"kind":"yes","body":"needle"}"#),
        ("02", r#"{"kind":"no","body":"hay"}"#),
    ] {
        std::fs::write(root.join(format!("data/{name}.json")), body).unwrap();
    }
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
    definition
        .schema
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
        vec![definition],
        crate::repository::RepositoryOptions {
            snapshot_mode: crate::repository::SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    let active = vec![
        crate::inventory::ActiveRecords::all(3)
            .unwrap()
            .retired(&[crate::model::RecordSlot(1)])
            .unwrap(),
    ];
    let query = Query {
        collections: vec!["data".into()],
        filter: Some(parse_filter("!(kind = 'no')").unwrap()),
        fts: None,
        options: QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let (result, actual) = search_analyze_with_active(&repo, &query, Some(&active)).unwrap();
    assert_eq!(result.count, Some(1));
    assert_eq!(
        result.hits,
        search_active(&repo, &query, &active).unwrap().hits
    );
    assert_eq!(actual[0].input_records, 2);
    assert!(actual[0].candidates <= 2);
    let estimated = explain_with_active(&repo, &query, Some(&active)).unwrap();
    assert_eq!(estimated[0].input_records, 2);
    assert!(estimated[0].candidates <= 2);
    assert_eq!(search_analyze(&repo, &query).unwrap().0.count, Some(2));
    assert_eq!(explain(&repo, &query).unwrap()[0].input_records, 3);
    std::fs::remove_dir_all(root).unwrap();
}

fn assert_concurrent_inferred_builds(same_key: bool) {
    use std::sync::mpsc;
    use std::time::Duration;

    let cache = Arc::new(Mutex::new(crate::sorting::InferredOrderCache::new(
        1024 * 1024,
    )));
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_first_tx, release_first_rx) = mpsc::channel();
    let (release_second_tx, release_second_rx) = mpsc::channel();
    let first_path = "first";
    let second_path = if same_key { "first" } else { "second" };
    let first_key = InferredOrderKey::new(
        0,
        1,
        FieldPath::parse(first_path).unwrap(),
        "de",
        SortOptions::default(),
        false,
    );
    let second_key = InferredOrderKey::new(
        0,
        1,
        FieldPath::parse(second_path).unwrap(),
        "de",
        SortOptions::default(),
        false,
    );
    std::thread::scope(|scope| {
        let first_cache = cache.clone();
        let first_entered = entered_tx.clone();
        let first = scope.spawn(move || {
            cached_inferred_order(&first_cache, first_key, || {
                first_entered.send(1).unwrap();
                release_first_rx.recv().unwrap();
                Ok(Some(concurrent_test_inferred_column(first_path)))
            })
            .unwrap()
            .unwrap()
        });
        assert_eq!(entered_rx.recv_timeout(Duration::from_secs(10)), Ok(1));
        assert!(
            cache.try_lock().is_ok(),
            "the first builder retained the cache lock"
        );
        let second_cache = cache.clone();
        let second = scope.spawn(move || {
            cached_inferred_order(&second_cache, second_key, || {
                entered_tx.send(2).unwrap();
                release_second_rx.recv().unwrap();
                Ok(Some(concurrent_test_inferred_column(second_path)))
            })
            .unwrap()
            .unwrap()
        });
        // Both builders must enter before either is released. A timed receive
        // bounds test failure without making elapsed time an acceptance gate.
        let second_entered = entered_rx.recv_timeout(Duration::from_secs(10));
        release_first_tx.send(()).unwrap();
        release_second_tx.send(()).unwrap();
        assert_eq!(second_entered, Ok(2));
        let first_column = first.join().unwrap();
        let second_column = second.join().unwrap();
        assert_eq!(Arc::ptr_eq(&first_column, &second_column), same_key);
    });
    let stats = cache.lock().unwrap().stats();
    assert_eq!(stats.builds, 2);
    assert_eq!(stats.entries, if same_key { 1 } else { 2 });
    assert_eq!(stats.hits, if same_key { 1 } else { 0 });
    assert!(stats.resident_bytes <= 1024 * 1024);
}

#[test]
fn distinct_inferred_orders_build_without_holding_one_global_mutex() {
    assert_concurrent_inferred_builds(false);
}

#[test]
fn concurrent_same_key_inferred_builds_publish_one_resident_order() {
    assert_concurrent_inferred_builds(true);
}

fn write_inferred_order_fixture(tag: &str, records: usize) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "objekte-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for slot in 0..records {
        let content = match slot {
            0 => r#"{"uuid":null,"label":null,"flag":null}"#.to_owned(),
            1 => r#"{"other":"missing"}"#.to_owned(),
            2 => "{".to_owned(),
            _ => serde_json::json!({
                "uuid": format!("00000000-0000-0000-0000-{slot:012}"),
                "label": if slot % 2 == 0 { "Ähre" } else { "Zug" },
                "flag": slot % 2 == 0,
            })
            .to_string(),
        };
        std::fs::write(root.join(format!("data/{slot:04}.json")), content).unwrap();
    }
    root
}

#[test]
fn inferred_order_limit_rejects_before_key_vector_in_both_modes() {
    use crate::inference::{InferenceConfig, InferenceMode};
    use crate::repository::{RepositoryOptions, SnapshotMode};
    use crate::streaming::{
        ExecutionContext, ExecutionErrorKind, ExecutionLimits, ResultScope, StreamControl,
        StreamError, StreamOptions,
    };
    use crate::workload::SortField;

    let root = write_inferred_order_fixture("inferred-order-limit", 32);
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    let request = Query {
        collections: vec!["data".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: vec![SortField::simple("label", false)],
            limit: 3,
            offset: 0,
            count: false,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let context = || {
        ExecutionContext::new(
            crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
            ExecutionLimits {
                inferred_order_bytes: 1,
                ..Default::default()
            },
            None,
        )
    };
    assert_eq!(
        repo.search_with_context(&request, &context())
            .unwrap_err()
            .kind,
        ExecutionErrorKind::Resource
    );
    let outcome = repo.search_stream_scoped_with_context(
        &request,
        ResultScope::Page {
            offset: 0,
            limit: 3,
        },
        StreamOptions::default(),
        &context(),
        &mut |_| Ok(StreamControl::Continue),
    );
    assert!(
        matches!(outcome, Err(StreamError::Resource { .. })),
        "{outcome:?}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn inferred_order_construction_observes_mid_build_cancellation() {
    use crate::inference::{InferenceConfig, InferenceMode};
    use crate::streaming::{
        CancellationToken, ExecutionContext, ExecutionErrorKind, ExecutionLimits,
    };
    let root = write_inferred_order_fixture("inferred-order-cancel", 32);
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        crate::repository::RepositoryOptions {
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            snapshot_mode: crate::repository::SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    let collection = &repo.collections[0];
    let index = collection
        .inference
        .as_ref()
        .unwrap()
        .complete_scalar_index(&["label".into()], &collection.logical_records)
        .unwrap();
    let token = CancellationToken::new();
    let context = ExecutionContext::new(
        crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
        ExecutionLimits::default(),
        Some(token.clone()),
    );
    context.cancel_on_check_for_test(3);
    let error = SortColumn::from_inferred_posting(
        index,
        &collection.generation,
        FieldPath::parse("label").unwrap(),
        &collection.definition.sort_locale,
        collection.definition.sort_options,
        false,
        Some(&context),
    )
    .unwrap_err();
    assert!(token.is_cancelled());
    assert!(context.checks_for_test() >= 3);
    assert_eq!(
        context.recorded_error(error).kind,
        ExecutionErrorKind::Cancelled
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn inferred_order_cache_reuses_pages_and_restores_without_persisting_orders() {
    use crate::inference::{InferenceConfig, InferenceMode};
    use crate::repository::{RepositoryOptions, SnapshotMode};
    use crate::workload::SortField;

    let root = write_inferred_order_fixture("inferred-order-cache", 150);
    let definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
    let options = RepositoryOptions {
        snapshot_mode: SnapshotMode::IfValid,
        verified_cache_bytes: 0,
        inference: InferenceConfig {
            mode: InferenceMode::AutoIndex,
            ..Default::default()
        },
        ..Default::default()
    };
    let repo = Repository::open_with_options(&root, vec![definition.clone()], options).unwrap();
    let request = |field: &str, descending: bool, offset: usize, count: bool| Query {
        collections: vec!["data".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: vec![SortField::simple(field, descending)],
            limit: 7,
            offset,
            count,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    for field in ["uuid", "label", "flag"] {
        for descending in [false, true] {
            let first = request(field, descending, 0, false);
            let before = repo.inferred_orders.lock().unwrap().stats();
            let estimate = repo.explain(&first).unwrap();
            assert!(estimate[0].sort_strategy_estimated);
            assert_eq!(repo.inferred_orders.lock().unwrap().stats(), before);
            for (offset, count) in [(0, false), (17, false), (17, true)] {
                let query = request(field, descending, offset, count);
                let result = repo.search(&query).unwrap();
                let reference = repo.search_reference(&query).unwrap();
                assert_eq!(
                    result.hits, reference.hits,
                    "{field} {descending} {offset} {count}"
                );
                assert_eq!(result.count, reference.count);
                assert_eq!(result.counters.files_opened, 0);
                assert_eq!(result.counters.record_parses, 0);
            }
            let after = repo.inferred_orders.lock().unwrap().stats();
            assert_eq!(after.builds, before.builds + 1, "{field} {descending}");
            assert!(after.hits >= before.hits + 2, "{field} {descending}");
            assert!(after.resident_bytes <= crate::sorting::INFERRED_ORDER_CACHE_BYTES);
        }
    }
    repo.save_default_snapshot_for_test();
    let restored = Repository::open_with_options(&root, vec![definition], options).unwrap();
    assert!(restored.startup().restored);
    assert_eq!(restored.inferred_orders.lock().unwrap().stats().builds, 0);
    let query = request("uuid", false, 17, true);
    let result = restored.search(&query).unwrap();
    assert_eq!(result.hits, restored.search_reference(&query).unwrap().hits);
    assert_eq!(restored.inferred_orders.lock().unwrap().stats().builds, 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn selective_stored_order_attempt_reuses_filter_membership_for_top_k() {
    use crate::workload::SortField;

    let root = std::env::temp_dir().join(format!("objekte-plan-reuse-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for slot in 0..12 {
        let body =
            serde_json::json!({"rank":slot,"kind":if slot == 7 { "rare" } else { "common" }});
        std::fs::write(root.join(format!("data/{slot:02}.json")), body.to_string()).unwrap();
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
    schema
        .register_field(FieldDefinition {
            path: vec!["rank".into()],
            kind: LogicalType::Number,
            required: true,
            nullable: false,
            indexed: false,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
    definition.schema = schema;
    definition.sort_fields = vec![FieldPath::parse("rank").unwrap()];
    let repo = Repository::open_with_options(
        &root,
        vec![definition],
        crate::repository::RepositoryOptions {
            snapshot_mode: crate::repository::SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    let request = Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(parse_filter("kind = 'rare'").unwrap()),
        options: QueryOptions {
            sort: vec![SortField::simple("rank", false)],
            limit: 1,
            offset: 0,
            count: false,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let (result, traces) = repo.explain_analyze(&request).unwrap();
    assert_eq!(result.hits, repo.search_reference(&request).unwrap().hits);
    assert_eq!(result.hits[0].record_id, "path:07.json");
    assert_eq!(traces[0].sort_strategy, "bounded_top_k");
    assert_eq!(traces[0].planning.reused_order_plans, 1);
    assert_eq!(traces[0].planning.candidate_bitmap_builds, 1);
    std::fs::remove_dir_all(root).unwrap();
}

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
    assert_eq!(repo.files.files.len(), 1);
    assert!(repo.files.files[0].diagnostics.is_empty());
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
            snapshot_mode: SnapshotMode::IfValid,
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
        let issue = built
            .files
            .diagnostic(crate::model::FileId(bad.file_id), bad.diagnostic_ref)
            .expect("fallback hit must identify its diagnostic");
        assert_eq!(issue.category, crate::source::DiagnosticCategory::Syntax);
        assert_eq!(issue.code, "json.parse");
        assert_eq!(issue.line, Some(2));
        assert_eq!(
            issue.range,
            Some(crate::inventory::ByteRange { start: 18, end: 27 })
        );
        let built_issue = issue.clone();
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
                pattern_parameter: None,
                mode: crate::query::FtsMode::Literal,
                case_sensitivity: crate::query::CaseSensitivity::Sensitive,
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
        let mut raw_filtered = raw_fts.clone();
        raw_filtered.filter = Some(parse_filter("@record.class = 1").unwrap());
        let filtered = built.search(&raw_filtered).unwrap();
        assert_eq!(filtered.count, Some(1));
        assert_eq!(
            filtered.hits[0].matched_fields,
            ["@record.class", "@record.raw_text"]
        );
        assert_eq!(filtered.counters.record_parses, 0);
        assert_eq!(filtered.counters.raw_text_verifications, 1);
        let selected = query(
            None,
            Some(FtsQuery {
                pattern_parameter: None,
                mode: crate::query::FtsMode::Literal,
                case_sensitivity: crate::query::CaseSensitivity::Sensitive,
                literal: "title".into(),
                fields: vec![crate::query::FtsField {
                    query_path: None,
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
            expression: None,
        }];
        let ordered = built.search(&sorted).unwrap();
        assert_eq!(ordered.hits[0].file_status, FileStatus::Valid);
        assert!(
            ordered.hits[1..]
                .iter()
                .all(|hit| hit.file_status == FileStatus::Broken)
        );
        assert_eq!(ordered.hits, built.search_reference(&sorted).unwrap().hits);
        built.save_default_snapshot_for_test();
        let restored =
            Repository::open_with_options(&root, vec![definition.clone()], options).unwrap();
        assert!(restored.startup().restored);
        assert_eq!(restored.search(&all).unwrap().hits, result.hits);
        assert_eq!(restored.search(&raw_fts).unwrap().hits, raw_result.hits);
        let restored_bad = restored
            .search(&all)
            .unwrap()
            .hits
            .into_iter()
            .find(|hit| hit.record_class == RecordClass::RawFallback)
            .unwrap();
        assert_eq!(restored_bad.diagnostic_ref, bad.diagnostic_ref);
        assert_eq!(
            restored.files.diagnostic(
                crate::model::FileId(restored_bad.file_id),
                restored_bad.diagnostic_ref
            ),
            Some(&built_issue)
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn fallback_negation_keeps_unavailable_truth_out_of_matched_fields() {
    use crate::inventory::RecordClass;
    use crate::repository::{RepositoryOptions, SnapshotMode};
    let root = std::env::temp_dir().join(format!(
        "objekte-fallback-negation-evidence-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/mixed.jsonl"),
        b"{\"title\":\"x\"}\n{\"title\":\n{\"title\":\"other\"}\n{}\n{\"title\":null}\n",
    )
    .unwrap();
    std::fs::write(root.join("data/valid.jsonl"), b"{}\n").unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["title".into()],
            kind: LogicalType::String,
            required: false,
            nullable: true,
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
    let critical = "(@file.status = 1 && !(title = 'x')) || @record.class = 1";
    let cases = [
        (critical, true, &["@record.class"][..]),
        (
            "@record.class = 1 || (@file.status = 1 && !(title = 'x'))",
            true,
            &["@record.class"],
        ),
        (
            "(@file.status = 0 && !(title = 'x')) || @record.class = 1",
            true,
            &["@record.class"],
        ),
        (
            "@record.class = 1 && !(@file.status = 0 && title = 'x')",
            true,
            &["@record.class"],
        ),
        ("@record.class = 1 && !(!(title = 'x'))", false, &[]),
        ("!(!(@record.class = 1))", true, &[]),
    ];
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
        built.save_default_snapshot_for_test();
        let restored =
            Repository::open_with_options(&root, vec![definition.clone()], options).unwrap();
        assert!(restored.startup().restored);
        for repo in [&built, &restored] {
            for cache_bytes in [0, 1 << 20] {
                repo.configure_verified_cache(cache_bytes).unwrap();
                for sorted in [false, true] {
                    for evidence in [false, true] {
                        for (filter, fallback_expected, fallback_fields) in cases {
                            let query = Query {
                                collections: vec!["data".into()],
                                filter: Some(parse_filter(filter).unwrap()),
                                fts: None,
                                options: QueryOptions {
                                    sort: if sorted {
                                        vec![crate::workload::SortField {
                                            field: "@file.status".into(),
                                            descending: false,
                                            expression: None,
                                        }]
                                    } else {
                                        vec![]
                                    },
                                    limit: 10,
                                    offset: 0,
                                    count: true,
                                    return_mode: ReturnMode::Locator,
                                    evidence,
                                },
                            };
                            let result = repo.search(&query).unwrap();
                            assert_eq!(
                                result.hits,
                                repo.search_reference(&query).unwrap().hits,
                                "{filter}"
                            );
                            if filter == critical {
                                assert_eq!(result.count, Some(4));
                            }
                            let fallback = result
                                .hits
                                .iter()
                                .find(|hit| hit.record_class == RecordClass::RawFallback);
                            assert_eq!(fallback.is_some(), fallback_expected, "{filter}");
                            if let Some(fallback) = fallback {
                                assert_eq!(
                                    fallback.matched_fields,
                                    if evidence { fallback_fields } else { &[] },
                                    "{filter}"
                                );
                            }
                            if filter == "@record.class = 1 && !(@file.status = 0 && title = 'x')" {
                                assert_eq!(result.counters.files_opened, 0);
                                assert_eq!(result.counters.record_parses, 0);
                            }
                            if cache_bytes != 0 {
                                let cached = repo.search(&query).unwrap();
                                assert!(cached.counters.verified_result_hit);
                                assert_eq!(cached.hits, result.hits);
                            }
                        }
                    }
                }
            }
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn one_fallback_does_not_disable_selective_logical_postings() {
    use crate::repository::{RepositoryOptions, SnapshotMode};
    let root = std::env::temp_dir().join(format!("objekte-mixed-posting-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let mut source = String::new();
    for index in 0..1000 {
        if index == 0 {
            source.push_str("{\"title\":\"other\",\"note\":\"other\",\"distractor\":\"needle\"}\n");
        } else {
            source.push_str("{\"title\":\"other\",\"note\":\"other\"}\n");
        }
    }
    source.push_str("{\"title\":\"needle\",\"note\":\"needle\"}\n{\"title\":needle\n");
    std::fs::write(root.join("data/records.jsonl"), source).unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["title".into()],
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
            path: vec!["note".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: false,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    let definitions = vec![CollectionDefinition {
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
    }];
    for alphabet in [
        crate::workload::BigramAlphabet::Ascii,
        crate::workload::BigramAlphabet::Unicode,
    ] {
        let options = RepositoryOptions {
            bigram_alphabet: alphabet,
            snapshot_mode: SnapshotMode::IfValid,
            ..RepositoryOptions::default()
        };
        let built = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
        built.save_default_snapshot_for_test();
        let restored = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
        assert!(restored.startup().restored);
        for repo in [&built, &restored] {
            for (filter, expected, fallback_hit) in [
                ("title = 'needle'", 1, false),
                ("!(title = 'needle')", 1000, false),
                ("@record.class = 1 || title = 'needle'", 2, true),
                ("@file.status = 1 && title = 'needle'", 1, false),
                ("!(@file.status = 0 && title = 'needle')", 1002, true),
                ("title:exists = false", 0, false),
                ("title:null = true", 0, false),
            ] {
                let query = Query {
                    collections: vec!["data".into()],
                    fts: None,
                    filter: Some(parse_filter(filter).unwrap()),
                    options: QueryOptions {
                        sort: vec![],
                        limit: 3,
                        offset: 0,
                        count: true,
                        return_mode: ReturnMode::Locator,
                        evidence: true,
                    },
                };
                let optimized = repo.search(&query).unwrap();
                let reference = repo.search_reference(&query).unwrap();
                assert_eq!(optimized.count, Some(expected), "{filter}");
                assert_eq!(optimized.hits, reference.hits, "{filter}");
                assert_eq!(optimized.counters.files_opened, 0, "{filter}");
                assert_eq!(optimized.counters.record_parses, 0, "{filter}");
                assert_eq!(
                    optimized
                        .hits
                        .iter()
                        .any(|hit| hit.record_class == crate::inventory::RecordClass::RawFallback),
                    fallback_hit,
                    "{filter}"
                );
                if filter == "title = 'needle'" {
                    assert_eq!(optimized.counters.candidates, 1);
                    assert_eq!(optimized.counters.exact_posting_matches, 1);
                }
            }
            let query = Query {
                collections: vec!["data".into()],
                fts: None,
                filter: Some(parse_filter("note ~ 'needle' || @record.class = 1").unwrap()),
                options: QueryOptions {
                    sort: vec![],
                    limit: 2,
                    offset: 0,
                    count: true,
                    return_mode: ReturnMode::Locator,
                    evidence: true,
                },
            };
            let result = repo.search(&query).unwrap();
            assert_eq!(result.count, Some(2));
            assert_eq!(result.hits, repo.search_reference(&query).unwrap().hits);
            assert_eq!(result.counters.record_parses, 2);
            assert_eq!(result.counters.files_opened, 1);
            assert_eq!(result.counters.exact_posting_matches, 1);
        }
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
            pattern_parameter: None,
            mode: crate::query::FtsMode::Literal,
            case_sensitivity: crate::query::CaseSensitivity::Sensitive,
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
                expression: None,
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

    // A count request keeps its syntactic sort contract, but never constructs
    // a page or ranks the 200 index-proven members.
    let mut count_only = query.clone();
    count_only.options.limit = 0;
    count_only.options.offset = 7;
    count_only.options.return_mode = ReturnMode::Logical;
    let direct_count = repo.search(&count_only).unwrap();
    assert_eq!(direct_count.count, Some(200));
    assert!(direct_count.hits.is_empty());
    assert_eq!(direct_count.counters.exact_posting_matches, 200);
    assert_eq!(direct_count.counters.files_opened, 0);
    assert_eq!(direct_count.counters.record_parses, 0);
    assert_eq!(direct_count.counters.sort_key_evaluations, 0);
    assert_eq!(direct_count.counters.peak_retained_payload_bytes, 0);
    assert_eq!(repo.select_exact("a", &count_only).unwrap().len(), 100);
    let direct_count_query = count_only.clone();
    let streamed = repo
        .search_stream(
            &direct_count_query,
            crate::streaming::StreamOptions::default(),
            &crate::streaming::CancellationToken::new(),
            &mut |event| {
                assert!(!matches!(
                    event,
                    crate::streaming::StreamEvent::OrderedRows { .. }
                ));
                Ok(crate::streaming::StreamControl::Continue)
            },
        )
        .unwrap();
    let crate::streaming::StreamOutcome::Complete(streamed) = streamed else {
        panic!("expected complete stream")
    };
    assert_eq!(streamed.count, direct_count.count);
    assert_eq!(streamed.returned_rows, 0);
    let mut invalid_sort = count_only.clone();
    invalid_sort.options.sort[0].field = "[unterminated".into();
    assert!(repo.search(&invalid_sort).is_err());
    assert!(repo.select_exact("a", &invalid_sort).is_err());

    count_only.filter = Some(parse_filter("kind = 'yes' && body ~ 'needle'").unwrap());
    let residual_count = repo.search(&count_only).unwrap();
    assert_eq!(residual_count.count, Some(200));
    assert!(residual_count.counters.scanned_records >= 200);
    assert_eq!(residual_count.counters.sort_key_evaluations, 0);
    assert_eq!(residual_count.counters.peak_retained_payload_bytes, 0);
    assert_eq!(repo.select_exact("a", &count_only).unwrap().len(), 100);
    count_only.filter = Some(parse_filter("kind = 'absent'").unwrap());
    let empty_count = repo.search(&count_only).unwrap();
    assert_eq!(empty_count.count, Some(0));
    assert_eq!(empty_count.counters.files_opened, 0);
    assert!(repo.select_exact("a", &count_only).unwrap().is_empty());

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
        expression: None,
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
        expression: None,
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
    let cached_count = cached_repo.search(&direct_count_query).unwrap();
    assert_eq!(cached_count.count, Some(200));
    assert_eq!(
        cached_count.count,
        repo.search(&direct_count_query).unwrap().count
    );
    assert_eq!(cached_count.counters.sort_key_evaluations, 0);
    assert_eq!(
        cached_repo
            .select_exact("a", &direct_count_query)
            .unwrap()
            .len(),
        repo.select_exact("a", &direct_count_query).unwrap().len()
    );
    query.options.count = false;
    let cached_page = cached_repo.search(&query).unwrap();
    assert!(cached_page.counters.verified_result_hit);
    assert_eq!(cached_page.count, None);
    assert_eq!(cached_page.hits, first_page.hits);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn exact_collection_handles_relation_membership_without_ranking() {
    use crate::relation::{RelationCardinality, RelationDefinition};

    let root = std::env::temp_dir().join(format!(
        "objekte-exact-relation-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    for name in ["sources", "targets"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }
    for (path, body) in [
        ("targets/t1.json", r#"{"id":"t1","title":"Needle"}"#),
        ("targets/t2.json", r#"{"id":"t2","title":"Other"}"#),
        ("sources/s1.json", r#"{"id":"s1","entry":"t1"}"#),
        ("sources/s2.json", r#"{"id":"s2","entry":"t2"}"#),
        ("sources/broken.json", "{broken"),
    ] {
        std::fs::write(root.join(path), body).unwrap();
    }
    let mut definitions = Vec::new();
    for name in ["sources", "targets"] {
        let mut schema = Schema::default();
        for field in if name == "sources" {
            &["id", "entry"][..]
        } else {
            &["id", "title"][..]
        } {
            schema
                .register_field(FieldDefinition {
                    path: vec![(*field).into()],
                    kind: if *field == "title" {
                        LogicalType::String
                    } else {
                        LogicalType::Id
                    },
                    required: false,
                    nullable: true,
                    indexed: true,
                    multi: false,
                    element_nullable: vec![],
                })
                .unwrap();
        }
        if name == "sources" {
            schema
                .register_relation(RelationDefinition {
                    path: vec!["entry".into()],
                    target_collection: "targets".into(),
                    cardinality: RelationCardinality::One,
                    reverse_alias: None,
                })
                .unwrap();
        }
        definitions.push(CollectionDefinition {
            name: name.into(),
            source_path: name.into(),
            format: SourceFormat::Json,
            identity: IdentityPolicy::Intrinsic(vec!["id".into()]),
            discovery: Default::default(),
            schema,
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        });
    }
    let repo = Repository::open_with_options(
        &root,
        definitions,
        crate::repository::RepositoryOptions {
            snapshot_mode: crate::repository::SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    let query = Query {
        collections: vec!["sources".into()],
        fts: None,
        filter: Some(parse_filter("entry.title ?~ 'Needle'").unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 0,
            offset: 2,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let counted = repo.search(&query).unwrap();
    assert_eq!(counted.count, Some(1));
    assert_eq!(counted.count, repo.search_reference(&query).unwrap().count);
    assert_eq!(counted.counters.sort_key_evaluations, 0);
    assert_eq!(repo.select_exact("sources", &query).unwrap().len(), 1);
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
                expression: None,
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
                expression: None,
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
            pattern_parameter: None,
            mode: crate::query::FtsMode::Literal,
            case_sensitivity: crate::query::CaseSensitivity::Sensitive,
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
        expression: None,
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
                expression: None,
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
    repo.save_default_snapshot_for_test();
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
            snapshot_mode: crate::repository::SnapshotMode::IfValid,
            ..Default::default()
        };
        for open in 0..2 {
            let repo =
                Repository::open_with_options(&root, vec![definition(indexed)], options).unwrap();
            assert_eq!(repo.startup().restored, open == 1);
            if open == 0 {
                repo.save_default_snapshot_for_test();
            }
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
                pattern_parameter: None,
                mode: crate::query::FtsMode::Literal,
                case_sensitivity: crate::query::CaseSensitivity::Sensitive,
                literal: literal.into(),
                fields: vec![crate::query::FtsField {
                    query_path: None,
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

#[test]
fn computed_sort_combines_payload_key_and_target_budgets() {
    assert!(super::check_computed_sort_budget(32 << 20, 32 << 20, 32 << 20).is_ok());
    assert!(
        super::check_computed_sort_budget(64 << 20, 1, 64 << 20)
            .unwrap_err()
            .contains("memory budget")
    );
    let cached = CachedResult {
        members: vec![
            CachedMember {
                collection_index: 0,
                slot: 0,
                matched_fields: vec!["title".into()],
            },
            CachedMember {
                collection_index: 0,
                slot: 1,
                matched_fields: vec![],
            },
        ],
    };
    let without_reads = super::cached_sort_fixed_bytes(&cached, 0).unwrap();
    let with_reads = super::cached_sort_fixed_bytes(&cached, 2).unwrap();
    assert_eq!(
        with_reads - without_reads,
        2 * std::mem::size_of::<super::CachedSortRead>() as u64
    );
    assert!(
        super::check_computed_sort_budget_with_limit(with_reads - 1, with_reads, 0, 0,)
            .unwrap_err()
            .contains("memory budget")
    );
}

#[test]
fn computed_sort_request_limit_rejects_before_source_batch_in_both_modes() {
    use crate::streaming::{
        ExecutionContext, ExecutionErrorKind, ExecutionLimits, ResultScope, StreamControl,
        StreamError, StreamOptions,
    };
    let root = std::env::temp_dir().join(format!(
        "objekte-computed-context-limit-{}",
        std::process::id()
    ));
    let source = root.join("items");
    std::fs::create_dir_all(&source).unwrap();
    for index in 0..4 {
        std::fs::write(
            source.join(format!("{index}.json")),
            format!(r#"{{"title":"Title {index}"}}"#),
        )
        .unwrap();
    }
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition::folder(
            "items",
            "items",
            SourceFormat::Json,
        )],
        crate::repository::RepositoryOptions {
            query_workers: 4,
            snapshot_mode: crate::repository::SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    let query = Query {
        collections: vec!["items".into()],
        filter: None,
        fts: None,
        options: QueryOptions {
            sort: vec![SortField::computed(
                crate::workload::SortExpression {
                    path: "title".into(),
                    binding: Some("lower".into()),
                    reducer: None,
                },
                false,
            )],
            limit: 2,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let context = || {
        ExecutionContext::new(
            crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
            ExecutionLimits {
                computed_sort_bytes: 1,
                ..Default::default()
            },
            None,
        )
    };
    let ordinary = repo.search_with_context(&query, &context()).unwrap_err();
    assert_eq!(ordinary.kind, ExecutionErrorKind::Resource, "{ordinary:?}");
    let result = repo.search_stream_scoped_with_context(
        &query,
        ResultScope::Page {
            offset: 0,
            limit: 2,
        },
        StreamOptions::default(),
        &context(),
        &mut |_| Ok(StreamControl::Continue),
    );
    assert!(
        matches!(result, Err(StreamError::Resource { .. })),
        "{result:?}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn computed_sort_cancels_after_worker_has_parsed_source_records() {
    use crate::source::{
        ExtractedRecord, JsonReader, ScanProjection, SourceReader, SourceRegistry,
    };
    use crate::streaming::{
        CancellationToken, ExecutionContext, ExecutionErrorKind, ExecutionLimits,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};


    struct CancellingReader {
        token: CancellationToken,
        query_reads: Arc<AtomicUsize>,
    }


    impl SourceReader for CancellingReader {
        fn name(&self) -> &'static str {
            "cancelling_json"
        }

        fn frame(&self, bytes: &[u8]) -> Result<Vec<ByteRange>, String> {
            JsonReader.frame(bytes)
        }

        fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
            JsonReader.parse(bytes)
        }

        fn parse_for_query(
            &self,
            bytes: &[u8],
            projection: &ScanProjection,
        ) -> Result<ExtractedRecord, String> {
            let record = JsonReader.parse_for_query(bytes, projection)?;
            if self.query_reads.fetch_add(1, Ordering::Relaxed) + 1 == 3 {
                self.token.cancel();
            }
            Ok(record)
        }
    }


    let root = std::env::temp_dir().join(format!(
        "objekte-computed-context-cancel-{}",
        std::process::id()
    ));
    let source = root.join("items");
    std::fs::create_dir_all(&source).unwrap();
    for index in 0..16 {
        std::fs::write(
            source.join(format!("{index:03}.tiny")),
            format!(r#"{{"title":"Title {index}"}}"#),
        )
        .unwrap();
    }
    let token = CancellationToken::new();
    let reads = Arc::new(AtomicUsize::new(0));
    let mut sources = SourceRegistry::builtin();
    sources
        .register(
            "cancelling_json",
            1,
            "tiny",
            Arc::new(CancellingReader {
                token: token.clone(),
                query_reads: reads.clone(),
            }),
        )
        .unwrap();
    let repo = Repository::open_with_sources(
        &root,
        vec![CollectionDefinition::folder(
            "items",
            "items",
            SourceFormat::registered("cancelling_json"),
        )],
        crate::repository::RepositoryOptions {
            query_workers: 1,
            snapshot_mode: crate::repository::SnapshotMode::Disabled,
            ..Default::default()
        },
        sources,
    )
    .unwrap();
    let query = Query {
        collections: vec!["items".into()],
        filter: None,
        fts: None,
        options: QueryOptions {
            sort: vec![SortField::computed(
                crate::workload::SortExpression {
                    path: "title".into(),
                    binding: Some("lower".into()),
                    reducer: None,
                },
                false,
            )],
            limit: 1,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let context = ExecutionContext::new(
        crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
        ExecutionLimits::default(),
        Some(token.clone()),
    );
    let error = repo.search_with_context(&query, &context).unwrap_err();
    assert_eq!(error.kind, ExecutionErrorKind::Cancelled, "{error:?}");
    assert!(token.is_cancelled());
    assert_eq!(reads.load(Ordering::Relaxed), 3);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn extension_query_failure_has_ordinary_and_stream_class() {
    use crate::source::{
        ExtractedRecord, JsonReader, ScanProjection, SourceReader, SourceRegistry,
    };
    use crate::streaming::{
        ExecutionContext, ExecutionErrorKind, ExecutionLimits, ResultScope, StreamControl,
        StreamOptions,
    };


    struct FailingReader;


    impl SourceReader for FailingReader {
        fn name(&self) -> &'static str {
            "failing_query_reader"
        }

        fn frame(&self, bytes: &[u8]) -> Result<Vec<ByteRange>, String> {
            JsonReader.frame(bytes)
        }

        fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
            JsonReader.parse(bytes)
        }

        fn parse_for_query(
            &self,
            _bytes: &[u8],
            _projection: &ScanProjection,
        ) -> Result<ExtractedRecord, String> {
            Err("query extension failed".into())
        }
    }


    let root = std::env::temp_dir().join(format!(
        "objekte-extension-query-error-{}",
        std::process::id()
    ));
    let source = root.join("items");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("item.tiny"), r#"{"title":"Example"}"#).unwrap();
    let mut sources = SourceRegistry::builtin();
    sources
        .register("failing_query_reader", 1, "tiny", Arc::new(FailingReader))
        .unwrap();
    let repo = Repository::open_with_sources(
        &root,
        vec![CollectionDefinition::folder(
            "items",
            "items",
            SourceFormat::registered("failing_query_reader"),
        )],
        crate::repository::RepositoryOptions {
            snapshot_mode: crate::repository::SnapshotMode::Disabled,
            ..Default::default()
        },
        sources,
    )
    .unwrap();
    let query = Query {
        collections: vec!["items".into()],
        filter: None,
        fts: None,
        options: QueryOptions {
            sort: vec![SortField::computed(
                crate::workload::SortExpression {
                    path: "title".into(),
                    binding: Some("lower".into()),
                    reducer: None,
                },
                false,
            )],
            limit: 1,
            offset: 0,
            count: false,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let context = || {
        ExecutionContext::new(
            crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
            ExecutionLimits::default(),
            None,
        )
    };
    assert_eq!(
        repo.search_with_context(&query, &context())
            .unwrap_err()
            .kind,
        ExecutionErrorKind::Extension
    );
    let stream = repo
        .search_stream_scoped_with_context(
            &query,
            ResultScope::Page {
                offset: 0,
                limit: 1,
            },
            StreamOptions::default(),
            &context(),
            &mut |_| Ok(StreamControl::Continue),
        )
        .unwrap_err();
    assert_eq!(stream.kind(), ExecutionErrorKind::Extension, "{stream:?}");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn computed_sort_pages_match_reference_across_workers_and_file_layouts() {
    let expression = crate::workload::SortExpression {
        path: "title".into(),
        binding: Some("lower".into()),
        reducer: None,
    };
    for split_files in [false, true] {
        let root = std::env::temp_dir().join(format!(
            "objekte-computed-sort-workers-{}-{split_files}",
            std::process::id()
        ));
        let source = root.join("items");
        std::fs::create_dir_all(&source).unwrap();
        let rows = (0..48)
            .map(|index| {
                format!(
                    "{{\"title\":\"{}\",\"payload\":\"{}\"}}\n",
                    ["Zulu", "alpha", "ALPHA", "Äpfel", "beta"][index % 5],
                    "x".repeat(4096)
                )
            })
            .collect::<Vec<_>>();
        if split_files {
            for (index, row) in rows.iter().enumerate() {
                std::fs::write(source.join(format!("{index:03}.jsonl")), row).unwrap();
            }
        } else {
            std::fs::write(source.join("all.jsonl"), rows.concat()).unwrap();
        }
        for workers in [1, 2, 4, 8] {
            let repo = Repository::open_with_options(
                &root,
                vec![CollectionDefinition::folder(
                    "items",
                    "items",
                    SourceFormat::JsonLines,
                )],
                crate::repository::RepositoryOptions {
                    query_workers: workers,
                    snapshot_mode: crate::repository::SnapshotMode::Disabled,
                    ..Default::default()
                },
            )
            .unwrap();
            for return_mode in [ReturnMode::Locator, ReturnMode::Bytes, ReturnMode::Logical] {
                let query = Query {
                    collections: vec!["items".into()],
                    filter: None,
                    fts: None,
                    options: QueryOptions {
                        sort: vec![SortField::computed(expression.clone(), false)],
                        limit: 11,
                        offset: 7,
                        count: true,
                        return_mode,
                        evidence: false,
                    },
                };
                let expected = repo.search_reference(&query).unwrap();
                let actual = repo.search(&query).unwrap();
                assert_eq!(
                    actual.count, expected.count,
                    "split={split_files} workers={workers}"
                );
                assert_eq!(
                    actual.hits, expected.hits,
                    "split={split_files} workers={workers}"
                );
                assert_eq!(actual.counters.peak_retained_payload_bytes, 0);
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn computed_sort_large_record_uses_single_worker_reservation() {
    let root = std::env::temp_dir().join(format!(
        "objekte-computed-sort-large-{}",
        std::process::id()
    ));
    let source = root.join("items");
    std::fs::create_dir_all(&source).unwrap();
    for index in 0..8 {
        let payload = if index == 7 {
            "x".repeat(18 * 1024 * 1024)
        } else {
            "x".into()
        };
        let title = if index == 7 { "A" } else { "Z" };
        std::fs::write(
            source.join(format!("{index:03}.json")),
            format!("{{\"title\":\"{title}\",\"payload\":\"{payload}\"}}"),
        )
        .unwrap();
    }
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition::folder(
            "items",
            "items",
            SourceFormat::Json,
        )],
        crate::repository::RepositoryOptions {
            query_workers: 8,
            snapshot_mode: crate::repository::SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    let query = Query {
        collections: vec!["items".into()],
        filter: None,
        fts: None,
        options: QueryOptions {
            sort: vec![SortField::computed(
                crate::workload::SortExpression {
                    path: "title".into(),
                    binding: Some("lower".into()),
                    reducer: None,
                },
                false,
            )],
            limit: 1,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Logical,
            evidence: false,
        },
    };
    assert_eq!(
        repo.search(&query).unwrap().hits,
        repo.search_reference(&query).unwrap().hits
    );
    drop(repo);
    std::fs::remove_dir_all(root).unwrap();
}
