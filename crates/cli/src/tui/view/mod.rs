//! Drawing. The frame keeps one fixed skeleton in every state:
//! header, a blank row, two query rows, a summary rule, the body, and the
//! footer. The detail view replaces only the three query rows.

pub(super) mod overlay;
pub(super) mod record;
pub(super) mod results;
mod syntax;
pub(super) mod theme;

use super::app::{App, Line, Representation, View};
use super::input::Input;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::{Modifier, Style},
    text::{Line as TextLine, Span},
    widgets::Paragraph,
};
use std::time::SystemTime;
use theme::*;
use unicode_width::UnicodeWidthStr;

/// Width from which the preview pane is shown beside the results.
pub(super) const PREVIEW_MIN_WIDTH: u16 = 110;
/// Width of the label column of the query lines.
pub(super) const LABEL: u16 = 10;


/// The rectangles of the fixed skeleton.
pub(super) struct Areas {
    pub(super) header: Rect,
    pub(super) search: Rect,
    pub(super) filter: Rect,
    pub(super) summary: Rect,
    pub(super) body: Rect,
    pub(super) footer: Rect,
}


pub(super) fn areas(area: Rect) -> Areas {
    let [header, _gap, search, filter, summary, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);
    Areas {
        header,
        search,
        filter,
        summary,
        body,
        footer,
    }
}


pub(super) fn draw(frame: &mut Frame<'_>, app: &App<'_>) {
    let areas = areas(frame.area());
    header(frame, app, areas.header);
    match app.view {
        View::Results => {
            query_line(frame, app, Line::Search, areas.search);
            query_line(frame, app, Line::Filter, areas.filter);
            summary(frame, app, areas.summary);
            body(frame, app, areas.body);
        }
        View::Detail => record::detail(frame, app, &areas),
    }
    footer(frame, app, areas.footer);
    overlay::draw(frame, app, &areas);
    theme::remap(frame.buffer_mut());
}


/// Results with the preview beside them on wide terminals.
fn body(frame: &mut Frame<'_>, app: &App<'_>, area: Rect) {
    if app.hits.is_empty() {
        app.visible_rows
            .set(area.height.saturating_sub(1).max(1) as usize);
        results::empty(frame, app, area);
        return;
    }
    if app.show_preview && area.width >= PREVIEW_MIN_WIDTH {
        let preview = (area.width * 2 / 5).clamp(40, 72);
        let [list, pane] =
            Layout::horizontal([Constraint::Fill(1), Constraint::Length(preview)]).areas(area);
        results::table(frame, app, list);
        record::preview(frame, app, pane);
    } else {
        results::table(frame, app, area);
    }
}


