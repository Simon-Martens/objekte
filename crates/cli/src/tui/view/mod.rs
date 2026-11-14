//! Fixed header, query lines, result list, detail, and small overlays.

mod syntax;
pub(super) mod theme;

use super::app::{App, Focus, Overlay, View};
use super::input::Input;
use super::record;
use objekte_search_core::diagnostics::milliseconds_3;
use objekte_search_core::inventory::FileStatus;
use objekte_search_core::lifecycle::InitialBuildPhase;
use objekte_search_core::path_repr;
use objekte_search_core::repository::SearchHit;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
    },
};
use std::time::Duration;
use theme::*;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(super) fn elapsed_label(elapsed: Duration) -> String {
    format!("{} ms", milliseconds_3(elapsed.as_nanos()))
}

#[cfg(test)]
pub(super) fn draw_loading(frame: &mut Frame<'_>, elapsed: Duration) {
    draw_loading_with_label(frame, elapsed, "Building search index");
}

pub(super) fn draw_loading_with_label(frame: &mut Frame<'_>, elapsed: Duration, label: &str) {
    let [header, _gap, status, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    frame.render_widget(
        Paragraph::new(" ◆ objekte").style(Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
        header,
    );
    frame.render_widget(
        Paragraph::new(format!(" {label}… {}", elapsed_label(elapsed))).style(Style::new().fg(DIM)),
        status,
    );
    frame.render_widget(
        Paragraph::new("Preparing repository; the first run may take longer."),
        body,
    );
    frame.render_widget(
        Paragraph::new(" Esc or Ctrl+Q quit").style(Style::new().fg(DIM)),
        footer,
    );
}

pub(super) fn draw(frame: &mut Frame<'_>, app: &mut App) {
    app.detail_area = Rect::default();
    app.results_area = Rect::default();
    let [header, _gap, search, filter, summary, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " ◆ objekte",
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled("   in  ", Style::new().fg(DIM)),
            Span::styled(
                app.scope_label().to_owned(),
                Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if app.single_collection {
                    "  Ctrl+O"
                } else {
                    "  ▾  Ctrl+O"
                },
                Style::new().fg(FAINT),
            ),
        ])),
        header,
    );
    if header.width >= 58 {
        let (label, color) = match app.build_status.as_ref() {
            Some(status) if status.phase == InitialBuildPhase::Failed => {
                ("indexing failed".to_owned(), RED)
            }
            Some(status) if status.phase == InitialBuildPhase::Cancelled => {
                ("indexing cancelled".to_owned(), RED)
            }
            Some(status) if !status.construction_complete => (
                format!(
                    "Indexing — {} of {} files",
                    status.processed_files, status.discovered_files
                ),
                DIM,
            ),
            _ if app.view == View::Files => ("file catalogue".to_owned(), DIM),
            _ => ("indexes ready".to_owned(), GREEN),
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("● ", Style::new().fg(color)),
                Span::styled(format!("{label} "), Style::new().fg(color)),
            ]))
            .right_aligned(),
            header,
        );
    }
    match app.view {
        View::Files => {
            frame.render_widget(
                Paragraph::new(" Files discovered from the source tree · Ctrl+I returns to search")
                    .style(Style::new().fg(DIM)),
                search,
            );
            frame.render_widget(
                Paragraph::new(
                    " File metadata only; record IDs and validity are known after indexing",
                )
                .style(Style::new().fg(FAINT)),
                filter,
            );
            frame.render_widget(
                Paragraph::new(format!(" {} files in catalogue", app.file_count()))
                    .style(Style::new().fg(DIM)),
                summary,
            );
            files(frame, app, body);
        }
        View::Results => {
            query_line(
                frame,
                "Search",
                &app.search,
                app.focus == Focus::Search,
                search,
                false,
                Some((app.case_sensitive, app.regex)),
            );
            query_line(
                frame,
                "Filter",
                &app.filter,
                app.focus == Focus::Filter,
                filter,
                true,
                None,
            );
            let status = if app.searching {
                if app.provisional {
                    "Searching available records…".to_owned()
                } else {
                    "Searching…".to_owned()
                }
            } else if let Some(error) = &app.error {
                let timing = app
                    .last_search_duration
                    .map_or_else(String::new, |elapsed| {
                        format!("  ·  all {}", elapsed_label(elapsed))
                    });
                format!("Error: {error}{timing}")
            } else if app.provisional {
                let progress = app
                    .build_status
                    .as_ref()
                    .map_or_else(String::new, |status| {
                        format!(
                            "  ·  Indexing — {} of {} files",
                            status.processed_files, status.discovered_files
                        )
                    });
                format!(
                    "≥{} provisional matches  ·  {} shown{progress}",
                    app.total,
                    app.hits.len()
                )
            } else {
                let sort = app.sort.as_ref().map_or_else(String::new, |sort| {
                    format!(
                        "  ·  {} {}",
                        sort.field,
                        if sort.descending { "↓" } else { "↑" }
                    )
                });
                let first = app
                    .last_first_result_duration
                    .map_or_else(String::new, |elapsed| {
                        format!("  ·  first {}", elapsed_label(elapsed))
                    });
                let timing = app
                    .last_search_duration
                    .map_or_else(String::new, |elapsed| {
                        format!("  ·  all {}", elapsed_label(elapsed))
                    });
                #[cfg(feature = "detailed-profiling")]
                let display = if app.single_collection {
                    app.last_projection_duration
                        .map_or_else(String::new, |elapsed| {
                            format!("  ·  display {}", elapsed_label(elapsed))
                        })
                } else {
                    String::new()
                };
                #[cfg(not(feature = "detailed-profiling"))]
                let display = String::new();
                format!(
                    "{} matches  ·  {} loaded{sort}{first}{timing}{display}",
                    app.total,
                    app.hits.len()
                )
            };
            frame.render_widget(
                Paragraph::new(format!(" {status}"))
                    .style(Style::new().fg(if app.error.is_some() { RED } else { DIM })),
                summary,
            );
            if app.show_preview && body.width >= 110 && !app.hits.is_empty() {
                let [table, preview_area] = Layout::horizontal([
                    Constraint::Fill(1),
                    Constraint::Length((body.width * 2 / 5).clamp(40, 72)),
                ])
                .areas(body);
                results(frame, app, table);
                preview(frame, app, preview_area);
            } else {
                results(frame, app, body);
            }
        }
        View::Detail => detail(
            frame,
            app,
            Rect {
                x: body.x,
                y: search.y,
                width: body.width,
                height: body.bottom().saturating_sub(search.y),
            },
        ),
    }
    let hint = if app.view == View::Files {
        " Ctrl+I or Esc search · ↑↓ files · PgUp/PgDn page · Ctrl+Q quit"
    } else if app.single_collection && app.view == View::Detail {
        " Esc back · ↑↓ fields · Enter fold · Alt+←→ records · / find · y/Y copy · r raw"
    } else if app.view == View::Detail && footer.width >= 96 {
        " Esc back · Tab sections · ↑↓ fields · Enter open · Alt+←→ records · / find · y/Y copy · r raw"
    } else if app.view == View::Detail {
        " Esc back · Tab sections · ↑↓ fields · Enter open · / find · y/Y copy · r raw"
    } else if footer.width < 78 {
        " Tab fields · ↑↓ rows · Enter open · Ctrl+K actions · F1 help"
    } else {
        " Tab fields · ↑↓ rows · Enter open · Ctrl+K actions · Alt+←→ columns · F1 help"
    };
    if let Some(input) = &app.record.find_input {
        frame.render_widget(
            Paragraph::new(format!(" Find ❯ {}", input.text)).style(Style::new().fg(ACCENT)),
            footer,
        );
        let prefix = input.text.chars().take(input.cursor).collect::<String>();
        frame.set_cursor_position(Position::new(
            footer.x + (prefix.width() + 8).min(footer.width.saturating_sub(1) as usize) as u16,
            footer.y,
        ));
    } else {
        let completion_hint = if app.overlay.is_none() {
            app.completion_hint()
        } else {
            None
        };
        let build_issue = app.build_status.as_ref().and_then(|status| {
            matches!(
                status.phase,
                InitialBuildPhase::Cancelled | InitialBuildPhase::Failed
            )
            .then(|| {
                status
                    .error
                    .as_deref()
                    .unwrap_or("Initial indexing stopped")
            })
        });
        frame.render_widget(
            Paragraph::new(
                build_issue
                    .or(completion_hint.as_deref())
                    .or(app.status.as_deref())
                    .unwrap_or(hint),
            )
            .style(Style::new().fg(if build_issue.is_some() { RED } else { DIM })),
            footer,
        );
    }
    if let Some(overlay) = app.overlay {
        overlay_view(frame, app, overlay);
    }
    theme::remap(frame.buffer_mut());
}

