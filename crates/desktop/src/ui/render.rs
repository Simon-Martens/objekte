//! Read-only workspace layout inspired by DBDelve's dense sidebar and grid.

use gpui_kit::assets::IconName as AssetIconName;
use gpui_kit::component::{
    Disableable as _, Icon, IconName, Selectable as _,
    button::{Button, ButtonVariants},
    input::Input,
    menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem},
    scroll::{ScrollableElement as _, Scrollbar, ScrollbarMode},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, Focusable as _, FontWeight, HighlightStyle,
    InteractiveElement as _, IntoElement, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, StyledText, Window, div, list, px, rgb,
};
use objekte_search_core::query::parse_structured_filter;

use super::model::{IndexTaskState, UiAction};
use super::sort;
use super::state::{ActionPanel, Busy};
use super::theme::palette;
use super::workspace::{Pane, SearchKind, TabId};
use super::{
    CloseWorkspaceTab, DesktopView, FocusPrimaryPane, FocusSecondaryPane, NewQueryTab,
    NextWorkspaceTab, PreviousWorkspaceTab,
};

#[derive(Clone)]
struct WorkspaceDrag {
    id: TabId,
}

struct WorkspaceDragPreview {
    title: String,
    dark: bool,
}

impl Render for WorkspaceDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let colors = palette(self.dark);
        div()
            .px_3()
            .py_2()
            .bg(rgb(colors.raised))
            .text_color(rgb(colors.text))
            .border_1()
            .border_color(rgb(colors.accent))
            .rounded_md()
            .child(self.title.clone())
    }
}

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

fn workspace_tab_menu(
    menu: PopupMenu,
    target: Entity<DesktopView>,
    id: TabId,
    pane: Pane,
    duplicate_label: Option<&'static str>,
) -> PopupMenu {
    let destination = pane.other();
    let move_target = target.clone();
    let duplicate_target = target.clone();
    let close_target = target;
    menu.item(
        PopupMenuItem::new(match destination {
            Pane::Primary => "Move to left pane",
            Pane::Secondary => "Move to right pane",
        })
        .icon(match destination {
            Pane::Primary => AssetIconName::PanelLeft,
            Pane::Secondary => AssetIconName::PanelRight,
        })
        .on_click(move |_, window, cx| {
            move_target.update(cx, |this, cx| {
                let at = this.state.workspace.group(destination).tabs.len();
                this.move_workspace_tab(id, destination, at, window, cx);
            });
        }),
    )
    .when_some(duplicate_label, |menu, label| {
        menu.item(
            PopupMenuItem::new(label)
                .icon(AssetIconName::Copy)
                .on_click(move |_, window, cx| {
                    duplicate_target.update(cx, |this, cx| {
                        this.duplicate_workspace_query(id, window, cx);
                    });
                }),
        )
    })
    .separator()
    .item(
        PopupMenuItem::new("Close tab")
            .icon(AssetIconName::X)
            .on_click(move |_, window, cx| {
                close_target.update(cx, |this, cx| {
                    this.close_workspace_tab(id, window, cx);
                });
            }),
    )
}

