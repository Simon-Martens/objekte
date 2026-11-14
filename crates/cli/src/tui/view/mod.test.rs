use super::*;
use crate::tui::test_support::{fixture, relation_fixture};

#[test]
fn discovered_file_catalogue_shows_physical_metadata_without_record_claims() {
    let mut fixture = fixture(2);
    let app = &mut fixture.app;
    app.toggle_files();
    assert_eq!(app.file_count(), 3);
    let backend = ratatui::backend::TestBackend::new(100, 15);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, app)).unwrap();
    let screen = format!("{:?}", terminal.backend().buffer());
    assert!(screen.contains("3 files in catalogue"));
    assert!(screen.contains("docs/0000.json"));
    assert!(screen.contains("mtime (Unix s)"));
    assert!(screen.contains("record IDs and validity are known after indexing"));
    assert!(!screen.contains("Valid"));
    app.move_file_selection(2);
    assert_eq!(app.file_selected, 2);
    app.toggle_files();
    assert_eq!(app.view, View::Results);
}

#[test]
fn cli_result_table_has_only_fixed_columns_until_query_adds_fields() {
    let mut fixture = fixture(2);
    let app = &mut fixture.app;
    app.single_collection = true;
    app.scope = 1;
    app.request_search();
    for _ in 0..1000 {
        app.tick(std::time::Instant::now());
        if !app.searching {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    for width in [60, 120] {
        let backend = ratatui::backend::TestBackend::new(width, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut header = String::new();
        for x in 0..app.results_area.width {
            header.push_str(buffer[(x, app.results_area.y)].symbol());
        }
        assert!(header.contains("id"));
        assert!(header.contains("match"));
        assert!(!header.contains("collection"));
        assert!(!header.contains("title"));
        app.open_detail();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let detail = format!("{:?}", terminal.backend().buffer());
        assert!(!detail.contains("Backlinks"));
        assert!(!detail.contains("2 Links"));
        app.back_detail();
    }
    app.filter = Input::with_text("number >= 0 SORT BY title");
    app.request_search();
    for _ in 0..1000 {
        app.tick(std::time::Instant::now());
        if !app.searching {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let backend = ratatui::backend::TestBackend::new(120, 20);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, app)).unwrap();
    let screen = format!("{:?}", terminal.backend().buffer());
    assert!(screen.contains("number"));
    assert!(screen.contains("title"));

    record::take_decode_count();
    for _ in 0..5 {
        terminal.draw(|frame| draw(frame, app)).unwrap();
    }
    assert_eq!(
        record::take_decode_count(),
        0,
        "redraw decoded result bytes"
    );
}

#[test]
fn loading_screen_shows_index_progress_before_repository_is_ready() {
    let backend = ratatui::backend::TestBackend::new(80, 12);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| draw_loading(frame, std::time::Duration::from_secs(12)))
        .unwrap();
    let screen = format!("{:?}", terminal.backend().buffer());
    assert!(screen.contains("Building search index"));
    assert!(screen.contains("12000.000 ms"));
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
    assert!(screen.contains(&format!("all {elapsed}")));
    app.view = View::Detail;
    terminal.draw(|frame| draw(frame, app)).unwrap();
    let screen = format!("{:?}", terminal.backend().buffer());
    assert!(screen.contains("needle title") || screen.contains("ordinary title"));
    assert!(screen.contains("1 Fields"));
    assert!(screen.contains("2 Links"));
    assert!(screen.contains("3 Backlinks"));
    assert!(screen.contains("Metadata"));
    let selected = &terminal.backend().buffer()[(1, 5)];
    assert_eq!(selected.symbol(), "▌");
    assert_eq!(selected.bg, SELECTED);
    app.overlay = Some(Overlay::Export { all: true });
    terminal.draw(|frame| draw(frame, app)).unwrap();
    let screen = format!("{:?}", terminal.backend().buffer());
    assert!(screen.contains("Export all matches"));
}

#[test]
fn long_record_values_wrap_and_keep_screen_lines_on_the_same_field() {
    let mut fixture = fixture(1);
    let app = &mut fixture.app;
    for _ in 0..1000 {
        app.tick(std::time::Instant::now());
        if !app.searching {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let title = "A long title with several words ".repeat(5);
    app.hits[0].payload = Some(objekte_search_core::repository::Payload::Bytes(
        format!(r#"{{"title":"{title}","number":0}}"#).into_bytes(),
    ));
    app.open_detail();
    let backend = ratatui::backend::TestBackend::new(80, 20);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, app)).unwrap();
    let hit = app.current_detail_hit().unwrap();
    let title_row = app
        .record
        .rows(hit, None)
        .iter()
        .position(|row| row.label == "title")
        .unwrap();
    assert!(
        app.record
            .visible_rows
            .iter()
            .filter(|&&row| row == title_row)
            .count()
            > 1
    );
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
fn match_column_stays_after_ids_when_result_fields_scroll() {
    let mut fixture = fixture(2);
    let app = &mut fixture.app;
    app.search = Input::with_text("needle");
    app.request_search();
    for _ in 0..1000 {
        app.tick(std::time::Instant::now());
        if !app.searching {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    app.column_offset = 1;
    let backend = ratatui::backend::TestBackend::new(110, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| results(frame, app, Rect::new(0, 0, 110, 5)))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let mut header = String::new();
    for x in 0..110 {
        header.push_str(buffer[(x, 0)].symbol());
    }
    let collection = header.find("collection").unwrap();
    let id = header.find("id").unwrap();
    let matched = header.find("match / record").unwrap();
    let number = header.find("number").unwrap();
    assert!(collection < id && id < matched && matched < number);
    assert!(!header.contains("title"));
    assert_eq!(buffer[(matched as u16, 1)].symbol(), "t");
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

#[test]
fn storage_overlay_keeps_sync_action_reachable_with_a_long_managed_path() {
    use objekte_search_core::index_storage::{IndexResidency, IndexStorageLocation, IndexStoragePolicy};
    let mut fixture = fixture(1);
    let app = &mut fixture.app;
    app.storage_policy = IndexStoragePolicy {
        location: IndexStorageLocation::RepositoryDirectory,
        residency: IndexResidency::OnDemand,
    };
    app.storage_view = Some(crate::storage_view::StorageStatusView {
        location: "Repository folder".into(),
        residency: "On-demand".into(),
        readiness: "Searchable".into(),
        reconciliation: "Source files current".into(),
        synchronization: "Changes pending".into(),
        generations: "Generation 4 (durable 3)".into(),
        validation: "0 fully validated, 2 trusted stamps, 0 stamps written".into(),
        managed_directory: Some(format!("/tmp/{}", "long/".repeat(24))),
        issue: None,
        validation_warning: None,
    });
    app.storage_sync_available = true;
    app.overlay = Some(Overlay::Storage);
    app.storage_picker = 7;
    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, app)).unwrap();
    let screen = format!("{:?}", terminal.backend().buffer());
    assert!(screen.contains("Synchronize now / Retry synchronization"), "{screen}");
}
