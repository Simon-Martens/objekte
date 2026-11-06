use super::test_support::fixture;
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
