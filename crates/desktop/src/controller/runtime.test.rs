use super::*;

#[test]
fn full_field_path_resolves_nested_json_without_display_clipping() {
    let value = serde_json::json!({"nested": [{"title": "a".repeat(9000)}]});
    let found = find_json_path(&value, "$", "$.nested[0].title").unwrap();
    assert_eq!(found.as_str().unwrap().len(), 9000);
}

#[test]
fn bounded_batch_status_extraction_skips_large_hits() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("events.ndjson");
    let huge = "x".repeat(32 * 1024);
    std::fs::write(&output, format!(
        "{{\"type\":\"query_start\",\"query_id\":1,\"line\":4,\"input\":\"a = 1\"}}\n{{\"type\":\"hit\",\"data\":\"{huge}\"}}\n{{\"type\":\"query_error\",\"query_id\":1,\"error\":\"invalid\"}}\n"
    )).unwrap();
    let rows = batch_rows(&output).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].line, 4);
    assert_eq!(rows[0].query, "a = 1");
    assert_eq!(rows[0].error.as_deref(), Some("invalid"));
}

#[test]
fn cancelled_replacement_open_invalidates_late_loader_without_losing_active_draft() {
    let (mut controller, _) = DesktopController::new();
    let active = OpenDraft {
        paths: "/tmp/active".into(),
        ..OpenDraft::default()
    };
    let attempted = OpenDraft {
        paths: "/tmp/attempted".into(),
        ..OpenDraft::default()
    };
    controller.open_draft = Some(active.clone());
    controller.opening_draft = Some(attempted);
    controller.open_generation = 7;
    let mut updates = Vec::new();
    controller.cancel_open(&mut updates);
    assert_eq!(controller.open_generation, 8);
    assert!(controller.opening_draft.is_none());
    assert_eq!(controller.open_draft, Some(active));
    assert!(controller.cancellation.is_cancelled());
    assert!(
        matches!(updates.as_slice(), [UiUpdate::Status(message)] if message == "Open cancelled")
    );
}

#[test]
fn invalid_filter_does_not_start_query_or_replace_visible_page() {
    let fixture = crate::backend::test_support::fixture();
    let (mut controller, _) = DesktopController::new();
    controller.session = Some(fixture.session);
    controller.page.offset = 11;
    let mut updates = Vec::new();
    controller.start_query(
        SearchDraft {
            structured: "year >=".into(),
            filter_enabled: true,
            ..SearchDraft::default()
        },
        0,
        &mut updates,
    );
    assert_eq!(controller.page.offset, 11);
    assert!(updates.iter().any(|update| matches!(
        update,
        UiUpdate::FilterValidation {
            state: FilterValidationState::Invalid { .. },
            ..
        }
    )));
    assert!(
        !updates
            .iter()
            .any(|update| matches!(update, UiUpdate::QueryStarted(_)))
    );
}
