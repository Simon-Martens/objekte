use super::*;
use crate::index::bigram::BigramAlphabet;
use crate::model::{FieldPath, LogicalType};
use crate::query::options::{ReturnMode, SortExpression, SortField};
use crate::query::{CaseSensitivity, FtsMode, FtsQuery, Query, QueryOptions, parse_filter};
use crate::repository::Repository;
use crate::repository::configuration::{CollectionDefinition, IdentityPolicy, RepositoryOptions};
use crate::schema::{FieldDefinition, Schema};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

fn stream_test_hit(id: &str, payload_bytes: usize) -> SearchHit {
    SearchHit {
        collection: "data".into(),
        record_id: id.into(),
        generation: 1,
        file: Some(crate::files::RecordFileInfo {
            file_id: crate::model::FileId(1),
            path: PathBuf::from("data/all.rows"),
            revision: crate::inventory::SourceRevision {
                size: payload_bytes as u64,
                modified_ns: 0,
                object_id: None,
            },
            created_ns: None,
            status: crate::inventory::FileStatus::Valid,
            range: Some(crate::inventory::ByteRange {
                start: 0,
                end: payload_bytes as u64,
            }),
        }),
        matched_fields: Vec::new(),
        record_class: crate::inventory::RecordClass::Logical,
        diagnostic_ref: crate::inventory::DiagnosticRef(0),
        payload: Some(Payload::Bytes(vec![b'x'; payload_bytes])),
    }
}

fn limited_context(
    limits: ExecutionLimits,
    cancellation: Option<CancellationToken>,
) -> ExecutionContext {
    ExecutionContext::new(
        crate::query::clock::ClockContext::new(time::OffsetDateTime::now_utc()),
        limits,
        cancellation,
    )
}

#[test]
fn execution_context_shares_accounting_and_preserves_failure_kind() {
    let context = limited_context(
        ExecutionLimits {
            relation_edges: 2,
            ..Default::default()
        },
        None,
    );
    context.charge_edges(1).unwrap();
    context.clone().charge_edges(1).unwrap();
    let message = context.charge_edges(1).unwrap_err();
    assert_eq!(
        context.recorded_error(message).kind,
        ExecutionErrorKind::Resource
    );
    let other = limited_context(ExecutionLimits::default(), None);
    other.error(ExecutionErrorKind::Extension, "same message");
    other.error(ExecutionErrorKind::Resource, "same message");
    assert_eq!(
        other.recorded_error("unrelated".into()).kind,
        ExecutionErrorKind::Extension
    );
    let recovered = limited_context(ExecutionLimits::default(), None);
    recovered.fail(ExecutionErrorKind::SourceChanged, "stale source");
    recovered.clear_recovered_source_changed();
    recovered.fail(ExecutionErrorKind::Resource, "later budget failure");
    assert_eq!(
        recovered.recorded_error("fallback".into()).kind,
        ExecutionErrorKind::Resource
    );
    other.enter().unwrap();
    assert_eq!(
        other.enter().unwrap_err().kind,
        ExecutionErrorKind::InvalidQuery
    );
}

#[test]
fn source_read_limit_has_ordinary_and_stream_resource_class() {
    let fixture = fixture();
    let request = query("body ~ 'needle'", true, false, ReturnMode::Locator);
    let limits = ExecutionLimits {
        source_record_bytes: 1,
        ..Default::default()
    };
    let ordinary = fixture
        .repo
        .search_with_context(&request, &limited_context(limits, None))
        .unwrap_err();
    assert_eq!(ordinary.kind, ExecutionErrorKind::Resource);
    let stream = fixture.repo.search_stream_scoped_with_context(
        &request,
        ResultScope::Page {
            offset: 0,
            limit: request.options.limit,
        },
        StreamOptions::default(),
        &limited_context(limits, Some(CancellationToken::new())),
        &mut |_| Ok(StreamControl::Continue),
    );
    assert!(
        matches!(stream, Err(StreamError::Resource { .. })),
        "{stream:?}"
    );
}

#[test]
fn exact_posting_payload_obeys_source_limit_across_delivery_paths() {
    let fixture = fixture();
    let limits = ExecutionLimits {
        source_record_bytes: 1,
        ..Default::default()
    };
    for sorted in [false, true] {
        let request = query("kind = 'yes'", sorted, false, ReturnMode::Bytes);
        let error = fixture
            .repo
            .search_with_context(&request, &limited_context(limits, None))
            .unwrap_err();
        assert_eq!(error.kind, ExecutionErrorKind::Resource, "sorted={sorted}");
        for scope in [
            ResultScope::Page {
                offset: 0,
                limit: 3,
            },
            ResultScope::All { offset: 0 },
        ] {
            let error = fixture
                .repo
                .search_stream_scoped_with_context(
                    &request,
                    scope,
                    StreamOptions::default(),
                    &limited_context(limits, Some(CancellationToken::new())),
                    &mut |_| Ok(StreamControl::Continue),
                )
                .unwrap_err();
            assert_eq!(
                error.kind(),
                ExecutionErrorKind::Resource,
                "sorted={sorted}"
            );
        }
    }
}

#[test]
fn exact_posting_payload_uses_published_range_after_external_edit() {
    let fixture = fixture();
    let path = fixture.root.join("data/000.row");
    let edited = std::fs::read_to_string(&path)
        .unwrap()
        .replace("needle", "absent");
    std::fs::write(&path, edited.as_bytes()).unwrap();
    let request = query("kind = 'yes'", false, false, ReturnMode::Bytes);
    assert!(
        fixture
            .repo
            .search_with_context(&request, &limited_context(ExecutionLimits::default(), None))
            .is_err()
    );
}

#[test]
fn invalid_stream_bounds_have_typed_invalid_query_class() {
    let fixture = fixture();
    let request = query("body ~ 'needle'", false, false, ReturnMode::Locator);
    let error = fixture
        .repo
        .search_stream(
            &request,
            StreamOptions {
                max_event_bytes: 0,
                ..Default::default()
            },
            &CancellationToken::new(),
            &mut |_| Ok(StreamControl::Continue),
        )
        .unwrap_err();
    assert_eq!(error.kind(), ExecutionErrorKind::InvalidQuery);
}

#[test]
fn unresolved_stream_parameter_has_typed_invalid_query_class() {
    let fixture = fixture();
    let mut request = query("kind = 'yes'", false, false, ReturnMode::Locator);
    request.filter = Some(parse_filter("kind = $missing").unwrap());
    let error = fixture
        .repo
        .search_stream_with_parameters(
            &request,
            &Default::default(),
            StreamOptions::default(),
            &CancellationToken::new(),
            &mut |_| Ok(StreamControl::Continue),
        )
        .unwrap_err();
    assert_eq!(error.kind(), ExecutionErrorKind::InvalidQuery);
}

#[test]
fn invalid_sort_has_typed_invalid_query_class_in_ordinary_and_stream_search() {
    let fixture = fixture();
    let mut request = query("kind = 'yes'", false, false, ReturnMode::Locator);
    request.options.sort = vec![SortField::simple("title.", false)];
    let ordinary = fixture
        .repo
        .search_with_context(&request, &limited_context(ExecutionLimits::default(), None))
        .unwrap_err();
    assert_eq!(ordinary.kind, ExecutionErrorKind::InvalidQuery);
    let stream = fixture
        .repo
        .search_stream(
            &request,
            StreamOptions::default(),
            &CancellationToken::new(),
            &mut |_| Ok(StreamControl::Continue),
        )
        .unwrap_err();
    assert_eq!(stream.kind(), ExecutionErrorKind::InvalidQuery);
}

