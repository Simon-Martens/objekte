//! Fixed header, query lines, result list, detail, and small overlays.

mod syntax;
pub(super) mod theme;

use super::app::{App, Focus, Overlay, View};
use super::input::Input;
use super::record;
use objekte_search_core::inventory::FileStatus;
use objekte_search_core::repository::SearchHit;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use std::time::Duration;
use theme::*;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(super) fn elapsed_label(elapsed: Duration) -> String {
    if elapsed.as_secs() > 0 {
        format!("{:.1} s", elapsed.as_secs_f64())
    } else {
        format!("{:.3} ms", elapsed.as_secs_f64() * 1_000.0)
    }
}

pub(super) fn draw_loading(frame: &mut Frame<'_>, elapsed: Duration) {
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
        Paragraph::new(format!(
            " Building search index… {}",
            elapsed_label(elapsed)
        ))
        .style(Style::new().fg(DIM)),
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
            Span::styled("  ▾  Ctrl+O", Style::new().fg(FAINT)),
        ])),
        header,
    );
    if header.width >= 58 {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("● ", Style::new().fg(GREEN)),
                Span::styled("indexes ready ", Style::new().fg(DIM)),
            ]))
            .right_aligned(),
            header,
        );
    }
    match app.view {
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
                "Searching…".to_owned()
            } else if let Some(error) = &app.error {
                let timing = app
                    .last_search_duration
                    .map_or_else(String::new, |elapsed| {
                        format!("  ·  search {}", elapsed_label(elapsed))
                    });
                format!("Error: {error}{timing}")
            } else {
                let sort = app.sort.as_ref().map_or_else(String::new, |sort| {
                    format!(
                        "  ·  {} {}",
                        sort.field,
                        if sort.descending { "↓" } else { "↑" }
                    )
                });
                let timing = app
                    .last_search_duration
                    .map_or_else(String::new, |elapsed| {
                        format!("  ·  search {}", elapsed_label(elapsed))
                    });
                format!(
                    "{} matches  ·  {} loaded{sort}{timing}",
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
                let [table, preview] = Layout::horizontal([
                    Constraint::Fill(1),
                    Constraint::Length((body.width * 2 / 5).clamp(40, 72)),
                ])
                .areas(body);
                results(frame, app, table);
                detail(frame, app, preview);
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
    let hint = if app.view == View::Detail {
        " Esc back · ↑↓ fields · Enter fold/open · r raw · Ctrl+Q quit"
    } else if footer.width < 78 {
        " Tab fields · ↑↓ rows · Enter open · Ctrl+K actions · F1 help"
    } else {
        " Tab fields · ↑↓ rows · Enter open · Ctrl+K actions · Alt+←→ columns · F1 help"
    };
    frame.render_widget(
        Paragraph::new(app.status.as_deref().unwrap_or(hint)).style(Style::new().fg(DIM)),
        footer,
    );
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
    let compact = area.width < 100;
    let collection_width = if compact { 12usize } else { 16 };
    let id_width = if compact { 16usize } else { 24 };
    let field_width = if area.width >= 150 { 24 } else { 16 };
    let fixed = 4 + collection_width + 2 + id_width + 2;
    let available = (area.width as usize).saturating_sub(fixed + 16);
    let fields = app
        .visible_columns()
        .into_iter()
        .skip(app.column_offset)
        .take((available / (field_width + 2)).min(4))
        .collect::<Vec<_>>();
    let mut header = format!(
        "    {}  {}  ",
        pad_clip("collection", collection_width),
        pad_clip("id", id_width)
    );
    for field in &fields {
        header.push_str(&pad_clip(field, field_width));
        header.push_str("  ");
    }
    header.push_str("match / record");
    frame.render_widget(
        Paragraph::new(header).style(Style::new().fg(DIM).add_modifier(Modifier::BOLD)),
        Rect { height: 1, ..area },
    );
    if app.hits.is_empty() {
        let message = if app.searching {
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
        ];
        let mut used = fixed;
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
            used += field_width + 2;
        }
        let remaining = (area.width as usize).saturating_sub(used);
        if let Some((path, value)) = summary.split_once(": ") {
            let label = clip(path, (remaining / 3).max(8));
            let rest = remaining.saturating_sub(label.width() + 2);
            spans.push(Span::styled(label, Style::new().fg(FAINT)));
            spans.push(Span::styled(": ", Style::new().fg(FAINT)));
            spans.push(Span::styled(clip(value, rest), Style::new().fg(TEXT)));
        } else {
            spans.push(Span::styled(
                clip(&summary, remaining),
                Style::new().fg(TEXT),
            ));
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

fn detail(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    app.detail_area = area;
    let Some(hit) = app.current_detail_hit().cloned() else {
        return;
    };
    let title = format!(
        " {} / {} [{}] ",
        hit.collection,
        hit.record_id,
        hit.file_status.label()
    );
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::new().fg(ACCENT));
    if app.record.raw {
        frame.render_widget(
            Paragraph::new(record::raw_text(&hit))
                .block(block)
                .wrap(Wrap { trim: false })
                .scroll((app.detail_scroll, 0)),
            area,
        );
        return;
    }
    let diagnostic = app
        .diagnostic(&hit)
        .map(|issue| format!("{}: {}", issue.code, issue.message));
    let mut rows = app.record.rows(&hit, diagnostic.as_deref());
    rows.extend(app.relation_rows.iter().map(|row| record::RecordRow {
        label: row.label.clone(),
        value: row.value.clone(),
        depth: 0,
        matched: row.action.is_some(),
        branch: None,
    }));
    let height = area.height.saturating_sub(2) as usize;
    app.record.cursor = app.record.cursor.min(rows.len().saturating_sub(1));
    if app.record.cursor < app.record.scroll {
        app.record.scroll = app.record.cursor;
    } else if app.record.cursor >= app.record.scroll.saturating_add(height.max(1)) {
        app.record.scroll = app.record.cursor + 1 - height.max(1);
    }
    let name_width = ((area.width as usize) * 2 / 5).clamp(12, 42);
    let lines = rows
        .iter()
        .enumerate()
        .skip(app.record.scroll)
        .take(height)
        .map(|(index, row)| {
            let indent = "  ".repeat(row.depth.min(12));
            let marker = match &row.branch {
                Some((path, array)) if app.record.is_folded(path, *array) => "▸ ",
                Some(_) => "▾ ",
                None => "  ",
            };
            let label = format!(
                "{}{}{}",
                if index == app.record.cursor && app.view == View::Detail {
                    "❯ "
                } else {
                    "  "
                },
                indent,
                marker
            );
            let label = format!("{label}{}", row.label);
            let value_width = (area.width as usize).saturating_sub(name_width + 3);
            let content = format!(
                "{} │ {}",
                pad_clip(&label, name_width),
                clip(&row.value, value_width)
            );
            Line::styled(
                content,
                Style::new().fg(if index == app.record.cursor && app.view == View::Detail {
                    ACCENT
                } else if row.matched {
                    TEXT
                } else {
                    DIM
                }),
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn overlay_view(frame: &mut Frame<'_>, app: &App, overlay: Overlay) {
    let area = frame.area();
    let width = area.width.min(70).saturating_sub(4).max(20);
    let height = match overlay {
        Overlay::Actions => {
            (super::app::Action::ALL.len() as u16 + 3).min(area.height.saturating_sub(2))
        }
        Overlay::Collections => {
            (app.collection_names().count() as u16 + 3).min(area.height.saturating_sub(2))
        }
        Overlay::Sort => (app.sort_choices().len() as u16 + 3).min(area.height.saturating_sub(2)),
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
        Overlay::Help => 21.min(area.height.saturating_sub(2)),
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
            .collect(),
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
                let visible = !app.settings.hidden(app.scope_name()).contains(&key);
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
        Overlay::Help => [
            "Keyboard shortcuts",
            "",
            "Ctrl+K            All actions",
            "Tab / Shift+Tab   Switch input and results",
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
            "Detail: Enter     Fold or follow link",
            "Detail: Backspace Previous record",
            "Detail: r         Raw / structured view",
            "F1 / Ctrl+Q      Help / quit",
        ]
        .into_iter()
        .map(Line::from)
        .collect(),
    };
    let selected = match overlay {
        Overlay::Actions => app.action_picker,
        Overlay::Collections => app.picker,
        Overlay::Sort => app.sort_picker,
        Overlay::History | Overlay::Saved => app.list_picker,
        Overlay::Columns => app.column_picker,
        Overlay::SaveQuery | Overlay::Export { .. } | Overlay::Help => 0,
    };
    let start = selected.saturating_sub(inner.height.saturating_sub(1) as usize);
    frame.render_widget(
        Paragraph::new(
            lines
                .into_iter()
                .skip(start)
                .take(inner.height as usize)
                .collect::<Vec<_>>(),
        )
        .style(Style::new().bg(PANEL)),
        inner,
    );
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
