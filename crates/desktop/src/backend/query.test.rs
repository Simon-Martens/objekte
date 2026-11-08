use super::*;
use crate::backend::test_support::fixture;

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
