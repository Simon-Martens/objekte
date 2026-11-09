//! GPUI controls for the optional synchronized filter builder.

use gpui_kit::assets::IconName as AssetIconName;
use gpui_kit::component::{
    Disableable as _, Selectable as _,
    button::{Button, ButtonVariants as _},
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    scroll::ScrollableElement as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Styled as _, Subscription, Window, div, px, rgb,
};

use super::DesktopView;
use super::filter_builder::{BuilderState, BuilderStatus, Comparison, FieldChoice, ValueType};
use super::model::{ErrorField, FilterValidationState, UiAction};
use super::state::Busy;


/// Complete builder changes are applied to the one free-text InputState.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BuilderEvent {
    Complete(String),
    CompleteValues { field: String, prefix: String },
    Close,
}


/// GPUI text controls for one builder condition.
struct RowInputs {
    field: Entity<InputState>,
    value: Entity<InputState>,
}


/// Nonmodal helper; the surrounding search field owns the actual filter text.
pub struct FilterBuilderView {
    dark: bool,
    state: BuilderState,
    source_text: String,
    fields: Vec<FieldChoice>,
    rows: Vec<RowInputs>,
    subscriptions: Vec<Subscription>,
    selected_row: usize,
    value_focused: bool,
    value_choices: Vec<(String, String)>,
    value_choices_partial: bool,
    local_error: Option<String>,
    needs_rebuild: bool,
}


impl EventEmitter<BuilderEvent> for FilterBuilderView {}


