use super::*;
use crate::tui::test_support::fixture;

fn settled(app: &mut App) {
    for _ in 0..1000 {
        app.tick(Instant::now() + DEBOUNCE);
        if !app.searching && app.pending.is_none() && app.requested_offset.is_none() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("query did not settle");
}

#[test]
fn filter_completion_uses_current_schema_metadata_and_posting_keys() {
    let mut fixture = fixture(3);
    let app = &mut fixture.app;
    app.focus = Focus::Filter;
    app.scope = 1;
    app.filter = Input::with_text("tit");
    app.refresh_completion();
    assert!(app.completion.as_ref().unwrap().choices.contains(&"title".into()));
    assert!(app.accept_completion());
    assert_eq!(app.filter.text, "title");
    app.refresh_completion();
    assert!(app.completion.is_none(), "accepted text must not capture Enter again");

    app.filter = Input::with_text("title = nee");
    app.refresh_completion();
    assert!(app.completion.as_ref().unwrap().choices.contains(&"\"needle title\"".into()));
    assert!(app.accept_completion());
    assert_eq!(app.filter.text, "title = \"needle title\"");

    app.filter = Input::with_text("title:nu");
    app.refresh_completion();
    assert!(app.completion.as_ref().unwrap().choices.contains(&"null".into()));
    app.filter = Input::with_text("title:exists = t");
    app.refresh_completion();
    assert_eq!(app.completion.as_ref().unwrap().choices, ["true"]);

    app.filter = Input::with_text("@file.pa");
    app.refresh_completion();
    assert!(app.completion.as_ref().unwrap().choices.contains(&"@file.path".into()));
}

#[test]
fn provisional_delta_preserves_selection_and_waits_for_complete_export() {
    let mut fixture = fixture(3);
    let app = &mut fixture.app;
    settled(app);
    let full = app.repo.search(&app.query(0).unwrap()).unwrap();
    let first = full.hits[0].clone();
    let selected = full.hits[1].clone();
    app.hits = vec![selected.clone()];
    app.selected = 0;
    app.total = 1;
    app.provisional = true;
    app.build_status = Some(InitialBuildStatus::default());
    app.generation += 1;
    app.inflight = 1;
    app.searching = true;
    app.requested_offset = Some(0);
    app.sender
        .send(Update {
            generation: app.generation,
            offset: 0,
            elapsed: Duration::ZERO,
            result: Ok(SearchResult {
                count: None,
                hits: vec![first.clone(), first],
                counters: full.counters.clone(),
            }),
            complete: false,
            delta: true,
            view: None,
        })
        .unwrap();
    app.tick(Instant::now());
    assert!(app.provisional);
    assert_eq!(app.hits.len(), 2);
    assert_eq!(app.hits[app.selected].record_id, selected.record_id);
    app.export_input = Input::with_text(fixture._temp.path().join("all.json").to_str().unwrap());
    app.finish_export(true);
    assert_eq!(
        app.status.as_deref(),
        Some("Export all waits for the complete index")
    );
    assert!(!fixture._temp.path().join("all.json").exists());

    app.build_status = Some(InitialBuildStatus {
        phase: InitialBuildPhase::Complete,
        construction_complete: true,
        ..InitialBuildStatus::default()
    });
    app.inflight = 1;
    app.searching = true;
    app.requested_offset = Some(0);
    app.sender
        .send(Update {
            generation: app.generation,
            offset: 0,
            elapsed: Duration::ZERO,
            result: Ok(full),
            complete: true,
            delta: false,
            view: None,
        })
        .unwrap();
    app.tick(Instant::now());
    assert!(!app.provisional);
    assert_eq!(app.total, 4);
    assert_eq!(app.hits[app.selected].record_id, selected.record_id);
}

#[test]
fn cli_columns_follow_filter_and_sort_without_schema_defaults() {
    let mut fixture = fixture(3);
    let app = &mut fixture.app;
    app.single_collection = true;
    app.scope = 1;
    app.request_search();
    settled(app);
    assert_eq!(app.total, 3);
    assert!(app.column_keys().is_empty());
    assert_eq!(app.display_rows.len(), 3);
    assert!(app.last_projection_duration.is_some());
    assert!(app.display_rows.iter().all(|row| row.matched.is_empty()));

    app.filter = Input::with_text("number >= 1 SORT BY title");
    app.request_search();
    settled(app);
    assert_eq!(app.total, 2);
    assert_eq!(app.column_keys(), ["number", "title"]);
    assert_eq!(app.display_rows[0].fields.len(), 2);

    app.filter = Input::with_text("number >= 1 SORT BY number DESC");
    app.request_search();
    settled(app);
    assert_eq!(app.column_keys(), ["number"]);
    app.filter = Input::with_text("number >");
    app.request_search();
    assert!(app.column_keys().is_empty());
    assert!(app.hits.is_empty());
}

#[test]
fn cli_navigation_keeps_only_three_result_pages() {
    let mut fixture = fixture(520);
    let app = &mut fixture.app;
    app.single_collection = true;
    app.scope = 1;
    app.request_search();
    settled(app);
    for _ in 0..6 {
        app.move_selection(isize::MAX);
        settled(app);
        assert!(app.hits.len() <= 3 * PAGE_SIZE);
        assert_eq!(app.display_rows.len(), app.hits.len());
    }
    assert!(app.window_start > 0);
    let previous = app.window_start;
    app.move_selection(-(app.selected as isize));
    app.move_selection(-1);
    settled(app);
    assert!(app.window_start < previous);
}

#[test]
fn cli_field_labels_include_source_id_and_both_comparison_operands() {
    let filter = parse_filter("id = 'A' || (number = title && @record.id = 'x')").unwrap();
    let mut fields = Vec::new();
    collect_filter_fields(&filter, &mut fields);
    assert_eq!(fields, ["@root.id", "number", "title"]);
}

#[test]
fn full_text_filter_and_collection_scope_use_new_core() {
    let mut fixture = fixture(3);
    settled(&mut fixture.app);
    assert_eq!(fixture.app.total, 4);
    assert!(fixture.app.last_search_duration.is_some());
    fixture.app.search = Input::with_text("needle");
    fixture.app.request_search();
    assert!(fixture.app.last_search_duration.is_none());
    settled(&mut fixture.app);
    assert_eq!(fixture.app.total, 2);
    assert!(fixture.app.last_search_duration.is_some());
    fixture.app.filter = Input::with_text("number >= 1");
    fixture.app.request_search();
    settled(&mut fixture.app);
    assert_eq!(fixture.app.total, 0);
    fixture.app.filter.clear();
    fixture.app.picker = 1;
    fixture.app.choose_scope();
    settled(&mut fixture.app);
    assert_eq!(fixture.app.scope_label(), "docs");
    assert_eq!(fixture.app.total, 1);
    assert!(fixture.app.hits[0].payload.is_some());
}

#[test]
fn search_modes_use_core_semantics_and_saved_queries_restore_them() {
    let mut fixture = fixture(3);
    let app = &mut fixture.app;
    app.search = Input::with_text("Needle");
    app.request_search();
    settled(app);
    assert_eq!(app.total, 0);
    app.toggle_case();
    settled(app);
    assert_eq!(app.total, 2);
    app.search = Input::with_text("^NEEDLE");
    app.toggle_regex();
    settled(app);
    assert_eq!(app.total, 2);
    let saved = app.current_query();
    app.toggle_case();
    settled(app);
    assert_eq!(app.total, 0);
    app.restore_query(saved);
    settled(app);
    assert!(!app.case_sensitive);
    assert!(app.regex);
    assert_eq!(app.total, 2);
}

#[test]
fn record_find_uses_the_same_non_turkic_full_fold_policy() {
    assert_eq!(fold_find("Straße Σ İ"), "strasse σ i\u{307}");
}

#[test]
fn pages_load_before_selection_reaches_the_end() {
    let mut fixture = fixture(130);
    settled(&mut fixture.app);
    assert_eq!(fixture.app.total, 131);
    assert_eq!(fixture.app.hits.len(), PAGE_SIZE);
    fixture.app.move_selection(75);
    settled(&mut fixture.app);
    assert_eq!(fixture.app.hits.len(), 131);
}

#[test]
fn invalid_filter_reports_error_and_stale_result_is_ignored() {
    let mut fixture = fixture(3);
    fixture.app.filter = Input::with_text("year >");
    fixture.app.request_search();
    assert!(fixture.app.error.is_some());
    assert!(fixture.app.last_search_duration.is_none());
    fixture.app.filter = Input::with_text("number >= 2");
    fixture.app.request_search();
    settled(&mut fixture.app);
    assert_eq!(fixture.app.total, 1);
    assert!(fixture.app.error.is_none());
}

#[test]
fn sort_choices_apply_only_to_the_selected_collection() {
    let mut fixture = fixture(3);
    fixture.app.picker = 1;
    fixture.app.choose_scope();
    fixture.app.sort_picker = 2;
    fixture.app.choose_sort();
    settled(&mut fixture.app);
    assert_eq!(fixture.app.total, 3);
    assert_eq!(fixture.app.hits[0].record_id, "0002");
    assert_eq!(fixture.app.hits[2].record_id, "0000");
}

#[test]
fn result_columns_prioritize_matches_filters_and_sort() {
    let mut fixture = fixture(3);
    let app = &mut fixture.app;
    app.picker = 1;
    app.choose_scope();
    settled(app);
    app.sort = Some(SortField::simple("number", false));
    assert_eq!(app.visible_columns(), ["number", "title"]);

    app.search = Input::with_text("needle");
    app.request_search();
    settled(app);
    assert_eq!(app.visible_columns(), ["title", "number"]);

    app.search.clear();
    app.filter = Input::with_text("number >= 0");
    app.sort = None;
    assert_eq!(app.visible_columns(), ["number", "title"]);
}

#[test]
fn history_saved_queries_and_columns_persist_in_old_settings_format() {
    let mut fixture = fixture(3);
    let app = &mut fixture.app;
    app.search = Input::with_text("needle");
    app.picker = 1;
    app.choose_scope();
    app.confirm_query();
    settled(app);
    assert_eq!(app.settings.history.len(), 1);
    app.save_input = Input::with_text("Needle");
    app.save_query();
    assert_eq!(app.settings.saved.len(), 1);
    let year = app
        .column_keys()
        .iter()
        .position(|key| key == "number")
        .unwrap();
    app.column_picker = year;
    app.toggle_column();
    assert!(!app.visible_columns().contains(&"number".to_owned()));
    let loaded = Settings::load(&app.settings_path).unwrap();
    assert_eq!(loaded.history, app.settings.history);
    assert_eq!(loaded.saved, app.settings.saved);
    assert_eq!(loaded.hidden(Some("docs")), ["number"]);
    app.search = Input::with_text("other");
    app.restore_query(loaded.saved[0].query.clone());
    assert_eq!(app.search.text, "needle");
    assert_eq!(app.scope_label(), "docs");
}

#[test]
fn selected_and_complete_exports_write_json_without_overwriting() {
    let mut fixture = fixture(130);
    let app = &mut fixture.app;
    settled(app);
    let path = app.repo.files().root.join("all.json");
    app.export_input = Input::with_text(path.to_str().unwrap());
    app.finish_export(true);
    let exported: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(exported["total"], 131);
    assert_eq!(exported["hits"].as_array().unwrap().len(), 131);
    app.finish_export(true);
    assert!(app.status.as_deref().unwrap().contains("Export failed"));
    let one = app.repo.files().root.join("one.json");
    app.export_input = Input::with_text(one.to_str().unwrap());
    app.finish_export(false);
    let exported: serde_json::Value = serde_json::from_slice(&std::fs::read(one).unwrap()).unwrap();
    assert!(exported["record"]["title"].is_string());
}