fn query_line(
    frame: &mut Frame<'_>,
    label: &str,
    input: &Input,
    active: bool,
    area: Rect,
    filter: bool,
    options: Option<(bool, bool)>,
) {
    let [label_area, remaining] =
        Layout::horizontal([Constraint::Length(10), Constraint::Fill(1)]).areas(area);
    let options = options.filter(|_| remaining.width >= 44);
    let option_width = if options.is_some() { 27 } else { 0 };
    let [text_area, option_area] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(option_width)])
            .areas(remaining);
    frame.render_widget(
        Paragraph::new(format!(" {} {label}", if active { "❯" } else { " " }))
            .style(Style::new().fg(if active { ACCENT } else { DIM })),
        label_area,
    );
    if input.text.is_empty() {
        let hint = if filter {
            "e.g. year >= 1800"
        } else {
            "words or text in any field"
        };
        frame.render_widget(
            Paragraph::new(hint).style(Style::new().fg(FAINT).add_modifier(Modifier::ITALIC)),
            text_area,
        );
    } else {
        let styles = if filter {
            syntax::filter_styles(&input.text)
        } else {
            vec![Style::new().fg(TEXT); input.len()]
        };
        let chars = input.text.chars().collect::<Vec<_>>();
        let cursor_width = chars
            .iter()
            .take(input.cursor)
            .map(|ch| ch.width().unwrap_or(0))
            .sum::<usize>();
        let available = text_area.width as usize;
        let mut skip_width = cursor_width.saturating_sub(available.saturating_sub(1));
        let mut skip = 0;
        while skip < chars.len() && skip_width > 0 {
            skip_width = skip_width.saturating_sub(chars[skip].width().unwrap_or(0));
            skip += 1;
        }
        let mut used = 0;
        let spans = chars
            .into_iter()
            .zip(styles)
            .skip(skip)
            .take_while(|(ch, _)| {
                let width = ch.width().unwrap_or(0);
                if used + width > available {
                    false
                } else {
                    used += width;
                    true
                }
            })
            .map(|(ch, style)| Span::styled(ch.to_string(), style))
            .collect::<Vec<_>>();
        frame.render_widget(Paragraph::new(Line::from(spans)), text_area);
        if active && text_area.width > 0 {
            let before = input
                .text
                .chars()
                .skip(skip)
                .take(input.cursor.saturating_sub(skip));
            let column = before.map(|ch| ch.width().unwrap_or(0)).sum::<usize>();
            frame.set_cursor_position(Position::new(
                text_area.x + column.min(available.saturating_sub(1)) as u16,
                text_area.y,
            ));
        }
    }
    if active && input.text.is_empty() && text_area.width > 0 {
        frame.set_cursor_position(Position::new(text_area.x, text_area.y));
    }
    if let Some((sensitive, regex)) = options {
        let chip = |label, on| {
            Span::styled(
                label,
                if on {
                    Style::new()
                        .fg(BACKGROUND_ON)
                        .bg(ACCENT)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(DIM)
                },
            )
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                chip(" Aa ", sensitive),
                Span::styled("Alt+C ", Style::new().fg(FAINT)),
                chip(" .* ", regex),
                Span::styled("Alt+R ", Style::new().fg(FAINT)),
            ])),
            option_area,
        );
    }
}