impl FilterBuilderView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            dark: true,
            state: BuilderState::from_text(""),
            source_text: String::new(),
            fields: Vec::new(),
            rows: Vec::new(),
            subscriptions: Vec::new(),
            selected_row: 0,
            value_focused: false,
            value_choices: Vec::new(),
            value_choices_partial: false,
            local_error: None,
            needs_rebuild: false,
        };
        this.rebuild_rows(window, cx);
        this
    }

    /// Text edits, restoration, undo, and redo rederive the helper. A text
    /// value emitted by this builder is already reflected in focused controls.
    pub fn sync_from_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if text == self.source_text {
            return;
        }
        self.source_text = text.to_owned();
        self.state = BuilderState::from_text(text);
        self.local_error = None;
        self.selected_row = 0;
        self.value_focused = false;
        self.value_choices.clear();
        self.needs_rebuild = true;
        cx.notify();
    }

    pub fn set_fields(&mut self, fields: Vec<FieldChoice>, cx: &mut Context<Self>) {
        self.fields = fields;
        cx.notify();
    }

    pub fn set_dark(&mut self, dark: bool, cx: &mut Context<Self>) {
        self.dark = dark;
        cx.notify();
    }

    pub fn set_value_choices(
        &mut self,
        field: &str,
        prefix: &str,
        values: Vec<(String, String)>,
        partial: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.value_focused
            || !self.state.rows.get(self.selected_row).is_some_and(|row| {
                row.field == field
                    && self
                        .rows
                        .get(self.selected_row)
                        .is_some_and(|inputs| inputs.value.read(cx).value().as_ref() == prefix)
            })
        {
            return;
        }
        self.value_choices = values;
        self.value_choices_partial = partial;
        cx.notify();
    }

    fn rebuild_rows(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rows.clear();
        self.subscriptions.clear();
        for index in 0..self.state.rows.len() {
            self.append_row(index, window, cx);
        }
    }

    fn append_row(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let draft = &self.state.rows[index];
        let field_text = draft.field.clone();
        let value_text = draft.value.clone().unwrap_or_default();
        let field = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Field path")
                .default_value(field_text)
        });
        let value = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Value")
                .default_value(value_text)
        });
        self.subscriptions.push(cx.subscribe(
            &field,
            move |this: &mut Self, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.field_changed(index, cx);
                }
                if matches!(event, InputEvent::Focus) {
                    this.selected_row = index;
                    this.value_focused = false;
                    this.value_choices.clear();
                    cx.notify();
                }
            },
        ));
        self.subscriptions.push(cx.subscribe(
            &value,
            move |this: &mut Self, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.value_changed(index, cx);
                }
                if matches!(event, InputEvent::Focus) {
                    this.selected_row = index;
                    this.value_focused = true;
                    this.request_values(index, cx);
                    cx.notify();
                }
            },
        ));
        self.rows.push(RowInputs { field, value });
    }

    fn field_changed(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(inputs) = self.rows.get(index) else {
            return;
        };
        let Some(draft) = self.state.rows.get_mut(index) else {
            return;
        };
        draft.field = inputs.field.read(cx).value().to_string();
        self.publish(cx);
    }

    fn value_changed(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(inputs) = self.rows.get(index) else {
            return;
        };
        let Some(draft) = self.state.rows.get_mut(index) else {
            return;
        };
        draft.value = Some(inputs.value.read(cx).value().to_string());
        self.publish(cx);
        self.value_focused = true;
        self.request_values(index, cx);
    }

    fn request_values(&mut self, index: usize, cx: &mut Context<Self>) {
        self.value_choices.clear();
        let Some(row) = self.state.rows.get(index) else {
            return;
        };
        if row.field.is_empty()
            || !matches!(row.value_type, Some(ValueType::String | ValueType::Number))
        {
            return;
        }
        let Some(inputs) = self.rows.get(index) else {
            return;
        };
        let prefix = inputs.value.read(cx).value().to_string();
        cx.emit(BuilderEvent::CompleteValues {
            field: row.field.clone(),
            prefix,
        });
    }

    fn publish(&mut self, cx: &mut Context<Self>) {
        match self.state.complete_change() {
            Ok(Some(text)) => {
                self.local_error = None;
                if text != self.source_text {
                    self.source_text = text.clone();
                    cx.emit(BuilderEvent::Complete(text));
                }
            }
            Ok(None) => self.local_error = None,
            Err(error) => self.local_error = Some(error),
        }
        cx.notify();
    }

    fn choose_field(&mut self, choice: &FieldChoice, window: &mut Window, cx: &mut Context<Self>) {
        self.value_focused = false;
        self.value_choices.clear();
        let index = self.selected_row.min(self.rows.len().saturating_sub(1));
        let Some(row) = self.state.rows.get_mut(index) else {
            return;
        };
        row.field = choice.path.clone();
        if row.value_type != choice.value_type {
            row.value = None;
            self.rows[index]
                .value
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
        row.value_type = choice.value_type;
        self.rows[index].field.update(cx, |input, cx| {
            input.set_value(choice.path.clone(), window, cx)
        });
        self.publish(cx);
    }

    fn choose_value(&mut self, literal: &str, window: &mut Window, cx: &mut Context<Self>) {
        let index = self.selected_row;
        let Some(row) = self.state.rows.get_mut(index) else {
            return;
        };
        let value = if row.value_type == Some(ValueType::String) {
            serde_json::from_str::<String>(literal).unwrap_or_else(|_| literal.to_owned())
        } else {
            literal.to_owned()
        };
        row.value = Some(value.clone());
        self.value_choices.clear();
        let Some(inputs) = self.rows.get(index) else {
            return;
        };
        inputs
            .value
            .update(cx, |input, cx| input.set_value(value, window, cx));
        self.value_focused = false;
        self.publish(cx);
    }

    fn change_comparison(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(row) = self.state.rows.get_mut(index) else {
            return;
        };
        let choices: &[Comparison] =
            if row.value_type == Some(ValueType::Number) || row.value_type.is_none() {
                &[
                    Comparison::Equal,
                    Comparison::NotEqual,
                    Comparison::Greater,
                    Comparison::AtLeast,
                    Comparison::Less,
                    Comparison::AtMost,
                ]
            } else {
                &[Comparison::Equal, Comparison::NotEqual]
            };
        let current = choices
            .iter()
            .position(|item| *item == row.comparison)
            .unwrap_or(0);
        row.comparison = choices[(current + 1) % choices.len()];
        self.publish(cx);
    }

    fn change_type(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.state.rows.get_mut(index) else {
            return;
        };
        row.value_type = Some(match row.value_type {
            None => ValueType::String,
            Some(ValueType::String) => ValueType::Number,
            Some(ValueType::Number) => ValueType::Boolean,
            Some(ValueType::Boolean) => ValueType::Null,
            Some(ValueType::Null) => ValueType::String,
        });
        row.value = None;
        self.rows[index]
            .value
            .update(cx, |input, cx| input.set_value("", window, cx));
        if row.value_type != Some(ValueType::Number)
            && !matches!(row.comparison, Comparison::Equal | Comparison::NotEqual)
        {
            row.comparison = Comparison::Equal;
        }
        self.publish(cx);
    }

    fn set_boolean(&mut self, index: usize, value: bool, cx: &mut Context<Self>) {
        let Some(row) = self.state.rows.get_mut(index) else {
            return;
        };
        row.value = Some(value.to_string());
        self.publish(cx);
    }

    fn remove_row(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.state.rows.len() {
            return;
        }
        self.state.rows.remove(index);
        self.selected_row = self
            .selected_row
            .min(self.state.rows.len().saturating_sub(1));
        self.rebuild_rows(window, cx);
        self.publish(cx);
    }

    fn add_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.rows.len() >= 16 {
            return;
        }
        self.state.add_condition();
        let index = self.state.rows.len() - 1;
        self.selected_row = index;
        self.append_row(index, window, cx);
        cx.notify();
    }
}


