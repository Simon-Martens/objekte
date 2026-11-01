//! Responsive query, result table, detail, and export-prompt rendering.

use super::{
    detail, match_preview,
    navigation::{BacklinkStatus, DetailSection},
    state::{App, Focus, Hit, Input, Representation, SearchKind},
    theme,
};
use objekte_core::RecordStatus;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Table, TableState},
};
use serde_json::Value;
use std::collections::BTreeSet;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const CELL_MAX: usize = 25;
const MATCH_MAX: usize = 65;

fn clip(text: &str, max: usize) -> String {
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

fn columns(hits: &[Hit], query_fields: &[String], sort_fields: &[String]) -> Vec<String> {
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

fn field(hit: &Hit, name: &str) -> String {
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

fn visible_columns(all: &[String], hits: &[Hit], start: usize, width: u16) -> Vec<(String, u16)> {
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

fn query_line(frame: &mut Frame, app: &App<'_>, area: Rect) {
    if app.kind == SearchKind::Combined {
        let [filter, text] =
            Layout::vertical([Constraint::Length(3), Constraint::Length(3)]).areas(area);
        query_box(
            frame,
            &app.filter_input,
            app.focus == Focus::FilterInput,
            " JSON FILTER  ·  Ctrl+G switches input ",
            "year >= 1800 SORT BY preferred_title",
            filter,
        );
        query_box(
            frame,
            &app.input,
            app.focus == Focus::Query,
            " FULL-TEXT SEARCH  ·  Enter to search ",
            "Wasser",
            text,
        );
        return;
    }
    let title = match app.kind {
        SearchKind::Filter => " FILTER QUERY  ·  Enter to search ",
        SearchKind::Text => " FULL-TEXT SEARCH  ·  Enter to search ",
        SearchKind::Combined => unreachable!(),
    };
    let placeholder = match app.kind {
        SearchKind::Filter => "year >= 1800 SORT BY preferred_title",
        SearchKind::Text => "Wasser SORT BY preferred_title",
        SearchKind::Combined => unreachable!(),
    };
    query_box(
        frame,
        &app.input,
        app.focus == Focus::Query,
        title,
        placeholder,
        area,
    );
}

fn query_box(
    frame: &mut Frame,
    input: &Input,
    focused: bool,
    title: &str,
    placeholder: &str,
    area: Rect,
) {
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(Style::default().bg(theme::PANEL).fg(theme::TEXT))
        .border_style(if focused {
            Style::default().fg(theme::ACCENT)
        } else {
            Style::default().fg(theme::BORDER)
        });
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let cursor_width = input
        .text
        .chars()
        .take(input.cursor)
        .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0))
        .sum::<usize>();
    let scroll = cursor_width.saturating_sub(inner.width.saturating_sub(1) as usize);
    let line = if input.text.is_empty() {
        Paragraph::new(placeholder).style(Style::default().fg(theme::MUTED).bg(theme::PANEL))
    } else {
        Paragraph::new(input.text.as_str())
            .style(Style::default().fg(theme::TEXT).bg(theme::PANEL))
            .scroll((0, scroll as u16))
    };
    frame.render_widget(line, inner);
    if focused {
        frame.set_cursor_position((inner.x + (cursor_width - scroll) as u16, inner.y));
    }
}

pub(super) fn column_count(app: &App<'_>) -> usize {
    columns(&app.hits, &app.query_fields, &app.sort_fields).len()
}

fn mode_chip(label: &str, selected: bool) -> Span<'static> {
    let style = if selected {
        Style::default()
            .fg(theme::BACKGROUND)
            .bg(theme::ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme::MUTED).bg(theme::PANEL)
    };
    Span::styled(format!(" {label} "), style)
}

fn detail_chip(label: &str, selected: bool, compact: bool) -> Span<'static> {
    let chip = mode_chip(label, selected);
    if compact {
        Span::styled(label.to_owned(), chip.style)
    } else {
        chip
    }
}

fn option(label: String, active: bool) -> Span<'static> {
    Span::styled(
        format!(" {label}  "),
        Style::default()
            .fg(if active { theme::ACCENT } else { theme::MUTED })
            .bg(theme::PANEL),
    )
}

