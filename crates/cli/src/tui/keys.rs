//! Keyboard handling for each screen and dialog.

use super::app::{Action, App, Chooser, Line, Overlay, View};
use super::navigation::DetailSection;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};


pub(super) fn key(app: &mut App<'_>, key: KeyEvent) {
    if key.kind == KeyEventKind::Release {
        return;
    }
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    if control && !alt && matches!(key.code, KeyCode::Char('c' | 'q' | 'Q')) {
        app.done = true;
        return;
    }
    if app.overlay.is_some() {
        overlay(app, key);
        return;
    }
    match key.code {
        KeyCode::F(1) => {
            app.overlay = Some(Overlay::Help);
            return;
        }
        KeyCode::Char('k') if control && !alt => {
            app.completion = None;
            app.overlay = Some(Overlay::Palette(Chooser::default()));
            return;
        }
        KeyCode::Char('o') if control && !alt => {
            app.run_action(Action::ChooseCollection);
            return;
        }
        KeyCode::Char('p') if control && !alt => {
            app.show_preview = !app.show_preview;
            return;
        }
        KeyCode::Char(ch) if alt && !control => {
            match ch.to_ascii_lowercase() {
                'r' => app.toggle_regex(),
                'c' => app.toggle_case(),
                't' => app.toggle_representation(),
                _ => {}
            }
            return;
        }
        _ => {}
    }
    match app.view {
        View::Detail => detail(app, key),
        View::Results => results(app, key),
    }
}


fn results(app: &mut App<'_>, key: KeyEvent) {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    if app.completion.is_some() {
        match key.code {
            KeyCode::Up => {
                app.completion_selected = app.completion_selected.saturating_sub(1);
                return;
            }
            KeyCode::Down => {
                let last = app
                    .completion
                    .as_ref()
                    .map_or(0, |found| found.items.len().saturating_sub(1));
                app.completion_selected = (app.completion_selected + 1).min(last);
                return;
            }
            KeyCode::Tab | KeyCode::Enter => {
                app.accept_completion();
                return;
            }
            KeyCode::Esc => {
                app.completion = None;
                return;
            }
            _ => {}
        }
    }
    match key.code {
        KeyCode::Tab => match app.line {
            Line::Search => app.line = Line::Filter,
            // Tab only ever completes here; Shift+Tab goes back to Search.
            Line::Filter => app.refresh_completion(true),
        },
        KeyCode::BackTab => {
            app.completion = None;
            app.line = match app.line {
                Line::Search => Line::Filter,
                Line::Filter => Line::Search,
            };
        }
        KeyCode::Up => app.move_selection(-1),
        KeyCode::Down => app.move_selection(1),
        KeyCode::PageUp => {
            let rows = app.visible_rows.get().max(2) - 1;
            app.move_selection(-(rows as isize));
        }
        KeyCode::PageDown => {
            let rows = app.visible_rows.get().max(2) - 1;
            app.move_selection(rows as isize);
        }
        KeyCode::Home if control => app.select_first(),
        KeyCode::End if control => app.select_last(),
        KeyCode::Left if alt => app.scroll_columns(false),
        KeyCode::Right if alt => app.scroll_columns(true),
        KeyCode::Enter => {
            if app.dirty.is_some() || app.last_request.is_none() || app.query_error.is_some() {
                app.search();
            } else if !app.hits.is_empty() {
                app.open_detail();
            }
        }
        KeyCode::Esc => {
            if !app.active_input().text.is_empty() {
                app.active_input().clear();
                app.edited();
            } else if !app.search.text.is_empty() || !app.filter.text.is_empty() {
                app.search.clear();
                app.filter.clear();
                app.line = Line::Search;
                app.edited();
            }
        }
        _ => {
            let open = app.completion.is_some();
            match app.active_input().key(key) {
                Some(true) => app.edited(),
                Some(false) if open => app.refresh_completion(false),
                _ => {}
            }
        }
    }
}


