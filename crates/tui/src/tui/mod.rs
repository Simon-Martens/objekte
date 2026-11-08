//! Interactive search screen adapted from the earlier terminal client.

mod app;
mod clipboard;
pub mod export;
mod input;
mod record;
pub mod relations;
pub mod settings;
#[cfg(test)]
mod test_support;
mod view;

use std::error::Error;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use app::{Action, App, Focus, Overlay, View};
use objekte_search_core::repository::{Repository, SnapshotAction};
use objekte_search_core::streaming::CancellationToken;
use ratatui::crossterm::{
    event::{
        self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
        MouseEventKind,
    },
    execute,
};


#[derive(Clone, Default)]
pub struct TuiOptions {
    pub single_collection: bool,
    pub initial_search: String,
    pub initial_filter: String,
    pub offset: usize,
    pub limit: Option<usize>,
    pub loading_label: Option<String>,
    pub interruption: Option<CancellationToken>,
}


pub fn run_with_loader<F>(loader: F, options: TuiOptions) -> Result<(), Box<dyn Error>>
where
    F: FnOnce() -> Result<Repository, String> + Send + 'static,
{
    let mut terminal = ratatui::try_init()?;
    let result = (|| -> Result<(), Box<dyn Error>> {
        execute!(std::io::stdout(), event::EnableMouseCapture)?;
        let interrupted = || {
            std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "interactive search interrupted",
            )
        };
        let started = Instant::now();
        let (sender, receiver) = mpsc::channel();
        let loading_label = options
            .loading_label
            .as_deref()
            .unwrap_or("Building search index")
            .to_owned();
        terminal.draw(|frame| {
            view::draw_loading_with_label(frame, started.elapsed(), &loading_label)
        })?;
        std::thread::spawn(move || {
            let result = loader();
            let _ = sender.send(result);
        });
        let repository = loop {
            if options
                .interruption
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled)
            {
                return Err(interrupted().into());
            }
            match receiver.try_recv() {
                Ok(Ok(repository)) => break repository,
                Ok(Err(error)) => return Err(std::io::Error::other(error).into()),
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(std::io::Error::other("repository loader stopped").into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            terminal.draw(|frame| {
                view::draw_loading_with_label(frame, started.elapsed(), &loading_label)
            })?;
            if event::poll(Duration::from_millis(100))? {
                match event::read()? {
                    Event::Key(pressed)
                        if pressed.kind != KeyEventKind::Release
                            && (pressed.code == KeyCode::Esc
                                || (pressed.code == KeyCode::Char('c')
                                    && pressed.modifiers.contains(KeyModifiers::CONTROL))
                                || (pressed.code == KeyCode::Char('q')
                                    && pressed.modifiers.contains(KeyModifiers::CONTROL))) =>
                    {
                        if pressed.code == KeyCode::Char('c')
                            && pressed.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            return Err(interrupted().into());
                        }
                        return Ok(());
                    }
                    _ => {}
                }
            }
        };
        let action = repository.startup().snapshot_action;
        let mut app = App::with_options(repository, options);
        let operation = if action == SnapshotAction::Restored {
            "Index loaded"
        } else {
            "Index built"
        };
        let ready = format!("{operation} in {}", view::elapsed_label(started.elapsed()));
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
            if app.interrupted() {
                return Err(interrupted().into());
            }
            app.tick(Instant::now());
            terminal.draw(|frame| view::draw(frame, &mut app))?;
            let poll_ms = if app.busy() { 30 } else { 250 };
            if !event::poll(Duration::from_millis(poll_ms))? {
                continue;
            }
            loop {
                match event::read()? {
                    Event::Key(pressed) if pressed.kind != KeyEventKind::Release => {
                        if pressed.code == KeyCode::Char('c')
                            && pressed.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            return Err(interrupted().into());
                        }
                        key(&mut app, pressed)
                    }
                    Event::Mouse(mouse) => mouse_event(&mut app, mouse),
                    _ => {}
                }
                if app.done || !event::poll(Duration::ZERO)? {
                    break;
                }
            }
            if let Some(text) = app.clipboard.take() {
                use std::io::Write;
                let mut stdout = std::io::stdout();
                stdout.write_all(clipboard::osc52(&text).as_bytes())?;
                stdout.flush()?;
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
            if app.view == View::Detail && !app.record.raw && inside(app.detail_area) {
                let visible = mouse.row.saturating_sub(app.detail_area.y) as usize;
                let target = app.record.visible_rows.get(visible).copied().or_else(|| {
                    app.record
                        .visible_rows
                        .is_empty()
                        .then(|| app.record.scroll.saturating_add(visible))
                });
                if let Some(target) = target {
                    app.record.cursor = target;
                    app.toggle_detail_branch();
                }
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
    if app.record.find_input.is_some() {
        match key.code {
            KeyCode::Esc => app.record.find_input = None,
            KeyCode::Enter => app.confirm_find(),
            _ => {
                if let Some(input) = &mut app.record.find_input {
                    input.key(key);
                }
            }
        }
        return;
    }
    match app.overlay {
        Some(Overlay::Actions) => {
            match key.code {
                KeyCode::Esc => app.overlay = None,
                KeyCode::Up => app.action_picker = app.action_picker.saturating_sub(1),
                KeyCode::Down => {
                    app.action_picker = (app.action_picker + 1).min(Action::ALL.len() - 1);
                }
                KeyCode::Enter => {
                    if let Some(action) = app.choose_action() {
                        run_action(app, action);
                    }
                }
                _ => {}
            }
            return;
        }
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
                _ if app.single_collection => {
                    app.sort_input.key(key);
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
    if control && key.code == KeyCode::Char('k') && app.view == View::Results {
        app.action_picker = 0;
        app.overlay = Some(Overlay::Actions);
        return;
    }
    if control && key.code == KeyCode::Char('o') {
        run_action(app, Action::Collections);
        return;
    }
    if control && key.code == KeyCode::Char('r') {
        run_action(app, Action::History);
        return;
    }
    if control && key.code == KeyCode::Char('b') {
        run_action(app, Action::Saved);
        return;
    }
    if control && key.code == KeyCode::Char('s') {
        run_action(app, Action::Save);
        return;
    }
    if control && key.code == KeyCode::Char('p') {
        run_action(app, Action::Preview);
        return;
    }
    if app.view == View::Results && !control && key.modifiers.contains(KeyModifiers::ALT) {
        match key.code {
            KeyCode::Char('c') => {
                run_action(app, Action::Case);
                return;
            }
            KeyCode::Char('r') => {
                run_action(app, Action::Regex);
                return;
            }
            KeyCode::Left => {
                app.scroll_columns(false);
                return;
            }
            KeyCode::Right => {
                app.scroll_columns(true);
                return;
            }
            _ => {}
        }
    }
    if key.modifiers.contains(KeyModifiers::ALT) && key.code == KeyCode::Char('s') {
        run_action(app, Action::Sort);
        return;
    }
    if key.modifiers.contains(KeyModifiers::ALT) && key.code == KeyCode::Char('l') {
        run_action(app, Action::Columns);
        return;
    }
    if key.modifiers.contains(KeyModifiers::ALT) && matches!(key.code, KeyCode::Char('e' | 'a')) {
        run_action(
            app,
            if key.code == KeyCode::Char('a') {
                Action::ExportAll
            } else {
                Action::ExportRecord
            },
        );
        return;
    }
    if key.modifiers.contains(KeyModifiers::ALT) && key.code == KeyCode::Char('t') {
        run_action(app, Action::Theme);
        return;
    }
    if app.view == View::Detail {
        match key.code {
            KeyCode::Tab => app.cycle_detail_section(!key.modifiers.contains(KeyModifiers::SHIFT)),
            KeyCode::BackTab => app.cycle_detail_section(false),
            KeyCode::Left if key.modifiers.contains(KeyModifiers::ALT) => {
                app.adjacent_result(false)
            }
            KeyCode::Right if key.modifiers.contains(KeyModifiers::ALT) => {
                app.adjacent_result(true)
            }
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
            KeyCode::Char('/') if !app.record.raw => {
                app.record.find_input = Some(input::Input::with_text(&app.record.find));
            }
            KeyCode::Char('/') => {
                app.status = Some("Find is available in structured view".into());
            }
            KeyCode::Char('n') => app.find_next(true),
            KeyCode::Char('N') => app.find_next(false),
            KeyCode::Char('y') => app.copy_detail(false),
            KeyCode::Char('Y') => app.copy_detail(true),
            KeyCode::Char('1') => app.jump_detail_section(1),
            KeyCode::Char('2') if !app.single_collection => app.jump_detail_section(2),
            KeyCode::Char('3') if !app.single_collection => app.jump_detail_section(3),
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
        KeyCode::Home if control || app.focus == Focus::Results => {
            app.focus = Focus::Results;
            app.move_selection(-(app.selected as isize));
        }
        KeyCode::End if control || app.focus == Focus::Results => {
            app.focus = Focus::Results;
            app.move_selection(isize::MAX);
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

fn run_action(app: &mut App, action: Action) {
    match action {
        Action::Collections => {
            if app.single_collection {
                app.status = Some(format!("Collection: {}", app.scope_label()));
            } else {
                app.picker = app.scope;
                app.overlay = Some(Overlay::Collections);
            }
        }
        Action::Sort => {
            if app.scope == 0 {
                app.status = Some("Choose a collection before sorting".into());
            } else {
                app.sort_picker = app
                    .sort_choices()
                    .iter()
                    .position(|choice| choice.as_ref() == app.sort.as_ref())
                    .unwrap_or(0);
                app.sort_input = input::Input::default();
                app.overlay = Some(Overlay::Sort);
            }
        }
        Action::Columns => {
            app.column_picker = 0;
            app.overlay = Some(Overlay::Columns);
        }
        Action::History => {
            if app.settings.history.is_empty() {
                app.status = Some("No recent queries".into());
            } else {
                app.list_picker = 0;
                app.overlay = Some(Overlay::History);
            }
        }
        Action::Saved => {
            if app.settings.saved.is_empty() {
                app.status = Some("No saved queries".into());
            } else {
                app.list_picker = 0;
                app.overlay = Some(Overlay::Saved);
            }
        }
        Action::Save => {
            app.save_input = input::Input::default();
            app.overlay = Some(Overlay::SaveQuery);
        }
        Action::ExportRecord | Action::ExportAll => {
            if app.busy() {
                app.status = Some("Wait for the search to finish before exporting".into());
            } else if matches!(action, Action::ExportRecord) && app.selected_hit().is_none() {
                app.status = Some("Select a record before exporting".into());
            } else {
                app.export_input = input::Input::default();
                app.overlay = Some(Overlay::Export {
                    all: matches!(action, Action::ExportAll),
                });
            }
        }
        Action::Case => app.toggle_case(),
        Action::Regex => app.toggle_regex(),
        Action::Preview => app.show_preview = !app.show_preview,
        Action::Theme => {
            let light = !view::theme::is_light();
            view::theme::set_light(light);
            app.set_theme(light);
        }
        Action::Help => app.overlay = Some(Overlay::Help),
    }
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
