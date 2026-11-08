use super::*;
use crate::backend::{query_page, test_support::fixture};
use objekte_search_core::streaming::CancellationToken;

#[test]
fn detail_marks_matching_fields_from_core_evidence() {
    let fixture = fixture();
    let page = query_page(
        &fixture.session,
        &QuerySpec {
            filter: "year = 2024".into(),
            ..Default::default()
        },
        0,
        10,
        &CancellationToken::new(),
    )
    .unwrap();
    let view = detail_view(&page.hits[0], &HashSet::new());
    assert!(
        view.structured
            .iter()
            .any(|row| row.label == "$.year" && row.matched)
    );
    assert!(
        view.structured
            .iter()
            .any(|row| row.label == "$.title" && !row.matched)
    );
}
