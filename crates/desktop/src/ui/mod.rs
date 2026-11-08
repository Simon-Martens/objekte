//! GPUI presentation and interaction state for the desktop application.

pub mod model;
pub mod open_dialog;
mod panels;
pub mod preferences;
mod render;
mod state;

use std::sync::mpsc::Sender;

use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, ScrollHandle, Subscription, Window, point, px,
};

use model::{SearchDraft, UiAction, UiUpdate};
use state::UiState;


/// GPUI input entities owned by one desktop window.
struct Inputs {
    paths: Entity<TextareaState>,
    schema: Entity<InputState>,
    snapshot_directory: Entity<InputState>,
    full_text: Entity<InputState>,
    structured: Entity<InputState>,
    sort: Entity<InputState>,
    offset: Entity<InputState>,
    limit: Entity<InputState>,
    find: Entity<InputState>,
    query_name: Entity<InputState>,
    export_path: Entity<InputState>,
    batch_text: Entity<TextareaState>,
    batch_path: Entity<InputState>,
    batch_output_path: Entity<InputState>,
}


/// Native read-only workspace; the controller owns repository and worker state.
pub struct DesktopView {
    actions: Sender<UiAction>,
    state: UiState,
    inputs: Inputs,
    grid_focus: FocusHandle,
    grid_scroll: ScrollHandle,
    detail_focus: FocusHandle,
    restoring_query: bool,
    _subscriptions: Vec<Subscription>,
}


