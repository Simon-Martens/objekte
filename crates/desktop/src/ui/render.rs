//! Read-only workspace layout inspired by DBDelve's dense sidebar and grid.

use gpui_kit::component::{
    Disableable as _, Icon, IconName, Selectable as _,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    input::Input,
    scroll::ScrollableElement as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, Focusable as _, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, Window, div, px, rgb,
};

use super::DesktopView;
use super::model::{ErrorField, UiAction};
use super::state::Busy;
use super::theme::palette;

/// A compact, left-aligned navigation control with keyboard and screen-reader support.
fn sidebar_item(
    id: String,
    icon: gpui_kit::assets::IconName,
    label: String,
    selected: bool,
) -> Button {
    Button::new(id)
        .ghost()
        .w_full()
        .h(px(30.))
        .selected(selected)
        .accessibility_label(label.clone())
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .w_full()
                .child(Icon::new(icon).size_4())
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_left()
                        .text_sm()
                        .child(label),
                ),
        )
}


impl DesktopView {
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = palette(self.state.dark);
        let root = self.state.root.as_deref().and_then(|path| {
            std::path::Path::new(path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        });
        let collection = self.state.search.collection.as_deref();
        div()
            .flex()
            .items_center()
            .h(px(44.))
            .flex_shrink_0()
            .gap_2()
            .px_3()
            .bg(rgb(colors.panel))
            .border_b_1()
            .border_color(rgb(colors.border))
            .child(
                div()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child("Objekte"),
            )
            .when(self.state.root.is_some(), |this| {
                this.child(
                    Button::new("sidebar-toggle")
                        .ghost()
                        .icon(IconName::PanelLeft)
                        .tooltip(if self.state.sidebar_visible {
                            "Hide collections"
                        } else {
                            "Show collections"
                        })
                        .accessibility_label("Toggle collection sidebar")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.sidebar_visible = !this.state.sidebar_visible;
                            cx.notify();
                        })),
                )
            })
            .when_some(root, |this, root| {
                this.child(div().text_color(rgb(colors.secondary)).child("/"))
                    .child(div().min_w_0().text_sm().child(root))
            })
            .when_some(collection, |this, collection| {
                this.child(div().text_color(rgb(colors.secondary)).child("/"))
                    .child(div().min_w_0().text_sm().child(collection.to_owned()))
            })
            .when(self.state.root.is_some(), |this| {
                this.child(
                    div()
                        .ml_2()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .bg(rgb(colors.raised))
                        .text_xs()
                        .text_color(rgb(colors.secondary))
                        .child("Read-only"),
                )
            })
            .child(div().flex_1())
            .child(
                Button::new("open-panel")
                    .icon(IconName::FolderOpen)
                    .label("Open…")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.show_open_dialog(window, cx);
                    })),
            )
            .child(
                Button::new("actions-panel")
                    .icon(IconName::Ellipsis)
                    .label("Menu")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.show_actions = !this.state.show_actions;
                        cx.notify();
                    })),
            )
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = palette(self.state.dark);
        let selected = self.state.search.collection.clone();
        let mut sidebar = div()
            .id("collection-sidebar")
            .flex()
            .flex_col()
            .w(px(220.))
            .h_full()
            .flex_shrink_0()
            .gap_1()
            .px_2()
            .py_3()
            .bg(rgb(colors.panel))
            .border_r_1()
            .border_color(rgb(colors.border))
            .child(
                div()
                    .px_2()
                    .mb_1()
                    .text_xs()
                    .text_color(rgb(colors.secondary))
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child("COLLECTIONS"),
            );
        if !self.state.single_collection && !self.state.collections.is_empty() {
            sidebar = sidebar.child(
                sidebar_item(
                    "scope-all".into(),
                    gpui_kit::assets::IconName::LayoutDashboard,
                    "All collections".into(),
                    selected.is_none(),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state.search.collection = None;
                    this.search_from_inputs(false, cx);
                })),
            );
        }
        let names = self.state.collections.clone();
        sidebar = sidebar.children(names.into_iter().enumerate().map(|(index, name)| {
            let chosen = selected.as_deref() == Some(name.as_str());
            sidebar_item(
                format!("scope-{index}"),
                if chosen {
                    gpui_kit::assets::IconName::FolderOpen
                } else {
                    gpui_kit::assets::IconName::FolderClosed
                },
                name.clone(),
                chosen,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.state.search.collection = Some(name.clone());
                this.search_from_inputs(false, cx);
            }))
        }));
        sidebar
            .child(div().flex_1())
            .child(
                div()
                    .border_t_1()
                    .border_color(rgb(colors.border))
                    .pt_3()
                    .px_2()
                    .text_xs()
                    .text_color(rgb(colors.secondary))
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child("QUERIES"),
            )
            .child(
                sidebar_item(
                    "sidebar-history".into(),
                    gpui_kit::assets::IconName::Clock,
                    "History".into(),
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state.show_history = true;
                    this.state.show_saved = false;
                    this.state.show_actions = true;
                    cx.notify();
                })),
            )
            .child(
                sidebar_item(
                    "sidebar-saved".into(),
                    gpui_kit::assets::IconName::Bookmark,
                    "Saved queries".into(),
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state.show_saved = true;
                    this.state.show_history = false;
                    this.state.show_actions = true;
                    cx.notify();
                })),
            )
            .child(
                div()
                    .px_2()
                    .mt_2()
                    .text_xs()
                    .text_color(rgb(colors.secondary))
                    .child(format!("{} collections", self.state.collections.len())),
            )
    }

    fn render_query(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let full_error = self.state.errors.get(&ErrorField::FullText).cloned();
        let filter_error = self.state.errors.get(&ErrorField::Structured).cloned();
        let range_error = self.state.errors.get(&ErrorField::Range).cloned();
        let sort_error = self.state.errors.get(&ErrorField::Sort).cloned();
        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap_2()
            .p_3()
            .child(
                div()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child("Search"),
            )
            .child(div().text_sm().child("Full Text"))
            .child(
                Input::new(&self.inputs.full_text)
                    .id("full-text-input")
                    .w_full(),
            )
            .when(full_error.is_some(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(rgb(0xc2410c))
                        .child(full_error.unwrap_or_default()),
                )
            })
            .child(div().text_sm().child("Structured"))
            .child(
                Input::new(&self.inputs.structured)
                    .id("structured-input")
                    .w_full(),
            )
            .when(filter_error.is_some(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(rgb(0xc2410c))
                        .child(filter_error.unwrap_or_default()),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        Checkbox::new("case-sensitive")
                            .accessibility_label("Case sensitive")
                            .child(
                                div()
                                    .text_color(rgb(if self.state.dark {
                                        0xe2e8f0
                                    } else {
                                        0x172033
                                    }))
                                    .child("Case sensitive"),
                            )
                            .checked(self.state.search.case_sensitive)
                            .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                this.state.search.case_sensitive = *checked;
                                this.search_from_inputs(false, cx);
                            })),
                    )
                    .child(
                        Checkbox::new("regex-search")
                            .accessibility_label("Regex")
                            .child(
                                div()
                                    .text_color(rgb(if self.state.dark {
                                        0xe2e8f0
                                    } else {
                                        0x172033
                                    }))
                                    .child("Regex"),
                            )
                            .checked(self.state.search.regex)
                            .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                this.state.search.regex = *checked;
                                this.search_from_inputs(false, cx);
                            })),
                    )
                    .child(
                        Checkbox::new("exact-count")
                            .accessibility_label("Exact count")
                            .child(
                                div()
                                    .text_color(rgb(if self.state.dark {
                                        0xe2e8f0
                                    } else {
                                        0x172033
                                    }))
                                    .child("Exact count"),
                            )
                            .checked(self.state.search.count)
                            .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                this.state.search.count = *checked;
                                this.search_from_inputs(false, cx);
                            })),
                    )
                    .child(
                        Button::new("run-query")
                            .primary()
                            .icon(IconName::Search)
                            .label("Run Query")
                            .on_click(
                                cx.listener(|this, _, _, cx| this.search_from_inputs(true, cx)),
                            ),
                    )
                    .child(
                        Button::new("cancel-query")
                            .icon(IconName::Close)
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, _| this.send(UiAction::Cancel))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().text_sm().child("Sort"))
                    .child(Input::new(&self.inputs.sort).id("sort-input").w(px(170.)))
                    .child(div().text_sm().child("Offset"))
                    .child(
                        Input::new(&self.inputs.offset)
                            .id("offset-input")
                            .w(px(90.)),
                    )
                    .child(div().text_sm().child("Limit"))
                    .child(Input::new(&self.inputs.limit).id("limit-input").w(px(90.)))
                    .child(
                        Button::new("columns-button")
                            .icon(IconName::PanelLeft)
                            .label("Columns")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.show_columns = !this.state.show_columns;
                                this.state.show_actions |= this.state.show_columns;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("preview-button")
                            .icon(if self.state.show_preview {
                                IconName::EyeOff
                            } else {
                                IconName::Eye
                            })
                            .label(if self.state.show_preview {
                                "Hide preview"
                            } else {
                                "Show preview"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.show_preview = !this.state.show_preview;
                                this.send(UiAction::TogglePreview);
                                cx.notify();
                            })),
                    ),
            )
            .when(sort_error.is_some(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(rgb(0xc2410c))
                        .child(sort_error.unwrap_or_default()),
                )
            })
            .when(range_error.is_some(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(rgb(0xc2410c))
                        .child(range_error.unwrap_or_default()),
                )
            })
    }

    fn render_grid(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = palette(self.state.dark);
        if self.state.root.is_none() {
            let opening = self.state.busy == Busy::Opening;
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_3()
                .bg(rgb(colors.canvas))
                .child(
                    div()
                        .text_xl()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .child(if opening {
                            "Opening data"
                        } else {
                            "Explore your data"
                        }),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(colors.secondary))
                        .child(if opening {
                            "Building the search index. This window remains usable."
                        } else {
                            "Open a folder to browse, search, and inspect its records."
                        }),
                )
                .when(!opening && !self.state.open_dialog.visible, |this| {
                    this.child(
                        Button::new("empty-open")
                            .primary()
                            .icon(IconName::FolderOpen)
                            .label("Open data…")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_open_dialog(window, cx);
                            })),
                    )
                })
                .into_any_element();
        }

        let page = &self.state.page;
        let visible_columns = page
            .columns
            .iter()
            .enumerate()
            .map(|(index, column)| {
                let width = *self.state.column_widths.get(column).unwrap_or(&160.);
                (index, column.as_str(), width)
            })
            .collect::<Vec<_>>();
        let table_width = 52.
            + visible_columns
                .iter()
                .map(|(_, _, width)| *width)
                .sum::<f32>();
        let selected = page.selected;
        let offset = page.offset;
        let sort = self.state.search.sort.trim();
        let header = div()
            .flex()
            .items_center()
            .w_full()
            .min_w(px(table_width))
            .h(px(32.))
            .flex_shrink_0()
            .bg(rgb(colors.raised))
            .border_b_1()
            .border_color(rgb(colors.border))
            .child(
                div()
                    .w(px(52.))
                    .flex_shrink_0()
                    .px_2()
                    .text_xs()
                    .text_color(rgb(colors.secondary))
                    .child("#"),
            )
            .children(visible_columns.iter().map(|(_, column, width)| {
                let name = (*column).to_owned();
                let label = if sort == format!("{name} ASC") {
                    format!("{name} ↑")
                } else if sort == format!("{name} DESC") {
                    format!("{name} ↓")
                } else {
                    name.clone()
                };
                Button::new(format!("sort-{name}"))
                    .ghost()
                    .label(label)
                    .tooltip(format!("Sort by {name}: ascending, descending, clear"))
                    .w(px(*width))
                    .h(px(31.))
                    .justify_start()
                    .text_xs()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.sort_by_column(name.clone(), window, cx);
                    }))
            }));
        let rows = div()
            .id("results-scroll")
            .track_focus(&self.grid_focus)
            .track_scroll(&self.grid_scroll)
            .on_key_down(cx.listener(|this, event: &gpui_kit::KeyDownEvent, _, cx| {
                match event.keystroke.key.as_str() {
                    "up" => this.select_relative(-1, cx),
                    "down" => this.select_relative(1, cx),
                    "home" => this.select_absolute(false, cx),
                    "end" => this.select_absolute(true, cx),
                    "pageup" => this.send(UiAction::PageRelative(-1)),
                    "pagedown" => this.send(UiAction::PageRelative(1)),
                    "enter" => {
                        this.state.inspector_visible = true;
                        this.send(UiAction::OpenDetail);
                        cx.notify();
                    }
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .w_full()
            .min_w(px(table_width))
            .bg(rgb(colors.panel))
            .children(page.rows.iter().enumerate().map(|(index, _row)| {
                let chosen = selected == Some(index);
                let shade = if chosen {
                    colors.selected
                } else if index % 2 == 0 {
                    colors.panel
                } else {
                    colors.row_alt
                };
                div()
                    .id(("result-row", index))
                    .flex()
                    .items_center()
                    .w_full()
                    .h(px(30.))
                    .flex_shrink_0()
                    .bg(rgb(shade))
                    .border_b_1()
                    .border_color(rgb(colors.border))
                    .child(
                        div()
                            .w(px(52.))
                            .flex_shrink_0()
                            .px_2()
                            .text_xs()
                            .text_color(rgb(colors.secondary))
                            .child((offset + index + 1).to_string()),
                    )
                    .children(visible_columns.iter().map(|(column_index, _, width)| {
                        div()
                            .w(px(*width))
                            .flex_shrink_0()
                            .px_2()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_sm()
                            .child(
                                self.state
                                    .grid_cache
                                    .rows
                                    .get(index)
                                    .and_then(|cells| cells.get(*column_index))
                                    .cloned()
                                    .unwrap_or_default(),
                            )
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.focus(&this.grid_focus, cx);
                        this.state.page.selected = Some(index);
                        this.state.inspector_visible = this.state.show_preview;
                        this.send(UiAction::SelectRow(index));
                        cx.notify();
                    }))
            }))
            .overflow_y_scrollbar();
        let first = if page.rows.is_empty() { 0 } else { offset + 1 };
        let last = offset + page.rows.len();
        let range = match page.exact_total {
            Some(total) => format!("{first}–{last} of {total} matches"),
            None if page.count_pending => format!("{first}–{last} · counting…"),
            None => format!("{first}–{last}"),
        };
        let range = if self.state.showing_last_valid_results {
            format!("Last valid: {range}")
        } else {
            range
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .bg(rgb(colors.panel))
            .child(
                div()
                    .id("table-horizontal")
                    .track_scroll(&self.grid_horizontal_scroll)
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(header)
                    .child(rows)
                    .overflow_x_scroll(),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(32.))
                    .flex_shrink_0()
                    .px_3()
                    .gap_2()
                    .border_t_1()
                    .border_color(rgb(colors.border))
                    .text_xs()
                    .text_color(rgb(colors.secondary))
                    .child(range)
                    .child(div().flex_1())
                    .child(
                        Button::new("previous-page")
                            .ghost()
                            .icon(IconName::ChevronLeft)
                            .tooltip("Previous page")
                            .accessibility_label("Previous page")
                            .disabled(!page.has_previous)
                            .on_click(cx.listener(|this, _, _, _| {
                                this.send(UiAction::PageRelative(-1));
                            })),
                    )
                    .child(format!("Page {}", offset / 100 + 1))
                    .child(
                        Button::new("next-page")
                            .ghost()
                            .icon(IconName::ChevronRight)
                            .tooltip("Next page")
                            .accessibility_label("Next page")
                            .disabled(!page.has_next)
                            .on_click(cx.listener(|this, _, _, _| {
                                this.send(UiAction::PageRelative(1));
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = palette(self.state.dark);
        let status = if self.state.busy == Busy::None {
            self.state.status.clone()
        } else {
            format!("Working: {}", self.state.status)
        };
        div()
            .flex()
            .items_center()
            .h(px(28.))
            .flex_shrink_0()
            .px_3()
            .bg(rgb(colors.panel))
            .border_t_1()
            .border_color(rgb(colors.border))
            .text_xs()
            .text_color(rgb(colors.secondary))
            .child(status)
            .child(div().flex_1())
            .child(
                Button::new("status-diagnostics")
                    .ghost()
                    .label("Diagnostics")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.show_diagnostics = !this.state.show_diagnostics;
                        this.state.show_actions = this.state.show_diagnostics;
                        cx.notify();
                    })),
            )
    }
}


impl Render for DesktopView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = palette(self.state.dark);
        div()
            .id("desktop-root")
            .relative()
            .on_mouse_move(
                cx.listener(|this, event: &gpui_kit::MouseMoveEvent, _, cx| {
                    if let Some((start_x, start_width)) = this.state.inspector_resize_origin {
                        let distance = start_x - f32::from(event.position.x);
                        this.state.inspector_width = (start_width + distance).clamp(300., 600.);
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.state.inspector_resize_origin = None;
                }),
            )
            .on_key_down(
                cx.listener(|this, event: &gpui_kit::KeyDownEvent, window, cx| {
                    let key = event.keystroke.key.as_str();
                    if event.keystroke.modifiers.control {
                        match key {
                            "o" => this.show_open_dialog(window, cx),
                            "f" => {
                                let focus = this.inputs.full_text.read(cx).focus_handle(cx);
                                window.focus(&focus, cx);
                            }
                            "l" => {
                                let focus = this.inputs.structured.read(cx).focus_handle(cx);
                                window.focus(&focus, cx);
                            }
                            _ => return,
                        }
                    } else {
                        match key {
                            "f1" => this.state.show_help = !this.state.show_help,
                            "escape" => {
                                this.state.show_help = false;
                                this.state.show_actions = false;
                                this.state.show_batch = false;
                            }
                            _ => return,
                        }
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(colors.canvas))
            .text_color(rgb(colors.text))
            .child(self.render_header(cx))
            .when(self.state.show_batch, |this| {
                this.child(self.render_batch(cx))
            })
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .when(
                        self.state.root.is_some() && self.state.sidebar_visible,
                        |this| this.child(self.render_sidebar(cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_h_0()
                            .min_w_0()
                            .when(self.state.root.is_some(), |this| {
                                this.child(self.render_query(cx))
                            })
                            .child(self.render_grid(cx)),
                    )
                    .when(
                        self.state.inspector_visible
                            && (self.state.detail.is_some() || self.state.page.selected.is_some()),
                        |this| this.child(self.render_detail(cx)),
                    ),
            )
            .when(self.state.show_actions, |this| {
                this.child(self.render_actions(cx))
            })
            .when(self.state.show_help, |this| this.child(self.render_help()))
            .child(self.render_status(cx))
            .when(self.state.open_dialog.visible, |this| {
                this.child(self.render_open_dialog(cx))
            })
    }
}
