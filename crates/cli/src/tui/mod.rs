//! Interactive search over a warmed repository cache.
//!
//! The screen layout is fixed: header, search and filter lines, a summary,
//! the results (with a preview on wide terminals), and a footer. Typing
//! searches as you go; nothing moves except the content.

mod app;
mod complete;
mod detail;
mod export;
mod files;
mod input;
mod keys;
mod lists;
mod match_preview;
mod navigation;
mod record_view;
mod relations;
mod search;
mod settings;
mod view;
mod warm;

use self::app::App;
use objekte_core::Repository;
use ratatui::crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyboardEnhancementFlags,
        MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
};
use std::error::Error;
use std::{
    sync::mpsc::TryRecvError,
    time::{Duration, Instant},
};


fn apply_warm_update(app: &mut App<'_>, update: warm::WarmUpdate) {
    match update {
        warm::WarmUpdate::Progress {
            completed,
            total,
            mode,
            restored,
        } => {
            let action = if mode == warm::WarmMode::Reindex {
                "Reindexing"
            } else if mode == warm::WarmMode::Save {
                "Saving"
            } else if restored > 0 {
                "Restoring"
            } else {
                "Building"
            };
            app.index_status = Some(if mode == warm::WarmMode::Warm && restored > 0 {
                format!(
                    "{action} indexes · {completed}/{total} collections ready · {restored} from cache"
                )
            } else {
                format!("{action} indexes · {completed}/{total} collections ready")
            });
        }
        warm::WarmUpdate::Ready(mut built, mode) => {
            app.value_counts.borrow_mut().clear();
            let restored = built.metrics().secondary_snapshot_loads;
            let bigram_restored = built.metrics().bigram_snapshot_loads;
            std::mem::swap(app.repo, &mut *built);
            app.reconciling = mode == warm::WarmMode::Warm && bigram_restored > 0;
            app.inventory_complete = false;
            app.reconciled_files = 0;
            app.index_status = app
                .reconciling
                .then(|| "Checking files in background · search available".into());
            if app.queued_search {
                app.search();
            } else {
                let text: String = match mode {
                    warm::WarmMode::Warm if restored > 0 => {
                        let noun = if restored == 1 {
                            "snapshot"
                        } else {
                            "snapshots"
                        };
                        format!("Search ready · {restored} index {noun} restored; checking files.")
                    }
                    warm::WarmMode::Warm if bigram_restored > 0 => {
                        "Search ready from saved bigram index; checking files.".into()
                    }
                    warm::WarmMode::Warm => "Search ready. Enter a query to search.".into(),
                    warm::WarmMode::Save => "Bigram caches saved.".into(),
                    warm::WarmMode::Reindex => "All indexes rebuilt and saved.".into(),
                };
                app.notice(text);
            }
        }
        warm::WarmUpdate::Inventory(inventory) => {
            app.pending_inventories.push_back(inventory);
        }
        warm::WarmUpdate::Checked => {
            app.inventory_complete = true;
        }
        warm::WarmUpdate::Failed(error) => {
            app.index_status = None;
            if app.reconciling {
                app.reconciling = false;
                app.pending_inventories.clear();
                app.error(format!("File check failed; search may be stale: {error}"));
            } else {
                app.queued_search = false;
                app.error(format!("Index build failed: {error}"));
            }
        }
    }
}

fn drive_reconciliation(app: &mut App<'_>, searches: &search::SearchWorker) {
    if !app.reconciling || !searches.is_idle() {
        return;
    }
    if let Some(inventory) = app.pending_inventories.pop_front() {
        match app.repo.apply_collection_inventory(inventory) {
            Ok(changed) => {
                app.reconciled_files += changed;
                if changed > 0 && app.last_request.is_some() && !app.searching {
                    app.rerun();
                }
            }
            Err(error) => {
                app.error(format!("File check failed; search may be stale: {error}"));
            }
        }
    }
    if app.inventory_complete && app.pending_inventories.is_empty() && searches.is_idle() {
        app.reconciling = false;
        app.index_status = None;
        if app.reconciled_files > 0 {
            app.notice(format!(
                "File check complete · {} changed files updated",
                app.reconciled_files
            ));
        }
    }
}

