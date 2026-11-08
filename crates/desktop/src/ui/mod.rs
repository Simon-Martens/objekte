//! GPUI presentation and interaction state for the desktop application.

mod completion;
pub mod filter_builder;
pub mod model;
pub mod open_dialog;
mod panels;
pub mod preferences;
pub mod query_bar;
mod render;
mod sort;
mod state;
mod theme;
pub mod typography;

use objekte_search_core::query::parse_structured_filter;
use std::sync::mpsc::Sender;

use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, Focusable as _, ListAlignment, ListState,
    ScrollHandle, Subscription, Window, px,
};

use model::{SearchDraft, TabViewState, UiAction, UiUpdate};
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
    rename_title: Entity<InputState>,
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
    suspended_filter: Option<String>,
    renaming_saved: Option<u64>,
    focused_saved: Option<u64>,
    grid_focus: FocusHandle,
    grid_list: ListState,
    grid_horizontal_scroll: ScrollHandle,
    columns_scroll: ScrollHandle,
    detail_focus: FocusHandle,
    fields_scroll: ScrollHandle,
    links_scroll: ScrollHandle,
    source_scroll: ScrollHandle,
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
            rename_title: cx.new(|cx| InputState::new(window, cx).placeholder("Query title")),
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
        subscriptions.push(
            cx.subscribe(
                &inputs.full_text,
                |this: &mut Self, _, event, cx| match event {
                    InputEvent::Change => this.search_from_inputs(false, cx),
                    InputEvent::PressEnter { .. } => this.search_from_inputs(true, cx),
                    _ => {}
                },
            ),
        );
        subscriptions.push(cx.subscribe_in(
            &inputs.structured,
            window,
            |this: &mut Self, _, event, window, cx| match event {
                InputEvent::Change => {
                    this.search_from_inputs(false, cx);
                    this.refresh_completion(cx);
                }
                InputEvent::Focus => this.refresh_completion(cx),
                InputEvent::PressEnter { .. } => {
                    if !this.accept_completion(window, cx) {
                        this.search_from_inputs(true, cx);
                    }
                }
                InputEvent::Blur => {
                    // Pointer selection is handled by the popup's own item click.
                }
            },
        ));
        subscriptions.push(
            cx.subscribe(&inputs.rename_title, |this: &mut Self, _, event, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.commit_rename(cx);
                }
            }),
        );
        Theme::change(ThemeMode::Dark, Some(window), cx);
        typography::apply_theme_fonts(cx);
        let state = UiState {
            dark: true,
            sidebar_visible: true,
            inspector_width: 365.,
            show_preview: true,
            index_mode: model::SnapshotMode::Write,
            status: "Open a folder or configured repository to begin".into(),
            ..UiState::default()
        };
        let grid_list = ListState::new(0, ListAlignment::Top, px(180.));
        let visible_actions = actions.clone();
        grid_list.set_scroll_handler(move |event, _, _| {
            let _ = visible_actions.send(UiAction::VisibleRows {
                start: event.visible_range.start,
                end: event.visible_range.end,
            });
        });
        Self {
            actions,
            state,
            inputs,
            builder_view,
            suspended_filter: None,
            renaming_saved: None,
            focused_saved: None,
            grid_focus: cx.focus_handle(),
            grid_list,
            grid_horizontal_scroll: ScrollHandle::new(),
            columns_scroll: ScrollHandle::new(),
            detail_focus: cx.focus_handle(),
            fields_scroll: ScrollHandle::new(),
            links_scroll: ScrollHandle::new(),
            source_scroll: ScrollHandle::new(),
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
            typography::apply_theme_fonts(cx);
            self.builder_view
                .update(cx, |builder, cx| builder.set_dark(*dark, cx));
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
            self.suspended_filter = None;
            self.inputs.full_text.update(cx, |input, cx| {
                input.set_value(draft.full_text.clone(), window, cx)
            });
            self.inputs.structured.update(cx, |input, cx| {
                input.set_value(draft.structured.clone(), window, cx)
            });
            self.builder_view.update(cx, |builder, cx| {
                builder.sync_from_text(&draft.structured, cx)
            });
        }
        if let UiUpdate::FieldChoices(fields) = &update {
            self.builder_view
                .update(cx, |builder, cx| builder.set_fields(fields.clone(), cx));
        }
        let reset_grid_scroll = matches!(&update, UiUpdate::QueryStarted(_) | UiUpdate::Page(_));
        let switching_tab =
            matches!(&update, UiUpdate::Tabs { active, .. } if *active != self.state.active_tab);
        if switching_tab && let Some(id) = self.state.active_tab {
            let view = self.capture_tab_view();
            self.state.tab_views.insert(id, view);
            self.send(UiAction::SetTabView { id, view });
        }
        let restore_view = match &update {
            UiUpdate::RestoreTabView(view) => Some(*view),
            UiUpdate::Tabs { active, .. } if switching_tab => {
                active.and_then(|id| self.state.tab_views.get(&id).copied())
            }
            _ => None,
        };
        let refresh_fields = matches!(&update, UiUpdate::FieldChoices(_));
        let old_count = self.state.loaded_count;
        let append = matches!(&update, UiUpdate::AppendPage(_));
        let remeasure_grid = matches!(&update, UiUpdate::TablePresentation { .. });
        let reveal = match &update {
            UiUpdate::RevealRow(index) => Some(*index),
            _ => None,
        };
        let inspected_record = matches!(&update, UiUpdate::Detail(Some(_)));
        self.state.apply(update);
        if switching_tab || restore_view.is_some() {
            self.restore_tab_scroll(restore_view.unwrap_or_default());
        }
        if refresh_fields && self.state.completion_context.is_some() {
            self.refresh_completion(cx);
        }
        if inspected_record && self.state.show_links {
            self.send(UiAction::LoadLinks);
        }
        if reset_grid_scroll {
            self.grid_list
                .reset_with_uniform_height(self.state.loaded_count, px(30.));
        } else if append && self.state.loaded_count > old_count {
            self.grid_list
                .splice(old_count..old_count, self.state.loaded_count - old_count);
        }
        if let Some(index) = reveal {
            self.grid_list.scroll_to_reveal_item(index);
        }
        if remeasure_grid {
            self.grid_list.remeasure();
        }
        if inspected_record
            && self.state.inspector_visible
            && f32::from(window.bounds().size.width) < 1100.
        {
            self.state.sidebar_visible = false;
        }
        self.restoring_query = false;
        cx.notify();
    }

    /// Read the current query definition without changing its source text.
    fn query_from_inputs(&self, cx: &mut Context<Self>) -> SearchDraft {
        let mut draft = self.state.search.clone();
        draft.full_text = self.inputs.full_text.read(cx).value().to_string();
        draft.structured = self.inputs.structured.read(cx).value().to_string();
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

    fn refresh_completion(&mut self, cx: &mut Context<Self>) {
        self.state.completion_revision += 1;
        self.state.completion_selected = 0;
        self.state.completion_suggestions.clear();
        self.state.completion_pending = false;
        self.state.completion_partial = false;
        if !self.state.search.filter_enabled || self.state.root.is_none() {
            self.state.completion_context = None;
            cx.notify();
            return;
        }
        let input = self.inputs.structured.read(cx);
        let value = input.value().to_string();
        let caret = input.cursor();
        let context = completion::context(&value, caret);
        if let Some(context) = &context {
            match &context.target {
                completion::CompletionTarget::Field { .. } => {
                    let prefix = context.prefix.to_lowercase();
                    self.state.completion_suggestions = self
                        .state
                        .completion_fields
                        .iter()
                        .filter(|choice| choice.path.to_lowercase().starts_with(&prefix))
                        .take(20)
                        .map(|choice| (choice.path.clone(), choice.path.clone()))
                        .collect();
                }
                completion::CompletionTarget::Value { field } => {
                    self.state.completion_pending = true;
                    self.send(UiAction::CompleteValues {
                        revision: self.state.completion_revision,
                        collection: self.state.search.collection.clone(),
                        field: field.clone(),
                        prefix: context.prefix.clone(),
                    });
                }
            }
        }
        self.state.completion_context = context;
        cx.notify();
    }

    fn dismiss_completion(&mut self, cx: &mut Context<Self>) {
        self.state.completion_context = None;
        self.state.completion_suggestions.clear();
        self.state.completion_pending = false;
        self.state.completion_revision += 1;
        cx.notify();
    }

    fn move_completion(&mut self, forward: bool, cx: &mut Context<Self>) -> bool {
        let count = self.state.completion_suggestions.len();
        if self.state.completion_context.is_none() || count == 0 {
            return false;
        }
        self.state.completion_selected = if forward {
            (self.state.completion_selected + 1) % count
        } else {
            (self.state.completion_selected + count - 1) % count
        };
        cx.notify();
        true
    }

    fn accept_completion(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(context) = self.state.completion_context.clone() else {
            return false;
        };
        let Some((insert, _)) = self
            .state
            .completion_suggestions
            .get(self.state.completion_selected)
            .cloned()
        else {
            return false;
        };
        let input = self.inputs.structured.read(cx);
        let value = input.value().to_string();
        if completion::insert(&value, context.replace.clone(), &insert).is_none() {
            return false;
        }
        self.dismiss_completion(cx);
        self.inputs.structured.update(cx, |input, cx| {
            input.set_selected_range(context.replace, cx);
            input.replace(insert, window, cx);
            input.focus(window, cx);
        });
        true
    }

    fn capture_tab_view(&self) -> TabViewState {
        TabViewState {
            raw: self.state.show_raw,
            links: self.state.show_links,
            metadata: self.state.show_detail_metadata,
            fields_y: f32::from(self.fields_scroll.offset().y).round() as i32,
            links_y: f32::from(self.links_scroll.offset().y).round() as i32,
            source_y: f32::from(self.source_scroll.offset().y).round() as i32,
        }
    }

    fn restore_tab_scroll(&self, view: TabViewState) {
        self.fields_scroll
            .set_offset(gpui_kit::point(px(0.), px(view.fields_y as f32)));
        self.links_scroll
            .set_offset(gpui_kit::point(px(0.), px(view.links_y as f32)));
        self.source_scroll
            .set_offset(gpui_kit::point(px(0.), px(view.source_y as f32)));
    }

    fn save_tab_view(&mut self) {
        if let Some(id) = self.state.active_tab {
            let view = self.capture_tab_view();
            self.state.tab_views.insert(id, view);
            self.send(UiAction::SetTabView { id, view });
        }
    }

    /// Cycle one header through the authoritative filter expression.
    fn sort_by_column(&mut self, column: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sort_field) = sort::sort_field_for_column(&column) else {
            self.state.status = "Match snippets cannot be sorted".into();
            cx.notify();
            return;
        };
        let current = self.inputs.structured.read(cx).value().to_string();
        if self.state.search.filter_enabled
            && !current.trim().is_empty()
            && parse_structured_filter(&current).is_err()
        {
            self.state.status = "Correct the Filter expression before sorting".into();
            let focus = self.inputs.structured.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
            cx.notify();
            return;
        }
        // A retained inactive predicate must never become active from a
        // header click. Keep it in the editor until explicitly restored.
        let source = if !self.state.search.filter_enabled && !current.trim().is_empty() {
            self.suspended_filter = Some(current);
            String::new()
        } else {
            current
        };
        let next = match sort::cycle_header_sort(&source, sort_field) {
            Ok(next) => next,
            Err(message) => {
                self.state.status = message;
                cx.notify();
                return;
            }
        };
        self.restoring_query = true;
        self.inputs
            .structured
            .update(cx, |input, cx| input.set_value(next, window, cx));
        self.restoring_query = false;
        self.state.search.filter_enabled = true;
        self.search_from_inputs(false, cx);
    }

    fn send(&mut self, action: UiAction) {
        if self.actions.send(action).is_err() {
            self.state.status = "Desktop controller disconnected".into();
        }
    }

    fn begin_rename(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(title) = self
            .state
            .saved
            .iter()
            .find(|entry| entry.id == Some(id))
            .and_then(|entry| entry.name.clone())
        else {
            return;
        };
        self.renaming_saved = Some(id);
        self.focused_saved = Some(id);
        self.inputs.rename_title.update(cx, |input, cx| {
            input.set_value(title, window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.renaming_saved else {
            return;
        };
        let title = self.inputs.rename_title.read(cx).value().to_string();
        if title.trim().is_empty() {
            self.state.status = "Saved query title cannot be empty".into();
            cx.notify();
            return;
        }
        self.send(UiAction::RenameSaved { id, title });
        self.renaming_saved = None;
        cx.notify();
    }

    fn select_relative(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.state.loaded_count;
        if len == 0 {
            return;
        }
        let next = self.state.page.selected.map_or(0, |current| {
            current.saturating_add_signed(delta).min(len - 1)
        });
        self.state.page.selected = Some(next);
        if self.state.show_preview {
            self.state.inspector_visible = self.state.active_tab.is_some();
        }
        self.grid_list.scroll_to_reveal_item(next);
        self.send(UiAction::VisibleRows {
            start: next,
            end: next + 1,
        });
        self.send(UiAction::SelectRow(next));
        cx.notify();
    }

    fn select_absolute(&mut self, last: bool, cx: &mut Context<Self>) {
        let len = self.state.loaded_count;
        if len == 0 {
            return;
        }
        let index = if last { len - 1 } else { 0 };
        self.state.page.selected = Some(index);
        if self.state.show_preview {
            self.state.inspector_visible = self.state.active_tab.is_some();
        }
        self.grid_list.scroll_to_reveal_item(index);
        self.send(UiAction::VisibleRows {
            start: index,
            end: index + 1,
        });
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
