//! Ordinary controls for opening, inspection, history, and output actions.

use gpui_kit::component::{
    Disableable as _, IconName, Selectable as _,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    input::{Input, Textarea},
    scroll::ScrollableElement as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, div, px, rgb,
};

use super::DesktopView;
use super::model::{
    BatchSource, DetailOrigin, DetailValueKind, ErrorField, ExportFormat, OpenMode, SnapshotMode,
    UiAction,
};


impl DesktopView {
    pub(super) fn render_open_options(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let open_error = self.state.errors.get(&ErrorField::Open).cloned();
        div()
            .id("open-options")
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(rgb(if self.state.dark { 0x202a35 } else { 0xe8f0f7 }))
            .child(
                div()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child("Open repository"),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("mode-folder")
                            .icon(IconName::FolderClosed)
                            .label("One folder")
                            .selected(self.state.open.mode == OpenMode::Folder)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.open.mode = OpenMode::Folder;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("mode-multiple")
                            .icon(IconName::FolderOpen)
                            .label("Multiple folders")
                            .selected(self.state.open.mode == OpenMode::MultipleFolders)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.open.mode = OpenMode::MultipleFolders;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("mode-repo")
                            .icon(IconName::LayoutDashboard)
                            .label("Configured repository")
                            .selected(self.state.open.mode == OpenMode::ConfiguredRepository)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.open.mode = OpenMode::ConfiguredRepository;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("choose-folder")
                            .icon(IconName::FolderOpen)
                            .label("Choose folder…")
                            .on_click(cx.listener(|this, _, _, cx| this.choose_folder(cx))),
                    ),
            )
            .child(div().text_sm().child(match self.state.open.mode {
                OpenMode::Folder => "Folder path",
                OpenMode::MultipleFolders => "One folder path per line",
                OpenMode::ConfiguredRepository => "Repository root path",
            }))
            .child(
                div()
                    .id("open-paths")
                    .child(Textarea::new(&self.inputs.paths).w_full().h(px(70.))),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Input::new(&self.inputs.schema)
                            .id("schema-path")
                            .w(px(270.)),
                    )
                    .child(
                        Checkbox::new("recursive")
                            .accessibility_label("Recursive")
                            .child(
                                div()
                                    .text_color(rgb(if self.state.dark {
                                        0xe2e8f0
                                    } else {
                                        0x172033
                                    }))
                                    .child("Recursive"),
                            )
                            .checked(self.state.open.recursive)
                            .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                this.state.open.recursive = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        Checkbox::new("auto-index")
                            .accessibility_label("Auto index")
                            .child(
                                div()
                                    .text_color(rgb(if self.state.dark {
                                        0xe2e8f0
                                    } else {
                                        0x172033
                                    }))
                                    .child("Auto index"),
                            )
                            .checked(self.state.open.auto_index)
                            .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                this.state.open.auto_index = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        Checkbox::new("unicode-index")
                            .accessibility_label("Unicode index alphabet")
                            .child(
                                div()
                                    .text_color(rgb(if self.state.dark {
                                        0xe2e8f0
                                    } else {
                                        0x172033
                                    }))
                                    .child("Unicode index alphabet"),
                            )
                            .checked(self.state.open.unicode_index)
                            .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                this.state.open.unicode_index = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        Checkbox::new("diagnostics")
                            .accessibility_label("Diagnostics")
                            .child(
                                div()
                                    .text_color(rgb(if self.state.dark {
                                        0xe2e8f0
                                    } else {
                                        0x172033
                                    }))
                                    .child("Diagnostics"),
                            )
                            .checked(self.state.open.diagnostic)
                            .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                this.state.open.diagnostic = *checked;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(div().text_sm().child("Snapshot"))
                    .child(
                        Button::new("snapshot-none")
                            .label("None")
                            .selected(self.state.open.snapshot == SnapshotMode::None)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.open.snapshot = SnapshotMode::None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("snapshot-load")
                            .label("Load")
                            .selected(self.state.open.snapshot == SnapshotMode::Load)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.open.snapshot = SnapshotMode::Load;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("snapshot-write")
                            .label("Write")
                            .selected(self.state.open.snapshot == SnapshotMode::Write)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.open.snapshot = SnapshotMode::Write;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("snapshot-both")
                            .label("Load + Write")
                            .selected(self.state.open.snapshot == SnapshotMode::LoadAndWrite)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.open.snapshot = SnapshotMode::LoadAndWrite;
                                cx.notify();
                            })),
                    )
                    .child(
                        Input::new(&self.inputs.snapshot_directory)
                            .id("snapshot-directory")
                            .w(px(230.)),
                    )
                    .child(
                        Button::new("open-repository")
                            .primary()
                            .icon(IconName::FolderOpen)
                            .label("Open")
                            .on_click(cx.listener(|this, _, _, cx| this.open_from_inputs(cx))),
                    )
                    .child(
                        Button::new("write-index")
                            .icon(IconName::HardDrive)
                            .label("Write Index")
                            .on_click(cx.listener(|this, _, _, _| this.send(UiAction::WriteIndex))),
                    ),
            )
            .when(open_error.is_some(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(rgb(0xc2410c))
                        .child(open_error.unwrap_or_default()),
                )
            })
    }

    pub(super) fn render_batch(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let error = self.state.errors.get(&ErrorField::Batch).cloned();
        let rows = self.state.batch.clone();
        let outcome = self.state.batch_outcome.clone();
        div()
            .id("batch-panel")
            .absolute()
            .top(px(100.))
            .left(px(300.))
            .w(px(640.))
            .max_h(px(600.))
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(rgb(super::theme::palette(self.state.dark).panel))
            .border_1()
            .border_color(rgb(super::theme::palette(self.state.dark).border))
            .rounded_md()
            .child(div().child("Query batch: paste one query per physical line or choose a file"))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("batch-source-paste")
                            .label(if self.state.batch_source == BatchSource::PastedText {
                                "● Pasted lines"
                            } else {
                                "Pasted lines"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.batch_source = BatchSource::PastedText;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("batch-source-file")
                            .label(if self.state.batch_source == BatchSource::QueryFile {
                                "● Query file"
                            } else {
                                "Query file"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.batch_source = BatchSource::QueryFile;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                Input::new(&self.inputs.batch_path)
                    .id("batch-file-path")
                    .w_full(),
            )
            .child(
                div()
                    .id("batch-text")
                    .child(Textarea::new(&self.inputs.batch_text).w_full().h(px(100.))),
            )
            .child(
                Input::new(&self.inputs.batch_output_path)
                    .id("batch-output-path")
                    .w_full(),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("run-batch")
                            .primary()
                            .icon(IconName::Play)
                            .label("Run Batch")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.send(UiAction::RunBatch {
                                    source: this.state.batch_source,
                                    text: this.inputs.batch_text.read(cx).value().to_string(),
                                    path: this.inputs.batch_path.read(cx).value().to_string(),
                                    output_path: this
                                        .inputs
                                        .batch_output_path
                                        .read(cx)
                                        .value()
                                        .to_string(),
                                });
                            })),
                    )
                    .child(
                        Button::new("cancel-batch")
                            .icon(IconName::Close)
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, _| this.send(UiAction::Cancel))),
                    ),
            )
            .when(error.is_some(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(rgb(0xc2410c))
                        .child(error.unwrap_or_default()),
                )
            })
            .when(outcome.is_some(), |this| {
                this.child(div().text_sm().child(outcome.unwrap_or_default()))
            })
            .children(rows.into_iter().map(|row| {
                div().text_sm().child(format!(
                    "Line {}: {} — {}",
                    row.line,
                    row.query,
                    row.error.unwrap_or(row.status)
                ))
            }))
    }

    pub(super) fn render_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = super::theme::palette(self.state.dark);
        let detail = self.state.detail.as_ref();
        let preview = self
            .state
            .page
            .selected
            .and_then(|index| self.state.page.rows.get(index))
            .and_then(|row| row.preview.as_ref());
        let title = detail.map_or_else(|| "Record".to_owned(), |detail| detail.title.clone());
        let mut panel = div()
            .id("record-panel")
            .track_focus(&self.detail_focus)
            .on_key_down(
                cx.listener(|this, event: &gpui_kit::KeyDownEvent, window, cx| {
                    if window.focused(cx).as_ref() != Some(&this.detail_focus) {
                        return;
                    }
                    match event.keystroke.key.as_str() {
                        "up" => this.select_detail_relative(-1, cx),
                        "down" => this.select_detail_relative(1, cx),
                        "enter" => this.activate_detail_row(),
                        _ => return,
                    }
                    cx.stop_propagation();
                }),
            )
            .flex()
            .flex_col()
            .w(px(self.state.inspector_width))
            .h_full()
            .flex_shrink_0()
            .relative()
            .bg(rgb(colors.panel))
            .border_l_1()
            .border_color(rgb(colors.border))
            .child(
                div()
                    .id("inspector-resize")
                    .absolute()
                    .left_0()
                    .top_0()
                    .h_full()
                    .w(px(5.))
                    .cursor_col_resize()
                    .on_mouse_down(
                        gpui_kit::MouseButton::Left,
                        cx.listener(|this, event: &gpui_kit::MouseDownEvent, _, cx| {
                            this.state.inspector_resize_origin =
                                Some((f32::from(event.position.x), this.state.inspector_width));
                            cx.stop_propagation();
                        }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(40.))
                    .flex_shrink_0()
                    .px_3()
                    .gap_1()
                    .border_b_1()
                    .border_color(rgb(colors.border))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        Button::new("previous-record")
                            .ghost()
                            .icon(IconName::ArrowLeft)
                            .tooltip("Previous result")
                            .accessibility_label("Previous result")
                            .on_click(cx.listener(|this, _, _, _| {
                                this.send(UiAction::AdjacentResult(false));
                            })),
                    )
                    .child(
                        Button::new("next-record")
                            .ghost()
                            .icon(IconName::ArrowRight)
                            .tooltip("Next result")
                            .accessibility_label("Next result")
                            .on_click(cx.listener(|this, _, _, _| {
                                this.send(UiAction::AdjacentResult(true));
                            })),
                    )
                    .child(
                        Button::new("relation-back")
                            .ghost()
                            .icon(IconName::Undo)
                            .tooltip("Back from relation")
                            .accessibility_label("Back from relation")
                            .on_click(cx.listener(|this, _, _, _| {
                                this.send(UiAction::BackRelation);
                            })),
                    )
                    .child(
                        Button::new("close-detail")
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip("Close inspector")
                            .accessibility_label("Close inspector")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.inspector_visible = false;
                                this.send(UiAction::CloseDetail);
                                cx.notify();
                            })),
                    ),
            );
        let Some(detail) = detail else {
            return panel
                .child(
                    div()
                        .p_3()
                        .text_sm()
                        .text_color(rgb(colors.secondary))
                        .child(preview.cloned().unwrap_or_else(|| "Loading record…".into())),
                )
                .into_any_element();
        };
        panel = panel
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(36.))
                    .flex_shrink_0()
                    .px_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(rgb(colors.border))
                    .child(
                        Button::new("fields-tab")
                            .ghost()
                            .label("Fields")
                            .selected(!self.state.show_raw)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.show_raw = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("source-tab")
                            .ghost()
                            .label("Source")
                            .selected(self.state.show_raw)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.show_raw = true;
                                this.send(UiAction::ToggleRawSource);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(42.))
                    .flex_shrink_0()
                    .px_2()
                    .gap_1()
                    .child(Input::new(&self.inputs.find).id("record-find").w_full())
                    .child(
                        Button::new("find-previous")
                            .ghost()
                            .label("↑")
                            .tooltip("Previous match in record")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.send(UiAction::FindInRecord {
                                    text: this.inputs.find.read(cx).value().to_string(),
                                    forward: false,
                                });
                            })),
                    )
                    .child(
                        Button::new("find-next")
                            .ghost()
                            .label("↓")
                            .tooltip("Next match in record")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.send(UiAction::FindInRecord {
                                    text: this.inputs.find.read(cx).value().to_string(),
                                    forward: true,
                                });
                            })),
                    ),
            );
        if self.state.show_raw {
            panel = panel
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .text_xs()
                        .text_color(rgb(colors.secondary))
                        .child(detail.source_label.clone()),
                )
                .child(
                    div()
                        .id("raw-source")
                        .flex_1()
                        .min_h_0()
                        .p_3()
                        .font_family("DejaVu Sans Mono")
                        .text_sm()
                        .child(self.state.detail_source.clone().unwrap_or_default())
                        .overflow_y_scrollbar(),
                );
        } else {
            let mut content = div()
                .id("structured-detail")
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0();
            content = content.children(
                detail
                    .structured
                    .iter()
                    .enumerate()
                    .filter(|(_, row)| {
                        (row.origin == DetailOrigin::Structured
                            && !(row.depth == 0 && row.label == "$" && detail.structured.len() > 1))
                            || (self.state.show_detail_metadata
                                && row.origin != DetailOrigin::Structured)
                    })
                    .map(|(index, row)| {
                        let value = match row.value_kind {
                            DetailValueKind::Present => self
                                .state
                                .detail_cache
                                .values
                                .get(index)
                                .cloned()
                                .unwrap_or_default(),
                            DetailValueKind::JsonNull => "null (JSON)".into(),
                            DetailValueKind::LogicalUnavailable => {
                                "unavailable logical data".into()
                            }
                        };
                        let marker = if row.expandable {
                            if row.expanded { "▾" } else { "▸" }
                        } else {
                            " "
                        };
                        let expandable = row.expandable;
                        div()
                            .id(("detail-row", index))
                            .flex()
                            .flex_col()
                            .gap_1()
                            .px_3()
                            .py_2()
                            .pl(px(12. + row.depth.min(10) as f32 * 12.))
                            .border_b_1()
                            .border_color(rgb(colors.border))
                            .bg(rgb(if detail.selected_row == index {
                                colors.selected
                            } else {
                                colors.panel
                            }))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .text_xs()
                                    .text_color(rgb(colors.secondary))
                                    .child(marker)
                                    .child(
                                        self.state
                                            .detail_cache
                                            .labels
                                            .get(index)
                                            .cloned()
                                            .unwrap_or_default(),
                                    )
                                    .when(row.matched, |this| this.child("· match")),
                            )
                            .child(div().text_sm().child(value))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                window.focus(&this.detail_focus, cx);
                                if let Some(detail) = this.state.detail.as_mut() {
                                    detail.selected_row = index;
                                }
                                this.send(UiAction::DetailRowSelected(index));
                                if expandable {
                                    this.send(UiAction::ToggleDetailExpansion(index));
                                }
                                cx.notify();
                            }))
                    }),
            );
            content = content.children(detail.relations.iter().enumerate().map(
                |(index, relation)| {
                    let label = self
                        .state
                        .detail_cache
                        .relation_labels
                        .get(index)
                        .cloned()
                        .unwrap_or_default();
                    let value = self
                        .state
                        .detail_cache
                        .relation_values
                        .get(index)
                        .cloned()
                        .unwrap_or_default();
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_3()
                        .py_1()
                        .border_b_1()
                        .border_color(rgb(colors.border))
                        .child(
                            Button::new(format!("relation-{index}"))
                                .ghost()
                                .disabled(!relation.available)
                                .child(div().flex().gap_1().child(label).child("↗").child(value))
                                .on_click(cx.listener(move |this, _, _, _| {
                                    this.send(UiAction::FollowRelation(index));
                                })),
                        )
                        .child(
                            Button::new(format!("copy-relation-{index}"))
                                .ghost()
                                .icon(IconName::Copy)
                                .tooltip("Copy relation value")
                                .accessibility_label("Copy relation value")
                                .on_click(cx.listener(move |this, _, _, _| {
                                    this.send(UiAction::CopyRelation(index));
                                })),
                        )
                },
            ));
            panel = panel.child(content.overflow_y_scrollbar()).child(
                Button::new("metadata-toggle")
                    .ghost()
                    .label(if self.state.show_detail_metadata {
                        "▾ Metadata & diagnostics"
                    } else {
                        "▸ Metadata & diagnostics"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.show_detail_metadata = !this.state.show_detail_metadata;
                        cx.notify();
                    })),
            );
        }
        panel
            .when(detail.clipped, |this| {
                this.child(
                    div()
                        .px_3()
                        .text_xs()
                        .text_color(rgb(colors.secondary))
                        .child("Visible values clipped; copy and export use complete records."),
                )
            })
            .when(
                self.state.show_detail_metadata && detail.diagnostic.is_some(),
                |this| {
                    this.child(
                        div()
                            .px_3()
                            .text_xs()
                            .text_color(rgb(colors.error))
                            .child(detail.diagnostic.clone().unwrap_or_default()),
                    )
                },
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(42.))
                    .flex_shrink_0()
                    .px_2()
                    .gap_1()
                    .border_t_1()
                    .border_color(rgb(colors.border))
                    .child(
                        Button::new("copy-value")
                            .ghost()
                            .icon(IconName::Copy)
                            .label("Copy value")
                            .on_click(cx.listener(|this, _, _, _| {
                                this.send(if this.state.show_raw {
                                    UiAction::CopyRecord
                                } else {
                                    UiAction::CopySelectedValue
                                });
                            })),
                    )
                    .child(
                        Button::new("copy-record")
                            .ghost()
                            .label("Copy record")
                            .on_click(cx.listener(|this, _, _, _| {
                                this.send(UiAction::CopyRecord);
                            })),
                    )
                    .child(
                        Button::new("inspector-export")
                            .ghost()
                            .label("Export…")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.show_actions = true;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_actions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let history = self.state.history.clone();
        let saved = self.state.saved.clone();
        let columns = self.state.available_columns.clone();
        let hidden_columns = self.state.hidden_columns.clone();
        let export_error = self.state.errors.get(&ErrorField::Export).cloned();
        div()
            .id("action-panel")
            .absolute()
            .top(px(48.))
            .right(px(8.))
            .w(px(700.))
            .max_h(px(560.))
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(rgb(super::theme::palette(self.state.dark).panel))
            .border_1()
            .border_color(rgb(super::theme::palette(self.state.dark).border))
            .rounded_md()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.state.show_actions = false;
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child("Menu"),
                    )
                    .child(
                        Button::new("menu-batch")
                            .icon(IconName::SquareTerminal)
                            .label("Run query batch…")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.show_actions = false;
                                this.state.show_batch = true;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("menu-index")
                            .icon(IconName::HardDrive)
                            .label("Index…")
                            .on_click(cx.listener(|this, _, _, _| {
                                this.send(UiAction::WriteIndex);
                            })),
                    )
                    .child(
                        Button::new("menu-theme")
                            .icon(if self.state.dark {
                                IconName::Sun
                            } else {
                                IconName::Moon
                            })
                            .label(if self.state.dark { "Light" } else { "Dark" })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.dark = !this.state.dark;
                                gpui_kit::component::Theme::change(
                                    if this.state.dark {
                                        gpui_kit::component::ThemeMode::Dark
                                    } else {
                                        gpui_kit::component::ThemeMode::Light
                                    },
                                    None,
                                    cx,
                                );
                                this.send(UiAction::SetTheme {
                                    dark: this.state.dark,
                                });
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("menu-help")
                            .icon(IconName::Info)
                            .label("Help")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.show_help = true;
                                this.state.show_actions = false;
                                this.send(UiAction::ShowHelp);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("menu-close")
                            .icon(IconName::Close)
                            .tooltip("Close menu")
                            .accessibility_label("Close menu")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.show_actions = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("history-button")
                            .label("History")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.show_history = !this.state.show_history;
                                cx.notify();
                            })),
                    )
                    .child(Button::new("saved-button").label("Saved Queries").on_click(
                        cx.listener(|this, _, _, cx| {
                            this.state.show_saved = !this.state.show_saved;
                            cx.notify();
                        }),
                    ))
                    .child(
                        Input::new(&self.inputs.query_name)
                            .id("query-name")
                            .w(px(170.)),
                    )
                    .child(
                        Button::new("save-query")
                            .label("Save Query")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.send(UiAction::SaveQuery {
                                    name: this.inputs.query_name.read(cx).value().to_string(),
                                    draft: this.query_from_inputs(cx),
                                });
                            })),
                    ),
            )
            .when(self.state.show_history, |this| {
                this.children(history.into_iter().rev().enumerate().map(|(index, entry)| {
                    Button::new(format!("history-{index}"))
                        .label(format!(
                            "{} | {}",
                            entry.draft.full_text, entry.draft.structured
                        ))
                        .on_click(cx.listener(move |this, _, _, _| {
                            this.send(UiAction::RestoreHistory(index))
                        }))
                }))
            })
            .when(self.state.show_saved, |this| {
                this.children(
                    saved
                        .into_iter()
                        .filter_map(|entry| entry.name)
                        .map(|name| {
                            Button::new(format!("saved-{name}"))
                                .label(name.clone())
                                .on_click(cx.listener(move |this, _, _, _| {
                                    this.send(UiAction::RestoreSaved(name.clone()))
                                }))
                        }),
                )
            })
            .when(self.state.show_columns, |this| {
                this.children(columns.into_iter().map(|column| {
                    Button::new(format!("column-{column}"))
                        .label(format!(
                            "{} {column}",
                            if hidden_columns.contains(&column) {
                                "○"
                            } else {
                                "●"
                            }
                        ))
                        .on_click(cx.listener(move |this, _, _, _| {
                            this.send(UiAction::ToggleColumn(column.clone()))
                        }))
                }))
            })
            .child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Input::new(&self.inputs.export_path)
                            .id("export-path")
                            .w(px(260.)),
                    )
                    .child(
                        Button::new("export-selected")
                            .label("Export Selected")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.export(ExportFormat::SelectedJson, cx)
                            })),
                    )
                    .child(
                        Button::new("export-results")
                            .label("Export Results")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.export(ExportFormat::ResultsJson, cx)
                            })),
                    )
                    .child(
                        Button::new("export-ndjson")
                            .label("Export NDJSON")
                            .on_click(
                                cx.listener(|this, _, _, cx| this.export(ExportFormat::Ndjson, cx)),
                            ),
                    ),
            )
            .when(export_error.is_some(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(rgb(0xc2410c))
                        .child(export_error.unwrap_or_default()),
                )
            })
            .child(
                Button::new("diagnostics-toggle")
                    .label("Diagnostics")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.show_diagnostics = !this.state.show_diagnostics;
                        cx.notify();
                    })),
            )
            .when(self.state.show_diagnostics, |this| {
                this.child(
                    div()
                        .id("diagnostics-panel")
                        .max_h(px(180.))
                        .overflow_y_scrollbar()
                        .children(
                            self.state
                                .diagnostics
                                .lines()
                                .map(|line| div().text_sm().child(line.to_owned())),
                        ),
                )
            })
    }

    fn export(&mut self, format: ExportFormat, cx: &mut Context<Self>) {
        self.send(UiAction::Export {
            format,
            path: self.inputs.export_path.read(cx).value().to_string(),
        });
    }

    pub(super) fn render_help(&self) -> impl IntoElement {
        div().id("help-panel").absolute().top(px(100.)).right(px(16.)).w(px(600.)).max_h(px(600.)).flex().flex_col().gap_1().p_3()
            .bg(rgb(if self.state.dark { 0x202a35 } else { 0xe8f0f7 }))
            .child(div().child(format!("Objekte Desktop {} · Help", env!("CARGO_PKG_VERSION"))))
            .child(div().text_sm().child("Use Open to choose a folder or configured repository. Type in Full Text and Structured independently; they run together."))
            .child(div().text_sm().child("A complete JSON string in Structured runs as a CLI-style quoted text query when Full Text is empty. Batch accepts the CLI's physical-line query format."))
            .child(div().text_sm().child("Enter confirms a query. Select a result and Inspect to view fields. Use Previous/Next for bounded pages."))
            .child(div().text_sm().child("Ctrl+O opens repository choices; Ctrl+F focuses Full Text; Ctrl+L focuses Structured. F1 opens Help; Escape closes panels."))
            .child(div().text_sm().child("Click a result to focus the grid. Up/Down and Home/End select rows; Page Up/Down change pages; Enter inspects the selected record."))
            .child(div().text_sm().child("Save Query stores query inputs. Export Results and Write Index are explicit derived-output actions. Source documents remain read-only."))
    }
}
