use super::*;
use crate::tui::app::View;
use objekte_core::Repository;
use std::fs;


fn repository() -> (tempfile::TempDir, Repository) {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(
        collection.join("a.json"),
        r#"{"title":"Moon","year":1800,"people":[{"name":"Ada","role":"author"},{"name":"Bob","role":"editor"}],"odd key":"x"}"#,
    )
    .unwrap();
    fs::write(collection.join("b.json"), r#"{"title":"Sun","year":1900}"#).unwrap();
    let repo = Repository::open(temp.path()).unwrap();
    (temp, repo)
}

fn open<'a>(repo: &'a mut Repository) -> App<'a> {
    let mut app = App::new(repo);
    app.search = Input::with_text("Moon");
    app.search();
    app.open_detail();
    app
}

/// Put the cursor on the row with this label.
fn select(app: &mut App<'_>, label: &str) {
    let layout = app.record_layout().unwrap();
    app.record.cursor = layout
        .rows
        .iter()
        .position(|row| row.name.trim() == label)
        .unwrap_or_else(|| panic!("no row {label}"));
}


#[test]
fn rows_carry_pointers_and_filter_paths() {
    let (_temp, mut repo) = repository();
    let app = open(&mut repo);
    let layout = app.record_layout().unwrap();
    let row = |label: &str| {
        layout
            .rows
            .iter()
            .find(|row| row.name.trim() == label)
            .unwrap()
    };
    assert_eq!(row("[1] name").pointer, "/people/1/name");
    assert_eq!(row("[1] name").filter_path, "people.name");
    assert_eq!(row("odd key").filter_path, "[\"odd key\"]");
    assert!(row("people").branch);
    assert_eq!(row("id").filter_path, "@id");
}

#[test]
fn space_folds_a_branch_and_keeps_it_folded_for_the_next_record() {
    let (_temp, mut repo) = repository();
    let mut app = open(&mut repo);
    let before = app.record_layout().unwrap().rows.len();
    select(&mut app, "people");
    app.record_toggle();
    let layout = app.record_layout().unwrap();
    assert_eq!(layout.rows.len(), before - 4);
    let people = layout
        .rows
        .iter()
        .position(|row| row.name == "people")
        .unwrap();
    assert_eq!(layout.hidden[people], 4);
    // On a leaf, Space folds the enclosing branch.
    app.record.toggle("/people");
    select(&mut app, "[0] name");
    app.record_toggle();
    assert_eq!(app.record_layout().unwrap().rows.len(), before - 4);
    assert_eq!(
        app.record_layout().unwrap().rows[app.record.cursor].name,
        "people"
    );
}

#[test]
fn filter_by_value_adds_a_term_before_the_sort_clause() {
    let (_temp, mut repo) = repository();
    let mut app = open(&mut repo);
    app.filter = Input::with_text("year > 0 SORT BY year");
    app.search();
    app.open_detail();
    let layout = app.record_layout().unwrap();
    app.record.cursor = layout
        .rows
        .iter()
        .position(|row| row.pointer == "/people/1/role")
        .unwrap();
    app.record_filter_by_value();
    assert_eq!(
        app.filter.text,
        "year > 0 && people.role ?= 'editor' SORT BY year"
    );
    assert_eq!(app.view, View::Results);
    assert_eq!(app.hits.len(), 1);
    // A branch has no single value.
    app.open_detail();
    select(&mut app, "people");
    app.record_filter_by_value();
    assert!(app.notice.as_ref().unwrap().text.contains("single value"));
}

#[test]
fn copy_gives_values_branches_and_whole_records() {
    let (_temp, mut repo) = repository();
    let mut app = open(&mut repo);
    select(&mut app, "year");
    assert_eq!(app.record_copy_text(false).unwrap(), "1800");
    select(&mut app, "people");
    let branch: Value = serde_json::from_str(&app.record_copy_text(false).unwrap()).unwrap();
    assert_eq!(branch[1]["name"], "Bob");
    let whole: Value = serde_json::from_str(&app.record_copy_text(true).unwrap()).unwrap();
    assert_eq!(whole["title"], "Moon");
}

#[test]
fn find_jumps_between_matching_rows_and_raw_lines() {
    let (_temp, mut repo) = repository();
    let mut app = open(&mut repo);
    app.record.find = "ROLE".into();
    app.record_find_next(true);
    let layout = app.record_layout().unwrap();
    assert_eq!(layout.rows[app.record.cursor].name.trim(), "role");
    app.record_find_next(true);
    assert_eq!(layout.rows[app.record.cursor].pointer, "/people/1/role");
    app.record_find_next(true);
    assert_eq!(
        layout.rows[app.record.cursor].pointer, "/people/0/role",
        "wraps"
    );
    assert_eq!(app.record_find_count(), 2);
    app.record.raw = true;
    app.record_find_next(true);
    let lines = app.record.raw_lines(app.current_detail_hit().unwrap());
    assert!(lines[app.record.scroll.get()].contains("role"));
}

#[test]
fn the_cursor_skips_the_separator_and_stops_at_the_ends() {
    let (_temp, mut repo) = repository();
    let mut app = open(&mut repo);
    app.record_move(-5);
    assert_eq!(app.record.cursor, 0);
    let layout = app.record_layout().unwrap();
    let separator = layout
        .rows
        .iter()
        .position(DetailRow::is_separator)
        .unwrap();
    app.record.cursor = separator - 1;
    app.record_move(1);
    assert_eq!(app.record.cursor, separator + 1);
    app.record_home(true);
    assert_eq!(app.record.cursor, layout.rows.len() - 1);
}

#[test]
fn layouts_are_cached_until_the_record_or_folding_changes() {
    let (_temp, mut repo) = repository();
    let mut app = open(&mut repo);
    let first = app.record_layout().unwrap();
    assert!(Rc::ptr_eq(&first, &app.record_layout().unwrap()));
    app.record.toggle("/people");
    assert!(!Rc::ptr_eq(&first, &app.record_layout().unwrap()));
}

#[test]
fn find_searches_folded_fields_and_unfolds_the_match() {
    let (_temp, mut repo) = repository();
    let mut app = open(&mut repo);
    app.record.toggle("/people");
    app.record.find = "bob".into();
    assert_eq!(app.record_find_count(), 1);
    app.record_find_next(true);
    assert!(app.record.collapsed.is_empty(), "the match is shown");
    let layout = app.record_layout().unwrap();
    assert_eq!(layout.rows[app.record.cursor].pointer, "/people/1/name");
}
