//! Floating elements: the completion popup and modal dialogs.

use super::super::app::{App, Chooser, Overlay};
use super::super::complete::Kind;
use super::super::lists::{ListItem, ListKind};
use super::{Areas, LABEL, clip, draw_input, theme::*};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};
use unicode_width::UnicodeWidthStr;


pub(in super::super) fn draw(frame: &mut Frame<'_>, app: &App<'_>, areas: &Areas) {
    if app.overlay.is_none() && app.completion.is_some() {
        completion(frame, app, areas);
    }
    let area = frame.area();
    match &app.overlay {
        None => {}
        Some(Overlay::List(kind, chooser)) => {
            let items = app.list_items(*kind, &chooser.input.text);
            let title = match (kind, &app.breakdown) {
                (ListKind::Breakdown, Some(breakdown)) => format!(
                    "{} · {} records{}",
                    breakdown.field,
                    breakdown.records,
                    if breakdown.truncated { "+" } else { "" }
                ),
                _ => kind.title().to_string(),
            };
            list_dialog(frame, area, &title, *kind, chooser, &items);
        }
        Some(Overlay::SaveQuery(input)) => {
            let inner = dialog(frame, area, "Save this query", 64, 7);
            frame.render_widget(
                Paragraph::new(Span::styled(
                    "Name it to find it again with Ctrl+B. The same name replaces it.",
                    Style::new().fg(DIM).bg(PANEL),
                )),
                Rect { height: 1, ..inner },
            );
            frame.render_widget(
                Paragraph::new(Span::styled("❯ ", Style::new().fg(ACCENT).bg(PANEL))),
                Rect::new(inner.x, inner.y + 2, 2, 1),
            );
            draw_input(
                frame,
                input,
                Rect::new(inner.x + 2, inner.y + 2, inner.width.saturating_sub(2), 1),
                true,
                Style::new().fg(TEXT).bg(PANEL),
            );
            keys_line(frame, inner, &[("Enter", "save"), ("Esc", "cancel")]);
        }
        Some(Overlay::Help) => help(frame, area),
        Some(Overlay::Export { all, input }) => {
            let title = if *all {
                "Export all matches"
            } else {
                "Export this record"
            };
            let inner = dialog(frame, area, title, 72, 7);
            frame.render_widget(
                Paragraph::new(Span::styled(
                    "Write JSON to a new file. Existing files are never overwritten.",
                    Style::new().fg(DIM).bg(PANEL),
                )),
                Rect { height: 1, ..inner },
            );
            frame.render_widget(
                Paragraph::new(Span::styled("❯ ", Style::new().fg(ACCENT).bg(PANEL))),
                Rect::new(inner.x, inner.y + 2, 2, 1),
            );
            draw_input(
                frame,
                input,
                Rect::new(inner.x + 2, inner.y + 2, inner.width.saturating_sub(2), 1),
                true,
                Style::new().fg(TEXT).bg(PANEL),
            );
            keys_line(frame, inner, &[("Enter", "export"), ("Esc", "cancel")]);
        }
        Some(Overlay::Infer) => {
            let inner = dialog(frame, area, "Infer fields", 64, 9);
            let scope = if app.infer_all || app.scope == 0 {
                "all collections".to_string()
            } else {
                app.scope_label().to_string()
            };
            let lines = vec![
                Line::styled(
                    "Scan records for field types and useful index rules.",
                    Style::new().fg(DIM),
                ),
                Line::raw(""),
                option_line("C", "Scope", &scope),
                option_line(
                    "A",
                    "Advanced indexing",
                    if app.infer_auto_index { "on" } else { "off" },
                ),
            ];
            frame.render_widget(Paragraph::new(lines).style(Style::new().bg(PANEL)), inner);
            keys_line(frame, inner, &[("Enter", "scan"), ("Esc", "cancel")]);
        }
        Some(Overlay::InferReady) => infer_ready(frame, app, area),
    }
}

fn option_line(key: &'static str, label: &'static str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!(" {key} "), Style::new().fg(BACKGROUND_ON).bg(DIM)),
        Span::styled(format!("  {label:<20}"), Style::new().fg(DIM)),
        Span::styled(
            value.to_string(),
            Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
        ),
    ])
}

