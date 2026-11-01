use super::table::{field, visible_columns};
use super::*;
use objekte_core::{InferOptions, RecordRelation, Repository, TargetState};
use ratatui::{Terminal, backend::TestBackend};
use std::fs;
use unicode_width::UnicodeWidthStr;

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

#[test]
fn filter_on_renders_two_editable_queries() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("a.json"), r#"{"text":"alpha"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.kind = SearchKind::FilterOn;
    app.input.text = "alpha".into();
    app.filter_input.text = "year >= 1800".into();
    app.focus = Focus::FilterInput;
    let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    let rendered = screen(&terminal);
    assert!(rendered.contains("JSON FILTER"));
    assert!(rendered.contains("year >= 1800"));
    assert!(rendered.contains("FULL-TEXT SEARCH"));
    assert!(rendered.contains("alpha"));
    assert!(rendered.contains("Ctrl+G input"));
}

#[test]
fn cache_shortcut_hints_are_visible_in_query_and_results() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("a.json"), r#"{"text":"alpha"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    let mut terminal = Terminal::new(TestBackend::new(160, 25)).unwrap();
    for focus in [Focus::Query, Focus::Results] {
        app.focus = focus;
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let rendered = screen(&terminal);
        assert!(rendered.contains("Alt+S"));
        assert!(rendered.contains("Alt+R"));
    }
}

#[test]
fn advanced_index_prompt_and_post_scan_schema_action_are_visible() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(collection.join("a.json"), r#"{"active":true}"#).unwrap();
    let schema = temp.path().join(".editor/collections/docs/schema.json");
    fs::create_dir_all(schema.parent().unwrap()).unwrap();
    fs::write(schema, r#"{"type":"object","properties":{}}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let report = repo
        .infer_and_auto_index(Some("docs"), InferOptions::default())
        .unwrap();
    let mut app = App::new(&mut repo);
    let mut terminal = Terminal::new(TestBackend::new(80, 25)).unwrap();
    app.focus = Focus::Infer;
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    let before = screen(&terminal);
    assert!(before.contains("A  Advanced indexing: on"));
    app.last_inference = Some(report);
    app.focus = Focus::InferReady;
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    let after = screen(&terminal);
    assert!(after.contains("W  Write types and index rules"));
}

#[test]
fn default_fts_layout_and_result_styles_fit_standard_terminals() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(collection.join("a.json"), r#"{"note":"Moon"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    let mut terminal = Terminal::new(TestBackend::new(80, 25)).unwrap();
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    let initial = screen(&terminal);
    assert!(initial.lines().next().unwrap().contains("FTS"));
    assert!(initial.lines().next().unwrap().ends_with("all collections"));
    assert!(initial.lines().nth(1).unwrap().trim().is_empty());
    assert!(
        initial
            .lines()
            .nth(2)
            .unwrap()
            .contains("Case: insensitive  Ctrl+E")
    );
    assert!(initial.contains("Search the whole collection"));
    assert!(initial.lines().last().unwrap().contains("Alt+S"));
    assert!(initial.lines().last().unwrap().contains("Alt+R"));
    app.index_status = Some("Building indexes · 0/1 collections ready".into());
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    let warming = screen(&terminal);
    assert!(warming.lines().last().unwrap().contains("Building indexes"));
    assert!(!warming.lines().last().unwrap().contains("n/p pages"));
    app.index_status = None;
    assert_eq!(
        terminal.backend().buffer().cell((0, 3)).unwrap().fg,
        theme::ACCENT
    );
    app.input.text = "absent".into();
    app.search();
    assert!(app.search_elapsed_ms.is_some());
    app.search_elapsed_ms = Some(3.25);
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    let empty = screen(&terminal);
    assert!(empty.contains("No matches · 3.25 ms"));
    assert!(empty.lines().last().unwrap().contains("Alt+S"));

    app.input.text = "Moon".into();
    app.search();
    assert!(app.search_elapsed_ms.is_some());
    app.search_elapsed_ms = Some(12.5);
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    let found = screen(&terminal);
    assert!(found.contains("RESULTS  ·  1–1 of 1"));
    assert!(found.contains("12.50 ms"));
    assert!(!found.contains("matches · showing"));
    assert!(found.contains("collection  id       status"));
    assert!(found.contains("match"));
    assert!(found.contains("note: Moon"));
    assert_eq!(
        terminal.backend().buffer().cell((3, 8)).unwrap().bg,
        theme::SELECTED
    );
    let footer = found.lines().last().unwrap();
    for label in [
        "↑↓ rows",
        "←→ cols",
        "n/p pages",
        "Esc input",
        "Enter open",
        "a all",
        "e hit",
        "Ctrl+Q quit",
    ] {
        assert!(footer.contains(label), "missing {label} in {footer}");
    }

    app.focus = Focus::Query;
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    let query_footer = screen(&terminal).lines().last().unwrap().to_owned();
    assert!(query_footer.contains("Enter search"));
    assert!(query_footer.contains("Ctrl+Q quit"));
    app.focus = Focus::Results;

    let mut narrow = Terminal::new(TestBackend::new(60, 20)).unwrap();
    narrow.draw(|frame| draw(frame, &app)).unwrap();
    let compact = screen(&narrow);
    assert!(compact.lines().next().unwrap().ends_with("all collections"));
    assert!(compact.lines().nth(2).unwrap().contains("ignore case ^E"));
    assert!(compact.lines().last().unwrap().contains("Ctrl+Q quit"));

    let mut tiny = Terminal::new(TestBackend::new(40, 15)).unwrap();
    tiny.draw(|frame| draw(frame, &app)).unwrap();
    let smallest = screen(&tiny);
    assert!(smallest.lines().next().unwrap().ends_with('…'));
    assert!(smallest.lines().nth(2).unwrap().contains("E:fold"));
    assert!(smallest.lines().last().unwrap().contains("Ctrl+Q"));

    app.focus = Focus::Detail;
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    let detail_footer = screen(&terminal).lines().last().unwrap().to_owned();
    for label in [
        "←→/p/n result",
        "↑↓ scroll",
        "PgUp/Dn",
        "e export",
        "Esc back",
        "Ctrl+Q quit",
    ] {
        assert!(
            detail_footer.contains(label),
            "missing {label} in {detail_footer}"
        );
    }
}

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
    assert_eq!(clip("text", 0), "");
    let headers = (0..10)
        .map(|index| format!("field_{index}"))
        .collect::<Vec<_>>();
    let shown = visible_columns(&headers, &[], 2, 40);
    assert_eq!(shown.first().unwrap().0, "field_2");
    assert!(shown.len() < headers.len());
}