fn results(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    app.results_area = area;
    app.viewport_rows = area.height.saturating_sub(1).max(1) as usize;
    if area.height == 0 {
        return;
    }
    if app.single_collection {
        results_cli(frame, app, area);
        return;
    }
    let compact = area.width < 100;
    let collection_width = if compact { 12usize } else { 16 };
    let id_width = if compact { 16usize } else { 24 };
    let field_width = if area.width >= 150 { 24 } else { 16 };
    let fixed = 4 + collection_width + 2 + id_width + 2;
    let minimum_match_width =
        (if compact { 18 } else { 24 }).min((area.width as usize).saturating_sub(fixed));
    let available = (area.width as usize).saturating_sub(fixed + minimum_match_width + 2);
    let columns = app.visible_columns();
    let field_count = (available / (field_width + 2)).min(4).min(columns.len());
    let match_width = minimum_match_width + available - field_count * (field_width + 2);
    let fields = columns
        .into_iter()
        .skip(app.column_offset)
        .take(field_count)
        .collect::<Vec<_>>();
    let mut header = format!(
        "    {}  {}  ",
        pad_clip("collection", collection_width),
        pad_clip("id", id_width)
    );
    header.push_str(&pad_clip("match / record", match_width));
    header.push_str("  ");
    for field in &fields {
        header.push_str(&pad_clip(field, field_width));
        header.push_str("  ");
    }
    frame.render_widget(
        Paragraph::new(header).style(Style::new().fg(DIM).add_modifier(Modifier::BOLD)),
        Rect { height: 1, ..area },
    );
    if app.hits.is_empty() {
        let message = if app.provisional {
            "No available matches yet; indexing continues."
        } else if app.searching {
            "Searching…"
        } else if app.error.is_some() {
            "Correct the query and press Enter."
        } else {
            "No matches. Try another query or collection."
        };
        frame.render_widget(
            Paragraph::new(message).style(Style::new().fg(FAINT)),
            Rect {
                y: area.y + 1,
                height: 1,
                ..area
            },
        );
        return;
    }
    for (row, hit) in app
        .hits
        .iter()
        .skip(app.scroll)
        .take(app.viewport_rows)
        .enumerate()
    {
        let index = app.scroll + row;
        let selected = index == app.selected;
        let base = if selected {
            Style::new().bg(SELECTED)
        } else {
            Style::new()
        };
        let summary = record_summary(hit);
        let status_color = match hit.file_status {
            FileStatus::Valid => GREEN,
            FileStatus::Broken | FileStatus::Unreadable => RED,
        };
        let mut spans = vec![
            Span::styled(if selected { "▌ " } else { "  " }, Style::new().fg(ACCENT)),
            Span::styled("● ", Style::new().fg(status_color)),
            Span::styled(
                pad_clip(&hit.collection, collection_width),
                Style::new().fg(VIOLET),
            ),
            Span::raw("  "),
            Span::styled(pad_clip(&hit.record_id, id_width), Style::new().fg(DIM)),
            Span::raw("  "),
            Span::styled(pad_clip(&summary, match_width), Style::new().fg(TEXT)),
            Span::raw("  "),
        ];
        for field in &fields {
            let value = record::column_value(hit, field);
            spans.push(Span::styled(
                pad_clip(&value, field_width),
                Style::new().fg(if value.starts_with('[') || value.starts_with('{') {
                    DIM
                } else {
                    TEXT
                }),
            ));
            spans.push(Span::raw("  "));
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans)).style(base),
            Rect {
                y: area.y + 1 + row as u16,
                height: 1,
                ..area
            },
        );
    }
}

fn files(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    app.results_area = area;
    app.viewport_rows = area.height.saturating_sub(1).max(1) as usize;
    if area.height == 0 {
        return;
    }
    let id_width = 9;
    let size_width = 12;
    let time_width = 18;
    let path_width = (area.width as usize).saturating_sub(id_width + size_width + time_width + 8);
    frame.render_widget(
        Paragraph::new(format!(
            "  {}  {}  {}  {}",
            pad_clip("FileId", id_width),
            pad_clip("relative path", path_width),
            pad_clip("bytes", size_width),
            pad_clip("mtime (Unix s)", time_width),
        ))
        .style(Style::new().fg(DIM).add_modifier(Modifier::BOLD)),
        Rect { height: 1, ..area },
    );
    for row in 0..app.viewport_rows {
        let index = app.file_scroll + row;
        let Some(file) = app.file_at(index) else {
            continue;
        };
        let selected = index == app.file_selected;
        let style = if selected {
            Style::new().bg(SELECTED)
        } else {
            Style::new()
        };
        let path = path_repr::query_path(file.relative_path);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(if selected { "▌ " } else { "  " }, Style::new().fg(ACCENT)),
                Span::styled(
                    pad_clip(&file.id.0.to_string(), id_width),
                    Style::new().fg(DIM),
                ),
                Span::raw("  "),
                Span::styled(pad_clip(&path, path_width), Style::new().fg(TEXT)),
                Span::raw("  "),
                Span::styled(
                    pad_clip(&file.size.to_string(), size_width),
                    Style::new().fg(DIM),
                ),
                Span::raw("  "),
                Span::styled(
                    pad_clip(&(file.modified_ns / 1_000_000_000).to_string(), time_width),
                    Style::new().fg(DIM),
                ),
            ]))
            .style(style),
            Rect {
                y: area.y + 1 + row as u16,
                height: 1,
                ..area
            },
        );
    }
}

