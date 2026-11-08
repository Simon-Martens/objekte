//! Ordinary controls for opening, inspection, history, and output actions.

use gpui_kit::component::{
    Disableable as _, IconName, Selectable as _,
    button::{Button, ButtonVariants},
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
    BatchSource, DetailOrigin, DetailValueKind, ErrorField, ExportFormat, UiAction,
};
use super::state::ActionPanel;


impl DesktopView {
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
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child("Run query batch"),
                    )
                    .child(
                        Button::new("batch-close")
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip("Close batch view")
                            .accessibility_label("Close batch view")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.show_batch = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .child("Paste one CLI query per line or choose a query file."),
            )
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
            .when(self.state.batch_source == BatchSource::QueryFile, |this| {
                this.child(
                    Input::new(&self.inputs.batch_path)
                        .id("batch-file-path")
                        .w_full(),
                )
            })
            .when(self.state.batch_source == BatchSource::PastedText, |this| {
                this.child(
                    div()
                        .id("batch-text")
                        .child(Textarea::new(&self.inputs.batch_text).w_full().h(px(100.))),
                )
            })
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
            .child(
                div()
                    .id("batch-outcomes")
                    .max_h(px(230.))
                    .overflow_y_scrollbar()
                    .children(rows.into_iter().map(|row| {
                        div().text_sm().child(format!(
                            "Line {}: {} — {}",
                            row.line,
                            row.query,
                            row.error.unwrap_or(row.status)
                        ))
                    })),
            )
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
                                this.state.action_panel = ActionPanel::Export;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_actions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = super::theme::palette(self.state.dark);
        let kind = self.state.action_panel;
        let title = match kind {
            ActionPanel::Menu => "Menu",
            ActionPanel::History => "History",
            ActionPanel::Saved => "Saved queries",
            ActionPanel::Columns => "Columns",
            ActionPanel::SaveQuery => "Save query",
            ActionPanel::Export => "Export",
            ActionPanel::Index => "Index",
            ActionPanel::Diagnostics => "Diagnostics",
        };
        let panel = div()
            .id("action-panel")
            .absolute()
            .top(px(48.))
            .right(px(8.))
            .w(px(370.))
            .max_h(px(600.))
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(rgb(colors.panel))
            .border_1()
            .border_color(rgb(colors.border))
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
                            .child(title),
                    )
                    .when(kind != ActionPanel::Menu, |this| {
                        this.child(
                            Button::new("action-back")
                                .ghost()
                                .icon(IconName::ArrowLeft)
                                .tooltip("Back to menu")
                                .accessibility_label("Back to menu")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state.action_panel = ActionPanel::Menu;
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        Button::new("action-close")
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip("Close")
                            .accessibility_label("Close")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.show_actions = false;
                                cx.notify();
                            })),
                    ),
            );
        let mut content = div()
            .id("action-body")
            .flex()
            .flex_col()
            .gap_1()
            .max_h(px(530.))
            .overflow_y_scrollbar();
        match kind {
            ActionPanel::Menu => {
                for (id, label, next) in [
                    ("menu-history", "History", ActionPanel::History),
                    ("menu-saved", "Saved queries", ActionPanel::Saved),
                    ("menu-save", "Save current query…", ActionPanel::SaveQuery),
                    ("menu-columns", "Columns…", ActionPanel::Columns),
                    ("menu-export", "Export…", ActionPanel::Export),
                    ("menu-index", "Index…", ActionPanel::Index),
                    ("menu-diagnostics", "Diagnostics", ActionPanel::Diagnostics),
                ] {
                    content =
                        content.child(Button::new(id).ghost().w_full().label(label).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.state.action_panel = next;
                                cx.notify();
                            }),
                        ));
                }
                content = content
                    .child(
                        Button::new("menu-batch")
                            .ghost()
                            .w_full()
                            .icon(IconName::SquareTerminal)
                            .label("Run query batch…")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.show_actions = false;
                                this.state.show_batch = true;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("menu-theme")
                            .ghost()
                            .w_full()
                            .icon(if self.state.dark {
                                IconName::Sun
                            } else {
                                IconName::Moon
                            })
                            .label(if self.state.dark {
                                "Light theme"
                            } else {
                                "Dark theme"
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.state.dark = !this.state.dark;
                                gpui_kit::component::Theme::change(
                                    if this.state.dark {
                                        gpui_kit::component::ThemeMode::Dark
                                    } else {
                                        gpui_kit::component::ThemeMode::Light
                                    },
                                    Some(window),
                                    cx,
                                );
                                this.send(UiAction::SetTheme {
                                    dark: this.state.dark,
                                });
                                this.state.show_actions = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("menu-help")
                            .ghost()
                            .w_full()
                            .icon(IconName::Info)
                            .label("Help and shortcuts")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.show_help = true;
                                this.state.show_actions = false;
                                cx.notify();
                            })),
                    );
            }
            ActionPanel::History => {
                content = content.child(
                    Input::new(&self.inputs.action_search)
                        .id("history-search")
                        .w_full(),
                );
                let needle = self.inputs.action_search.read(cx).value().to_lowercase();
                for (index, entry) in self.state.history.iter().enumerate().rev().take(100) {
                    let label = format!("{} {}", entry.draft.full_text, entry.draft.structured);
                    if !label.to_lowercase().contains(&needle) {
                        continue;
                    }
                    content = content.child(
                        Button::new(format!("history-{index}"))
                            .ghost()
                            .w_full()
                            .label(if label.trim().is_empty() {
                                "All records".to_owned()
                            } else {
                                label
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.send(UiAction::RestoreHistory(index));
                                this.state.show_actions = false;
                                cx.notify();
                            })),
                    );
                }
            }
            ActionPanel::Saved => {
                content = content.child(
                    Input::new(&self.inputs.action_search)
                        .id("saved-search")
                        .w_full(),
                );
                let needle = self.inputs.action_search.read(cx).value().to_lowercase();
                for name in self
                    .state
                    .saved
                    .iter()
                    .filter_map(|entry| entry.name.as_ref())
                {
                    if !name.to_lowercase().contains(&needle) {
                        continue;
                    }
                    let name = name.clone();
                    content = content.child(
                        Button::new(format!("saved-{name}"))
                            .ghost()
                            .w_full()
                            .label(name.clone())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.send(UiAction::RestoreSaved(name.clone()));
                                this.state.show_actions = false;
                                cx.notify();
                            })),
                    );
                }
            }
            ActionPanel::Columns => {
                for column in &self.state.available_columns {
                    let column = column.clone();
                    let visible = !self.state.hidden_columns.contains(&column);
                    content = content.child(
                        Button::new(format!("column-{column}"))
                            .ghost()
                            .w_full()
                            .label(format!("{} {column}", if visible { "☑" } else { "☐" }))
                            .on_click(cx.listener(move |this, _, _, _| {
                                this.send(UiAction::ToggleColumn(column.clone()));
                            })),
                    );
                }
            }
            ActionPanel::SaveQuery => {
                content = content
                    .child(div().text_sm().child("Name"))
                    .child(
                        Input::new(&self.inputs.query_name)
                            .id("query-name")
                            .w_full(),
                    )
                    .child(
                        Button::new("save-query")
                            .primary()
                            .label("Save query")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.send(UiAction::SaveQuery {
                                    name: this.inputs.query_name.read(cx).value().to_string(),
                                    draft: this.query_from_inputs(cx),
                                });
                            })),
                    );
            }
            ActionPanel::Export => {
                let invalid = !matches!(
                    self.state.filter_validation,
                    Some(super::model::FilterValidationState::Valid)
                );
                content = content.child(div().text_sm().child("Export scope and format"));
                for (id, label, format) in [
                    (
                        "export-results",
                        "All results · JSON",
                        ExportFormat::ResultsJson,
                    ),
                    (
                        "export-selected",
                        "Selected record · JSON",
                        ExportFormat::SelectedJson,
                    ),
                    (
                        "export-ndjson",
                        "All results · CLI NDJSON",
                        ExportFormat::Ndjson,
                    ),
                ] {
                    content = content.child(
                        Button::new(id)
                            .ghost()
                            .w_full()
                            .label(label)
                            .selected(self.state.export_format == format)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state.export_format = format;
                                cx.notify();
                            })),
                    );
                }
                content = content
                    .child(div().text_sm().child("New output path"))
                    .child(
                        Input::new(&self.inputs.export_path)
                            .id("export-path")
                            .w_full(),
                    )
                    .child(
                        Button::new("export-confirm")
                            .primary()
                            .label("Export")
                            .disabled(
                                invalid
                                    || (self.state.export_format == ExportFormat::SelectedJson
                                        && self.state.page.selected.is_none()),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.export(this.state.export_format, cx);
                            })),
                    );
                if invalid {
                    content = content.child(
                        div()
                            .text_sm()
                            .text_color(rgb(colors.error))
                            .child("Finish or clear the current filter before exporting."),
                    );
                }
                if let Some(error) = self.state.errors.get(&ErrorField::Export) {
                    content = content.child(
                        div()
                            .text_sm()
                            .text_color(rgb(colors.error))
                            .child(error.clone()),
                    );
                }
            }
            ActionPanel::Index => {
                content = content.child(div().text_sm().child("Run an index operation on the open source. The current results stay available."));
                for (id, label, mode) in [
                    (
                        "index-memory",
                        "Build in memory",
                        super::model::SnapshotMode::None,
                    ),
                    (
                        "index-load",
                        "Load existing index",
                        super::model::SnapshotMode::Load,
                    ),
                    (
                        "index-write",
                        "Build and save index",
                        super::model::SnapshotMode::Write,
                    ),
                    (
                        "index-load-write",
                        "Load or rebuild and save",
                        super::model::SnapshotMode::LoadAndWrite,
                    ),
                ] {
                    content = content.child(
                        Button::new(id)
                            .ghost()
                            .w_full()
                            .label(label)
                            .selected(self.state.index_mode == mode)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state.index_mode = mode;
                                cx.notify();
                            })),
                    );
                }
                if self.state.index_mode != super::model::SnapshotMode::None {
                    content = content
                        .child(
                            div()
                                .text_sm()
                                .child("Index directory (empty uses default)"),
                        )
                        .child(
                            Input::new(&self.inputs.snapshot_directory)
                                .id("index-directory")
                                .w_full(),
                        );
                }
                content = content.child(
                    Button::new("index-run")
                        .primary()
                        .label("Run index operation")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.send(UiAction::Index {
                                mode: this.state.index_mode,
                                directory: this
                                    .inputs
                                    .snapshot_directory
                                    .read(cx)
                                    .value()
                                    .to_string(),
                            });
                            this.state.show_actions = false;
                            cx.notify();
                        })),
                );
            }
            ActionPanel::Diagnostics => {
                content = content.children(
                    self.state
                        .diagnostics
                        .lines()
                        .map(|line| div().text_sm().child(line.to_owned())),
                );
            }
        }
        panel.child(content)
    }

    fn export(&mut self, format: ExportFormat, cx: &mut Context<Self>) {
        self.send(UiAction::Export {
            format,
            path: self.inputs.export_path.read(cx).value().to_string(),
        });
    }

    pub(super) fn render_help(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = super::theme::palette(self.state.dark);
        div()
            .id("help-panel")
            .absolute()
            .top(px(48.))
            .right(px(8.))
            .w(px(470.))
            .max_h(px(600.))
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(rgb(colors.panel))
            .border_1()
            .border_color(rgb(colors.border))
            .rounded_md()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.state.show_help = false;
                cx.notify();
            }))
            .child(div().flex().items_center()
                .child(div().flex_1().font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child(format!("Objekte {} · Help", env!("CARGO_PKG_VERSION"))))
                .child(Button::new("help-close").ghost().icon(IconName::Close)
                    .tooltip("Close help").accessibility_label("Close help")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.show_help = false;
                        cx.notify();
                    }))))
            .child(div().text_sm().child("Open a folder, choose a collection, search, then select a row to inspect it."))
            .child(div().text_sm().child("Search words go in Search. Enable Filter to edit the complete structured expression; search and filter combine with AND."))
            .child(div().text_sm().child("A complete quoted JSON string in Filter runs as a CLI-style text query when Search is empty. Batch accepts one CLI query per line."))
            .child(div().text_sm().child("Click a column header to cycle ascending, descending, and unsorted. Use Query options for sort expressions, ranges, and exact count."))
            .child(div().text_sm().child("Ctrl+O: Open · Ctrl+F: Search · Ctrl+L: Filter · F1: Help · Escape: close the nearest panel."))
            .child(div().text_sm().child("Use Up/Down and Home/End in the table. Page Up/Down changes result pages. Enter inspects the selected record."))
            .child(div().text_sm().child("Source records and schemas are read-only. Export and index writes are explicit actions."))
    }
}
