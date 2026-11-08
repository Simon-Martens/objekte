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

#[test]
fn fields_hide_empty_values_and_inline_small_scalar_arrays() {
    assert_eq!(field_display(&serde_json::json!(null)), None);
    assert_eq!(field_display(&serde_json::json!("  ")), None);
    assert_eq!(field_display(&serde_json::json!([])), None);
    assert_eq!(field_display(&serde_json::json!({})), None);
    assert_eq!(field_display(&serde_json::json!([null, " "])), None);
    assert_eq!(
        field_display(&serde_json::json!(0)),
        Some(("0".into(), false))
    );
    assert_eq!(
        field_display(&serde_json::json!(false)),
        Some(("false".into(), false))
    );
    assert_eq!(
        field_display(&serde_json::json!(["Graphik", null, "Gedicht", "Graphik"])),
        Some(("Graphik; Gedicht; Graphik".into(), false))
    );
    assert_eq!(
        field_display(&serde_json::json!(["first", {"nested": 1}])),
        Some(("2 items".into(), true))
    );
}

#[test]
fn leading_columns_follow_predicate_order_and_keep_metadata_distinct() {
    let fixture = fixture();
    let draft = SearchDraft {
        full_text: "needle".into(),
        structured: "year >= 1900 && title = \"SORT BY\" SORT BY year DESC".into(),
        filter_enabled: true,
        ..SearchDraft::default()
    };
    let columns = initial_columns(&fixture.session, &draft);
    assert_eq!(&columns[..5], ["@id", "@status", "@match", "year", "title"]);
    assert_eq!(columns.iter().filter(|column| *column == "year").count(), 1);
    assert!(!columns.contains(&"@collection".to_owned()));
    assert!(!columns.contains(&"@file".to_owned()));
}

#[test]
fn nested_projection_distinguishes_null_missing_and_unavailable() {
    let value = serde_json::json!({"nested": {"title": "Hallo", "nil": null}, "a.b": 7});
    assert_eq!(project_field(Some(&value), "nested.title"), "Hallo");
    assert_eq!(project_field(Some(&value), "nested.nil"), "null");
    assert_eq!(project_field(Some(&value), "nested.absent"), "—");
    assert_eq!(project_field(Some(&value), "[\"a.b\"]"), "7");
    assert_eq!(
        project_field(Some(&value), "@backlinks.related"),
        "Unavailable"
    );
}
