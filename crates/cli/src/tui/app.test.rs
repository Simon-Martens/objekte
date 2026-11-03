use super::*;
use crate::tui::{keys, view};
use objekte_core::Repository;
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
};
use serde_json::json;
use std::{fs, path::Path};


/// `docs` holds `count` records with a number `n`, a title, and a kind.
fn repository(count: usize) -> (tempfile::TempDir, Repository) {
    let temp = tempfile::tempdir().unwrap();
    write_docs(temp.path(), count);
    let repo = Repository::open(temp.path()).unwrap();
    (temp, repo)
}

fn write_docs(root: &Path, count: usize) {
    let collection = root.join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    for n in 0..count {
        let record = json!({"n": n, "title": format!("Moon {n}"), "kind": if n % 2 == 0 { "even" } else { "odd" }});
        fs::write(
            collection.join(format!("{n:04}.json")),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
    }
}

fn press(app: &mut App<'_>, code: KeyCode) {
    keys::key(app, KeyEvent::new(code, KeyModifiers::NONE));
}

fn press_with(app: &mut App<'_>, code: KeyCode, modifiers: KeyModifiers) {
    keys::key(app, KeyEvent::new(code, modifiers));
}

fn type_text(app: &mut App<'_>, text: &str) {
    for ch in text.chars() {
        press(app, KeyCode::Char(ch));
    }
}

/// Let the debounce pass, as the event loop would.
fn settle(app: &mut App<'_>) {
    app.tick(Instant::now() + DEBOUNCE * 2);
}

fn ids(app: &App<'_>) -> Vec<String> {
    app.hits.iter().map(|hit| hit.record().id.clone()).collect()
}

fn screen(app: &App<'_>, width: u16, height: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| view::draw(frame, app)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect()
}


#[test]
fn typing_searches_after_a_pause_not_on_every_key() {
    let (_temp, mut repo) = repository(5);
    let mut app = App::new(&mut repo);
    type_text(&mut app, "Moon");
    assert!(app.last_request.is_none(), "no search before the pause");
    app.tick(Instant::now());
    assert!(app.last_request.is_none());
    settle(&mut app);
    assert_eq!(app.hits.len(), 5);
    assert_eq!(app.total, Some(5));
}

#[test]
fn enter_searches_at_once_and_then_opens_the_selected_record() {
    let (_temp, mut repo) = repository(3);
    let mut app = App::new(&mut repo);
    type_text(&mut app, "Moon");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.hits.len(), 3);
    assert_eq!(app.view, View::Results);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.view, View::Detail);
    assert_eq!(
        app.current_detail_hit().unwrap().record().id,
        app.hits[1].record().id
    );
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.view, View::Results);
    assert_eq!(app.selected, 1);
}

#[test]
fn an_unfinished_filter_keeps_results_and_only_hints_until_enter() {
    let (_temp, mut repo) = repository(4);
    let mut app = App::new(&mut repo);
    app.filter = Input::with_text("n >= 2");
    app.line = Line::Filter;
    app.search();
    assert_eq!(app.hits.len(), 2);
    type_text(&mut app, " && kind");
    settle(&mut app);
    assert!(app.query_error.is_some());
    assert!(!app.error_confirmed, "typing shows a quiet hint");
    assert_eq!(app.hits.len(), 2, "results stay while typing");
    press(&mut app, KeyCode::Enter);
    assert!(app.error_confirmed, "Enter shows the error");
    type_text(&mut app, " = 'odd'");
    settle(&mut app);
    assert!(app.query_error.is_none());
    assert_eq!(ids(&app), ["0003"]);
}

#[test]
fn rows_load_continuously_while_moving_down() {
    let (_temp, mut repo) = repository(PAGE_SIZE * 2 + 10);
    let mut app = App::new(&mut repo);
    app.filter = Input::with_text("n >= 0 SORT BY n");
    app.search();
    assert_eq!(app.hits.len(), PAGE_SIZE);
    assert_eq!(app.total, Some(PAGE_SIZE * 2 + 10));
    app.move_selection((PAGE_SIZE - LOAD_AHEAD) as isize);
    assert_eq!(app.hits.len(), PAGE_SIZE * 2);
    assert_eq!(app.hits[PAGE_SIZE].record().id, format!("{PAGE_SIZE:04}"));
    app.select_last();
    app.select_last();
    assert_eq!(app.hits.len(), PAGE_SIZE * 2 + 10);
    assert_eq!(app.selected, PAGE_SIZE * 2 + 9);
}