#[test]
fn filter_table_puts_queried_fields_first_in_query_order() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(
        collection.join("a.json"),
        r#"{"alpha":"A","beta":true,"gamma":"G","nested":{"name":"X"},"zeta":1}"#,
    )
    .unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.kind = SearchKind::FilterOn;
    app.filter_input.text =
        "zeta = 1 && missing(absent) && beta = true && nested.name = 'X' && zeta > 0 SORT BY gamma DESC && zeta ASC".into();
    app.search();
    assert_eq!(app.hits.len(), 1);
    assert_eq!(
        columns(&app.hits, &app.query_fields, &app.sort_fields),
        [
            "@collection",
            "@id",
            "@status",
            "zeta",
            "absent",
            "beta",
            "nested",
            "gamma",
            "alpha",
        ]
    );
    assert_eq!(app.sort_fields, ["gamma", "zeta"]);
    assert_eq!(field(&app.hits[0], "absent"), "·");

    app.filter_input.text = "beta = true && zeta = 1".into();
    app.search();
    assert_eq!(
        &columns(&app.hits, &app.query_fields, &app.sort_fields)[3..5],
        ["beta", "zeta"]
    );
}

#[test]
fn full_text_table_previews_the_exact_matched_leaf_before_other_fields() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(
        collection.join("a.json"),
        r#"{"notes":[{"body":"Sun"},{"body":"Moonlight over the mountain"}],"other":"Venus"}"#,
    )
    .unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.input.text = "Moonlight".into();
    app.search();
    assert_eq!(app.hits[0].matched_paths(), ["notes[1].body"]);
    assert_eq!(
        &columns(&app.hits, &app.query_fields, &app.sort_fields)[..4],
        ["@collection", "@id", "@status", "@match"]
    );
    assert_eq!(
        field(&app.hits[0], "@match"),
        "notes[1].body: Moonlight over the mountain"
    );
    let mut terminal = Terminal::new(TestBackend::new(150, 20)).unwrap();
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    let rendered = screen(&terminal);
    assert!(rendered.contains("RESULTS  ·  1–1 of 1"));
    assert!(rendered.contains("notes[1].body: Moonlight over the mountain"));
    assert!(rendered.lines().nth(6).unwrap().contains("RESULTS"));
    assert!(rendered.lines().nth(7).unwrap().contains("match"));

    let mut standard = Terminal::new(TestBackend::new(80, 20)).unwrap();
    standard.draw(|frame| draw(frame, &app)).unwrap();
    let standard_screen = screen(&standard);
    assert!(standard_screen.lines().nth(7).unwrap().contains("match"));
    assert!(standard_screen.contains("notes[1].body: Moonlight"));

    app.kind = SearchKind::FilterOn;
    app.input.text.clear();
    app.filter_input.text = "other = 'Venus'".into();
    app.search();
    assert_eq!(
        &columns(&app.hits, &app.query_fields, &app.sort_fields)[..3],
        ["@collection", "@id", "@status"]
    );
    assert!(!columns(&app.hits, &app.query_fields, &app.sort_fields).contains(&"@match".into()));
}

