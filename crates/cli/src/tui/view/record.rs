//! One record: the preview beside the results and the full detail view.

use super::super::app::{App, Hit, status_name};
use super::super::navigation::{BacklinkStatus, DetailSection};
use super::super::record_view::Layout;
use super::results::status_color;
use super::{Areas, clip, marked, spinner, theme::*};
use objekte_core::RecordRelation;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};
use regex::Regex;
use unicode_width::UnicodeWidthStr;


/// Collection, ID, and status of a record.
fn identity(hit: &Hit) -> Vec<Span<'static>> {
    let record = hit.record();
    vec![
        Span::styled(record.collection.clone(), Style::new().fg(VIOLET)),
        Span::styled(" / ", Style::new().fg(FAINT)),
        Span::styled(
            record.id.clone(),
            Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
        ),
        Span::styled("   ● ", Style::new().fg(status_color(record.state.status))),
        Span::styled(status_name(record.state.status), Style::new().fg(DIM)),
    ]
}

/// How to draw field lines.
struct Paint<'r> {
    /// Search and find patterns to mark in values.
    marks: Vec<&'r Regex>,
    /// Shown row under the cursor, in the record view.
    cursor: Option<usize>,
    /// Pointers with validation problems.
    problems: &'r [String],
    /// Pointers that link to other records.
    links: &'r [String],
}


/// Field lines `range` of a laid-out record: dim names, highlighted when
/// the search matched them, values with matches marked.
fn field_lines(
    layout: &Layout,
    range: std::ops::Range<usize>,
    width: u16,
    paint: &Paint<'_>,
) -> Vec<Line<'static>> {
    let columns = &layout.columns;
    let names_width = columns.names_width;
    range
        .filter(|&line| line < columns.names.len())
        .map(|line| {
            let row_index = columns.row[line];
            let row = &layout.rows[row_index];
            let first = line == 0 || columns.row[line - 1] != row_index;
            let chosen = paint.cursor == Some(row_index);
            let base = if chosen {
                Style::new().bg(SELECTED)
            } else {
                Style::new()
            };
            let problem = !row.pointer.is_empty()
                && paint.problems.iter().any(|pointer| {
                    *pointer == row.pointer || pointer.starts_with(&format!("{}/", row.pointer))
                });
            let linked = paint.links.contains(&row.pointer);
            let name_style = if problem {
                base.fg(RED).add_modifier(Modifier::BOLD)
            } else if columns.matched[line] {
                base.fg(ACCENT).add_modifier(Modifier::BOLD)
            } else if chosen {
                base.fg(TEXT).add_modifier(Modifier::BOLD)
            } else {
                base.fg(DIM)
            };
            let marker = match (chosen && first, columns.matched[line]) {
                (true, _) => "▌ ",
                (false, true) => "▸ ",
                _ => "  ",
            };
            let name = &columns.names[line];
            let mut spans = vec![
                Span::styled(marker, base.fg(ACCENT)),
                Span::styled(
                    format!(
                        "{name}{}",
                        " ".repeat(names_width.saturating_sub(name.width()))
                    ),
                    name_style,
                ),
                Span::styled("  ", base),
            ];
            let value = &columns.values[line];
            if first && row.branch {
                let text = match layout.hidden[row_index] {
                    0 => String::new(),
                    hidden => format!("▸ {hidden} folded"),
                };
                spans.push(Span::styled(text, base.fg(FAINT)));
            } else {
                let style = if linked {
                    base.fg(VIOLET)
                } else {
                    base.fg(TEXT)
                };
                spans.extend(marked(value, style, &paint.marks));
                if first && linked {
                    spans.push(Span::styled("  →", base.fg(VIOLET)));
                }
            }
            if chosen {
                let used = spans.iter().map(|span| span.content.width()).sum::<usize>();
                spans.push(Span::styled(
                    " ".repeat((width as usize).saturating_sub(used)),
                    base,
                ));
            }
            Line::from(spans)
        })
        .collect()
}


/// The selected record beside the results, separated by a thin rule. It
/// opens at the first field the search matched.
pub(in super::super) fn preview(frame: &mut Frame<'_>, app: &App<'_>, area: Rect) {
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
    let Some(hit) = app.selected_hit() else {
        return;
    };
    let mut head = identity(hit);
    let fit = head.iter().map(|span| span.content.width()).sum::<usize>();
    if fit > inner.width as usize {
        head.truncate(3);
    }
    frame.render_widget(
        Paragraph::new(Line::from(head)),
        Rect { height: 1, ..inner },
    );
    let width = inner.width.saturating_sub(2);
    let layout = app.record.layout(hit, false, width as usize, false);
    let body = Rect {
        y: inner.y + 2,
        height: inner.height.saturating_sub(2),
        ..inner
    };
    let height = body.height as usize;
    let total = layout.columns.names.len();
    if height == 0 || total == 0 {
        return;
    }
    // Start a little above the first match when it would be hidden.
    let start = match layout.first_match {
        Some(line) if line + 1 >= height => {
            line.saturating_sub(2).min(total.saturating_sub(height - 1))
        }
        _ => 0,
    };
    let paint = Paint {
        marks: app.highlight.iter().collect(),
        cursor: None,
        problems: &[],
        links: &[],
    };
    let mut lines = Vec::new();
    if start > 0 {
        lines.push(Line::styled(
            format!("  … {start} lines above"),
            Style::new().fg(FAINT),
        ));
    }
    let room = height - lines.len();
    let below = total.saturating_sub(start + room);
    let take = if below > 0 { room - 1 } else { room };
    lines.extend(field_lines(&layout, start..start + take, width, &paint));
    if below > 0 {
        lines.push(Line::styled(
            format!("  … {} more lines · Enter opens the record", below + 1),
            Style::new().fg(FAINT),
        ));
    }
    frame.render_widget(Paragraph::new(lines), body);
}


