//! Interactive search screen adapted from the earlier terminal client.

mod app;
mod export;
mod input;
mod record;
mod relations;
mod settings;
#[cfg(test)]
mod test_support;
mod view;

use std::error::Error;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use app::{App, Focus, Overlay, View};
use objekte_search_core::repository::{CollectionDefinition, Repository};
use ratatui::crossterm::{
    event::{
        self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
        MouseEventKind,
    },
    execute,
};

pub(super) fn run(
    root: PathBuf,
    definitions: Vec<CollectionDefinition>,
) -> Result<(), Box<dyn Error>> {
    let mut terminal = ratatui::try_init()?;
    execute!(std::io::stdout(), event::EnableMouseCapture)?;
    let result = (|| -> Result<(), Box<dyn Error>> {
        let started = Instant::now();
        let (sender, receiver) = mpsc::channel();
        terminal.draw(|frame| view::draw_loading(frame, started.elapsed()))?;
        std::thread::spawn(move || {
            let result = Repository::open(root, definitions);
            let _ = sender.send(result);
        });
        let repository = loop {
            match receiver.try_recv() {
                Ok(Ok(repository)) => break repository,
                Ok(Err(error)) => return Err(std::io::Error::other(error).into()),
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(std::io::Error::other("repository loader stopped").into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            terminal.draw(|frame| view::draw_loading(frame, started.elapsed()))?;
            if event::poll(Duration::from_millis(100))? {
                match event::read()? {
                    Event::Key(pressed)
                        if pressed.kind != KeyEventKind::Release
                            && (pressed.code == KeyCode::Esc
                                || (pressed.code == KeyCode::Char('q')
                                    && pressed.modifiers.contains(KeyModifiers::CONTROL))) =>
                    {
                        return Ok(());
                    }
                    _ => {}
                }
            }
        };
        let mut app = App::new(repository);
        let ready = format!("Index ready in {}", view::elapsed_label(started.elapsed()));
        app.status = Some(match app.status.take() {
            Some(status) => format!("{status}  ·  {ready}"),
            None => ready,
        });
        let light = match app.settings.theme.as_deref() {
            Some("light") => Some(true),
            Some("dark") => Some(false),
            _ => view::theme::detect_light(),
        };
        if let Some(light) = light {
            view::theme::set_light(light);
        }
        while !app.done {
            app.tick(Instant::now());
            terminal.draw(|frame| view::draw(frame, &mut app))?;
            let poll_ms = if app.busy() { 30 } else { 250 };
            if !event::poll(Duration::from_millis(poll_ms))? {
                continue;
            }
            loop {
                match event::read()? {
                    Event::Key(pressed) if pressed.kind != KeyEventKind::Release => {
                        key(&mut app, pressed)
                    }
                    Event::Mouse(mouse) => mouse_event(&mut app, mouse),
                    _ => {}
                }
                if app.done || !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
        Ok(())
    })();
    let mouse = execute!(std::io::stdout(), event::DisableMouseCapture);
    let restore = ratatui::try_restore();
    result?;
    mouse?;
    restore?;
    Ok(())
}

fn mouse_event(app: &mut App, mouse: MouseEvent) {
    let inside = |area: ratatui::layout::Rect| {
        mouse.column >= area.x
            && mouse.column < area.right()
            && mouse.row >= area.y
            && mouse.row < area.bottom()
    };
    match mouse.kind {
        MouseEventKind::ScrollDown => {
            if app.view == View::Detail {
                app.move_detail(3);
            } else {
                app.move_selection(3);
            }
        }
        MouseEventKind::ScrollUp => {
            if app.view == View::Detail {
                app.move_detail(-3);
            } else {
                app.move_selection(-3);
            }
        }
        MouseEventKind::Down(MouseButton::Left) if app.overlay.is_none() => {
            if app.view == View::Detail
                && !app.record.raw
                && inside(app.detail_area)
                && mouse.row > app.detail_area.y
                && mouse.row + 1 < app.detail_area.bottom()
            {
                let visible = mouse.row.saturating_sub(app.detail_area.y + 1) as usize;
                app.record.cursor = app.record.scroll.saturating_add(visible);
                app.toggle_detail_branch();
            } else if app.view == View::Results && inside(app.results_area) {
                let visible = mouse.row.saturating_sub(app.results_area.y + 1) as usize;
                let index = app.scroll.saturating_add(visible);
                if index < app.hits.len() {
                    app.move_selection(index as isize - app.selected as isize);
                    app.open_detail();
                }
            } else if app.view == View::Results && inside(app.detail_area) {
                app.open_detail();
            }
        }
        _ => {}
    }
}

fn key(app: &mut App, key: KeyEvent) {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    if control && key.code == KeyCode::Char('q') {
        app.done = true;
        return;
    }
    match app.overlay {
        Some(Overlay::Help) => {
            if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::F(1)) {
                app.overlay = None;
            }
            return;
        }
        Some(Overlay::Collections) => {
            match key.code {
                KeyCode::Esc => app.overlay = None,
                KeyCode::Enter => app.choose_scope(),
                KeyCode::Up => app.picker = app.picker.saturating_sub(1),
                KeyCode::Down => {
                    app.picker = (app.picker + 1).min(app.collection_names().count());
                }
                _ => {}
            }
            return;
        }
        Some(Overlay::Sort) => {
            match key.code {
                KeyCode::Esc => app.overlay = None,
                KeyCode::Enter => app.choose_sort(),
                KeyCode::Up => app.sort_picker = app.sort_picker.saturating_sub(1),
                KeyCode::Down => {
                    app.sort_picker = (app.sort_picker + 1).min(app.sort_choices().len() - 1);
                }
                _ => {}
            }
            return;
        }
        Some(Overlay::History) => {
            match key.code {
                KeyCode::Esc => app.overlay = None,
                KeyCode::Up => app.list_picker = app.list_picker.saturating_sub(1),
                KeyCode::Down => {
                    app.list_picker =
                        (app.list_picker + 1).min(app.settings.history.len().saturating_sub(1));
                }
                KeyCode::Enter => {
                    if let Some(query) = app
                        .settings
                        .history
                        .iter()
                        .rev()
                        .nth(app.list_picker)
                        .cloned()
                    {
                        app.restore_query(query);
                    }
                }
                _ => {}
            }
            return;
        }
        Some(Overlay::Saved) => {
            match key.code {
                KeyCode::Esc => app.overlay = None,
                KeyCode::Up => app.list_picker = app.list_picker.saturating_sub(1),
                KeyCode::Down => {
                    app.list_picker =
                        (app.list_picker + 1).min(app.settings.saved.len().saturating_sub(1));
                }
                KeyCode::Enter => {
                    if let Some(saved) = app.settings.saved.get(app.list_picker).cloned() {
                        app.restore_query(saved.query);
                    }
                }
                _ => {}
            }
            return;
        }
        Some(Overlay::SaveQuery) => {
            match key.code {
                KeyCode::Esc => app.overlay = None,
                KeyCode::Enter => app.save_query(),
                _ => {
                    app.save_input.key(key);
                }
            }
            return;
        }
        Some(Overlay::Columns) => {
            match key.code {
                KeyCode::Esc => app.overlay = None,
                KeyCode::Up => app.column_picker = app.column_picker.saturating_sub(1),
                KeyCode::Down => {
                    app.column_picker =
                        (app.column_picker + 1).min(app.column_keys().len().saturating_sub(1));
                }
                KeyCode::Enter => app.toggle_column(),
                _ => {}
            }
            return;
        }
        Some(Overlay::Export { all }) => {
            match key.code {
                KeyCode::Esc => app.overlay = None,
                KeyCode::Enter => app.finish_export(all),
                _ => {
                    app.export_input.key(key);
                }
            }
            return;
        }
        None => {}
    }
    if key.code == KeyCode::F(1) {
        app.overlay = Some(Overlay::Help);
        return;
    }
    if control && key.code == KeyCode::Char('o') {
        app.picker = app.scope;
        app.overlay = Some(Overlay::Collections);
        return;
    }
    if control && key.code == KeyCode::Char('r') {
        if app.settings.history.is_empty() {
            app.status = Some("No recent queries".into());
        } else {
            app.list_picker = 0;
            app.overlay = Some(Overlay::History);
        }
        return;
    }
    if control && key.code == KeyCode::Char('b') {
        if app.settings.saved.is_empty() {
            app.status = Some("No saved queries".into());
        } else {
            app.list_picker = 0;
            app.overlay = Some(Overlay::Saved);
        }
        return;
    }
    if control && key.code == KeyCode::Char('s') {
        app.save_input = input::Input::default();
        app.overlay = Some(Overlay::SaveQuery);
        return;
    }
    if control && key.code == KeyCode::Char('p') {
        app.show_preview = !app.show_preview;
        return;
    }
    if key.modifiers.contains(KeyModifiers::ALT) && key.code == KeyCode::Char('s') {
        if app.scope == 0 {
            app.status = Some("Choose a collection before sorting".into());
        } else {
            app.sort_picker = app
                .sort_choices()
                .iter()
                .position(|choice| choice.as_ref() == app.sort.as_ref())
                .unwrap_or(0);
            app.overlay = Some(Overlay::Sort);
        }
        return;
    }
    if key.modifiers.contains(KeyModifiers::ALT) && key.code == KeyCode::Char('l') {
        app.column_picker = 0;
        app.overlay = Some(Overlay::Columns);
        return;
    }
    if key.modifiers.contains(KeyModifiers::ALT) && matches!(key.code, KeyCode::Char('e' | 'a')) {
        if app.busy() {
            app.status = Some("Wait for the search to finish before exporting".into());
        } else if key.code == KeyCode::Char('e') && app.selected_hit().is_none() {
            app.status = Some("Select a record before exporting".into());
        } else {
            app.export_input = input::Input::default();
            app.overlay = Some(Overlay::Export {
                all: key.code == KeyCode::Char('a'),
            });
        }
        return;
    }
    if key.modifiers.contains(KeyModifiers::ALT) && key.code == KeyCode::Char('t') {
        let light = !view::theme::is_light();
        view::theme::set_light(light);
        app.set_theme(light);
        return;
    }
    if app.view == View::Detail {
        match key.code {
            KeyCode::Esc | KeyCode::Left | KeyCode::Backspace => app.back_detail(),
            KeyCode::Up => app.move_detail(-1),
            KeyCode::Down => app.move_detail(1),
            KeyCode::PageUp => app.move_detail(-10),
            KeyCode::PageDown => app.move_detail(10),
            KeyCode::Enter | KeyCode::Right => app.toggle_detail_branch(),
            KeyCode::Char('r') => {
                app.record.raw = !app.record.raw;
                app.detail_scroll = 0;
            }
            _ => {}
        }
        return;
    }
    match key.code {
        KeyCode::Tab => {
            app.focus = match app.focus {
                Focus::Search => Focus::Filter,
                Focus::Filter => Focus::Results,
                Focus::Results => Focus::Search,
            };
        }
        KeyCode::BackTab => {
            app.focus = match app.focus {
                Focus::Search => Focus::Results,
                Focus::Filter => Focus::Search,
                Focus::Results => Focus::Filter,
            };
        }
        KeyCode::Esc => app.focus = Focus::Search,
        KeyCode::Enter if app.focus == Focus::Results => {
            app.open_detail();
        }
        KeyCode::Enter => app.confirm_query(),
        KeyCode::Up => {
            app.focus = Focus::Results;
            app.move_selection(-1);
        }
        KeyCode::Down => {
            app.focus = Focus::Results;
            app.move_selection(1);
        }
        KeyCode::PageUp => {
            app.focus = Focus::Results;
            app.move_selection(-(app.viewport_rows as isize));
        }
        KeyCode::PageDown => {
            app.focus = Focus::Results;
            app.move_selection(app.viewport_rows as isize);
        }
        _ if app.focus == Focus::Results => {}
        _ => {
            let input = match app.focus {
                Focus::Search => &mut app.search,
                Focus::Filter => &mut app.filter,
                Focus::Results => unreachable!(),
            };
            if input.key(key) == Some(true) {
                app.changed(Instant::now());
            }
        }
    }
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
