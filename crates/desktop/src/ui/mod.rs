//! GPUI presentation and interaction state for the desktop application.

pub mod filter_builder;
pub mod model;
pub mod open_dialog;
mod panels;
pub mod preferences;
pub mod query_bar;
mod render;
mod state;
mod theme;

use objekte_search_core::query::parse_structured_filter;
use std::sync::mpsc::Sender;

use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, Focusable as _, ScrollHandle, Subscription,
    Window, point, px,
};

use model::{SearchDraft, UiAction, UiUpdate};
use query_bar::{BuilderEvent, FilterBuilderView};
use state::UiState;


/// GPUI input entities owned by one desktop window.
struct Inputs {
    open_folder: Entity<InputState>,
    open_extra_folders: Vec<Entity<InputState>>,
    schema: Entity<InputState>,
    snapshot_directory: Entity<InputState>,
    full_text: Entity<InputState>,
    structured: Entity<InputState>,
    sort: Entity<InputState>,
    offset: Entity<InputState>,
    limit: Entity<InputState>,
    find: Entity<InputState>,
    query_name: Entity<InputState>,
    action_search: Entity<InputState>,
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
    builder_view: Entity<FilterBuilderView>,
    show_query_options: bool,
    grid_focus: FocusHandle,
    grid_scroll: ScrollHandle,
    grid_horizontal_scroll: ScrollHandle,
    detail_focus: FocusHandle,
    restoring_query: bool,
    _subscriptions: Vec<Subscription>,
    open_extra_subscriptions: Vec<Subscription>,
}