impl DesktopView {
    fn render_close_dialog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = palette(self.state.dark);
        let error = self
            .state
            .closing_index
            .as_ref()
            .and_then(|error| error.clone());
        let waiting = error.is_none();
        let surface = div()
            .w(px(430.))
            .max_w_full()
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .bg(rgb(colors.panel))
            .border_1()
            .border_color(rgb(colors.border))
            .rounded_lg()
            .child(div().text_lg().child("Closing Objekte"))
            .child(div().text_sm().child(error.unwrap_or_else(|| {
                "Saving the latest index and stopping background work…".into()
            })))
            .child(div().text_xs().text_color(rgb(colors.secondary)).child(
                "Source files are unaffected. You can close without saving the derived index cache.",
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .when(!waiting, |this| {
                        this.child(
                            Button::new("close-retry")
                                .label("Retry")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.send(UiAction::RetryClose);
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("close-cancel")
                                .ghost()
                                .label("Cancel close")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.send(UiAction::CancelClose);
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        Button::new("close-without-saving")
                            .ghost()
                            .label("Close without saving index")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.send(UiAction::CloseWithoutSaving);
                                cx.notify();
                            })),
                    ),
            );
        gpui_kit::base::Dialog::new(cx)
            .open(self.state.closing_index.is_some())
            .close_on_backdrop_press(false)
            .backdrop(div().bg(gpui_kit::rgba(0x00000099)))
            .popup(surface)
            .on_cancel({
                let target = cx.entity();
                move |_, _, cx| {
                    if waiting {
                        false
                    } else {
                        target.update(cx, |view, cx| {
                            view.send(UiAction::CancelClose);
                            cx.notify();
                        });
                        true
                    }
                }
            })
    }

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
                let has_repository = self.state.root.is_some();
                let dark = self.state.dark;
                Button::new("actions-panel")
                    .icon(IconName::Menu)
                    .tooltip("Application menu")
                    .accessibility_label("Application menu")
                    .dropdown_menu(move |menu, _, _| {
                        let save_target = target.clone();
                        let new_query_target = target.clone();
                        let batch_target = target.clone();
                        let check_target = target.clone();
                        let theme_target = target.clone();
                        let help_target = target.clone();
                        menu.item(
                            PopupMenuItem::new("New query tab")
                                .icon(AssetIconName::SquarePlus)
                                .disabled(!has_repository)
                                .on_click(move |_, window, cx| {
                                    new_query_target.update(cx, |this, cx| {
                                        this.open_query_tab(window, cx);
                                    });
                                }),
                        )
                        .item(
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
                        .separator()
                        .item(
                            PopupMenuItem::new("Check for changes")
                                .icon(AssetIconName::RefreshCw)
                                .on_click(move |_, _, cx| {
                                    check_target.update(cx, |this, _| {
                                        this.send(UiAction::CheckForChanges);
                                    });
                                }),
                        )
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
                .on_click(cx.listener(|this, _, window, cx| {
                    this.open_table_tab(None, window, cx);
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
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_table_tab(Some(name.clone()), window, cx);
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

        self.request_visible_grid_rows();

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
        let table_width = visible_columns
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
                    let row_key = state.projected_rows.get(&index).map(|row| row.key.clone());
                    let shade = if selected {
                        colors.selected
                    } else if index % 2 == 0 {
                        colors.panel
                    } else {
                        colors.row_alt
                    };
                    if row_key.is_none() {
                        return div()
                            .id(("result-row", index))
                            .flex()
                            .items_center()
                            .w_full()
                            .h(px(30.))
                            .bg(rgb(shade))
                            .border_b_1()
                            .border_color(rgb(colors.border))
                            .child(
                                div()
                                    .px_2()
                                    .text_sm()
                                    .text_color(rgb(colors.secondary))
                                    .child("Loading row…"),
                            )
                            .into_any_element();
                    }
                    div()
                        .id(("result-row", index))
                        .flex()
                        .items_start()
                        .w_full()
                        .min_h(px(30.))
                        .bg(rgb(shade))
                        .border_b_1()
                        .border_color(rgb(colors.border))
                        .children(row_columns.iter().map(
                            |(column_index, column, width, wrapped)| {
                                let cell = cells.get(*column_index).cloned().unwrap_or_default();
                                let content = if column == "@status" {
                                    let valid = cell.starts_with("Valid");
                                    div()
                                        .id(("status-dot", index))
                                        .aria_label(cell.clone())
                                        .text_base()
                                        .text_color(rgb(if valid {
                                            0x28a66a
                                        } else {
                                            colors.error
                                        }))
                                        .child("●")
                                        .into_any_element()
                                } else if column == "@match" {
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
                                    .when(*wrapped || column == "@match", |this| {
                                        this.whitespace_normal()
                                    })
                                    .when(!*wrapped && column != "@match", |this| {
                                        this.whitespace_nowrap().text_ellipsis()
                                    })
                                    .child(content)
                            },
                        ))
                        .on_click({
                            let target = target.clone();
                            move |event: &gpui_kit::ClickEvent, window, cx| {
                                target.update(cx, |this, cx| {
                                    window.focus(&this.grid_focus, cx);
                                    this.state.page.selected = Some(index);
                                    if this.state.show_preview {
                                        this.state.inspector_visible = true;
                                    }
                                    this.send(if event.modifiers().shift {
                                        UiAction::OpenRecordNewTab(index)
                                    } else {
                                        UiAction::OpenRecord(index)
                                    });
                                    cx.notify();
                                });
                            }
                        })
                        .context_menu({
                            let target = target.clone();
                            move |menu, _, _| {
                                let target = target.clone();
                                let record_id = row_key.as_ref().map(|key| key.record_id.clone());
                                let file = row_key.as_ref().map(|key| key.file.clone());
                                menu.item(
                                    PopupMenuItem::new("Open in new tab")
                                        .icon(AssetIconName::ExternalLink)
                                        .on_click(move |_, _, cx| {
                                            target.update(cx, |this, _| {
                                                this.send(UiAction::OpenRecordNewTab(index))
                                            });
                                        }),
                                )
                                .separator()
                                .item(
                                    PopupMenuItem::new("Copy record ID")
                                        .icon(AssetIconName::Copy)
                                        .disabled(record_id.is_none())
                                        .on_click(move |_, _, cx| {
                                            if let Some(id) = record_id.clone() {
                                                super::copy_text_to_clipboard(id, cx);
                                            }
                                        }),
                                )
                                .item(
                                    PopupMenuItem::new("Copy file path")
                                        .icon(AssetIconName::Copy)
                                        .disabled(file.is_none())
                                        .on_click(move |_, _, cx| {
                                            if let Some(file) = file.clone() {
                                                super::copy_text_to_clipboard(file, cx);
                                            }
                                        }),
                                )
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
            .relative()
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
                    .overflow_x_scroll()
                    .restrict_scroll_to_axis(),
            )
            .child(
                div()
                    .flex()
                    .h(Scrollbar::width())
                    .flex_shrink_0()
                    .child(
                        div().flex_1().child(
                            Scrollbar::horizontal(&self.grid_horizontal_scroll)
                                .mode(ScrollbarMode::Always)
                                .viewport_from_layout(),
                        ),
                    )
                    .child(div().w(Scrollbar::width()).flex_shrink_0()),
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
                    .when(page.exact_total.is_none() && !page.count_pending, |this| {
                        this.child(
                            Button::new("count-exact")
                                .ghost()
                                .label("Count all")
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
                    .when(self.state.loading_more, |this| this.child("Loading more…"))
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
                    ),
            )
            .when(self.state.delivered_count > 0, |this| {
                this.child(
                    div()
                        .absolute()
                        .top(px(32.))
                        .right_0()
                        .bottom(px(32.) + Scrollbar::width())
                        .w(Scrollbar::width())
                        .child(
                            Scrollbar::vertical(&self.grid_list)
                                .mode(ScrollbarMode::Always)
                                .viewport_from_layout(),
                        ),
                )
            })
            .into_any_element()
    }

    fn render_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = palette(self.state.dark);
        let status = if self.state.busy == Busy::None {
            self.state.status.clone()
        } else {
            format!("Working: {}", self.state.status)
        };
        let index = &self.state.index_status;
        let index_status = if matches!(index.maintenance, IndexTaskState::Running { .. }) {
            if index.maintenance_kind == Some(super::model::IndexTaskKind::Compact) {
                "Compacting index…"
            } else {
                "Rebuilding index…"
            }
        } else if matches!(index.check, IndexTaskState::Running { .. }) {
            "Checking files…"
        } else if matches!(index.save, IndexTaskState::Running { .. }) {
            "Saving index…"
        } else if matches!(index.check, IndexTaskState::Failed(_)) {
            "Some files could not be checked"
        } else if matches!(index.save, IndexTaskState::Failed(_)) {
            "Index could not be saved"
        } else if index.dirty {
            "Index changes not saved"
        } else if index.check_complete == Some(true) {
            "Files checked"
        } else {
            "Index ready"
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
            .when(self.state.result_outdated, |this| {
                this.child(
                    Button::new("results-refresh")
                        .ghost()
                        .label("Results changed — Refresh")
                        .tooltip("Refresh the last valid results with the latest index")
                        .on_click(cx.listener(|this, _, _, _| {
                            this.send(UiAction::RefreshResults);
                        })),
                )
            })
            .when(self.state.root.is_some(), |this| {
                this.child(
                    Button::new("index-status")
                        .ghost()
                        .label(index_status)
                        .tooltip("Index status and maintenance")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.show_actions = true;
                            this.state.action_panel = ActionPanel::Index;
                            cx.notify();
                        })),
                )
            })
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

impl DesktopView {
    fn render_workspace_bar(&self, pane: Pane, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = palette(self.state.dark);
        let group = self.state.workspace.group(pane);
        let active = group.active;
        let focused = self.state.workspace.focused_pane == pane;
        let tabs = group.tabs.clone();
        let menu_tabs = tabs.clone();
        let count = tabs.len();
        let target = cx.entity();
        let menu_target = target.clone();
        let split_target = target.clone();
        let tabs_scroll = match pane {
            Pane::Primary => &self.primary_tabs_scroll,
            Pane::Secondary => &self.secondary_tabs_scroll,
        };
        let show_split_target = pane == Pane::Primary
            && group.active.is_some()
            && self.state.workspace.secondary.tabs.is_empty();

        div()
            .id(format!("workspace-bar-{pane:?}"))
            .flex()
            .items_center()
            .h(px(38.))
            .flex_shrink_0()
            .border_b_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.raised))
            .child(
                div()
                    .id(format!("workspace-tabs-{pane:?}"))
                    .flex()
                    .items_center()
                    .gap_1()
                    .h_full()
                    .min_w_0()
                    .overflow_x_scroll()
                    .track_scroll(tabs_scroll)
                    .px_1()
                    .children(tabs.into_iter().enumerate().map(|(index, tab)| {
                        let id = tab.id;
                        let title = tab.title.clone();
                        let display_title = match id {
                            TabId::Record(_) => {
                                let record_id = title
                                    .rsplit(" / ")
                                    .find(|part| !part.is_empty())
                                    .unwrap_or(&title);
                                record_id
                                    .rsplit(['/', ':'])
                                    .find(|part| !part.is_empty())
                                    .unwrap_or(record_id)
                                    .to_owned()
                            }
                            TabId::Search(_) => title.clone(),
                        };
                        let selected = active == Some(id);
                        let icon = match (id, tab.search_kind) {
                            (TabId::Record(_), _) => AssetIconName::FileText,
                            (TabId::Search(_), Some(SearchKind::Table)) => AssetIconName::Table2,
                            _ => AssetIconName::Search,
                        };
                        let select_target = target.clone();
                        let close_target = target.clone();
                        let drop_target = target.clone();
                        let menu_target = target.clone();
                        let drag_title = title.clone();
                        let drag_dark = self.state.dark;
                        let duplicate_label = match tab.search_kind {
                            Some(SearchKind::Table) => Some("Duplicate as query"),
                            Some(SearchKind::Query) => Some("Duplicate query"),
                            None => None,
                        };
                        div()
                            .id(format!("workspace-tab-{pane:?}-{index}"))
                            .relative()
                            .flex()
                            .items_center()
                            .gap_1()
                            .h(px(33.))
                            .min_w(px(if pane == Pane::Secondary { 130. } else { 142. }))
                            .max_w(px(220.))
                            .px_2()
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(if selected {
                                colors.border
                            } else {
                                colors.raised
                            }))
                            .bg(rgb(if selected {
                                colors.panel
                            } else {
                                colors.raised
                            }))
                            .when(!selected, |this| {
                                this.hover(|this| this.bg(rgb(colors.selected)))
                            })
                            .when(selected, |this| {
                                this.child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .left_1()
                                        .right_1()
                                        .h(px(2.))
                                        .bg(rgb(if focused {
                                            colors.accent
                                        } else {
                                            colors.secondary
                                        })),
                                )
                            })
                            .child(
                                div()
                                    .id(format!("workspace-select-{pane:?}-{index}"))
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .h_full()
                                    .min_w_0()
                                    .flex_1()
                                    .cursor_pointer()
                                    .aria_label(format!(
                                        "{} tab: {}",
                                        if selected && focused {
                                            "Focused"
                                        } else if selected {
                                            "Active"
                                        } else {
                                            "Open"
                                        },
                                        title
                                    ))
                                    .child(Icon::new(icon).size_4().text_color(rgb(if selected && focused {
                                        colors.accent
                                    } else {
                                        colors.secondary
                                    })))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .text_sm()
                                            .text_color(rgb(if selected {
                                                colors.text
                                            } else {
                                                colors.secondary
                                            }))
                                            .child(display_title),
                                    )
                                    .on_click(move |_, window, cx| {
                                        select_target.update(cx, |this, cx| {
                                            this.activate_workspace_tab(id, window, cx);
                                        });
                                    })
                                    .on_mouse_down(
                                        gpui_kit::MouseButton::Middle,
                                        cx.listener(move |this, _, window, cx| {
                                            this.close_workspace_tab(id, window, cx);
                                            cx.stop_propagation();
                                        }),
                                    )
                                    .on_drag(WorkspaceDrag { id }, move |_, _, _, cx| {
                                        cx.new(|_| WorkspaceDragPreview {
                                            title: drag_title.clone(),
                                            dark: drag_dark,
                                        })
                                    })
                                    .context_menu(move |menu, _, _| {
                                        workspace_tab_menu(
                                            menu,
                                            menu_target.clone(),
                                            id,
                                            pane,
                                            duplicate_label,
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .id(format!("workspace-close-{pane:?}-{index}"))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(20.))
                                    .flex_shrink_0()
                                    .rounded_sm()
                                    .cursor_pointer()
                                    .text_color(rgb(colors.secondary))
                                    .hover(|this| {
                                        this.bg(rgb(colors.selected)).text_color(rgb(colors.text))
                                    })
                                    .aria_label(format!("Close {} tab", title))
                                    .child(Icon::new(AssetIconName::X).size_3())
                                    .on_click(move |_, window, cx| {
                                        close_target.update(cx, |this, cx| {
                                            this.close_workspace_tab(id, window, cx);
                                        });
                                    }),
                            )
                            .drag_over::<WorkspaceDrag>(move |this, _, _, _| {
                                this.border_l_2().border_color(rgb(colors.accent))
                            })
                            .on_drop(move |drag: &WorkspaceDrag, window, cx| {
                                drop_target.update(cx, |this, cx| {
                                    this.move_workspace_tab(drag.id, pane, index, window, cx);
                                });
                            })
                    })),
            )
            .child(
                Button::new(format!("new-query-tab-{pane:?}"))
                    .ghost()
                    .icon(AssetIconName::Plus)
                    .tooltip("New query tab in this pane")
                    .accessibility_label("New query tab in this pane")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_query_tab_in(pane, window, cx);
                    })),
            )
            .child(
                div()
                    .id(format!("workspace-drop-end-{pane:?}"))
                    .flex_1()
                    .min_w(px(20.))
                    .h_full()
                    .drag_over::<WorkspaceDrag>(move |this, _, _, _| this.bg(rgb(colors.selected)))
                    .on_drop(move |drag: &WorkspaceDrag, window, cx| {
                        target.update(cx, |this, cx| {
                            this.move_workspace_tab(drag.id, pane, count, window, cx);
                        });
                    }),
            )
            .when(count > 1, |this| {
                this.child(
                    Button::new(format!("workspace-open-tabs-{pane:?}"))
                        .ghost()
                        .icon(AssetIconName::List)
                        .tooltip("Open tabs in this pane")
                        .accessibility_label("Open tabs in this pane")
                        .dropdown_menu(move |menu, _, _| {
                            menu_tabs.iter().fold(menu.scrollable(true), |menu, tab| {
                                let id = tab.id;
                                let select_target = menu_target.clone();
                                let icon = match (id, tab.search_kind) {
                                    (TabId::Record(_), _) => AssetIconName::FileText,
                                    (TabId::Search(_), Some(SearchKind::Table)) => {
                                        AssetIconName::Table2
                                    }
                                    _ => AssetIconName::Search,
                                };
                                menu.item(
                                    PopupMenuItem::new(tab.title.clone())
                                        .icon(icon)
                                        .checked(active == Some(id))
                                        .on_click(move |_, window, cx| {
                                            select_target.update(cx, |this, cx| {
                                                this.activate_workspace_tab(id, window, cx);
                                            });
                                        }),
                                )
                            })
                        }),
                )
            })
            .when(show_split_target, |this| {
                this.child(
                    div()
                        .id("workspace-split-drop")
                        .flex()
                        .items_center()
                        .h_full()
                        .drag_over::<WorkspaceDrag>(move |this, _, _, _| {
                            this.bg(rgb(colors.selected))
                        })
                        .on_drop(move |drag: &WorkspaceDrag, window, cx| {
                            split_target.update(cx, |this, cx| {
                                this.move_workspace_tab(drag.id, Pane::Secondary, 0, window, cx);
                            });
                        })
                        .child(
                            Button::new("workspace-split-active")
                                .ghost()
                                .icon(AssetIconName::PanelRight)
                                .tooltip("Move active tab to a right pane")
                                .accessibility_label("Move active tab to right pane")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    if let Some(id) = this.state.workspace.primary.active {
                                        this.move_workspace_tab(id, Pane::Secondary, 0, window, cx);
                                    }
                                })),
                        ),
                )
            })
    }

    fn render_inactive_query(&self, id: TabId, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = palette(self.state.dark);
        let activate_target = cx.entity();
        let tab = self.state.workspace.tab(id);
        let title = tab.map_or("Query".to_string(), |tab| tab.title.clone());
        let draft = tab.and_then(|tab| tab.draft.as_ref());
        let scope = draft
            .and_then(|draft| draft.collection.as_deref())
            .unwrap_or("All collections")
            .to_string();
        let expression = draft
            .map(|draft| {
                if !draft.full_text.is_empty() {
                    draft.full_text.clone()
                } else if draft.filter_enabled {
                    draft.structured.clone()
                } else {
                    "All records".into()
                }
            })
            .unwrap_or_default();
        let snapshot = match id {
            TabId::Search(id) => self.state.query_snapshots.get(&id),
            TabId::Record(_) => None,
        };
        let mut columns = Vec::new();
        if let Some(snapshot) = snapshot {
            for preferred in ["@id", "name", "@match", "description"] {
                if let Some(index) = snapshot
                    .columns
                    .iter()
                    .position(|column| column == preferred)
                {
                    columns.push(index);
                }
                if columns.len() == 3 {
                    break;
                }
            }
            for (index, column) in snapshot.columns.iter().enumerate() {
                if columns.len() == 3 {
                    break;
                }
                if column != "@status" && !columns.contains(&index) {
                    columns.push(index);
                }
            }
        }
        let headers = snapshot
            .map(|snapshot| {
                columns
                    .iter()
                    .map(|index| sort::column_label(&snapshot.columns[*index]).to_owned())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let rows = snapshot
            .map(|snapshot| {
                snapshot
                    .rows
                    .iter()
                    .map(|row| {
                        columns
                            .iter()
                            .map(|index| row.cells.get(*index).cloned().unwrap_or_default())
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let count = snapshot.and_then(|snapshot| snapshot.exact_total);
        let has_snapshot = snapshot.is_some();
        let empty_snapshot = has_snapshot && rows.is_empty();
        let empty_title = if count == Some(0) { "No matches" } else { "No preview rows" };
        let empty_message = if count == Some(0) {
            "This query returned no records."
        } else {
            "Activate this query to refresh its results."
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
                    .flex()
                    .items_center()
                    .gap_3()
                    .h(px(56.))
                    .px_3()
                    .border_b_1()
                    .border_color(rgb(colors.border))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_xs()
                                    .text_color(rgb(colors.secondary))
                                    .child(format!("{scope} · {expression}")),
                            ),
                    )
                    .child(
                        Button::new(format!("activate-query-{id:?}"))
                            .ghost()
                            .label("Activate query")
                            .tooltip("Restore the editor and refresh results")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.activate_workspace_tab(id, window, cx);
                            })),
                    ),
            )
            .when(has_snapshot, |this| this.child(
                div()
                    .flex()
                    .items_center()
                    .h(px(30.))
                    .px_3()
                    .text_xs()
                    .text_color(rgb(colors.secondary))
                    .child("Result preview"),
            ))
            .when(!has_snapshot, |this| this.child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .px_4()
                    .text_center()
                    .text_color(rgb(colors.secondary))
                    .child(Icon::new(AssetIconName::Search).size_8())
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_color(rgb(colors.text)).child("No current preview"))
                    .child(div().text_sm().child("Activate this query to edit or refresh its results.")),
            ))
            .when(!headers.is_empty(), |this| {
                this.child(
                    div()
                        .flex()
                        .h(px(32.))
                        .items_center()
                        .bg(rgb(colors.raised))
                        .border_b_1()
                        .border_color(rgb(colors.border))
                        .children(headers.into_iter().map(|header| {
                            div()
                                .flex_1()
                                .min_w_0()
                                .px_3()
                                .text_xs()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_color(rgb(colors.secondary))
                                .child(header)
                        })),
                )
            })
            .when(empty_snapshot, |this| this.child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .px_4()
                    .text_center()
                    .text_color(rgb(colors.secondary))
                    .child(Icon::new(AssetIconName::Search).size_8())
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_color(rgb(colors.text)).child(empty_title))
                    .child(div().text_sm().child(empty_message)),
            ))
            .when(has_snapshot && !empty_snapshot, |this| this.child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .children(rows.into_iter().enumerate().map(|(index, cells)| {
                        let target = activate_target.clone();
                        div()
                            .id(format!("inactive-query-row-{id:?}-{index}"))
                            .flex()
                            .items_center()
                            .h(px(36.))
                            .flex_shrink_0()
                            .cursor_pointer()
                            .hover(|this| this.bg(rgb(colors.selected)))
                            .border_b_1()
                            .border_color(rgb(colors.border))
                            .on_click(move |_, window, cx| {
                                target.update(cx, |this, cx| {
                                    this.activate_workspace_tab(id, window, cx);
                                });
                            })
                            .children(cells.into_iter().map(|cell| {
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .px_3()
                                    .text_sm()
                                    .child(cell)
                            }))
                    })),
            ))
            .when_some(count, |this, count| {
                this.child(
                    div()
                        .h(px(32.))
                        .px_3()
                        .border_t_1()
                        .border_color(rgb(colors.border))
                        .text_xs()
                        .text_color(rgb(colors.secondary))
                        .child(format!("{count} matches")),
                )
            })
    }

    fn render_record_snapshot(&self, id: u64, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = palette(self.state.dark);
        let activate_target = cx.entity();
        let preview = self.state.record_detail_snapshots.get(&id);
        let title = preview.map_or_else(
            || {
                self.state
                    .workspace
                    .tab(TabId::Record(id))
                    .map_or_else(|| "Record preview".to_string(), |tab| tab.title.clone())
            },
            |preview| preview.title.clone(),
        );
        let fields = preview.map_or_else(Vec::new, |preview| preview.fields.clone());
        let clipped = preview.is_some_and(|preview| preview.clipped);
        let missing = preview.is_none();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .bg(rgb(colors.panel))
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(52.))
                    .px_3()
                    .border_b_1()
                    .border_color(rgb(colors.border))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(title.replace(" / path:", " / ")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(colors.secondary))
                                    .child("Record preview"),
                            ),
                    )
                    .child(
                        Button::new(format!("activate-record-{id}"))
                            .ghost()
                            .label("Activate")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.activate_workspace_tab(TabId::Record(id), window, cx);
                            })),
                    ),
            )
            .when(missing, |this| {
                this.child(
                    div()
                        .px_3()
                        .py_3()
                        .text_color(rgb(colors.secondary))
                        .child("Activate to load this record."),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .children(fields.into_iter().enumerate().map(|(index, (label, value))| {
                        let target = activate_target.clone();
                        let label = if label == "$" {
                            "Record".to_string()
                        } else {
                            label.strip_prefix("$.").unwrap_or(&label).to_string()
                        };
                        div()
                            .id(format!("inactive-record-row-{id}-{index}"))
                            .flex()
                            .gap_3()
                            .px_3()
                            .py_2()
                            .cursor_pointer()
                            .hover(|this| this.bg(rgb(colors.selected)))
                            .border_b_1()
                            .border_color(rgb(colors.border))
                            .on_click(move |_, window, cx| {
                                target.update(cx, |this, cx| {
                                    this.activate_workspace_tab(TabId::Record(id), window, cx);
                                });
                            })
                            .child(
                                div()
                                    .w(px(130.))
                                    .flex_shrink_0()
                                    .text_color(rgb(colors.secondary))
                                    .child(label),
                            )
                            .child(div().flex_1().min_w_0().whitespace_normal().child(value))
                    })),
            )
            .when(clipped, |this| {
                this.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_xs()
                        .text_color(rgb(colors.secondary))
                        .child("Preview is shortened. Activate for full details."),
                )
            })
    }

    fn render_workspace_content(&self, pane: Pane, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        match self.state.workspace.group(pane).active {
            Some(id @ TabId::Search(_)) if self.state.workspace.current_search == Some(id) => {
                let show_preview = self.state.preview_visible_for(id);
                let pane_width = self.workspace_layout.get().map_or(780., |layout| {
                    if pane == Pane::Secondary {
                        layout.1
                    } else {
                        let sidebar = if layout.2 { 220. } else { 0. };
                        let secondary = if layout.3 { layout.1 } else { 0. };
                        layout.0 - sidebar - secondary
                    }
                });
                let preview_width = self.state.inspector_width.min((pane_width - 280.).max(260.));
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(self.render_query_bar(cx))
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_h_0()
                            .min_w_0()
                            .child(self.render_grid(cx))
                            .when(show_preview, |this| {
                                this.child(
                                    div()
                                        .w(px(preview_width))
                                        .flex_shrink_0()
                                        .min_h_0()
                                        .child(self.render_detail(Pane::Secondary, cx)),
                                )
                            }),
                    )
                    .into_any_element()
            }
            Some(id @ TabId::Search(_)) => self.render_inactive_query(id, cx).into_any_element(),
            Some(TabId::Record(id)) if self.state.active_tab == Some(id) => {
                self.render_detail(pane, cx).into_any_element()
            }
            Some(TabId::Record(id)) => self.render_record_snapshot(id, cx).into_any_element(),
            None => {
                let colors = palette(self.state.dark);
                let other_has_tabs = !self.state.workspace.group(pane.other()).tabs.is_empty();
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .text_color(rgb(colors.secondary))
                    .child(Icon::new(AssetIconName::PanelsTopLeft).size_8())
                    .child(
                        div()
                            .text_color(rgb(colors.text))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if other_has_tabs {
                                "No tabs in this pane"
                            } else {
                                "No tabs open"
                            }),
                    )
                    .child(if other_has_tabs {
                        "Drag a tab here or create a query."
                    } else {
                        "Open a collection or create a query."
                    })
                    .child(
                        Button::new(format!("workspace-empty-new-query-{pane:?}"))
                            .icon(AssetIconName::Plus)
                            .label("New query")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_query_tab_in(pane, window, cx);
                            })),
                    )
                    .into_any_element()
            }
        }
    }

    fn render_workspace_group(&self, pane: Pane, cx: &mut Context<Self>) -> impl IntoElement {
        let target = cx.entity();
        let end = self.state.workspace.group(pane).tabs.len();
        let colors = palette(self.state.dark);
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(self.render_workspace_bar(pane, cx))
            .child(
                div()
                    .id(format!("workspace-content-drop-{pane:?}"))
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .on_mouse_down(
                        gpui_kit::MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            if this.state.workspace.focused_pane != pane {
                                this.state.workspace.focused_pane = pane;
                                cx.notify();
                            }
                        }),
                    )
                    .drag_over::<WorkspaceDrag>(move |this, _, _, _| {
                        this.border_2().border_color(rgb(colors.accent))
                    })
                    .on_drop(move |drag: &WorkspaceDrag, window, cx| {
                        target.update(cx, |this, cx| {
                            this.move_workspace_tab(drag.id, pane, end, window, cx);
                        });
                    })
                    .child(self.render_workspace_content(pane, cx)),
            )
    }

    fn render_workspace(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let right = self.state.workspace.group(Pane::Secondary);
        let primary_empty = self.state.workspace.primary.tabs.is_empty() && !right.tabs.is_empty();
        let right_width = if matches!(right.active, Some(TabId::Record(_))) {
            self.state.inspector_width
        } else {
            self.state.secondary_query_width
        };
        let window_width = f32::from(window.bounds().size.width);
        let primary_reserve = if primary_empty {
            260.
        } else if window_width < 1100. {
            220.
        } else {
            320.
        };
        let available = window_width
            - if self.state.sidebar_visible { 220. } else { 0. }
            - primary_reserve;
        let right_width = if primary_empty {
            available.max(300.)
        } else {
            right_width.min(available.max(300.))
        };
        let layout = (
            f32::from(window.bounds().size.width),
            right_width,
            self.state.sidebar_visible,
            !right.tabs.is_empty(),
        );
        if self.workspace_layout.get() != Some(layout) {
            self.workspace_layout.set(Some(layout));
            for pane in [Pane::Primary, Pane::Secondary] {
                if let Some(id) = self.state.workspace.group(pane).active {
                    self.reveal_workspace_tab(id);
                }
            }
        }
        let right_is_query = matches!(right.active, Some(TabId::Search(_)));
        div()
            .flex()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .child(self.render_workspace_group(Pane::Primary, cx))
            .when(!right.tabs.is_empty(), |this| {
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .relative()
                        .min_h_0()
                        .w(px(right_width))
                        .flex_shrink_0()
                        .child(self.render_workspace_group(Pane::Secondary, cx))
                        .when(right_is_query && !primary_empty, |this| {
                            this.child(
                                div()
                                    .id("secondary-query-resize")
                                    .absolute()
                                    .left_0()
                                    .top_0()
                                    .h_full()
                                    .w(px(5.))
                                    .cursor_col_resize()
                                    .on_mouse_down(
                                        gpui_kit::MouseButton::Left,
                                        cx.listener(
                                            |this, event: &gpui_kit::MouseDownEvent, _, cx| {
                                                this.state.secondary_resize_origin = Some((
                                                    f32::from(event.position.x),
                                                    this.state.secondary_query_width,
                                                ));
                                                cx.stop_propagation();
                                            },
                                        ),
                                    ),
                            )
                        }),
                )
            })
    }
}

