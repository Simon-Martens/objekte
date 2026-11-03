//! Keyboard handling for each screen and dialog.

use super::app::{Action, App, Line, Overlay, View};
use super::input::Input;
use super::lists::{Deferred, ListKind};
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
        KeyCode::F(5) => {
            app.run_action(Action::RefreshFiles);
            return;
        }
        KeyCode::Char('k') if control && !alt => {
            app.open_list(ListKind::Palette);
            return;
        }
        KeyCode::Char('r') if control && !alt => {
            app.run_action(Action::History);
            return;
        }
        KeyCode::Char('b') if control && !alt => {
            app.run_action(Action::SavedQueries);
            return;
        }
        KeyCode::Char('s') if control && !alt => {
            app.run_action(Action::SaveQuery);
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
                'e' => app.edit_record(),
                's' => app.run_action(Action::SortBy),
                'b' => app.run_action(Action::Breakdown),
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
        // Vim-style paging; these replace line editing's Ctrl+U here.
        KeyCode::Char('u') if control => {
            let rows = app.visible_rows.get().max(2) - 1;
            app.move_selection(-(rows as isize));
        }
        KeyCode::Char('d') if control => {
            let rows = app.visible_rows.get().max(2) - 1;
            app.move_selection(rows as isize);
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
            let stale =
                app.dirty.is_some() || app.last_request.is_none() || app.query_error.is_some();
            if stale {
                app.search();
            }
            if app.query_error.is_none() && app.last_request.is_some() {
                app.remember_query();
            }
            if !stale && !app.hits.is_empty() && app.enter_opens(std::time::Instant::now()) {
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


fn detail(app: &mut App<'_>, mut key: KeyEvent) {
    // Typing the find text.
    if let Some(input) = app.record.find_input.as_mut() {
        match key.code {
            KeyCode::Esc => app.record.find_input = None,
            KeyCode::Enter => {
                let text = input.text.clone();
                app.record.find_input = None;
                app.record.find = text;
                if !app.record.find.is_empty() {
                    app.record_find_next(true);
                }
            }
            _ => {
                input.key(key);
            }
        }
        return;
    }
    let fields = app.links.section == DetailSection::Fields;
    let page = if fields {
        app.record.height.get().max(2) as isize - 1
    } else {
        app.visible_rows.get().max(2) as isize - 1
    };
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        key.code = match key.code {
            KeyCode::Char('u') => KeyCode::PageUp,
            KeyCode::Char('d') => KeyCode::PageDown,
            KeyCode::Home => KeyCode::Home,
            KeyCode::End => KeyCode::End,
            _ => return,
        };
    }
    match key.code {
        KeyCode::Esc if !app.record.find.is_empty() => app.record.find.clear(),
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
            if fields {
                app.record_move(delta);
            } else {
                for _ in 0..delta.unsigned_abs() {
                    app.links.move_selection(delta > 0);
                }
            }
        }
        KeyCode::Home if fields => app.record_home(false),
        KeyCode::End if fields => app.record_home(true),
        KeyCode::Enter if fields => app.record_enter(),
        KeyCode::Enter => app.open_selected_relation(),
        KeyCode::Char(' ') if fields && !app.record.raw => app.record_toggle(),
        KeyCode::Char('f') if fields && !app.record.raw => app.record_filter_by_value(),
        KeyCode::Char('y') if fields => {
            let text = if app.record.raw {
                app.record_copy_text(true)
            } else {
                app.record_copy_text(false)
            };
            app.copy(text, "value");
        }
        KeyCode::Char('Y') => {
            let text = app.record_copy_text(true);
            app.copy(text, "record");
        }
        KeyCode::Char('r') if fields => {
            app.record.raw = !app.record.raw;
            app.record.scroll.set(0);
        }
        KeyCode::Char('/') if fields => {
            app.record.find_input = Some(Input::with_text(&app.record.find));
        }
        KeyCode::Char('n') if fields => app.record_find_next(true),
        KeyCode::Char('N') if fields => app.record_find_next(false),
        KeyCode::Char('e') => app.edit_record(),
        KeyCode::Char('b') if fields && !app.record.raw => app.record_breakdown(),
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
        Overlay::List(kind, chooser) => {
            let kind = *kind;
            let filter = chooser.input.text.clone();
            let items = app.list_items(kind, &filter);
            let Some(Overlay::List(_, chooser)) = app.overlay.as_mut() else {
                return;
            };
            let last = items.len().saturating_sub(1);
            chooser.selected = chooser.selected.min(last);
            let selected = items.get(chooser.selected).map(|item| item.value.clone());
            match key.code {
                KeyCode::Esc => app.overlay = None,
                KeyCode::Up => chooser.selected = chooser.selected.saturating_sub(1),
                KeyCode::Down => chooser.selected = (chooser.selected + 1).min(last),
                KeyCode::PageUp => chooser.selected = chooser.selected.saturating_sub(10),
                KeyCode::PageDown => chooser.selected = (chooser.selected + 10).min(last),
                KeyCode::Char(' ') if kind == ListKind::Columns => {
                    if let Some(value) = selected {
                        app.list_accept(kind, value);
                    }
                }
                KeyCode::Enter => {
                    if let Some(value) = selected {
                        app.list_accept(kind, value);
                    }
                }
                KeyCode::Delete if matches!(kind, ListKind::History | ListKind::Saved) => {
                    if let Some(value) = selected {
                        app.list_remove(&value);
                    }
                }
                _ => {
                    if chooser.input.key(key) == Some(true) {
                        chooser.selected = 0;
                    }
                }
            }
        }
        Overlay::SaveQuery(input) => match key.code {
            KeyCode::Esc => app.overlay = None,
            KeyCode::Enter => {
                let name = input.text.clone();
                app.save_query(&name);
            }
            _ => {
                input.key(key);
            }
        },
        Overlay::Export { input, .. } => match key.code {
            KeyCode::Esc => app.overlay = None,
            KeyCode::Enter => app.defer(Deferred::Export),
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


/// A left click: select a result (open it on a double click), switch a
/// record tab, or put the cursor on a field (follow it on a double click).
pub(super) fn click(app: &mut App<'_>, x: u16, y: u16, now: std::time::Instant) {
    if app.overlay.is_some() {
        return;
    }
    let inside = |area: ratatui::layout::Rect| {
        x >= area.x && x < area.right() && y >= area.y && y < area.bottom()
    };
    let double = |app: &mut App<'_>, target: usize| {
        let repeat = app.last_click.is_some_and(|(at, last)| {
            last == target && now.duration_since(at) < std::time::Duration::from_millis(400)
        });
        app.last_click = if repeat { None } else { Some((now, target)) };
        repeat
    };
    match app.view {
        View::Results => {
            let area = app.table_area.get();
            // The first row of the table is its header.
            if !inside(area) || y == area.y {
                return;
            }
            let index = app.scroll + (y - area.y - 1) as usize;
            if index >= app.hits.len() {
                return;
            }
            app.selected = index;
            app.selection_moved = true;
            app.keep_visible();
            app.load_more();
            if double(app, index) {
                app.open_detail();
            }
        }
        View::Detail => {
            let tabs = app.tabs_area.get();
            if inside(tabs) {
                // Tabs are drawn as `   ` + ` 1 Fields ` + `  ` + ` 2 Links n ` + …
                let column = (x - tabs.x) as usize;
                app.links.section = match column {
                    0..=13 => DetailSection::Fields,
                    14..=27 => DetailSection::Links,
                    _ => DetailSection::Backlinks,
                };
                return;
            }
            let area = app.record_area.get();
            if app.links.section != DetailSection::Fields || app.record.raw || !inside(area) {
                return;
            }
            let Some(layout) = app.record_layout() else {
                return;
            };
            let line = app.record.scroll.get() + (y - area.y) as usize;
            let Some(&row) = layout.columns.row.get(line) else {
                return;
            };
            if layout.rows[row].is_separator() {
                return;
            }
            app.record.cursor = row;
            if double(app, row) {
                app.record_enter();
            }
        }
    }
}


/// Mouse wheel: scroll the results or the record.
pub(super) fn wheel(app: &mut App<'_>, delta: isize) {
    if app.overlay.is_some() {
        return;
    }
    match app.view {
        View::Results => app.move_selection(delta),
        View::Detail if app.links.section == DetailSection::Fields => app.record_move(delta),
        View::Detail => {}
    }
}


#[cfg(test)]
#[path = "keys.test.rs"]
mod tests;
