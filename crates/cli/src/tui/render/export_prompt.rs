//! Export destination dialog and its cursor position.

use super::*;
use unicode_width::UnicodeWidthChar;

pub(super) fn export_prompt(frame: &mut Frame, app: &App<'_>, area: Rect) {
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
