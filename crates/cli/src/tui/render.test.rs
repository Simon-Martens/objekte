use super::*;
use objekte_core::Repository;
use ratatui::{Terminal, backend::TestBackend};
use std::fs;

#[test]
fn table_cells_never_exceed_twenty_five_terminal_columns() {
    assert_eq!(
        UnicodeWidthStr::width(clip("abcdefghijklmnopqrstuvwxyz", 25).as_str()),
        25
    );
    assert_eq!(
        UnicodeWidthStr::width(clip("🐈".repeat(20).as_str(), 25).as_str()),
        25
    );
    assert_eq!(clip("a\nb", 25), "a b");
    let headers = (0..10)
        .map(|index| format!("field_{index}"))
        .collect::<Vec<_>>();
    let shown = visible_columns(&headers, &[], 2, 40);
    assert_eq!(shown.first().unwrap().0, "field_2");
    assert!(shown.len() < headers.len());
}

#[test]
fn test_backend_renders_results_and_detail() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(
        collection.join("a.json"),
        r#"{"id":"business","note":"Hello"}"#,
    )
    .unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.input.text = "note ~ 'Hello'".into();
    app.search();
    assert!(columns(&app.hits).contains(&"id".into()));
    assert_eq!(field(&app.hits[0], "id"), "business");
    assert_eq!(field(&app.hits[0], "@id"), "a");
    let backend = TestBackend::new(80, 25);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    app.focus = Focus::Detail;
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer.cell((0, 7)).unwrap().symbol(), "c");
    for y in 7..23 {
        for x in 0..80 {
            assert!(
                !["┌", "┐", "└", "┘", "│", "─"].contains(&buffer.cell((x, y)).unwrap().symbol())
            );
        }
    }
}

#[test]
fn only_available_search_options_are_rendered() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(collection.join("a.json"), r#"{"note":"Hello"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    let labels = |app: &App<'_>| {
        controls(app)
            .lines
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.content.as_ref()))
            .collect::<String>()
    };
    let filter = labels(&app);
    assert!(filter.contains("Mode: Filter"));
    assert!(filter.contains("Collection:"));
    assert!(!filter.contains("Regex:"));
    assert!(!filter.contains("Text:"));
    assert!(!filter.contains("Case:"));

    app.kind = SearchKind::Text;
    let word = labels(&app);
    assert!(word.contains("Regex:"));
    assert!(word.contains("Text: normalized"));
    assert!(word.contains("Case: insensitive"));
    app.regex = true;
    assert!(!labels(&app).contains("Case:"));
}

#[test]
fn full_text_detail_colors_matched_field_names() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(
        collection.join("a.json"),
        r#"{"note":"Moon","other":"Sun"}"#,
    )
    .unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.kind = SearchKind::Text;
    app.input.text = "Moon".into();
    app.search();
    assert_eq!(app.hits[0].matched_paths(), ["note"]);
    app.focus = Focus::Detail;
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    let buffer = terminal.backend().buffer();
    assert!((7..28).any(|y| {
        buffer
            .cell((0, y))
            .is_some_and(|cell| cell.symbol() == "n" && cell.fg == Color::Yellow)
    }));
}
