//! Ordinary controls for opening, inspection, history, and output actions.

use gpui_kit::assets::IconName as AssetIconName;
use gpui_kit::component::{
    Disableable as _, Icon, IconName, Selectable as _,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    input::{Input, Textarea},
    menu::{ContextMenuExt as _, PopupMenuItem},
    scroll::ScrollableElement as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, FontWeight, HighlightStyle, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, StyledText, div, px, rgb,
};

use super::DesktopView;
use super::model::{
    BatchSource, DetailOrigin, DetailRow, DetailValueKind, ErrorField, ExportFormat, IndexTaskKind,
    IndexTaskState, TabFreshness, UiAction,
};
use super::state::ActionPanel;
use super::workspace::{Pane, TabId};

#[cfg(test)]
mod display_title_tests {
    use super::*;
    use crate::ui::model::DetailView;

    #[test]
    fn uses_a_present_display_name_and_falls_back_to_record_identity() {
        let mut detail = DetailView { title: "files / path:0001.json".into(), ..DetailView::default() };
        assert_eq!(detail.display_title(), detail.title);
        detail.structured.push(DetailRow {
            label: "$.name".into(),
            value: "Example: part".into(),
            depth: 0,
            matched: false,
            filter_participant: false,
            fts_matched: false,
            value_highlights: Vec::new(),
            expandable: false,
            expanded: false,
            origin: DetailOrigin::Structured,
            value_kind: DetailValueKind::Present,
        });
        assert_eq!(detail.display_title(), "Example: part");
        detail.structured[0].value_kind = DetailValueKind::LogicalUnavailable;
        assert_eq!(detail.display_title(), detail.title);
    }
}

fn copyable_field_path(row: &DetailRow, clipped: bool) -> Option<String> {
    let path = if row.origin == DetailOrigin::Structured {
        row.label.strip_prefix("$.").unwrap_or(&row.label)
    } else {
        &row.label
    };
    (!path.is_empty() && !(clipped && row.label.ends_with('…'))).then(|| path.to_owned())
}

