//! Fixed header, query lines, result list, detail, and small overlays.

mod syntax;
pub(super) mod theme;

use super::app::{App, Focus, Overlay, View};
use super::input::Input;
use objekte_search_core::repository::{Payload, SearchHit};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use theme::*;
use unicode_width::UnicodeWidthStr;

pub(super) fn draw(frame: &mut Frame<'_>, app: &mut App) {
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
    match app.view {
        View::Results => {
            query_line(
                frame,
                "Search",
                &app.search,
                app.focus == Focus::Search,
                search,
                false,
            );
            query_line(
                frame,
                "Filter",
                &app.filter,
                app.focus == Focus::Filter,
                filter,
                true,
            );
            let status = if app.searching {
                "Searching…".to_owned()
            } else if let Some(error) = &app.error {
                format!("Error: {error}")
            } else {
                let sort = app.sort.as_ref().map_or_else(String::new, |sort| {
                    format!(
                        "  ·  {} {}",
                        sort.field,
                        if sort.descending { "↓" } else { "↑" }
                    )
                });
                format!("{} matches  ·  {} loaded{sort}", app.total, app.hits.len())
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
        " Esc back  ·  ↑↓ scroll  ·  Ctrl+Q quit"
    } else {
        " Tab fields  ·  ↑↓ results  ·  Enter open  ·  Ctrl+O scope  ·  F1 help  ·  Ctrl+Q quit"
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
) {
    let [label_area, text_area] =
        Layout::horizontal([Constraint::Length(10), Constraint::Fill(1)]).areas(area);
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
        let spans = input
            .text
            .chars()
            .zip(styles)
            .map(|(character, style)| Span::styled(character.to_string(), style))
            .collect::<Vec<_>>();
        frame.render_widget(Paragraph::new(Line::from(spans)), text_area);
    }
    if active && text_area.width > 0 {
        let prefix = input.text.chars().take(input.cursor).collect::<String>();
        let column = prefix
            .width()
            .min(text_area.width.saturating_sub(1) as usize) as u16;
        frame.set_cursor_position(Position::new(text_area.x + column, text_area.y));
    }
}

fn results(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    app.viewport_rows = area.height.saturating_sub(1).max(1) as usize;
    if area.height == 0 {
        return;
    }
    let compact = area.width < 90;
    let collection_width = if compact { 12usize } else { 16 };
    let id_width = if compact { 16usize } else { 24 };
    let field_width = 16usize;
    let min_summary = 16usize;
    let available =
        (area.width as usize).saturating_sub(2 + collection_width + 2 + id_width + 2 + min_summary);
    let fields = app
        .visible_columns()
        .into_iter()
        .take((available / (field_width + 2)).min(2))
        .collect::<Vec<_>>();
    let mut header = format!("  {:collection_width$}  {:id_width$}  ", "collection", "id");
    for field in &fields {
        header.push_str(&format!("{:field_width$}  ", clip(field, field_width)));
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
        let style = if selected {
            Style::new().bg(SELECTED)
        } else {
            Style::new()
        };
        let summary = record_summary(hit);
        let mut content = format!(
            "{} {:collection_width$}  {:id_width$}  ",
            if selected { "▌" } else { " " },
            clip(&hit.collection, collection_width),
            clip(&hit.record_id, id_width),
        );
        let value = match &hit.payload {
            Some(Payload::Bytes(bytes)) => serde_json::from_slice::<serde_json::Value>(bytes).ok(),
            _ => None,
        };
        for field in &fields {
            let display = value
                .as_ref()
                .and_then(|value| value.get(field))
                .map_or_else(String::new, |value| {
                    value
                        .as_str()
                        .map_or_else(|| value.to_string(), str::to_owned)
                });
            content.push_str(&format!("{:field_width$}  ", clip(&display, field_width)));
        }
        content.push_str(&summary);
        frame.render_widget(
            Paragraph::new(content).style(style.fg(if selected { TEXT } else { DIM })),
            Rect {
                y: area.y + 1 + row as u16,
                height: 1,
                ..area
            },
        );
    }
}

fn record_summary(hit: &SearchHit) -> String {
    let Some(Payload::Bytes(bytes)) = &hit.payload else {
        return String::new();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
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

fn detail(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let Some(hit) = app.selected_hit() else {
        return;
    };
    let title = format!(" {} / {} ", hit.collection, hit.record_id);
    let body = match &hit.payload {
        Some(Payload::Bytes(bytes)) => serde_json::from_slice::<serde_json::Value>(bytes)
            .ok()
            .and_then(|value| serde_json::to_string_pretty(&value).ok())
            .unwrap_or_else(|| String::from_utf8_lossy(bytes).into_owned()),
        Some(Payload::Logical(value)) => format!("{value:?}"),
        None => String::new(),
    };
    let body = format!(
        "File: {}\nBytes: {}..{}\nMatched fields: {}\n\n{}",
        hit.file_path.display(),
        hit.byte_start,
        hit.byte_end,
        hit.matched_fields.join(", "),
        body,
    );
    frame.render_widget(
        Paragraph::new(body)
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(Style::new().fg(ACCENT)),
            )
            .wrap(Wrap { trim: false })
            .scroll((app.detail_scroll, 0)),
        area,
    );
}

fn overlay_view(frame: &mut Frame<'_>, app: &App, overlay: Overlay) {
    let area = frame.area();
    let width = area.width.min(70).saturating_sub(4).max(20);
    let height = match overlay {
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
        Overlay::Help => 18.min(area.height.saturating_sub(2)),
    };
    let panel = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, panel);
    frame.render_widget(
        Block::default()
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
            "Tab / Shift+Tab   Switch input and results",
            "Enter             Search or open selected record",
            "Esc               Return to results",
            "↑ / ↓             Move through results",
            "Ctrl+O            Choose collection",
            "Ctrl+R            Recent queries",
            "Ctrl+B            Saved queries",
            "Ctrl+S            Save this query",
            "Alt+S             Sort current collection",
            "Alt+L             Show or hide columns",
            "Alt+E / Alt+A     Export record / all",
            "Ctrl+P            Toggle preview pane",
            "Alt+T             Switch light / dark theme",
            "F1                Show this help",
            "Ctrl+Q            Quit",
        ]
        .into_iter()
        .map(Line::from)
        .collect(),
    };
    let selected = match overlay {
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

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
