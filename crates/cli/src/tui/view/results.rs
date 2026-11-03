//! The result list: fixed columns, a reserved selection marker, and a
//! viewport that only moves when the selection leaves it.

use super::super::app::{App, Hit};
use super::super::match_preview;
use super::{clip, marked, theme::*};
use objekte_core::RecordStatus;
use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

/// Space between columns.
const GAP: u16 = 2;


/// The display text of one cell.
pub(in super::super) fn cell_text(hit: &Hit, key: &str) -> String {
    let record = hit.record();
    match key {
        "@collection" => record.collection.clone(),
        "@id" => record.id.clone(),
        "@status" => String::from("●"),
        "@match" => match_preview::first_match(hit).unwrap_or_default(),
        "$value" => value_text(&record.data),
        key => match &record.data {
            Value::Object(map) => map.get(key).map_or_else(String::new, value_text),
            _ => String::new(),
        },
    }
}

fn value_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.split_whitespace().collect::<Vec<_>>().join(" "),
        Value::Array(items) if items.iter().all(Value::is_string) => items
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", "),
        Value::Array(items) => format!("[{}]", items.len()),
        Value::Object(map) => format!("{{{} fields}}", map.len()),
        other => other.to_string(),
    }
}

pub(in super::super) fn status_color(status: RecordStatus) -> ratatui::style::Color {
    match status {
        RecordStatus::Valid => GREEN,
        RecordStatus::Invalid | RecordStatus::IncorrectJson => RED,
        RecordStatus::Pending => AMBER,
        RecordStatus::NoSchema => FAINT,
    }
}


/// The columns that fit, starting with the fixed identity columns.
fn visible_columns(app: &App<'_>, width: u16) -> Vec<(usize, u16)> {
    let fixed = app.fixed_columns();
    let hidden = app.settings.hidden(app.scope_name());
    let order = (0..fixed)
        .chain(fixed + app.column_offset..app.columns.len())
        .filter(|&index| !hidden.contains(&app.columns[index].key));
    let mut used = 2;
    let mut out = Vec::new();
    for index in order {
        let column = &app.columns[index];
        let need = column.width + GAP;
        if used + need > width {
            // Give the last column whatever room is left.
            let rest = width.saturating_sub(used + GAP);
            if rest >= 6 && index >= fixed {
                out.push((index, rest));
            }
            break;
        }
        used += need;
        out.push((index, column.width));
    }
    out
}


pub(in super::super) fn table(frame: &mut Frame<'_>, app: &App<'_>, area: Rect) {
    let rows = area.height.saturating_sub(1) as usize;
    app.visible_rows.set(rows.max(1));
    app.table_area.set(area);
    let columns = visible_columns(app, area.width);
    // Header row.
    let mut header = vec![Span::raw("  ")];
    for (index, width) in &columns {
        let key = &app.columns[*index].key;
        let label = if key == "@status" {
            String::from(" ")
        } else {
            key.trim_start_matches('@').to_string()
        };
        header.push(Span::styled(
            pad(&clip(&label, *width as usize), *width as usize),
            Style::new().fg(DIM).add_modifier(Modifier::BOLD),
        ));
        header.push(Span::raw(" ".repeat(GAP as usize)));
    }
    frame.render_widget(
        Paragraph::new(Line::from(header)),
        Rect { height: 1, ..area },
    );
    for (row, index) in (app.scroll..app.hits.len()).take(rows).enumerate() {
        let hit = &app.hits[index];
        let selected = index == app.selected;
        let base = if selected {
            Style::new().bg(SELECTED)
        } else {
            Style::new()
        };
        let mut spans = vec![if selected {
            Span::styled("▌ ", base.fg(ACCENT))
        } else {
            Span::styled("  ", base)
        }];
        for (column, width) in &columns {
            let key = app.columns[*column].key.as_str();
            spans.extend(cell(
                hit,
                key,
                *width as usize,
                base,
                selected,
                app.highlight.as_ref(),
            ));
            spans.push(Span::styled(" ".repeat(GAP as usize), base));
        }
        let used = spans.iter().map(|span| span.content.width()).sum::<usize>();
        spans.push(Span::styled(
            " ".repeat((area.width as usize).saturating_sub(used)),
            base,
        ));
        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect {
                y: area.y + 1 + row as u16,
                height: 1,
                ..area
            },
        );
    }
}

