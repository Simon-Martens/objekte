//! Inference progress and post-scan dialogs.

use super::*;

pub(super) fn infer_prompt(frame: &mut Frame, app: &App<'_>, area: Rect) {
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

pub(super) fn infer_ready_prompt(frame: &mut Frame, app: &App<'_>, area: Rect) {
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