impl Render for FilterBuilderView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.needs_rebuild {
            self.rebuild_rows(window, cx);
            self.needs_rebuild = false;
        }
        let status = self.state.status.clone();
        let selected_field = self
            .rows
            .get(self.selected_row)
            .map(|row| row.field.read(cx).value().to_string())
            .unwrap_or_default();
        let suggestions: Vec<_> = self
            .fields
            .iter()
            .filter(|choice| {
                selected_field.is_empty()
                    || choice
                        .path
                        .to_lowercase()
                        .contains(&selected_field.to_lowercase())
            })
            .take(8)
            .cloned()
            .collect();
        let value_suggestions = if self.value_focused {
            self.value_choices
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let editable = matches!(
            status,
            BuilderStatus::Empty | BuilderStatus::Synchronized | BuilderStatus::Incomplete
        );
        div().id("filter-builder")
            .on_mouse_down(gpui_kit::MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
            .w(px(560.)).max_h(px(320.)).p_3().gap_2().flex().flex_col()
            .bg(rgb(if self.dark { 0x1c1f24 } else { 0xffffff }))
            .text_color(rgb(if self.dark { 0xe5e7eb } else { 0x172033 }))
            .border_1().border_color(rgb(if self.dark { 0x30353c } else { 0xcbd5e1 })).rounded_md()
            .child(div().flex().items_center().gap_2()
                .child(div().flex_1().child("Filter records"))
                .child(Button::new("filter-builder-close").label("Close").on_click(cx.listener(|_, _, _, cx| cx.emit(BuilderEvent::Close)))))
            .child(div().text_sm().child("Match all conditions"))
            .when(editable, |this| this.children(self.rows.iter().enumerate().map(|(index, inputs)| {
                let draft = &self.state.rows[index];
                let kind = match draft.value_type { None => "Choose type", Some(ValueType::String) => "String", Some(ValueType::Number) => "Number", Some(ValueType::Boolean) => "Boolean", Some(ValueType::Null) => "Null" };
                div().flex().items_center().gap_1()
                    .child(Input::new(&inputs.field).id(("builder-field", index)).w(px(155.)).aria_label("Field path"))
                    .child(Button::new(("builder-operator", index)).label(draft.comparison.symbol()).on_click(cx.listener(move |this, _, _, cx| this.change_comparison(index, cx))))
                    .child(Button::new(("builder-type", index)).label(kind).on_click(cx.listener(move |this, _, window, cx| this.change_type(index, window, cx))))
                    .when(draft.value_type == Some(ValueType::Boolean), |this| this
                        .child(Button::new(("builder-true", index)).label("True").on_click(cx.listener(move |this, _, _, cx| this.set_boolean(index, true, cx))))
                        .child(Button::new(("builder-false", index)).label("False").on_click(cx.listener(move |this, _, _, cx| this.set_boolean(index, false, cx)))))
                    .when(!matches!(draft.value_type, Some(ValueType::Boolean | ValueType::Null)), |this| this.child(Input::new(&inputs.value).id(("builder-value", index)).w(px(135.)).aria_label("Value")))
                    .child(Button::new(("builder-remove", index)).label("×").on_click(cx.listener(move |this, _, window, cx| this.remove_row(index, window, cx))))
            })))
            .when(editable && !suggestions.is_empty(), |this| this.child(div().flex().gap_1().children(suggestions.into_iter().enumerate().map(|(index, choice)| {
                Button::new(("builder-suggestion", index)).label(choice.path.clone())
                    .on_click(cx.listener(move |this, _, window, cx| this.choose_field(&choice, window, cx)))
            }))))
            .when(editable && !value_suggestions.is_empty(), |this| this.child(
                div().flex().flex_wrap().gap_1().children(value_suggestions.into_iter().enumerate().map(|(index, (literal, label))| {
                    Button::new(("builder-value-suggestion", index)).label(label)
                        .on_click(cx.listener(move |this, _, window, cx| this.choose_value(&literal, window, cx)))
                }))
            ))
            .when(editable && self.value_focused && self.value_choices_partial, |this| this.child(div().text_sm().child("More indexed values may exist")))
            .when(editable, |this| this.child(Button::new("builder-add").label("+ Add condition").disabled(self.state.rows.len() >= 16).on_click(cx.listener(|this, _, window, cx| this.add_row(window, cx)))))
            .child(div().text_sm().child(match status {
                BuilderStatus::Empty => "Add a condition; the filter text stays empty until it is complete.".into(),
                BuilderStatus::Synchronized => "✓ Synchronized with the filter text".into(),
                BuilderStatus::Incomplete => "Complete each row to update the filter text.".into(),
                BuilderStatus::Unsupported => "This expression cannot be represented by the builder. Edit the filter text above.".into(),
                BuilderStatus::Invalid(error) => format!("{error}. Edit the filter text above."),
            }))
            .when(self.local_error.is_some(), |this| this.child(div().text_sm().child(self.local_error.clone().unwrap_or_default())))
            .overflow_y_scrollbar()
    }
}


