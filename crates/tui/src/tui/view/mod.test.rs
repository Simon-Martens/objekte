use super::*;
use crate::tui::test_support::fixture;

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