#[test]
fn selection_follows_the_record_and_keeps_its_screen_row_on_rerun() {
    let (temp, mut repo) = repository(30);
    let mut app = App::new(&mut repo);
    app.visible_rows.set(10);
    app.filter = Input::with_text("n >= 0 SORT BY n");
    app.search();
    app.move_selection(15);
    let chosen = app.hits[app.selected].record().id.clone();
    let row = app.selected - app.scroll;
    // Another program moves an earlier record behind the chosen one.
    fs::write(
        temp.path().join("data/docs/0002.json"),
        serde_json::to_vec(&json!({"n": 99, "title": "Moon 2", "kind": "even"})).unwrap(),
    )
    .unwrap();
    app.repo.diff("docs").unwrap();
    app.rerun();
    assert_eq!(app.hits[app.selected].record().id, chosen);
    assert_eq!(app.selected - app.scroll, row, "the row stays on screen");
}

#[test]
fn tab_completes_in_filter_and_shift_tab_returns_to_search() {
    let temp = tempfile::tempdir().unwrap();
    let schema = temp.path().join(".objekte/collections/docs.schema.json");
    fs::create_dir_all(schema.parent().unwrap()).unwrap();
    fs::write(
        schema,
        r#"{"type":"object","properties":{"title":{"type":"string"},"n":{"type":"integer"}},"x-objekte":{"name":"docs","path":"data/docs","type":"data","format":"json","id":"filename"}}"#,
    )
    .unwrap();
    write_docs(temp.path(), 2);
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.line, Line::Filter);
    type_text(&mut app, "ti");
    // Suggestions appear while typing; Tab takes the first.
    assert_eq!(app.completion.as_ref().unwrap().items[0].label, "title");
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.filter.text, "title");
    assert_eq!(app.line, Line::Filter);
    type_text(&mut app, " ");
    let open = app.completion.as_ref().expect("operators without Tab");
    assert!(open.items.len() > 1);
    assert!(
        !app.completion_active,
        "opened by a space: nothing highlighted"
    );
    // The first Down highlights the top entry, the second moves on.
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.filter.text, "title != ");
    // Values follow at once: here only date macros fit a text field.
    let values = app.completion.as_ref().expect("values");
    assert!(values.items.iter().any(|item| item.label == "@now"));
    press(&mut app, KeyCode::Esc);
    assert!(app.completion.is_none());
    press(&mut app, KeyCode::BackTab);
    assert_eq!(app.line, Line::Search);
}

#[test]
fn escape_clears_the_active_line_then_everything() {
    let (_temp, mut repo) = repository(2);
    let mut app = App::new(&mut repo);
    app.search = Input::with_text("Moon");
    app.filter = Input::with_text("n > 0");
    app.line = Line::Filter;
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.filter.text, "");
    assert_eq!(app.search.text, "Moon");
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.search.text, "");
    settle(&mut app);
    assert!(app.hits.is_empty());
    assert!(app.last_request.is_none());
}

#[test]
fn collection_picker_scopes_the_search() {
    let temp = tempfile::tempdir().unwrap();
    write_docs(temp.path(), 2);
    let other = temp.path().join("data/notes");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("x.json"), r#"{"title":"Moon note"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.search = Input::with_text("Moon");
    app.search();
    assert_eq!(app.hits.len(), 3);
    press_with(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);
    assert!(matches!(
        app.overlay,
        Some(Overlay::List(crate::tui::lists::ListKind::Collections, _))
    ));
    type_text(&mut app, "note");
    press(&mut app, KeyCode::Enter);
    assert!(app.overlay.is_none());
    assert_eq!(app.scope_name(), Some("notes"));
    assert_eq!(ids(&app), ["x"]);
}

#[test]
fn palette_runs_actions_by_name() {
    let (_temp, mut repo) = repository(2);
    let mut app = App::new(&mut repo);
    press_with(&mut app, KeyCode::Char('k'), KeyModifiers::CONTROL);
    type_text(&mut app, "regex");
    press(&mut app, KeyCode::Enter);
    assert!(app.regex);
    assert!(app.overlay.is_none());
}

