//! GPUI presentation and interaction state for the desktop application.

pub mod model;
mod panels;
pub mod preferences;
mod render;
mod state;

use std::sync::mpsc::Sender;

use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::{AppContext as _, Context, Entity, Subscription, Window};

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
}


/// Native read-only workspace; the controller owns repository and worker state.
pub struct DesktopView {
    actions: Sender<UiAction>,
    state: UiState,
    inputs: Inputs,
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
        let mut state = UiState::default();
        state.show_preview = true;
        state.status = "Open a folder or configured repository to begin".into();
        Self {
            actions,
            state,
            inputs,
            _subscriptions: subscriptions,
        }
    }

    /// Apply one controller-validated update; only bounded display data remains.
    pub fn apply(&mut self, update: UiUpdate, window: &mut Window, cx: &mut Context<Self>) {
        if let UiUpdate::ChosenPaths(paths) = &update {
            self.inputs
                .paths
                .update(cx, |input, cx| input.set_value(paths.clone(), window, cx));
        }
        if let UiUpdate::RestoreQuery(draft) = &update {
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
        self.state.apply(update);
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
}
