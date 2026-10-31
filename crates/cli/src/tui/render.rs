//! Responsive query, result table, detail, and export-prompt rendering.

use super::{
    detail,
    state::{App, Focus, Hit, Representation, SearchKind},
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState},
};
use serde_json::Value;
use std::collections::BTreeSet;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const CELL_MAX: usize = 25;


fn clip(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut width = 0;
    let clean = text.replace(['\n', '\r', '\t'], " ");
    let full_width = UnicodeWidthStr::width(clean.as_str());
    let allowance = if full_width > max {
        max.saturating_sub(1)
    } else {
        max
    };
    for ch in clean.chars() {
        let cells = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + cells > allowance {
            break;
        }
        out.push(ch);
        width += cells;
    }
    if full_width > max {
        out.push('…');
    }
    out
}

fn preview(value: Option<&Value>) -> String {
    match value {
        None => "·".into(),
        Some(Value::String(text)) => clip(text, CELL_MAX),
        Some(Value::Object(fields)) => format!("{{{} fields}}", fields.len()),
        Some(Value::Array(items)) => format!("[{} items]", items.len()),
        Some(value) => clip(&value.to_string(), CELL_MAX),
    }
}

fn columns(hits: &[Hit]) -> Vec<String> {
    let mut fields = BTreeSet::new();
    for hit in hits {
        if let Value::Object(object) = &hit.record().data {
            fields.extend(object.keys().cloned());
        } else if hit.record().raw.is_none() {
            fields.insert("$value".into());
        }
    }
    ["@collection".into(), "@id".into(), "@status".into()]
        .into_iter()
        .chain(fields)
        .collect()
}

fn field(hit: &Hit, name: &str) -> String {
    let record = hit.record();
    match name {
        "@collection" => clip(&record.collection, CELL_MAX),
        "@id" => clip(&record.id, CELL_MAX),
        "@status" => clip(&format!("{:?}", record.state.status), CELL_MAX),
        "$value" => preview(Some(&record.data)),
        name => preview(record.data.as_object().and_then(|object| object.get(name))),
    }
}

fn visible_columns(all: &[String], hits: &[Hit], start: usize, width: u16) -> Vec<(String, u16)> {
    let available = width.saturating_sub(3) as usize;
    let mut used = 0;
    let mut visible = Vec::new();
    for name in all.iter().skip(start) {
        let natural = hits
            .iter()
            .map(|hit| UnicodeWidthStr::width(field(hit, name).as_str()))
            .chain(std::iter::once(UnicodeWidthStr::width(name.as_str())))
            .max()
            .unwrap_or(8)
            .clamp(8, CELL_MAX);
        let remaining = available.saturating_sub(used);
        if !visible.is_empty() && natural + 1 > remaining {
            break;
        }
        let cells = natural.min(remaining.max(1));
        used += cells + 1;
        visible.push((name.clone(), cells as u16));
    }
    visible
}

fn query_line(frame: &mut Frame, app: &App<'_>, area: Rect) {
    let title = match app.kind {
        SearchKind::Filter => "Filter query  (Enter: run)",
        SearchKind::Text => "Full-text query  (Enter: run)",
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(if app.focus == Focus::Query {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default()
        });
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let cursor_width = app
        .input
        .text
        .chars()
        .take(app.input.cursor)
        .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0))
        .sum::<usize>();
    let scroll = cursor_width.saturating_sub(inner.width.saturating_sub(1) as usize);
    frame.render_widget(
        Paragraph::new(app.input.text.as_str()).scroll((0, scroll as u16)),
        inner,
    );
    if app.focus == Focus::Query {
        frame.set_cursor_position((inner.x + (cursor_width - scroll) as u16, inner.y));
    }
}

pub(super) fn column_count(app: &App<'_>) -> usize {
    columns(&app.hits).len()
}

fn control(label: String) -> Span<'static> {
    Span::styled(format!(" {label} "), Style::default().fg(Color::Cyan))
}

