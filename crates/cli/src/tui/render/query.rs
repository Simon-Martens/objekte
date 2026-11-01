//! Editable search boxes and their focus styling.

use super::*;
use unicode_width::UnicodeWidthChar;

pub(super) fn query_line(frame: &mut Frame, app: &App<'_>, area: Rect) {
    if app.kind == SearchKind::FilterOn {
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
            "Wasser SORT BY preferred_title",
            text,
        );
        return;
    }
    query_box(
        frame,
        &app.input,
        app.focus == Focus::Query,
        " FULL-TEXT SEARCH  ·  Enter to search ",
        "Wasser SORT BY preferred_title",
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