/// Leave the screen to the editor, then take it back. Returns whether the
/// editor exited successfully.
fn edit_externally(
    terminal: &mut ratatui::DefaultTerminal,
    path: &std::path::Path,
) -> Result<bool, Box<dyn Error>> {
    let command = files::editor();
    execute!(
        std::io::stdout(),
        DisableMouseCapture,
        PopKeyboardEnhancementFlags
    )?;
    ratatui::try_restore()?;
    let status = std::process::Command::new(&command[0])
        .args(&command[1..])
        .arg(path)
        .status();
    *terminal = ratatui::try_init()?;
    execute!(
        std::io::stdout(),
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
        ),
        EnableMouseCapture
    )?;
    terminal.clear()?;
    Ok(status?.success())
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
        ),
        EnableMouseCapture
    ) {
        let _ = ratatui::try_restore();
        return Err(error.into());
    }
    let result = (|| -> Result<(), Box<dyn Error>> {
        let startup_status = warm::startup_status(repo);
        let mut updates = warm::start(repo, warm::WarmMode::Warm);
        let mut app = App::new(repo);
        app.load_settings();
        let mut searches = search::SearchWorker::default();
        app.async_search = true;
        app.index_status = Some(startup_status);
        while !app.done {
            loop {
                match updates.try_recv() {
                    Ok(update) => apply_warm_update(&mut app, update),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        if app.index_status.is_some() && !app.inventory_complete {
                            app.index_status = None;
                            app.queued_search = false;
                            app.error("Index worker stopped unexpectedly.");
                        }
                        break;
                    }
                }
            }
            searches.drive(&mut app);
            drive_reconciliation(&mut app, &searches);
            app.tick(Instant::now());
            terminal.draw(|frame| view::draw(frame, &app))?;
            // Poll quickly while something animates or a typed query waits.
            let busy = app.searching || app.dirty.is_some() || app.index_status.is_some();
            let poll_ms = if busy { 30 } else { 250 };
            if !event::poll(Duration::from_millis(poll_ms))? {
                continue;
            }
            // Handle everything already queued before drawing again, so
            // fast typing and held keys never lag behind.
            loop {
                match event::read()? {
                    Event::Key(pressed) => keys::key(&mut app, pressed),
                    Event::Mouse(mouse) => match mouse.kind {
                        MouseEventKind::ScrollDown => keys::wheel(&mut app, 3),
                        MouseEventKind::ScrollUp => keys::wheel(&mut app, -3),
                        MouseEventKind::Down(event::MouseButton::Left) => {
                            keys::click(&mut app, mouse.column, mouse.row, Instant::now());
                        }
                        _ => {}
                    },
                    _ => {}
                }
                if app.done || !event::poll(Duration::ZERO)? {
                    break;
                }
            }
            if let Some(text) = app.clipboard.take() {
                use std::io::Write;
                let mut stdout = std::io::stdout();
                stdout.write_all(files::osc52(&text).as_bytes())?;
                stdout.flush()?;
            }
            // Some work needs the repository, which a running search or
            // backlink lookup has borrowed; it waits until it is back.
            if !app.deferred.is_empty()
                && searches.is_idle()
                && app.pending_search.is_none()
                && let Some(path) = app.run_deferred()
            {
                match edit_externally(&mut terminal, &path) {
                    Ok(true) => app.apply_edit(&path),
                    Ok(false) => app.error("The editor reported an error"),
                    Err(error) => app.error(format!("Cannot start the editor: {error}")),
                }
            }
            if let Some(mode) = app.pending_cache_action.take() {
                app.reconciling = false;
                app.pending_inventories.clear();
                updates = warm::start(app.repo, mode);
                app.index_status = Some(if mode == warm::WarmMode::Save {
                    "Saving indexes · 0 collections ready".into()
                } else {
                    "Reindexing indexes · 0 collections ready".into()
                });
            }
        }
        Ok(())
    })();
    let pop = execute!(
        std::io::stdout(),
        DisableMouseCapture,
        PopKeyboardEnhancementFlags
    );
    let restore = ratatui::try_restore();
    result?;
    pop?;
    restore?;
    Ok(())
}