#[test]
fn partial_streamed_page_shows_rows_without_claiming_a_final_total() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(collection.join("a.json"), r#"{"note":"Moon"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.input.text = "Moon".into();
    app.search();
    app.searching = true;
    app.total = None;
    let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    let partial = screen(&terminal);
    assert!(partial.contains("RESULTS  ·  1–1 · searching…"));
    assert!(partial.contains("note: Moon"));
    assert!(!partial.contains("1–1 of 1"));

    app.hits.clear();
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    let waiting = screen(&terminal);
    assert!(waiting.contains("Searching…"));
    assert!(!waiting.contains("No matches"));
}

#[test]
fn backlink_table_keeps_selected_row_visible_without_rendering_every_edge() {
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
    app.links.status = BacklinkStatus::Complete;
    app.links.incoming = (0..30)
        .map(|number| RecordRelation {
            source_collection: "books".into(),
            source_id: format!("row{number:02}"),
            location: "/person".into(),
            target_collection: "docs".into(),
            target_id: "a".into(),
            target_state: TargetState::Unknown,
        })
        .collect();
    app.links.incoming_selected = 29;
    let mut terminal = Terminal::new(TestBackend::new(80, 15)).unwrap();
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    let rendered = screen(&terminal);
    assert!(rendered.lines().next().unwrap().contains("BACKLINKS (30)"));
    assert!(rendered.contains("row29"));
    assert!(!rendered.contains("row00"));
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer.cell((0, 2)).unwrap().symbol(), "╭");
    assert_eq!(buffer.cell((79, 13)).unwrap().symbol(), "╯");
}

#[test]
fn full_text_match_preview_is_capped_at_sixty_five_cells_and_supports_malformed_json() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(collection.join("a.json"), "{\"note\":\"Moon".repeat(30)).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.input.text = "Moon".into();
    app.search();
    assert_eq!(app.hits[0].matched_paths(), ["$raw"]);
    let preview = field(&app.hits[0], "@match");
    assert!(preview.starts_with("$raw: {\"note\":\"Moon"));
    assert!(preview.ends_with('…'));
    assert_eq!(UnicodeWidthStr::width(preview.as_str()), 65);
    assert_eq!(
        visible_columns(
            &columns(&app.hits, &app.query_fields, &app.sort_fields),
            &app.hits,
            3,
            120
        )[0]
        .1,
        65
    );
    let mut standard = Terminal::new(TestBackend::new(80, 20)).unwrap();
    standard.draw(|frame| draw(frame, &app)).unwrap();
    let rendered = screen(&standard);
    assert!(rendered.lines().nth(7).unwrap().contains("match"));
    assert!(rendered.contains("$raw: {\"note\":\"Moon"));
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
    app.kind = SearchKind::FilterOn;
    app.filter_input.text = "note ~ 'Hello'".into();
    app.search();
    assert!(columns(&app.hits, &app.query_fields, &app.sort_fields).contains(&"id".into()));
    assert_eq!(field(&app.hits[0], "id"), "business");
    assert_eq!(field(&app.hits[0], "@id"), "a");
    let backend = TestBackend::new(80, 25);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    app.focus = Focus::Detail;
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer.cell((0, 2)).unwrap().symbol(), "╭");
    assert_eq!(buffer.cell((79, 2)).unwrap().symbol(), "╮");
    assert_eq!(buffer.cell((0, 23)).unwrap().symbol(), "╰");
    assert_eq!(buffer.cell((79, 23)).unwrap().symbol(), "╯");
    assert_eq!(buffer.cell((1, 3)).unwrap().symbol(), "c");
    for y in 3..23 {
        assert_eq!(buffer.cell((0, y)).unwrap().symbol(), "│");
        assert_eq!(buffer.cell((79, y)).unwrap().symbol(), "│");
    }
}

