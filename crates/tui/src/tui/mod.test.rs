use super::test_support::{fixture, relation_fixture};
use super::*;

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
            row: (record_rows + index + 1) as u16,
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
            row: (record_rows + index + 1) as u16,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert_eq!(app.current_detail_hit().unwrap().record_id, "A");
    std::fs::remove_file(fixture._temp.path().join(source)).unwrap();
    app.refresh_relations();
    assert!(app.relation_rows.iter().any(|row| row.label.trim() == "m1"));
}