fn results_cli(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let width = area.width as usize;
    let id_width = if width < 80 { 18 } else { 28 };
    let fixed = 6 + id_width + 2;
    let field_width = if width < 110 { 14 } else { 22 };
    let optional = app.visible_columns();
    let available = width.saturating_sub(fixed + 16);
    let field_count =
        (available / (field_width + 2)).min(optional.len().saturating_sub(app.column_offset));
    let match_width = width.saturating_sub(fixed + field_count * (field_width + 2));
    let fields = optional
        .iter()
        .skip(app.column_offset)
        .take(field_count)
        .collect::<Vec<_>>();
    let mut header = format!("valid {}  ", pad_clip("id", id_width));
    for field in &fields {
        header.push_str(&pad_clip(field, field_width));
        header.push_str("  ");
    }
    header.push_str(&pad_clip("match", match_width));
    frame.render_widget(
        Paragraph::new(header).style(Style::new().fg(DIM).add_modifier(Modifier::BOLD)),
        Rect { height: 1, ..area },
    );
    if app.hits.is_empty() {
        let message = if app.provisional {
            "No available matches yet; indexing continues."
        } else if app.searching {
            "Searching…"
        } else if app.error.is_some() {
            "Correct the query and press Enter."
        } else {
            "No matches."
        };
        frame.render_widget(
            Paragraph::new(message).style(Style::new().fg(FAINT)),
            Rect {
                y: area.y + 1,
                height: 1,
                ..area
            },
        );
        return;
    }
    for (row, hit) in app
        .hits
        .iter()
        .skip(app.scroll)
        .take(app.viewport_rows)
        .enumerate()
    {
        let index = app.scroll + row;
        let selected = index == app.selected;
        let display = app.display_rows.get(index);
        let status_color = match hit.file_status {
            FileStatus::Valid => GREEN,
            FileStatus::Broken | FileStatus::Unreadable => RED,
        };
        let mut spans = vec![
            Span::styled(if selected { "▌ " } else { "  " }, Style::new().fg(ACCENT)),
            Span::styled("● ", Style::new().fg(status_color)),
            Span::raw("  "),
            Span::styled(pad_clip(&hit.record_id, id_width), Style::new().fg(DIM)),
            Span::raw("  "),
        ];
        for field in &fields {
            let position = app.column_keys().iter().position(|name| name == *field);
            let value = position
                .and_then(|position| display.and_then(|row| row.fields.get(position)))
                .map_or("", String::as_str);
            spans.push(Span::styled(
                pad_clip(value, field_width),
                Style::new().fg(TEXT),
            ));
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            pad_clip(display.map_or("", |row| row.matched.as_str()), match_width),
            Style::new().fg(TEXT),
        ));
        frame.render_widget(
            Paragraph::new(Line::from(spans)).style(if selected {
                Style::new().bg(SELECTED)
            } else {
                Style::new()
            }),
            Rect {
                y: area.y + 1 + row as u16,
                height: 1,
                ..area
            },
        );
    }
}

fn record_summary(hit: &SearchHit) -> String {
    let Some(value) = record::json_value(hit) else {
        return String::new();
    };
    for field in &hit.matched_fields {
        let selected = field
            .split('.')
            .try_fold(&value, |current, key| current.as_object()?.get(key));
        if let Some(selected) = selected {
            let display = selected
                .as_str()
                .map_or_else(|| selected.to_string(), str::to_owned);
            return format!(
                "{field}: {}",
                display.split_whitespace().collect::<Vec<_>>().join(" ")
            );
        }
    }
    if let Some(map) = value.as_object() {
        for key in ["preferred_title", "title", "name", "label", "text"] {
            if let Some(text) = map.get(key).and_then(serde_json::Value::as_str) {
                return text.split_whitespace().collect::<Vec<_>>().join(" ");
            }
        }
        if let Some((key, value)) = map.iter().find(|(_, value)| value.is_string()) {
            return format!("{key}: {}", value.as_str().unwrap_or_default());
        }
    }
    String::new()
}

fn mark_truncated(lines: &mut [String], width: usize) {
    if let Some(last) = lines.last_mut() {
        while last.width() + 1 > width {
            last.pop();
        }
        last.push('…');
    }
}

fn wrapped_record_value(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    if width == 0 || max_lines == 0 {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let safe = safe_terminal_text(text);
    let mut paragraphs = safe.split('\n').peekable();
    while let Some(paragraph) = paragraphs.next() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let mut remaining = word;
            loop {
                if !line.is_empty() {
                    if line.width() + 1 + remaining.width() <= width {
                        line.push(' ');
                        line.push_str(remaining);
                        break;
                    }
                    lines.push(std::mem::take(&mut line));
                    if lines.len() == max_lines {
                        mark_truncated(&mut lines, width);
                        return lines;
                    }
                }
                let mut split = remaining.len();
                let mut used = 0;
                for (offset, ch) in remaining.char_indices() {
                    let next = used + UnicodeWidthChar::width(ch).unwrap_or(0);
                    if next > width && offset > 0 {
                        split = offset;
                        break;
                    }
                    used = next;
                }
                let (part, rest) = remaining.split_at(split);
                line.push_str(part);
                if rest.is_empty() {
                    break;
                }
                lines.push(std::mem::take(&mut line));
                if lines.len() == max_lines {
                    mark_truncated(&mut lines, width);
                    return lines;
                }
                remaining = rest;
            }
        }
        lines.push(line);
        if lines.len() == max_lines {
            if paragraphs.peek().is_some() {
                mark_truncated(&mut lines, width);
            }
            return lines;
        }
    }
    lines
}

fn detail_row_lines(
    app: &App,
    row: &record::RecordRow,
    index: usize,
    field_rows: usize,
    name_width: usize,
    width: usize,
    height: usize,
) -> Vec<Line<'static>> {
    let chosen = app.view == View::Detail && index == app.record.cursor;
    let base = if chosen {
        Style::new().bg(SELECTED)
    } else {
        Style::new()
    };
    let heading = (index < field_rows
        && row.depth == 0
        && matches!(row.label.as_str(), "Fields" | "Metadata")
        && row.value.is_empty())
        || (index >= field_rows && matches!(row.label.as_str(), "Outgoing" | "Backlinks"));
    let marker = if chosen {
        "▌ "
    } else if row.matched {
        "▸ "
    } else {
        "  "
    };
    let label = if heading {
        row.label.clone()
    } else {
        let branch = match &row.branch {
            Some((path, array)) if app.record.is_folded(path, *array) => "▸ ",
            Some(_) => "▾ ",
            None => "  ",
        };
        format!("{}{}{}", "  ".repeat(row.depth.min(12)), branch, row.label)
    };
    let label_style = if heading || row.matched {
        base.fg(ACCENT).add_modifier(Modifier::BOLD)
    } else if chosen {
        base.fg(TEXT).add_modifier(Modifier::BOLD)
    } else {
        base.fg(DIM)
    };
    let value_style = if row.value.starts_with("dangling") {
        base.fg(RED)
    } else if index >= field_rows && row.matched {
        base.fg(VIOLET)
    } else if heading {
        base.fg(FAINT)
    } else {
        base.fg(TEXT)
    };
    let value_width = width.saturating_sub(name_width + 4);
    let value = if index >= field_rows && row.matched {
        format!("{}  →", row.value)
    } else {
        row.value.clone()
    };
    wrapped_record_value(&value, value_width, height.max(1))
        .into_iter()
        .enumerate()
        .map(|(line, value)| {
            let used = 2 + name_width + 2 + value.width();
            Line::from(vec![
                Span::styled(if line == 0 { marker } else { "  " }, base.fg(ACCENT)),
                Span::styled(
                    if line == 0 {
                        pad_clip(&label, name_width)
                    } else {
                        " ".repeat(name_width)
                    },
                    label_style,
                ),
                Span::styled("  ", base),
                Span::styled(value, value_style),
                Span::styled(" ".repeat(width.saturating_sub(used)), base),
            ])
        })
        .collect()
}

