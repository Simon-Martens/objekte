use super::*;

#[test]
fn indexed_directory_tree_follows_added_and_deleted_files() {
    let fixture = crate::backend::test_support::fixture();
    let index = fixture.session.repository.index();
    assert!(
        indexed_directories(&index.view().unwrap(), &fixture.session.collections)
            .all
            .is_empty()
    );
    let file = fixture._root.path().join("docs/nested/new.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, r#"{"title":"New"}"#).unwrap();
    index.add_file(&file).unwrap();
    let view = index.view().unwrap();
    let directories = indexed_directories(&view, &fixture.session.collections);
    assert_eq!(directories.all, ["docs", "docs/nested"]);
    assert_eq!(
        directories.by_collection[&fixture.session.collections[0]],
        ["docs", "docs/nested"]
    );
    assert!(view.file_paths_for_collection("missing").next().is_none());
    let handle = view.file_handle_for_path(&file).unwrap();
    index.delete_file(handle).unwrap();
    assert!(
        indexed_directories(&index.view().unwrap(), &fixture.session.collections)
            .all
            .is_empty()
    );
}

#[test]
fn presentation_refresh_keeps_selection_after_preview_is_closed() {
    let fixture = crate::backend::test_support::fixture();
    let (mut controller, _) = DesktopController::new();
    let draft = SearchDraft::default();
    controller.session = Some(fixture.session);
    controller.current_draft = draft.clone();
    controller.executed_draft = Some(draft.clone());
    controller.selected_index = Some(798);

    let mut updates = Vec::new();
    controller.start_query_impl(draft, 0, false, true, &mut updates);

    assert_eq!(controller.selected_index, Some(798));
    assert!(updates.iter().any(|update| matches!(
        update,
        UiUpdate::QueryStarted {
            retain_preview: true,
            ..
        }
    )));
}

#[test]
fn loaded_index_receives_check_policy_before_old_scheduler_retires() {
    let fixture = crate::backend::test_support::fixture();
    let original = fixture.session.clone();
    let saved = tempfile::tempdir().unwrap();
    let path = saved.path().join("index.snapshot");
    backend::index_lifecycle::save_index(&original, &path, false).unwrap();
    backend::index_lifecycle::set_periodic_check(&original, Some(Duration::from_secs(60))).unwrap();
    let replacement =
        backend::index_lifecycle::load_index(&original, &path, &CancellationToken::new()).unwrap();
    let (mut controller, _) = DesktopController::new();
    controller.open_generation = 4;
    controller.session = Some(replacement.clone());
    controller.requested_check = (true, 60);
    controller.transfer_check_policy(replacement.clone(), Some(original.clone()));
    let event = controller
        .workers
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    assert!(matches!(
        event,
        WorkerEvent::AutomaticCheckChanged {
            open_generation: 4,
            enabled: true,
            result: Ok(()),
            ..
        }
    ));
    assert!(
        replacement
            .repository
            .index()
            .periodic_reconciliation_enabled()
            .unwrap()
    );
    assert!(
        !original
            .repository
            .index()
            .periodic_reconciliation_enabled()
            .unwrap()
    );
    backend::index_lifecycle::shutdown(&replacement).unwrap();
    backend::index_lifecycle::shutdown(&original).unwrap();
}

#[test]
fn disabled_check_policy_retires_old_scheduler_on_replacement() {
    let fixture = crate::backend::test_support::fixture();
    let original = fixture.session.clone();
    let saved = tempfile::tempdir().unwrap();
    let path = saved.path().join("index.snapshot");
    backend::index_lifecycle::save_index(&original, &path, false).unwrap();
    backend::index_lifecycle::set_periodic_check(&original, Some(Duration::from_secs(60))).unwrap();
    let replacement =
        backend::index_lifecycle::load_index(&original, &path, &CancellationToken::new()).unwrap();
    let (mut controller, _) = DesktopController::new();
    controller.open_generation = 9;
    controller.session = Some(replacement.clone());
    controller.requested_check = (false, 37);
    controller.transfer_check_policy(replacement.clone(), Some(original.clone()));
    assert!(matches!(
        controller
            .workers
            .recv_timeout(Duration::from_secs(5))
            .unwrap(),
        WorkerEvent::AutomaticCheckChanged {
            open_generation: 9,
            enabled: false,
            interval_seconds: 37,
            result: Ok(()),
            ..
        }
    ));
    assert!(
        !replacement
            .repository
            .index()
            .periodic_reconciliation_enabled()
            .unwrap()
    );
    assert!(
        !original
            .repository
            .index()
            .periodic_reconciliation_enabled()
            .unwrap()
    );
    backend::index_lifecycle::shutdown(&replacement).unwrap();
    backend::index_lifecycle::shutdown(&original).unwrap();
}

#[test]
fn rapid_replacements_and_toggle_keep_only_latest_scheduler() {
    let fixture = crate::backend::test_support::fixture();
    let original = fixture.session.clone();
    let saved = tempfile::tempdir().unwrap();
    let path = saved.path().join("index.snapshot");
    backend::index_lifecycle::save_index(&original, &path, false).unwrap();
    backend::index_lifecycle::set_periodic_check(&original, Some(Duration::from_secs(60))).unwrap();
    let first =
        backend::index_lifecycle::load_index(&original, &path, &CancellationToken::new()).unwrap();
    let second =
        backend::index_lifecycle::load_index(&original, &path, &CancellationToken::new()).unwrap();
    let (mut controller, _) = DesktopController::new();
    controller.open_generation = 11;
    let gate = Arc::clone(&controller.automatic_check_gate);
    let held = gate.lock().unwrap();
    controller.session = Some(first.clone());
    controller.requested_check = (true, 60);
    controller.transfer_check_policy(first.clone(), Some(original.clone()));
    controller.set_automatic_check(true, 90, &mut Vec::new());
    controller.session = Some(second.clone());
    controller.open_generation += 1;
    controller.transfer_check_policy(second.clone(), Some(first.clone()));
    drop(held);
    assert!(matches!(
        controller
            .workers
            .recv_timeout(Duration::from_secs(5))
            .unwrap(),
        WorkerEvent::AutomaticCheckChanged {
            open_generation: 12,
            enabled: true,
            interval_seconds: 90,
            result: Ok(()),
            ..
        }
    ));
    assert!(
        second
            .repository
            .index()
            .periodic_reconciliation_enabled()
            .unwrap()
    );
    assert!(
        !first
            .repository
            .index()
            .periodic_reconciliation_enabled()
            .unwrap()
    );
    assert!(
        !original
            .repository
            .index()
            .periodic_reconciliation_enabled()
            .unwrap()
    );
    backend::index_lifecycle::shutdown(&second).unwrap();
    backend::index_lifecycle::shutdown(&first).unwrap();
    backend::index_lifecycle::shutdown(&original).unwrap();
}

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
    let index = objekte_search_core::lifecycle::Index::from_repository(repository).unwrap();
    session.repository =
        Arc::new(objekte_search_core::lifecycle_caller::IndexedRepository::new(index).unwrap());
    session.collections = vec!["files".into()];
    let (values, partial, visited) = posting_completions(
        &session,
        &CompletionRequest {
            origin: CompletionOrigin::Filter,
            revision: 1,
            collection: Some("files".into()),
            field: "name".into(),
            prefix: "Al".into(),
        },
        &CancellationToken::new(),
    );
    assert_eq!(values, [(r#""Alpha""#.into(), r#""Alpha""#.into())]);
    assert!(!partial);
    assert!(visited <= 20);
}

#[test]
fn declared_enum_completes_without_a_posting_dictionary() {
    use objekte_cli::arguments::Arguments;

    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.json"), r#"{"status":"Ready"}"#).unwrap();
    let schema = root.path().join("schema.json");
    std::fs::write(
        &schema,
        r#"{"type":"object","properties":{"status":{"type":"string","enum":["Ready","Paused"]}}}"#,
    )
    .unwrap();
    let session = backend::open(
        &OpenSpec {
            args: Arguments {
                path: vec![root.path().to_path_buf()],
                schema: Some(schema),
                ..Default::default()
            },
            cwd: root.path().to_path_buf(),
            configured_cache_policy: None,
        },
        &CancellationToken::new(),
    )
    .unwrap();
    let (values, _, visited) = posting_completions(
        &session,
        &CompletionRequest {
            origin: CompletionOrigin::Filter,
            revision: 1,
            collection: Some("files".into()),
            field: "status".into(),
            prefix: "Re".into(),
        },
        &CancellationToken::new(),
    );
    assert_eq!(values, [(r#""Ready""#.into(), r#""Ready""#.into())]);
    assert_eq!(visited, 0);
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
    controller.pending_open_generation = 12;
    let mut updates = Vec::new();
    controller.cancel_open(&mut updates);
    assert_eq!(controller.open_generation, 7);
    assert_eq!(controller.pending_open_generation, 13);
    assert!(controller.opening_draft.is_none());
    assert_eq!(controller.open_draft, Some(active));
    assert!(controller.open_cancellation.is_cancelled());
    assert!(!controller.cancellation.is_cancelled());
    assert!(
        matches!(updates.as_slice(), [UiUpdate::Status(message)] if message == "Open cancelled")
    );
}

#[test]
fn publication_changes_invalidate_results_once_but_noop_checks_do_not() {
    let fixture = crate::backend::test_support::fixture();
    let (mut controller, _) = DesktopController::new();
    let view = fixture.session.repository.index().view().unwrap();
    let initial = Publication {
        instance_id: view.instance_id(),
        revision: view.revision(),
    };
    controller.observed_publication = Some(initial);
    controller.executed_view = Some(view);
    controller.executed_spec = Some(backend::QuerySpec::default());
    controller.executed_draft = Some(SearchDraft::default());
    controller.session = Some(fixture.session);
    let mut updates = Vec::new();
    controller.poll_publication(&mut updates);
    assert!(updates.is_empty());
    let report = backend::index_lifecycle::check_for_changes(controller.session.as_ref().unwrap())
        .unwrap()
        .wait()
        .unwrap();
    assert_eq!(report.added + report.updated + report.removed, 0);
    controller.poll_publication(&mut updates);
    assert!(!controller.results_outdated);
    assert!(updates.is_empty());

    std::fs::write(fixture._root.path().join("new.json"), r#"{"title":"New"}"#).unwrap();
    backend::index_lifecycle::check_for_changes(controller.session.as_ref().unwrap())
        .unwrap()
        .wait()
        .unwrap();
    controller.poll_publication(&mut updates);
    assert!(controller.results_outdated);
    assert!(controller.executed_view.is_none());
    assert!(matches!(
        updates.as_slice(),
        [UiUpdate::ResultsOutdated(true)]
    ));
    updates.clear();
    controller.poll_publication(&mut updates);
    assert!(updates.is_empty());
}

#[test]
fn manual_close_has_no_implicit_index_save() {
    let fixture = crate::backend::test_support::fixture();
    let (mut controller, _) = DesktopController::new();
    controller.session = Some(fixture.session);
    let mut updates = Vec::new();
    controller.request_close(&mut updates);
    assert!(matches!(
        updates.as_slice(),
        [UiUpdate::ClosingIndex { error: None }]
    ));
    assert!(matches!(
        controller
            .workers
            .recv_timeout(Duration::from_secs(5))
            .unwrap(),
        WorkerEvent::IndexShutdown { result: Ok(()), .. }
    ));
    assert!(!fixture._root.path().join(".objekte").exists());
}

#[test]
fn pending_automatic_save_enable_cannot_write_after_manual_close() {
    let fixture = crate::backend::test_support::fixture();
    let (mut controller, _) = DesktopController::new();
    controller.session = Some(fixture.session);
    controller.index_ui.automatic_save_available = true;
    let save_gate = Arc::clone(&controller.automatic_save_gate);
    let held = save_gate.lock().unwrap();
    let mut updates = Vec::new();
    controller.set_automatic_save(true, &mut updates);
    controller.request_close(&mut updates);
    drop(held);
    assert!(matches!(
        controller
            .workers
            .recv_timeout(Duration::from_secs(5))
            .unwrap(),
        WorkerEvent::IndexShutdown { result: Ok(()), .. }
    ));
    assert!(!fixture._root.path().join(".objekte").exists());
    assert_eq!(
        backend::index_lifecycle::status(controller.session.as_ref().unwrap())
            .unwrap()
            .saved_revision,
        None
    );
}

#[test]
fn superseded_automatic_save_enable_cannot_run_after_disable() {
    let fixture = crate::backend::test_support::fixture();
    let (mut controller, _) = DesktopController::new();
    controller.session = Some(fixture.session);
    controller.index_ui.automatic_save_available = true;
    let save_gate = Arc::clone(&controller.automatic_save_gate);
    let held = save_gate.lock().unwrap();
    controller.set_automatic_save(true, &mut Vec::new());
    controller.set_automatic_save(false, &mut Vec::new());
    drop(held);
    assert!(matches!(
        controller
            .workers
            .recv_timeout(Duration::from_secs(5))
            .unwrap(),
        WorkerEvent::AutomaticSaveChanged {
            enabled: false,
            result: Ok(()),
            ..
        }
    ));
    assert!(!fixture._root.path().join(".objekte").exists());
    backend::index_lifecycle::shutdown(controller.session.as_ref().unwrap()).unwrap();
}

#[test]
fn unchanged_index_status_does_not_redraw_the_workspace() {
    let fixture = crate::backend::test_support::fixture();
    let (mut controller, _) = DesktopController::new();
    controller.session = Some(fixture.session);
    let status = controller
        .session
        .as_ref()
        .unwrap()
        .repository
        .index()
        .status()
        .unwrap();
    let mut updates = Vec::new();
    controller.update_index_ui(&status, &mut updates);
    assert!(matches!(updates.as_slice(), [UiUpdate::IndexStatus(_)]));

    updates.clear();
    controller.update_index_ui(&status, &mut updates);
    assert!(updates.is_empty());

    controller.index_ui.check = IndexTaskState::Succeeded("Files checked".into());
    controller.update_index_ui(&status, &mut updates);
    assert!(matches!(updates.as_slice(), [UiUpdate::IndexStatus(_)]));
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
            .any(|update| matches!(update, UiUpdate::QueryStarted { .. }))
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
    controller.open_record(0, false, &mut updates);
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
    assert_eq!(controller.tabs[0].context.draft, SearchDraft::default());
    assert!(!token.is_cancelled());
    controller.close_tab(tab_id, &mut updates);
    assert!(controller.tabs.is_empty());
    assert!(token.is_cancelled());
}

#[test]
fn row_preview_reuses_only_its_own_slot_and_explicit_tabs_stay_pinned() {
    let fixture = crate::backend::test_support::fixture();
    let page = backend::query_page(
        &fixture.session,
        &backend::QuerySpec::default(),
        0,
        10,
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(page.hits.len() >= 2);
    let (mut controller, _) = DesktopController::new();
    controller.session = Some(fixture.session);
    controller.executed_spec = Some(backend::QuerySpec::default());
    controller.executed_draft = Some(SearchDraft::default());
    controller.keys = page
        .hits
        .iter()
        .map(|hit| {
            Some(RecordKey {
                collection: hit.collection.clone(),
                id: hit.record_id.clone(),
            })
        })
        .collect();
    let mut updates = Vec::new();
    controller.open_record(0, false, &mut updates);
    let first_id = controller.active_tab.unwrap();
    assert!(!controller.tabs[0].pinned);
    controller.open_record(1, false, &mut updates);
    assert_eq!(controller.active_tab, Some(first_id));
    assert_eq!(controller.tabs.len(), 1);
    controller.open_record(1, true, &mut updates);
    assert_eq!(controller.tabs.len(), 2);
    let pinned_id = controller.active_tab.unwrap();
    assert_ne!(pinned_id, first_id);
    assert!(controller.tabs.iter().find(|tab| tab.id == pinned_id).unwrap().pinned);

    controller.open_record(0, false, &mut updates);
    assert_eq!(controller.active_tab, Some(first_id));
    assert_eq!(controller.tabs.len(), 2);
    assert!(controller.tabs.iter().find(|tab| tab.id == pinned_id).unwrap().pinned);
    controller.pin_preview(&mut updates);
    assert!(controller.tabs.iter().all(|tab| tab.pinned));
    assert!(updates.iter().any(|update| matches!(
        update,
        UiUpdate::Tabs { tabs, active: Some(id) }
            if *id == first_id && tabs.iter().find(|tab| tab.id == first_id).unwrap().pinned
    )));
}

#[test]
fn activating_record_tab_selects_matching_result_or_clears_old_selection() {
    let fixture = crate::backend::test_support::fixture();
    let page = backend::query_page(
        &fixture.session,
        &backend::QuerySpec::default(),
        0,
        10,
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(page.hits.len() >= 2);
    let (mut controller, _) = DesktopController::new();
    controller.session = Some(fixture.session);
    controller.executed_spec = Some(backend::QuerySpec::default());
    controller.executed_draft = Some(SearchDraft::default());
    controller.keys = page
        .hits
        .iter()
        .map(|hit| {
            Some(RecordKey {
                collection: hit.collection.clone(),
                id: hit.record_id.clone(),
            })
        })
        .collect();
    let mut updates = Vec::new();
    controller.open_record(0, false, &mut updates);
    let first_id = controller.active_tab.unwrap();
    controller.open_record(1, true, &mut updates);
    let second_id = controller.active_tab.unwrap();

    updates.clear();
    controller.activate_tab(first_id, &mut updates);
    assert_eq!(controller.selected_index, Some(0));
    assert!(updates
        .iter()
        .any(|update| matches!(update, UiUpdate::RowSelection(Some(0)))));

    updates.clear();
    controller.close_tab(first_id, &mut updates);
    assert_eq!(controller.active_tab, None);
    assert!(updates
        .iter()
        .any(|update| matches!(update, UiUpdate::Tabs { active: None, .. })));
    controller.activate_tab(second_id, &mut updates);
    assert_eq!(controller.active_tab, Some(second_id));
    assert_eq!(controller.selected_index, Some(1));
    assert!(updates
        .iter()
        .any(|update| matches!(update, UiUpdate::RowSelection(Some(1)))));

    controller.keys.clear();
    updates.clear();
    controller.activate_tab(second_id, &mut updates);
    assert_eq!(controller.selected_index, None);
    assert!(updates
        .iter()
        .any(|update| matches!(update, UiUpdate::RowSelection(None))));
}

#[test]
fn active_record_selection_follows_reordered_and_appended_results() {
    let fixture = crate::backend::test_support::fixture();
    let page = backend::query_page(
        &fixture.session,
        &backend::QuerySpec::default(),
        0,
        10,
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(page.hits.len() >= 2);
    let first = RecordKey {
        collection: page.hits[0].collection.clone(),
        id: page.hits[0].record_id.clone(),
    };
    let second = RecordKey {
        collection: page.hits[1].collection.clone(),
        id: page.hits[1].record_id.clone(),
    };
    let (mut controller, _) = DesktopController::new();
    controller.session = Some(fixture.session);
    controller.executed_spec = Some(backend::QuerySpec::default());
    controller.executed_draft = Some(SearchDraft::default());
    controller.keys = vec![Some(first.clone()), Some(second.clone())];
    let mut updates = Vec::new();
    controller.open_record(1, false, &mut updates);

    // A new query may load the same record at a different position.
    controller.keys = vec![Some(second.clone()), Some(first.clone())];
    updates.clear();
    controller.sync_active_tab_row_selection_in(0, &mut updates);
    assert_eq!(controller.selected_index, Some(0));
    assert!(updates
        .iter()
        .any(|update| matches!(update, UiUpdate::RowSelection(Some(0)))));

    // Selection stays clear until the page containing that record arrives.
    controller.keys = vec![Some(first), None];
    updates.clear();
    controller.sync_active_tab_row_selection_in(0, &mut updates);
    assert_eq!(controller.selected_index, None);
    controller.keys[1] = Some(second);
    updates.clear();
    controller.sync_active_tab_row_selection_in(1, &mut updates);
    assert_eq!(controller.selected_index, Some(1));
}

#[test]
fn opening_record_outside_results_clears_previous_row_selection() {
    let fixture = crate::backend::test_support::fixture();
    let view = fixture.session.repository.index().view().unwrap();
    let (mut controller, _) = DesktopController::new();
    controller.session = Some(fixture.session);
    controller.executed_view = Some(view);
    controller.executed_spec = Some(backend::QuerySpec::default());
    controller.executed_draft = Some(SearchDraft::default());
    controller.selected_index = Some(0);
    let mut updates = Vec::new();
    controller.open_canonical_outside_results(
        RecordKey {
            collection: "files".into(),
            id: "missing-record".into(),
        },
        true,
        &mut updates,
    );
    assert_eq!(controller.selected_index, None);
    assert!(updates
        .iter()
        .any(|update| matches!(update, UiUpdate::RowSelection(None))));
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
