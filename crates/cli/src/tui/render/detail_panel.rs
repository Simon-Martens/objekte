//! Record details and relationship tables.

use super::controls::detail_title;
use super::*;

pub(super) fn render_detail(frame: &mut Frame, app: &App<'_>, area: Rect) {
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