fn controls(app: &App<'_>, width: u16) -> Text<'static> {
    let compact = width < 80;
    let mut first = vec![
        Span::styled(
            if compact {
                " ◆ OBJ  "
            } else {
                " ◆ OBJEKTE  "
            },
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        mode_chip("FTS", app.kind == SearchKind::Text),
        Span::raw(" "),
        mode_chip("FILTER", app.kind == SearchKind::Filter),
        Span::raw(" "),
        mode_chip(
            if width < 60 { "B" } else { "BOTH" },
            app.kind == SearchKind::Combined,
        ),
        Span::styled("  Tab", Style::default().fg(theme::MUTED)),
    ];
    let left_width = first
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum::<usize>();
    let collection_prefix = if compact {
        "COL  "
    } else {
        "Ctrl+Tab  COLLECTION  "
    };
    let prefix_width = UnicodeWidthStr::width(collection_prefix);
    let label = clip(
        app.collection_label(),
        (width as usize).saturating_sub(left_width + prefix_width + 1),
    );
    let right_width = prefix_width + UnicodeWidthStr::width(label.as_str());
    let gap = (width as usize).saturating_sub(left_width + right_width);
    first.push(Span::raw(" ".repeat(gap)));
    first.push(Span::styled(
        collection_prefix,
        Style::default().fg(theme::MUTED),
    ));
    first.push(Span::styled(
        label,
        Style::default()
            .fg(theme::TEXT)
            .add_modifier(Modifier::BOLD),
    ));
    let first = Line::from(first);
    let options_line = if app.kind != SearchKind::Filter {
        let representation = if app.representation == Representation::Normalized {
            "normalized"
        } else {
            "full"
        };
        let regex = if app.regex { "on" } else { "off" };
        let mut options = vec![Span::raw("  ")];
        if width < 52 {
            options.push(option(format!("R:{regex}"), app.regex));
            options.push(option(
                format!(
                    "F:{}",
                    if app.representation == Representation::Normalized {
                        "norm"
                    } else {
                        "full"
                    }
                ),
                true,
            ));
            if !app.regex {
                options.push(option(
                    format!("E:{}", if app.case_sensitive { "exact" } else { "fold" }),
                    app.case_sensitive,
                ));
            }
            return Text::from(vec![first, Line::from(""), Line::from(options)]);
        }
        options.push(option(
            if compact {
                format!("Regex {regex} ^R")
            } else {
                format!("Regex: {regex}  Ctrl+R")
            },
            app.regex,
        ));
        options.push(option(
            if compact {
                format!("{representation} ^F")
            } else {
                format!("Text: {representation}  Ctrl+F")
            },
            true,
        ));
        if !app.regex {
            options.push(option(
                if compact {
                    format!(
                        "{} ^E",
                        if app.case_sensitive {
                            "exact case"
                        } else {
                            "ignore case"
                        }
                    )
                } else {
                    format!(
                        "Case: {}  Ctrl+E",
                        if app.case_sensitive {
                            "sensitive"
                        } else {
                            "insensitive"
                        }
                    )
                },
                app.case_sensitive,
            ));
        }
        Line::from(options)
    } else {
        Line::from("")
    };
    Text::from(vec![first, Line::from(""), options_line])
}

fn detail_header(app: &App<'_>, width: u16) -> Text<'static> {
    let compact = width < 62;
    let links = if compact {
        "LINKS".to_owned()
    } else {
        format!("LINKS ({})", app.links.outgoing.len())
    };
    let backlinks = if compact {
        "BACKLINKS".to_owned()
    } else {
        match app.links.status {
            BacklinkStatus::Loading => format!("BACKLINKS ({}+)", app.links.incoming.len()),
            _ => format!("BACKLINKS ({})", app.links.incoming.len()),
        }
    };
    let mut top = vec![
        Span::styled(
            " ◆ OBJEKTE  ",
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        detail_chip(
            "FIELDS",
            app.links.section == DetailSection::Fields,
            compact,
        ),
        Span::raw(" "),
        detail_chip(&links, app.links.section == DetailSection::Links, compact),
        Span::raw(" "),
        detail_chip(
            &backlinks,
            app.links.section == DetailSection::Backlinks,
            compact,
        ),
    ];
    let used = top
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum::<usize>();
    let back = if width < 45 { "[ESC]" } else { "[ESC] BACK" };
    if used + back.len() < width as usize {
        top.push(Span::raw(" ".repeat(width as usize - used - back.len())));
        top.push(Span::styled(
            back,
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ));
    }
    Text::from(vec![Line::from(top), Line::from("")])
}

fn detail_title(app: &App<'_>, width: u16) -> String {
    let position = if app.searching {
        format!("Record {} · searching…", app.offset + app.selected + 1)
    } else {
        let suffix = if app.total_truncated { "+" } else { "" };
        format!(
            "Record {} of {}{}",
            app.offset + app.selected + 1,
            app.total.unwrap_or(app.offset + app.hits.len()),
            suffix,
        )
    };
    let position = if app.links.override_hit.is_some() {
        format!("Linked · {position}")
    } else {
        position
    };
    let collection = app.current_detail_hit().map_or_else(
        || app.collection_label().to_uppercase(),
        |hit| {
            let record = hit.record();
            if app.links.override_hit.is_some() {
                format!("{}/{}", record.collection.to_uppercase(), record.id)
            } else {
                record.collection.to_uppercase()
            }
        },
    );
    // Reserve room for the record position before clipping the query text.
    // The title's spaces, separators, and "Search: " label occupy 16 cells.
    const TITLE_OVERHEAD: usize = 16;
    let available = (width as usize).saturating_sub(2);
    let position_width = UnicodeWidthStr::width(position.as_str());
    let term_reserve = if app.input.text.is_empty() { 0 } else { 5 };
    let collection = clip(
        &collection,
        available.saturating_sub(position_width + TITLE_OVERHEAD + term_reserve),
    );
    let term = clip(
        &app.input.text,
        available.saturating_sub(
            UnicodeWidthStr::width(collection.as_str()) + position_width + TITLE_OVERHEAD,
        ),
    );
    format!(" {collection} · Search: {term} · {position} ")
}

fn results(frame: &mut Frame, app: &App<'_>, area: Rect) {
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
            ("Explore your JSON", "Enter a field filter above")
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

fn detail(frame: &mut Frame, app: &App<'_>, area: Rect) {
    let block = Block::default()
        .title(Span::styled(
            detail_title(app, area.width),
            Style::default()
                .fg(theme::SECONDARY)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::BORDER));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let area = inner;
    if app.pending_detail.is_some() {
        frame.render_widget(
            Paragraph::new("Loading result page…")
                .style(Style::default().fg(theme::MUTED))
                .alignment(Alignment::Center),
            area,
        );
        return;
    }
    if app.links.section != DetailSection::Fields {
        relation_table(frame, app, area);
        return;
    }
    let Some(hit) = app.current_detail_hit() else {
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
                                .fg(theme::ACCENT)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(theme::MUTED)
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
        .style(Style::default().fg(theme::TEXT))
        .scroll((offset, 0)),
        right,
    );
}

fn relation_table(frame: &mut Frame, app: &App<'_>, area: Rect) {
    let backlinks = app.links.section == DetailSection::Backlinks;
    let edges = if backlinks {
        &app.links.incoming
    } else {
        &app.links.outgoing
    };
    let selected = if backlinks {
        app.links.incoming_selected
    } else {
        app.links.outgoing_selected
    };
    if edges.is_empty() {
        let hint = if backlinks {
            match &app.links.status {
                BacklinkStatus::Loading => "Searching for backlinks…",
                BacklinkStatus::Error(error) => error.as_str(),
                BacklinkStatus::None | BacklinkStatus::Complete => "No backlinks",
            }
        } else {
            "No declared links"
        };
        frame.render_widget(
            Paragraph::new(hint)
                .style(Style::default().fg(theme::MUTED))
                .alignment(Alignment::Center),
            area,
        );
        return;
    }
    let headers = if backlinks {
        ["collection", "id", "field"]
    } else {
        ["field", "collection", "id"]
    };
    let widths = [
        Constraint::Percentage(34),
        Constraint::Percentage(25),
        Constraint::Percentage(41),
    ];
    // Keep rendering proportional to terminal height even for a target with
    // thousands of incoming references.
    let window = area.height.saturating_sub(1).max(1) as usize;
    let start = selected
        .saturating_sub(window / 2)
        .min(edges.len().saturating_sub(window));
    let end = (start + window).min(edges.len());
    let rows = edges[start..end].iter().map(|edge| {
        let values = if backlinks {
            [
                edge.source_collection.as_str(),
                edge.source_id.as_str(),
                edge.location.as_str(),
            ]
        } else {
            [
                edge.location.as_str(),
                edge.target_collection.as_str(),
                edge.target_id.as_str(),
            ]
        };
        Row::new(values.into_iter().map(|value| Cell::from(clip(value, 80))))
            .style(Style::default().fg(theme::TEXT))
    });
    let table = Table::new(rows, widths)
        .header(
            Row::new(headers).style(
                Style::default()
                    .fg(theme::MUTED)
                    .add_modifier(Modifier::BOLD),
            ),
        )
        .column_spacing(1)
        .row_highlight_style(
            Style::default()
                .bg(theme::SELECTED)
                .fg(theme::TEXT)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut state =
        TableState::default().with_selected(Some(selected.min(edges.len() - 1) - start));
    frame.render_stateful_widget(table, area, &mut state);
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
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::ACCENT))
        .style(Style::default().bg(theme::PANEL).fg(theme::TEXT));
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
        Paragraph::new(app.export_input.text.as_str())
            .style(Style::default().fg(theme::TEXT).bg(theme::PANEL))
            .scroll((0, scroll as u16)),
        inner,
    );
    frame.set_cursor_position((inner.x + (cursor_width - scroll) as u16, inner.y));
}

fn key_hints(hints: &[(&str, &str)]) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    for (index, &(key, action)) in hints.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            key.to_owned(),
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ));
        if !action.is_empty() {
            spans.push(Span::styled(
                format!(" {action}"),
                Style::default().fg(theme::MUTED),
            ));
        }
    }
    Line::from(spans)
}

