//! Read-only workspace layout inspired by DBDelve's dense sidebar and grid.

use gpui_kit::component::{
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    input::Input,
    scroll::ScrollableElement as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, Window, div, px, rgb,
};

use super::DesktopView;
use super::model::{ErrorField, UiAction};
use super::state::Busy;


impl DesktopView {
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let root = self
            .state
            .root
            .clone()
            .unwrap_or_else(|| "No repository open".into());
        div()
            .flex()
            .items_center()
            .gap_3()
            .px_4()
            .py_2()
            .bg(rgb(if self.state.dark { 0x171c25 } else { 0xe9eef5 }))
            .child(div().text_lg().child("Objekte"))
            .child(div().flex_1().min_w_0().text_sm().child(root))
            .child(
                Button::new("open-panel")
                    .label("Open")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.show_open_options = !this.state.show_open_options;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("batch-panel")
                    .label("Batch")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.show_batch = !this.state.show_batch;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("actions-panel")
                    .label("Actions")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.show_actions = !this.state.show_actions;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("theme-button")
                    .label(if self.state.dark { "Light" } else { "Dark" })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.dark = !this.state.dark;
                        this.send(UiAction::SetTheme {
                            dark: this.state.dark,
                        });
                        cx.notify();
                    })),
            )
            .child(
                Button::new("help-button")
                    .label("Help")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.show_help = !this.state.show_help;
                        this.send(UiAction::ShowHelp);
                        cx.notify();
                    })),
            )
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.state.search.collection.clone();
        let mut sidebar = div()
            .id("collection-sidebar")
            .flex()
            .flex_col()
            .w(px(220.))
            .h_full()
            .gap_1()
            .p_3()
            .bg(rgb(if self.state.dark { 0x131922 } else { 0xf0f4f8 }))
            .child(div().child("Collections"));
        if !self.state.single_collection {
            sidebar = sidebar.child(Button::new("scope-all").label("All collections").on_click(
                cx.listener(|this, _, _, cx| {
                    this.state.search.collection = None;
                    this.search_from_inputs(false, cx);
                }),
            ));
        }
        let names = self.state.collections.clone();
        sidebar
            .children(names.into_iter().enumerate().map(|(index, name)| {
                let chosen = selected.as_deref() == Some(name.as_str());
                Button::new(format!("scope-{index}"))
                    .label(if chosen {
                        format!("● {name}")
                    } else {
                        name.clone()
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.search.collection = Some(name.clone());
                        this.search_from_inputs(false, cx);
                    }))
            }))
            .overflow_y_scrollbar()
    }

    fn render_query(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let full_error = self.state.errors.get(&ErrorField::FullText).cloned();
        let filter_error = self.state.errors.get(&ErrorField::Structured).cloned();
        let range_error = self.state.errors.get(&ErrorField::Range).cloned();
        let sort_error = self.state.errors.get(&ErrorField::Sort).cloned();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .child(div().child("Query"))
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
                            .label("Case sensitive")
                            .checked(self.state.search.case_sensitive)
                            .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                this.state.search.case_sensitive = *checked;
                                this.search_from_inputs(false, cx);
                            })),
                    )
                    .child(
                        Checkbox::new("regex-search")
                            .label("Regex")
                            .checked(self.state.search.regex)
                            .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                this.state.search.regex = *checked;
                                this.search_from_inputs(false, cx);
                            })),
                    )
                    .child(
                        Checkbox::new("exact-count")
                            .label("Exact count")
                            .checked(self.state.search.count)
                            .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                this.state.search.count = *checked;
                                this.search_from_inputs(false, cx);
                            })),
                    )
                    .child(
                        Button::new("run-query")
                            .primary()
                            .label("Run Query")
                            .on_click(
                                cx.listener(|this, _, _, cx| this.search_from_inputs(true, cx)),
                            ),
                    )
                    .child(
                        Button::new("cancel-query")
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
                            .label("Columns")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.show_columns = !this.state.show_columns;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("preview-button")
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
        let page = self.state.page.clone();
        let total = page.exact_total.map_or_else(
            || {
                if page.count_pending {
                    "Count pending".into()
                } else {
                    "Count not requested".into()
                }
            },
            |count| format!("{count} total"),
        );
        let header = div()
            .flex()
            .gap_1()
            .child(div().w(px(52.)).child("#"))
            .children(
                page.columns
                    .iter()
                    .map(|column| div().w(px(160.)).child(column.clone())),
            );
        let selected = page.selected;
        let visible_columns = page.columns.len();
        let offset = page.offset;
        let clipped = page.clipped;
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .p_3()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().child("Results"))
                    .child(div().text_sm().child(total))
                    .child(div().flex_1())
                    .child(Button::new("previous-page").label("Previous").on_click(
                        cx.listener(|this, _, _, _| this.send(UiAction::PageRelative(-1))),
                    ))
                    .child(Button::new("next-page").label("Next").on_click(
                        cx.listener(|this, _, _, _| this.send(UiAction::PageRelative(1))),
                    ))
                    .child(
                        Button::new("open-detail")
                            .label("Inspect")
                            .on_click(cx.listener(|this, _, _, _| this.send(UiAction::OpenDetail))),
                    ),
            )
            .child(
                div()
                    .id("results-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(header)
                    .children(page.rows.into_iter().enumerate().map(|(index, row)| {
                        let chosen = selected == Some(index);
                        let shade = if chosen {
                            0x294d62
                        } else if index % 2 == 0 {
                            0x222a35
                        } else {
                            0x1a222d
                        };
                        let mut line = div()
                            .id(("result-row", index))
                            .flex()
                            .gap_1()
                            .py_1()
                            .bg(rgb(shade))
                            .child(div().w(px(52.)).child((offset + index + 1).to_string()));
                        for cell in row.cells.into_iter().take(visible_columns) {
                            line = line.child(div().w(px(160.)).child(cell));
                        }
                        line.on_click(cx.listener(move |this, _, _, cx| {
                            this.state.page.selected = Some(index);
                            this.send(UiAction::SelectRow(index));
                            cx.notify();
                        }))
                    }))
                    .overflow_scrollbar(),
            )
            .when(clipped, |this| {
                this.child(
                    div()
                        .text_sm()
                        .child("Visible cells are clipped; exports contain complete records."),
                )
            })
    }

    fn render_status(&self) -> impl IntoElement {
        let status = if self.state.busy == Busy::None {
            self.state.status.clone()
        } else {
            format!("Working: {}", self.state.status)
        };
        div()
            .px_4()
            .py_2()
            .bg(rgb(if self.state.dark { 0x171c25 } else { 0xe9eef5 }))
            .text_sm()
            .child(status)
    }
}


impl Render for DesktopView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(if self.state.dark { 0x111821 } else { 0xf8fafc }))
            .text_color(rgb(if self.state.dark { 0xe2e8f0 } else { 0x172033 }))
            .child(self.render_header(cx))
            .when(self.state.show_open_options, |this| {
                this.child(self.render_open_options(cx))
            })
            .when(self.state.show_batch, |this| {
                this.child(self.render_batch(cx))
            })
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_sidebar(cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(self.render_query(cx))
                            .child(self.render_grid(cx)),
                    )
                    .child(self.render_detail(cx)),
            )
            .when(self.state.show_actions, |this| {
                this.child(self.render_actions(cx))
            })
            .when(self.state.show_help, |this| this.child(self.render_help()))
            .child(self.render_status())
    }
}