#[test]
fn layout_rows_never_move() {
    let (_temp, mut repo) = repository(40);
    let mut app = App::new(&mut repo);
    let check = |screen: &[String], state: &str| {
        assert!(screen[2].contains("Search"), "{state}: {:?}", screen[2]);
        assert!(screen[3].contains("Filter"), "{state}: {:?}", screen[3]);
        assert!(!screen[39].trim().is_empty(), "{state}: footer");
    };
    check(&screen(&app, 120, 40), "empty");
    app.search = Input::with_text("Moon");
    app.search();
    let results = screen(&app, 120, 40);
    check(&results, "results");
    assert!(results[5].contains("id"), "header row: {:?}", results[5]);
    assert!(results[6].starts_with('▌'), "first row: {:?}", results[6]);
    app.filter = Input::with_text("n >=");
    app.search();
    check(&screen(&app, 120, 40), "error");
    app.notice("Saved");
    check(&screen(&app, 120, 40), "notice");
    app.searching = true;
    check(&screen(&app, 120, 40), "searching");
    app.searching = false;
    app.open_detail();
    // The record replaces the query lines, in the same rows.
    let detail = screen(&app, 120, 40);
    assert!(detail[2].contains("1 of 40"), "{:?}", detail[2]);
    assert!(detail[3].contains("1 Fields"), "{:?}", detail[3]);
    assert!(!detail[39].trim().is_empty(), "detail footer");
}

#[test]
fn the_selected_row_keeps_its_marker_column() {
    let (_temp, mut repo) = repository(5);
    let mut app = App::new(&mut repo);
    app.scope = 1;
    app.search = Input::with_text("Moon");
    app.search();
    let first = screen(&app, 100, 20);
    let second_id = app.hits[1].record().id.clone();
    app.move_selection(1);
    let second = screen(&app, 100, 20);
    let column = |line: &str| {
        let byte = line.find(&second_id).unwrap();
        line[..byte].chars().count()
    };
    assert!(second[7].starts_with('▌'), "{:?}", second[7]);
    assert_eq!(column(&first[7]), column(&second[7]), "text never shifts");
}

#[test]
fn narrow_terminals_hide_the_preview() {
    let (_temp, mut repo) = repository(3);
    let mut app = App::new(&mut repo);
    app.search = Input::with_text("Moon");
    app.search();
    let wide = screen(&app, 140, 30);
    assert!(wide.iter().any(|line| line.contains("│")), "preview rule");
    let narrow = screen(&app, 80, 30);
    assert!(!narrow[6].contains("│"), "{:?}", narrow[6]);
}

#[test]
fn option_chips_are_bold_exactly_when_active() {
    use ratatui::style::Modifier;
    let (_temp, mut repo) = repository(1);
    let mut app = App::new(&mut repo);
    let bold = |app: &App<'_>, label: &str| {
        let mut terminal = Terminal::new(TestBackend::new(120, 10)).unwrap();
        terminal.draw(|frame| view::draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let line = (0..120)
            .map(|x| buffer[(x, 2)].symbol().to_string())
            .collect::<Vec<_>>();
        let start = (0..120)
            .find(|&x| line[x..].concat().starts_with(label))
            .unwrap_or_else(|| panic!("{label} not on the Search line"));
        buffer[(start as u16, 2)].modifier.contains(Modifier::BOLD)
    };
    assert!(!bold(&app, "Aa"));
    assert!(!bold(&app, ".*"));
    assert!(
        bold(&app, "txt"),
        "the current text mode is always shown as active"
    );
    app.toggle_case();
    app.toggle_representation();
    assert!(bold(&app, "Aa"));
    assert!(bold(&app, "</>"));
    app.toggle_regex();
    assert!(bold(&app, ".*"));
}

#[test]
fn array_cells_show_only_their_length() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(
        collection.join("a.json"),
        r#"{"tags":[{"a":1},{"a":2},{"a":3}]}"#,
    )
    .unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let record = repo.get_record("docs", "a").unwrap().unwrap();
    assert_eq!(
        view::results::cell_text(&Hit::Filter(record), "tags"),
        "[3]"
    );
}