fn infer_prompt(frame: &mut Frame, app: &App<'_>, area: Rect) {
    let width = area.width.saturating_sub(4).min(68);
    let popup = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(7) / 2,
        width,
        height: 7.min(area.height),
    };
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .title("Infer fields")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::ACCENT))
        .style(Style::default().bg(theme::PANEL).fg(theme::TEXT));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let scope = if app.infer_all || app.collection == 0 {
        "all collections".to_owned()
    } else {
        app.collection_label().to_owned()
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(format!("C  Scope: {scope}")),
            Line::from(format!(
                "A  Advanced indexing: {}",
                if app.infer_auto_index { "on" } else { "off" }
            )),
            Line::from(""),
            Line::from("Enter  scan     Esc  cancel"),
        ])
        .style(Style::default().fg(theme::TEXT).bg(theme::PANEL)),
        inner,
    );
}

fn infer_ready_prompt(frame: &mut Frame, app: &App<'_>, area: Rect) {
    let Some(report) = app.last_inference.as_ref() else {
        return;
    };
    let width = area.width.saturating_sub(4).min(72);
    let popup = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(8) / 2,
        width,
        height: 8.min(area.height),
    };
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .title(if app.inference_saved {
            "Schema rules saved"
        } else {
            "Field inference complete"
        })
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::ACCENT))
        .style(Style::default().bg(theme::PANEL).fg(theme::TEXT));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let fields: usize = report
        .collections
        .iter()
        .map(|collection| collection.fields.len())
        .sum();
    let indexed: usize = report
        .collections
        .iter()
        .map(|collection| collection.indexed_paths)
        .sum();
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
    let action = if app.inference_saved {
        "Rules saved. Enter or Esc to return.".to_owned()
    } else if schemas == 0 {
        "No existing schema in this scope. Enter or Esc to return.".to_owned()
    } else {
        format!("W  Write types and index rules to {schemas} existing schemas")
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(format!(
                "{} collections · {fields} fields · {indexed} indexed paths",
                report.collections.len()
            )),
            Line::from(format!("Scan: {:.0} ms", report.elapsed_ms)),
            Line::from(""),
            Line::from(action),
            Line::from("Enter / Esc  return"),
        ])
        .style(Style::default().fg(theme::TEXT).bg(theme::PANEL)),
        inner,
    );
}

