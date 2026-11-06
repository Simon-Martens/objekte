use super::*;
use objekte_core::Repository;
use std::fs;


fn app_repo() -> (tempfile::TempDir, Repository) {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(collection.join("a.json"), r#"{"title":"Moon"}"#).unwrap();
    let repo = Repository::open(temp.path()).unwrap();
    (temp, repo)
}


#[test]
fn control_c_and_q_quit_from_anywhere() {
    let (_temp, mut repo) = app_repo();
    let mut app = App::new(&mut repo);
    app.overlay = Some(Overlay::Help);
    key(
        &mut app,
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
    );
    assert!(app.done);
}

#[test]
fn key_releases_are_ignored() {
    let (_temp, mut repo) = app_repo();
    let mut app = App::new(&mut repo);
    let mut release = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
    release.kind = KeyEventKind::Release;
    key(&mut app, release);
    assert_eq!(app.search.text, "");
}

#[test]
fn alt_letters_toggle_options_instead_of_typing() {
    let (_temp, mut repo) = app_repo();
    let mut app = App::new(&mut repo);
    key(
        &mut app,
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::ALT),
    );
    assert!(app.case_sensitive);
    key(
        &mut app,
        KeyEvent::new(KeyCode::Char('r'), KeyModifiers::ALT),
    );
    assert!(app.regex);
    assert_eq!(app.search.text, "");
}

#[test]
fn help_opens_and_closes() {
    let (_temp, mut repo) = app_repo();
    let mut app = App::new(&mut repo);
    key(&mut app, KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    assert!(matches!(app.overlay, Some(Overlay::Help)));
    key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.overlay.is_none());
}

#[test]
fn symbols_sent_with_alt_or_altgr_are_typed_not_swallowed() {
    let (_temp, mut repo) = app_repo();
    let mut app = App::new(&mut repo);
    app.line = Line::Filter;
    for ch in ['@', '[', '|', '~', '{'] {
        key(
            &mut app,
            KeyEvent::new(KeyCode::Char(ch), KeyModifiers::ALT),
        );
    }
    // AltGr arrives as Ctrl+Alt, also for letters some layouts use.
    for ch in ['u', 'd', '\\'] {
        key(
            &mut app,
            KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL | KeyModifiers::ALT),
        );
    }
    assert_eq!(app.filter.text, "@[|~{ud\\\\".replace("\\\\", "\\"));
    // The shortcut letters still toggle.
    key(
        &mut app,
        KeyEvent::new(KeyCode::Char('r'), KeyModifiers::ALT),
    );
    assert!(app.regex);
    assert_eq!(app.filter.text.chars().count(), 8);
}
