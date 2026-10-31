use super::*;
use crate::tui::state::{Representation, SearchKind};
use objekte_core::Repository;
use std::{fs, time::Duration};

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
    assert!(app.kind == SearchKind::Text);
    press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert!(app.kind == SearchKind::Filter);
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
fn inference_prompt_queues_selected_or_all_collections_and_advanced_index_choice() {
    let temp = tempfile::tempdir().unwrap();
    for collection in ["docs", "notes"] {
        let folder = temp.path().join(format!("data/{collection}"));
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("a.json"), r#"{"note":"Moon"}"#).unwrap();
    }
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    press(&mut app, KeyCode::Tab, KeyModifiers::CONTROL);
    press(&mut app, KeyCode::Char('i'), KeyModifiers::CONTROL);
    assert!(app.focus == Focus::Infer);
    press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    let request = app.pending_inference.take().unwrap();
    assert_eq!(request.collection.as_deref(), Some("docs"));
    assert!(!request.auto_index);
    press(&mut app, KeyCode::Char('i'), KeyModifiers::CONTROL);
    press(&mut app, KeyCode::Char('c'), KeyModifiers::NONE);
    press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    let request = app.pending_inference.take().unwrap();
    assert!(request.collection.is_none());
    assert!(request.auto_index);
    press(&mut app, KeyCode::Char('i'), KeyModifiers::CONTROL);
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.pending_inference.is_none());
}

#[test]
fn result_fallback_cycles_collections_and_detail_still_opens() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(collection.join("a.json"), r#"{"note":"Moon"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.kind = SearchKind::Filter;
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
fn escape_steps_back_to_input_and_q_quits_from_every_view() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(collection.join("a.json"), r#"{"note":"Moon"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.input.text = "Moon".into();
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.focus == Focus::Results);
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.focus == Focus::Detail);

    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.focus == Focus::Results);
    assert!(!app.done);
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.focus == Focus::Query);
    assert!(!app.done);
    assert_eq!(app.input.text, "Moon");
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.focus == Focus::Query);
    assert!(!app.done);
    assert!(app.input.text.is_empty());
    assert_eq!(app.input.cursor, 0);
    assert!(app.hits.is_empty());
    assert!(app.last_request.is_none());
    assert!(app.search_elapsed_ms.is_none());

    for focus in [
        Focus::Query,
        Focus::Results,
        Focus::Detail,
        Focus::ExportAll,
        Focus::ExportOne,
    ] {
        app.focus = focus;
        app.done = false;
        press(&mut app, KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(app.done);
    }
}

#[test]
fn active_search_refreshes_when_collections_change() {
    let temp = tempfile::tempdir().unwrap();
    for collection in ["docs", "notes"] {
        let folder = temp.path().join(format!("data/{collection}"));
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("a.json"), r#"{"note":"Moon"}"#).unwrap();
    }
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.input.text = "Moon".into();
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(app.total, Some(2));

    press(&mut app, KeyCode::Tab, KeyModifiers::CONTROL);
    assert_eq!(app.collection_label(), "docs");
    assert_eq!(app.total, Some(1));
    assert_eq!(app.hits[0].record().collection, "docs");
    assert!(app.focus == Focus::Results);

    press(&mut app, KeyCode::Char(']'), KeyModifiers::NONE);
    assert_eq!(app.collection_label(), "notes");
    assert_eq!(app.total, Some(1));
    assert_eq!(app.hits[0].record().collection, "notes");

    press(&mut app, KeyCode::Char(']'), KeyModifiers::NONE);
    assert_eq!(app.collection_label(), "all collections");
    assert_eq!(app.total, Some(2));
}

#[test]
fn active_full_text_search_refreshes_when_options_change() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(collection.join(".editor")).unwrap();
    fs::write(
        collection.join(".editor/schema.json"),
        r#"{"type":"object","properties":{"note":{"type":"string","x-full-text":"markdown"}}}"#,
    )
    .unwrap();
    fs::write(collection.join("a.json"), r#"{"note":"**He**lp"}"#).unwrap();
    fs::write(collection.join("b.json"), r#"{"note":"HELP"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.input.text = "Help".into();
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(app.total, Some(2));

    press(&mut app, KeyCode::Char('e'), KeyModifiers::CONTROL);
    assert_eq!(app.total, Some(1));
    assert_eq!(app.hits[0].record().id, "a");
    press(&mut app, KeyCode::Char('e'), KeyModifiers::CONTROL);
    assert_eq!(app.total, Some(2));

    press(&mut app, KeyCode::Char('f'), KeyModifiers::CONTROL);
    assert_eq!(app.total, Some(1));
    assert_eq!(app.hits[0].record().id, "b");
    press(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL);
    assert_eq!(app.total, Some(0));
    assert!(app.hits.is_empty());
    press(&mut app, KeyCode::Char('f'), KeyModifiers::CONTROL);
    assert_eq!(app.total, Some(1));
    assert_eq!(app.hits[0].record().id, "a");
    press(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL);
    assert_eq!(app.total, Some(2));
    assert!(app.focus == Focus::Results);
}

#[test]
fn index_warmup_keeps_input_available_and_runs_the_queued_search() {
    let temp = tempfile::tempdir().unwrap();
    for (collection, value) in [("docs", "Moon"), ("notes", "Sun")] {
        let folder = temp.path().join(format!("data/{collection}"));
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("a.json"), format!(r#"{{"note":"{value}"}}"#)).unwrap();
    }
    let mut repo = Repository::open(temp.path()).unwrap();
    let updates = warm::start(&repo);
    let mut app = App::new(&mut repo);
    app.index_status = Some("Building indexes · 0/2 collections ready".into());
    app.input.text = "Moon".into();
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.queued_search);
    assert!(app.hits.is_empty());
    assert!(app.message.contains("queued"));

    press(&mut app, KeyCode::Tab, KeyModifiers::CONTROL);
    assert_eq!(app.collection_label(), "docs");
    assert!(app.queued_search);

    loop {
        let update = updates.recv_timeout(Duration::from_secs(5)).unwrap();
        let ready = matches!(update, warm::WarmUpdate::Ready(_));
        apply_warm_update(&mut app, update);
        if ready {
            break;
        }
    }
    assert!(app.index_status.is_none());
    assert!(!app.queued_search);
    assert_eq!(app.total, Some(1));
    assert_eq!(app.hits[0].record().collection, "docs");
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
    app.kind = SearchKind::Filter;
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
    app.kind = SearchKind::Filter;
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

#[test]
fn result_paging_stops_at_boundaries_without_stale_loading_message() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    for index in 0..101 {
        fs::write(
            collection.join(format!("item_{index:03}.json")),
            r#"{"note":"Moon"}"#,
        )
        .unwrap();
    }
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.input.text = "Moon".into();
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(app.total, Some(101));
    assert_eq!(app.hits.len(), 100);

    let previous = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE);
    let next = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE);
    assert!(pending_message(&app, previous).is_none());
    press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
    assert_eq!(app.offset, 0);
    assert_eq!(app.message, "Already on the first page.");

    assert_eq!(pending_message(&app, next), Some("Loading page…"));
    press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE);
    assert_eq!(app.offset, 100);
    assert_eq!(app.hits.len(), 1);
    assert!(pending_message(&app, next).is_none());
    assert!(pending_message(&app, KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE)).is_none());
    press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE);
    assert_eq!(app.offset, 100);
    assert_eq!(app.message, "Already on the last page.");

    assert_eq!(pending_message(&app, previous), Some("Loading page…"));
    press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
    assert_eq!(app.offset, 0);
    assert!(pending_message(&app, KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE)).is_none());
}
