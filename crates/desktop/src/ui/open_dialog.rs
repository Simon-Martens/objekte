//! Opening-dialog draft lifecycle, separate from the active repository.

use crate::controller::open_detection::DetectedSource;
use gpui_kit::component::{
    Disableable as _, Selectable as _,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    input::{Input, InputEvent, InputState},
    scroll::ScrollableElement as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Focusable as _, InteractiveElement as _, IntoElement,
    ParentElement as _, Styled as _, Window, div, px, rgb, rgba,
};

use super::DesktopView;
use super::model::UiAction;
use super::model::{OpenDraft, OpenMode};


/// Explicit source interpretation selected under Advanced options.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SourceChoice {
    #[default]
    Automatic,
    Folder,
    ConfiguredRepository,
}


/// Advisory mode detection for the current first folder.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum DetectionState {
    #[default]
    Idle,
    Pending {
        path: String,
    },
    Ready {
        path: String,
        source: DetectedSource,
    },
    Failed {
        path: String,
        message: String,
    },
}


/// Editable modal state; none of its changes open data before confirmation.
#[derive(Clone, Debug)]
pub struct OpenDialogState {
    pub visible: bool,
    pub folders: Vec<String>,
    pub source_choice: SourceChoice,
    pub advanced_expanded: bool,
    pub detection: DetectionState,
    pub generation: u64,
    pub picker_generation: u64,
    pub pending_picker_slot: Option<usize>,
    pub pending_schema_picker: bool,
    pub error: Option<String>,
}


impl Default for OpenDialogState {
    fn default() -> Self {
        Self {
            visible: false,
            folders: vec![String::new()],
            source_choice: SourceChoice::Automatic,
            advanced_expanded: false,
            detection: DetectionState::Idle,
            generation: 0,
            picker_generation: 0,
            pending_picker_slot: None,
            pending_schema_picker: false,
            error: None,
        }
    }
}


impl OpenDialogState {
    pub fn show(&mut self) {
        self.visible = true;
        self.source_choice = SourceChoice::Automatic;
        self.advanced_expanded = false;
        self.error = None;
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.generation = self.generation.wrapping_add(1);
        self.picker_generation = self.picker_generation.wrapping_add(1);
        self.pending_picker_slot = None;
        self.pending_schema_picker = false;
        self.detection = DetectionState::Idle;
        self.error = None;
    }

    pub fn edit_folder(&mut self, slot: usize, path: String) -> bool {
        let Some(folder) = self.folders.get_mut(slot) else {
            return false;
        };
        if *folder == path {
            return false;
        }
        *folder = path;
        self.invalidate_detection();
        true
    }

    pub fn add_folder(&mut self) -> usize {
        self.folders.push(String::new());
        self.invalidate_detection();
        self.folders.len() - 1
    }

    pub fn remove_folder(&mut self, slot: usize) -> bool {
        if self.folders.len() <= 1 || slot >= self.folders.len() {
            return false;
        }
        self.folders.remove(slot);
        self.invalidate_detection();
        true
    }

    pub fn begin_detection(&mut self) -> Option<(u64, String)> {
        if !self.visible || self.folders.len() != 1 {
            self.detection = DetectionState::Idle;
            return None;
        }
        let path = self.folders[0].trim().to_owned();
        if path.is_empty() {
            self.detection = DetectionState::Idle;
            return None;
        }
        self.generation = self.generation.wrapping_add(1);
        self.detection = DetectionState::Pending { path: path.clone() };
        Some((self.generation, path))
    }

    pub fn accept_detection(
        &mut self,
        generation: u64,
        path: &str,
        result: Result<DetectedSource, String>,
    ) -> bool {
        if !self.visible
            || self.folders.len() != 1
            || self.generation != generation
            || self.folders[0].trim() != path
            || !matches!(&self.detection, DetectionState::Pending { path: pending } if pending == path)
        {
            return false;
        }
        self.detection = match result {
            Ok(source) => DetectionState::Ready {
                path: path.to_owned(),
                source,
            },
            Err(message) => DetectionState::Failed {
                path: path.to_owned(),
                message,
            },
        };
        true
    }

