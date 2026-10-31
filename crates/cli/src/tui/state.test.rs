use super::*;
use ratatui::crossterm::event::KeyModifiers;
use std::fs;


#[test]
fn input_edits_unicode_at_character_boundaries() {
    let mut input = Input::default();
    for ch in ['A', 'ä', '🐈'] {
        input.key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    input.key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    input.key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    assert_eq!(input.text, "A🐈");
    input.key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
    input.key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
    assert_eq!(input.text, "🐈");
    assert_eq!(input.cursor, 0);
}

#[test]
fn input_keeps_shifted_and_altgr_characters() {
    let mut input = Input::default();
    input.key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::SHIFT));
    input.key(KeyEvent::new(
        KeyCode::Char('~'),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    ));
    input.key(KeyEvent::new(KeyCode::Char('Ä'), KeyModifiers::SHIFT));
    assert_eq!(input.text, "A~Ä");
}

#[test]
fn full_text_controls_choose_all_four_search_modes() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(collection.join("a.json"), r#"{"note":"**Help**"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.kind = SearchKind::Text;
    app.input.text = "Help".into();
    for (regex, representation, expected) in [
        (false, Representation::Normalized, TextMode::Word),
        (false, Representation::Full, TextMode::WordRaw),
        (true, Representation::Normalized, TextMode::RegexClean),
        (true, Representation::Full, TextMode::RegexRaw),
    ] {
        app.regex = regex;
        app.representation = representation;
        let Request::Text(request) = app.request() else {
            panic!("expected text request")
        };
        assert_eq!(request.mode, expected);
        assert_eq!(request.case_sensitive, regex);
    }
    app.collection = 1;
    let Request::Text(request) = app.request() else {
        panic!("expected text request")
    };
    assert_eq!(request.collections, Some(vec!["docs".into()]));
}

#[test]
fn full_text_input_finds_partial_words_in_the_result_table() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(collection.join("a.json"), r#"{"name":"Lichtenberg"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.kind = SearchKind::Text;
    app.input.text = "Lichtenber".into();
    app.search();
    assert_eq!(app.total, Some(1));
    assert_eq!(app.hits.len(), 1);
    assert_eq!(app.hits[0].matched_paths(), &["name".to_owned()]);
}

#[test]
fn direct_controls_invalidate_results_and_respect_search_mode() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(collection.join("a.json"), r#"{"note":"Moon"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.input.text = "note ~ 'Moon'".into();
    app.search();
    assert_eq!(app.hits.len(), 1);
    assert!(app.last_request.is_some());
    assert!(app.focus == Focus::Results);

    app.toggle_regex();
    app.toggle_representation();
    app.toggle_case();
    assert!(!app.regex);
    assert!(app.representation == Representation::Normalized);
    assert!(!app.case_sensitive);
    assert_eq!(app.hits.len(), 1);

    app.cycle_collection(false);
    assert_eq!(app.collection_label(), "docs");
    assert!(app.focus == Focus::Results);
    assert!(app.hits.is_empty());
    assert!(app.last_request.is_none());

    app.toggle_mode();
    assert!(app.kind == SearchKind::Text);
    assert!(app.focus == Focus::Query);
    app.toggle_regex();
    assert!(app.regex);
    app.toggle_case();
    assert!(!app.case_sensitive);
}
