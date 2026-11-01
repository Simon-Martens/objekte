//! Search mode, options, and detail header text.

use super::*;
use unicode_width::UnicodeWidthStr;

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

pub(super) fn controls(app: &App<'_>, width: u16) -> Text<'static> {
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

pub(super) fn detail_header(app: &App<'_>, width: u16) -> Text<'static> {
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

pub(super) fn detail_title(app: &App<'_>, width: u16) -> String {
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
