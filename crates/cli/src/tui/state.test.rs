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
    assert!(app.kind == SearchKind::Text);
    assert!(app.representation == Representation::Normalized);
    assert!(!app.regex);
    assert!(!app.case_sensitive);
    app.input.text = "Help".into();
    for (regex, representation, expected) in [
        (false, Representation::Normalized, TextMode::Word),
        (false, Representation::Full, TextMode::WordRaw),
        (true, Representation::Normalized, TextMode::RegexClean),
        (true, Representation::Full, TextMode::RegexRaw),
    ] {
        app.regex = regex;
        app.representation = representation;
        let Request::Text(request) = app.request().unwrap() else {
            panic!("expected text request")
        };
        assert_eq!(request.mode, expected);
        assert_eq!(request.case_sensitive, regex);
    }
    app.collection = 1;
    let Request::Text(request) = app.request().unwrap() else {
        panic!("expected text request")
    };
    assert_eq!(request.collections, Some(vec!["docs".into()]));
    app.input.text = "Help SORT BY note DESC".into();
    let Request::Text(request) = app.request().unwrap() else {
        panic!("expected text request")
    };
    assert_eq!(request.pattern, "Help");
    assert_eq!(request.sort[0].field, "note");
    assert_eq!(request.sort[0].direction, objekte_core::Direction::Desc);
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
fn direct_controls_invalidate_results_with_filter_on() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(collection.join("a.json"), r#"{"note":"Moon"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.kind = SearchKind::FilterOn;
    app.filter_input.text = "note ~ 'Moon'".into();
    app.search();
    assert_eq!(app.hits.len(), 1);
    assert!(app.last_request.is_some());
    assert!(app.focus == Focus::Results);

    app.toggle_regex();
    app.toggle_representation();
    app.toggle_case();
    assert!(app.regex);
    assert!(app.representation == Representation::Full);
    assert!(!app.case_sensitive);
    assert_eq!(app.hits.len(), 1);

    app.cycle_collection(false);
    assert_eq!(app.collection_label(), "docs");
    assert!(app.focus == Focus::Results);
    assert_eq!(app.hits.len(), 1);
    assert!(app.last_request.is_some());

    app.toggle_mode();
    assert!(app.kind == SearchKind::Text);
    assert!(app.focus == Focus::Query);
    app.toggle_regex();
    assert!(!app.regex);
    app.toggle_case();
    assert!(app.case_sensitive);
}

#[test]
fn filter_on_keeps_both_inputs_and_filters_text_results() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(collection.join("a.json"), r#"{"year":1800,"note":"Moon"}"#).unwrap();
    fs::write(collection.join("b.json"), r#"{"year":1900,"note":"Moon"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.input.text = "Moon".into();
    app.toggle_mode();
    app.filter_input.text = "year = 1800 SORT BY note DESC".into();
    assert!(app.kind == SearchKind::FilterOn);
    assert_eq!(app.input.text, "Moon");
    assert_eq!(app.filter_input.text, "year = 1800 SORT BY note DESC");
    let Request::Text(request) = app.request().unwrap() else {
        panic!("expected text request")
    };
    assert_eq!(request.filter.as_deref(), Some("year = 1800"));
    assert_eq!(request.sort[0].field, "note");
    app.search();
    assert_eq!(app.total, Some(1));
    assert_eq!(app.hits[0].record().id, "a");
    assert_eq!(app.query_fields, ["year"]);
    assert_eq!(app.sort_fields, ["note"]);
    app.filter_input.text = "year =".into();
    app.search();
    assert!(app.focus == Focus::FilterInput);
    assert!(!app.message.is_empty());
}

#[test]
fn filter_on_accepts_either_input_and_fts_sort_wins() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(
        collection.join("a.json"),
        r#"{"year":1800,"note":"Moon","rank":2}"#,
    )
    .unwrap();
    fs::write(
        collection.join("b.json"),
        r#"{"year":1900,"note":"Sun","rank":3}"#,
    )
    .unwrap();
    fs::write(
        collection.join("c.json"),
        r#"{"year":2000,"note":"Moon","rank":1}"#,
    )
    .unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.toggle_mode();

    app.input.text = "Moon".into();
    let Request::Text(fts) = app.request().unwrap() else {
        panic!("expected FTS")
    };
    assert!(fts.filter.is_none());
    app.search();
    assert_eq!(app.total, Some(2));

    app.input.text.clear();
    app.filter_input.text = "year >= 1800 SORT BY year ASC".into();
    let Request::Filter(filter) = app.request().unwrap() else {
        panic!("expected filter")
    };
    assert_eq!(filter.sort[0].field, "year");
    app.search();
    assert_eq!(app.total, Some(3));
    assert_eq!(app.hits[0].record().id, "a");
    assert!(matches!(app.hits[0], Hit::Filter(_)));

    app.input.text = "Moon SORT BY rank ASC".into();
    let Request::Text(both) = app.request().unwrap() else {
        panic!("expected combined FTS")
    };
    assert_eq!(both.filter.as_deref(), Some("year >= 1800"));
    assert_eq!(both.sort.len(), 1);
    assert_eq!(both.sort[0].field, "rank");
    app.search();
    assert_eq!(app.total, Some(2));
    assert_eq!(app.hits[0].record().id, "c");
    assert_eq!(app.hits[1].record().id, "a");
    assert_eq!(app.sort_fields, ["rank"]);

    app.input.text = "SORT BY rank ASC".into();
    let Request::Filter(filter) = app.request().unwrap() else {
        panic!("expected filter")
    };
    assert_eq!(filter.sort[0].field, "rank");
    app.search();
    assert_eq!(app.hits[0].record().id, "c");

    app.input.text = "Moon".into();
    app.toggle_mode();
    let Request::Text(fts) = app.request().unwrap() else {
        panic!("expected FTS")
    };
    assert!(fts.filter.is_none());
    app.toggle_mode();
    assert_eq!(app.filter_input.text, "year >= 1800 SORT BY year ASC");

    app.input.text.clear();
    app.filter_input.text.clear();
    assert!(app.request().is_err());
}