fn copyable_field_value(row: &DetailRow, clipped: bool) -> Option<String> {
    if row.expandable
        || row.value_kind == DetailValueKind::LogicalUnavailable
        || (clipped && (row.value.is_empty() || row.value.ends_with('…')))
    {
        return None;
    }
    Some(if row.value_kind == DetailValueKind::JsonNull {
        "null".into()
    } else {
        row.value.clone()
    })
}

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
                            .on_click(
                                cx.listener(|this, _, _, _| this.send(UiAction::CancelBatch)),
                            ),
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

    pub(super) fn render_detail(
        &self,
        pane: Pane,
        split_preview: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = super::theme::palette(self.state.dark);
        let show_divider = pane == Pane::Secondary
            && (split_preview || !self.state.workspace.primary.tabs.is_empty());
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
        let is_preview = active_tab.is_some_and(|tab| !tab.pinned);
        let freshness = active_tab.map_or(TabFreshness::Current, |tab| tab.freshness);
        let navigation_current = freshness == TabFreshness::Current;
        let record_current = matches!(
            freshness,
            TabFreshness::Current | TabFreshness::NavigationUnavailable
        );
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
            .w_full()
            .h_full()
            .flex_shrink_0()
            .relative()
            .bg(rgb(colors.panel))
            .when(show_divider, |this| {
                this.border_l_1().border_color(rgb(colors.border))
            })
            .when(show_divider, |this| {
                this.child(
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
            })
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
                            .tooltip(if navigation_current {
                                "Back"
                            } else {
                                "Refresh record to navigate"
                            })
                            .accessibility_label("Back")
                            .disabled(
                                !navigation_current || !active_tab.is_some_and(|tab| tab.can_back),
                            )
                            .on_click(cx.listener(|this, _, _, _| {
                                this.save_tab_view();
                                this.send(UiAction::BackRelation);
                            })),
                    )
                    .child(
                        Button::new("previous-record")
                            .ghost()
                            .icon(IconName::ArrowLeft)
                            .tooltip(if navigation_current {
                                "Previous result"
                            } else {
                                "Refresh record to navigate"
                            })
                            .accessibility_label("Previous result")
                            .disabled(
                                !navigation_current
                                    || !active_tab.is_some_and(|tab| tab.can_previous),
                            )
                            .on_click(cx.listener(|this, _, _, _| {
                                this.save_tab_view();
                                this.send(UiAction::AdjacentResult(false));
                            })),
                    )
                    .child(
                        Button::new("next-record")
                            .ghost()
                            .icon(IconName::ArrowRight)
                            .tooltip(if navigation_current {
                                "Next result"
                            } else {
                                "Refresh record to navigate"
                            })
                            .accessibility_label("Next result")
                            .disabled(
                                !navigation_current || !active_tab.is_some_and(|tab| tab.can_next),
                            )
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
                            .icon(AssetIconName::Search)
                            .tooltip("Open query")
                            .accessibility_label("Open query")
                            .on_click(cx.listener(|this, _, _, _| {
                                this.send(UiAction::OpenTabQuery);
                            })),
                    )
                    .when(is_preview, |this| {
                        this.child(
                            Button::new("pin-record-preview")
                                .ghost()
                                .icon(AssetIconName::ExternalLink)
                                .label("Open in tab")
                                .tooltip("Keep this record in its own tab")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state.preview_search = None;
                                    this.state.inspector_visible = false;
                                    this.send(UiAction::PinPreview);
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("close-record-preview")
                                .ghost()
                                .icon(AssetIconName::X)
                                .tooltip("Close preview")
                                .accessibility_label("Close preview")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let resume = [Pane::Primary, Pane::Secondary]
                                        .into_iter()
                                        .find(|pane| this.preview_docked_in(*pane))
                                        .and_then(|pane| match this.state.workspace.group(pane).active {
                                            Some(TabId::Record(id)) => Some(id),
                                            _ => None,
                                        });
                                    this.state.preview_search = None;
                                    this.state.inspector_visible = false;
                                    if let Some(id) = this.state.active_tab {
                                        this.send(UiAction::CloseTab(id));
                                    }
                                    if let Some(id) = resume {
                                        this.send(UiAction::ActivateTab(id));
                                    }
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .when(freshness != TabFreshness::Current, |this| {
                let label = match freshness {
                    TabFreshness::Current => "",
                    TabFreshness::Refreshing => "Refreshing record…",
                    TabFreshness::Outdated => "Record may have changed — previously loaded content",
                    TabFreshness::NavigationUnavailable => {
                        "Record refreshed — position in original results unavailable"
                    }
                    TabFreshness::Removed => {
                        "Record is no longer available — previously loaded content"
                    }
                    TabFreshness::Failed => "Could not refresh record — previously loaded content",
                };
                this.child(
                    div()
                        .id("record-freshness")
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .py_1()
                        .bg(rgb(colors.raised))
                        .child(div().text_xs().flex_1().child(label))
                        .when(freshness != TabFreshness::Refreshing, |this| {
                            this.child(
                                Button::new("refresh-record")
                                    .ghost()
                                    .label("Refresh")
                                    .tooltip("Refresh record")
                                    .accessibility_label("Refresh record")
                                    .on_click(cx.listener(|this, _, _, _| {
                                        if let Some(id) = this.state.active_tab {
                                            this.send(UiAction::RefreshRecord(id));
                                        }
                                    })),
                            )
                        }),
                )
            });
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
        let identity = active_tab.map_or_else(String::new, |tab| tab.identity.clone());
        panel = panel
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(colors.border))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors.secondary))
                            .child(if is_preview { "PREVIEW" } else { "RECORD" }),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(detail.display_title().to_string()),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_xs()
                            .text_color(rgb(colors.secondary))
                            .child(identity),
                    ),
            )
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
                            .flex_1()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .w_full()
                                    .child("Metadata")
                                    .child(div().flex_1())
                                    .child(
                                        Icon::new(if self.state.show_detail_metadata {
                                            AssetIconName::ChevronDown
                                        } else {
                                            AssetIconName::ChevronRight
                                        })
                                        .size_4(),
                                    ),
                            )
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
                            .disabled(!navigation_current)
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
                    .child(
                        Button::new("copy-record")
                            .ghost()
                            .icon(AssetIconName::Copy)
                            .disabled(!record_current)
                            .tooltip("Copy complete record")
                            .accessibility_label("Copy complete record")
                            .on_click(cx.listener(|this, _, _, _| this.send(UiAction::CopyRecord))),
                    )
                    .when(self.state.show_raw, |this| {
                        this.child(
                            Button::new("export-record")
                                .ghost()
                                .icon(AssetIconName::Download)
                                .disabled(!record_current)
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
                                        .disabled(!navigation_current || !relation.available)
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
                                            .icon(AssetIconName::SquarePlus)
                                            .disabled(!navigation_current || !relation.available)
                                            .tooltip("Open relation in a new record tab")
                                            .accessibility_label(
                                                "Open relation in a new record tab",
                                            )
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
        let expandable = row.expandable;
        let expanded = row.expanded;
        let detail_clipped = self
            .state
            .detail
            .as_ref()
            .is_some_and(|detail| detail.clipped);
        let copy_path = copyable_field_path(row, detail_clipped);
        let copy_value = copyable_field_value(row, detail_clipped);
        let path_label = if row.origin == DetailOrigin::Structured {
            "Copy field path"
        } else {
            "Copy label"
        };
        let value_label = if expandable {
            "Copy value (aggregate)"
        } else if copy_value.is_none() {
            "Copy value (unavailable)"
        } else {
            "Copy value"
        };
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
                    .child(
                        self.state
                            .detail_cache
                            .labels
                            .get(index)
                            .cloned()
                            .unwrap_or_default(),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .child(StyledText::new(value).with_highlights(highlights)),
            )
            .when(expandable, |this| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .pl_1()
                        .text_color(rgb(colors.secondary))
                        .child(
                            Icon::new(if expanded {
                                AssetIconName::ChevronDown
                            } else {
                                AssetIconName::ChevronRight
                            })
                            .size_4(),
                        ),
                )
            })
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
            .context_menu(move |menu, _, _| {
                let path = copy_path.clone();
                let value = copy_value.clone();
                menu.item(
                    PopupMenuItem::new(path_label)
                        .icon(AssetIconName::Copy)
                        .disabled(path.is_none())
                        .on_click(move |_, _, cx| {
                            if let Some(path) = path.clone() {
                                super::copy_text_to_clipboard(path, cx);
                            }
                        }),
                )
                .item(
                    PopupMenuItem::new(value_label)
                        .icon(AssetIconName::Copy)
                        .disabled(value.is_none())
                        .on_click(move |_, _, cx| {
                            if let Some(value) = value.clone() {
                                super::copy_text_to_clipboard(value, cx);
                            }
                        }),
                )
            })
    }

    pub(super) fn render_actions(
        &self,
        window_width: f32,
        window_height: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = super::theme::palette(self.state.dark);
        let kind = self.state.action_panel;
        let title = match kind {
            ActionPanel::Columns => "Columns",
            ActionPanel::Export => "Export",
            ActionPanel::Index => "Index",
            ActionPanel::Diagnostics => "Diagnostics",
        };
        let max_panel_height = (window_height
            - if kind == ActionPanel::Columns { 72. } else { 60. })
            .clamp(100., 600.);
        let scroll = match kind {
            ActionPanel::Columns => &self.columns_scroll,
            ActionPanel::Export => &self.export_scroll,
            ActionPanel::Index => &self.index_scroll,
            ActionPanel::Diagnostics => &self.diagnostics_scroll,
        };
        let panel = div()
            .id(format!("action-panel-{kind:?}"))
            .absolute()
            .when(kind == ActionPanel::Columns, |this| {
                let query_on_left = self
                    .state
                    .workspace
                    .current_search
                    .and_then(|id| self.state.workspace.pane_of(id)) == Some(Pane::Primary);
                let preferred_right = if self.state.inspector_visible && query_on_left {
                    self.state.inspector_width + 8.
                } else {
                    8.
                };
                let right = preferred_right.min((window_width - 378.).max(8.));
                this.bottom(px(60.)).right(px(right))
            })
            .when(kind != ActionPanel::Columns, |this| {
                this.top(px(48.)).right(px(8.))
            })
            .w(px(if kind == ActionPanel::Index {
                440.
            } else {
                370.
            }))
            .max_h(px(max_panel_height))
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
            .id(format!("action-body-{kind:?}"))
            .flex()
            .flex_col()
            .gap_1()
            .max_h(px((max_panel_height - 64.).clamp(40., 530.)))
            .track_scroll(scroll)
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
                let index = &self.state.index_status;
                let source = if index.source_location.is_empty() {
                    "No repository open".to_owned()
                } else {
                    index.source_location.clone()
                };
                content = content
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Source checking"))
                    .child(div().text_xs().child(format!("Source: {source}")))
                    .child(div().text_xs().child(format!("Mode: {}", index.opening_mode)))
                    .child(div().text_xs().child(match &index.last_check {
                        Some(time) if index.check_complete == Some(true) => format!("Last check: {time} · complete"),
                        Some(time) => format!("Last check: {time} · some files could not be checked"),
                        None => "Files have not been checked yet".to_owned(),
                    }))
                    .child(div().flex().items_center().gap_2().child(
                        Button::new("index-check-now")
                            .label("Check for changes")
                            .disabled(self.state.root.is_none())
                            .on_click(cx.listener(|this, _, _, _| this.send(UiAction::CheckForChanges))),
                    ))
                    .child(
                        Checkbox::new("index-auto-check")
                            .checked(index.automatic_check)
                            .accessibility_label("Automatically check for changes")
                            .child("Automatically check for changes")
                            .disabled(self.state.root.is_none())
                            .on_change(cx.listener(|this, enabled: &bool, _, cx| {
                                let seconds = if *enabled {
                                    parse_index_interval_seconds(
                                        this.inputs.index_interval.read(cx).value().as_ref(),
                                    )
                                } else {
                                    Ok(this.state.index_status.check_interval_seconds)
                                };
                                match seconds {
                                    Ok(interval_seconds) => {
                                        this.state.index_interval_error = None;
                                        this.send(UiAction::SetAutomaticChecking {
                                            enabled: *enabled,
                                            interval_seconds,
                                        });
                                    }
                                    Err(message) => this.state.index_interval_error = Some(message),
                                }
                                cx.notify();
                            })),
                    )
                    .child(div().flex().items_center().gap_2()
                        .child(div().text_xs().child("Interval (seconds)"))
                        .child(Input::new(&self.inputs.index_interval).id("index-check-interval").w(px(95.)))
                        .child(Button::new("index-apply-interval").label("Apply")
                            .disabled(!index.automatic_check)
                            .on_click(cx.listener(|this, _, _, cx| {
                                match parse_index_interval_seconds(
                                    this.inputs.index_interval.read(cx).value().as_ref(),
                                ) {
                                    Ok(interval_seconds) => {
                                        this.state.index_interval_error = None;
                                        this.send(UiAction::SetAutomaticChecking { enabled: true, interval_seconds });
                                    }
                                    Err(message) => this.state.index_interval_error = Some(message),
                                }
                                cx.notify();
                            }))))
                    .when(self.state.index_interval_error.is_some(), |this| this.child(
                        div().text_xs().text_color(rgb(colors.error))
                            .child(self.state.index_interval_error.clone().unwrap_or_default())
                    ))
                    .child(div().text_xs().text_color(rgb(colors.secondary)).child(
                        "The first check after loading a saved index runs even when periodic checking is off.",
                    ))
                    .child(div().h(px(1.)).my_2().bg(rgb(colors.border)))
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Saved index"))
                    .child(div().text_xs().child(format!("Loaded from: {}", index.loaded_from.as_deref().unwrap_or("None"))))
                    .child(div().text_xs().child(format!("Save destination: {}", index.save_destination.as_deref().unwrap_or("None selected"))))
                    .child(div().text_xs().child(if index.save_destination.is_none() {
                        "No save destination selected"
                    } else if index.dirty {
                        "Index changes not saved"
                    } else {
                        "Index saved to selected destination"
                    }))
                    .child(div().flex().gap_2()
                        .child(Button::new("index-save").label("Save index")
                            .disabled(self.state.root.is_none())
                            .on_click(cx.listener(|this, _, _, _| this.send(UiAction::SaveIndex))))
                        .child(Button::new("index-save-as").label("Save index as…")
                            .disabled(self.state.root.is_none())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.index_overwrite_pending = None;
                                let path = this.inputs.index_save_path.read(cx).value().to_string();
                                if path.trim().is_empty() {
                                    this.state.index_picker_generation = this.state.index_picker_generation.wrapping_add(1);
                                    this.send(UiAction::ChooseIndexSavePath { generation: this.state.index_picker_generation, save_after_selection: true });
                                } else {
                                    this.send(UiAction::SaveIndexAs(path));
                                }
                            }))))
                    .when(self.state.index_overwrite_pending.is_some(), |this| {
                        let path = self.state.index_overwrite_pending.clone().unwrap_or_default();
                        this.child(div().flex().flex_col().gap_1().p_2().bg(rgb(colors.raised))
                            .child(div().text_xs().text_color(rgb(colors.error))
                                .child(format!("A different file already exists at {path}. Replace it with this index?")))
                            .child(div().flex().gap_2()
                                .child(Button::new("index-confirm-overwrite").label("Replace file")
                                    .on_click(cx.listener(move |this, _, _, _| {
                                        this.send(UiAction::ConfirmSaveIndexAs(path.clone()));
                                    })))
                                .child(Button::new("index-cancel-overwrite").ghost().label("Cancel")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.state.index_overwrite_pending = None;
                                        cx.notify();
                                    })))))
                    })
                    .child(div().flex().items_center().gap_2()
                        .child(Input::new(&self.inputs.index_save_path).id("index-save-path").w_full())
                        .child(Button::new("index-save-browse").label("Browse…")
                            .on_click(cx.listener(|this, _, _, _| {
                                this.state.index_picker_generation = this.state.index_picker_generation.wrapping_add(1);
                                this.send(UiAction::ChooseIndexSavePath { generation: this.state.index_picker_generation, save_after_selection: false });
                            }))))
                    .child(div().text_xs().child("Save index as writes a separate snapshot. It does not change automatic saving."))
                    .child(div().flex().items_center().gap_2()
                        .child(Input::new(&self.inputs.index_load_path).id("index-load-path").w_full())
                        .child(Button::new("index-load-browse").label("Browse…")
                            .on_click(cx.listener(|this, _, _, _| {
                                this.state.index_picker_generation = this.state.index_picker_generation.wrapping_add(1);
                                this.send(UiAction::ChooseIndexLoadPath { generation: this.state.index_picker_generation, load_after_selection: false });
                            }))))
                    .child(Button::new("index-load").label("Load index…")
                        .disabled(self.state.root.is_none())
                        .on_click(cx.listener(|this, _, _, cx| {
                            let path = this.inputs.index_load_path.read(cx).value().to_string();
                            if path.trim().is_empty() {
                                this.state.index_picker_generation = this.state.index_picker_generation.wrapping_add(1);
                                this.send(UiAction::ChooseIndexLoadPath { generation: this.state.index_picker_generation, load_after_selection: true });
                            } else {
                                this.send(UiAction::LoadIndex(path));
                            }
                        })))
                    .child(
                        Checkbox::new("index-auto-save")
                            .checked(index.automatic_save)
                            .accessibility_label("Automatically save index")
                            .child("Automatically save index")
                            .disabled(!index.automatic_save_available)
                            .on_change(cx.listener(|this, enabled: &bool, _, _| {
                                this.send(UiAction::SetAutomaticSaving(*enabled));
                            })),
                    )
                    .when(index.automatic_save_reason.is_some(), |this| this.child(
                        div().text_xs().text_color(rgb(colors.secondary))
                            .child(index.automatic_save_reason.clone().unwrap_or_default())
                    ))
                    .child(div().h(px(1.)).my_2().bg(rgb(colors.border)))
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Maintenance"))
                    .child(div().text_xs().child(format!("{} files · {} records",
                        index.indexed_files.map_or_else(|| "—".to_owned(), |n| n.to_string()),
                        index.live_records.map_or_else(|| "—".to_owned(), |n| n.to_string()))))
                    .child(div().flex().gap_2()
                        .child(Button::new("index-rebuild").label("Rebuild index")
                            .disabled(self.state.root.is_none())
                            .tooltip("Reread configured source files")
                            .on_click(cx.listener(|this, _, _, _| this.send(UiAction::RebuildIndex))))
                        .child(Button::new("index-compact").label("Compact index")
                            .disabled(self.state.root.is_none())
                            .tooltip("Remove obsolete index entries without deleting source files")
                            .on_click(cx.listener(|this, _, _, _| this.send(UiAction::CompactIndex)))))
                    .child(Button::new("index-details").ghost().label("Details…")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.action_panel = ActionPanel::Diagnostics;
                            cx.notify();
                        })));
                for (label, kind, task) in [
                    ("File check", IndexTaskKind::Check, &index.check),
                    ("Saving", IndexTaskKind::Save, &index.save),
                    ("Loading", IndexTaskKind::Load, &index.load),
                    (
                        "Maintenance",
                        index.maintenance_kind.unwrap_or(IndexTaskKind::Rebuild),
                        &index.maintenance,
                    ),
                ] {
                    if let Some(summary) = index_task_summary(label, task) {
                        content = content.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().text_xs().flex_1().child(summary))
                                .when(matches!(task, IndexTaskState::Running { .. }), |this| {
                                    this.child(
                                        Button::new(format!("index-cancel-{kind:?}"))
                                            .ghost()
                                            .label("Cancel")
                                            .on_click(cx.listener(move |this, _, _, _| {
                                                this.send(UiAction::CancelIndexTask(kind))
                                            })),
                                    )
                                }),
                        );
                    }
                }
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

    pub(super) fn render_help(
        &self,
        window_width: f32,
        window_height: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = super::theme::palette(self.state.dark);
        let panel_height = (window_height - 60.).clamp(100., 600.);
        let body = div()
            .id("help-body")
            .flex()
            .flex_col()
            .gap_2()
            .max_h(px((panel_height - 64.).clamp(40., 530.)))
            .track_scroll(&self.help_scroll)
            .child(div().text_sm().child("Open a folder and search. Saved queries appear in the sidebar; expand History to revisit recent searches. Save from the application menu, then rename a saved row with F2."))
            .child(div().text_sm().child("Search words go in Search. The funnel reveals Filter; the adjacent icon opens Filter Builder. Search and Filter combine with AND, and Filter suggests fields and indexed values."))
            .child(div().text_sm().child("A complete quoted JSON string in Filter runs as a CLI-style text query when Search is empty. Batch accepts one CLI query per line."))
            .child(div().text_sm().child("Click a column header to cycle ascending, descending, and unsorted. Headers edit SORT BY in Filter. Drag a header edge to resize; right-click it to wrap text. Columns chooses visible fields. The footer shows the exact match count."))
            .child(div().text_sm().child("Ctrl+O: Open · Ctrl+F: Search · Ctrl+L: Filter · Ctrl+T: New query · Ctrl+W: Close tab · Ctrl+Tab / Ctrl+PageDown: Next tab · Ctrl+Shift+Tab / Ctrl+PageUp: Previous tab · Ctrl+1 / Ctrl+2: Focus left / right pane · F1: Help · Escape: close the nearest panel."))
            .child(div().text_sm().child("Collections and queries open in tabs. Drag a tab label to reorder it or move it between panes; right-click for move and duplicate commands. The list icon shows all open tabs. Click + for a query or middle-click a tab to close it."))
            .child(div().text_sm().child("When queries appear in both panes, the inactive query shows its latest result preview. Activate it to edit or refresh its results."))
            .child(div().text_sm().child("Scroll the table to load more results in batches of 100. Up/Down select rows; Page Up/Down move selection by a viewport. Click a row or press Enter to preview it beside the table. Use Open in tab, Shift-click, or the row menu to keep a record in its own tab."))
            .child(div().text_sm().child("Record views retain their query. Back, Previous, Next and Open query use that context. Fields and Links are inspector views; Source shows the original record. The inspector Copy icon copies the complete record."))
            .child(div().text_sm().child("Right-click a field to copy its full path or scalar value. Aggregate and clipped previews are not complete values; expand an aggregate or copy the complete record instead."))
            .child(div().text_sm().child("Source records and schemas are read-only. Export and index writes are explicit actions."));
        div()
            .id("help-panel")
            .absolute()
            .top(px(48.))
            .right(px(8.))
            .w(px((window_width - 16.).clamp(300., 470.)))
            .max_h(px(panel_height))
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
            .child(body.overflow_y_scrollbar())
    }
}

