use super::*;

#[test]
fn completion_uses_bounded_postings_and_keeps_typed_literals() {
    use objekte_search_core::model::LogicalType;
    use objekte_search_core::reference::FieldDefinition;
    use objekte_search_core::repository::{CollectionDefinition, Repository, RepositoryOptions};
    use objekte_search_core::source::SourceFormat;

    let fixture = crate::backend::test_support::fixture();
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("items")).unwrap();
    std::fs::write(root.path().join("items/a.json"), r#"{"name":"Alpha"}"#).unwrap();
    std::fs::write(root.path().join("items/b.json"), r#"{"name":"Beta"}"#).unwrap();
    let mut definition = CollectionDefinition::folder("files", "items", SourceFormat::Json);
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["name".into()],
            kind: LogicalType::String,
            required: false,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let repository =
        Repository::open_with_options(root.path(), vec![definition], RepositoryOptions::default())
            .unwrap();
    let mut session = fixture.session;
    session.repository = Arc::new(repository);
    session.collections = vec!["files".into()];
    let (values, partial, visited) = posting_completions(
        &session,
        &CompletionRequest {
            revision: 1,
            collection: Some("files".into()),
            field: "name".into(),
            prefix: "Al".into(),
        },
    );
    assert_eq!(values, [(r#""Alpha""#.into(), r#""Alpha""#.into())]);
    assert!(!partial);
    assert!(visited <= 20);
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
    controller.selected_index = Some(11);
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
    assert_eq!(controller.selected_index, Some(11));
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

#[test]
fn main_query_change_preserves_tab_context_and_close_cancels_its_work() {
    let fixture = crate::backend::test_support::fixture();
    let page = backend::query_page(
        &fixture.session,
        &backend::QuerySpec::default(),
        0,
        1,
        &CancellationToken::new(),
    )
    .unwrap();
    let hit = &page.hits[0];
    let (mut controller, _) = DesktopController::new();
    controller.session = Some(fixture.session);
    controller.executed_spec = Some(backend::QuerySpec::default());
    controller.executed_draft = Some(SearchDraft::default());
    controller.keys.push(Some(RecordKey {
        collection: hit.collection.clone(),
        id: hit.record_id.clone(),
    }));
    let mut updates = Vec::new();
    controller.open_record(0, &mut updates);
    assert_eq!(controller.tabs.len(), 1);
    let tab_id = controller.tabs[0].id;
    let token = controller.tabs[0].cancellation.clone();
    controller.start_query(
        SearchDraft {
            full_text: "different".into(),
            ..SearchDraft::default()
        },
        0,
        &mut updates,
    );
    assert_eq!(controller.tabs[0].context.query_generation, 0);
    assert_eq!(controller.tabs[0].context.draft, SearchDraft::default());
    assert!(!token.is_cancelled());
    controller.close_tab(tab_id, &mut updates);
    assert!(controller.tabs.is_empty());
    assert!(token.is_cancelled());
}

#[test]
fn invalid_current_request_cannot_export_previous_results() {
    let fixture = crate::backend::test_support::fixture();
    let (mut controller, _) = DesktopController::new();
    controller.session = Some(fixture.session);
    controller.current_draft = SearchDraft {
        structured: "year >=".into(),
        filter_enabled: true,
        ..SearchDraft::default()
    };
    let path = fixture._root.path().join("should-not-export.json");
    let mut updates = Vec::new();
    controller.start_export(
        ExportFormat::ResultsJson,
        path.display().to_string(),
        &mut updates,
    );
    assert!(!path.exists());
    assert!(matches!(
        updates.as_slice(),
        [UiUpdate::Error {
            field: ErrorField::Export,
            ..
        }]
    ));
}

#[test]
fn export_uses_current_valid_draft_before_debounce_runs() {
    let fixture = crate::backend::test_support::fixture();
    let (mut controller, _) = DesktopController::new();
    controller.session = Some(fixture.session);
    controller.current_draft = SearchDraft {
        structured: "year >= 2024".into(),
        filter_enabled: true,
        ..SearchDraft::default()
    };
    let output = tempfile::tempdir().unwrap();
    let path = output.path().join("current.json");
    let mut updates = Vec::new();
    controller.start_export(
        ExportFormat::ResultsJson,
        path.display().to_string(),
        &mut updates,
    );
    for _ in 0..100 {
        if path.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let result: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(result["query"]["filter"], "year >= 2024");
    assert_eq!(result["total"], 1);
}