fn detail(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let Some(hit) = app.current_detail_hit().cloned() else {
        return;
    };
    let position = if app.selected_hit().is_some_and(|selected| {
        selected.collection == hit.collection && selected.record_id == hit.record_id
    }) {
        format!("{} of {} ", app.selected + 1, app.total)
    } else {
        "linked record ".to_owned()
    };
    let status_color = match hit.file_status {
        FileStatus::Valid => GREEN,
        FileStatus::Broken | FileStatus::Unreadable => RED,
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ◂ ", Style::new().fg(ACCENT)),
            Span::styled(hit.collection.clone(), Style::new().fg(VIOLET)),
            Span::styled(" / ", Style::new().fg(FAINT)),
            Span::styled(
                hit.record_id.clone(),
                Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled("   ● ", Style::new().fg(status_color)),
            Span::styled(hit.file_status.label(), Style::new().fg(DIM)),
        ])),
        Rect { height: 1, ..area },
    );
    frame.render_widget(
        Paragraph::new(Span::styled(position, Style::new().fg(DIM))).right_aligned(),
        Rect { height: 1, ..area },
    );

    let diagnostic = app
        .diagnostic(&hit)
        .map(|issue| format!("{}: {}", issue.code, issue.message));
    let mut rows = app.record.rows(&hit, diagnostic.as_deref());
    let field_rows = rows.len();
    rows.extend(app.relation_rows.iter().map(|row| record::RecordRow {
        label: row.label.clone(),
        value: row.value.clone(),
        depth: 0,
        matched: row.action.is_some(),
        branch: None,
    }));
    app.record.cursor = app.record.cursor.min(rows.len().saturating_sub(1));
    let section = rows
        .iter()
        .enumerate()
        .take(app.record.cursor + 1)
        .filter_map(|(index, row)| {
            (index >= field_rows)
                .then_some(row.label.as_str())
                .and_then(|label| match label {
                    "Outgoing" => Some(2),
                    "Backlinks" => Some(3),
                    _ => None,
                })
        })
        .last()
        .unwrap_or(1);
    let tab = |label: String, number| {
        Span::styled(
            format!(" {label} "),
            if section == number {
                Style::new()
                    .fg(BACKGROUND_ON)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(DIM)
            },
        )
    };
    let tabs = if app.single_collection {
        vec![Span::raw("   "), tab("Fields".into(), 1)]
    } else {
        vec![
            Span::raw("   "),
            tab("1 Fields".into(), 1),
            Span::raw("  "),
            tab("2 Links".into(), 2),
            Span::raw("  "),
            tab("3 Backlinks".into(), 3),
        ]
    };
    frame.render_widget(
        Paragraph::new(Line::from(tabs)),
        Rect {
            y: area.y + 1,
            height: 1,
            ..area
        },
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            "─".repeat(area.width as usize),
            Style::new().fg(FAINT),
        )),
        Rect {
            y: area.y + 2,
            height: 1,
            ..area
        },
    );
    let body = Rect {
        x: area.x + 1,
        y: area.y + 3,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(3),
    };
    app.detail_area = body;
    app.record.visible_rows.clear();
    if app.record.raw {
        frame.render_widget(
            Paragraph::new(record::raw_text(&hit))
                .wrap(Wrap { trim: false })
                .scroll((app.detail_scroll, 0)),
            Rect {
                x: body.x + 1,
                width: body.width.saturating_sub(1),
                ..body
            },
        );
        return;
    }
    let height = body.height as usize;
    let name_width = (body.width as usize / 3)
        .clamp(14, 36)
        .min((body.width as usize).saturating_sub(6));
    let width = body.width as usize;
    let mut start = app.record.scroll.min(app.record.cursor);
    let mut needed = (start..=app.record.cursor)
        .map(|index| {
            detail_row_lines(
                app,
                &rows[index],
                index,
                field_rows,
                name_width,
                width,
                height,
            )
            .len()
        })
        .sum::<usize>();
    while needed > height && start < app.record.cursor {
        needed -= detail_row_lines(
            app,
            &rows[start],
            start,
            field_rows,
            name_width,
            width,
            height,
        )
        .len();
        start += 1;
    }
    app.record.scroll = start;
    let mut line = 0;
    for (index, row) in rows.iter().enumerate().skip(start) {
        for content in detail_row_lines(app, row, index, field_rows, name_width, width, height) {
            if line == height {
                break;
            }
            frame.render_widget(
                Paragraph::new(content),
                Rect {
                    y: body.y + line as u16,
                    height: 1,
                    ..body
                },
            );
            app.record.visible_rows.push(index);
            line += 1;
        }
        if line == height {
            break;
        }
    }
    let shown_rows = app
        .record
        .visible_rows
        .last()
        .map_or(0, |last| last + 1 - start);
    if rows.len() > shown_rows && height > 0 {
        let mut state = ScrollbarState::new(rows.len() - shown_rows).position(start);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(Some("│"))
                .track_style(Style::new().fg(FAINT))
                .thumb_style(Style::new().fg(DIM)),
            Rect {
                x: body.right(),
                width: 1,
                ..body
            },
            &mut state,
        );
    }
}