fn parse_index_interval_seconds(raw: &str) -> Result<u64, String> {
    let seconds = raw
        .trim()
        .parse::<u64>()
        .map_err(|_| "Enter a whole number of seconds".to_owned())?;
    if !(1..=86_400).contains(&seconds) {
        return Err("Choose an interval from 1 to 86,400 seconds".into());
    }
    Ok(seconds)
}

fn index_task_summary(label: &str, task: &IndexTaskState) -> Option<String> {
    match task {
        IndexTaskState::Idle => None,
        IndexTaskState::Running {
            processed, total, ..
        } => Some(match (processed, total) {
            (Some(done), Some(total)) => format!("{label}: {done} of {total}"),
            (Some(done), None) => format!("{label}: {done} processed"),
            _ => format!("{label}…"),
        }),
        IndexTaskState::Succeeded(message) => Some(format!("{label}: {message}")),
        IndexTaskState::Failed(message) => Some(format!("{label} failed: {message}")),
        IndexTaskState::Cancelled => Some(format!("{label} stopped")),
    }
}

#[cfg(test)]
mod index_controls_tests {
    use super::*;

    #[test]
    fn interval_is_validated_only_when_applied() {
        assert_eq!(parse_index_interval_seconds("30"), Ok(30));
        assert!(parse_index_interval_seconds("0").is_err());
        assert!(parse_index_interval_seconds("1.5").is_err());
        assert!(parse_index_interval_seconds("86401").is_err());
    }