#[test]
fn externally_changed_candidate_is_verified_without_revision_probe() {
    let fixture = fixture();
    let path = fixture.root.join("data/000.row");
    let edited = std::fs::read_to_string(&path)
        .unwrap()
        .replace("needle", "absent");
    std::fs::write(&path, edited).unwrap();
    let request = query("body ~ 'needle'", true, false, ReturnMode::Locator);
    assert!(
        fixture
            .repo
            .search_with_context(&request, &limited_context(ExecutionLimits::default(), None))
            .is_err()
    );
    let stream = fixture.repo.search_stream_scoped_with_context(
        &request,
        ResultScope::Page {
            offset: 0,
            limit: request.options.limit,
        },
        StreamOptions::default(),
        &limited_context(ExecutionLimits::default(), Some(CancellationToken::new())),
        &mut |_| Ok(StreamControl::Continue),
    );
    assert!(stream.is_err(), "{stream:?}");
}

/// Temporary repository whose source files are removed after each test.
struct Fixture {
    root: PathBuf,
    repo: Repository,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).ok();
    }
}

fn fixture() -> Fixture {
    fixture_with_alphabet(BigramAlphabet::Ascii)
}

fn fixture_with_alphabet(alphabet: BigramAlphabet) -> Fixture {
    fixture_with_count(alphabet, 12)
}

