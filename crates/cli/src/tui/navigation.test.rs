use super::*;
use crate::tui::{render, search::SearchWorker};
use objekte_core::Repository;
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
};
use std::{
    fs, thread,
    time::{Duration, Instant},
};

fn screen(terminal: &Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    let area = buffer.area;
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buffer.cell((x, y)).unwrap().symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn press(app: &mut App<'_>, code: KeyCode) {
    crate::tui::key(app, KeyEvent::new(code, KeyModifiers::NONE));
}

#[test]
fn detail_tabs_cycle_fields_links_and_backlinks() {
    let mut links = DetailLinks::default();
    assert!(links.section == DetailSection::Fields);
    links.cycle_section(false);
    assert!(links.section == DetailSection::Links);
    links.cycle_section(false);
    assert!(links.section == DetailSection::Backlinks);
    links.cycle_section(false);
    assert!(links.section == DetailSection::Fields);
    links.cycle_section(true);
    assert!(links.section == DetailSection::Backlinks);
    links.incoming = ["a", "b"]
        .into_iter()
        .map(|id| RecordRelation {
            source_collection: "books".into(),
            source_id: id.into(),
            location: "/person".into(),
            target_collection: "people".into(),
            target_id: "p".into(),
            target_state: objekte_core::TargetState::Unknown,
        })
        .collect();
    links.move_selection(true);
    assert_eq!(links.selected_relation().unwrap().source_id, "b");
    links.move_selection(true);
    assert_eq!(links.selected_relation().unwrap().source_id, "b");
    links.move_selection(false);
    assert_eq!(links.selected_relation().unwrap().source_id, "a");
}

#[test]
fn escape_returns_from_a_relation_tab_to_results() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(collection.join("a.json"), r#"{"note":"Moon"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.input.text = "Moon".into();
    app.search();
    app.open_detail();
    app.links.section = DetailSection::Backlinks;

    press(&mut app, KeyCode::Esc);

    assert!(app.focus == Focus::Results);
}

#[test]
fn backlinks_arrive_in_detail_and_both_table_directions_open_by_id() {
    let temp = tempfile::tempdir().unwrap();
    let books = temp.path().join("data/books");
    let people = temp.path().join("data/people");
    fs::create_dir_all(books.join(".editor")).unwrap();
    fs::create_dir_all(&people).unwrap();
    fs::write(
        books.join(".editor/schema.json"),
        r#"{"type":"object","properties":{"person":{"type":"string","x-relation":{"collection":"people"}}}}"#,
    )
    .unwrap();
    fs::write(
        books.join("b.json"),
        r#"{"title":"Moon Book","person":"p"}"#,
    )
    .unwrap();
    fs::write(people.join("p.json"), r#"{"name":"Alice"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.input.text = "Alice".into();
    app.search();
    app.open_detail();
    assert_eq!(
        app.current_detail_hit().unwrap().record().collection,
        "people"
    );
    assert!(matches!(app.links.status, BacklinkStatus::Loading));
    let mut terminal = Terminal::new(TestBackend::new(100, 25)).unwrap();
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Tab);
    assert!(app.links.section == DetailSection::Backlinks);
    terminal.draw(|frame| render::draw(frame, &app)).unwrap();
    assert!(screen(&terminal).contains("Searching for backlinks"));

    let mut worker = SearchWorker::default();
    let deadline = Instant::now() + Duration::from_secs(5);
    while matches!(app.links.status, BacklinkStatus::Loading) && Instant::now() < deadline {
        worker.drive(&mut app);
        thread::sleep(Duration::from_millis(1));
    }
    assert!(matches!(app.links.status, BacklinkStatus::Complete));
    assert_eq!(app.links.incoming.len(), 1);
    assert_eq!(app.links.incoming[0].source_id, "b");
    terminal.draw(|frame| render::draw(frame, &app)).unwrap();
    let backlinks = screen(&terminal);
    assert!(backlinks.lines().next().unwrap().contains("BACKLINKS (1)"));
    assert!(backlinks.contains("books"));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.current_detail_hit().unwrap().record().id, "b");
    assert_eq!(app.links.outgoing.len(), 1);
    app.start_export(false);
    let exported = temp.path().join("linked.json");
    app.export_input.text = exported.to_string_lossy().into_owned();
    app.finish_export();
    let value: serde_json::Value = serde_json::from_slice(&fs::read(exported).unwrap()).unwrap();
    assert_eq!(value["kind"], "record");
    assert!(value["query"].is_null());
    assert_eq!(value["hit"]["record"]["id"], "b");
    press(&mut app, KeyCode::Tab);
    assert!(app.links.section == DetailSection::Links);
    terminal.draw(|frame| render::draw(frame, &app)).unwrap();
    assert!(
        screen(&terminal)
            .lines()
            .next()
            .unwrap()
            .contains("LINKS (1)")
    );
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.current_detail_hit().unwrap().record().id, "p");
    press(&mut app, KeyCode::Backspace);
    assert_eq!(app.current_detail_hit().unwrap().record().id, "b");
    press(&mut app, KeyCode::Backspace);
    assert_eq!(app.current_detail_hit().unwrap().record().id, "p");
    press(&mut app, KeyCode::Esc);
    assert!(app.focus == Focus::Results);
}

#[test]
fn missing_link_target_reports_its_identity_without_leaving_detail() {
    let temp = tempfile::tempdir().unwrap();
    let books = temp.path().join("data/books");
    let people = temp.path().join("data/people");
    fs::create_dir_all(books.join(".editor")).unwrap();
    fs::create_dir_all(&people).unwrap();
    fs::write(
        books.join(".editor/schema.json"),
        r#"{"type":"object","properties":{"person":{"type":"string","x-relation":{"collection":"people"}}}}"#,
    )
    .unwrap();
    fs::write(books.join("b.json"), r#"{"title":"Moon","person":"gone"}"#).unwrap();
    fs::write(people.join("p.json"), r#"{"name":"Alice"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.input.text = "Moon".into();
    app.search();
    app.open_detail();
    app.links.section = DetailSection::Links;
    app.open_selected_relation();
    assert!(app.focus == Focus::Detail);
    assert_eq!(app.current_detail_hit().unwrap().record().id, "b");
    assert!(app.message.contains("Missing target: people/gone"));
}