impl Render for DesktopView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.request_visible_grid_rows();
        let colors = palette(self.state.dark);
        div()
            .id("desktop-root")
            .relative()
            .on_mouse_move(
                cx.listener(|this, event: &gpui_kit::MouseMoveEvent, _, cx| {
                    if let Some((start_x, start_width)) = this.state.inspector_resize_origin {
                        let distance = start_x - f32::from(event.position.x);
                        this.state.inspector_width = (start_width + distance).clamp(300., 1000.);
                        cx.notify();
                    }
                    if let Some((start_x, start_width)) = this.state.secondary_resize_origin {
                        let distance = start_x - f32::from(event.position.x);
                        this.state.secondary_query_width =
                            (start_width + distance).clamp(420., 1000.);
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
                    this.state.secondary_resize_origin = None;
                    if let Some((column, _, _)) = this.state.column_resize_origin.take() {
                        if let Some(width) = this.state.column_widths.get(&column).copied() {
                            if column == "@match" || this.state.wrapped_columns.contains(&column) {
                                this.refresh_grid_height_hints();
                            }
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
            .key_context("ObjekteWorkspace")
            .track_focus(&self.workspace_focus)
            .on_action(cx.listener(|this, _: &NewQueryTab, window, cx| {
                this.open_query_tab(window, cx);
            }))
            .on_action(cx.listener(|this, _: &CloseWorkspaceTab, window, cx| {
                if let Some(id) = this.state.workspace.focused_active_tab() {
                    this.close_workspace_tab(id, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &NextWorkspaceTab, window, cx| {
                if let Some(id) = this.state.workspace.adjacent_tab(false) {
                    this.activate_workspace_tab(id, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &PreviousWorkspaceTab, window, cx| {
                if let Some(id) = this.state.workspace.adjacent_tab(true) {
                    this.activate_workspace_tab(id, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &FocusPrimaryPane, window, cx| {
                if let Some(id) = this.state.workspace.primary.active {
                    this.activate_workspace_tab(id, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &FocusSecondaryPane, window, cx| {
                if let Some(id) = this.state.workspace.secondary.active {
                    this.activate_workspace_tab(id, window, cx);
                }
            }))
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
                    .when(self.state.root.is_some(), |this| {
                        this.child(self.render_workspace(window, cx))
                    })
                    .when(self.state.root.is_none(), |this| {
                        this.child(self.render_grid(cx))
                    }),
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
            .when(self.state.closing_index.is_some(), |this| {
                this.child(self.render_close_dialog(cx))
            })
    }
}