fn controls(app: &App<'_>) -> Text<'static> {
    let kind = if app.kind == SearchKind::Filter {
        "Filter"
    } else {
        "Full text"
    };
    let first = Line::from(vec![
        Span::raw(" Objekte "),
        control(format!("Mode: {kind} (Tab)")),
        control(format!("Collection: {} (Ctrl+Tab)", app.collection_label())),
    ]);
    let second = if app.kind == SearchKind::Text {
        let representation = if app.representation == Representation::Normalized {
            "normalized"
        } else {
            "full"
        };
        let mut options = vec![
            Span::raw("  "),
            control(format!(
                "Regex: {} (Ctrl+R)",
                if app.regex { "on" } else { "off" }
            )),
            control(format!("Text: {representation} (Ctrl+F)")),
        ];
        if !app.regex {
            options.push(control(format!(
                "Case: {} (Ctrl+E)",
                if app.case_sensitive {
                    "sensitive"
                } else {
                    "insensitive"
                }
            )));
        }
        Line::from(options)
    } else {
        Line::from("")
    };
    Text::from(vec![first, second])
}

fn results(frame: &mut Frame, app: &App<'_>, area: Rect) {
    let all = columns(&app.hits);
    let start = app.column_offset.min(all.len().saturating_sub(1));
    let visible = visible_columns(&all, &app.hits, start, area.width);
    let widths = visible
        .iter()
        .map(|(_, width)| Constraint::Length(*width))
        .collect::<Vec<_>>();
    let header = Row::new(
        visible
            .iter()
            .map(|(name, _)| Cell::from(clip(name, CELL_MAX)))
            .collect::<Vec<_>>(),
    )
    .style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );
    let rows = app.hits.iter().map(|hit| {
        Row::new(
            visible
                .iter()
                .map(|(name, _)| Cell::from(field(hit, name)))
                .collect::<Vec<_>>(),
        )
    });
    let title = format!(
        "Results  page {}  columns {}–{}/{}",
        app.offset / 100 + 1,
        start + 1,
        start + visible.len(),
        all.len()
    );
    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().title(title).borders(Borders::ALL))
        .column_spacing(1)
        .row_highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White))
        .highlight_symbol("› ");
    let mut state = TableState::default();
    if !app.hits.is_empty() {
        state.select(Some(app.selected.min(app.hits.len() - 1)));
    }
    frame.render_stateful_widget(table, area, &mut state);
}

fn detail(frame: &mut Frame, app: &App<'_>, area: Rect) {
    let Some(hit) = app.hits.get(app.selected) else {
        return;
    };
    let left_width = area.width.saturating_sub(4).clamp(12, 34);
    let [left, _, right] = Layout::horizontal([
        Constraint::Length(left_width),
        Constraint::Length(2),
        Constraint::Fill(1),
    ])
    .areas(area);
    let detail::DetailColumns {
        names,
        values,
        matched,
    } = detail::columns(hit, left.width as usize, right.width as usize);
    let offset =
        (app.detail_scroll as usize).min(names.len().saturating_sub(left.height as usize)) as u16;
    frame.render_widget(
        Paragraph::new(Text::from(
            names
                .into_iter()
                .zip(matched)
                .map(|(name, matched)| {
                    Line::styled(
                        name,
                        if matched {
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                        },
                    )
                })
                .collect::<Vec<_>>(),
        ))
        .scroll((offset, 0)),
        left,
    );
    frame.render_widget(
        Paragraph::new(Text::from(
            values.into_iter().map(Line::from).collect::<Vec<_>>(),
        ))
        .scroll((offset, 0)),
        right,
    );
}

fn export_prompt(frame: &mut Frame, app: &App<'_>, area: Rect) {
    let width = area.width.saturating_sub(4).min(76);
    let popup = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + area.height.saturating_sub(5) / 2,
        width,
        height: 5.min(area.height),
    };
    frame.render_widget(Clear, popup);
    let title = if app.focus == Focus::ExportAll {
        "Export all matches to a new JSON file"
    } else {
        "Export selected hit to a new JSON file"
    };
    let block = Block::default().title(title).borders(Borders::ALL);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let cursor_width = app
        .export_input
        .text
        .chars()
        .take(app.export_input.cursor)
        .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0))
        .sum::<usize>();
    let scroll = cursor_width.saturating_sub(inner.width.saturating_sub(1) as usize);
    frame.render_widget(
        Paragraph::new(app.export_input.text.as_str()).scroll((0, scroll as u16)),
        inner,
    );
    frame.set_cursor_position((inner.x + (cursor_width - scroll) as u16, inner.y));
}

pub(super) fn draw(frame: &mut Frame, app: &App<'_>) {
    let [mode, input, status, body, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(2),
    ])
    .areas(frame.area());
    frame.render_widget(Paragraph::new(controls(app)), mode);
    query_line(frame, app, input);
    frame.render_widget(Paragraph::new(app.message.as_str()), status);
    let in_detail = app.focus == Focus::Detail
        || (matches!(app.focus, Focus::ExportAll | Focus::ExportOne)
            && app.export_return_focus == Focus::Detail);
    if in_detail {
        detail(frame, app, body);
    } else {
        results(frame, app, body);
    }
    let help = if in_detail {
        " ←→ or p/n Result  ↑↓ Scroll  PgUp/PgDn Scroll page  e Export hit  Esc Back"
    } else {
        " Tab Mode  Ctrl+Tab Collection  Enter Search/Open  [ ] Collection in results\n ↑↓ Rows  ←→ Columns  n/p Pages  a Export all  e Export hit  / Query  q Quit"
    };
    frame.render_widget(
        Paragraph::new(help).style(Style::default().fg(Color::Gray)),
        footer,
    );
    if matches!(app.focus, Focus::ExportAll | Focus::ExportOne) {
        export_prompt(frame, app, frame.area());
    }
}

#[cfg(test)]
#[path = "render.test.rs"]
mod tests;
