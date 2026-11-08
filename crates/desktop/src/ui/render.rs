//! Read-only workspace layout inspired by DBDelve's dense sidebar and grid.

use gpui_kit::assets::IconName as AssetIconName;
use gpui_kit::component::{
    Disableable as _, Icon, IconName, Selectable as _,
    button::{Button, ButtonVariants},
    input::Input,
    menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem},
    scroll::ScrollableElement as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, Entity, Focusable as _, FontWeight, HighlightStyle, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _,
    StyledText, Window, div, list, px, rgb,
};
use objekte_search_core::query::parse_structured_filter;

use super::DesktopView;
use super::model::UiAction;
use super::sort;
use super::state::{ActionPanel, Busy};
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

fn panel_menu_item(
    target: Entity<DesktopView>,
    label: &'static str,
    icon: AssetIconName,
    panel: ActionPanel,
) -> PopupMenuItem {
    PopupMenuItem::new(label)
        .icon(icon)
        .on_click(move |_, _, cx| {
            target.update(cx, |this, cx| {
                this.state.action_panel = panel;
                this.state.show_actions = true;
                cx.notify();
            });
        })
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
                        .text_color(rgb(colors.accent))
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
            .child({
                let target = cx.entity();
                let can_save = matches!(
                    self.state.filter_validation,
                    Some(super::model::FilterValidationState::Valid)
                );
                let dark = self.state.dark;
                Button::new("actions-panel")
                    .icon(IconName::Menu)
                    .tooltip("Application menu")
                    .accessibility_label("Application menu")
                    .dropdown_menu(move |menu, _, _| {
                        let save_target = target.clone();
                        let batch_target = target.clone();
                        let theme_target = target.clone();
                        let help_target = target.clone();
                        menu.item(
                            PopupMenuItem::new("Save current query")
                                .icon(AssetIconName::Bookmark)
                                .disabled(!can_save)
                                .on_click(move |_, _, cx| {
                                    save_target.update(cx, |this, cx| {
                                        let draft = this.query_from_inputs(cx);
                                        this.send(UiAction::SaveQuery(draft));
                                    });
                                }),
                        )
                        .item(panel_menu_item(
                            target.clone(),
                            "Columns…",
                            AssetIconName::Columns3,
                            ActionPanel::Columns,
                        ))
                        .separator()
                        .item(panel_menu_item(
                            target.clone(),
                            "Export…",
                            AssetIconName::ArrowRight,
                            ActionPanel::Export,
                        ))
                        .item(panel_menu_item(
                            target.clone(),
                            "Index…",
                            AssetIconName::HardDrive,
                            ActionPanel::Index,
                        ))
                        .item(
                            PopupMenuItem::new("Run query batch…")
                                .icon(AssetIconName::SquareTerminal)
                                .on_click(move |_, _, cx| {
                                    batch_target.update(cx, |this, cx| {
                                        this.state.show_batch = true;
                                        cx.notify();
                                    });
                                }),
                        )
                        .separator()
                        .item(panel_menu_item(
                            target.clone(),
                            "Diagnostics",
                            AssetIconName::Info,
                            ActionPanel::Diagnostics,
                        ))
                        .item(
                            PopupMenuItem::new(if dark { "Light theme" } else { "Dark theme" })
                                .icon(if dark {
                                    AssetIconName::Sun
                                } else {
                                    AssetIconName::Moon
                                })
                                .on_click(move |_, window, cx| {
                                    theme_target.update(cx, |this, cx| {
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
                                        super::typography::apply_theme_fonts(cx);
                                        this.send(UiAction::SetTheme {
                                            dark: this.state.dark,
                                        });
                                        this.builder_view.update(cx, |builder, cx| {
                                            builder.set_dark(this.state.dark, cx)
                                        });
                                        cx.notify();
                                    });
                                }),
                        )
                        .item(
                            PopupMenuItem::new("Help and shortcuts")
                                .icon(AssetIconName::Info)
                                .on_click(move |_, _, cx| {
                                    help_target.update(cx, |this, cx| {
                                        this.state.show_help = true;
                                        cx.notify();
                                    });
                                }),
                        )
                    })
            })
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
            .overflow_y_scrollbar()
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
        sidebar = sidebar.child(
            div()
                .border_t_1()
                .border_color(rgb(colors.border))
                .mt_3()
                .pt_3()
                .px_2()
                .text_xs()
                .text_color(rgb(colors.secondary))
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .child("SAVED QUERIES"),
        );
        if self.state.saved.is_empty() {
            sidebar = sidebar.child(
                div()
                    .px_2()
                    .py_1()
                    .text_sm()
                    .text_color(rgb(colors.secondary))
                    .child("No saved queries"),
            );
        }
        for entry in &self.state.saved {
            let Some(id) = entry.id else { continue };
            let title = entry
                .name
                .clone()
                .unwrap_or_else(|| super::preferences::generated_title(&entry.draft));
            let description = super::preferences::generated_title(&entry.draft);
            let active = self.state.search == entry.draft;
            let mut row = div()
                .id(("saved-row", id as usize))
                .flex()
                .items_center()
                .min_w_0();
            if self.renaming_saved == Some(id) {
                row = row.child(
                    Input::new(&self.inputs.rename_title)
                        .id("rename-saved-input")
                        .flex_1()
                        .min_w_0(),
                );
            } else {
                row = row.child(
                    Button::new(format!("sidebar-saved-{id}"))
                        .ghost()
                        .flex_1()
                        .min_w_0()
                        .h(px(30.))
                        .selected(active)
                        .tooltip(description)
                        .accessibility_label(title.clone())
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_left()
                                .text_sm()
                                .child(title),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.focused_saved = Some(id);
                            this.send(UiAction::RestoreSaved(id));
                            cx.notify();
                        })),
                );
            }
            row = row.child(
                Button::new(format!("sidebar-remove-{id}"))
                    .ghost()
                    .icon(IconName::Star)
                    .tooltip("Remove saved query")
                    .accessibility_label("Remove saved query")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.send(UiAction::RemoveSaved(id));
                        cx.notify();
                    })),
            );
            let target = cx.entity();
            sidebar = sidebar.child(row.context_menu(move |menu, _, _| {
                let rename_target = target.clone();
                let remove_target = target.clone();
                menu.item(PopupMenuItem::new("Rename").on_click(move |_, window, cx| {
                    rename_target.update(cx, |this, cx| this.begin_rename(id, window, cx));
                }))
                .item(PopupMenuItem::new("Remove").on_click(move |_, _, cx| {
                    remove_target.update(cx, |this, _| this.send(UiAction::RemoveSaved(id)));
                }))
            }));
        }
        sidebar = sidebar.child(
            sidebar_item(
                "sidebar-history".into(),
                if self.state.history_expanded {
                    gpui_kit::assets::IconName::ChevronDown
                } else {
                    gpui_kit::assets::IconName::ChevronRight
                },
                "History".into(),
                self.state.history_expanded,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.state.history_expanded = !this.state.history_expanded;
                cx.notify();
            })),
        );
        if self.state.history_expanded {
            for (index, entry) in self.state.history.iter().rev().enumerate() {
                let title = super::preferences::generated_title(&entry.draft);
                let bookmark = self
                    .state
                    .saved
                    .iter()
                    .find(|saved| saved.draft == entry.draft)
                    .and_then(|saved| saved.id);
                let draft = entry.draft.clone();
                let active = self.state.search == entry.draft;
                sidebar = sidebar.child(
                    div()
                        .flex()
                        .items_center()
                        .min_w_0()
                        .child(
                            Button::new(format!("sidebar-history-{index}"))
                                .ghost()
                                .flex_1()
                                .min_w_0()
                                .h(px(30.))
                                .selected(active)
                                .tooltip(title.clone())
                                .accessibility_label(title.clone())
                                .child(
                                    div()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_left()
                                        .text_sm()
                                        .child(title),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.send(UiAction::RestoreHistory(index));
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new(format!("sidebar-star-{index}"))
                                .ghost()
                                .icon(IconName::Star)
                                .selected(bookmark.is_some())
                                .tooltip(if bookmark.is_some() {
                                    "Remove bookmark"
                                } else {
                                    "Save query"
                                })
                                .accessibility_label(if bookmark.is_some() {
                                    "Remove bookmark"
                                } else {
                                    "Save query"
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(id) = bookmark {
                                        this.send(UiAction::RemoveSaved(id));
                                    } else {
                                        this.send(UiAction::SaveQuery(draft.clone()));
                                    }
                                    cx.notify();
                                })),
                        ),
                );
            }
        }
        sidebar.child(
            div()
                .px_2()
                .mt_2()
                .text_xs()
                .text_color(rgb(colors.secondary))
                .child(format!("{} collections", self.state.collections.len())),
        )
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
        let loaded_count = self.state.loaded_count;
        let sort = self
            .state
            .search
            .filter_enabled
            .then(|| parse_structured_filter(&self.state.search.structured).ok())
            .flatten()
            .and_then(|parsed| parsed.sort)
            .unwrap_or_default();
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
                let wrapped = self.state.wrapped_columns.contains(&name);
                let sortable = sort::sort_field_for_column(&name);
                let display = sort::column_label(&name).to_owned();
                let label = sort
                    .iter()
                    .position(|field| sortable.is_some_and(|key| field.field == key))
                    .map(|index| {
                        format!(
                            "{display} {}{}",
                            if sort[index].descending { "↓" } else { "↑" },
                            if sort.len() > 1 {
                                format!("{}", index + 1)
                            } else {
                                String::new()
                            }
                        )
                    })
                    .unwrap_or(display);
                let sort_name = name.clone();
                let resize_name = name.clone();
                let menu_name = name.clone();
                let menu_target = cx.entity();
                div()
                    .id(format!("header-{name}"))
                    .flex()
                    .items_center()
                    .w(px(*width))
                    .h(px(31.))
                    .flex_shrink_0()
                    .child(
                        Button::new(format!("sort-{name}"))
                            .ghost()
                            .label(label)
                            .tooltip(if sortable.is_some() {
                                format!(
                                    "Sort by {}: ascending, descending, clear",
                                    sort::column_label(&name)
                                )
                            } else {
                                "Match excerpts cannot be sorted".to_owned()
                            })
                            .disabled(sortable.is_none())
                            .w(px((*width - 7.).max(40.)))
                            .h(px(31.))
                            .justify_start()
                            .text_xs()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.sort_by_column(sort_name.clone(), window, cx);
                            })),
                    )
                    .child(
                        div()
                            .id(format!("resize-{name}"))
                            .w(px(7.))
                            .h_full()
                            .cursor_col_resize()
                            .on_mouse_down(
                                gpui_kit::MouseButton::Left,
                                cx.listener(
                                    move |this, event: &gpui_kit::MouseDownEvent, _, cx| {
                                        let width = *this
                                            .state
                                            .column_widths
                                            .get(&resize_name)
                                            .unwrap_or(&160.);
                                        this.state.column_resize_origin = Some((
                                            resize_name.clone(),
                                            f32::from(event.position.x),
                                            width,
                                        ));
                                        this.state.status = format!(
                                            "Resizing {}…",
                                            sort::column_label(&resize_name)
                                        );
                                        cx.notify();
                                        cx.stop_propagation();
                                    },
                                ),
                            ),
                    )
                    .context_menu(move |menu, _, _| {
                        let target = menu_target.clone();
                        let column = menu_name.clone();
                        menu.item(PopupMenuItem::new("Wrap text").checked(wrapped).on_click(
                            move |_, _, cx| {
                                target.update(cx, |this, _| {
                                    this.send(UiAction::ToggleWrapColumn(column.clone()))
                                });
                            },
                        ))
                    })
            }));
        let target = cx.entity();
        let row_columns = visible_columns
            .iter()
            .map(|(index, column, width)| {
                (
                    *index,
                    (*column).to_owned(),
                    *width,
                    self.state
                        .wrapped_columns
                        .iter()
                        .any(|wrapped| wrapped == *column),
                )
            })
            .collect::<Vec<_>>();
        let rows = div()
            .id("results-scroll")
            .track_focus(&self.grid_focus)
            .on_key_down(cx.listener(|this, event: &gpui_kit::KeyDownEvent, _, cx| {
                match event.keystroke.key.as_str() {
                    "up" => this.select_relative(-1, cx),
                    "down" => this.select_relative(1, cx),
                    "home" => this.select_absolute(false, cx),
                    "end" => this.select_absolute(true, cx),
                    "pageup" => this.grid_list.scroll_by(px(-600.)),
                    "pagedown" => this.grid_list.scroll_by(px(600.)),
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
            .child(
                list(self.grid_list.clone(), move |index, _, cx| {
                    let state = &target.read(cx).state;
                    let selected = state.page.selected == Some(index);
                    let cells = state
                        .projected_rows
                        .get(&index)
                        .map(|row| row.cells.clone())
                        .unwrap_or_default();
                    let match_highlights = state
                        .projected_rows
                        .get(&index)
                        .map(|row| row.match_highlights.clone())
                        .unwrap_or_default();
                    let shade = if selected {
                        colors.selected
                    } else if index % 2 == 0 {
                        colors.panel
                    } else {
                        colors.row_alt
                    };
                    div()
                        .id(("result-row", index))
                        .flex()
                        .items_start()
                        .w_full()
                        .min_h(px(30.))
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
                                .child((index + 1).to_string()),
                        )
                        .children(row_columns.iter().map(
                            |(column_index, column, width, wrapped)| {
                                let cell = cells.get(*column_index).cloned().unwrap_or_default();
                                let content = if column == "@match" {
                                    let highlights = match_highlights
                                        .iter()
                                        .filter(|(range, _)| {
                                            range.end <= cell.len()
                                                && cell.is_char_boundary(range.start)
                                                && cell.is_char_boundary(range.end)
                                        })
                                        .map(|(range, span)| {
                                            (
                                                range.clone(),
                                                HighlightStyle {
                                                    font_weight: Some(FontWeight::BOLD),
                                                    color: (*span)
                                                        .then(|| rgb(colors.accent).into()),
                                                    ..Default::default()
                                                },
                                            )
                                        })
                                        .collect::<Vec<_>>();
                                    StyledText::new(cell.clone())
                                        .with_highlights(highlights)
                                        .into_any_element()
                                } else {
                                    div().child(cell).into_any_element()
                                };
                                div()
                                    .w(px(*width))
                                    .flex_shrink_0()
                                    .px_2()
                                    .overflow_hidden()
                                    .text_sm()
                                    .when(*wrapped, |this| this.whitespace_normal().line_clamp(4))
                                    .when(!*wrapped, |this| {
                                        this.whitespace_nowrap().text_ellipsis()
                                    })
                                    .child(content)
                            },
                        ))
                        .on_click({
                            let target = target.clone();
                            move |_, window, cx| {
                                target.update(cx, |this, cx| {
                                    window.focus(&this.grid_focus, cx);
                                    this.state.page.selected = Some(index);
                                    if this.state.show_preview {
                                        this.state.inspector_visible = true;
                                    }
                                    this.send(if this.state.show_preview {
                                        UiAction::OpenRecord(index)
                                    } else {
                                        UiAction::SelectRow(index)
                                    });
                                    cx.notify();
                                });
                            }
                        })
                        .into_any_element()
                })
                .flex_1()
                .min_h_0()
                .min_w(px(table_width)),
            );
        let first = if self.state.delivered_count == 0 {
            0
        } else {
            page.offset + 1
        };
        let last = loaded_count;
        let mut range = match page.exact_total {
            Some(total) => format!("{first}–{last} of {total} matches"),
            None if page.count_pending => {
                format!("{} loaded · counting…", self.state.delivered_count)
            }
            None => format!("{} loaded", self.state.delivered_count),
        };
        let undisplayed = self
            .state
            .available_columns
            .iter()
            .filter(|column| !self.state.hidden_columns.contains(column))
            .count()
            .saturating_sub(page.columns.len());
        if undisplayed > 0 {
            range.push_str(&format!(" · +{undisplayed} columns (Columns)"));
        }
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
                        Button::new("table-columns")
                            .ghost()
                            .icon(IconName::PanelLeft)
                            .label("Columns")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.action_panel = ActionPanel::Columns;
                                this.state.show_actions = true;
                                cx.notify();
                            })),
                    )
                    .when(page.exact_total.is_none(), |this| {
                        this.child(
                            Button::new("count-exact")
                                .ghost()
                                .label(if page.count_pending {
                                    "Counting…"
                                } else {
                                    "Count all"
                                })
                                .disabled(page.count_pending)
                                .on_click(cx.listener(|this, _, _, _| {
                                    this.send(UiAction::CountExact);
                                })),
                        )
                    })
                    .when(self.state.continuation_error.is_some(), |this| {
                        this.child(
                            Button::new("retry-more")
                                .ghost()
                                .label("Retry loading")
                                .on_click(cx.listener(|this, _, _, _| {
                                    this.send(UiAction::RetryContinuation);
                                })),
                        )
                    })
                    .when(self.state.loading_more, |this| this.child("Loading more…")),
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
            .when(self.state.preference_write_failed, |this| {
                this.child(
                    Button::new("retry-preferences")
                        .ghost()
                        .label("Retry save")
                        .tooltip("Retry saving desktop preferences")
                        .on_click(cx.listener(|this, _, _, _| {
                            this.send(UiAction::RetryPreferences);
                        })),
                )
            })
            .child(
                Button::new("status-diagnostics")
                    .ghost()
                    .label("Diagnostics")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let was_open = this.state.show_actions
                            && this.state.action_panel == ActionPanel::Diagnostics;
                        this.state.show_actions = !was_open;
                        this.state.action_panel = ActionPanel::Diagnostics;
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
                    if let Some((column, start_x, start_width)) = &this.state.column_resize_origin {
                        let width = (*start_width + f32::from(event.position.x) - *start_x)
                            .clamp(80., 600.);
                        this.state.column_widths.insert(column.clone(), width);
                        let first = this
                            .grid_list
                            .logical_scroll_top()
                            .item_ix
                            .saturating_sub(10);
                        this.grid_list
                            .remeasure_items(first..(first + 100).min(this.state.loaded_count));
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.state.inspector_resize_origin = None;
                    if let Some((column, _, _)) = this.state.column_resize_origin.take() {
                        if let Some(width) = this.state.column_widths.get(&column) {
                            this.state.status = format!(
                                "{} width: {} px",
                                sort::column_label(&column),
                                width.round() as u16
                            );
                            this.send(UiAction::ResizeColumn {
                                column,
                                width: width.round() as u16,
                            });
                            cx.notify();
                        }
                    }
                }),
            )
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, event: &gpui_kit::MouseDownEvent, window, cx| {
                    if !this.state.show_filter_builder {
                        return;
                    }
                    let x = f32::from(event.position.x);
                    let y = f32::from(event.position.y);
                    let width = f32::from(window.bounds().size.width);
                    let sidebar = if this.state.sidebar_visible { 220. } else { 0. };
                    let right = if this.state.inspector_visible {
                        this.state.inspector_width + 8.
                    } else {
                        8.
                    };
                    let in_filter_line =
                        (95.0..180.0).contains(&y) && x >= sidebar + 12. && x <= width - right;
                    let in_builder = (150.0..470.0).contains(&y)
                        && x >= width - right - 560.
                        && x <= width - right;
                    if !in_filter_line && !in_builder {
                        this.state.show_filter_builder = false;
                        cx.notify();
                    }
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
                                if !this.state.search.filter_enabled {
                                    this.state.search.filter_enabled = true;
                                    this.search_from_inputs(false, cx);
                                }
                                let focus = this.inputs.structured.read(cx).focus_handle(cx);
                                window.focus(&focus, cx);
                            }
                            _ => return,
                        }
                    } else {
                        match key {
                            "f1" => this.state.show_help = !this.state.show_help,
                            "f2" if this.focused_saved.is_some() => {
                                this.begin_rename(this.focused_saved.unwrap(), window, cx);
                            }
                            "escape" if this.renaming_saved.is_some() => {
                                this.renaming_saved = None;
                            }
                            "escape" if this.state.open_dialog.visible => {
                                this.close_open_dialog(cx);
                            }
                            "escape" if this.state.show_filter_builder => {
                                this.state.show_filter_builder = false;
                                let focus = this.inputs.structured.read(cx).focus_handle(cx);
                                window.focus(&focus, cx);
                            }
                            "escape" if this.state.show_help => this.state.show_help = false,
                            "escape" if this.state.show_actions => this.state.show_actions = false,
                            "escape" if this.state.show_batch => this.state.show_batch = false,
                            "escape" => return,
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
            .font_family(super::typography::SANS)
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
                                this.child(self.render_query_bar(cx))
                            })
                            .child(self.render_grid(cx)),
                    )
                    .when(
                        self.state.inspector_visible && self.state.active_tab.is_some(),
                        |this| this.child(self.render_detail(cx)),
                    ),
            )
            .when(self.state.show_filter_builder, |this| {
                this.child(self.render_builder_overlay())
            })
            .when(self.state.show_actions, |this| {
                this.child(self.render_actions(cx))
            })
            .when(self.state.show_help, |this| {
                this.child(self.render_help(cx))
            })
            .child(self.render_status(cx))
            .when(self.state.open_dialog.visible, |this| {
                this.child(self.render_open_dialog(cx))
            })
    }
}
