use super::*;
use crate::tui::app::{App, Overlay};
use objekte_core::Repository;
use serde_json::json;
use std::fs;


fn repository() -> (tempfile::TempDir, Repository) {
    let temp = tempfile::tempdir().unwrap();
    for (collection, id, record) in [
        (
            "books",
            "a",
            json!({"title":"Moon","kind":"poem","tags":["x","y"]}),
        ),
        (
            "books",
            "b",
            json!({"title":"Moon river","kind":"prose","tags":["x"]}),
        ),
        ("books", "c", json!({"title":"Moon light","kind":"poem"})),
        ("notes", "n", json!({"title":"Moon note"})),
    ] {
        let path = temp.path().join(format!("data/{collection}/{id}.json"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, record.to_string()).unwrap();
    }
    let repo = Repository::open(temp.path()).unwrap();
    (temp, repo)
}

fn searched<'a>(repo: &'a mut Repository, text: &str) -> App<'a> {
    let mut app = App::new(repo);
    app.search = Input::with_text(text);
    app.search();
    app
}

fn labels(app: &App<'_>, kind: ListKind, filter: &str) -> Vec<String> {
    app.list_items(kind, filter)
        .into_iter()
        .map(|item| item.label)
        .collect()
}


#[test]
fn breakdown_counts_values_over_the_whole_result_and_filters_by_one() {
    let (_temp, mut repo) = repository();
    let mut app = searched(&mut repo, "Moon");
    app.defer(Deferred::Breakdown("kind".into()));
    assert_eq!(app.run_deferred(), None);
    let items = app.list_items(ListKind::Breakdown, "");
    let counts = items
        .iter()
        .map(|item| (item.label.as_str(), item.note.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(counts, [("poem", "2"), ("(missing)", "1"), ("prose", "1")]);
    assert!(matches!(
        app.overlay,
        Some(Overlay::List(ListKind::Breakdown, _))
    ));
    app.list_accept(ListKind::Breakdown, items[0].value.clone());
    assert_eq!(app.filter.text, "kind = 'poem'");
    assert_eq!(app.hits.len(), 2);
}

#[test]
fn breakdown_of_an_array_field_filters_with_any() {
    let (_temp, mut repo) = repository();
    let mut app = searched(&mut repo, "Moon");
    app.run_breakdown("tags");
    let breakdown = app.breakdown.as_ref().unwrap();
    assert!(breakdown.many);
    assert!(breakdown.counts.contains(&(Some(json!("x")), 2)));
    assert!(
        breakdown.counts.contains(&(None, 2)),
        "c and the note have no tags"
    );
    let value = app.list_items(ListKind::Breakdown, "y")[0].value.clone();
    app.list_accept(ListKind::Breakdown, value);
    assert_eq!(app.filter.text, "tags ?= 'y'");
    assert_eq!(app.hits.len(), 1);
}

#[test]
fn collection_breakdown_and_picker_counts_use_every_collection() {
    let (_temp, mut repo) = repository();
    let mut app = searched(&mut repo, "Moon");
    app.set_scope(1);
    app.open_list(ListKind::Collections);
    app.run_deferred();
    let notes = app
        .list_items(ListKind::Collections, "")
        .into_iter()
        .map(|item| (item.label, item.note))
        .collect::<Vec<_>>();
    assert_eq!(notes[0], ("all collections".into(), "4".into()));
    assert_eq!(notes[1], ("books".into(), "current · 3".into()));
    assert_eq!(notes[2], ("notes".into(), "1".into()));
}

#[test]
fn sort_by_replaces_the_clause_on_the_line_that_has_it() {
    let (_temp, mut repo) = repository();
    let mut app = searched(&mut repo, "Moon");
    app.filter = Input::with_text("kind = 'poem' SORT BY title");
    app.set_sort("title", true);
    assert_eq!(app.filter.text, "kind = 'poem' SORT BY title DESC");
    app.search = Input::with_text("Moon SORT BY kind");
    app.set_sort("@modified", false);
    assert_eq!(app.search.text, "Moon SORT BY @modified");
    assert!(labels(&app, ListKind::SortBy, "title").contains(&"title ↓".to_string()));
}

#[test]
fn history_and_saved_queries_restore_their_lines_and_collection() {
    let (_temp, mut repo) = repository();
    let mut app = searched(&mut repo, "Moon");
    app.set_scope(2);
    app.remember_query();
    app.save_query("Notes about the moon");
    app.search = Input::with_text("other");
    app.set_scope(0);
    let history = app.list_items(ListKind::History, "");
    assert_eq!(history[0].label, "Moon");
    assert_eq!(history[0].note, "notes");
    let saved = app.list_items(ListKind::Saved, "moon");
    app.list_accept(ListKind::Saved, saved[0].value.clone());
    assert_eq!(app.search.text, "Moon");
    assert_eq!(app.scope_name(), Some("notes"));
    assert_eq!(app.hits.len(), 1);
    app.list_remove(&history[0].value);
    assert!(app.settings.history.is_empty());
}

#[test]
fn hidden_columns_are_remembered_per_collection() {
    let (_temp, mut repo) = repository();
    let mut app = searched(&mut repo, "Moon");
    let title = ListValue::Column("title".into());
    app.list_accept(ListKind::Columns, title.clone());
    let items = app.list_items(ListKind::Columns, "title");
    assert_eq!(items[0].checked, Some(false));
    assert_eq!(app.settings.hidden(None), ["title"]);
    app.list_accept(ListKind::Columns, title);
    assert!(app.settings.hidden(None).is_empty());
}

#[test]
fn lists_filter_by_substring_and_in_order_letters() {
    let (_temp, mut repo) = repository();
    let app = searched(&mut repo, "Moon");
    assert_eq!(
        labels(&app, ListKind::Palette, "this query"),
        ["Save this query…"]
    );
    assert_eq!(
        labels(&app, ListKind::Palette, "svq"),
        ["Saved queries…", "Save this query…"]
    );
    assert!(
        labels(&app, ListKind::Palette, "colour")
            .iter()
            .any(|label| label.contains("colours"))
    );
}