fn footer_hints(app: &App<'_>, in_detail: bool, width: u16) -> Line<'static> {
    if in_detail {
        if app.links.section != DetailSection::Fields {
            return if width < 60 {
                key_hints(&[
                    ("Tab", ""),
                    ("↑↓", ""),
                    ("Enter", ""),
                    ("Esc", ""),
                    ("Ctrl+Q", ""),
                ])
            } else {
                key_hints(&[
                    ("Tab", "section"),
                    ("↑↓", "rows"),
                    ("Enter", "open"),
                    ("Esc", "back"),
                    ("Ctrl+Q", "quit"),
                ])
            };
        }
        if width < 60 {
            key_hints(&[
                ("Tab", ""),
                ("←→/p/n", ""),
                ("↑↓", ""),
                ("e", ""),
                ("Esc", ""),
                ("Ctrl+Q", ""),
            ])
        } else {
            key_hints(&[
                ("Tab", "links"),
                ("←→/p/n", "result"),
                ("↑↓", "scroll"),
                ("PgUp/Dn", ""),
                ("e", "export"),
                ("Esc", "back"),
                ("Ctrl+Q", "quit"),
            ])
        }
    } else if matches!(app.focus, Focus::Query | Focus::FilterInput) {
        if width < 60 {
            key_hints(&[("Enter", ""), ("Ctrl+G", ""), ("Alt+S", ""), ("Ctrl+Q", "")])
        } else {
            key_hints(&[
                ("Enter", "search"),
                ("Ctrl+G", "input"),
                ("Alt+I", "infer"),
                ("Alt+S", "save"),
                ("Alt+R", "reindex"),
                ("Ctrl+Q", "quit"),
            ])
        }
    } else if app.hits.is_empty() {
        key_hints(&[
            ("Esc", "input"),
            ("Alt+S", "save"),
            ("Alt+R", "reindex"),
            ("Ctrl+Q", "quit"),
        ])
    } else if width < 60 {
        key_hints(&[
            ("↑↓", ""),
            ("←→", ""),
            ("n/p", ""),
            ("Esc", ""),
            ("↵", ""),
            ("a", ""),
            ("e", ""),
            ("Ctrl+Q", ""),
        ])
    } else if width < 72 {
        key_hints(&[
            ("↑↓", "row"),
            ("←→", "col"),
            ("n/p", ""),
            ("Esc", "input"),
            ("↵", "go"),
            ("Ctrl+Q", "quit"),
        ])
    } else if width < 100 {
        key_hints(&[
            ("↑↓", "rows"),
            ("←→", "cols"),
            ("n/p", "pages"),
            ("Esc", "input"),
            ("Enter", "open"),
            ("a", "all"),
            ("e", "hit"),
            ("Ctrl+Q", "quit"),
        ])
    } else {
        key_hints(&[
            ("↑↓", "rows"),
            ("←→", "cols"),
            ("n/p", "pages"),
            ("Esc", "input"),
            ("Enter", "open"),
            ("a", "all"),
            ("e", "hit"),
            ("i", "infer"),
            ("Alt+S", "save"),
            ("Alt+R", "reindex"),
            ("Ctrl+Q", "quit"),
        ])
    }
}

pub(super) fn draw(frame: &mut Frame, app: &App<'_>) {
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::BACKGROUND)),
        frame.area(),
    );
    let in_detail = app.focus == Focus::Detail
        || (matches!(app.focus, Focus::ExportAll | Focus::ExportOne)
            && app.export_return_focus == Focus::Detail);
    let show_footer = true;
    let [header, input, status, body, footer] = Layout::vertical([
        Constraint::Length(if in_detail { 2 } else { 3 }),
        Constraint::Length(if in_detail {
            0
        } else if app.kind == SearchKind::Combined {
            6
        } else {
            3
        }),
        Constraint::Length(if app.message.is_empty() { 0 } else { 2 }),
        Constraint::Fill(1),
        Constraint::Length(if show_footer { 1 } else { 0 }),
    ])
    .areas(frame.area());
    if in_detail {
        frame.render_widget(
            Paragraph::new(detail_header(app, header.width))
                .style(Style::default().bg(theme::BACKGROUND)),
            header,
        );
    } else {
        frame.render_widget(
            Paragraph::new(controls(app, header.width))
                .style(Style::default().bg(theme::BACKGROUND)),
            header,
        );
        query_line(frame, app, input);
    }
    if !app.message.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" ●  ", Style::default().fg(theme::ACCENT)),
                Span::styled(app.message.as_str(), Style::default().fg(theme::MUTED)),
            ])),
            status,
        );
    }
    if in_detail {
        detail(frame, app, body);
    } else {
        results(frame, app, body);
    }
    if show_footer {
        let line = if let Some(status) = &app.index_status {
            Line::from(vec![
                Span::styled(" ◐  ", Style::default().fg(theme::ACCENT)),
                Span::styled(status.as_str(), Style::default().fg(theme::TEXT)),
            ])
        } else {
            footer_hints(app, in_detail, footer.width)
        };
        frame.render_widget(
            Paragraph::new(line).style(Style::default().bg(theme::PANEL)),
            footer,
        );
    }
    if matches!(app.focus, Focus::ExportAll | Focus::ExportOne) {
        export_prompt(frame, app, frame.area());
    }
    if app.focus == Focus::Infer {
        infer_prompt(frame, app, frame.area());
    }
    if app.focus == Focus::InferReady {
        infer_ready_prompt(frame, app, frame.area());
    }
}

#[cfg(test)]
#[path = "render.test.rs"]
mod tests;
