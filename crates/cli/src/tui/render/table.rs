//! Result columns, cell previews, and the visible table.

use super::*;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const CELL_MAX: usize = 25;
const MATCH_MAX: usize = 65;

pub(super) fn clip(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut width = 0;
    let clean = text
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>();
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

pub(super) fn columns(
    hits: &[Hit],
    query_fields: &[String],
    sort_fields: &[String],
) -> Vec<String> {
    let mut fields = BTreeSet::new();
    for hit in hits {
        if let Value::Object(object) = &hit.record().data {
            fields.extend(object.keys().cloned());
        } else if hit.record().raw.is_none() {
            fields.insert("$value".into());
        }
    }
    let mut columns = vec!["@collection".into(), "@id".into(), "@status".into()];
    if hits.iter().any(|hit| matches!(hit, Hit::Text(_))) {
        columns.push("@match".into());
    }
    // Put searched fields first and sort fields next; the remaining JSON
    // fields follow in their stable object-key order.
    for field in query_fields {
        columns.push(field.clone());
        fields.remove(field);
    }
    for field in sort_fields {
        if !columns.contains(field) {
            columns.push(field.clone());
            fields.remove(field);
        }
    }
    columns.extend(fields);
    columns
}

pub(super) fn field(hit: &Hit, name: &str) -> String {
    let record = hit.record();
    match name {
        "@collection" => clip(&record.collection, CELL_MAX),
        "@id" => clip(&record.id, CELL_MAX),
        "@status" => clip(&format!("{:?}", record.state.status), CELL_MAX),
        "@match" => clip(
            &match_preview::first_match(hit).unwrap_or_default(),
            MATCH_MAX,
        ),
        "$value" => preview(Some(&record.data)),
        name => preview(record.data.as_object().and_then(|object| object.get(name))),
    }
}

fn status_color(status: RecordStatus) -> Color {
    match status {
        RecordStatus::Valid => theme::ACCENT,
        RecordStatus::Invalid | RecordStatus::IncorrectJson => theme::ERROR,
        RecordStatus::Pending => theme::WARNING,
        RecordStatus::NoSchema => theme::MUTED,
    }
}

pub(super) fn visible_columns(
    all: &[String],
    hits: &[Hit],
    start: usize,
    width: u16,
) -> Vec<(String, u16)> {
    let available = width.saturating_sub(3) as usize;
    let mut used = 0;
    let mut visible = Vec::new();
    for name in all.iter().skip(start) {
        let maximum = if name == "@match" {
            MATCH_MAX
        } else {
            CELL_MAX
        };
        let natural = hits
            .iter()
            .map(|hit| UnicodeWidthStr::width(field(hit, name).as_str()))
            .chain(std::iter::once(UnicodeWidthStr::width(name.as_str())))
            .max()
            .unwrap_or(8)
            .clamp(8, maximum);
        let remaining = available.saturating_sub(used);
        if !visible.is_empty() && (remaining < 8 || (name != "@match" && natural + 1 > remaining)) {
            break;
        }
        let cells = natural.min(remaining.max(1));
        used += cells + 1;
        visible.push((name.clone(), cells as u16));
    }
    visible
}

pub(super) fn results(frame: &mut Frame, app: &App<'_>, area: Rect) {
    if app.hits.is_empty() {
        let (heading, hint) = if app.searching {
            ("Searching…", "Matching results will appear here")
        } else if app.last_request.is_some() {
            ("No matches", "Try a shorter phrase or another collection")
        } else if app.kind == SearchKind::Text {
            (
                "Search the whole collection",
                "Type a word, fragment, or phrase above",
            )
        } else {
            (
                "Search or filter your JSON",
                "Enter text or a field filter above",
            )
        };
        let heading = if let Some(elapsed_ms) = app.search_elapsed_ms {
            format!("{heading} · {elapsed_ms:.2} ms")
        } else {
            heading.to_owned()
        };
        let height = area.height.min(3);
        let centered = Rect {
            y: area.y + area.height.saturating_sub(height) / 2,
            height,
            ..area
        };
        frame.render_widget(
            Paragraph::new(Text::from(vec![
                Line::styled(
                    heading,
                    Style::default()
                        .fg(theme::TEXT)
                        .add_modifier(Modifier::BOLD),
                ),
                Line::from(""),
                Line::styled(hint, Style::default().fg(theme::MUTED)),
            ]))
            .alignment(Alignment::Center),
            centered,
        );
        return;
    }
    let all = columns(&app.hits, &app.query_fields, &app.sort_fields);
    let start = app.column_offset.min(all.len().saturating_sub(1));
    let visible = visible_columns(&all, &app.hits, start, area.width);
    let widths = visible
        .iter()
        .map(|(_, width)| Constraint::Length(*width))
        .collect::<Vec<_>>();
    let header = Row::new(
        visible
            .iter()
            .map(|(name, _)| Cell::from(clip(name.trim_start_matches('@'), CELL_MAX)))
            .collect::<Vec<_>>(),
    )
    .style(
        Style::default()
            .fg(theme::MUTED)
            .bg(theme::PANEL)
            .add_modifier(Modifier::BOLD),
    );
    let rows = app.hits.iter().enumerate().map(|(index, hit)| {
        Row::new(
            visible
                .iter()
                .map(|(name, width)| {
                    let cell = Cell::from(clip(&field(hit, name), *width as usize));
                    if name == "@status" {
                        cell.style(Style::default().fg(status_color(hit.record().state.status)))
                    } else {
                        cell
                    }
                })
                .collect::<Vec<_>>(),
        )
        .style(
            Style::default()
                .bg(if index % 2 == 0 {
                    theme::BACKGROUND
                } else {
                    theme::ROW_ALT
                })
                .fg(theme::TEXT),
        )
    });
    let range = if app.searching {
        format!(
            "{}–{} · searching…",
            app.offset + 1,
            app.offset + app.hits.len()
        )
    } else {
        let suffix = if app.total_truncated { "+" } else { "" };
        format!(
            "{}–{} of {}{}",
            app.offset + 1,
            app.offset + app.hits.len(),
            app.total.unwrap_or(app.hits.len()),
            suffix,
        )
    };
    let elapsed = app
        .search_elapsed_ms
        .map(|elapsed_ms| format!("  ·  {elapsed_ms:.2} ms"))
        .unwrap_or_default();
    let title = format!(
        " RESULTS  ·  {range}{elapsed}  ·  columns {}–{}/{} ",
        start + 1,
        start + visible.len(),
        all.len()
    );
    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .title(Span::styled(
                    title,
                    Style::default()
                        .fg(theme::SECONDARY)
                        .add_modifier(Modifier::BOLD),
                ))
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme::BORDER)),
        )
        .column_spacing(1)
        .row_highlight_style(
            Style::default()
                .bg(theme::SELECTED)
                .fg(theme::TEXT)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut state = TableState::default();
    if !app.hits.is_empty() {
        state.select(Some(app.selected.min(app.hits.len() - 1)));
    }
    frame.render_stateful_widget(table, area, &mut state);
}
