//! Interactive read-only search over a warmed repository cache.

mod detail;
mod export;
mod match_preview;
mod navigation;
mod relations;
mod render;
mod search;
mod state;
mod theme;
mod warm;

use self::{
    navigation::DetailSection,
    state::{App, Focus},
};
use objekte_core::Repository;
use ratatui::crossterm::{
    event::{
        self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
};
use std::error::Error;
use std::{sync::mpsc::TryRecvError, time::Duration};

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
    if key.code == KeyCode::Char('q')
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        app.done = true;
        return;
    }
    if key.code == KeyCode::Char('i')
        && key.modifiers.contains(KeyModifiers::ALT)
        && matches!(app.focus, Focus::Query | Focus::Results)
    {
        app.open_inference();
        return;
    }
    if key.modifiers.contains(KeyModifiers::ALT)
        && matches!(app.focus, Focus::Query | Focus::Results)
        && matches!(key.code, KeyCode::Char('s' | 'S' | 'r' | 'R'))
    {
        if app.index_status.is_some() {
            app.message = "Index work is already running.".into();
        } else {
            let mode = if matches!(key.code, KeyCode::Char('s' | 'S')) {
                warm::WarmMode::Save
            } else {
                warm::WarmMode::Reindex
            };
            let search_again = !app.input.text.trim().is_empty();
            app.invalidate_results();
            app.queued_search = search_again;
            app.pending_cache_action = Some(mode);
            app.message = if mode == warm::WarmMode::Save {
                "Saving bigram caches…".into()
            } else {
                "Reindexing all collections…".into()
            };
        }
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
    if app.focus == Focus::Infer {
        match key.code {
            KeyCode::Esc => app.close_inference(),
            KeyCode::Char('c') => app.infer_all = !app.infer_all,
            KeyCode::Char('a') => app.infer_auto_index = !app.infer_auto_index,
            KeyCode::Enter => {
                app.pending_inference = Some(state::InferRequest {
                    collection: if app.infer_all || app.collection == 0 {
                        None
                    } else {
                        Some(app.collections[app.collection - 1].clone())
                    },
                    auto_index: app.infer_auto_index,
                });
                app.focus = app.infer_return_focus;
                app.message = "Inference queued…".into();
            }
            _ => {}
        }
        return;
    }
    if app.focus == Focus::InferReady {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => app.close_inference(),
            KeyCode::Char('w') if app.pending_schema_write || app.index_status.is_some() => {
                app.message = "Schema write already queued or running.".into();
            }
            KeyCode::Char('w')
                if !app.inference_saved
                    && !app.pending_schema_write
                    && app.index_status.is_none()
                    && app.last_inference.as_ref().is_some_and(|report| {
                        report.collections.iter().any(|collection| {
                            app.repo
                                .model()
                                .collections
                                .get(&collection.collection)
                                .is_some_and(|current| current.schema_path.is_some())
                        })
                    }) =>
            {
                app.pending_schema_write = true;
                app.message = "Schema write queued…".into();
            }
            KeyCode::Char('w') => {
                app.message = "No unsaved inference for an existing schema.".into()
            }
            _ => {}
        }
        return;
    }
    if app.focus == Focus::Detail {
        match key.code {
            KeyCode::Esc => app.close_detail(),
            KeyCode::Backspace => app.leave_detail(),
            KeyCode::Tab | KeyCode::BackTab => app.links.cycle_section(
                key.code == KeyCode::BackTab || key.modifiers.contains(KeyModifiers::SHIFT),
            ),
            KeyCode::Up if app.links.section == DetailSection::Fields => {
                app.detail_scroll = app.detail_scroll.saturating_sub(1)
            }
            KeyCode::Down if app.links.section == DetailSection::Fields => {
                app.detail_scroll = app.detail_scroll.saturating_add(1)
            }
            KeyCode::Up => app.links.move_selection(false),
            KeyCode::Down => app.links.move_selection(true),
            KeyCode::PageUp => app.detail_scroll = app.detail_scroll.saturating_sub(20),
            KeyCode::PageDown => app.detail_scroll = app.detail_scroll.saturating_add(20),
            KeyCode::Home => app.detail_scroll = 0,
            KeyCode::Enter => app.open_selected_relation(),
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
                'i' => {
                    app.open_inference();
                    return;
                }
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
            KeyCode::Esc => {
                app.input.text.clear();
                app.input.cursor = 0;
                app.invalidate_results();
                app.message.clear();
            }
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
        KeyCode::Esc => app.focus = Focus::Query,
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
            app.open_detail();
        }
        KeyCode::Char('n') | KeyCode::PageDown => app.next_page(),
        KeyCode::Char('p') | KeyCode::PageUp => app.previous_page(),
        KeyCode::Char('a') => app.start_export(true),
        KeyCode::Char('e') => app.start_export(false),
        KeyCode::Char('i') => app.open_inference(),
        KeyCode::Char('[') => app.cycle_collection(true),
        KeyCode::Char(']') => app.cycle_collection(false),
        _ => {}
    }
}