impl DesktopView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>, actions: Sender<UiAction>) -> Self {
        let inputs = Inputs {
            open_folder: cx.new(|cx| InputState::new(window, cx).placeholder("Choose a folder")),
            open_extra_folders: Vec::new(),
            schema: cx
                .new(|cx| InputState::new(window, cx).placeholder("Schema file (single folder)")),
            snapshot_directory: cx
                .new(|cx| InputState::new(window, cx).placeholder("Index directory (optional)")),
            full_text: cx.new(|cx| InputState::new(window, cx).placeholder("Search entries…")),
            structured: cx.new(|cx| {
                InputState::new(window, cx).placeholder("year >= 1900 && type = \"book\"")
            }),
            sort: cx.new(|cx| InputState::new(window, cx).placeholder("Sort field (optional)")),
            offset: cx.new(|cx| InputState::new(window, cx).default_value("0")),
            limit: cx.new(|cx| InputState::new(window, cx).placeholder("Unlimited")),
            find: cx.new(|cx| InputState::new(window, cx).placeholder("Find in record")),
            query_name: cx.new(|cx| InputState::new(window, cx).placeholder("Saved query name")),
            action_search: cx.new(|cx| InputState::new(window, cx).placeholder("Find a query")),
            export_path: cx.new(|cx| InputState::new(window, cx).placeholder("New output path")),
            batch_text: cx.new(|cx| TextareaState::new(window, cx)),
            batch_path: cx
                .new(|cx| InputState::new(window, cx).placeholder("Batch file path (optional)")),
            batch_output_path: cx
                .new(|cx| InputState::new(window, cx).placeholder("New NDJSON output path")),
        };
        let mut subscriptions = Vec::new();
        subscriptions.push(
            cx.subscribe(&inputs.open_folder, |this: &mut Self, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    this.open_folder_edited(0, cx);
                }
            }),
        );
        let builder_view = cx.new(|cx| FilterBuilderView::new(window, cx));
        subscriptions.push(cx.subscribe_in(
            &builder_view,
            window,
            |this: &mut Self, _, event: &BuilderEvent, window, cx| match event {
                BuilderEvent::Complete(text) => {
                    this.state.search.filter_enabled = true;
                    this.inputs
                        .structured
                        .update(cx, |input, cx| input.replace_all(text.clone(), window, cx));
                }
                BuilderEvent::Close => {
                    this.state.show_filter_builder = false;
                    this.inputs
                        .structured
                        .update(cx, |input, cx| input.focus(window, cx));
                    cx.notify();
                }
            },
        ));
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
        subscriptions.push(
            cx.subscribe(&inputs.action_search, |_: &mut Self, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
        );
        Theme::change(ThemeMode::Dark, Some(window), cx);
        let state = UiState {
            dark: true,
            sidebar_visible: true,
            inspector_width: 365.,
            show_preview: true,
            index_mode: model::SnapshotMode::Write,
            status: "Open a folder or configured repository to begin".into(),
            ..UiState::default()
        };
        Self {
            actions,
            state,
            inputs,
            builder_view,
            show_query_options: false,
            grid_focus: cx.focus_handle(),
            grid_scroll: ScrollHandle::new(),
            grid_horizontal_scroll: ScrollHandle::new(),
            detail_focus: cx.focus_handle(),
            restoring_query: false,
            _subscriptions: subscriptions,
            open_extra_subscriptions: Vec::new(),
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
        if let UiUpdate::ChosenFolder {
            generation,
            slot,
            path,
        } = &update
            && self
                .state
                .open_dialog
                .accept_picker(*generation, *slot, path.clone())
        {
            let input = if *slot == 0 {
                Some(&self.inputs.open_folder)
            } else {
                self.inputs.open_extra_folders.get(*slot - 1)
            };
            if let Some(input) = input {
                input.update(cx, |input, cx| input.set_value(path.clone(), window, cx));
            }
            self.start_open_detection();
        }
        if let UiUpdate::ChosenSchema { generation, path } = &update
            && self.state.open_dialog.accept_schema_picker(*generation)
        {
            self.state.open.schema = path.clone();
            self.inputs
                .schema
                .update(cx, |input, cx| input.set_value(path.clone(), window, cx));
        }
        if let UiUpdate::RestoreQuery(draft) = &update {
            self.restoring_query = true;
            self.show_query_options = !draft.sort.is_empty()
                || draft.offset != "0"
                || !draft.limit.is_empty()
                || draft.count;
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
            self.builder_view.update(cx, |builder, cx| {
                builder.sync_from_text(&draft.structured, cx)
            });
        }
        if let UiUpdate::FieldChoices(fields) = &update {
            self.builder_view
                .update(cx, |builder, cx| builder.set_fields(fields.clone(), cx));
        }
        let reset_grid_scroll = match &update {
            UiUpdate::QueryStarted(_) => true,
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
        self.builder_view.update(cx, |builder, cx| {
            builder.sync_from_text(&draft.structured, cx)
        });
        self.state.search = draft.clone();
        self.state.filter_validation = Some(model::FilterValidationState::Pending);
        self.state.showing_last_valid_results = self.state.root.is_some();
        self.send(if explicit {
            UiAction::SearchNow(draft)
        } else {
            UiAction::SearchEdited(draft)
        });
        cx.notify();
    }

    /// Cycle a column's global query sort without changing an inline SORT BY.
    fn sort_by_column(&mut self, column: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.search.filter_enabled
            && parse_structured_filter(&self.inputs.structured.read(cx).value())
                .ok()
                .and_then(|filter| filter.sort)
                .is_some()
        {
            self.state.status = "Edit SORT BY in the Filter line to change ordering".into();
            let focus = self.inputs.structured.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
            cx.notify();
            return;
        }
        let current = self.inputs.sort.read(cx).value().trim().to_owned();
        let next = if current == format!("{column} ASC") {
            format!("{column} DESC")
        } else if current == format!("{column} DESC") {
            String::new()
        } else {
            format!("{column} ASC")
        };
        self.restoring_query = true;
        self.inputs
            .sort
            .update(cx, |input, cx| input.set_value(next, window, cx));
        self.restoring_query = false;
        self.search_from_inputs(false, cx);
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
        self.state.inspector_visible = self.state.show_preview;
        self.grid_scroll.scroll_to_item(next);
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
        self.state.inspector_visible = self.state.show_preview;
        self.grid_scroll.scroll_to_item(index);
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
