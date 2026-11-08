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
    assert_eq!(&columns[..5], ["@status", "@id", "@match", "year", "title"]);
    assert_eq!(columns.iter().filter(|column| *column == "year").count(), 1);
    assert!(!columns.contains(&"@collection".to_owned()));
    assert!(!columns.contains(&"@file".to_owned()));
}

#[test]
fn nested_projection_distinguishes_null_missing_and_unavailable() {
    let value = serde_json::json!({"nested": {"title": "Hallo", "nil": null}, "a.b": 7, "empty": "", "complex": [{"part":"one"},{"part":"two"}]});
    assert_eq!(project_field(Some(&value), "nested.title"), "Hallo");
    assert_eq!(project_field(Some(&value), "nested.nil"), "null");
    assert_eq!(project_field(Some(&value), "nested.absent"), "—");
    assert_eq!(project_field(Some(&value), "[\"a.b\"]"), "7");
    assert_eq!(project_field(Some(&value), "empty"), "-");
    assert_eq!(project_field(Some(&value), "complex.part"), "one; two");
    assert_eq!(
        project_field(Some(&value), "@backlinks.related"),
        "Unavailable"
    );
}

#[test]
fn nested_columns_are_available_but_hidden_until_explicitly_shown() {
    let fixture = fixture();
    let page = query_page(&fixture.session, &QuerySpec::default(), 0, 10, &CancellationToken::new()).unwrap();
    let prior = vec!["@status".into(), "@id".into(), "nested.title".into()];
    let (view, available, hidden) = page_view_with_prior(&page, &QuerySpec::default(), &[], &[], &prior);
    assert!(available.contains(&"nested.title".into()));
    assert!(hidden.contains(&"nested.title".into()));
    assert!(!view.columns.contains(&"nested.title".into()));
    let (view, _, hidden) = page_view_with_prior(&page, &QuerySpec::default(), &[], &["nested.title".into()], &prior);
    assert!(view.columns.contains(&"nested.title".into()));
    assert!(!hidden.contains(&"nested.title".into()));
}

#[test]
fn match_preview_keeps_every_nested_field_without_an_excerpt() {
    let fixture = fixture();
    let page = query_page(&fixture.session, &QuerySpec::default(), 0, 10, &CancellationToken::new()).unwrap();
    let mut hit = page.hits[0].clone();
    hit.matched_fields = vec!["nested.title".into(), "nested.other".into(), "missing.path".into()];
    let query = FtsQuery { literal: "Alpha".into(), mode: FtsMode::Literal, case_sensitivity: CaseSensitivity::Insensitive, pattern_parameter: None, fields: Vec::new() };
    let matcher = PreviewMatcher::new(&query).unwrap();
    let json = serde_json::json!({"nested": {"title": "Alpha here", "other": "value"}});
    let (text, highlights) = match_preview(&hit, Some(&json), Some(&matcher));
    assert_eq!(text, "nested.title: Alpha here\nnested.other\nmissing.path");
    assert_eq!(highlights.len(), 4);
    let json = serde_json::json!({"complex": [{"part": "other"}, {"part": "Alpha in an array"}]});
    assert_eq!(matched_excerpt(Some(&json), "complex.part", &matcher).unwrap().0, "Alpha in an array");
}

#[test]
fn match_preview_marks_actual_field_and_fts_span() {
    let fixture = fixture();
    let spec = QuerySpec {
        full_text: "Alpha".into(),
        filter: "year = 2024".into(),
        ..Default::default()
    };
    let page = query_page(&fixture.session, &spec, 0, 10, &CancellationToken::new()).unwrap();
    let (view, _, _) = page_view_with_prior(
        &page,
        &spec,
        &[],
        &[],
        &[
            "@id".into(),
            "@status".into(),
            "@match".into(),
            "title".into(),
            "year".into(),
        ],
    );
    let row = &view.rows[0];
    assert_eq!(row.cells[2], "title: Alpha\nyear");
    assert_eq!(row.match_highlights, [(0..5, false), (7..12, true), (13..17, false)]);
}

#[test]
fn inspector_uses_captured_query_for_filter_and_fts_emphasis() {
    let fixture = fixture();
    let spec = QuerySpec {
        full_text: "Alpha".into(),
        filter: "year = 2024".into(),
        ..Default::default()
    };
    let page = query_page(&fixture.session, &spec, 0, 10, &CancellationToken::new()).unwrap();
    let view = detail_view_with_query(&page.hits[0], &HashSet::new(), Some(&spec));
    let title = view
        .structured
        .iter()
        .find(|row| row.label == "$.title")
        .unwrap();
    assert!(title.fts_matched);
    assert!(!title.filter_participant);
    assert_eq!(title.value_highlights.len(), 1);
    assert_eq!(title.value_highlights[0], 0..5);
    let year = view
        .structured
        .iter()
        .find(|row| row.label == "$.year")
        .unwrap();
    assert!(year.filter_participant);
    assert!(!year.fts_matched);
    assert!(year.value_highlights.is_empty());
}