fn header(frame: &mut Frame<'_>, app: &App<'_>, area: Rect) {
    let mut left = vec![
        Span::styled(
            " ◆ objekte",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled("   ", Style::new()),
        Span::styled(" ", Style::new()),
        Span::styled(
            app.scope_label().to_string(),
            Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ▾", Style::new().fg(DIM)),
        Span::styled("  Ctrl+O", Style::new().fg(FAINT)),
    ];
    if app.scope == 0 {
        left.insert(2, Span::styled("in", Style::new().fg(DIM)));
    } else {
        left.insert(2, Span::styled("in collection", Style::new().fg(DIM)));
    }
    frame.render_widget(Paragraph::new(TextLine::from(left)), area);
    let right = if let Some(status) = &app.index_status {
        TextLine::from(vec![
            Span::styled(format!("{} ", spinner()), Style::new().fg(AMBER)),
            Span::styled(format!("{status} "), Style::new().fg(DIM)),
        ])
    } else {
        TextLine::from(vec![
            Span::styled("● ", Style::new().fg(GREEN)),
            Span::styled("indexes ready ", Style::new().fg(DIM)),
        ])
    };
    frame.render_widget(Paragraph::new(right).right_aligned(), area);
}


/// One query line: a label, the text with a cursor, and option chips.
fn query_line(frame: &mut Frame<'_>, app: &App<'_>, line: Line, area: Rect) {
    let active = app.line == line && app.overlay.is_none();
    let (label, input, hint) = match line {
        Line::Search => ("Search", &app.search, "words or text in any field"),
        Line::Filter => (
            "Filter",
            &app.filter,
            "fields, e.g.  year >= 1800 && entry.year < 1820   (suggests as you type)",
        ),
    };
    let error = app.error_confirmed
        && app
            .query_error
            .as_ref()
            .is_some_and(|(failed, _)| *failed == line);
    let label_style = if error {
        Style::new().fg(RED).add_modifier(Modifier::BOLD)
    } else if active {
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(DIM)
    };
    let marker = if active { "❯" } else { " " };
    let [label_area, text_area] =
        Layout::horizontal([Constraint::Length(LABEL), Constraint::Fill(1)]).areas(area);
    frame.render_widget(
        Paragraph::new(TextLine::from(vec![
            Span::styled(format!(" {marker} "), Style::new().fg(ACCENT)),
            Span::styled(label, label_style),
        ])),
        label_area,
    );
    let chips = if line == Line::Search {
        chips(app)
    } else {
        Vec::new()
    };
    let chips_width = chips.iter().map(|span| span.content.width()).sum::<usize>() as u16;
    let [text_area, chip_area] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(chips_width)]).areas(text_area);
    if input.text.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                hint,
                Style::new().fg(FAINT).add_modifier(Modifier::ITALIC),
            )),
            text_area,
        );
        if active {
            frame.set_cursor_position(Position::new(text_area.x, text_area.y));
        }
    } else {
        let styles = if line == Line::Filter {
            syntax::filter_styles(&input.text)
        } else {
            vec![Style::new().fg(TEXT); input.len()]
        };
        // Point at the character a filter error names, once Enter made it
        // an error; while typing, the line is incomplete most of the time.
        let mark = app
            .query_error
            .as_ref()
            .filter(|(failed, _)| *failed == line && app.error_confirmed)
            .and_then(|(_, error)| syntax::error_position(&input.text, error))
            .map(|at| {
                (
                    at,
                    Style::new()
                        .add_modifier(Modifier::UNDERLINED)
                        .underline_color(RED)
                        .bg(MATCH),
                )
            });
        let ghost = (line == Line::Filter && active)
            .then(|| app.completion_ghost())
            .flatten();
        draw_styled_input(
            frame,
            input,
            text_area,
            active,
            &styles,
            mark,
            ghost.as_deref(),
        );
    }
    frame.render_widget(Paragraph::new(TextLine::from(chips)), chip_area);
}

