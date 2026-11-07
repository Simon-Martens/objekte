use super::*;
use crate::tui::test_support::{fixture, relation_fixture};

#[test]
fn loading_screen_shows_index_progress_before_repository_is_ready() {
    let backend = ratatui::backend::TestBackend::new(80, 12);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| draw_loading(frame, std::time::Duration::from_secs(12)))
        .unwrap();
    let screen = format!("{:?}", terminal.backend().buffer());
    assert!(screen.contains("Building search index"));
    assert!(screen.contains("12.0 s"));
    assert!(screen.contains("Ctrl+Q quit"));
}

#[test]
fn draws_query_results_and_detail_on_small_terminal() {
    let mut fixture = fixture(2);
    let app = &mut fixture.app;
    for _ in 0..1000 {
        app.tick(std::time::Instant::now());
        if !app.searching {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let backend = ratatui::backend::TestBackend::new(90, 25);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, app)).unwrap();
    let screen = format!("{:?}", terminal.backend().buffer());
    assert!(screen.contains("objekte"));
    assert!(screen.contains("docs"));
    let elapsed = elapsed_label(app.last_search_duration.unwrap());
    assert!(screen.contains(&format!("search {elapsed}")));
    app.view = View::Detail;
    terminal.draw(|frame| draw(frame, app)).unwrap();
    let screen = format!("{:?}", terminal.backend().buffer());
    assert!(screen.contains("needle title") || screen.contains("ordinary title"));
    app.overlay = Some(Overlay::Export { all: true });
    terminal.draw(|frame| draw(frame, app)).unwrap();
    let screen = format!("{:?}", terminal.backend().buffer());
    assert!(screen.contains("Export all matches"));
}

#[test]
fn match_summary_uses_the_field_value_it_labels() {
    let mut fixture = fixture(2);
    fixture.app.search.text = "needle".into();
    fixture.app.request_search();
    for _ in 0..1000 {
        fixture.app.tick(std::time::Instant::now());
        if !fixture.app.searching {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let summary = record_summary(&fixture.app.hits[0]);
    assert!(summary.contains("needle title"));
    assert!(!summary.contains("number: needle"));
}

#[test]
fn detail_renders_structured_fields_and_relation_states() {
    let mut fixture = relation_fixture();
    let app = &mut fixture.app;
    for _ in 0..1_000 {
        app.tick(std::time::Instant::now());
        if !app.searching {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    app.selected = app
        .hits
        .iter()
        .position(|hit| hit.record_id == "B")
        .unwrap();
    app.open_detail();
    let backend = ratatui::backend::TestBackend::new(100, 40);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, app)).unwrap();
    let screen = format!("{:?}", terminal.backend().buffer());
    assert!(screen.contains("Dangling"));
    assert!(screen.contains("Outgoing"));
    assert!(screen.contains("dangling"));
    app.back_detail();
    app.selected = app
        .hits
        .iter()
        .position(|hit| hit.record_id == "m1")
        .unwrap();
    app.open_detail();
    terminal.draw(|frame| draw(frame, app)).unwrap();
    let screen = format!("{:?}", terminal.backend().buffer());
    assert!(screen.contains("Backlinks"));
    assert!(screen.contains("entry_mentions"));
}

#[test]
fn search_options_and_action_palette_are_visible() {
    let mut fixture = fixture(2);
    let app = &mut fixture.app;
    app.regex = true;
    app.case_sensitive = false;
    let backend = ratatui::backend::TestBackend::new(100, 28);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, app)).unwrap();
    let screen = format!("{:?}", terminal.backend().buffer());
    assert!(screen.contains("Alt+C"));
    assert!(screen.contains("Alt+R"));
    assert!(screen.contains("indexes ready"));
    app.overlay = Some(Overlay::Actions);
    terminal.draw(|frame| draw(frame, app)).unwrap();
    let screen = format!("{:?}", terminal.backend().buffer());
    assert!(screen.contains("Toggle regex search"));
    assert!(screen.contains("Show or hide columns"));
}

#[test]
fn long_search_line_keeps_the_cursor_and_tail_visible() {
    let mut fixture = fixture(1);
    let app = &mut fixture.app;
    app.search = Input::with_text(&format!("{}TAIL", "a".repeat(90)));
    let backend = ratatui::backend::TestBackend::new(72, 20);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, app)).unwrap();
    let screen = format!("{:?}", terminal.backend().buffer());
    assert!(screen.contains("TAIL"));
}