fn cell(
    hit: &Hit,
    key: &str,
    width: usize,
    base: Style,
    selected: bool,
    highlight: Option<&regex::Regex>,
) -> Vec<Span<'static>> {
    let marks = highlight.into_iter().collect::<Vec<_>>();
    let record = hit.record();
    let text_style = if selected {
        base.fg(TEXT).add_modifier(Modifier::BOLD)
    } else {
        base.fg(TEXT)
    };
    match key {
        "@status" => vec![Span::styled(
            "●",
            base.fg(status_color(record.state.status)),
        )],
        "@collection" => vec![Span::styled(
            pad(&clip(&record.collection, width), width),
            base.fg(VIOLET),
        )],
        "@id" => vec![Span::styled(
            pad(&clip(&record.id, width), width),
            base.fg(DIM),
        )],
        "@match" => {
            let text = cell_text(hit, key);
            match text.split_once(": ") {
                Some((path, value)) => {
                    let path = clip(path, (width / 3).max(8));
                    let rest = width.saturating_sub(path.width() + 2);
                    let value = clip(
                        &value.split_whitespace().collect::<Vec<_>>().join(" "),
                        rest,
                    );
                    let used = path.width() + 2 + value.width();
                    let mut spans = vec![
                        Span::styled(path, base.fg(FAINT)),
                        Span::styled(": ", base.fg(FAINT)),
                    ];
                    spans.extend(marked(&value, text_style, &marks));
                    spans.push(Span::styled(" ".repeat(width.saturating_sub(used)), base));
                    spans
                }
                None => marked(&pad(&clip(&text, width), width), text_style, &marks),
            }
        }
        key => {
            let text = cell_text(hit, key);
            let style = if text.starts_with('[') || text.starts_with('{') {
                base.fg(DIM)
            } else {
                text_style
            };
            marked(&pad(&clip(&text, width), width), style, &marks)
        }
    }
}

fn pad(text: &str, width: usize) -> String {
    format!("{text}{}", " ".repeat(width.saturating_sub(text.width())))
}


/// The body before a search, while searching, or without matches.
pub(in super::super) fn empty(frame: &mut Frame<'_>, app: &App<'_>, area: Rect) {
    let (title, lines): (&str, Vec<&str>) = if app.searching {
        ("Searching…", vec!["Matches appear here as they are found."])
    } else if app.last_request.is_some() {
        (
            "No matches",
            vec![
                "Try fewer words, another collection (Ctrl+O),",
                "or check the filter for typos.",
            ],
        )
    } else if app.query_error.is_some() && app.error_confirmed {
        ("Fix the query to search", vec!["The error is shown above."])
    } else {
        (
            "What are you looking for?",
            vec![
                "Type in Search to find words in any field.",
                "Use Filter for fields:   year >= 1800 && pseudonym = false",
                "Press Tab in Filter to complete field names from the schema.",
                "",
                "Ctrl+K opens all actions · F1 lists every key",
            ],
        )
    };
    let top = area.y + area.height.saturating_sub(lines.len() as u16 + 2) / 3;
    let mut text = vec![
        Line::styled(title, Style::new().fg(TEXT).add_modifier(Modifier::BOLD)),
        Line::raw(""),
    ];
    text.extend(
        lines
            .into_iter()
            .map(|line| Line::styled(line, Style::new().fg(DIM))),
    );
    let height = text.len() as u16;
    frame.render_widget(
        Paragraph::new(text).alignment(Alignment::Center),
        Rect {
            y: top,
            height: height.min(area.bottom().saturating_sub(top)),
            ..area
        },
    );
}
