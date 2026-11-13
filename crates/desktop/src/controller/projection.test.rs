use super::*;
use crate::backend::{query_page, test_support::fixture};
use objekte_search_core::streaming::CancellationToken;

#[test]
fn metadata_columns_show_captured_values_without_a_json_payload() {
    let fixture = fixture();
    let spec = QuerySpec::default();
    let mut page = query_page(&fixture.session, &spec, 0, 10, &CancellationToken::new()).unwrap();
    let columns = initial_columns(
        &fixture.session.repository.index().view().unwrap(),
        &SearchDraft::default(),
    );
    // Render the same known values with and without a decoded source record.
    for payload_available in [true, false] {
        if !payload_available {
            for hit in &mut page.hits {
                hit.payload = None;
            }
        }
        for hit in &mut page.hits {
            hit.file_created_ns = Some(1_000_000_000);
            hit.modified_ns = -1_234_567_890;
        }
        let (view, _, _) = page_view_with_prior(&page, &spec, &[], &[], &columns);
        for (row, hit) in view.rows.iter().zip(&page.hits) {
            let actual_size = std::fs::metadata(fixture._root.path().join(&hit.file_path)).unwrap().len();
            for (column, expected) in [
                ("@record.id", hit.record_id.clone()),
                ("@record.byte_start", hit.byte_start.to_string()),
                ("@record.byte_end", hit.byte_end.to_string()),
                ("@record.byte_length", (hit.byte_end - hit.byte_start).to_string()),
                ("@record.class", hit.record_class.code().to_string()),
                ("@file.id", hit.file_id.to_string()),
                ("@file.path", hit.file_path.display().to_string()),
                ("@file.size", actual_size.to_string()),
                ("@file.created", "1000.000 ms".into()),
                ("@file.modified", "-1234.568 ms".into()),
                ("@file.status", hit.file_status.code().to_string()),
                ("@collection.name", hit.collection.clone()),
                ("@repository.root", fixture._root.path().display().to_string()),
            ] {
                let index = view.columns.iter().position(|name| name == column).unwrap();
                assert_eq!(row.cells[index], expected, "{column}, payload={payload_available}");
            }
        }
    }
}

#[test]
fn metadata_and_quoted_data_paths_remain_distinct_and_creation_can_be_null() {
    let fixture = fixture();
    let spec = QuerySpec::default();
    let mut page = query_page(&fixture.session, &spec, 0, 10, &CancellationToken::new()).unwrap();
    page.hits.truncate(1);
    let hit = &mut page.hits[0];
    hit.file_created_ns = None;
    hit.file_path = "percent%file.json".into();
    hit.payload = Some(Payload::Bytes(br#"{"@file":{"size":"data size"}}"#.to_vec()));
    let columns: Vec<String> = vec!["@file.size".into(), "[\"@file\"].size".into(), "@file.created".into(), "@file.path".into()];
    let (view, _, _) = page_view_with_prior(&page, &spec, &[], &[columns[1].clone()], &columns);
    assert_eq!(&view.rows[0].cells[..4], &[
        page.hits[0].file_size.to_string(), "data size".into(), "null".into(), "percent%25file.json".into(),
    ]);
}

#[test]
fn metadata_projection_keeps_the_page_values_after_the_source_disappears() {
    let fixture = fixture();
    let spec = QuerySpec { filter: "year = 2024".into(), ..Default::default() };
    let mut page = query_page(&fixture.session, &spec, 0, 10, &CancellationToken::new()).unwrap();
    let hit = &mut page.hits[0];
    hit.payload = None;
    std::fs::remove_file(fixture._root.path().join(&hit.file_path)).unwrap();
    let expected = hit.file_size.to_string();
    let columns = vec!["@file.size".into()];
    let (view, _, _) = page_view_with_prior(&page, &spec, &[], &[], &columns);
    assert_eq!(view.rows[0].cells[0], expected);
}

#[test]
fn metadata_cells_agree_with_filters_combining_metadata_and_record_fields() {
    let fixture = fixture();
    let size = std::fs::metadata(fixture._root.path().join("a.json")).unwrap().len();
    let root = serde_json::to_string(&objekte_search_core::path_repr::query_path(
        fixture._root.path(),
    )).unwrap();
    for filter in [
        "@file.size < 2KB && year = 2024".to_owned(),
        format!("year = 2024 && (@file.size >= 02GiB || @record.byte_length = {size})"),
        format!("(@file.path = 'a.json' || year < 0) && @collection.name = 'files' && @repository.root = {root}"),
        format!("@record.id = 'path:a.json' && @record.byte_start = 0 && @record.byte_end = {size} SORT BY @file.size DESC"),
        "@file.size < 23MB && (year = 2024 || year < 0)".to_owned(),
    ] {
        let spec = QuerySpec { filter: filter.clone(), count: true, ..Default::default() };
        let page = query_page(&fixture.session, &spec, 0, 10, &CancellationToken::new()).unwrap();
        assert_eq!(page.count, Some(1), "{filter}");
        let columns = initial_columns(
            &fixture.session.repository.index().view().unwrap(),
            &SearchDraft {
                structured: filter.clone(),
                filter_enabled: true,
                ..Default::default()
            },
        );
        let (view, _, _) = page_view_with_prior(&page, &spec, &[], &[], &columns);
        assert_eq!(view.rows.len(), 1, "{filter}");
        for (column, expected) in [
            ("@record.id", "path:a.json".to_owned()),
            ("@file.path", "a.json".to_owned()),
            ("@file.size", size.to_string()),
            ("@record.byte_length", size.to_string()),
            ("@record.byte_start", "0".to_owned()),
            ("@record.byte_end", size.to_string()),
            ("@collection.name", "files".to_owned()),
            ("@repository.root", fixture._root.path().display().to_string()),
        ] {
            let index = view.columns.iter().position(|name| name == column).unwrap();
            assert_eq!(view.rows[0].cells[index], expected, "{column}: {filter}");
        }
    }
}

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
    let columns = initial_columns(&fixture.session.repository.index().view().unwrap(), &draft);
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