#[test]
fn control_d_and_u_page_through_results() {
    let (_temp, mut repo) = repository(60);
    let mut app = App::new(&mut repo);
    app.visible_rows.set(10);
    app.search = Input::with_text("Moon");
    app.search();
    press_with(&mut app, KeyCode::Char('d'), KeyModifiers::CONTROL);
    assert_eq!(app.selected, 9);
    press_with(&mut app, KeyCode::Char('d'), KeyModifiers::CONTROL);
    assert_eq!(app.selected, 18);
    press_with(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
    assert_eq!(app.selected, 9);
    assert_eq!(app.search.text, "Moon", "paging does not edit the line");
}

#[test]
fn clicks_select_rows_and_a_double_click_opens_the_record() {
    let (_temp, mut repo) = repository(5);
    let mut app = App::new(&mut repo);
    app.search = Input::with_text("Moon");
    app.search();
    screen(&app, 120, 30);
    let area = app.table_area.get();
    let start = Instant::now();
    keys::click(&mut app, area.x + 4, area.y + 3, start);
    assert_eq!(app.selected, 2);
    assert_eq!(app.view, View::Results);
    keys::click(
        &mut app,
        area.x + 4,
        area.y + 3,
        start + Duration::from_millis(200),
    );
    assert_eq!(app.view, View::Detail);
    // In the record, clicks pick the tab and the field.
    screen(&app, 120, 30);
    let tabs = app.tabs_area.get();
    keys::click(&mut app, tabs.x + 18, tabs.y, start);
    assert_eq!(
        app.links.section,
        crate::tui::navigation::DetailSection::Links
    );
    keys::click(&mut app, tabs.x + 5, tabs.y, start);
    screen(&app, 120, 30);
    let fields = app.record_area.get();
    keys::click(
        &mut app,
        fields.x + 2,
        fields.y + 1,
        start + Duration::from_secs(5),
    );
    assert_eq!(app.record.cursor, 1);
}

#[test]
fn value_completion_reads_counts_from_the_index() {
    let temp = tempfile::tempdir().unwrap();
    let schema = temp.path().join(".objekte/collections/docs.schema.json");
    fs::create_dir_all(schema.parent().unwrap()).unwrap();
    fs::write(
        schema,
        r#"{"type":"object","properties":{"kind":{"type":"string","index":true},"n":{"type":"integer"}},"x-objekte":{"name":"docs","path":"data/docs","type":"data","format":"json","id":"filename"}}"#,
    )
    .unwrap();
    write_docs(temp.path(), 5);
    let mut repo = Repository::open(temp.path()).unwrap();
    repo.diff("docs").unwrap();
    let mut app = App::new(&mut repo);
    app.filter = Input::with_text("kind = ");
    app.line = Line::Filter;
    app.refresh_completion(false);
    let found = app.completion.as_ref().expect("values");
    let shown = found
        .items
        .iter()
        .map(|item| (item.label.as_str(), item.detail.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(shown, [("even", "3 records"), ("odd", "2 records")]);
}

#[test]
fn suggestions_appear_while_typing_and_enter_still_searches() {
    let temp = tempfile::tempdir().unwrap();
    let schema = temp.path().join(".objekte/collections/docs.schema.json");
    fs::create_dir_all(schema.parent().unwrap()).unwrap();
    fs::write(
        schema,
        r#"{"type":"object","properties":{"kind":{"type":"string","index":true},"n":{"type":"integer"}},"x-objekte":{"name":"docs","path":"data/docs","type":"data","format":"json","id":"filename"}}"#,
    )
    .unwrap();
    write_docs(temp.path(), 4);
    let mut repo = Repository::open(temp.path()).unwrap();
    repo.diff("docs").unwrap();
    let mut app = App::new(&mut repo);
    app.line = Line::Filter;
    assert!(app.completion.is_none(), "nothing on an empty line");
    type_text(&mut app, "kind = ");
    let values = app.completion.as_ref().expect("values while typing");
    assert_eq!(values.items[0].label, "even");
    type_text(&mut app, "'odd'");
    // Enter searches; it does not take a suggestion nobody picked.
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.filter.text, "kind = 'odd'");
    assert!(app.completion.is_none());
    assert_eq!(app.hits.len(), 2);
    // After a value, the connectives are offered and narrow as you type.
    type_text(&mut app, " ");
    let next = app.completion.as_ref().expect("connectives");
    assert_eq!(next.items.len(), 3);
    type_text(&mut app, "S");
    let narrowed = app.completion.as_ref().expect("narrowed");
    assert_eq!(narrowed.items[0].label, "SORT BY");
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.filter.text, "kind = 'odd' SORT BY ");
    let fields = app.completion.as_ref().expect("sort fields");
    assert!(fields.items.iter().any(|item| item.label == "kind"));
    // Moving the cursor keeps the list current.
    press(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::Right);
}

#[test]
fn value_counts_survive_searches_and_misses_are_not_kept() {
    let temp = tempfile::tempdir().unwrap();
    let schema = temp.path().join(".objekte/collections/docs.schema.json");
    fs::create_dir_all(schema.parent().unwrap()).unwrap();
    fs::write(
        schema,
        r#"{"type":"object","properties":{"kind":{"type":"string","index":true},"n":{"type":"integer"}},"x-objekte":{"name":"docs","path":"data/docs","type":"data","format":"json","id":"filename"}}"#,
    )
    .unwrap();
    write_docs(temp.path(), 4);
    let mut repo = Repository::open(temp.path()).unwrap();
    repo.diff("docs").unwrap();
    let mut app = App::new(&mut repo);
    app.filter = Input::with_text("n = ");
    app.line = Line::Filter;
    app.refresh_completion(false);
    assert!(
        app.value_counts.borrow().is_empty(),
        "n is not indexed: no entry"
    );
    app.filter = Input::with_text("kind = ");
    app.refresh_completion(false);
    assert_eq!(app.value_counts.borrow().len(), 1);
    app.search();
    assert_eq!(app.value_counts.borrow().len(), 1, "kept across searches");
}

#[test]
fn the_error_position_is_only_marked_after_enter() {
    use ratatui::style::Modifier;
    let (_temp, mut repo) = repository(2);
    let mut app = App::new(&mut repo);
    app.line = Line::Filter;
    type_text(&mut app, "n >");
    settle(&mut app);
    assert!(app.query_error.is_some());
    let marked = |app: &App<'_>| {
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal.draw(|frame| view::draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..80).any(|x| buffer[(x, 3)].modifier.contains(Modifier::UNDERLINED))
    };
    assert!(!marked(&app), "no marker while typing");
    press(&mut app, KeyCode::Enter);
    assert!(app.error_confirmed);
    assert!(marked(&app), "Enter marks the position");
}

