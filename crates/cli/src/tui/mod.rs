//! Interactive read-only search over a warmed repository cache.

mod detail;
mod export;
mod render;
mod state;

use self::state::{App, Focus};
use objekte_core::Repository;
use ratatui::crossterm::{
    event::{
        self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute, terminal,
};
use std::error::Error;


fn key(app: &mut App<'_>, key: KeyEvent) {
    if key.kind == KeyEventKind::Release {
        return;
    }
    if key.code == KeyCode::Char('c')
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::ALT)
    {
        app.done = true;
        return;
    }
    if matches!(app.focus, Focus::ExportAll | Focus::ExportOne) {
        match key.code {
            KeyCode::Esc => app.focus = app.export_return_focus,
            KeyCode::Enter => app.finish_export(),
            _ => {
                app.export_input.key(key);
            }
        }
        return;
    }
    if app.focus == Focus::Detail {
        match key.code {
            KeyCode::Esc | KeyCode::Backspace => app.focus = Focus::Results,
            KeyCode::Up => app.detail_scroll = app.detail_scroll.saturating_sub(1),
            KeyCode::Down => app.detail_scroll = app.detail_scroll.saturating_add(1),
            KeyCode::PageUp => app.detail_scroll = app.detail_scroll.saturating_sub(20),
            KeyCode::PageDown => app.detail_scroll = app.detail_scroll.saturating_add(20),
            KeyCode::Home => app.detail_scroll = 0,
            KeyCode::Left | KeyCode::Char('p') => app.detail_neighbor(false),
            KeyCode::Right | KeyCode::Char('n') => app.detail_neighbor(true),
            KeyCode::Char('e') => app.start_export(false),
            _ => {}
        }
        return;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && !key.modifiers.contains(KeyModifiers::ALT) {
        match key.code {
            KeyCode::Tab | KeyCode::BackTab => {
                app.cycle_collection(
                    key.code == KeyCode::BackTab || key.modifiers.contains(KeyModifiers::SHIFT),
                );
                return;
            }
            KeyCode::Char(ch) => match ch.to_ascii_lowercase() {
                'r' => app.toggle_regex(),
                'f' => app.toggle_representation(),
                'e' => app.toggle_case(),
                _ => return,
            },
            _ => return,
        }
        return;
    }
    if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
        app.toggle_mode();
        return;
    }
    if app.focus == Focus::Query {
        match key.code {
            KeyCode::Enter => {
                app.offset = 0;
                app.search();
            }
            KeyCode::Esc if !app.hits.is_empty() => app.focus = Focus::Results,
            KeyCode::Esc => app.done = true,
            _ => {
                let before = app.input.text.clone();
                app.input.key(key);
                if app.input.text != before {
                    app.invalidate_results();
                    app.message = "Query changed; press Enter to search.".into();
                }
            }
        }
        return;
    }
    match key.code {
        KeyCode::Char('/') => app.focus = Focus::Query,
        KeyCode::Esc | KeyCode::Char('q') => app.done = true,
        KeyCode::Up => app.selected = app.selected.saturating_sub(1),
        KeyCode::Down => {
            app.selected = (app.selected + 1).min(app.hits.len().saturating_sub(1));
        }
        KeyCode::Home => app.selected = 0,
        KeyCode::End => app.selected = app.hits.len().saturating_sub(1),
        KeyCode::Left => app.column_offset = app.column_offset.saturating_sub(1),
        KeyCode::Right => {
            app.column_offset =
                (app.column_offset + 1).min(render::column_count(app).saturating_sub(1));
        }
        KeyCode::Enter if !app.hits.is_empty() => {
            app.detail_scroll = 0;
            app.focus = Focus::Detail;
            app.detail_position();
        }
        KeyCode::Char('n') | KeyCode::PageDown => app.next_page(),
        KeyCode::Char('p') | KeyCode::PageUp => app.previous_page(),
        KeyCode::Char('a') => app.start_export(true),
        KeyCode::Char('e') => app.start_export(false),
        KeyCode::Char('[') => app.cycle_collection(true),
        KeyCode::Char(']') => app.cycle_collection(false),
        _ => {}
    }
}

/// Warm every collection once, then reuse the same repository for all queries.
pub(super) fn run(repo: &mut Repository) -> Result<(), Box<dyn Error>> {
    let mut terminal = ratatui::try_init()?;
    let enhanced = matches!(terminal::supports_keyboard_enhancement(), Ok(true));
    // Keep ordinary text as UTF-8; encoding every key without its shifted
    // character turns Shift+A and layout-specific symbols into base keys.
    if enhanced
        && let Err(error) = execute!(
            std::io::stdout(),
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
            )
        )
    {
        let _ = ratatui::try_restore();
        return Err(error.into());
    }
    let result = (|| -> Result<(), Box<dyn Error>> {
        terminal.draw(|frame| {
            frame.render_widget("Warming the in-memory bigram indexes…", frame.area());
        })?;
        repo.warm_all_bigram_indexes()?;
        let mut app = App::new(repo);
        while !app.done {
            terminal.draw(|frame| render::draw(frame, &app))?;
            if let Event::Key(pressed) = event::read()? {
                let pending = match (app.focus, pressed.code) {
                    (Focus::Query, KeyCode::Enter) => Some("Searching…"),
                    (
                        Focus::Results,
                        KeyCode::Char('n' | 'p') | KeyCode::PageDown | KeyCode::PageUp,
                    ) => Some("Loading page…"),
                    (Focus::ExportAll, KeyCode::Enter) => Some("Exporting all matches…"),
                    (Focus::ExportOne, KeyCode::Enter) => Some("Exporting selected hit…"),
                    _ => None,
                };
                if let Some(message) = pending {
                    app.message = message.into();
                    terminal.draw(|frame| render::draw(frame, &app))?;
                }
                key(&mut app, pressed);
            }
        }
        Ok(())
    })();
    let pop = if enhanced {
        execute!(std::io::stdout(), PopKeyboardEnhancementFlags)
    } else {
        Ok(())
    };
    let restore = ratatui::try_restore();
    result?;
    pop?;
    restore?;
    Ok(())
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
