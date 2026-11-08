//! Ordinary controls for opening, inspection, history, and output actions.

use gpui_kit::assets::IconName as AssetIconName;
use gpui_kit::component::{
    Disableable as _, IconName, Selectable as _,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    input::{Input, Textarea},
    scroll::ScrollableElement as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, FontWeight, HighlightStyle, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, StyledText, div, px, rgb,
};

use super::DesktopView;
use super::model::{
    BatchSource, DetailOrigin, DetailRow, DetailValueKind, ErrorField, ExportFormat, UiAction,
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
            .and_then(|index| self.state.projected_rows.get(&index))
            .and_then(|row| row.preview.as_ref());
        let active_tab = self
            .state
            .tabs
            .iter()
            .find(|tab| Some(tab.id) == self.state.active_tab);
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
                    .id("record-tabs")
                    .flex()
                    .items_center()
                    .h(px(38.))
                    .flex_shrink_0()
                    .gap_1()
                    .px_2()
                    .border_b_1()
                    .border_color(rgb(colors.border))
                    .children(self.state.tabs.iter().map(|tab| {
                        let id = tab.id;
                        div()
                            .flex()
                            .items_center()
                            .flex_shrink_0()
                            .child(
                                Button::new(format!("record-tab-{id}"))
                                    .ghost()
                                    .selected(self.state.active_tab == Some(id))
                                    .label(tab.title.clone())
                                    .tooltip(tab.identity.clone())
                                    .on_click(cx.listener(move |this, _, _, _| {
                                        this.send(UiAction::ActivateTab(id));
                                    })),
                            )
                            .child(
                                Button::new(format!("close-tab-{id}"))
                                    .ghost()
                                    .icon(IconName::Close)
                                    .tooltip("Close record tab")
                                    .accessibility_label("Close record tab")
                                    .on_click(cx.listener(move |this, _, _, _| {
                                        this.send(UiAction::CloseTab(id));
                                    })),
                            )
                    }))
                    .overflow_x_scroll(),
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
                        Button::new("record-back")
                            .ghost()
                            .icon(IconName::Undo)
                            .tooltip("Back")
                            .accessibility_label("Back")
                            .disabled(!active_tab.is_some_and(|tab| tab.can_back))
                            .on_click(cx.listener(|this, _, _, _| {
                                this.save_tab_view();
                                this.send(UiAction::BackRelation);
                            })),
                    )
                    .child(
                        Button::new("previous-record")
                            .ghost()
                            .icon(IconName::ArrowLeft)
                            .tooltip("Previous result")
                            .accessibility_label("Previous result")
                            .disabled(!active_tab.is_some_and(|tab| tab.can_previous))
                            .on_click(cx.listener(|this, _, _, _| {
                                this.save_tab_view();
                                this.send(UiAction::AdjacentResult(false));
                            })),
                    )
                    .child(
                        Button::new("next-record")
                            .ghost()
                            .icon(IconName::ArrowRight)
                            .tooltip("Next result")
                            .accessibility_label("Next result")
                            .disabled(!active_tab.is_some_and(|tab| tab.can_next))
                            .on_click(cx.listener(|this, _, _, _| {
                                this.save_tab_view();
                                this.send(UiAction::AdjacentResult(true));
                            })),
                    )
                    .child(active_tab.map_or_else(String::new, |tab| format!("#{}", tab.position)))
                    .child(div().flex_1())
                    .child(
                        Button::new("open-tab-query")
                            .ghost()
                            .label("Open query")
                            .on_click(cx.listener(|this, _, _, _| {
                                this.send(UiAction::OpenTabQuery);
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
                    .h(px(32.))
                    .flex_shrink_0()
                    .px_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(colors.border))
                    .child(div().w(px(7.)).h(px(7.)).rounded_full().bg(rgb(
                        match detail.validity {
                            Some(true) => 0x46bc77,
                            Some(false) => colors.error,
                            None => colors.secondary,
                        },
                    )))
                    .child(
                        Button::new("metadata-toggle")
                            .ghost()
                            .label(if self.state.show_detail_metadata {
                                "▾ Metadata"
                            } else {
                                "▸ Metadata"
                            })
                            .accessibility_label(format!(
                                "Metadata: {}",
                                match detail.validity {
                                    Some(true) => "Valid",
                                    Some(false) => "Invalid",
                                    None => "Unknown",
                                }
                            ))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.show_detail_metadata = !this.state.show_detail_metadata;
                                this.save_tab_view();
                                cx.notify();
                            })),
                    ),
            )
            .when(self.state.show_detail_metadata, |this| {
                this.child(
                    div()
                        .id("record-metadata")
                        .max_h(px(220.))
                        .overflow_y_scrollbar()
                        .children(
                            detail
                                .structured
                                .iter()
                                .enumerate()
                                .filter(|(_, row)| row.origin != DetailOrigin::Structured)
                                .map(|(index, row)| {
                                    self.inspector_field_row(index, row, cx).into_any_element()
                                })
                                .collect::<Vec<_>>(),
                        ),
                )
            })
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
                            .selected(!self.state.show_raw && !self.state.show_links)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.show_raw = false;
                                this.state.show_links = false;
                                this.save_tab_view();
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("links-tab")
                            .ghost()
                            .label("Links")
                            .selected(self.state.show_links)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.show_raw = false;
                                this.state.show_links = true;
                                this.save_tab_view();
                                this.send(UiAction::LoadLinks);
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    .when(self.state.show_raw, |this| {
                        this.child(
                            Button::new("copy-record")
                                .ghost()
                                .icon(AssetIconName::Copy)
                                .tooltip("Copy record")
                                .accessibility_label("Copy record")
                                .on_click(
                                    cx.listener(|this, _, _, _| this.send(UiAction::CopyRecord)),
                                ),
                        )
                        .child(
                            Button::new("export-record")
                                .ghost()
                                .icon(AssetIconName::Download)
                                .tooltip("Export record")
                                .accessibility_label("Export record")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state.export_format = ExportFormat::SelectedJson;
                                    this.state.show_actions = true;
                                    this.state.action_panel = ActionPanel::Export;
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        Button::new("source-tab")
                            .ghost()
                            .icon(AssetIconName::CodeXml)
                            .tooltip("Source")
                            .accessibility_label("Source")
                            .selected(self.state.show_raw)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.show_raw = true;
                                this.state.show_links = false;
                                this.save_tab_view();
                                this.send(UiAction::ToggleRawSource);
                                cx.notify();
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
                        .font_family(super::typography::MONO)
                        .track_scroll(&self.source_scroll)
                        .text_sm()
                        .child(self.state.detail_source.clone().unwrap_or_default())
                        .overflow_y_scrollbar(),
                );
        } else if self.state.show_links {
            let links = div()
                .id("record-links")
                .track_scroll(&self.links_scroll)
                .flex_1()
                .min_h_0()
                .children(
                    detail
                        .relations
                        .iter()
                        .enumerate()
                        .map(|(index, relation)| {
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
                                        .child(div().flex().gap_1().child(label).child(value))
                                        .on_click(cx.listener(move |this, _, _, _| {
                                            this.save_tab_view();
                                            this.send(UiAction::FollowRelation(index));
                                        })),
                                )
                                .when(relation.target, |this| {
                                    this.child(
                                        Button::new(format!("relation-new-tab-{index}"))
                                            .ghost()
                                            .label("New tab")
                                            .tooltip("Open relation in a new record tab")
                                            .on_click(cx.listener(move |this, _, _, _| {
                                                this.save_tab_view();
                                                this.send(UiAction::OpenRelationTab(index));
                                            })),
                                    )
                                })
                        }),
                );
            panel = panel.child(
                links
                    .when(detail.relations.is_empty(), |this| {
                        this.child(
                            div()
                                .p_3()
                                .text_sm()
                                .text_color(rgb(colors.secondary))
                                .child(if active_tab.is_some_and(|tab| tab.links_loading) {
                                    "Loading links…"
                                } else {
                                    "No links found"
                                }),
                        )
                    })
                    .overflow_y_scrollbar(),
            );
        } else {
            let fields = detail
                .structured
                .iter()
                .enumerate()
                .filter(|(_, row)| {
                    row.origin == DetailOrigin::Structured
                        && !(row.depth == 0 && row.label == "$" && detail.structured.len() > 1)
                })
                .collect::<Vec<_>>();
            let content = div()
                .id("structured-detail")
                .track_scroll(&self.fields_scroll)
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .children(
                    fields
                        .iter()
                        .map(|(index, row)| {
                            self.inspector_field_row(*index, row, cx).into_any_element()
                        })
                        .collect::<Vec<_>>(),
                )
                .when(fields.is_empty(), |this| {
                    this.child(
                        div()
                            .p_3()
                            .text_sm()
                            .text_color(rgb(colors.secondary))
                            .child("No non-empty fields"),
                    )
                });
            panel = panel.child(content.overflow_y_scrollbar());
        }
        panel
            .when(detail.clipped, |this| {
                this.child(
                    div()
                        .px_3()
                        .text_xs()
                        .text_color(rgb(colors.secondary))
                        .child("Visible values clipped; Source and export remain complete."),
                )
            })
            .into_any_element()
    }

    fn inspector_field_row(
        &self,
        index: usize,
        row: &DetailRow,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = super::theme::palette(self.state.dark);
        let value = match row.value_kind {
            DetailValueKind::Present => self
                .state
                .detail_cache
                .values
                .get(index)
                .cloned()
                .unwrap_or_default(),
            DetailValueKind::JsonNull => "null (JSON)".into(),
            DetailValueKind::LogicalUnavailable => "unavailable logical data".into(),
        };
        let marker = if row.expandable {
            if row.expanded { "▾" } else { "▸" }
        } else {
            " "
        };
        let expandable = row.expandable;
        let selected = self
            .state
            .detail
            .as_ref()
            .is_some_and(|detail| detail.selected_row == index);
        let highlights = row
            .value_highlights
            .iter()
            .cloned()
            .map(|range| {
                (
                    range,
                    HighlightStyle {
                        font_weight: Some(FontWeight::BOLD),
                        color: Some(rgb(colors.accent).into()),
                        ..Default::default()
                    },
                )
            })
            .collect::<Vec<_>>();
        div()
            .id(("detail-row", index))
            .flex()
            .items_start()
            .w_full()
            .px_2()
            .py_2()
            .border_b_1()
            .border_color(rgb(colors.border))
            .bg(rgb(if selected {
                colors.selected
            } else {
                colors.panel
            }))
            .child(
                div()
                    .w(px(128.))
                    .flex_shrink_0()
                    .pl(px(row.depth.min(6) as f32 * 9.))
                    .pr_2()
                    .text_xs()
                    .text_color(rgb(if row.matched {
                        colors.accent
                    } else {
                        colors.secondary
                    }))
                    .font_weight(if row.matched {
                        gpui_kit::FontWeight::BOLD
                    } else {
                        gpui_kit::FontWeight::NORMAL
                    })
                    .child(format!(
                        "{marker}{}",
                        self.state
                            .detail_cache
                            .labels
                            .get(index)
                            .cloned()
                            .unwrap_or_default()
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .child(StyledText::new(value).with_highlights(highlights)),
            )
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
    }

    pub(super) fn render_actions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = super::theme::palette(self.state.dark);
        let kind = self.state.action_panel;
        let title = match kind {
            ActionPanel::Columns => "Columns",
            ActionPanel::Export => "Export",
            ActionPanel::Index => "Index",
            ActionPanel::Diagnostics => "Diagnostics",
        };
        let panel = div()
            .id("action-panel")
            .absolute()
            .when(kind == ActionPanel::Columns, |this| {
                this.bottom(px(60.)).right(px(if self.state.inspector_visible && self.state.active_tab.is_some() {
                    self.state.inspector_width + 8.
                } else {
                    8.
                }))
            })
            .when(kind != ActionPanel::Columns, |this| this.top(px(48.)).right(px(8.)))
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
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
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
            .track_scroll(&self.columns_scroll)
            .overflow_y_scrollbar();
        match kind {
            ActionPanel::Columns => {
                for column in &self.state.available_columns {
                    let column = column.clone();
                    let display = super::sort::column_label(&column).to_owned();
                    let visible = !self.state.hidden_columns.contains(&column);
                    content = content.child(
                        Checkbox::new(format!("column-{column}"))
                            .w_full()
                            .checked(visible)
                            .accessibility_label(format!("Show {display}"))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_left()
                                    .child(display),
                            )
                            .on_change(cx.listener(move |this, _: &bool, _, _| {
                                this.send(UiAction::ToggleColumn(column.clone()));
                            })),
                    );
                }
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
            .overflow_y_scrollbar()
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
            .child(div().text_sm().child("Open a folder and search. Saved queries appear in the sidebar; expand History to revisit recent searches. Save from the application menu, then rename a saved row with F2."))
            .child(div().text_sm().child("Search words go in Search. The funnel reveals Filter; the adjacent icon opens Filter Builder. Search and Filter combine with AND, and Filter suggests fields and indexed values."))
            .child(div().text_sm().child("A complete quoted JSON string in Filter runs as a CLI-style text query when Search is empty. Batch accepts one CLI query per line."))
            .child(div().text_sm().child("Click a column header to cycle ascending, descending, and unsorted. Headers edit SORT BY in Filter. Drag a header edge to resize; right-click it to wrap text. Columns chooses visible fields. Count all requests an exact total."))
            .child(div().text_sm().child("Ctrl+O: Open · Ctrl+F: Search · Ctrl+L: Filter · F1: Help · Escape: close the nearest panel."))
            .child(div().text_sm().child("Scroll the table to load more results in batches of 100. Up/Down select rows; Page Up/Down scroll a viewport. Click a row or press Enter to open a record tab."))
            .child(div().text_sm().child("Record tabs retain their query. Back, Previous, Next and Open query use that context. Fields and Links are inspector views; the Source icon opens the original record with Copy and Export actions."))
            .child(div().text_sm().child("Source records and schemas are read-only. Export and index writes are explicit actions."))
    }
}
