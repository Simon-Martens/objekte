//! Context-sensitive footer shortcuts.

use super::*;

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

pub(super) fn footer_hints(app: &App<'_>, in_detail: bool, width: u16) -> Line<'static> {
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