    pub fn begin_picker(&mut self, slot: usize) -> Option<u64> {
        if !self.visible || slot >= self.folders.len() {
            return None;
        }
        self.picker_generation = self.picker_generation.wrapping_add(1);
        self.pending_picker_slot = Some(slot);
        self.pending_schema_picker = false;
        Some(self.picker_generation)
    }

    pub fn accept_picker(&mut self, generation: u64, slot: usize, path: String) -> bool {
        if !self.visible
            || self.picker_generation != generation
            || self.pending_picker_slot != Some(slot)
        {
            return false;
        }
        self.pending_picker_slot = None;
        self.edit_folder(slot, path);
        true
    }

    pub fn begin_schema_picker(&mut self) -> Option<u64> {
        if !self.visible || self.folders.len() != 1 {
            return None;
        }
        self.picker_generation = self.picker_generation.wrapping_add(1);
        self.pending_picker_slot = None;
        self.pending_schema_picker = true;
        Some(self.picker_generation)
    }

    pub fn accept_schema_picker(&mut self, generation: u64) -> bool {
        if !self.visible || !self.pending_schema_picker || self.picker_generation != generation {
            return false;
        }
        self.pending_schema_picker = false;
        true
    }

    pub fn to_open_draft(&self, template: &OpenDraft) -> Result<OpenDraft, String> {
        if !self.visible {
            return Err("Open dialog is closed".into());
        }
        if self.folders.iter().any(|path| path.trim().is_empty()) {
            return Err("Choose a folder for every row".into());
        }
        if self.folders.iter().any(|path| path.contains(['\n', '\r'])) {
            return Err("Enter one folder path per row".into());
        }
        let mode = if self.folders.len() > 1 {
            OpenMode::MultipleFolders
        } else {
            match self.source_choice {
                SourceChoice::Folder => OpenMode::Folder,
                SourceChoice::ConfiguredRepository => OpenMode::ConfiguredRepository,
                SourceChoice::Automatic => match &self.detection {
                    DetectionState::Ready {
                        source: DetectedSource::PlainFolder,
                        ..
                    } => OpenMode::Folder,
                    DetectionState::Ready {
                        source: DetectedSource::ConfiguredRepository { .. },
                        ..
                    } => OpenMode::ConfiguredRepository,
                    DetectionState::Ready {
                        source: DetectedSource::InvalidConfiguration { message },
                        ..
                    }
                    | DetectionState::Failed { message, .. } => {
                        return Err(format!(
                            "Configured repository could not be read: {message}. Choose Folder under Advanced to override."
                        ));
                    }
                    DetectionState::Idle | DetectionState::Pending { .. } => {
                        return Err("Wait for folder detection before opening".into());
                    }
                },
            }
        };
        let mut draft = template.clone();
        draft.mode = mode;
        if !(mode == OpenMode::ConfiguredRepository && draft.configured_use_repository_settings)
            && draft.snapshot != super::model::SnapshotMode::None
            && draft.snapshot_directory.trim().is_empty()
        {
            return Err("Choose a concrete index location for this opening policy".into());
        }
        draft.paths = self
            .folders
            .iter()
            .map(|path| path.trim())
            .collect::<Vec<_>>()
            .join("\n");
        Ok(draft)
    }

    fn invalidate_detection(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.picker_generation = self.picker_generation.wrapping_add(1);
        self.pending_picker_slot = None;
        self.pending_schema_picker = false;
        self.detection = DetectionState::Idle;
        self.error = None;
    }
}