fn preview(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    app.detail_area = area;
    let Some(hit) = app.current_detail_hit().cloned() else {
        return;
    };
    for y in area.top()..area.bottom() {
        frame.render_widget(
            Paragraph::new(Span::styled("│", Style::new().fg(FAINT))),
            Rect::new(area.x, y, 1, 1),
        );
    }
    let inner = Rect {
        x: area.x + 2,
        width: area.width.saturating_sub(3),
        ..area
    };
    let status_color = match hit.file_status {
        FileStatus::Valid => GREEN,
        FileStatus::Broken | FileStatus::Unreadable => RED,
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(hit.collection.clone(), Style::new().fg(VIOLET)),
            Span::styled(" / ", Style::new().fg(FAINT)),
            Span::styled(hit.record_id.clone(), Style::new().fg(TEXT)),
            Span::styled("  ● ", Style::new().fg(status_color)),
            Span::styled(hit.file_status.label(), Style::new().fg(DIM)),
        ])),
        Rect { height: 1, ..inner },
    );
    let body = Rect {
        y: inner.y + 2,
        height: inner.height.saturating_sub(2),
        ..inner
    };
    let diagnostic = app
        .diagnostic(&hit)
        .map(|issue| format!("{}: {}", issue.code, issue.message));
    let mut rows = app.record.rows(&hit, diagnostic.as_deref());
    if let Some(metadata) = rows.iter().position(|row| row.label == "Metadata") {
        rows.truncate(metadata);
    }
    let height = body.height as usize;
    let width = body.width as usize;
    let name_width = (width / 3).clamp(12, 28).min(width.saturating_sub(6));
    let first_match = rows.iter().position(|row| row.matched).unwrap_or(0);
    let mut start = first_match.saturating_sub(2);
    while start < first_match {
        let needed = (start..=first_match)
            .map(|index| {
                detail_row_lines(
                    app,
                    &rows[index],
                    index,
                    rows.len(),
                    name_width,
                    width,
                    height,
                )
                .len()
            })
            .sum::<usize>();
        if needed <= height {
            break;
        }
        start += 1;
    }
    let mut line = 0;
    for (index, row) in rows.iter().enumerate().skip(start) {
        for content in detail_row_lines(app, row, index, rows.len(), name_width, width, height) {
            if line == height {
                break;
            }
            frame.render_widget(
                Paragraph::new(content),
                Rect {
                    y: body.y + line as u16,
                    height: 1,
                    ..body
                },
            );
            line += 1;
        }
        if line == height {
            break;
        }
    }
}

fn storage_current_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from("Current index storage"),
        Line::from("Memory: no index files are read or written."),
        Line::from("On-demand: reads index bytes from disk when needed."),
        Line::from("Full residency: copies index bytes before readiness."),
        Line::from("Changed settings reopen this view; unsaved disk changes flush first."),
    ];
    let Some(view) = app.storage_view.as_ref() else {
        lines.push(Line::from("  Memory only · No index files are read or written"));
        return lines;
    };
    lines.extend([
        Line::from(format!("  Location: {}", view.location)),
        Line::from(format!("  Residency: {}", view.residency)),
        Line::from(format!("  Readiness: {}", view.readiness)),
        Line::from(format!("  Sources: {}", view.reconciliation)),
        Line::from(format!("  Synchronization: {}", view.synchronization)),
    ]);
    if !view.generations.is_empty() {
        lines.push(Line::from(format!("  {}", view.generations)));
        lines.push(Line::from(format!("  Validation: {}", view.validation)));
    }
    if let Some(path) = &view.managed_directory {
        lines.push(Line::from(format!("  Managed folder: {path}")));
    }
    if let Some(issue) = &view.issue {
        lines.push(Line::from(format!("  {}: {}", issue.summary, issue.guidance)));
        lines.push(Line::from(format!("  Details: {}", issue.detail)));
    }
    if let Some(warning) = &view.validation_warning {
        lines.push(Line::from(format!("  Warning: {}", warning.summary)));
        lines.push(Line::from(format!("  {} ({})", warning.guidance, warning.detail)));
    }
    lines
}

fn storage_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = storage_current_lines(app);
    for (index, label) in ["Memory", "System temporary folder", "Repository folder", "Custom folder"]
        .into_iter().enumerate() {
        lines.push(Line::from(Span::styled(
            format!("{} [{}] {label}", if app.storage_picker == index { "❯" } else { " " },
                if app.storage_choice == index { "x" } else { " " }),
            Style::new().fg(if app.storage_picker == index { ACCENT } else { TEXT }),
        )));
    }
    let options = [
        format!("{} [{}] Keep entire index in memory{}",
            if app.storage_picker == 4 { "❯" } else { " " },
            if app.storage_choice == 0 || app.storage_full { "x" } else { " " },
            if app.storage_choice == 0 { " (required)" } else { "" }),
        format!("{} Custom base: {}{}",
            if app.storage_picker == 5 { "❯" } else { " " }, app.storage_directory.text,
            if app.storage_editing { " ▏ editing" } else { "" }),
        format!("{} Apply and reopen", if app.storage_picker == 6 { "❯" } else { " " }),
        format!("{} {}", if app.storage_picker == 7 { "❯" } else { " " },
            if app.storage_syncing { "Synchronizing…" } else if app.storage_policy.location
                == objekte_search_core::index_storage::IndexStorageLocation::MemoryOnly {
                "No index files to synchronize (disabled)"
            } else if app.storage_sync_available { "Synchronize now / Retry synchronization" }
            else { "Storage is current (synchronize unavailable)" }),
    ];
    for (offset, label) in options.into_iter().enumerate() {
        lines.push(Line::from(Span::styled(label,
            Style::new().fg(if app.storage_picker == offset + 4 { ACCENT } else { TEXT }))));
    }
    lines
}