/// The full record, replacing the query rows with its title and tabs.
pub(in super::super) fn detail(frame: &mut Frame<'_>, app: &App<'_>, areas: &Areas) {
    let Some(hit) = app.current_detail_hit() else {
        return;
    };
    // Title row.
    let mut title = vec![Span::styled(" ◂ ", Style::new().fg(ACCENT))];
    title.extend(identity(hit));
    frame.render_widget(Paragraph::new(Line::from(title)), areas.search);
    let position = if app.links.override_hit.is_some() {
        format!("linked record · {} back ", app.history_depth())
    } else {
        match app.total {
            Some(total) => format!(
                "{} of {}{} ",
                app.selected + 1,
                total,
                if app.truncated { "+" } else { "" }
            ),
            None => format!("{} of {}… ", app.selected + 1, app.hits.len()),
        }
    };
    frame.render_widget(
        Paragraph::new(Span::styled(position, Style::new().fg(DIM))).right_aligned(),
        areas.search,
    );
    // Tab row.
    let tab = |label: String, section: DetailSection| {
        if app.links.section == section {
            Span::styled(
                format!(" {label} "),
                Style::new()
                    .fg(BACKGROUND_ON)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(format!(" {label} "), Style::new().fg(DIM))
        }
    };
    let loading = matches!(app.links.status, BacklinkStatus::Loading);
    let backlinks = if loading {
        format!("Backlinks {}{}", app.links.incoming.len(), spinner())
    } else {
        format!("Backlinks {}", app.links.incoming.len())
    };
    app.tabs_area.set(areas.filter);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw("   "),
            tab("1 Fields".into(), DetailSection::Fields),
            Span::raw("  "),
            tab(
                format!("2 Links {}", app.links.outgoing.len()),
                DetailSection::Links,
            ),
            Span::raw("  "),
            tab(format!("3 {backlinks}"), DetailSection::Backlinks),
        ])),
        areas.filter,
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            "─".repeat(areas.summary.width as usize),
            Style::new().fg(FAINT),
        )),
        areas.summary,
    );
    let body = Rect {
        x: areas.body.x + 1,
        width: areas.body.width.saturating_sub(2),
        ..areas.body
    };
    match app.links.section {
        DetailSection::Fields => fields(frame, app, hit, body),
        DetailSection::Links => relations(
            frame,
            &app.links.outgoing,
            app.links.outgoing_selected,
            true,
            "No links declared for this record's fields.",
            body,
        ),
        DetailSection::Backlinks => {
            let empty = match &app.links.status {
                BacklinkStatus::Loading => "Looking for records that link here…".to_string(),
                BacklinkStatus::Error(error) => error.clone(),
                _ => "No records link here.".to_string(),
            };
            relations(
                frame,
                &app.links.incoming,
                app.links.incoming_selected,
                false,
                &empty,
                body,
            );
        }
    }
}