impl DesktopView {
    pub(super) fn render_open_dialog(
        &self,
        window_width: f32,
        window_height: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let dark = self.state.dark;
        let (panel, raised, border, text, secondary, error) = if dark {
            (0x1c1f24, 0x24282e, 0x30353c, 0xe5e7eb, 0xa0a7b1, 0xf2a59b)
        } else {
            (0xffffff, 0xf2f4f7, 0xd4d9e1, 0x1c2430, 0x637083, 0xb42318)
        };
        let folders = &self.state.open_dialog.folders;
        let mut rows = Vec::with_capacity(folders.len());
        for slot in 0..folders.len() {
            let input = if slot == 0 {
                &self.inputs.open_folder
            } else {
                &self.inputs.open_extra_folders[slot - 1]
            };
            let mut row = div()
                .flex()
                .items_center()
                .gap_2()
                .child(Input::new(input).id(format!("open-folder-{slot}")).w_full())
                .child(
                    Button::new(format!("browse-folder-{slot}"))
                        .label("Browse…")
                        .on_click(cx.listener(move |this, _, _, _| this.pick_open_folder(slot))),
                );
            if slot > 0 {
                row = row.child(
                    Button::new(format!("remove-folder-{slot}"))
                        .label("Remove")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.remove_open_folder(slot, window, cx)
                        })),
                );
            }
            rows.push(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(if slot == 0 {
                                "Folder".to_owned()
                            } else {
                                format!("Folder {}", slot + 1)
                            }),
                    )
                    .child(row)
                    .into_any_element(),
            );
        }
        let detection = if folders.len() > 1 {
            Some((
                "Multiple folders".to_owned(),
                "Each folder becomes one collection. Configured repositories are not merged."
                    .to_owned(),
                false,
            ))
        } else {
            match &self.state.open_dialog.detection {
                DetectionState::Idle => None,
                DetectionState::Pending { .. } => Some((
                    "Checking folder…".to_owned(),
                    "Looking for Objekte collection definitions.".to_owned(),
                    false,
                )),
                DetectionState::Ready {
                    source: DetectedSource::PlainFolder,
                    ..
                } => Some((
                    "Plain folder".to_owned(),
                    "JSON files in this folder will be searched.".to_owned(),
                    false,
                )),
                DetectionState::Ready {
                    source: DetectedSource::ConfiguredRepository { collections, schema_files, default_cache },
                    ..
                } => Some((
                    "Objekte repository detected".to_owned(),
                    format!(
                        "{collections} configured collections · Schemas: {} · Default index: {default_cache}. Use repository settings to load a compatible index without writing automatically.",
                        if schema_files.is_empty() { "none".to_owned() } else { schema_files.join(", ") },
                    ),
                    false,
                )),
                DetectionState::Ready {
                    source: DetectedSource::InvalidConfiguration { message },
                    ..
                }
                | DetectionState::Failed { message, .. } => Some((
                    "Repository configuration could not be read".to_owned(),
                    format!("{message} Select Folder under Advanced to override."),
                    true,
                )),
            }
        };
        let source_choice = self.state.open_dialog.source_choice;
        let folder_mode = folders.len() == 1
            && (source_choice == SourceChoice::Folder
                || source_choice == SourceChoice::Automatic
                    && matches!(
                        self.state.open_dialog.detection,
                        DetectionState::Ready {
                            source: DetectedSource::PlainFolder,
                            ..
                        }
                    ));
        let stored = self.state.open.snapshot;
        let configured_mode = folders.len() == 1
            && (source_choice == SourceChoice::ConfiguredRepository
                || source_choice == SourceChoice::Automatic
                    && matches!(
                        self.state.open_dialog.detection,
                        DetectionState::Ready {
                            source: DetectedSource::ConfiguredRepository { .. },
                            ..
                        }
                    ));
        let use_repository_settings = configured_mode
            && self.state.open.configured_use_repository_settings;
        let can_open = self
            .state
            .open_dialog
            .to_open_draft(&self.state.open)
            .is_ok()
            && self.state.busy != super::state::Busy::Opening;
        let surface_width = (window_width - 24.).clamp(320., 600.);
        let surface_height = (window_height - 24.).clamp(160., 700.);
        let advanced_summary = if use_repository_settings {
            "Configured repository settings will select schemas and a compatible stored index. No automatic index write is enabled.".to_owned()
        } else if self.state.open.snapshot == super::model::SnapshotMode::None {
            "Source files are read-only. No index file will be saved.".to_owned()
        } else {
            format!("Source files are read-only. Stored index: {:?}.", stored)
        };
        let mut content = div()
            .id("open-dialog-content")
            .max_h(px((surface_height - 120.).clamp(40., 550.)))
            .overflow_y_scrollbar()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(secondary))
                    .child("Choose a folder to browse and search its records."),
            )
            .children(rows)
            .child(
                Button::new("open-add-folder")
                    .label("+ Add another folder")
                    .on_click(cx.listener(|this, _, window, cx| this.add_open_folder(window, cx))),
            );
        if let Some((heading, detail, invalid)) = detection {
            content = content.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_3()
                    .bg(rgb(raised))
                    .rounded_md()
                    .text_color(rgb(if invalid { error } else { text }))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(heading),
                    )
                    .child(div().text_xs().child(detail)),
            );
        }
        content = content.child(div().h(px(1.)).bg(rgb(border))).child(
            Button::new("open-advanced-toggle")
                .label(if self.state.open_dialog.advanced_expanded {
                    "▾ Advanced options"
                } else {
                    "› Advanced options"
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state.open_dialog.advanced_expanded =
                        !this.state.open_dialog.advanced_expanded;
                    cx.notify();
                })),
        );
        if self.state.open_dialog.advanced_expanded {
            content = content.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child("Source interpretation"),
                    )
                    .when(folders.len() > 1, |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(rgb(secondary))
                                .child("Multiple folders open as separate collections."),
                        )
                    })
                    .when(folders.len() == 1, |this| {
                        this.child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_2()
                                .child(
                                    Button::new("open-source-auto")
                                        .label("Automatic")
                                        .selected(source_choice == SourceChoice::Automatic)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.state.open_dialog.source_choice =
                                                SourceChoice::Automatic;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new("open-source-folder")
                                        .label("Folder")
                                        .selected(source_choice == SourceChoice::Folder)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.state.open_dialog.source_choice =
                                                SourceChoice::Folder;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new("open-source-configured")
                                        .label("Configured repository")
                                        .selected(
                                            source_choice == SourceChoice::ConfiguredRepository,
                                        )
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.state.open_dialog.source_choice =
                                                SourceChoice::ConfiguredRepository;
                                            cx.notify();
                                        })),
                                ),
                        )
                    }),
            );
            if folder_mode || folders.len() > 1 {
                content = content.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child("Folder discovery"),
                        )
                        .child(
                            Checkbox::new("open-recursive")
                                .accessibility_label("Include subfolders")
                                .checked(self.state.open.recursive)
                                .child("Include subfolders")
                                .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                    this.state.open.recursive = *checked;
                                    cx.notify();
                                })),
                        )
                        .when(folder_mode, |this| {
                            this.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        Input::new(&self.inputs.schema).id("open-schema").w_full(),
                                    )
                                    .child(
                                        Button::new("open-schema-browse")
                                            .label("Browse…")
                                            .on_click(
                                                cx.listener(|this, _, _, _| {
                                                    this.pick_open_schema()
                                                }),
                                            ),
                                    ),
                            )
                        }),
                );
            }
            content = content
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child("Index construction"),
                        )
                        .child(
                            Checkbox::new("open-auto-index")
                                .accessibility_label("Infer field indexes")
                                .checked(self.state.open.auto_index)
                                .child("Infer field indexes")
                                .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                    this.state.open.auto_index = *checked;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Checkbox::new("open-unicode-index")
                                .accessibility_label("Unicode index alphabet")
                                .checked(self.state.open.unicode_index)
                                .child("Unicode index alphabet")
                                .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                    this.state.open.unicode_index = *checked;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Checkbox::new("open-diagnostics")
                                .accessibility_label("Collect detailed diagnostics")
                                .checked(self.state.open.diagnostic)
                                .child("Collect detailed diagnostics")
                                .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                    this.state.open.diagnostic = *checked;
                                    cx.notify();
                                })),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child("Stored index"),
                        )
                        .when(configured_mode, |this| {
                            this.child(
                                Button::new("open-use-repository-settings")
                                    .label("Use repository settings")
                                    .selected(use_repository_settings)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.state.open.configured_use_repository_settings = true;
                                        cx.notify();
                                    })),
                            )
                        })
                        .child(
                            div().flex().flex_wrap().gap_2().children(
                                [
                                    (super::model::SnapshotMode::None, "Build in memory"),
                                    (super::model::SnapshotMode::Load, "Load selected index"),
                                    (super::model::SnapshotMode::Write, "Build and save"),
                                    (
                                        super::model::SnapshotMode::LoadAndWrite,
                                        "Load or rebuild and save",
                                    ),
                                ]
                                .into_iter()
                                .map(|(mode, label)| {
                                    Button::new(format!("open-snapshot-{mode:?}"))
                                        .label(label)
                                        .selected(!use_repository_settings && stored == mode)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.state.open.snapshot = mode;
                                            this.state.open.configured_use_repository_settings = false;
                                            cx.notify();
                                        }))
                                }),
                            ),
                        )
                        .when(!use_repository_settings && stored != super::model::SnapshotMode::None, |this| {
                            this.child(
                                Input::new(&self.inputs.snapshot_directory)
                                    .id("open-index-directory")
                                    .w_full(),
                            )
                            .child(
                                div().text_xs().text_color(rgb(secondary)).child(
                                    "Choose the exact index file or legacy index directory. The selected location will be shown after opening.",
                                ),
                            )
                        }),
                );
        }
        content = content.child(
            div()
                .text_xs()
                .text_color(rgb(secondary))
                .child(advanced_summary),
        );
        if let Some(message) = &self.state.open_dialog.error {
            content = content.child(
                div()
                    .id("open-dialog-error")
                    .text_sm()
                    .text_color(rgb(error))
                    .child(message.clone()),
            );
        }
        let surface = div()
            .id("open-data-dialog")
            .w(px(surface_width))
            .max_h(px(surface_height))
            .flex()
            .flex_col()
            .bg(rgb(panel))
            .border_1()
            .border_color(rgb(border))
            .rounded_md()
            .text_color(rgb(text))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .py_3()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child("Open data"),
                    )
                    .child(
                        Button::new("open-dialog-close")
                            .label("×")
                            .on_click(cx.listener(|this, _, _, cx| this.close_open_dialog(cx))),
                    ),
            )
            .child(content)
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(rgb(border))
                    .child(
                        Button::new("open-dialog-cancel")
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| this.close_open_dialog(cx))),
                    )
                    .child(
                        Button::new("open-dialog-confirm")
                            .primary()
                            .label("Open")
                            .disabled(!can_open)
                            .on_click(cx.listener(|this, _, _, cx| this.submit_open_dialog(cx))),
                    ),
            );
        let cancel_view = cx.entity();
        let confirm_view = cx.entity();
        gpui_kit::base::Dialog::new(cx)
            .open(self.state.open_dialog.visible)
            .close_on_backdrop_press(false)
            .backdrop(div().bg(rgba(0x00000099)))
            .popup(surface)
            .on_cancel(move |_, _, cx| {
                cancel_view.update(cx, |view, cx| view.close_open_dialog(cx));
                true
            })
            .on_ok(move |_, _, cx| {
                confirm_view.update(cx, |view, cx| view.submit_open_dialog(cx));
                false
            })
    }

    pub(super) fn show_open_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.open_dialog.folders.len() == 1
            && self.state.open_dialog.folders[0].is_empty()
            && !self.state.open.paths.trim().is_empty()
        {
            self.state.open_dialog.folders =
                self.state.open.paths.lines().map(str::to_owned).collect();
        }
        self.state.open_dialog.show();
        let first = self.state.open_dialog.folders[0].clone();
        self.inputs
            .open_folder
            .update(cx, |input, cx| input.set_value(first, window, cx));
        while self.inputs.open_extra_folders.len() < self.state.open_dialog.folders.len() - 1 {
            self.add_open_folder_input(window, cx);
        }
        while self.inputs.open_extra_folders.len() > self.state.open_dialog.folders.len() - 1 {
            self.inputs.open_extra_folders.pop();
            self.open_extra_subscriptions.pop();
        }
        for (input, path) in self
            .inputs
            .open_extra_folders
            .iter()
            .zip(self.state.open_dialog.folders.iter().skip(1))
        {
            input.update(cx, |input, cx| input.set_value(path.clone(), window, cx));
        }
        self.start_open_detection();
        let focus = self.inputs.open_folder.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(super) fn close_open_dialog(&mut self, cx: &mut Context<Self>) {
        self.send(UiAction::CancelOpen);
        self.state.open_dialog.close();
        cx.notify();
    }

    pub(super) fn open_folder_edited(&mut self, slot: usize, cx: &mut Context<Self>) {
        if !self.state.open_dialog.visible {
            return;
        }
        let input = if slot == 0 {
            Some(&self.inputs.open_folder)
        } else {
            self.inputs.open_extra_folders.get(slot - 1)
        };
        let Some(input) = input else {
            return;
        };
        let text = input.read(cx).value().to_string();
        if self.state.open_dialog.edit_folder(slot, text) {
            self.start_open_detection();
            cx.notify();
        }
    }

    pub(super) fn start_open_detection(&mut self) {
        if let Some((generation, path)) = self.state.open_dialog.begin_detection() {
            self.send(UiAction::DetectOpen { path, generation });
        }
    }

    pub(super) fn add_open_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.open_dialog.add_folder();
        self.add_open_folder_input(window, cx);
        if let Some(input) = self.inputs.open_extra_folders.last() {
            let focus = input.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    fn add_open_folder_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let slot = self.inputs.open_extra_folders.len() + 1;
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Additional folder"));
        self.open_extra_subscriptions.push(cx.subscribe(
            &input,
            move |this: &mut Self, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    this.open_folder_edited(slot, cx);
                }
            },
        ));
        self.inputs.open_extra_folders.push(input);
    }

    pub(super) fn remove_open_folder(
        &mut self,
        slot: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.open_dialog.remove_folder(slot) {
            let remaining = self
                .state
                .open_dialog
                .folders
                .iter()
                .skip(1)
                .cloned()
                .collect::<Vec<_>>();
            self.inputs.open_extra_folders.clear();
            self.open_extra_subscriptions.clear();
            for path in remaining {
                self.add_open_folder_input(window, cx);
                if let Some(input) = self.inputs.open_extra_folders.last() {
                    input.update(cx, |input, cx| input.set_value(path, window, cx));
                }
            }
            self.start_open_detection();
            cx.notify();
        }
    }

    pub(super) fn pick_open_folder(&mut self, slot: usize) {
        if let Some(generation) = self.state.open_dialog.begin_picker(slot) {
            self.send(UiAction::ChooseFolder {
                draft: self.state.open.clone(),
                slot,
                generation,
            });
        }
    }

    pub(super) fn pick_open_schema(&mut self) {
        if let Some(generation) = self.state.open_dialog.begin_schema_picker() {
            self.send(UiAction::ChooseSchema { generation });
        }
    }

    pub(super) fn submit_open_dialog(&mut self, cx: &mut Context<Self>) {
        let mut template = self.state.open.clone();
        template.schema = self.inputs.schema.read(cx).value().to_string();
        template.snapshot_directory = self.inputs.snapshot_directory.read(cx).value().to_string();
        match self.state.open_dialog.to_open_draft(&template) {
            Ok(draft) => {
                self.state.open = draft.clone();
                self.state.open_dialog.error = None;
                self.send(UiAction::Open(draft));
            }
            Err(message) => self.state.open_dialog.error = Some(message),
        }
        cx.notify();
    }
}


#[cfg(test)]
#[path = "open_dialog.test.rs"]
mod tests;