    #[test]
    fn unknown_total_is_indeterminate() {
        let task = IndexTaskState::Running {
            operation_id: 1,
            processed: Some(7),
            total: None,
        };
        assert_eq!(
            index_task_summary("File check", &task),
            Some("File check: 7 processed".into())
        );
    }
}

#[cfg(test)]
mod field_copy_tests {
    use super::*;

    fn row() -> DetailRow {
        DetailRow {
            label: "$.metadata.inner".into(),
            value: "visible value".into(),
            depth: 2,
            matched: false,
            filter_participant: false,
            fts_matched: false,
            value_highlights: Vec::new(),
            expandable: false,
            expanded: false,
            origin: DetailOrigin::Structured,
            value_kind: DetailValueKind::Present,
        }
    }

    #[test]
    fn nested_field_copies_full_path_and_scalar_value() {
        let row = row();
        assert_eq!(
            copyable_field_path(&row, false).as_deref(),
            Some("metadata.inner")
        );
        assert_eq!(
            copyable_field_value(&row, false).as_deref(),
            Some("visible value")
        );
    }

    #[test]
    fn aggregate_and_clipped_previews_are_not_copied_as_complete_values() {
        let mut row = row();
        row.expandable = true;
        row.value = "3 fields".into();
        assert_eq!(copyable_field_value(&row, false), None);
        row.expandable = false;
        row.value = "incomplete…".into();
        assert_eq!(copyable_field_value(&row, true), None);
        row.label = "$.partial…".into();
        assert_eq!(copyable_field_path(&row, true), None);
    }

    #[test]
    fn json_null_copies_its_literal() {
        let mut row = row();
        row.value_kind = DetailValueKind::JsonNull;
        assert_eq!(copyable_field_value(&row, false).as_deref(), Some("null"));
    }
}
