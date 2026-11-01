//! Responsive query, result table, detail, and export-prompt rendering.
//! `draw` owns the frame layout; child modules draw each region.

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
mod controls;
mod detail_panel;
mod export_prompt;
mod hints;
mod inference_prompt;
mod query;
mod table;

use controls::{controls, detail_header};
use detail_panel::render_detail;
use export_prompt::export_prompt;
use hints::footer_hints;
use inference_prompt::{infer_prompt, infer_ready_prompt};
use query::query_line;
use table::{clip, columns, results};

/// Number of result columns before horizontal clipping.
pub(super) fn column_count(app: &App<'_>) -> usize {
    columns(&app.hits, &app.query_fields, &app.sort_fields).len()
}

/// Compose one terminal frame from the current application state.
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
        } else if app.kind == SearchKind::FilterOn {
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
        render_detail(frame, app, body);
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