/// Option chips: a lit chip is active.
fn chips(app: &App<'_>) -> Vec<Span<'static>> {
    let chip = |text: &'static str, on: bool, key: &'static str| {
        let style = if on {
            Style::new()
                .fg(BACKGROUND_ON)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(DIM)
        };
        [
            Span::styled(format!(" {text} "), style),
            Span::styled(format!("{key} "), Style::new().fg(FAINT)),
        ]
    };
    // With regex on, case and markup options do not apply: a regex reads
    // the stored text and sets its own case. Their chips are greyed out.
    let disabled = |text: &'static str, key: &'static str| {
        [
            Span::styled(format!(" {text} "), Style::new().fg(FAINT)),
            Span::styled(format!("{key} "), Style::new().fg(FAINT)),
        ]
    };
    let mut spans = Vec::new();
    if app.regex {
        spans.extend(disabled("Aa", "Alt+C"));
    } else {
        spans.extend(chip("Aa", app.case_sensitive, "Alt+C"));
    }
    spans.extend(chip(".*", app.regex, "Alt+R"));
    // The text chip names the current mode, so it is always lit: bold for
    // the default normalized text, highlighted for the stored text.
    if app.regex {
        spans.extend(disabled("txt", "Alt+T"));
    } else if app.representation == Representation::Full {
        spans.extend(chip("</>", true, "Alt+T"));
    } else {
        spans.extend([
            Span::styled(" txt ", Style::new().fg(TEXT).add_modifier(Modifier::BOLD)),
            Span::styled("Alt+T ", Style::new().fg(FAINT)),
        ]);
    }
    spans
}

/// Draw an input, scrolled so the cursor stays visible.
pub(super) fn draw_input(
    frame: &mut Frame<'_>,
    input: &Input,
    area: Rect,
    active: bool,
    style: Style,
) {
    let styles = vec![style; input.len()];
    draw_styled_input(frame, input, area, active, &styles, None, None);
}

/// Draw an input with one style per character, scrolled so the cursor
/// stays visible. `mark` underlines one character, or the end of the text.
pub(super) fn draw_styled_input(
    frame: &mut Frame<'_>,
    input: &Input,
    area: Rect,
    active: bool,
    styles: &[Style],
    mark: Option<(usize, Style)>,
    ghost: Option<&str>,
) {
    let width = area.width.max(1) as usize;
    let ghost_span = || ghost.map(|text| Span::styled(text.to_owned(), Style::new().fg(FAINT)));
    let before = input.text.chars().take(input.cursor).collect::<String>();
    let cursor_column = before.width();
    let skip_columns = cursor_column.saturating_sub(width.saturating_sub(1));
    let mut skipped = 0;
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut count = 0;
    for (index, ch) in input.text.chars().enumerate() {
        count = index + 1;
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if skipped + w <= skip_columns && spans.is_empty() {
            skipped += w;
            continue;
        }
        if index == input.cursor
            && let Some(span) = ghost_span()
        {
            spans.push(span);
        }
        let mut style = styles.get(index).copied().unwrap_or_default();
        if let Some((at, marked)) = mark
            && at == index
        {
            style = style.patch(marked);
        }
        match spans.last_mut() {
            Some(last) if last.style == style => last.content.to_mut().push(ch),
            _ => spans.push(Span::styled(ch.to_string(), style)),
        }
    }
    if input.cursor >= count
        && let Some(span) = ghost_span()
    {
        spans.push(span);
    }
    if let Some((at, marked)) = mark
        && at >= count
    {
        spans.push(Span::styled(" ", marked));
    }
    frame.render_widget(Paragraph::new(TextLine::from(spans)), area);
    if active {
        let x = area.x + (cursor_column - skipped).min(width.saturating_sub(1)) as u16;
        frame.set_cursor_position(Position::new(x, area.y));
    }
}


/// The rule below the query lines: counts, timing, sort, or an error.
fn summary(frame: &mut Frame<'_>, app: &App<'_>, area: Rect) {
    let rule = Style::new().fg(FAINT);
    let mut left = vec![Span::styled(" ", rule)];
    let hint_hidden = !app.error_confirmed && app.completion.is_some();
    if let Some((_, error)) = app.query_error.as_ref().filter(|_| !hint_hidden) {
        if app.error_confirmed {
            left.push(Span::styled(
                " ✗ ",
                Style::new().fg(RED).add_modifier(Modifier::BOLD),
            ));
            left.push(Span::styled(compact(error), Style::new().fg(RED)));
        } else {
            left.push(Span::styled(" … ", Style::new().fg(AMBER)));
            left.push(Span::styled(compact(error), Style::new().fg(DIM)));
        }
        left.push(Span::styled(" ", rule));
    } else if app.last_request.is_some() || app.searching {
        let count = match (app.total, app.searching) {
            (Some(total), _) => format!(
                "{total}{} {}",
                if app.truncated { "+" } else { "" },
                if total == 1 { "match" } else { "matches" }
            ),
            (None, true) => format!("{} searching", spinner()),
            (None, false) => String::new(),
        };
        left.push(Span::styled(
            format!(" {count} "),
            Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
        ));
        if app.searching && app.total.is_some() {
            left.push(Span::styled(
                format!("{} ", spinner()),
                Style::new().fg(AMBER),
            ));
        }
        if let Some(ms) = app.elapsed_ms {
            left.push(Span::styled(
                format!("· {} ", duration(ms)),
                Style::new().fg(DIM),
            ));
        }
        if let Some(request) = &app.last_request
            && !request.sort().is_empty()
        {
            let sort = request
                .sort()
                .iter()
                .map(|term| {
                    let arrow = if term.direction == objekte_core::Direction::Desc {
                        "↓"
                    } else {
                        "↑"
                    };
                    format!("{}{arrow}", term.field)
                })
                .collect::<Vec<_>>()
                .join(", ");
            left.push(Span::styled(
                format!("· sorted by {sort} "),
                Style::new().fg(DIM),
            ));
        }
    } else if app.queued_search || app.indexes_busy() && app.dirty.is_some() {
        left.push(Span::styled(
            " waiting for indexes… ",
            Style::new().fg(AMBER),
        ));
    }
    let used = left.iter().map(|span| span.content.width()).sum::<usize>();
    let right = if app.hits.is_empty() {
        String::new()
    } else {
        let rows = app.visible_rows.get();
        let last = (app.scroll + rows).min(app.hits.len());
        if app.total == Some(app.hits.len()) {
            format!(" {}–{} of {} ", app.scroll + 1, last, app.hits.len())
        } else {
            format!(" {}–{} · {} loaded ", app.scroll + 1, last, app.hits.len())
        }
    };
    let fill = (area.width as usize).saturating_sub(used + right.width());
    left.push(Span::styled("─".repeat(fill), rule));
    left.push(Span::styled(right, Style::new().fg(DIM)));
    frame.render_widget(Paragraph::new(TextLine::from(left)), area);
}

/// The first line of an error, shortened.
fn compact(error: &str) -> String {
    let line = error.lines().next().unwrap_or("");
    line.strip_prefix("invalid request: ")
        .unwrap_or(line)
        .to_string()
}

fn duration(ms: f64) -> String {
    if ms < 10.0 {
        format!("{ms:.1} ms")
    } else if ms < 1000.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{:.2} s", ms / 1000.0)
    }
}