fn infer_ready(frame: &mut Frame<'_>, app: &App<'_>, area: Rect) {
    let Some(report) = app.last_inference.as_ref() else {
        return;
    };
    let title = if app.inference_saved {
        "Schema rules saved"
    } else {
        "Inference complete"
    };
    let inner = dialog(frame, area, title, 68, 9);
    let fields: usize = report.collections.iter().map(|c| c.fields.len()).sum();
    let indexed: usize = report.collections.iter().map(|c| c.indexed_paths).sum();
    let schemas = report
        .collections
        .iter()
        .filter(|collection| {
            app.repo
                .model()
                .collections
                .get(&collection.collection)
                .is_some_and(|current| current.schema_path.is_some())
        })
        .count();
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{}", report.collections.len()),
                Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" collections · ", Style::new().fg(DIM)),
            Span::styled(
                format!("{fields}"),
                Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" fields · ", Style::new().fg(DIM)),
            Span::styled(
                format!("{indexed}"),
                Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" indexed paths", Style::new().fg(DIM)),
        ]),
        Line::styled(
            format!("Scanned in {:.0} ms", report.elapsed_ms),
            Style::new().fg(DIM),
        ),
        Line::raw(""),
    ];
    let mut keys = vec![("Esc", "close")];
    if app.inference_saved {
        lines.push(Line::styled(
            "Types and index rules were written.",
            Style::new().fg(GREEN),
        ));
    } else if schemas == 0 {
        lines.push(Line::styled(
            "No schema files in this scope to update.",
            Style::new().fg(DIM),
        ));
    } else {
        lines.push(Line::from(vec![
            Span::styled("Write types and index rules to ", Style::new().fg(TEXT)),
            Span::styled(
                format!("{schemas}"),
                Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" schema files?", Style::new().fg(TEXT)),
        ]));
        keys.insert(0, ("W", "write"));
    }
    frame.render_widget(Paragraph::new(lines).style(Style::new().bg(PANEL)), inner);
    keys_line(frame, inner, &keys);
}


/// A centred dialog with a title; returns its padded inner area.
fn dialog(frame: &mut Frame<'_>, area: Rect, title: &str, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    let popup = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + (area.height.saturating_sub(height) / 3).max(2),
        width,
        height,
    };
    frame.render_widget(Clear, popup);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(FAINT).bg(PANEL))
        .title(Line::from(vec![Span::styled(
            format!(" {title} "),
            Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
        )]))
        .style(Style::new().bg(PANEL));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    Rect {
        x: inner.x + 1,
        y: inner.y,
        width: inner.width.saturating_sub(2),
        height: inner.height,
    }
}

/// Key hints on the last row of a dialog.
fn keys_line(frame: &mut Frame<'_>, inner: Rect, keys: &[(&str, &str)]) {
    let mut spans = Vec::new();
    for (key, action) in keys {
        spans.push(Span::styled(
            *key,
            Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!(" {action}    "), Style::new().fg(DIM)));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::new().bg(PANEL)),
        Rect {
            y: inner.bottom().saturating_sub(1),
            height: 1,
            ..inner
        },
    );
}

