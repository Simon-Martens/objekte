use super::test_support::{fixture, relation_fixture};
use super::*;

#[test]
fn cli_mode_keeps_shortcuts_and_restricts_collection_and_links() {
    let mut fixture = fixture(2);
    let app = &mut fixture.app;
    app.single_collection = true;
    app.scope = 1;
    app.request_search();
    wait_for_hits(app);
    key(
        app,
        KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
    );
    assert_eq!(app.overlay, None);
    assert_eq!(app.scope_label(), "docs");
    key(app, KeyEvent::new(KeyCode::Char('s'), KeyModifiers::ALT));
    assert_eq!(app.overlay, Some(Overlay::Sort));
    for ch in "title DESC".chars() {
        key(app, KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    key(app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.sort.is_some());
    wait_for_hits(app);
    key(app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.open_detail();
    assert!(app.relation_rows.is_empty());
    key(app, KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));
    assert!(app.relation_rows.is_empty());
}

#[test]
fn keys_switch_inputs_open_detail_and_choose_collection() {
    let mut fixture = fixture(2);
    let app = &mut fixture.app;
    for _ in 0..1000 {
        app.tick(Instant::now());
        if !app.searching {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    key(app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.focus, Focus::Filter);
    key(app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.focus, Focus::Results);
    key(app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.view, View::Detail);
    key(app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.view, View::Results);
    key(
        app,
        KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
    );
    assert_eq!(app.overlay, Some(Overlay::Collections));
    key(app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    key(app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.scope_label(), "docs");
    key(app, KeyEvent::new(KeyCode::Char('s'), KeyModifiers::ALT));
    assert_eq!(app.overlay, Some(Overlay::Sort));
    key(app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    key(app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.sort.is_some());
    key(
        app,
        KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
    );
    assert!(!app.show_preview);
}

#[test]
fn old_search_mode_shortcuts_toggle_new_core_requests() {
    let mut fixture = fixture(2);
    let app = &mut fixture.app;
    assert!(app.case_sensitive);
    assert!(!app.regex);
    key(app, KeyEvent::new(KeyCode::Char('c'), KeyModifiers::ALT));
    key(app, KeyEvent::new(KeyCode::Char('r'), KeyModifiers::ALT));
    assert!(!app.case_sensitive);
    assert!(app.regex);
}

#[test]
fn action_palette_opens_existing_controls_and_columns_scroll() {
    let mut fixture = fixture(2);
    let app = &mut fixture.app;
    key(
        app,
        KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL),
    );
    assert_eq!(app.overlay, Some(Overlay::Actions));
    key(app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    key(app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    key(app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.overlay, Some(Overlay::Columns));
    key(app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    key(app, KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
    assert_eq!(app.column_offset, 1);
    key(app, KeyEvent::new(KeyCode::Left, KeyModifiers::ALT));
    assert_eq!(app.column_offset, 0);
}

#[test]
fn record_find_copy_and_adjacent_result_shortcuts_work() {
    let mut fixture = fixture(3);
    let app = &mut fixture.app;
    wait_for_hits(app);
    app.selected = app
        .hits
        .iter()
        .position(|hit| hit.record_id == "0001")
        .unwrap();
    app.open_detail();
    key(app, KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    app.record.find_input = Some(input::Input::with_text("ordinary"));
    key(app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let rows = app.record.rows(app.current_detail_hit().unwrap(), None);
    assert_eq!(rows[app.record.cursor].value, "ordinary title");
    key(app, KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
    assert_eq!(app.clipboard.as_deref(), Some("ordinary title"));
    key(app, KeyEvent::new(KeyCode::Char('Y'), KeyModifiers::NONE));
    assert!(app.clipboard.as_deref().unwrap().contains("ordinary title"));
    key(app, KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
    assert_eq!(app.current_detail_hit().unwrap().record_id, "0002");
    key(app, KeyEvent::new(KeyCode::Left, KeyModifiers::ALT));
    assert_eq!(app.current_detail_hit().unwrap().record_id, "0001");
}

#[test]
fn section_keys_jump_to_outgoing_and_backlinks() {
    let mut fixture = relation_fixture();
    let app = &mut fixture.app;
    wait_for_hits(app);
    select_id(app, "A");
    key(app, KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));
    let base = app
        .record
        .rows(app.current_detail_hit().unwrap(), None)
        .len();
    assert_eq!(
        app.relation_rows[app.record.cursor - base].label,
        "Outgoing"
    );
    key(app, KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE));
    assert!(app.status.as_deref().unwrap().contains("No Backlinks"));
    key(app, KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE));
    assert_eq!(app.record.cursor, 0);
}

#[test]
fn tab_cycles_available_record_sections_and_shift_tab_reverses() {
    let mut fixture = relation_fixture();
    let app = &mut fixture.app;
    wait_for_hits(app);
    select_id(app, "A");
    app.relation_rows.push(relations::RelationRow {
        label: "Backlinks".into(),
        value: "entry_mentions".into(),
        action: None,
    });
    let base = app
        .record
        .rows(app.current_detail_hit().unwrap(), None)
        .len();
    let outgoing = base
        + app
            .relation_rows
            .iter()
            .position(|row| row.label == "Outgoing")
            .unwrap();
    let backlinks = base
        + app
            .relation_rows
            .iter()
            .position(|row| row.label == "Backlinks")
            .unwrap();
    key(app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.record.cursor, outgoing);
    let backend = ratatui::backend::TestBackend::new(80, 10);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|frame| view::draw(frame, app)).unwrap();
    assert!(app.record.visible_rows.contains(&outgoing));
    key(app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.record.cursor, backlinks);
    key(app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.record.cursor, 0);
    key(app, KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(app.record.cursor, backlinks);
    key(app, KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT));
    assert_eq!(app.record.cursor, outgoing);

    app.back_detail();
    select_id(app, "m1");
    let base = app
        .record
        .rows(app.current_detail_hit().unwrap(), None)
        .len();
    key(app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(
        app.relation_rows[app.record.cursor - base].label,
        "Backlinks"
    );
    key(app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.record.cursor, 0);
}

fn wait_for_hits(app: &mut App) {
    for _ in 0..1_000 {
        app.tick(Instant::now());
        if !app.searching {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!app.searching);
}

fn select_id(app: &mut App, id: &str) {
    app.selected = app.hits.iter().position(|hit| hit.record_id == id).unwrap();
    app.open_detail();
}

fn choose_link(app: &mut App, label: &str) {
    let record_rows = app
        .record
        .rows(app.current_detail_hit().unwrap(), None)
        .len();
    let index = app
        .relation_rows
        .iter()
        .position(|row| row.label.trim() == label)
        .unwrap();
    app.record.cursor = record_rows + index;
    key(app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
}

#[test]
fn enter_navigates_outgoing_and_backlinks_with_history_and_dangling_states() {
    let mut fixture = relation_fixture();
    let app = &mut fixture.app;
    wait_for_hits(app);
    select_id(app, "A");
    assert!(app.relation_rows.iter().any(|row| row.label.trim() == "m1"));
    choose_link(app, "m1");
    assert_eq!(app.current_detail_hit().unwrap().record_id, "m1");
    assert!(app.relation_rows.iter().any(|row| row.label.trim() == "A"));
    choose_link(app, "A");
    assert_eq!(app.current_detail_hit().unwrap().record_id, "A");
    key(app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.current_detail_hit().unwrap().record_id, "m1");
    key(app, KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    assert_eq!(app.current_detail_hit().unwrap().record_id, "A");
    key(app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.view, View::Results);

    select_id(app, "B");
    assert!(
        app.relation_rows
            .iter()
            .any(|row| row.value.contains("dangling"))
    );
    assert!(
        app.relation_rows
            .iter()
            .all(|row| row.label.trim() != "lost" || row.action.is_none())
    );
    app.back_detail();
    select_id(app, "C");
    assert!(
        app.relation_rows
            .iter()
            .any(|row| row.label.trim() == "Missing")
    );
}

#[test]
fn mouse_click_opens_references_and_backlinks_and_inspection_needs_no_source_file() {
    let mut fixture = relation_fixture();
    let app = &mut fixture.app;
    wait_for_hits(app);
    select_id(app, "A");
    let source = app.current_detail_hit().unwrap().file_path.clone();
    app.detail_area = ratatui::layout::Rect::new(0, 0, 80, 60);
    let record_rows = app
        .record
        .rows(app.current_detail_hit().unwrap(), None)
        .len();
    let index = app
        .relation_rows
        .iter()
        .position(|row| row.label.trim() == "m1")
        .unwrap();
    mouse_event(
        app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 4,
            row: (record_rows + index) as u16,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert_eq!(app.current_detail_hit().unwrap().record_id, "m1");
    app.detail_area = ratatui::layout::Rect::new(0, 0, 80, 60);
    let record_rows = app
        .record
        .rows(app.current_detail_hit().unwrap(), None)
        .len();
    let index = app
        .relation_rows
        .iter()
        .position(|row| row.label.trim() == "A")
        .unwrap();
    mouse_event(
        app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 4,
            row: (record_rows + index) as u16,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert_eq!(app.current_detail_hit().unwrap().record_id, "A");
    std::fs::remove_file(fixture._temp.path().join(source)).unwrap();
    app.refresh_relations();
    assert!(app.relation_rows.iter().any(|row| row.label.trim() == "m1"));
}

#[test]
fn mouse_click_on_wrapped_value_selects_its_record_field() {
    let mut fixture = fixture(1);
    let app = &mut fixture.app;
    wait_for_hits(app);
    let title = "wrapped record value ".repeat(7);
    app.hits[0].payload = Some(objekte_search_core::repository::Payload::Bytes(
        format!(r#"{{"title":"{title}","number":0}}"#).into_bytes(),
    ));
    app.open_detail();
    let backend = ratatui::backend::TestBackend::new(80, 20);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|frame| view::draw(frame, app)).unwrap();
    let row = app
        .record
        .rows(app.current_detail_hit().unwrap(), None)
        .iter()
        .position(|row| row.label == "title")
        .unwrap();
    let line = app
        .record
        .visible_rows
        .iter()
        .rposition(|&visible| visible == row)
        .unwrap();
    mouse_event(
        app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: app.detail_area.x + 4,
            row: app.detail_area.y + line as u16,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert_eq!(app.record.cursor, row);
}