fn footer(frame: &mut Frame<'_>, app: &App<'_>, area: Rect) {
    if app.view == View::Detail
        && let Some(input) = &app.record.find_input
    {
        let [label, text] =
            Layout::horizontal([Constraint::Length(8), Constraint::Fill(1)]).areas(area);
        frame.render_widget(
            Paragraph::new(Span::styled(
                " Find ❯ ",
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            )),
            label,
        );
        let count = app.record_find_count();
        let note = format!(" {count} {} ", if count == 1 { "match" } else { "matches" });
        let [text, note_area] =
            Layout::horizontal([Constraint::Fill(1), Constraint::Length(note.width() as u16)])
                .areas(text);
        draw_input(frame, input, text, true, Style::new().fg(TEXT));
        frame.render_widget(
            Paragraph::new(Span::styled(note, Style::new().fg(DIM))),
            note_area,
        );
        return;
    }
    if let Some(notice) = &app.notice
        && notice.at.elapsed() < super::app::NOTICE_TIME
    {
        let (icon, color) = if notice.error {
            ("✗", RED)
        } else {
            ("●", ACCENT)
        };
        frame.render_widget(
            Paragraph::new(TextLine::from(vec![
                Span::styled(format!(" {icon} "), Style::new().fg(color)),
                Span::styled(
                    notice.text.clone(),
                    Style::new().fg(if notice.error { RED } else { TEXT }),
                ),
            ])),
            area,
        );
        return;
    }
    let keys = hints(app);
    let mut spans = vec![Span::raw(" ")];
    let mut width = 1;
    for (key, action) in keys {
        // `key action` plus three spaces before the next hint; the last
        // hint may end at the edge.
        let item = key.width() + 1 + action.width();
        if width + item > area.width as usize {
            break;
        }
        let item = item + 3;
        width += item;
        spans.push(Span::styled(
            key,
            Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!(" {action}   "), Style::new().fg(DIM)));
    }
    frame.render_widget(Paragraph::new(TextLine::from(spans)), area);
}