/// A filterable list dialog.
fn list_dialog(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    kind: ListKind,
    chooser: &Chooser,
    items: &[ListItem],
) {
    let rows = items.len().clamp(1, 14) as u16;
    let width = if kind == ListKind::History || kind == ListKind::Saved {
        84
    } else {
        64
    };
    let inner = dialog(frame, area, title, width, rows + 5);
    frame.render_widget(
        Paragraph::new(Span::styled("❯ ", Style::new().fg(ACCENT).bg(PANEL))),
        Rect::new(inner.x, inner.y, 2, 1),
    );
    let input_area = Rect::new(inner.x + 2, inner.y, inner.width.saturating_sub(2), 1);
    if chooser.input.text.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                kind.placeholder(),
                Style::new().fg(FAINT).bg(PANEL),
            )),
            input_area,
        );
        frame.set_cursor_position(Position::new(input_area.x, input_area.y));
    } else {
        draw_input(
            frame,
            &chooser.input,
            input_area,
            true,
            Style::new().fg(TEXT).bg(PANEL),
        );
    }
    frame.render_widget(
        Paragraph::new(Span::styled(
            "─".repeat(inner.width as usize),
            Style::new().fg(FAINT).bg(PANEL),
        )),
        Rect::new(inner.x, inner.y + 1, inner.width, 1),
    );
    let list = Rect::new(inner.x, inner.y + 2, inner.width, rows);
    if items.is_empty() {
        let empty = match kind {
            ListKind::History => "No queries yet. Enter remembers a query.",
            ListKind::Saved => "Nothing saved yet. Ctrl+S saves the current query.",
            _ => "No matches",
        };
        frame.render_widget(
            Paragraph::new(Span::styled(empty, Style::new().fg(DIM).bg(PANEL))),
            list,
        );
    }
    let selected = chooser.selected.min(items.len().saturating_sub(1));
    let start = selected.saturating_sub(rows as usize - 1);
    // Notes form a right-aligned column, so counts and bars line up.
    let note_column = items
        .iter()
        .map(|item| item.note.width())
        .max()
        .unwrap_or(0);
    for (row, (index, item)) in items
        .iter()
        .enumerate()
        .skip(start)
        .take(rows as usize)
        .enumerate()
    {
        let chosen = index == selected;
        let base = if chosen {
            Style::new().bg(SELECTED)
        } else {
            Style::new().bg(PANEL)
        };
        let mut spans = vec![Span::styled(
            if chosen { "▌ " } else { "  " },
            base.fg(ACCENT),
        )];
        if let Some(checked) = item.checked {
            spans.push(Span::styled(
                if checked { "☑ " } else { "☐ " },
                base.fg(if checked { ACCENT } else { DIM }),
            ));
        }
        let bar_width = if item.bar.is_some() { 12 } else { 0 };
        let note_width = note_column;
        let room = (list.width as usize)
            .saturating_sub(4 + note_width + bar_width + 2 * usize::from(item.checked.is_some()));
        let label = clip(&item.label, room);
        let label_style = if chosen {
            base.fg(TEXT).add_modifier(Modifier::BOLD)
        } else if item.checked == Some(false) {
            base.fg(DIM)
        } else {
            base.fg(TEXT)
        };
        spans.push(Span::styled(label.clone(), label_style));
        let used = spans.iter().map(|span| span.content.width()).sum::<usize>();
        let fill = (list.width as usize).saturating_sub(used + note_width + bar_width + 1);
        spans.push(Span::styled(" ".repeat(fill), base));
        if let Some(share) = item.bar {
            let cells = ((share * 10.0).round() as usize).clamp(1, 10);
            spans.push(Span::styled(
                format!("{}{} ", "█".repeat(cells), " ".repeat(10 - cells)),
                base.fg(ACCENT),
            ));
        }
        spans.push(Span::styled(
            format!(
                "{}{} ",
                " ".repeat(note_column.saturating_sub(item.note.width())),
                item.note
            ),
            base.fg(DIM),
        ));
        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(list.x, list.y + row as u16, list.width, 1),
        );
    }
    keys_line(frame, inner, kind.keys());
}


fn kind_icon(kind: Kind) -> (&'static str, ratatui::style::Color) {
    match kind {
        Kind::Field => ("◇", ACCENT),
        Kind::Relation => ("→", VIOLET),
        Kind::System => ("@", TEAL),
        Kind::Macro => ("◷", AMBER),
        Kind::Function => ("ƒ", GREEN),
        Kind::Keyword => ("⋯", DIM),
        Kind::Value => ("•", GREEN),
        Kind::Operator => ("·", DIM),
    }
}

/// Suggestions below the filter line, starting under the completed token.
fn completion(frame: &mut Frame<'_>, app: &App<'_>, areas: &Areas) {
    let Some(found) = &app.completion else {
        return;
    };
    let area = frame.area();
    let rows = found.items.len().min(10) as u16;
    let label_width = found
        .items
        .iter()
        .map(|item| item.label.width())
        .max()
        .unwrap_or(0)
        .clamp(8, 36);
    let detail_width = found
        .items
        .iter()
        .map(|item| item.detail.width())
        .max()
        .unwrap_or(0)
        .min(30);
    let width = (label_width + detail_width + 9) as u16;
    let typed_before = app
        .filter
        .text
        .chars()
        .take(found.start)
        .collect::<String>()
        .width() as u16;
    let x = (areas.filter.x + LABEL + typed_before)
        .saturating_sub(3)
        .min(area.right().saturating_sub(width));
    let popup = Rect::new(x, areas.filter.y + 1, width.min(area.width), rows + 2);
    frame.render_widget(Clear, popup);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(FAINT).bg(PANEL))
        .style(Style::new().bg(PANEL));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let selected = app
        .completion_selected
        .min(found.items.len().saturating_sub(1));
    let start = selected.saturating_sub(rows as usize - 1);
    for (row, (index, item)) in found
        .items
        .iter()
        .enumerate()
        .skip(start)
        .take(rows as usize)
        .enumerate()
    {
        let chosen = index == selected;
        let base = if chosen {
            Style::new().bg(SELECTED)
        } else {
            Style::new().bg(PANEL)
        };
        let (icon, color) = kind_icon(item.kind);
        let label = clip(&item.label, label_width);
        let fill = label_width.saturating_sub(label.width());
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!(" {icon} "), base.fg(color)),
                Span::styled(
                    label,
                    if chosen {
                        base.fg(TEXT).add_modifier(Modifier::BOLD)
                    } else {
                        base.fg(TEXT)
                    },
                ),
                Span::styled(" ".repeat(fill + 2), base),
                Span::styled(clip(&item.detail, detail_width), base.fg(DIM)),
                Span::styled(" ".repeat(inner.width as usize), base),
            ])),
            Rect::new(inner.x, inner.y + row as u16, inner.width, 1),
        );
    }
    if found.items.len() > rows as usize {
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!(" {}/{} ", selected + 1, found.items.len()),
                Style::new().fg(DIM).bg(PANEL),
            ))
            .right_aligned(),
            Rect::new(
                popup.x,
                popup.bottom().saturating_sub(1),
                popup.width.saturating_sub(1),
                1,
            ),
        );
    }
}