impl DesktopView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>, actions: Sender<UiAction>) -> Self {
        let inputs = Inputs {
            paths: cx.new(|cx| TextareaState::new(window, cx)),
            schema: cx
                .new(|cx| InputState::new(window, cx).placeholder("Schema file (single folder)")),
            snapshot_directory: cx
                .new(|cx| InputState::new(window, cx).placeholder("Index directory (optional)")),
            full_text: cx.new(|cx| InputState::new(window, cx).placeholder("Search words or text")),
            structured: cx.new(|cx| {
                InputState::new(window, cx).placeholder("Structured filter, including SORT BY")
            }),
            sort: cx.new(|cx| InputState::new(window, cx).placeholder("Sort field (optional)")),
            offset: cx.new(|cx| InputState::new(window, cx).default_value("0")),
            limit: cx.new(|cx| InputState::new(window, cx).placeholder("Unlimited")),
            find: cx.new(|cx| InputState::new(window, cx).placeholder("Find in record")),
            query_name: cx.new(|cx| InputState::new(window, cx).placeholder("Saved query name")),
            export_path: cx.new(|cx| InputState::new(window, cx).placeholder("New output path")),
            batch_text: cx.new(|cx| TextareaState::new(window, cx)),
            batch_path: cx
                .new(|cx| InputState::new(window, cx).placeholder("Batch file path (optional)")),
            batch_output_path: cx
                .new(|cx| InputState::new(window, cx).placeholder("New NDJSON output path")),
        };
        let mut subscriptions = Vec::new();
        for input in [
            &inputs.full_text,
            &inputs.structured,
            &inputs.sort,
            &inputs.offset,
            &inputs.limit,
        ] {
            subscriptions.push(
                cx.subscribe(input, |this: &mut Self, _, event, cx| match event {
                    InputEvent::Change => this.search_from_inputs(false, cx),
                    InputEvent::PressEnter { .. } => this.search_from_inputs(true, cx),
                    _ => {}
                }),
            );
        }
        Theme::change(ThemeMode::Dark, Some(window), cx);
        let state = UiState {
            dark: true,
            show_preview: true,
            status: "Open a folder or configured repository to begin".into(),
            ..UiState::default()
        };
        Self {
            actions,
            state,
            inputs,
            grid_focus: cx.focus_handle(),
            grid_scroll: ScrollHandle::new(),
            detail_focus: cx.focus_handle(),
            restoring_query: false,
            _subscriptions: subscriptions,
        }
    }

    /// Apply one controller-validated update; only bounded display data remains.
    pub fn apply(&mut self, update: UiUpdate, window: &mut Window, cx: &mut Context<Self>) {
        if let UiUpdate::Clipboard(text) = update {
            if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                std::thread::spawn(move || write_wayland_clipboard(text));
            } else {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
            }
            return;
        }
        if let UiUpdate::Theme { dark } = &update {
            Theme::change(
                if *dark {
                    ThemeMode::Dark
                } else {
                    ThemeMode::Light
                },
                Some(window),
                cx,
            );
        }
        if let UiUpdate::ChosenPaths(paths) = &update {
            self.inputs
                .paths
                .update(cx, |input, cx| input.set_value(paths.clone(), window, cx));
        }
        if let UiUpdate::RestoreQuery(draft) = &update {
            self.restoring_query = true;
            self.inputs.full_text.update(cx, |input, cx| {
                input.set_value(draft.full_text.clone(), window, cx)
            });
            self.inputs.structured.update(cx, |input, cx| {
                input.set_value(draft.structured.clone(), window, cx)
            });
            self.inputs.sort.update(cx, |input, cx| {
                input.set_value(draft.sort.clone(), window, cx)
            });
            self.inputs.offset.update(cx, |input, cx| {
                input.set_value(draft.offset.clone(), window, cx)
            });
            self.inputs.limit.update(cx, |input, cx| {
                input.set_value(draft.limit.clone(), window, cx)
            });
        }
        let reset_grid_scroll = match &update {
            UiUpdate::Opening(_) | UiUpdate::QueryStarted(_) => true,
            UiUpdate::Page(page) => {
                page.offset != self.state.page.offset
                    || page.rows.first().map(|row| row.key.token)
                        != self.state.page.rows.first().map(|row| row.key.token)
            }
            _ => false,
        };
        self.state.apply(update);
        if reset_grid_scroll {
            self.grid_scroll.set_offset(point(px(0.), px(0.)));
        }
        self.restoring_query = false;
        cx.notify();
    }

    /// Read the current query without losing invalid numeric text.
    fn query_from_inputs(&self, cx: &mut Context<Self>) -> SearchDraft {
        let mut draft = self.state.search.clone();
        draft.full_text = self.inputs.full_text.read(cx).value().to_string();
        draft.structured = self.inputs.structured.read(cx).value().to_string();
        draft.sort = self.inputs.sort.read(cx).value().to_string();
        draft.offset = self.inputs.offset.read(cx).value().to_string();
        draft.limit = self.inputs.limit.read(cx).value().to_string();
        draft
    }

    fn search_from_inputs(&mut self, explicit: bool, cx: &mut Context<Self>) {
        if self.restoring_query {
            return;
        }
        let draft = self.query_from_inputs(cx);
        self.state.search = draft.clone();
        self.send(if explicit {
            UiAction::SearchNow(draft)
        } else {
            UiAction::SearchEdited(draft)
        });
        cx.notify();
    }

    fn open_from_inputs(&mut self, cx: &mut Context<Self>) {
        let mut draft = self.state.open.clone();
        draft.paths = self.inputs.paths.read(cx).value().to_string();
        draft.schema = self.inputs.schema.read(cx).value().to_string();
        draft.snapshot_directory = self.inputs.snapshot_directory.read(cx).value().to_string();
        self.state.open = draft.clone();
        self.send(UiAction::Open(draft));
    }

    fn choose_folder(&mut self, cx: &mut Context<Self>) {
        let mut draft = self.state.open.clone();
        draft.paths = self.inputs.paths.read(cx).value().to_string();
        self.send(UiAction::ChooseFolder(draft));
    }

    fn send(&mut self, action: UiAction) {
        if self.actions.send(action).is_err() {
            self.state.status = "Desktop controller disconnected".into();
        }
    }

    fn select_relative(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.state.page.rows.len();
        if len == 0 {
            return;
        }
        let next = self.state.page.selected.map_or(0, |current| {
            current.saturating_add_signed(delta).min(len - 1)
        });
        self.state.page.selected = Some(next);
        self.grid_scroll.scroll_to_item(next + 1);
        self.send(UiAction::SelectRow(next));
        cx.notify();
    }

    fn select_absolute(&mut self, last: bool, cx: &mut Context<Self>) {
        let len = self.state.page.rows.len();
        if len == 0 {
            return;
        }
        let index = if last { len - 1 } else { 0 };
        self.state.page.selected = Some(index);
        self.grid_scroll.scroll_to_item(index + 1);
        self.send(UiAction::SelectRow(index));
        cx.notify();
    }

    fn select_detail_relative(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(detail) = self.state.detail.as_mut() else {
            return;
        };
        let len = detail.structured.len() + detail.relations.len();
        if len == 0 {
            return;
        }
        let next = detail
            .selected_row
            .saturating_add_signed(delta)
            .min(len - 1);
        detail.selected_row = next;
        self.send(UiAction::DetailRowSelected(next));
        cx.notify();
    }

    fn activate_detail_row(&mut self) {
        let Some(detail) = self.state.detail.as_ref() else {
            return;
        };
        let selected = detail.selected_row;
        let action = if let Some(row) = detail.structured.get(selected) {
            row.expandable
                .then_some(UiAction::ToggleDetailExpansion(selected))
        } else {
            detail
                .relations
                .get(selected.saturating_sub(detail.structured.len()))
                .and_then(|relation| {
                    relation
                        .available
                        .then_some(UiAction::FollowRelation(selected - detail.structured.len()))
                })
        };
        if let Some(action) = action {
            self.send(action);
        }
    }
}


/// GPUI's delayed clipboard writes stay internal on the tested headless Wayland
/// seat. Mirror complete worker results to the system clipboard off the UI thread.
fn write_wayland_clipboard(text: String) {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let result = (|| -> std::io::Result<()> {
        let mut child = Command::new("wl-copy")
            .arg("--type")
            .arg("text/plain;charset=utf-8")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(text.as_bytes())?;
        }
        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Err(std::io::Error::other(
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ));
        }
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!("objekte desktop: Wayland clipboard update failed: {error}");
    }
}
