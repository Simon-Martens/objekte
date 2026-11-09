use super::*;
use crate::backend::test_support::fixture;

#[test]
fn desktop_page_reuses_verified_order_and_keeps_pinned_revision() {
    let fixture = fixture();
    let view = fixture.session.repository.index().view().unwrap();
    let spec = QuerySpec {
        full_text: "a".into(),
        filter: "year >= 2022 SORT BY year DESC".into(),
        count: true,
        ..QuerySpec::default()
    };
    let token = CancellationToken::new();
    let cold = query_page_on_view(&fixture.session, &view, &spec, 0, 1, &token).unwrap();
    assert_eq!(cold.count, Some(3));
    assert!(!cold.counters.verified_result_hit);
    let warm = query_page_on_view(&fixture.session, &view, &spec, 0, 1, &token).unwrap();
    assert!(warm.counters.verified_result_hit);
    assert_eq!(warm.hits[0].record_id, cold.hits[0].record_id);
    assert_eq!(warm.revision, view.revision());
    assert!(warm.counters.scanned_records < cold.counters.scanned_records);
    let later = query_page_on_view(
        &fixture.session,
        &view,
        &QuerySpec {
            count: false,
            ..spec
        },
        1,
        1,
        &token,
    )
    .unwrap();
    assert!(later.counters.verified_result_hit);
    assert_eq!(later.hits.len(), 1);
}