type Section = (&'static str, &'static [(&'static str, &'static str)]);

const HELP_LEFT: [Section; 3] = [
    (
        "Search",
        &[
            ("type", "search as you type"),
            ("Enter", "search now; then open the record"),
            ("Tab", "to Filter; there, insert a suggestion"),
            ("Shift+Tab", "switch Search and Filter"),
            ("↑↓  PgUp PgDn", "move; more rows load as you go"),
            ("Ctrl+U  Ctrl+D", "page up / down"),
            ("Ctrl+Home  End", "first / last result"),
            ("Alt+←→", "scroll columns"),
            ("mouse", "click selects, double click opens"),
            ("Esc", "close suggestions, clear the line"),
        ],
    ),
    (
        "Queries and results",
        &[
            ("Ctrl+R", "recent queries"),
            ("Ctrl+S  Ctrl+B", "save query / saved queries"),
            ("Alt+S", "sort by a column"),
            ("Alt+B", "count values of a field"),
            ("Ctrl+O", "choose the collection"),
            ("F5", "check files for changes"),
            ("Alt+E", "open the record in $EDITOR"),
        ],
    ),
    (
        "Options",
        &[
            ("Alt+C Alt+R Alt+T", "case · regex · full text"),
            ("Ctrl+P", "show or hide the preview"),
            ("Ctrl+K", "all actions, columns, colours"),
            ("Ctrl+Q", "quit"),
        ],
    ),
];

const HELP_RIGHT: [Section; 2] = [
    (
        "Record",
        &[
            ("1 2 3  Tab", "fields · links · backlinks"),
            ("←→", "previous / next result"),
            ("↑↓  PgUp PgDn", "move between fields"),
            ("Enter", "follow a link, fold a branch"),
            ("Space", "fold or unfold"),
            ("f", "filter by this value"),
            ("b", "count values of this field"),
            ("y  Y", "copy value / whole record"),
            ("/  n  N", "find in the record"),
            ("r", "raw JSON"),
            ("e", "open in $EDITOR"),
            ("Backspace", "back to the previous record"),
            ("Esc", "back to the results"),
        ],
    ),
    (
        "Editing a line",
        &[
            ("Ctrl+A  Ctrl+E", "start / end of line"),
            ("Ctrl+W", "delete a word"),
            ("Ctrl+←→", "move by word"),
        ],
    ),
];

fn help_lines(sections: &[Section], key_width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (section, keys) in sections {
        lines.push(Line::styled(
            *section,
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
        for (key, text) in *keys {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {key:<key_width$}"),
                    Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(*text, Style::new().fg(DIM)),
            ]));
        }
        lines.push(Line::raw(""));
    }
    lines
}

fn help(frame: &mut Frame<'_>, area: Rect) {
    let left = help_lines(&HELP_LEFT, 19);
    let right = help_lines(&HELP_RIGHT, 15);
    let wide = area.width >= 110;
    let rows = if wide {
        left.len().max(right.len())
    } else {
        left.len() + right.len()
    } as u16;
    let inner = dialog(
        frame,
        area,
        "Keyboard",
        if wide { 108 } else { 60 },
        rows + 3,
    );
    let body = Rect {
        height: inner.height.saturating_sub(1),
        ..inner
    };
    if wide {
        let [first, second] =
            Layout::horizontal([Constraint::Length(56), Constraint::Fill(1)]).areas(body);
        frame.render_widget(Paragraph::new(left).style(Style::new().bg(PANEL)), first);
        frame.render_widget(Paragraph::new(right).style(Style::new().bg(PANEL)), second);
    } else {
        let mut lines = left;
        lines.extend(right);
        frame.render_widget(Paragraph::new(lines).style(Style::new().bg(PANEL)), body);
    }
    keys_line(frame, inner, &[("Esc", "close")]);
}