fn fixture_with_count(alphabet: BigramAlphabet, count: usize) -> Fixture {
    let root = std::env::temp_dir().join(format!(
        "objekte-stream-{}-{}",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for n in 0..count {
        std::fs::write(
            root.join(format!("data/{n:03}.row")),
            format!(
                r#"("kind"="yes","body"="needle {n}","score"={})"#,
                11 - n as isize,
            ),
        )
        .unwrap();
    }
    let mut schema = Schema::default();
    for (path, kind, indexed) in [
        ("kind", LogicalType::String, true),
        ("body", LogicalType::String, false),
        ("score", LogicalType::Number, false),
    ] {
        schema
            .register_field(FieldDefinition {
                path: vec![path.into()],
                kind,
                required: true,
                nullable: false,
                indexed,
                multi: false,
                element_nullable: vec![],
            })
            .unwrap();
    }
    let definition = CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: crate::source::test_support::document_format(),
        identity: IdentityPolicy::Filename,
        discovery: Default::default(),
        schema,
        sort_fields: vec![
            FieldPath::parse("score").unwrap(),
            FieldPath::parse("kind").unwrap(),
        ],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    };
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition],
        RepositoryOptions {
            verified_cache_bytes: 1 << 20,
            query_workers: 4,
            bigram_alphabet: alphabet,
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    Fixture { root, repo }
}

fn residual_payload_fixture() -> Fixture {
    let root = std::env::temp_dir().join(format!(
        "objekte-stream-residual-payload-{}-{}",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for n in 0..4 {
        std::fs::write(
            root.join(format!("data/record-{n:03}-{}.row", "x".repeat(80))),
            format!(
                r#"("body"="{}","score"={n},"padding"="{}")"#,
                if n < 2 { "match" } else { "miss" },
                "x".repeat(1800)
            ),
        )
        .unwrap();
    }
    let mut schema = Schema::default();
    for (path, kind) in [
        ("body", LogicalType::String),
        ("score", LogicalType::Number),
        ("padding", LogicalType::String),
    ] {
        schema
            .register_field(FieldDefinition {
                path: vec![path.into()],
                kind,
                required: true,
                nullable: false,
                indexed: false,
                multi: false,
                element_nullable: vec![],
            })
            .unwrap();
    }
    let definition = CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: crate::source::test_support::document_format(),
        identity: IdentityPolicy::Filename,
        discovery: Default::default(),
        schema,
        sort_fields: vec![FieldPath::parse("score").unwrap()],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    };
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition],
        RepositoryOptions {
            verified_cache_bytes: 1024 * 1024,
            query_workers: 4,
            bigram_alphabet: BigramAlphabet::Ascii,
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    Fixture { root, repo }
}

fn cache_closed_fixture() -> Fixture {
    let root = std::env::temp_dir().join(format!(
        "objekte-stream-cache-closed-{}-{}",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for n in 0..20 {
        std::fs::write(
            root.join(format!("data/{n:03}.row")),
            format!(r#"("kind"="yes","score"={n})"#),
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
    schema
        .register_field(FieldDefinition {
            path: vec!["score".into()],
            kind: LogicalType::Number,
            required: true,
            nullable: false,
            indexed: false,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: crate::source::test_support::document_format(),
            identity: IdentityPolicy::Filename,
            discovery: Default::default(),
            schema,
            sort_fields: vec![FieldPath::parse("score").unwrap()],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }],
        RepositoryOptions {
            verified_cache_bytes: 512,
            query_workers: 4,
            bigram_alphabet: BigramAlphabet::Ascii,
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    Fixture { root, repo }
}

#[test]
fn all_scope_streams_more_than_fifty_rows_once_in_bounded_batches() {
    let fixture = fixture_with_count(BigramAlphabet::Ascii, 73);
    let mut request = query("body ~ 'needle'", false, false, ReturnMode::Bytes);
    request.options.limit = 2;
    let mut rows = Vec::new();
    let mut first_seen = None;
    let outcome = fixture
        .repo
        .search_stream_scoped(
            &request,
            ResultScope::All { offset: 3 },
            StreamOptions {
                batch_rows: 7,
                max_event_bytes: 8 * 1024,
                max_buffered_bytes: 16 * 1024,
                ..Default::default()
            },
            &CancellationToken::new(),
            &mut |event| {
                if let StreamEvent::OrderedRows {
                    start,
                    rows: batch,
                    progress,
                    ..
                } = event
                {
                    assert_eq!(start, rows.len());
                    assert!(batch.len() <= 7);
                    first_seen.get_or_insert(progress.observed_matches);
                    rows.extend(batch.iter().cloned());
                }
                Ok(StreamControl::Continue)
            },
        )
        .unwrap();
    let StreamOutcome::Complete(summary) = outcome else {
        panic!("expected complete")
    };
    assert_eq!(summary.count, Some(73));
    assert_eq!(summary.returned_rows, 70);
    assert_eq!(rows.len(), 70);
    assert_eq!(rows[0].record_id, "003");
    assert!(
        rows.iter()
            .all(|row| matches!(row.payload, Some(crate::query_results::Payload::Bytes(_))))
    );
    assert!(first_seen.unwrap() < 73);
    assert_eq!(summary.counters.scanned_records, 73);
    assert!(summary.progress.peak_buffered_bytes <= 16 * 1024);
}

#[test]
fn all_scope_computed_order_materializes_after_key_barrier() {
    let fixture = fixture_with_count(BigramAlphabet::Ascii, 65);
    let mut request = query("kind = 'yes'", false, false, ReturnMode::Bytes);
    request.options.sort = vec![SortField {
        field: "body".into(),
        descending: true,
        expression: None,
    }];
    let expected = {
        let mut finite = request.clone();
        finite.options.limit = 65;
        finite.options.count = true;
        fixture.repo.search(&finite).unwrap()
    };
    let mut ids = Vec::new();
    let outcome = fixture
        .repo
        .search_stream_scoped(
            &request,
            ResultScope::All { offset: 0 },
            StreamOptions {
                batch_rows: 5,
                ..Default::default()
            },
            &CancellationToken::new(),
            &mut |event| {
                if let StreamEvent::OrderedRows { start, rows, .. } = event {
                    assert_eq!(start, ids.len());
                    ids.extend(rows.iter().map(|row| row.record_id.clone()));
                    assert!(rows.iter().all(|row| row.payload.is_some()));
                }
                Ok(StreamControl::Continue)
            },
        )
        .unwrap();
    let StreamOutcome::Complete(summary) = outcome else {
        panic!("expected complete")
    };
    assert_eq!(summary.count, Some(65));
    assert_eq!(summary.returned_rows, 65);
    assert_eq!(
        ids,
        expected
            .hits
            .iter()
            .map(|hit| hit.record_id.clone())
            .collect::<Vec<_>>()
    );
}

#[test]
fn all_scope_huge_batch_row_hint_keeps_byte_bounded_delivery() {
    let fixture = fixture_with_count(BigramAlphabet::Ascii, 65);
    let mut request = query("kind = 'yes'", false, true, ReturnMode::Bytes);
    request.options.sort = vec![SortField {
        field: "body".into(),
        descending: false,
        expression: None,
    }];
    let mut events = 0;
    let mut returned = 0;
    let outcome = fixture
        .repo
        .search_stream_scoped(
            &request,
            ResultScope::All { offset: 0 },
            StreamOptions {
                batch_rows: usize::MAX,
                max_event_bytes: 2048,
                max_buffered_bytes: 256 * 1024,
                ..Default::default()
            },
            &CancellationToken::new(),
            &mut |event| {
                if let StreamEvent::OrderedRows { rows, .. } = event {
                    events += 1;
                    returned += rows.len();
                }
                Ok(StreamControl::Continue)
            },
        )
        .unwrap();
    let StreamOutcome::Complete(summary) = outcome else {
        panic!("expected complete")
    };
    assert_eq!(summary.count, Some(65));
    assert_eq!(summary.returned_rows, 65);
    assert_eq!(returned, 65);
    assert!(events > 1);
    assert!(summary.progress.peak_buffered_bytes <= 256 * 1024);
}

#[test]
fn scoped_page_zero_count_and_all_cancellation_are_distinct() {
    let fixture = fixture_with_count(BigramAlphabet::Ascii, 61);
    let mut request = query("kind = 'yes'", false, true, ReturnMode::Locator);
    request.options.limit = 4;
    let page = fixture
        .repo
        .search_stream_scoped(
            &request,
            ResultScope::Page {
                offset: 7,
                limit: 0,
            },
            StreamOptions::default(),
            &CancellationToken::new(),
            &mut |event| {
                assert!(!matches!(event, StreamEvent::OrderedRows { .. }));
                Ok(StreamControl::Continue)
            },
        )
        .unwrap();
    let StreamOutcome::Complete(page) = page else {
        panic!("expected complete")
    };
    assert_eq!(page.count, Some(61));
    assert_eq!(page.returned_rows, 0);

    let mut fts = request.clone();
    fts.filter = None;
    fts.fts = Some(FtsQuery {
        case_sensitivity: CaseSensitivity::Sensitive,
        mode: FtsMode::Literal,
        literal: "needle".into(),
        pattern_parameter: None,
        fields: Vec::new(),
    });
    let counted_fts = fixture
        .repo
        .search_stream_scoped(
            &fts,
            ResultScope::Page {
                offset: 0,
                limit: 0,
            },
            StreamOptions::default(),
            &CancellationToken::new(),
            &mut |_| Ok(StreamControl::Continue),
        )
        .unwrap();
    let StreamOutcome::Complete(counted_fts) = counted_fts else {
        panic!("expected complete")
    };
    assert_eq!(counted_fts.count, Some(61));
    assert_eq!(counted_fts.returned_rows, 0);

    let mut without_count = request.clone();
    without_count.options.count = false;
    let zero = fixture
        .repo
        .search_stream_scoped(
            &without_count,
            ResultScope::Page {
                offset: 7,
                limit: 0,
            },
            StreamOptions::default(),
            &CancellationToken::new(),
            &mut |_| Ok(StreamControl::Continue),
        )
        .unwrap();
    let StreamOutcome::Complete(zero) = zero else {
        panic!("expected complete")
    };
    assert_eq!(zero.count, None);
    assert_eq!(zero.returned_rows, 0);

    let beyond = fixture
        .repo
        .search_stream_scoped(
            &request,
            ResultScope::All { offset: 100 },
            StreamOptions::default(),
            &CancellationToken::new(),
            &mut |event| {
                assert!(!matches!(event, StreamEvent::OrderedRows { .. }));
                Ok(StreamControl::Continue)
            },
        )
        .unwrap();
    let StreamOutcome::Complete(beyond) = beyond else {
        panic!("expected complete")
    };
    assert_eq!(beyond.count, Some(61));
    assert_eq!(beyond.returned_rows, 0);

    let mut delivered = 0;
    let cancelled = fixture
        .repo
        .search_stream_scoped(
            &request,
            ResultScope::All { offset: 0 },
            StreamOptions {
                batch_rows: 4,
                ..Default::default()
            },
            &CancellationToken::new(),
            &mut |event| {
                if let StreamEvent::OrderedRows { rows, .. } = event {
                    delivered += rows.len();
                    return Ok(StreamControl::Cancel);
                }
                Ok(StreamControl::Continue)
            },
        )
        .unwrap();
    assert!(matches!(cancelled, StreamOutcome::Cancelled(_)));
    assert_eq!(delivered, 4);
    assert!(fixture.repo.search(&request).unwrap().count.is_some());

    let failed = fixture.repo.search_stream_scoped(
        &request,
        ResultScope::All { offset: 0 },
        StreamOptions {
            batch_rows: 4,
            ..Default::default()
        },
        &CancellationToken::new(),
        &mut |event| {
            if matches!(event, StreamEvent::OrderedRows { .. }) {
                Err("sink stopped".into())
            } else {
                Ok(StreamControl::Continue)
            }
        },
    );
    assert!(matches!(failed, Err(StreamError::Sink { .. })));
}

#[test]
fn insensitive_fts_with_offset_reconstructs_under_both_alphabets() {
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let fixture = fixture_with_alphabet(alphabet);
        let mut query = query("kind = 'yes'", false, true, ReturnMode::Logical);
        query.filter = None;
        query.fts = Some(FtsQuery {
            case_sensitivity: CaseSensitivity::Insensitive,
            mode: FtsMode::Literal,
            literal: "NEEDLE".into(),
            pattern_parameter: None,
            fields: vec![],
        });
        query.options.offset = 2;
        let expected = fixture.repo.search_reference(&query).unwrap();
        let mut page = Vec::new();
        let outcome = fixture
            .repo
            .search_stream(
                &query,
                StreamOptions {
                    batch_rows: 1,
                    ..Default::default()
                },
                &CancellationToken::new(),
                &mut |event| {
                    if let StreamEvent::OrderedRows { start, rows, .. } = event {
                        assert_eq!(start, page.len());
                        page.extend_from_slice(rows);
                    }
                    Ok(StreamControl::Continue)
                },
            )
            .unwrap();
        let StreamOutcome::Complete(summary) = outcome else {
            panic!("expected complete")
        };
        assert_eq!(summary.count, expected.count);
        assert_eq!(page, expected.hits);
    }
}

fn query(filter: &str, sorted: bool, count: bool, return_mode: ReturnMode) -> Query {
    Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(parse_filter(filter).unwrap()),
        options: QueryOptions {
            sort: if sorted {
                vec![
                    SortField {
                        field: "score".into(),
                        descending: false,
                        expression: None,
                    },
                    SortField {
                        field: "kind".into(),
                        descending: false,
                        expression: None,
                    },
                ]
            } else {
                Vec::new()
            },
            limit: 3,
            offset: 0,
            count,
            return_mode,
            evidence: true,
        },
    }
}

#[test]
fn ordered_rows_arrive_before_count_finishes_and_reconstruct_page() {
    let fixture = fixture();
    let query = query("kind = 'yes'", false, true, ReturnMode::Bytes);
    let expected = fixture.repo.search_reference(&query).unwrap();
    let caller = std::thread::current().id();
    let mut page = Vec::new();
    let mut first_observed = None;
    let outcome = fixture
        .repo
        .search_stream(
            &query,
            StreamOptions {
                batch_rows: 2,
                ..Default::default()
            },
            &CancellationToken::new(),
            &mut |event| {
                assert_eq!(caller, std::thread::current().id());
                if let StreamEvent::OrderedRows {
                    start,
                    rows,
                    progress,
                    ..
                } = event
                {
                    assert_eq!(start, page.len());
                    first_observed.get_or_insert(progress.observed_matches);
                    page.extend_from_slice(rows);
                }
                Ok(StreamControl::Continue)
            },
        )
        .unwrap();
    let StreamOutcome::Complete(summary) = outcome else {
        panic!("expected complete")
    };
    assert_eq!(summary.count, Some(12));
    assert!(
        first_observed.unwrap() < 12,
        "page must arrive before COUNT finishes"
    );
    assert_eq!(page, expected.hits);
    assert_eq!(summary.returned_rows, 3);
}

#[test]
fn ordered_search_and_stream_prepare_once_per_invocation() {
    let fixture = fixture();
    fixture.repo.configure_verified_cache(0).unwrap();
    let exact = query("kind = 'yes'", false, true, ReturnMode::Locator);
    let before = preparation_counts();
    let expected = fixture.repo.search(&exact).unwrap();
    assert_eq!(preparation_delta(before), (1, 1, 1));
    let repeated = fixture.repo.search(&exact).unwrap();
    assert_eq!(preparation_delta(before), (2, 2, 2));
    assert_eq!(repeated.hits, expected.hits);

    let before_stream = preparation_counts();
    let mut stream_query = exact.clone();
    stream_query.options.count = false;
    let outcome = fixture
        .repo
        .search_stream(
            &stream_query,
            StreamOptions {
                batch_rows: 2,
                ..Default::default()
            },
            &CancellationToken::new(),
            &mut |_| Ok(StreamControl::Continue),
        )
        .unwrap();
    assert!(matches!(outcome, StreamOutcome::Complete(_)));
    assert_eq!(preparation_delta(before_stream), (1, 1, 1));

    let residual = query("body ~ 'needle'", false, true, ReturnMode::Locator);
    let before_residual = preparation_counts();
    let outcome = fixture
        .repo
        .search_stream(
            &residual,
            StreamOptions::default(),
            &CancellationToken::new(),
            &mut |_| Ok(StreamControl::Continue),
        )
        .unwrap();
    assert!(matches!(outcome, StreamOutcome::Complete(_)));
    assert_eq!(preparation_delta(before_residual), (1, 1, 1));

    let declined = query("kind = 'yes'", true, false, ReturnMode::Locator);
    let before_declined = preparation_counts();
    let outcome = fixture
        .repo
        .search_stream(
            &declined,
            StreamOptions::default(),
            &CancellationToken::new(),
            &mut |_| Ok(StreamControl::Continue),
        )
        .unwrap();
    assert!(matches!(outcome, StreamOutcome::Complete(_)));
    assert_eq!(
        crate::query::prepared::production_prepare_calls() - before_declined.0,
        1
    );
}

fn residual_payload_query() -> Query {
    Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(parse_filter("body != 'miss'").unwrap()),
        options: QueryOptions {
            sort: vec![SortField::simple("score", false)],
            limit: 1,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Bytes,
            evidence: true,
        },
    }
}

#[test]
fn residual_count_keeps_page_charges_only_for_retained_modes() {
    let fixture = residual_payload_fixture();
    let query = residual_payload_query();
    let record_path = std::fs::read_dir(fixture.root.join("data"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .min()
        .unwrap();
    let record_bytes = std::fs::read(record_path).unwrap().len();
    let max_buffered_bytes = record_bytes * 2;
    let options = StreamOptions {
        max_buffered_bytes,
        max_event_bytes: record_bytes + 1024,
        ..Default::default()
    };

    fixture.repo.configure_verified_cache(1024 * 1024).unwrap();
    crate::query_execution::runner::reset_ordered_tail_observations();
    let mut page = Vec::new();
    let outcome = fixture
        .repo
        .search_stream(
            &query,
            options.clone(),
            &CancellationToken::new(),
            &mut |event| {
                if let StreamEvent::OrderedRows { rows, .. } = event {
                    page.extend_from_slice(rows);
                }
                Ok(StreamControl::Continue)
            },
        )
        .unwrap();
    let StreamOutcome::Complete(summary) = outcome else {
        panic!("ordered page did not complete")
    };
    let (tail_rows, tail_bytes, tail_reads) =
        crate::query_execution::runner::ordered_tail_observations();
    assert!(
        tail_reads >= 2,
        "tail observation: rows={tail_rows} bytes={tail_bytes} reads={tail_reads}; count={:?} page={}",
        summary.count,
        page.len()
    );
    assert_eq!(tail_rows, 0);
    assert_eq!(tail_bytes, 0);
    assert_eq!(summary.count, Some(2));
    assert_eq!(page.len(), 1);

    fixture.repo.configure_verified_cache(1024 * 1024).unwrap();
    crate::query_execution::runner::reset_ordered_tail_observations();
    let observation_budget = record_bytes * 3;
    let outcome = fixture.repo.search_stream(
        &query,
        StreamOptions {
            mode: StreamMode::PageReplacements,
            checkpoint_files: 1,
            max_buffered_bytes: observation_budget,
            max_event_bytes: record_bytes + 1024,
            ..Default::default()
        },
        &CancellationToken::new(),
        &mut |_| Ok(StreamControl::Continue),
    );
    let StreamOutcome::Complete(observed) = outcome.unwrap() else {
        panic!("observation replacement budget did not complete")
    };
    let (replacement_rows, replacement_bytes, replacement_reads) =
        crate::query_execution::runner::ordered_tail_observations();
    assert!(replacement_reads >= 1);
    assert_eq!(replacement_rows, 1);
    assert!(replacement_bytes > 0);
    let (_, page_component, identity_component, cursor_component, source_component) =
        crate::query_execution::runner::ordered_tail_components();
    assert!(identity_component > 0);
    assert!(cursor_component > 0);
    assert!(source_component > 0);
    let simultaneous = page_component + identity_component + cursor_component + source_component;
    assert!(observed.progress.peak_buffered_bytes >= simultaneous);
    let excluded_identity = page_component + cursor_component + source_component;
    let callback_boundary = simultaneous - 1;
    assert!(
        simultaneous > callback_boundary,
        "page={page_component} identity={identity_component} cursor={cursor_component} source={source_component} limit={callback_boundary}"
    );
    assert!(
        excluded_identity <= callback_boundary,
        "page={page_component} identity={identity_component} cursor={cursor_component} source={source_component} limit={callback_boundary}"
    );
    fixture.repo.configure_verified_cache(1024 * 1024).unwrap();
    crate::query_execution::runner::reset_ordered_tail_observations();
    let outcome = fixture.repo.search_stream(
        &query,
        StreamOptions {
            mode: StreamMode::PageReplacements,
            checkpoint_files: 1,
            max_buffered_bytes: callback_boundary,
            max_event_bytes: record_bytes + 1024,
            ..Default::default()
        },
        &CancellationToken::new(),
        &mut |_| Ok(StreamControl::Continue),
    );
    let Err(StreamError::Resource { progress, .. }) = outcome else {
        panic!("replacement page should retain its payload in the residual budget")
    };
    assert_eq!(progress.sequence, 1);
    assert_eq!(progress.emitted_rows, 1);
    let after_error = fixture.repo.search(&query).unwrap();
    assert!(!after_error.counters.verified_result_hit);

    fixture.repo.configure_verified_cache(1024 * 1024).unwrap();
    crate::query_execution::runner::reset_ordered_tail_observations();
    let sufficient = simultaneous + 1024;
    let callbacks = AtomicUsize::new(0);
    let outcome = fixture.repo.search_stream(
        &query,
        StreamOptions {
            mode: StreamMode::PageReplacements,
            checkpoint_files: 1,
            max_buffered_bytes: sufficient,
            max_event_bytes: record_bytes + 1024,
            ..Default::default()
        },
        &CancellationToken::new(),
        &mut |_| {
            callbacks.fetch_add(1, Ordering::Relaxed);
            Ok(StreamControl::Continue)
        },
    );
    let StreamOutcome::Complete(summary) = outcome.unwrap() else {
        panic!("sufficient replacement budget did not complete")
    };
    let (success_rows, success_page, success_identity, success_cursor, success_source) =
        crate::query_execution::runner::ordered_tail_components();
    assert_eq!(success_rows, 1);
    let success_components = success_page + success_identity + success_cursor + success_source;
    assert!(summary.progress.peak_buffered_bytes >= success_components);
    assert_eq!(summary.count, Some(2));
    assert!(callbacks.load(Ordering::Relaxed) >= 1);

    fixture.repo.configure_verified_cache(1024 * 1024).unwrap();
    crate::query_execution::runner::reset_ordered_tail_observations();
    let token = CancellationToken::new();
    let outcome = fixture
        .repo
        .search_stream(&query, options, &token, &mut |event| {
            if matches!(event, StreamEvent::OrderedRows { .. }) {
                token.cancel();
            }
            Ok(StreamControl::Continue)
        });
    assert!(matches!(outcome, Ok(StreamOutcome::Cancelled(_))));
    assert_eq!(
        crate::query_execution::runner::ordered_tail_observations().2,
        0
    );

    crate::query_execution::runner::reset_ordered_tail_observations();
    let owned = fixture.repo.search(&query).unwrap();
    let (owned_tail_rows, owned_tail_bytes, owned_tail_reads) =
        crate::query_execution::runner::ordered_tail_observations();
    assert!(owned_tail_reads >= 2);
    assert_eq!(owned_tail_rows, 0);
    assert_eq!(owned_tail_bytes, 0);
    assert_eq!(owned.count, Some(2));
    assert_eq!(owned.hits.len(), 1);
}

#[test]
fn replacement_callback_charges_old_and_new_identities_together() {
    let first_id = "A".repeat(160);
    let second_id = "B".repeat(160);
    let first = stream_test_hit(&first_id, 400);
    let second = stream_test_hit(&second_id, 500);
    let first_bytes = hit_bytes(&first);
    let second_bytes = hit_bytes(&second);
    let first_identity = identity_bytes([("data", first_id.as_str())]);
    let second_identity = identity_bytes([("data", second_id.as_str())]);
    let cursor_bytes = 300usize;
    let limit = (first_bytes + first_identity)
        .max(second_bytes + cursor_bytes)
        .max(cursor_bytes + first_identity + second_identity)
        + 1;
    assert!(second_bytes + cursor_bytes + first_identity + second_identity > limit);

    let token = CancellationToken::new();
    let callbacks = AtomicUsize::new(0);
    let mut sink = |_: StreamEvent<'_>| {
        callbacks.fetch_add(1, Ordering::Relaxed);
        Ok(StreamControl::Continue)
    };
    let mut delivery = Delivery::new(
        StreamOptions {
            mode: StreamMode::PageReplacements,
            max_buffered_bytes: limit,
            max_event_bytes: first_bytes.max(second_bytes) + 1,
            ..Default::default()
        },
        &token,
        &mut sink,
    );
    delivery
        .replacement(std::slice::from_ref(&first), &WorkCounters::default(), 0)
        .unwrap();
    let error = delivery
        .replacement(
            std::slice::from_ref(&second),
            &WorkCounters::default(),
            cursor_bytes,
        )
        .unwrap_err();
    assert_eq!(error, "stream memory budget exceeded");
    assert_eq!(callbacks.load(Ordering::Relaxed), 1);
    assert_eq!(delivery.last_page_sequence(), 1);
}

#[test]
fn ordered_successor_accounts_for_the_live_popped_key() {
    let root = std::env::temp_dir().join(format!(
        "objekte-ordered-live-key-{}-{}",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, prefix) in [("a", "01"), ("b", "02")] {
        std::fs::write(
            root.join(format!("data/{name}.row")),
            fixture_value!({ "flag" => (true), "sort" => (format!("{prefix}-{}", "x".repeat(20_000))) })
            .fixture_text(),
        )
        .unwrap();
    }
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["flag".into()],
            kind: LogicalType::Bool,
            required: true,
            nullable: false,
            indexed: false,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    schema
        .register_field(FieldDefinition {
            path: vec!["sort".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: crate::source::test_support::document_format(),
            identity: IdentityPolicy::Filename,
            discovery: Default::default(),
            schema,
            sort_fields: vec![FieldPath::parse("sort").unwrap()],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }],
        RepositoryOptions {
            verified_cache_bytes: 4 * 1024 * 1024,
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let query = Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(parse_filter("flag = true").unwrap()),
        options: QueryOptions {
            sort: vec![SortField::simple("sort", false)],
            limit: 1,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    };
    let sufficient = StreamOptions {
        max_event_bytes: 8 * 1024,
        max_buffered_bytes: 256 * 1024,
        ..Default::default()
    };
    crate::query_results::ranking::reset_segment_rank_key_constructions();
    crate::query_execution::runner::reset_ordered_key_observations();
    let mut rows = Vec::new();
    let outcome = repo
        .search_stream(
            &query,
            sufficient,
            &CancellationToken::new(),
            &mut |event| {
                if let StreamEvent::OrderedRows { rows: batch, .. } = event {
                    rows.extend_from_slice(batch);
                }
                Ok(StreamControl::Continue)
            },
        )
        .unwrap();
    let StreamOutcome::Complete(summary) = outcome else {
        panic!("sufficient live-key stream did not complete")
    };
    assert_eq!(summary.count, Some(2));
    assert_eq!(rows, repo.search_reference(&query).unwrap().hits);
    let (base, first_key, second_key) = crate::query_execution::runner::ordered_key_observations();
    assert!(first_key > 20_000);
    assert!(second_key > 20_000);
    assert!(base + first_key < base + first_key + second_key);
    let overlap = base + first_key + second_key;
    let boundary = overlap - 1;
    assert!(base + first_key <= boundary);
    assert!(base + second_key <= boundary);
    assert!(boundary >= 8 * 1024);
    assert!(summary.progress.peak_buffered_bytes >= overlap);
    let (constructions, last_key) = crate::query_results::ranking::segment_rank_key_constructions();
    assert_eq!(constructions, 2);
    let (segment_id, last_slot) = last_key;
    assert_eq!(last_slot, 1);

    repo.configure_verified_cache(0).unwrap();
    repo.configure_verified_cache(4 * 1024 * 1024).unwrap();
    let initial_callbacks = AtomicUsize::new(0);
    crate::query_results::ranking::reset_segment_rank_key_constructions();
    let outcome = repo.search_stream(
        &query,
        StreamOptions {
            max_event_bytes: 8 * 1024,
            max_buffered_bytes: base + first_key - 1,
            ..Default::default()
        },
        &CancellationToken::new(),
        &mut |_| {
            initial_callbacks.fetch_add(1, Ordering::Relaxed);
            Ok(StreamControl::Continue)
        },
    );
    let Err(StreamError::Resource { progress, .. }) = outcome else {
        panic!("initial key unexpectedly exceeded its budget without rejection")
    };
    assert_eq!(progress.sequence, 0);
    assert_eq!(progress.emitted_rows, 0);
    assert_eq!(initial_callbacks.load(Ordering::Relaxed), 0);
    assert_eq!(
        crate::query_results::ranking::segment_rank_key_constructions(),
        (0, (0, 0)),
        "the rejected initial key must not be materialized"
    );
    let after_initial_error = repo.search(&query).unwrap();
    assert!(!after_initial_error.counters.verified_result_hit);

    repo.configure_verified_cache(0).unwrap();
    repo.configure_verified_cache(4 * 1024 * 1024).unwrap();
    let callbacks = AtomicUsize::new(0);
    crate::query_results::ranking::reset_segment_rank_key_constructions();
    let outcome = repo.search_stream(
        &query,
        StreamOptions {
            max_event_bytes: 8 * 1024,
            max_buffered_bytes: boundary,
            ..Default::default()
        },
        &CancellationToken::new(),
        &mut |_| {
            callbacks.fetch_add(1, Ordering::Relaxed);
            Ok(StreamControl::Continue)
        },
    );
    let Err(StreamError::Resource { progress, .. }) = outcome else {
        panic!(
            "overlapping keys completed under a budget that fits only one: base={base} first={first_key} second={second_key} boundary={boundary}"
        )
    };
    assert_eq!(progress.sequence, 0);
    assert_eq!(progress.emitted_rows, 0);
    assert_eq!(callbacks.load(Ordering::Relaxed), 0);
    assert_eq!(
        crate::query_results::ranking::segment_rank_key_constructions(),
        (1, (segment_id, 0)),
        "only the admitted initial key may be materialized"
    );
    let after_error = repo.search(&query).unwrap();
    assert!(!after_error.counters.verified_result_hit);
    assert_eq!(
        after_error.hits,
        repo.search_reference(&query).unwrap().hits
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn ordered_cache_admission_stops_cache_only_work_after_rejection() {
    let fixture = cache_closed_fixture();
    fixture.repo.configure_verified_cache(512).unwrap();
    let query = Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(parse_filter("kind = 'yes'").unwrap()),
        options: QueryOptions {
            sort: Vec::new(),
            limit: 1,
            offset: 10,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    };
    crate::query_execution::runner::reset_cache_closed_work();
    let result = fixture.repo.search(&query).unwrap();
    let (closed_candidates, closed_keys, closed_members, closed_evidence) =
        crate::query_execution::runner::cache_closed_work();
    assert!(closed_candidates > 0);
    assert_eq!((closed_keys, closed_members, closed_evidence), (0, 0, 0));
    assert_eq!(result.count, Some(20));
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].matched_fields, ["kind"]);
    assert_eq!(result.counters.files_opened, 0);
    assert_eq!(result.counters.bytes_read, 0);
    assert_eq!(result.counters.record_parses, 0);

    let mut later = query.clone();
    later.options.offset = 11;
    later.options.limit = 2;
    let later = fixture.repo.search(&later).unwrap();
    assert!(!later.counters.verified_result_hit);
    assert_eq!(later.count, Some(20));
    assert!(later.hits.iter().all(|hit| hit.matched_fields == ["kind"]));

    fixture.repo.configure_verified_cache(1024 * 1024).unwrap();
    let cold = fixture.repo.search(&query).unwrap();
    assert!(!cold.counters.verified_result_hit);
    let warm = fixture.repo.search(&query).unwrap();
    assert!(warm.counters.verified_result_hit);
    assert_eq!(warm.count, cold.count);
    assert_eq!(warm.hits, cold.hits);
    assert_eq!(warm.hits[0].matched_fields, ["kind"]);
}

fn preparation_counts() -> (usize, usize, usize) {
    (
        crate::query::prepared::production_prepare_calls(),
        crate::query::sort_binding::production_bound_sort_calls(),
        crate::query_execution::restrictions::predicate_bounds_calls(),
    )
}

fn preparation_delta(before: (usize, usize, usize)) -> (usize, usize, usize) {
    let after = preparation_counts();
    (after.0 - before.0, after.1 - before.1, after.2 - before.2)
}

#[test]
fn replacements_correct_late_better_sort_key_and_preserve_payloads() {
    let fixture = fixture();
    let query = query("body ~ 'needle'", true, true, ReturnMode::Logical);
    let expected = fixture.repo.search_reference(&query).unwrap();
    let mut snapshots = Vec::new();
    let outcome = fixture
        .repo
        .search_stream(
            &query,
            StreamOptions {
                mode: StreamMode::PageReplacements,
                checkpoint_files: 1,
                ..Default::default()
            },
            &CancellationToken::new(),
            &mut |event| {
                if let StreamEvent::PageReplacement { rows, .. } = event {
                    assert!(
                        rows.iter()
                            .all(|hit| !hit.matched_fields.is_empty() && hit.payload.is_some())
                    );
                    snapshots.push(rows.to_vec());
                }
                Ok(StreamControl::Continue)
            },
        )
        .unwrap();
    let StreamOutcome::Complete(summary) = outcome else {
        panic!("expected complete")
    };
    assert_eq!(summary.count, Some(12));
    assert!(!snapshots.is_empty(), "the final page must be delivered");
    assert_eq!(snapshots.last().unwrap(), &expected.hits);

    let peak = summary.progress.peak_buffered_bytes;
    assert!(peak > 0);
    for (limit, expect_success) in [(peak, true), (peak.saturating_sub(1), false)] {
        let fixture = crate::query_execution::streaming::tests::fixture();
        let outcome = fixture.repo.search_stream(
            &query,
            StreamOptions {
                mode: StreamMode::PageReplacements,
                checkpoint_files: 1,
                max_event_bytes: limit,
                max_buffered_bytes: limit,
                ..Default::default()
            },
            &CancellationToken::new(),
            &mut |_| Ok(StreamControl::Continue),
        );
        if expect_success {
            assert!(matches!(outcome, Ok(StreamOutcome::Complete(_))));
        } else {
            assert!(matches!(outcome, Err(StreamError::Resource { .. })));
        }
    }
}

#[test]
fn computed_sort_stream_reranks_warm_membership_and_preserves_page() {
    let fixture = fixture();
    let mut query = query("body ~ 'needle'", false, true, ReturnMode::Logical);
    query.options.sort = vec![SortField::computed(
        SortExpression {
            path: "body".into(),
            binding: None,
            reducer: None,
        },
        false,
    )];
    query.options.offset = 2;
    let expected = fixture.repo.search_reference(&query).unwrap();
    for warm in [false, true] {
        if warm {
            let collected = fixture.repo.search(&query).unwrap();
            assert_eq!(collected.hits, expected.hits);
        }
        let mut page = Vec::new();
        let outcome = fixture
            .repo
            .search_stream(
                &query,
                StreamOptions {
                    batch_rows: 1,
                    ..Default::default()
                },
                &CancellationToken::new(),
                &mut |event| {
                    if let StreamEvent::OrderedRows { start, rows, .. } = event {
                        assert_eq!(start, page.len());
                        page.extend_from_slice(rows);
                    }
                    Ok(StreamControl::Continue)
                },
            )
            .unwrap();
        let StreamOutcome::Complete(summary) = outcome else {
            panic!("expected completion")
        };
        assert_eq!(summary.count, expected.count);
        assert_eq!(page, expected.hits);
        if warm {
            assert!(summary.counters.verified_result_hit);
            assert_eq!(
                summary.progress.observed_matches,
                expected.count.unwrap() as u64
            );
        }
    }
}

#[test]
fn cancellation_and_sink_failure_do_not_publish_verified_membership() {
    for fail_sink in [false, true] {
        let fixture = fixture();
        let query = query("body ~ 'needle'", true, true, ReturnMode::Locator);
        let token = CancellationToken::new();
        let mut calls = 0;
        let outcome = fixture.repo.search_stream(
            &query,
            StreamOptions {
                mode: StreamMode::PageReplacements,
                checkpoint_files: 1,
                ..Default::default()
            },
            &token,
            &mut |event| {
                if matches!(event, StreamEvent::PageReplacement { .. }) {
                    calls += 1;
                    if fail_sink {
                        return Err("sink stopped".into());
                    }
                    token.cancel();
                }
                Ok(StreamControl::Continue)
            },
        );
        assert_eq!(calls, 1);
        if fail_sink {
            let Err(StreamError::Sink { progress, .. }) = outcome else {
                panic!("expected sink error")
            };
            assert_eq!(progress.sequence, 0);
            assert_eq!(progress.emitted_rows, 0);
        } else {
            let Ok(StreamOutcome::Cancelled(progress)) = outcome else {
                panic!("expected cancellation")
            };
            assert_eq!(progress.sequence, 1);
            assert!(progress.emitted_rows > 0);
        }
        assert!(
            !fixture
                .repo
                .search(&query)
                .unwrap()
                .counters
                .verified_result_hit
        );
    }
}

#[test]
fn cancellation_before_work_and_late_source_error_remain_distinct() {
    let fixture = fixture();
    let query = query("body ~ 'needle'", false, false, ReturnMode::Locator);
    let token = CancellationToken::new();
    token.cancel();
    let mut calls = 0;
    assert!(matches!(
        fixture
            .repo
            .search_stream(&query, StreamOptions::default(), &token, &mut |_| {
                calls += 1;
                Ok(StreamControl::Continue)
            }),
        Ok(StreamOutcome::Cancelled(_))
    ));
    let mut unresolved = query.clone();
    unresolved.filter = Some(parse_filter("kind = $missing").unwrap());
    assert!(matches!(
        fixture.repo.search_stream_with_parameters(
            &unresolved,
            &Default::default(),
            StreamOptions::default(),
            &token,
            &mut |_| {
                calls += 1;
                Ok(StreamControl::Continue)
            }
        ),
        Ok(StreamOutcome::Cancelled(_))
    ));
    assert_eq!(calls, 0);

    let mut first = true;
    let outcome = fixture.repo.search_stream(
        &query,
        StreamOptions {
            mode: StreamMode::PageReplacements,
            checkpoint_files: 1,
            ..Default::default()
        },
        &CancellationToken::new(),
        &mut |event| {
            if first && matches!(event, StreamEvent::PageReplacement { .. }) {
                first = false;
                std::fs::remove_file(fixture.root.join("data/011.row")).unwrap();
            }
            Ok(StreamControl::Continue)
        },
    );
    assert!(!first, "at least one verified page was delivered");
    assert!(
        matches!(outcome, Err(StreamError::Execution { .. })),
        "{outcome:?}"
    );
}

#[test]
fn zero_limit_and_event_budget_have_explicit_outcomes() {
    let fixture = fixture();
    let mut query = query("kind = 'yes'", false, true, ReturnMode::Locator);
    query.options.limit = 0;
    let mut data_events = 0;
    let outcome = fixture
        .repo
        .search_stream(
            &query,
            StreamOptions::default(),
            &CancellationToken::new(),
            &mut |event| {
                if !matches!(event, StreamEvent::Progress { .. }) {
                    data_events += 1;
                }
                Ok(StreamControl::Continue)
            },
        )
        .unwrap();
    assert_eq!(data_events, 0);
    let StreamOutcome::Complete(summary) = outcome else {
        panic!("expected complete")
    };
    assert_eq!(summary.count, Some(12));
    assert_eq!(summary.returned_rows, 0);

    query.options.limit = 3;
    let outcome = fixture.repo.search_stream(
        &query,
        StreamOptions {
            max_event_bytes: 1,
            max_buffered_bytes: 1 << 20,
            ..Default::default()
        },
        &CancellationToken::new(),
        &mut |_| Ok(StreamControl::Continue),
    );
    assert!(matches!(outcome, Err(StreamError::Resource { .. })));

    let mut scanning = self::query("body ~ 'needle'", true, true, ReturnMode::Locator);
    scanning.options.limit = 0;
    let mut progress_events = 0;
    let outcome = fixture
        .repo
        .search_stream(
            &scanning,
            StreamOptions {
                mode: StreamMode::PageReplacements,
                checkpoint_files: 1,
                ..Default::default()
            },
            &CancellationToken::new(),
            &mut |event| {
                assert!(matches!(event, StreamEvent::Progress { .. }));
                progress_events += 1;
                Ok(StreamControl::Continue)
            },
        )
        .unwrap();
    let StreamOutcome::Complete(summary) = outcome else {
        panic!("expected complete")
    };
    assert!(progress_events > 0);
    assert!(summary.progress.sequence > 0);
    assert_eq!(summary.final_sequence, 0, "progress is not a page");
}

#[test]
fn slow_sink_applies_backpressure_with_multiple_workers() {
    let fixture = fixture();
    let query = query("body ~ 'needle'", true, true, ReturnMode::Bytes);
    let (seen_tx, seen_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::sync_channel(0);
    std::thread::scope(|scope| {
        let repo = &fixture.repo;
        let query = &query;
        let worker = scope.spawn(move || {
            repo.search_stream(
                query,
                StreamOptions {
                    mode: StreamMode::PageReplacements,
                    checkpoint_files: 4,
                    max_event_bytes: 16 * 1024,
                    max_buffered_bytes: 64 * 1024,
                    ..Default::default()
                },
                &CancellationToken::new(),
                &mut |event| {
                    if let StreamEvent::PageReplacement {
                        sequence, progress, ..
                    } = event
                    {
                        assert!(progress.peak_buffered_bytes <= 64 * 1024);
                        seen_tx.send(sequence).unwrap();
                        resume_rx.recv().unwrap();
                    }
                    Ok(StreamControl::Continue)
                },
            )
        });
        let first = seen_rx.recv().unwrap();
        assert_eq!(first, 1);
        // The coordinator cannot deliver another event until this callback
        // returns. No unbounded producer queue is available behind the sink.
        assert!(seen_rx.try_recv().is_err());
        resume_tx.send(()).unwrap();
        while seen_rx.recv().is_ok() {
            resume_tx.send(()).unwrap();
        }
        let outcome = worker.join().unwrap().unwrap();
        let StreamOutcome::Complete(summary) = outcome else {
            panic!("expected complete")
        };
        assert!(summary.progress.peak_buffered_bytes <= 64 * 1024);
    });
}

#[test]
fn warm_membership_cache_streams_page_without_holding_cache_lock() {
    let fixture = fixture();
    let query = query("kind = 'yes'", false, true, ReturnMode::Bytes);
    let expected = fixture.repo.search(&query).unwrap();
    assert!(
        fixture
            .repo
            .search(&query)
            .unwrap()
            .counters
            .verified_result_hit
    );
    let mut page = Vec::new();
    let outcome = fixture
        .repo
        .search_stream(
            &query,
            StreamOptions {
                batch_rows: 1,
                ..Default::default()
            },
            &CancellationToken::new(),
            &mut |event| {
                if let StreamEvent::OrderedRows { rows, .. } = event {
                    assert!(
                        fixture
                            .repo
                            .search(&query)
                            .unwrap()
                            .counters
                            .verified_result_hit
                    );
                    page.extend_from_slice(rows);
                }
                Ok(StreamControl::Continue)
            },
        )
        .unwrap();
    let StreamOutcome::Complete(summary) = outcome else {
        panic!("expected complete")
    };
    assert!(summary.counters.verified_result_hit);
    assert_eq!(summary.count, expected.count);
    assert_eq!(page, expected.hits);
}

#[test]
fn warm_cache_cancellation_stops_before_loading_next_page_record() {
    let fixture = fixture();
    let query = query("kind = 'yes'", false, true, ReturnMode::Logical);
    fixture.repo.search(&query).unwrap();
    assert!(
        fixture
            .repo
            .search(&query)
            .unwrap()
            .counters
            .verified_result_hit
    );
    let token = CancellationToken::new();
    let mut rows = 0;
    let outcome = fixture.repo.search_stream(
        &query,
        StreamOptions {
            batch_rows: 1,
            ..Default::default()
        },
        &token,
        &mut |event| {
            if let StreamEvent::OrderedRows { rows: batch, .. } = event {
                rows += batch.len();
                token.cancel();
            }
            Ok(StreamControl::Continue)
        },
    );
    assert_eq!(rows, 1);
    assert!(matches!(outcome, Ok(StreamOutcome::Cancelled(_))));
}

#[test]
fn cancellation_on_final_page_delivery_prevents_cache_publication() {
    let fixture = fixture();
    let query = query("kind = 'yes'", true, true, ReturnMode::Locator);
    let token = CancellationToken::new();
    let mut delivered = 0;
    let outcome = fixture.repo.search_stream(
        &query,
        StreamOptions {
            mode: StreamMode::PageReplacements,
            checkpoint_files: usize::MAX,
            ..Default::default()
        },
        &token,
        &mut |event| {
            if let StreamEvent::PageReplacement { rows, .. } = event {
                assert_eq!(rows.len(), 3);
                delivered += 1;
                token.cancel();
            }
            Ok(StreamControl::Continue)
        },
    );
    assert_eq!(delivered, 1);
    assert!(matches!(outcome, Ok(StreamOutcome::Cancelled(_))));
    assert!(
        !fixture
            .repo
            .search(&query)
            .unwrap()
            .counters
            .verified_result_hit
    );
}

#[test]
fn stored_order_cursor_merges_collection_ties_before_delivery() {
    let root = std::env::temp_dir().join(format!(
        "objekte-stream-ties-{}-{}",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    ));
    for collection in ["a", "b"] {
        std::fs::create_dir_all(root.join(collection)).unwrap();
        for (n, score) in [0, 0, 1].into_iter().enumerate() {
            std::fs::write(
                root.join(format!("{collection}/{n:03}.row")),
                format!(r#"("score"={score})"#),
            )
            .unwrap();
        }
    }
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["score".into()],
            kind: LogicalType::Number,
            required: true,
            nullable: false,
            indexed: false,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let definitions = ["a", "b"].map(|name| CollectionDefinition {
        name: name.into(),
        source_path: name.into(),
        format: crate::source::test_support::document_format(),
        identity: IdentityPolicy::Filename,
        discovery: Default::default(),
        schema: schema.clone(),
        sort_fields: vec![FieldPath::parse("score").unwrap()],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    });
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        definitions.to_vec(),
        RepositoryOptions {
            verified_cache_bytes: 0,
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let query = Query {
        collections: vec![],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: vec![SortField {
                field: "score".into(),
                descending: false,
                expression: None,
            }],
            limit: 4,
            offset: 1,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let mut page = Vec::new();
    let outcome = repo
        .search_stream(
            &query,
            StreamOptions {
                batch_rows: 2,
                ..Default::default()
            },
            &CancellationToken::new(),
            &mut |event| {
                if let StreamEvent::OrderedRows { start, rows, .. } = event {
                    assert_eq!(start, page.len());
                    page.extend(
                        rows.iter()
                            .map(|hit| (hit.collection.clone(), hit.record_id.clone())),
                    );
                }
                Ok(StreamControl::Continue)
            },
        )
        .unwrap();
    let StreamOutcome::Complete(summary) = outcome else {
        panic!("expected complete")
    };
    assert_eq!(summary.count, Some(6));
    assert_eq!(
        page,
        [("a", "001"), ("b", "000"), ("b", "001"), ("a", "002")]
            .map(|(collection, id)| (collection.to_string(), id.to_string()))
    );
    std::fs::remove_dir_all(root).unwrap();
}

use crate::source::test_support::{FixtureText, fixture_value};