impl DesktopView {
    fn copy_query_text(&self, filter: bool, cx: &mut Context<Self>) {
        let text = if filter {
            self.inputs.structured.read(cx).value().to_string()
        } else {
            self.inputs.full_text.read(cx).value().to_string()
        };
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            std::thread::spawn(move || super::write_wayland_clipboard(text));
        } else {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
        }
    }

    fn paste_query_text(
        &mut self,
        filter: bool,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Updating the input may emit Change. Run one explicit edit path after
        // both the field value and the filter-enabled flag are in place.
        self.restoring_query = true;
        if filter {
            self.state.search.filter_enabled = true;
            self.inputs.structured.update(cx, |input, cx| {
                input.set_value(text, window, cx);
            });
        } else {
            self.inputs.full_text.update(cx, |input, cx| {
                input.set_value(text, window, cx);
            });
        }
        self.restoring_query = false;
        self.search_from_inputs(false, cx);
    }

    /// Compact search row and optional filter line.
    /// The two InputState values remain authoritative across every render.
    pub(super) fn render_query_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let text_error = self.state.errors.get(&ErrorField::FullText).cloned();
        let filter_error = self.state.errors.get(&ErrorField::Structured).cloned();
        let validation = self.state.filter_validation.clone();
        let invalid = !matches!(validation, Some(FilterValidationState::Valid));
        let filter_invalid = matches!(validation, Some(FilterValidationState::Invalid { .. }));
        let filter_explanation = filter_error.or_else(|| match &validation {
            Some(FilterValidationState::Invalid {
                message,
                byte_offset,
            }) => Some(byte_offset.map_or_else(
                || message.clone(),
                |offset| format!("{message} (byte {offset})"),
            )),
            _ => None,
        });
        let filter_enabled = self.state.search.filter_enabled;
        let querying = self.state.busy == Busy::Querying;
        let dark = self.state.dark;
        let search_text_available = !self.inputs.full_text.read(cx).value().is_empty();
        let filter_text_available = !self.inputs.structured.read(cx).value().is_empty();
        let menu_target = cx.entity();
        div()
            .id("query-bar")
            .relative()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap_2()
            .p_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().text_sm().child("Search"))
                    .child(
                        Input::new(&self.inputs.full_text)
                            .id("full-text-input")
                            .w_full()
                            .aria_label("Full text search"),
                    )
                    .child(
                        Button::new("query-text-menu")
                            .ghost()
                            .icon(AssetIconName::Ellipsis)
                            .tooltip("Query text actions; right-click a field for selection editing")
                            .accessibility_label("Query text actions")
                            .dropdown_menu(move |menu, _, cx| {
                                let clipboard_text = cx.read_from_clipboard().and_then(|item| item.text());
                                let paste_search = clipboard_text.clone();
                                let paste_filter = clipboard_text;
                                let copy_search_target = menu_target.clone();
                                let copy_filter_target = menu_target.clone();
                                let paste_search_target = menu_target.clone();
                                let paste_filter_target = menu_target.clone();
                                menu.item(
                                    PopupMenuItem::new("Copy entire search")
                                        .icon(AssetIconName::Copy)
                                        .disabled(!search_text_available)
                                        .on_click(move |_, _, cx| {
                                            copy_search_target.update(cx, |this, cx| {
                                                this.copy_query_text(false, cx);
                                            });
                                        }),
                                )
                                .item(
                                    PopupMenuItem::new("Paste as search (replace)")
                                        .icon(AssetIconName::ClipboardPaste)
                                        .disabled(paste_search.is_none())
                                        .on_click(move |_, window, cx| {
                                            if let Some(text) = paste_search.clone() {
                                                paste_search_target.update(cx, |this, cx| {
                                                    this.paste_query_text(false, text, window, cx);
                                                });
                                            }
                                        }),
                                )
                                .separator()
                                .item(
                                    PopupMenuItem::new("Copy entire filter")
                                        .icon(AssetIconName::Copy)
                                        .disabled(!filter_text_available)
                                        .on_click(move |_, _, cx| {
                                            copy_filter_target.update(cx, |this, cx| {
                                                this.copy_query_text(true, cx);
                                            });
                                        }),
                                )
                                .item(
                                    PopupMenuItem::new("Paste as filter (replace)")
                                        .icon(AssetIconName::ClipboardPaste)
                                        .disabled(paste_filter.is_none())
                                        .on_click(move |_, window, cx| {
                                            if let Some(text) = paste_filter.clone() {
                                                paste_filter_target.update(cx, |this, cx| {
                                                    this.paste_query_text(true, text, window, cx);
                                                });
                                            }
                                        }),
                                )
                            }),
                    )
                    .child(
                        Button::new("query-case")
                            .label("Aa")
                            .selected(self.state.search.case_sensitive)
                            .tooltip("Case sensitive")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.search.case_sensitive =
                                    !this.state.search.case_sensitive;
                                this.search_from_inputs(false, cx);
                            })),
                    )
                    .child(
                        Button::new("query-regex")
                            .label(".*")
                            .selected(self.state.search.regex)
                            .tooltip("Regular expression")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.search.regex = !this.state.search.regex;
                                this.search_from_inputs(false, cx);
                            })),
                    )
                    .child(
                        Button::new("query-filter-toggle")
                            .icon(AssetIconName::Funnel)
                            .tooltip("Filter")
                            .accessibility_label("Filter")
                            .selected(filter_enabled)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.state.search.filter_enabled =
                                    !this.state.search.filter_enabled;
                                if this.state.search.filter_enabled {
                                    this.inputs
                                        .structured
                                        .update(cx, |input, cx| input.focus(window, cx));
                                } else {
                                    this.state.show_filter_builder = false;
                                }
                                this.search_from_inputs(false, cx);
                            })),
                    )
                    .child(
                        Button::new("query-run")
                            .label("Run")
                            .primary()
                            .disabled(invalid)
                            .on_click(
                                cx.listener(|this, _, _, cx| this.search_from_inputs(true, cx)),
                            ),
                    )
                    .when(querying, |this| {
                        this.child(
                            Button::new("query-stop")
                                .label("Stop")
                                .on_click(cx.listener(|this, _, _, _| this.send(UiAction::Cancel))),
                        )
                    }),
            )
            .when(text_error.is_some(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(rgb(0xdc6b55))
                        .child(text_error.unwrap_or_default()),
                )
            })
            .when(filter_enabled, |this| {
                this.child(
                    div()
                        .relative()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .on_key_down(cx.listener(|this, event: &gpui_kit::KeyDownEvent, _, cx| {
                            let handled = match event.keystroke.key.as_str() {
                                "up" => this.move_completion(false, cx),
                                "down" => this.move_completion(true, cx),
                                "escape" if this.state.completion_context.is_some() => {
                                    this.dismiss_completion(cx);
                                    true
                                }
                                _ => false,
                            };
                            if handled {
                                cx.stop_propagation();
                            }
                        }))
                        .on_key_up(cx.listener(|this, event: &gpui_kit::KeyUpEvent, _, cx| {
                            if matches!(
                                event.keystroke.key.as_str(),
                                "left" | "right" | "home" | "end"
                            ) {
                                this.refresh_completion(cx);
                            }
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().text_sm().child("Filter records"))
                                .child(
                                    gpui_kit::base::Popup::new(
                                        "filter-completions",
                                        Input::new(&self.inputs.structured)
                                            .id("structured-input")
                                            .w_full()
                                            .border_color(rgb(if filter_invalid {
                                                0xdc6b55
                                            } else if dark {
                                                0x394655
                                            } else {
                                                0xcbd5e1
                                            }))
                                            .aria_label(if filter_invalid {
                                                format!(
                                                    "Structured filter, invalid: {}",
                                                    filter_explanation.clone().unwrap_or_default()
                                                )
                                            } else {
                                                "Structured filter".into()
                                            }),
                                    )
                                    .w_full()
                                    .when(
                                        self.state.completion_focused
                                            && !self.state.show_filter_builder
                                            && self.state.completion_context.is_some()
                                            && (self.state.completion_pending
                                                || !self.state.completion_suggestions.is_empty()),
                                        |popup| popup.content(self.render_completion_overlay(cx)),
                                    ),
                                )
                                .child(
                                    Button::new("filter-builder-toggle")
                                        .icon(AssetIconName::ListFilterPlus)
                                        .tooltip("Filter Builder")
                                        .accessibility_label("Filter Builder")
                                        .selected(self.state.show_filter_builder)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.state.show_filter_builder =
                                                !this.state.show_filter_builder;
                                            if this.state.show_filter_builder {
                                                this.dismiss_completion(cx);
                                            }
                                            cx.notify();
                                        })),
                                )
                                .child(Button::new("filter-clear").label("Clear").on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.inputs.structured.update(cx, |input, cx| {
                                            input.replace_all("", window, cx)
                                        });
                                    }),
                                )),
                        )
                        .when(self.suspended_filter.is_some(), |this| {
                            this.child(
                                Button::new("restore-inactive-filter")
                                    .ghost()
                                    .label("Restore inactive filter")
                                    .tooltip("Restore the filter retained before sorting")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        if let Some(text) = this.suspended_filter.take() {
                                            this.inputs.structured.update(cx, |input, cx| {
                                                input.set_value(text, window, cx)
                                            });
                                            this.state.search.filter_enabled = true;
                                            this.search_from_inputs(false, cx);
                                        }
                                    })),
                            )
                        }),
                )
            })
    }

    pub(super) fn render_completion_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = super::theme::palette(self.state.dark);
        div()
            .id("filter-completions")
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            .w(px(430.))
            .max_h(px(250.))
            .p_1()
            .flex()
            .flex_col()
            .bg(rgb(colors.panel))
            .border_1()
            .border_color(rgb(colors.border))
            .rounded_sm()
            .overflow_y_scrollbar()
            .children(self.state.completion_suggestions.iter().enumerate().map(|(index, (_, label))| {
                let (name, kind) = label.rsplit_once(" · ").unwrap_or((label.as_str(), "Value"));
                let selected = index == self.state.completion_selected;
                div()
                    .id(format!("filter-completion-{index}"))
                    .flex()
                    .items_center()
                    .justify_start()
                    .gap_2()
                    .w_full()
                    .h(px(26.))
                    .px_2()
                    .bg(rgb(if selected { if self.state.dark { 0x244767 } else { 0xdceafe } } else { colors.panel }))
                    .border_l_2()
                    .border_color(rgb(if selected { colors.accent } else { colors.panel }))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis()
                        .font_family(super::typography::MONO).font_weight(gpui_kit::FontWeight::BOLD)
                        .text_sm().child(name.to_owned()))
                    .child(div().text_xs().text_color(rgb(colors.secondary))
                        .font_weight(gpui_kit::FontWeight::LIGHT).italic().child(kind.to_owned()))
                    .on_mouse_down(gpui_kit::MouseButton::Left, cx.listener(move |this, _, window, cx| {
                        this.state.completion_selected = index;
                        this.accept_completion(window, cx);
                        cx.stop_propagation();
                    }))
            }))
            .when(self.state.completion_pending, |this| {
                this.child(div().text_xs().child("Loading suggestions…"))
            })
            .when(
                self.state.completion_partial && !self.state.completion_pending,
                |this| this.child(div().text_xs().child("More values may exist")),
            )
    }

    pub(super) fn render_builder_overlay(&self) -> impl IntoElement {
        let right = if self.state.inspector_visible {
            self.state.inspector_width + 8.
        } else {
            8.
        };
        div()
            .absolute()
            .top(px(150.))
            .right(px(right))
            .child(self.builder_view.clone())
    }
}
