use super::*;
use crate::tui::state::{Representation, SearchKind};
use objekte_core::Repository;
use std::fs;


fn press(app: &mut App<'_>, code: KeyCode, modifiers: KeyModifiers) {
    key(app, KeyEvent::new(code, modifiers));
}

#[test]
fn shortcuts_switch_modes_collections_and_available_text_options() {
    let temp = tempfile::tempdir().unwrap();
    for collection in ["docs", "notes"] {
        let folder = temp.path().join(format!("data/{collection}"));
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("a.json"), r#"{"note":"Moon"}"#).unwrap();
    }
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    press(&mut app, KeyCode::F(2), KeyModifiers::NONE);
    assert!(app.kind == SearchKind::Filter);
    press(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL);
    assert!(!app.regex);
    press(&mut app, KeyCode::Tab, KeyModifiers::CONTROL);
    assert_eq!(app.collection_label(), "docs");
    press(&mut app, KeyCode::Tab, KeyModifiers::CONTROL);
    assert_eq!(app.collection_label(), "notes");
    press(
        &mut app,
        KeyCode::BackTab,
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    assert_eq!(app.collection_label(), "docs");
    assert!(app.focus == Focus::Query);

    press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert!(app.kind == SearchKind::Text);
    press(&mut app, KeyCode::Char('f'), KeyModifiers::CONTROL);
    assert!(app.representation == Representation::Full);
    press(&mut app, KeyCode::Char('e'), KeyModifiers::CONTROL);
    assert!(app.case_sensitive);
    press(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL);
    assert!(app.regex);
    press(&mut app, KeyCode::Char('e'), KeyModifiers::CONTROL);
    assert!(app.case_sensitive);
    press(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT);
    assert!(app.kind == SearchKind::Filter);
}

#[test]
fn result_fallback_cycles_collections_and_detail_still_opens() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(collection.join("a.json"), r#"{"note":"Moon"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.input.text = "note ~ 'Moon'".into();
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(app.hits.len(), 1);
    assert!(app.focus == Focus::Results);
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.focus == Focus::Detail);
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.focus == Focus::Results);
    press(&mut app, KeyCode::Char(']'), KeyModifiers::NONE);
    assert_eq!(app.collection_label(), "docs");
    assert!(app.focus == Focus::Results);
    press(&mut app, KeyCode::Char('['), KeyModifiers::NONE);
    assert_eq!(app.collection_label(), "all collections");
    assert!(app.focus == Focus::Results);
}

#[test]
fn filter_input_accepts_shifted_letters_and_altgr_symbols() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(collection.join("a.json"), r#"{"note":"Moon"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.input.text = "note ".into();
    app.input.cursor = 5;
    press(
        &mut app,
        KeyCode::Char('~'),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    );
    for ch in " '".chars() {
        press(&mut app, KeyCode::Char(ch), KeyModifiers::NONE);
    }
    press(&mut app, KeyCode::Char('m'), KeyModifiers::SHIFT);
    for ch in "oon'".chars() {
        press(&mut app, KeyCode::Char(ch), KeyModifiers::NONE);
    }
    assert_eq!(app.input.text, "note ~ 'Moon'");
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(app.hits.len(), 1);
}

#[test]
fn detail_navigation_crosses_query_pages_in_both_directions() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    for index in 0..102 {
        fs::write(
            collection.join(format!("item_{index:03}.json")),
            format!(r#"{{"note":"Moon","number":{index}}}"#),
        )
        .unwrap();
    }
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.input.text = "note ~ 'Moon'".into();
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(app.hits.len(), 100);
    assert_eq!(app.total, Some(102));
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.focus == Focus::Detail);
    press(&mut app, KeyCode::Left, KeyModifiers::NONE);
    assert_eq!(app.selected, 0);
    app.detail_scroll = 8;
    for _ in 0..100 {
        press(&mut app, KeyCode::Right, KeyModifiers::NONE);
    }
    assert_eq!(app.offset, 100);
    assert_eq!(app.selected, 0);
    assert_eq!(app.hits[0].record().id, "item_100");
    assert_eq!(app.detail_scroll, 0);
    assert!(app.focus == Focus::Detail);
    press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE);
    assert_eq!(app.hits[app.selected].record().id, "item_101");
    press(&mut app, KeyCode::Right, KeyModifiers::NONE);
    assert_eq!(app.selected, 1);
    press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
    press(&mut app, KeyCode::Left, KeyModifiers::NONE);
    assert_eq!(app.offset, 0);
    assert_eq!(app.selected, 99);
    assert_eq!(app.hits[app.selected].record().id, "item_099");
    assert!(app.focus == Focus::Detail);
}