#[test]
fn detail_has_its_own_context_and_relation_tabs_without_search_controls() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(collection.join("a.json"), r#"{"note":"Moon"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.input.text = "Moon".into();
    app.search();
    app.open_detail();
    let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    let rendered = screen(&terminal);
    let lines = rendered.lines().collect::<Vec<_>>();
    assert!(lines[0].contains("◆ OBJEKTE"));
    assert!(lines[0].contains("FIELDS"));
    assert!(lines[0].contains("LINKS (0)"));
    assert!(lines[0].contains("BACKLINKS (0)"));
    assert!(lines[0].contains("[ESC] BACK"));
    assert!(lines[1].trim().is_empty());
    assert!(lines[2].contains("DOCS · Search: Moon · Record 1 of 1"));
    assert!(!rendered.contains("FULL-TEXT SEARCH"));
    assert!(!rendered.contains("Regex:"));
    assert!(!rendered.contains("Ctrl+Tab"));

    app.links.section = DetailSection::Backlinks;
    app.links.status = BacklinkStatus::Loading;
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    assert!(
        screen(&terminal)
            .lines()
            .next()
            .unwrap()
            .contains("BACKLINKS (0+)")
    );

    app.input.text = "Moon".repeat(30);
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    let long_title = screen(&terminal).lines().nth(2).unwrap().to_owned();
    assert!(long_title.contains("DOCS · Search: Moon"));
    assert!(long_title.contains("Record 1 of 1"));

    app.links.override_hit = Some(app.hits[0].clone());
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    let linked = screen(&terminal);
    assert!(linked.contains("Linked · Record 1 of 1"));
    assert!(linked.contains("DOCS/a · Search: Moon"));

    app.links.override_hit = None;
    let mut narrow = Terminal::new(TestBackend::new(40, 12)).unwrap();
    narrow.draw(|frame| draw(frame, &app)).unwrap();
    let narrow_screen = screen(&narrow);
    let narrow_lines = narrow_screen.lines().collect::<Vec<_>>();
    assert!(narrow_lines[0].contains("◆ OBJEKTE"));
    assert!(narrow_lines[0].contains("BACKLINKS"));
    assert!(narrow_lines[0].contains("[ESC]"));
    assert!(narrow_lines[1].trim().is_empty());
    assert!(narrow_lines[2].contains("Search: Moon"));
    assert!(narrow_lines[2].contains("Record 1 of 1"));
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
        controls(app, 80)
            .lines
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.content.as_ref()))
            .collect::<String>()
    };
    assert!(app.kind == SearchKind::Text);
    let word = labels(&app);
    assert!(word.contains("FTS"));
    assert!(word.contains("COLLECTION"));
    assert!(word.contains("Regex:"));
    assert!(word.contains("Text: normalized"));
    assert!(word.contains("Case: insensitive"));
    app.regex = true;
    assert!(!labels(&app).contains("Case:"));

    app.kind = SearchKind::FilterOn;
    let filter = labels(&app);
    assert!(filter.contains("FILTER ON"));
    assert!(filter.contains("Regex:"));
    assert!(filter.contains("Text:"));
    assert!(!filter.contains("Case:"));
    assert!(!filter.contains("BOTH"));
    let filter_lines = controls(&app, 80);
    assert!(
        filter_lines.lines[1].spans.is_empty()
            || filter_lines.lines[1]
                .spans
                .iter()
                .all(|span| span.content.trim().is_empty())
    );
    assert!(!filter_lines.lines[2].spans.is_empty());
    let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    let rendered = screen(&terminal);
    assert!(rendered.lines().nth(1).unwrap().trim().is_empty());
    assert!(rendered.lines().nth(2).unwrap().contains("Regex:"));
    assert!(rendered.lines().nth(3).unwrap().starts_with('╭'));
    assert!(rendered.contains("JSON FILTER"));
    assert!(rendered.contains("FULL-TEXT SEARCH"));
    app.collection = 1;
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    assert!(screen(&terminal).lines().next().unwrap().ends_with("docs"));
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
    assert!((3..28).any(|y| {
        buffer
            .cell((1, y))
            .is_some_and(|cell| cell.symbol() == "n" && cell.fg == theme::ACCENT)
    }));
}
