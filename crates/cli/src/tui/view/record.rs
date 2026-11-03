//! One record: the preview beside the results and the full detail view.

use super::super::app::{App, Hit, status_name};
use super::super::detail;
use super::super::navigation::{BacklinkStatus, DetailSection};
use super::results::status_color;
use super::{Areas, clip, spinner, theme::*};
use objekte_core::RecordRelation;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};
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

/// Field lines: dim names, highlighted when the search matched them.
fn field_lines(hit: &Hit, width: u16, metadata: bool) -> (Vec<Line<'static>>, usize) {
    let columns = detail::columns(
        hit,
        (width as usize * 2 / 5).max(10),
        width as usize,
        metadata,
    );
    let names_width = columns.names_width;
    let lines = columns
        .names
        .iter()
        .zip(&columns.values)
        .zip(&columns.matched)
        .map(|((name, value), matched)| {
            let name_style = if *matched {
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(DIM)
            };
            let marker = if *matched { "▸ " } else { "  " };
            Line::from(vec![
                Span::styled(marker, Style::new().fg(ACCENT)),
                Span::styled(
                    format!(
                        "{name}{}",
                        " ".repeat(names_width.saturating_sub(name.width()))
                    ),
                    name_style,
                ),
                Span::raw("  "),
                Span::styled(value.clone(), Style::new().fg(TEXT)),
            ])
        })
        .collect::<Vec<_>>();
    let count = lines.len();
    (lines, count)
}


/// The selected record beside the results, separated by a thin rule.
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
    let (lines, _) = field_lines(hit, inner.width.saturating_sub(2), false);
    let body = Rect {
        y: inner.y + 2,
        height: inner.height.saturating_sub(2),
        ..inner
    };
    let shown = lines.len().min(body.height as usize);
    let hidden = lines.len() - shown;
    let mut lines = lines
        .into_iter()
        .take(body.height as usize)
        .collect::<Vec<_>>();
    if hidden > 0 && !lines.is_empty() {
        let last = lines.len() - 1;
        lines[last] = Line::styled(
            format!("  … {} more lines · Enter opens the record", hidden + 1),
            Style::new().fg(FAINT),
        );
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
    let (lines, count) = field_lines(hit, area.width.saturating_sub(2).min(120), true);
    let height = area.height as usize;
    let scroll = app.detail_scroll.min(count.saturating_sub(height));
    let visible = lines
        .into_iter()
        .skip(scroll)
        .take(height)
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(visible), area);
    if count > height {
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