/// Key hints for the current state, most important first.
fn hints(app: &App<'_>) -> Vec<(&'static str, &'static str)> {
    if app.completion.is_some() {
        let enter = if app.completion_active && !app.completion_is_typed() {
            "insert"
        } else {
            "search"
        };
        return vec![
            ("Tab", "insert"),
            ("↑↓", "choose"),
            ("Enter", enter),
            ("Esc", "close"),
        ];
    }
    match app.view {
        View::Detail => {
            let fields = app.links.section == super::navigation::DetailSection::Fields;
            let mut keys = Vec::new();
            if fields && !app.record.find.is_empty() {
                keys.push(("n N", "next / previous match"));
                keys.push(("Esc", "end find"));
            }
            keys.push(("←→", "previous / next"));
            keys.push(("Tab", "fields · links · backlinks"));
            if fields && app.record.raw {
                keys.extend([("r", "fields"), ("/", "find"), ("Y", "copy record")]);
            } else if fields {
                keys.extend([
                    ("Enter", "follow link / fold"),
                    ("f", "filter by value"),
                    ("y", "copy"),
                    ("/", "find"),
                    ("r", "raw JSON"),
                    ("e", "edit"),
                ]);
            } else {
                keys.push(("Enter", "open"));
            }
            if app.history_depth() > 0 {
                keys.push(("Backspace", "back"));
            }
            keys.extend([("Esc", "results"), ("F1", "help")]);
            keys
        }
        View::Results => {
            let mut keys = Vec::new();
            if !app.hits.is_empty() {
                keys.push(("↑↓", "move"));
                keys.push(("Enter", "open"));
            }
            keys.push(match app.line {
                Line::Search => ("Tab", "filter"),
                Line::Filter => ("Tab", "complete"),
            });
            if app.line == Line::Filter {
                keys.push(("⇧Tab", "search"));
            }
            if app.columns.len() > app.fixed_columns() + 1 && !app.hits.is_empty() {
                keys.push(("Alt+←→", "columns"));
            }
            keys.extend([
                ("Ctrl+O", "collection"),
                ("Ctrl+K", "actions"),
                ("F1", "help"),
                ("Ctrl+Q", "quit"),
            ]);
            keys
        }
    }
}


/// `text` in `style`, with every match of `patterns` marked.
pub(super) fn marked(text: &str, style: Style, patterns: &[&regex::Regex]) -> Vec<Span<'static>> {
    let mut ranges = patterns
        .iter()
        .flat_map(|pattern| pattern.find_iter(text).map(|found| found.range()))
        .filter(|range| !range.is_empty())
        .collect::<Vec<_>>();
    if ranges.is_empty() {
        return vec![Span::styled(text.to_owned(), style)];
    }
    ranges.sort_by_key(|range| range.start);
    let mark = style.bg(MATCH).fg(TEXT).add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    let mut at = 0;
    for range in ranges {
        if range.end <= at {
            continue;
        }
        let start = range.start.max(at);
        if start > at {
            spans.push(Span::styled(text[at..start].to_owned(), style));
        }
        spans.push(Span::styled(text[start..range.end].to_owned(), mark));
        at = range.end;
    }
    if at < text.len() {
        spans.push(Span::styled(text[at..].to_owned(), style));
    }
    spans
}

/// A braille spinner frame derived from the clock.
pub(super) fn spinner() -> &'static str {
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let millis = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    FRAMES[(millis / 80) as usize % FRAMES.len()]
}

/// Truncate text to `width` cells, ending with an ellipsis when cut.
pub(super) fn clip(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}