#[test]
#[ignore = "manual desktop backend cache timing and work-counter measurement"]
fn measure_desktop_lifecycle_query_cache() {
    use crate::backend::{OpenSpec, open};
    use objekte_cli::arguments::Arguments;

    let root = tempfile::tempdir().unwrap();
    for n in 0..2_000 {
        std::fs::write(
            root.path().join(format!("{n:05}.json")),
            format!(r#"{{"title":"needle {n}","year":2024}}"#),
        )
        .unwrap();
    }
    let session = open(
        &OpenSpec {
            args: Arguments {
                path: vec![root.path().to_path_buf()],
                ..Default::default()
            },
            cwd: root.path().to_path_buf(),
            configured_cache_policy: None,
        },
        &CancellationToken::new(),
    )
    .unwrap();
    let view = session.repository.index().view().unwrap();
    let spec = QuerySpec {
        full_text: "needle".into(),
        filter: "year >= 2024 SORT BY title ASC".into(),
        count: true,
        ..QuerySpec::default()
    };
    let mut samples = Vec::new();
    for _ in 0..7 {
        let page =
            query_page_on_view(&session, &view, &spec, 0, 100, &CancellationToken::new()).unwrap();
        assert_eq!(page.count, Some(2_000));
        samples.push((
            page.elapsed_ns,
            page.counters.verified_result_hit,
            page.counters.scanned_records,
        ));
    }
    eprintln!("desktop lifecycle cache samples (elapsed ns, hit, scanned): {samples:?}");
    assert!(!samples[0].1);
    assert!(samples[1..].iter().all(|sample| sample.1 && sample.2 == 0));
}

#[test]
fn hundred_row_continuations_keep_equal_sort_keys_unique_at_boundaries() {
    use crate::backend::{OpenSpec, open};
    use objekte_cli::arguments::Arguments;
    use std::collections::HashSet;

    let root = tempfile::tempdir().unwrap();
    for n in 0..1001 {
        std::fs::write(
            root.path().join(format!("{n:04}.json")),
            format!(r#"{{"n":{n},"bucket":"same"}}"#),
        )
        .unwrap();
    }
    let session = open(
        &OpenSpec {
            args: Arguments {
                path: vec![root.path().to_path_buf()],
                ..Default::default()
            },
            cwd: root.path().to_path_buf(),
            configured_cache_policy: None,
        },
        &CancellationToken::new(),
    )
    .unwrap();
    for count in [0, 1, 99, 100, 101, 200, 201, 1000] {
        let spec = QuerySpec {
            filter: format!("n < {count} SORT BY bucket ASC"),
            ..QuerySpec::default()
        };
        let mut seen = HashSet::new();
        let mut next = Some(0);
        let mut requests = 0;
        while let Some(start) = next {
            requests += 1;
            assert!(requests <= 12, "continuation must make progress");
            let page = query_page(&session, &spec, start, 100, &CancellationToken::new()).unwrap();
            for hit in page.hits {
                assert!(seen.insert((hit.collection, hit.record_id)));
            }
            next = page.next_page_start;
        }
        assert_eq!(seen.len(), count, "wrong result count for {count}");
        assert_eq!(
            std::fs::read_to_string(root.path().join("0000.json")).unwrap(),
            r#"{"n":0,"bucket":"same"}"#
        );
    }
}

#[test]
fn combines_full_text_filter_and_case_options() {
    let fixture = fixture();
    let spec = QuerySpec {
        full_text: "Alpha".into(),
        filter: "year >= 2022".into(),
        count: true,
        ..QuerySpec::default()
    };
    let page = query_page(&fixture.session, &spec, 0, 10, &CancellationToken::new()).unwrap();
    assert_eq!(page.count, Some(1));
    assert_eq!(page.hits[0].record_id, "path:a.json");
    let insensitive = QuerySpec {
        case_sensitive: false,
        ..spec
    };
    assert_eq!(
        count_exact(&fixture.session, &insensitive, &CancellationToken::new()).unwrap(),
        2
    );
}

#[test]
fn viewport_pages_respect_range_without_becoming_total_limit() {
    let fixture = fixture();
    let spec = QuerySpec {
        offset: 1,
        limit: Some(2),
        count: true,
        ..QuerySpec::default()
    };
    let first = query_page(&fixture.session, &spec, 0, 1, &CancellationToken::new()).unwrap();
    let second = query_page(&fixture.session, &spec, 1, 1, &CancellationToken::new()).unwrap();
    assert_eq!(first.count, Some(3));
    assert_eq!(first.hits.len(), 1);
    assert_eq!(first.next_page_start, Some(1));
    assert_eq!(second.hits.len(), 1);
    assert_eq!(second.next_page_start, None);
    let zero = QuerySpec {
        offset: 0,
        limit: Some(0),
        count: true,
        ..QuerySpec::default()
    };
    let empty = query_page(&fixture.session, &zero, 0, 10, &CancellationToken::new()).unwrap();
    assert!(empty.hits.is_empty());
    assert_eq!(empty.count, Some(3));
    assert_eq!(empty.next_page_start, None);
}

#[test]
fn inline_sort_and_cancelled_work_are_reported() {
    let fixture = fixture();
    let spec = QuerySpec {
        filter: "year >= 2022 SORT BY year DESC".into(),
        ..QuerySpec::default()
    };
    assert_eq!(build_query(&spec).unwrap().options.sort.len(), 1);
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert_eq!(
        query_page(&fixture.session, &QuerySpec::default(), 0, 10, &cancelled)
            .unwrap_err()
            .kind,
        BackendErrorKind::Cancelled
    );
    assert_eq!(
        query_page(
            &fixture.session,
            &QuerySpec::default(),
            0,
            MAX_PAGE_ROWS + 1,
            &CancellationToken::new()
        )
        .unwrap_err()
        .kind,
        BackendErrorKind::Options
    );
}

#[test]
fn pinned_pages_and_counts_do_not_mix_after_a_publication() {
    let fixture = fixture();
    let index = fixture.session.repository.index();
    let old_view = index.view().unwrap();
    let spec = QuerySpec {
        count: true,
        ..Default::default()
    };
    let token = CancellationToken::new();
    let first = query_page_on_view(&fixture.session, &old_view, &spec, 0, 2, &token).unwrap();
    assert_eq!(first.count, Some(3));
    assert_eq!(first.instance_id, old_view.instance_id());
    assert_eq!(first.revision, old_view.revision());
    let added = fixture._root.path().join("d.json");
    std::fs::write(&added, r#"{"title":"Delta","year":2025}"#).unwrap();
    index.add_file(&added).unwrap();
    let new_view = index.view().unwrap();
    assert_ne!(old_view.revision(), new_view.revision());
    let old_continuation =
        query_page_on_view(&fixture.session, &old_view, &spec, 2, 2, &token).unwrap();
    assert_eq!(old_continuation.count, Some(3));
    assert_eq!(old_continuation.hits.len(), 1);
    assert_eq!(count_exact_on_view(&old_view, &spec, &token).unwrap(), 3);
    let new_first = query_page_on_view(&fixture.session, &new_view, &spec, 0, 2, &token).unwrap();
    assert_eq!(new_first.count, Some(4));
    assert_eq!(count_exact_on_view(&new_view, &spec, &token).unwrap(), 4);
}

#[test]
fn source_unavailability_never_supplies_a_false_exact_total() {
    let fixture = fixture();
    let view = fixture.session.repository.index().view().unwrap();
    std::fs::remove_file(fixture._root.path().join("a.json")).unwrap();
    let page = query_page_on_view(
        &fixture.session,
        &view,
        &QuerySpec {
            count: true,
            ..Default::default()
        },
        0,
        10,
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(page.incomplete());
    assert_eq!(page.count, None);
    assert!(page.counters.source_unavailable_events > 0);
    assert_eq!(
        count_exact_on_view(
            &view,
            &QuerySpec {
                filter: "year >= 0".into(),
                ..Default::default()
            },
            &CancellationToken::new(),
        )
        .unwrap_err()
        .kind,
        BackendErrorKind::Query
    );
}