fn pending_message(app: &App<'_>, pressed: KeyEvent) -> Option<&'static str> {
    if pressed.kind == KeyEventKind::Release {
        return None;
    }
    match (app.focus, pressed.code) {
        (Focus::Query, KeyCode::Enter) if app.index_status.is_none() => Some("Searching…"),
        (Focus::Results, KeyCode::Char('n') | KeyCode::PageDown) if app.can_next_page() => {
            Some("Loading page…")
        }
        (Focus::Results, KeyCode::Char('p') | KeyCode::PageUp) if app.can_previous_page() => {
            Some("Loading page…")
        }
        (Focus::ExportAll, KeyCode::Enter) => Some("Exporting all matches…"),
        (Focus::ExportOne, KeyCode::Enter) => Some("Exporting selected hit…"),
        _ => None,
    }
}

fn apply_warm_update(app: &mut App<'_>, update: warm::WarmUpdate) {
    match update {
        warm::WarmUpdate::Progress {
            completed,
            total,
            mode,
        } => {
            let action = if mode == warm::WarmMode::Reindex {
                "Reindexing"
            } else if mode == warm::WarmMode::Save {
                "Saving"
            } else {
                "Building"
            };
            app.index_status = Some(format!(
                "{action} indexes · {completed}/{total} collections ready"
            ));
        }
        warm::WarmUpdate::Ready(mut built, mode) => {
            std::mem::swap(app.repo, &mut *built);
            app.index_status = None;
            if app.queued_search {
                app.search();
            } else {
                app.message = match mode {
                    warm::WarmMode::Warm => "Indexes ready. Enter a query to search.",
                    warm::WarmMode::Save => "Bigram caches saved.",
                    warm::WarmMode::Reindex => "All indexes rebuilt and saved.",
                }
                .into();
            }
        }
        warm::WarmUpdate::Failed(error) => {
            app.index_status = None;
            app.queued_search = false;
            app.message = format!("Index build failed: {error}");
        }
    }
}

/// Show the UI after configuration loading while indexes warm in a worker.
pub(super) fn run(repo: &mut Repository) -> Result<(), Box<dyn Error>> {
    let mut terminal = ratatui::try_init()?;
    // Keep ordinary text as UTF-8; encoding every key without its shifted
    // character turns Shift+A and layout-specific symbols into base keys.
    // The support probe waits for a terminal reply and can stall startup for
    // seconds. Unsupported terminals ignore this CSI sequence harmlessly.
    if let Err(error) = execute!(
        std::io::stdout(),
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
        )
    ) {
        let _ = ratatui::try_restore();
        return Err(error.into());
    }
    let result = (|| -> Result<(), Box<dyn Error>> {
        let total = repo.model().collections.len();
        let mut updates = warm::start(repo, warm::WarmMode::Warm);
        let mut app = App::new(repo);
        let mut searches = search::SearchWorker::default();
        app.async_search = true;
        app.index_status = Some(format!("Building indexes · 0/{total} collections ready"));
        while !app.done {
            loop {
                match updates.try_recv() {
                    Ok(update) => apply_warm_update(&mut app, update),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        if app.index_status.is_some() {
                            app.index_status = None;
                            app.queued_search = false;
                            app.message = "Index worker stopped unexpectedly.".into();
                        }
                        break;
                    }
                }
            }
            searches.drive(&mut app);
            terminal.draw(|frame| render::draw(frame, &app))?;
            let poll_ms = if app.searching { 20 } else { 100 };
            if event::poll(Duration::from_millis(poll_ms))?
                && let Event::Key(pressed) = event::read()?
            {
                if let Some(message) = pending_message(&app, pressed) {
                    app.message = message.into();
                    terminal.draw(|frame| render::draw(frame, &app))?;
                }
                key(&mut app, pressed);
                if let Some(mode) = app.pending_cache_action.take() {
                    updates = warm::start(app.repo, mode);
                    app.index_status = Some(if mode == warm::WarmMode::Save {
                        "Saving indexes · 0 collections ready".into()
                    } else {
                        "Reindexing indexes · 0 collections ready".into()
                    });
                }
            }
        }
        Ok(())
    })();
    let pop = execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
    let restore = ratatui::try_restore();
    result?;
    pop?;
    restore?;
    Ok(())
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