fn fields(frame: &mut Frame<'_>, app: &App<'_>, hit: &Hit, area: Rect) {
    let record = hit.record();
    let find = app.record.find_regex();
    let mut marks = app.highlight.iter().collect::<Vec<_>>();
    marks.extend(find.iter());
    // Validation problems come first, at most a few lines of them.
    let issues = &record.state.issues;
    let mut top = 0;
    if !issues.is_empty() && area.height > 6 {
        let shown = issues.len().min(4);
        let mut lines = vec![Line::from(vec![
            Span::styled(" ✗ ", Style::new().fg(RED).add_modifier(Modifier::BOLD)),
            Span::styled(
                format!(
                    "{} validation {}",
                    issues.len(),
                    if issues.len() == 1 {
                        "problem"
                    } else {
                        "problems"
                    }
                ),
                Style::new().fg(RED).add_modifier(Modifier::BOLD),
            ),
        ])];
        for issue in issues.iter().take(shown) {
            let location = if issue.location.is_empty() {
                "/"
            } else {
                &issue.location
            };
            lines.push(Line::from(vec![
                Span::styled(format!("   {location}  "), Style::new().fg(DIM)),
                Span::styled(issue.message.clone(), Style::new().fg(RED)),
            ]));
        }
        if issues.len() > shown {
            lines.push(Line::styled(
                format!("   … {} more", issues.len() - shown),
                Style::new().fg(FAINT),
            ));
        }
        top = lines.len() as u16 + 1;
        frame.render_widget(
            Paragraph::new(lines),
            Rect {
                height: top - 1,
                ..area
            },
        );
    }
    let area = Rect {
        y: area.y + top,
        height: area.height.saturating_sub(top),
        ..area
    };
    let height = area.height as usize;
    app.record.height.set(height.max(1));
    let width = area.width.saturating_sub(2).min(120);
    app.record_width.set(width as usize);
    if app.record.raw {
        raw(frame, app, hit, area, &marks);
        return;
    }
    app.record_area.set(area);
    let layout = app.record.layout(hit, true, width as usize, true);
    let count = layout.columns.names.len();
    let cursor = app.record.cursor.min(layout.rows.len().saturating_sub(1));
    // Scroll as little as possible to keep the cursor row in view.
    let lines_of = layout.lines_of(cursor);
    let mut scroll = app.record.scroll.get().min(count.saturating_sub(height));
    if lines_of.start < scroll {
        scroll = lines_of.start;
    } else if lines_of.end > scroll + height {
        scroll = lines_of.end.saturating_sub(height).min(lines_of.start);
    }
    app.record.scroll.set(scroll);
    let problems = issues
        .iter()
        .map(|issue| issue.location.clone())
        .collect::<Vec<_>>();
    let links = app
        .links
        .outgoing
        .iter()
        .map(|edge| edge.location.clone())
        .collect::<Vec<_>>();
    let paint = Paint {
        marks,
        cursor: Some(cursor),
        problems: &problems,
        links: &links,
    };
    let visible = field_lines(&layout, scroll..scroll + height, width, &paint);
    frame.render_widget(Paragraph::new(visible), area);
    scrollbar(frame, area, count, height, scroll);
}

fn raw(frame: &mut Frame<'_>, app: &App<'_>, hit: &Hit, area: Rect, marks: &[&Regex]) {
    let lines = app.record.raw_lines(hit);
    let height = area.height as usize;
    let scroll = app
        .record
        .scroll
        .get()
        .min(lines.len().saturating_sub(height));
    app.record.scroll.set(scroll);
    let text = lines
        .iter()
        .skip(scroll)
        .take(height)
        .map(|line| {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(marked(line, Style::new().fg(TEXT), marks));
            Line::from(spans)
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(text), area);
    scrollbar(frame, area, lines.len(), height, scroll);
}

fn scrollbar(frame: &mut Frame<'_>, area: Rect, count: usize, height: usize, scroll: usize) {
    if count <= height {
        return;
    }
    let mut state = ScrollbarState::new(count.saturating_sub(height)).position(scroll);
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("│"))
            .track_style(Style::new().fg(FAINT))
            .thumb_style(Style::new().fg(DIM)),
        Rect {
            x: area.right(),
            width: 1,
            ..area
        },
        &mut state,
    );
}

fn relations(
    frame: &mut Frame<'_>,
    edges: &[RecordRelation],
    selected: usize,
    outgoing: bool,
    empty: &str,
    area: Rect,
) {
    if edges.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(format!("  {empty}"), Style::new().fg(DIM))),
            Rect {
                y: area.y + 1,
                height: 1,
                ..area
            },
        );
        return;
    }
    let field_width = (area.width as usize / 3).clamp(12, 40);
    let header = if outgoing {
        ("field", "links to")
    } else {
        ("linked from", "field")
    };
    let pad = |text: &str, width: usize| {
        let text = clip(text, width);
        let fill = width.saturating_sub(text.width());
        format!("{text}{}", " ".repeat(fill))
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                pad(header.0, field_width),
                Style::new().fg(DIM).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(header.1, Style::new().fg(DIM).add_modifier(Modifier::BOLD)),
        ])),
        Rect { height: 1, ..area },
    );
    let rows = area.height.saturating_sub(1) as usize;
    let start = selected.saturating_sub(rows.saturating_sub(1));
    for (row, (index, edge)) in edges.iter().enumerate().skip(start).take(rows).enumerate() {
        let chosen = index == selected;
        let base = if chosen {
            Style::new().bg(SELECTED)
        } else {
            Style::new()
        };
        let (first, second) = if outgoing {
            (
                edge.location.clone(),
                format!("{} / {}", edge.target_collection, edge.target_id),
            )
        } else {
            (
                format!("{} / {}", edge.source_collection, edge.source_id),
                edge.location.clone(),
            )
        };
        let mut spans = vec![
            Span::styled(if chosen { "▌ " } else { "  " }, base.fg(ACCENT)),
            Span::styled(
                pad(&first, field_width),
                base.fg(if outgoing { DIM } else { TEXT }),
            ),
            Span::styled("  ", base),
            Span::styled(second, base.fg(if outgoing { TEXT } else { DIM })),
        ];
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
