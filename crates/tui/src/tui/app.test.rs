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