fn detail(app: &mut App<'_>, key: KeyEvent) {
    let page = app.visible_rows.get().max(2) as isize - 1;
    match key.code {
        KeyCode::Esc => app.close_detail(),
        KeyCode::Backspace => app.leave_detail(),
        KeyCode::Tab => app.links.cycle_section(false),
        KeyCode::BackTab => app.links.cycle_section(true),
        KeyCode::Char('1') => app.links.section = DetailSection::Fields,
        KeyCode::Char('2') => app.links.section = DetailSection::Links,
        KeyCode::Char('3') => app.links.section = DetailSection::Backlinks,
        KeyCode::Left => app.detail_step(false),
        KeyCode::Right => app.detail_step(true),
        KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown => {
            let delta = match key.code {
                KeyCode::Up => -1,
                KeyCode::Down => 1,
                KeyCode::PageUp => -page,
                _ => page,
            };
            if app.links.section == DetailSection::Fields {
                app.scroll_detail(delta);
            } else {
                for _ in 0..delta.unsigned_abs() {
                    app.links.move_selection(delta > 0);
                }
            }
        }
        KeyCode::Home => app.detail_scroll = 0,
        KeyCode::End => app.detail_scroll = usize::MAX / 2,
        KeyCode::Enter if app.links.section != DetailSection::Fields => {
            app.open_selected_relation();
        }
        _ => {}
    }
}


fn overlay(app: &mut App<'_>, key: KeyEvent) {
    let Some(current) = app.overlay.as_mut() else {
        return;
    };
    match current {
        Overlay::Help => {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Enter | KeyCode::F(1) | KeyCode::Char('q')
            ) {
                app.overlay = None;
            }
        }
        Overlay::Collections(chooser) | Overlay::Palette(chooser) => match key.code {
            KeyCode::Esc => app.overlay = None,
            KeyCode::Up => chooser.selected = chooser.selected.saturating_sub(1),
            KeyCode::Down => chooser.selected += 1,
            KeyCode::Enter => {
                let filter = chooser.input.text.clone();
                let selected = chooser.selected;
                if matches!(current, Overlay::Collections(_)) {
                    let choices = app.collection_choices(&filter);
                    if let Some(&scope) = choices.get(selected.min(choices.len().saturating_sub(1)))
                    {
                        app.overlay = None;
                        app.set_scope(scope);
                    }
                } else {
                    let actions = app.palette_actions(&filter);
                    if let Some(&action) =
                        actions.get(selected.min(actions.len().saturating_sub(1)))
                    {
                        app.run_action(action);
                    }
                }
            }
            _ => {
                if chooser.input.key(key) == Some(true) {
                    chooser.selected = 0;
                }
            }
        },
        Overlay::Export { input, .. } => match key.code {
            KeyCode::Esc => app.overlay = None,
            KeyCode::Enter => app.finish_export(),
            _ => {
                input.key(key);
            }
        },
        Overlay::Infer => match key.code {
            KeyCode::Esc => app.overlay = None,
            KeyCode::Char('c' | 'C') => app.infer_all = !app.infer_all,
            KeyCode::Char('a' | 'A') => app.infer_auto_index = !app.infer_auto_index,
            KeyCode::Enter => app.queue_inference(),
            _ => {}
        },
        Overlay::InferReady => match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                app.overlay = None;
                if app.queued_search {
                    app.search();
                }
            }
            KeyCode::Char('w' | 'W') => {
                let has_schema = app.last_inference.as_ref().is_some_and(|report| {
                    report.collections.iter().any(|collection| {
                        app.repo
                            .model()
                            .collections
                            .get(&collection.collection)
                            .is_some_and(|current| current.schema_path.is_some())
                    })
                });
                if app.pending_schema_write || app.index_status.is_some() {
                    app.notice("Schema write already queued or running");
                } else if !app.inference_saved && has_schema {
                    app.pending_schema_write = true;
                    app.overlay = None;
                    app.notice("Writing inferred rules to schema files…");
                } else {
                    app.notice("Nothing to write");
                }
            }
            _ => {}
        },
    }
}


/// Mouse wheel: scroll the results or the record.
pub(super) fn wheel(app: &mut App<'_>, delta: isize) {
    if app.overlay.is_some() {
        return;
    }
    match app.view {
        View::Results => app.move_selection(delta),
        View::Detail if app.links.section == DetailSection::Fields => app.scroll_detail(delta),
        View::Detail => {}
    }
}


#[cfg(test)]
#[path = "keys.test.rs"]
mod tests;