fn overlay_view(frame: &mut Frame<'_>, app: &App, overlay: Overlay) {
    let area = frame.area();
    let width = area.width.min(if overlay == Overlay::Storage { 100 } else { 70 })
        .saturating_sub(4).max(20);
    let height = match overlay {
        Overlay::Actions => {
            (super::app::Action::ALL.len() as u16 + 3).min(area.height.saturating_sub(2))
        }
        Overlay::Storage => 27.min(area.height.saturating_sub(2)),
        Overlay::Collections => {
            (app.collection_names().count() as u16 + 3).min(area.height.saturating_sub(2))
        }
        Overlay::Sort => (app.sort_choices().len() as u16
            + if app.single_collection { 5 } else { 3 })
        .min(area.height.saturating_sub(2)),
        Overlay::History => (app.settings.history.len() as u16 + 2)
            .min(20)
            .min(area.height.saturating_sub(2)),
        Overlay::Saved => (app.settings.saved.len() as u16 + 2)
            .min(20)
            .min(area.height.saturating_sub(2)),
        Overlay::SaveQuery => 5.min(area.height.saturating_sub(2)),
        Overlay::Export { .. } => 5.min(area.height.saturating_sub(2)),
        Overlay::Columns => (app.column_keys().len() as u16 + 2)
            .min(20)
            .min(area.height.saturating_sub(2)),
        Overlay::Help => 24.min(area.height.saturating_sub(2)),
    };
    let panel = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, panel);
    let title = match overlay {
        Overlay::Actions => " Actions · Ctrl+K ",
        Overlay::Storage => " Index storage ",
        Overlay::Collections => " Collections ",
        Overlay::Sort => " Sort by ",
        Overlay::History => " Recent queries ",
        Overlay::Saved => " Saved queries ",
        Overlay::SaveQuery => " Save query ",
        Overlay::Columns => " Columns ",
        Overlay::Export { all: true } => " Export all matches ",
        Overlay::Export { all: false } => " Export record ",
        Overlay::Help => " Keyboard ",
    };
    frame.render_widget(
        Block::default()
            .title(Span::styled(
                title,
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ))
            .style(Style::new().bg(PANEL))
            .borders(Borders::ALL)
            .border_style(Style::new().fg(ACCENT)),
        panel,
    );
    let inner = Rect {
        x: panel.x + 2,
        y: panel.y + 1,
        width: panel.width.saturating_sub(4),
        height: panel.height.saturating_sub(2),
    };
    let lines = match overlay {
        Overlay::Actions => super::app::Action::ALL
            .iter()
            .enumerate()
            .map(|(index, action)| {
                Line::from(vec![
                    Span::styled(
                        if index == app.action_picker {
                            "❯ "
                        } else {
                            "  "
                        },
                        Style::new().fg(ACCENT),
                    ),
                    Span::styled(
                        action.label(),
                        Style::new().fg(if index == app.action_picker {
                            TEXT
                        } else {
                            DIM
                        }),
                    ),
                ])
            })
            .collect(),
        Overlay::Storage => storage_lines(app),
        Overlay::Collections => std::iter::once("All collections".to_owned())
            .chain(app.collection_names().map(str::to_owned))
            .enumerate()
            .map(|(index, name)| {
                Line::from(Span::styled(
                    format!("{} {name}", if index == app.picker { "❯" } else { " " }),
                    Style::new().fg(if index == app.picker { ACCENT } else { TEXT }),
                ))
            })
            .collect::<Vec<_>>(),
        Overlay::Sort => app
            .sort_choices()
            .into_iter()
            .enumerate()
            .map(|(index, choice)| {
                let label = choice.map_or_else(
                    || "Default order".to_owned(),
                    |sort| {
                        format!(
                            "{} {}",
                            sort.field,
                            if sort.descending {
                                "descending"
                            } else {
                                "ascending"
                            }
                        )
                    },
                );
                Line::from(Span::styled(
                    format!(
                        "{} {label}",
                        if index == app.sort_picker { "❯" } else { " " }
                    ),
                    Style::new().fg(if index == app.sort_picker {
                        ACCENT
                    } else {
                        TEXT
                    }),
                ))
            })
            .collect::<Vec<_>>(),
        Overlay::History => app
            .settings
            .history
            .iter()
            .rev()
            .enumerate()
            .map(|(index, query)| {
                let scope = query.collection.as_deref().unwrap_or("*");
                let label = format!("{scope}  {}  {}", query.search, query.filter);
                Line::from(Span::styled(
                    format!(
                        "{} {label}",
                        if index == app.list_picker { "❯" } else { " " }
                    ),
                    Style::new().fg(if index == app.list_picker {
                        ACCENT
                    } else {
                        TEXT
                    }),
                ))
            })
            .collect(),
        Overlay::Saved => app
            .settings
            .saved
            .iter()
            .enumerate()
            .map(|(index, saved)| {
                Line::from(Span::styled(
                    format!(
                        "{} {}  {}  {}",
                        if index == app.list_picker { "❯" } else { " " },
                        saved.name,
                        saved.query.search,
                        saved.query.filter
                    ),
                    Style::new().fg(if index == app.list_picker {
                        ACCENT
                    } else {
                        TEXT
                    }),
                ))
            })
            .collect(),
        Overlay::SaveQuery => vec![
            Line::from("Save query as:"),
            Line::from(Span::styled(
                format!("❯ {}", app.save_input.text),
                Style::new().fg(ACCENT),
            )),
            Line::from(Span::styled(
                "Enter save  ·  Esc cancel",
                Style::new().fg(DIM),
            )),
        ],
        Overlay::Export { all } => vec![
            Line::from(if all {
                "Export all matches to:"
            } else {
                "Export selected record to:"
            }),
            Line::from(Span::styled(
                format!("❯ {}", app.export_input.text),
                Style::new().fg(ACCENT),
            )),
            Line::from(Span::styled(
                "Enter write new JSON file  ·  Esc cancel",
                Style::new().fg(DIM),
            )),
        ],
        Overlay::Columns => app
            .column_keys()
            .into_iter()
            .enumerate()
            .map(|(index, key)| {
                let visible = if app.single_collection {
                    app.visible_columns().contains(&key)
                } else {
                    !app.settings.hidden(app.scope_name()).contains(&key)
                };
                Line::from(Span::styled(
                    format!(
                        "{} [{}] {key}",
                        if index == app.column_picker {
                            "❯"
                        } else {
                            " "
                        },
                        if visible { "x" } else { " " }
                    ),
                    Style::new().fg(if index == app.column_picker {
                        ACCENT
                    } else {
                        TEXT
                    }),
                ))
            })
            .collect(),
        Overlay::Help => (if app.single_collection {
            vec![
                "Keyboard shortcuts",
                "",
                "Ctrl+K            All actions",
                "  Index storage: Memory / Temp / Repository / Custom",
                "  Residency, sync/retry; quit flushes disk changes",
                "Ctrl+I            File catalogue / results",
                "Tab / Shift+Tab   Switch input and results",
                "Filter: Alt+↑↓  Choose suggestion; Enter insert",
                "Enter / Esc       Search, open / return",
                "↑↓ / PgUp PgDn   Move rows or a page",
                "Alt+←→           Scroll columns / adjacent record",
                "Alt+C / Alt+R     Toggle case / regex",
                "Ctrl+O            Show current collection",
                "Ctrl+R / B / S    Recent / saved / save",
                "Alt+S / L        Sort / select query fields",
                "Alt+E / A        Export record / all",
                "Ctrl+P / Alt+T   Preview / theme",
                "",
                "Detail: Enter     Fold or expand",
                "Detail: / n N    Find / next / previous",
                "Detail: y Y      Copy value / record",
                "Detail: r        Raw / structured view",
                "F1 / Ctrl+Q      Help / quit",
            ]
        } else {
            vec![
                "Keyboard shortcuts",
                "",
                "Ctrl+K            All actions",
                "  Index storage: Memory / Temp / Repository / Custom",
                "  Residency, sync/retry; quit flushes disk changes",
                "Ctrl+I            File catalogue / results",
                "Tab / Shift+Tab   Switch input and results",
                "Filter: Alt+↑↓  Choose suggestion; Enter insert",
                "Enter / Esc       Search, open / return",
                "↑↓ / PgUp PgDn   Move rows or a page",
                "Alt+←→           Scroll result columns",
                "Alt+C / Alt+R     Toggle case / regex",
                "Ctrl+O            Choose collection",
                "Ctrl+R / B / S    Recent / saved / save",
                "Alt+S / L        Sort / select columns",
                "Alt+E / A        Export record / all",
                "Ctrl+P / Alt+T   Preview / theme",
                "",
                "Detail: 1 2 3   Fields/links/backlinks",
                "Detail: Tab / Shift+Tab   Next / previous section",
                "Detail: Enter     Fold or follow link",
                "Detail: Alt+←→   Previous/next result",
                "Detail: / n N    Find / next / previous",
                "Detail: y Y      Copy value / record",
                "Detail: r        Raw / structured view",
                "F1 / Ctrl+Q      Help / quit",
            ]
        })
        .into_iter()
        .map(Line::from)
        .collect(),
    };
    let selected = match overlay {
        Overlay::Actions => app.action_picker,
        Overlay::Storage => storage_current_lines(app).len() + app.storage_picker
            + if app.storage_picker >= 5 { 6 } else { 0 },
        Overlay::Collections => app.picker,
        Overlay::Sort => app.sort_picker,
        Overlay::History | Overlay::Saved => app.list_picker,
        Overlay::Columns => app.column_picker,
        Overlay::SaveQuery | Overlay::Export { .. } | Overlay::Help => 0,
    };
    let list_area = if overlay == Overlay::Sort && app.single_collection {
        Rect {
            height: inner.height.saturating_sub(1),
            ..inner
        }
    } else {
        inner
    };
    let start = selected.saturating_sub(list_area.height.saturating_sub(1) as usize);
    frame.render_widget(
        Paragraph::new(
            lines
                .into_iter()
                .skip(start)
                .take(list_area.height as usize)
                .collect::<Vec<_>>(),
        ).wrap(Wrap { trim: false })
        .style(Style::new().bg(PANEL)),
        list_area,
    );
    if overlay == Overlay::Sort && app.single_collection && inner.height > 0 {
        let input_area = Rect {
            y: inner.bottom() - 1,
            height: 1,
            ..inner
        };
        frame.render_widget(
            Paragraph::new(format!("Expression ❯ {}", app.sort_input.text))
                .style(Style::new().fg(ACCENT).bg(PANEL)),
            input_area,
        );
        let cursor = app
            .sort_input
            .text
            .chars()
            .take(app.sort_input.cursor)
            .collect::<String>();
        frame.set_cursor_position(Position::new(
            input_area.x
                + (13 + cursor.width()).min(input_area.width.saturating_sub(1) as usize) as u16,
            input_area.y,
        ));
    }
    if matches!(overlay, Overlay::SaveQuery | Overlay::Export { .. }) && inner.width > 3 {
        let input = if overlay == Overlay::SaveQuery {
            &app.save_input
        } else {
            &app.export_input
        };
        let cursor = input.text.chars().take(input.cursor).collect::<String>();
        frame.set_cursor_position(Position::new(
            inner.x + 2 + cursor.width().min(inner.width.saturating_sub(3) as usize) as u16,
            inner.y + 1,
        ));
    }
}

fn clip(text: &str, width: usize) -> String {
    let safe = safe_terminal_text(text).replace(['\n', '\t'], " ");
    let text = safe.as_str();
    if text.width() <= width {
        return text.to_owned();
    }
    let mut out = String::new();
    for character in text.chars() {
        if out.width() + UnicodeWidthStr::width(character.to_string().as_str()) + 1 > width {
            break;
        }
        out.push(character);
    }
    out.push('…');
    out
}

fn safe_terminal_text(text: &str) -> String {
    text.chars()
        .map(|ch| {
            if ch.is_control() && ch != '\n' && ch != '\t' {
                '�'
            } else {
                ch
            }
        })
        .collect()
}

fn pad_clip(text: &str, width: usize) -> String {
    let clipped = clip(text, width);
    format!(
        "{clipped}{}",
        " ".repeat(width.saturating_sub(clipped.width()))
    )
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