#[test]
fn enter_inserts_the_highlighted_suggestion_like_vs_code() {
    let temp = tempfile::tempdir().unwrap();
    let schema = temp.path().join(".objekte/collections/docs.schema.json");
    fs::create_dir_all(schema.parent().unwrap()).unwrap();
    fs::write(
        schema,
        r#"{"type":"object","properties":{"kind":{"type":"string","index":true},"n":{"type":"integer"},"title":{"type":"string"}},"x-objekte":{"name":"docs","path":"data/docs","type":"data","format":"json","id":"filename"}}"#,
    )
    .unwrap();
    write_docs(temp.path(), 4);
    let mut repo = Repository::open(temp.path()).unwrap();
    repo.diff("docs").unwrap();
    let mut app = App::new(&mut repo);
    app.line = Line::Filter;
    // Typing highlights the best match and shows the rest as a ghost.
    type_text(&mut app, "ki");
    assert!(app.completion_active);
    assert_eq!(app.completion_ghost().as_deref(), Some("nd"));
    // Typing on stays free: the ghost is not text.
    type_text(&mut app, "n");
    assert_eq!(app.filter.text, "kin");
    assert_eq!(app.completion_ghost().as_deref(), Some("d"));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.filter.text, "kind");
    assert!(app.completion.is_none(), "the list closes after a name");
    // After a space nothing is highlighted, so Enter would search.
    type_text(&mut app, " ");
    assert!(app.completion.is_some());
    assert!(!app.completion_active);
    assert_eq!(app.completion_ghost(), None);
    type_text(&mut app, "= ");
    assert!(
        !app.completion_active,
        "values after an operator start plain"
    );
    type_text(&mut app, "'e");
    assert_eq!(app.completion_ghost().as_deref(), Some("ven'"));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.filter.text, "kind = 'even'");
    assert!(app.completion.is_none());
    // Enter on a closed list searches.
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.hits.len(), 2);
}

#[test]
fn enter_searches_when_the_suggestion_is_already_typed() {
    let temp = tempfile::tempdir().unwrap();
    let schema = temp.path().join(".objekte/collections/docs.schema.json");
    fs::create_dir_all(schema.parent().unwrap()).unwrap();
    fs::write(
        schema,
        r#"{"type":"object","properties":{"n":{"type":"integer"},"name":{"type":"string"}},"x-objekte":{"name":"docs","path":"data/docs","type":"data","format":"json","id":"filename"}}"#,
    )
    .unwrap();
    write_docs(temp.path(), 2);
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.line = Line::Filter;
    type_text(&mut app, "n");
    // `n` and `name` both match; the exact `n` is highlighted.
    assert!(app.completion_active);
    assert!(app.completion_is_typed());
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.filter.text, "n");
    assert!(app.completion.is_none());
    assert!(app.error_confirmed, "Enter searched the unfinished filter");
}

#[test]
fn highlights_ignore_punctuation_like_the_search() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(collection.join("a.json"), r#"{"title":"„Goethe“, Johann – Faust"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.search = Input::with_text("goethe johann faust");
    app.search();
    assert_eq!(app.hits.len(), 1);
    let highlight = app.highlight.as_ref().unwrap();
    assert_eq!(
        highlight.find("„Goethe“, Johann – Faust").unwrap().as_str(),
        "Goethe“, Johann – Faust"
    );
}
