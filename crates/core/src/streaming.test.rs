use super::*;
use crate::model::{FieldPath, LogicalType};
use crate::query::{CaseSensitivity, FtsMode, FtsQuery, Query, QueryOptions, parse_filter};
use crate::reference::{FieldDefinition, Schema};
use crate::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions,
};
use crate::source::SourceFormat;
use crate::workload::{BigramAlphabet, ReturnMode, SortExpression, SortField};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

fn limited_context(
    limits: ExecutionLimits,
    cancellation: Option<CancellationToken>,
) -> ExecutionContext {
    ExecutionContext::new(
        crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
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
    let path = fixture.root.join("data/000.json");
    let edited = std::fs::read_to_string(&path)
        .unwrap()
        .replace("needle", "absent");
    std::fs::write(&path, edited.as_bytes()).unwrap();
    let request = query("kind = 'yes'", false, false, ReturnMode::Bytes);
    let result = fixture
        .repo
        .search_with_context(&request, &limited_context(ExecutionLimits::default(), None))
        .unwrap();
    assert!(result.hits.iter().any(|hit| {
        hit.record_id == "000"
            && matches!(&hit.payload, Some(crate::repository::Payload::Bytes(bytes)) if bytes == edited.as_bytes())
    }));
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
    let path = fixture.root.join("data/000.json");
    let edited = std::fs::read_to_string(&path)
        .unwrap()
        .replace("needle", "absent");
    std::fs::write(&path, edited).unwrap();
    let request = query("body ~ 'needle'", true, false, ReturnMode::Locator);
    let ordinary = fixture
        .repo
        .search_with_context(&request, &limited_context(ExecutionLimits::default(), None))
        .unwrap();
    assert!(ordinary.hits.iter().all(|hit| hit.record_id != "000"));
    let stream = fixture
        .repo
        .search_stream_scoped_with_context(
            &request,
            ResultScope::Page {
                offset: 0,
                limit: request.options.limit,
            },
            StreamOptions::default(),
            &limited_context(ExecutionLimits::default(), Some(CancellationToken::new())),
            &mut |_| Ok(StreamControl::Continue),
        )
        .unwrap();
    assert!(matches!(stream, StreamOutcome::Complete(_)), "{stream:?}");
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
            root.join(format!("data/{n:03}.json")),
            format!(
                r#"{{"kind":"yes","body":"needle {n}","score":{}}}"#,
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
        format: SourceFormat::Json,
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
    let repo = Repository::open_with_options(
        &root,
        vec![definition],
        RepositoryOptions {
            verified_cache_bytes: 1 << 20,
            query_workers: 4,
            bigram_alphabet: alphabet,
            ..Default::default()
        },
    )
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
            .all(|row| matches!(row.payload, Some(crate::repository::Payload::Bytes(_))))
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
    assert!(
        snapshots.len() > 1,
        "later better keys must replace the first page"
    );
    assert_ne!(snapshots[0], expected.hits);
    assert_eq!(snapshots.last().unwrap(), &expected.hits);
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
    let query = query("body ~ 'needle'", true, false, ReturnMode::Locator);
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
                std::fs::remove_file(fixture.root.join("data/011.json")).unwrap();
            }
            Ok(StreamControl::Continue)
        },
    );
    assert!(!first, "at least one verified page was delivered");
    assert!(matches!(outcome, Err(StreamError::Execution { .. })));
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
                root.join(format!("{collection}/{n:03}.json")),
                format!(r#"{{"score":{score}}}"#),
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
        format: SourceFormat::Json,
        identity: IdentityPolicy::Filename,
        discovery: Default::default(),
        schema: schema.clone(),
        sort_fields: vec![FieldPath::parse("score").unwrap()],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    });
    let repo = Repository::open_with_options(
        &root,
        definitions.to_vec(),
        RepositoryOptions {
            verified_cache_bytes: 0,
            ..Default::default()
        },
    )
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
