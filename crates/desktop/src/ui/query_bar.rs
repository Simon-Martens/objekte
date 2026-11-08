//! GPUI controls for the optional synchronized filter builder.

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    input::{Input, InputEvent, InputState},
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
    Close,
}


/// GPUI text controls for one builder condition.
struct RowInputs {
    field: Entity<InputState>,
    value: Entity<InputState>,
}


/// Nonmodal helper; the surrounding search field owns the actual filter text.
pub struct FilterBuilderView {
    state: BuilderState,
    source_text: String,
    fields: Vec<FieldChoice>,
    rows: Vec<RowInputs>,
    subscriptions: Vec<Subscription>,
    selected_row: usize,
    local_error: Option<String>,
    needs_rebuild: bool,
}


impl EventEmitter<BuilderEvent> for FilterBuilderView {}


impl FilterBuilderView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            state: BuilderState::from_text(""),
            source_text: String::new(),
            fields: Vec::new(),
            rows: Vec::new(),
            subscriptions: Vec::new(),
            selected_row: 0,
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
        self.needs_rebuild = true;
        cx.notify();
    }

    pub fn set_fields(&mut self, mut fields: Vec<FieldChoice>, cx: &mut Context<Self>) {
        fields.truncate(128);
        self.fields = fields;
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
        let editable = matches!(
            status,
            BuilderStatus::Empty | BuilderStatus::Synchronized | BuilderStatus::Incomplete
        );
        div().id("filter-builder").w(px(560.)).max_h(px(320.)).p_3().gap_2().flex().flex_col()
            .bg(cx.theme().popover).text_color(cx.theme().popover_foreground)
            .border_1().border_color(cx.theme().border).rounded_md()
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
    /// Compact search row, optional filter line, and expandable query options.
    /// The two InputState values remain authoritative across every render.
    pub(super) fn render_query_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let text_error = self.state.errors.get(&ErrorField::FullText).cloned();
        let filter_error = self.state.errors.get(&ErrorField::Structured).cloned();
        let validation = self.state.filter_validation.clone();
        let invalid = matches!(
            validation,
            Some(FilterValidationState::Pending | FilterValidationState::Invalid { .. })
        );
        let filter_invalid = matches!(validation, Some(FilterValidationState::Invalid { .. }));
        let filter_enabled = self.state.search.filter_enabled;
        let querying = self.state.busy == Busy::Querying;
        let dark = self.state.dark;
        div()
            .id("query-bar")
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
                            .label("Filter")
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
                        Button::new("query-options-toggle")
                            .label("Options")
                            .selected(self.show_query_options)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_query_options = !this.show_query_options;
                                cx.notify();
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
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().text_sm().child("Filter records"))
                                .child(
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
                                            "Structured filter, invalid"
                                        } else {
                                            "Structured filter"
                                        }),
                                )
                                .child(
                                    Button::new("filter-builder-toggle")
                                        .label("Build filter")
                                        .selected(self.state.show_filter_builder)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.state.show_filter_builder =
                                                !this.state.show_filter_builder;
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
                        .when(self.state.show_filter_builder, |this| {
                            this.child(
                                div()
                                    .absolute()
                                    .top(px(38.))
                                    .right(px(0.))
                                    .child(self.builder_view.clone()),
                            )
                        })
                        .when(filter_error.is_some(), |this| {
                            this.child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(0xdc6b55))
                                    .child(filter_error.unwrap_or_default()),
                            )
                        }),
                )
            })
            .when(filter_enabled && validation.is_some(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(rgb(match validation {
                            Some(FilterValidationState::Invalid { .. }) => 0xdc6b55,
                            _ => {
                                if dark {
                                    0xaab7c4
                                } else {
                                    0x43566a
                                }
                            }
                        }))
                        .child(match validation {
                            Some(FilterValidationState::Pending) => "Checking filter…".to_owned(),
                            Some(FilterValidationState::Valid) => "Filter valid".to_owned(),
                            Some(FilterValidationState::Invalid {
                                message,
                                byte_offset,
                            }) => match byte_offset {
                                Some(offset) => format!("Error: {message} (byte {offset})"),
                                None => format!("Error: {message}"),
                            },
                            None => String::new(),
                        }),
                )
            })
            .when(self.state.showing_last_valid_results, |this| {
                this.child(div().text_sm().child("Showing last valid results"))
            })
            .when(self.show_query_options, |this| {
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().text_sm().child("Sort"))
                        .child(
                            Input::new(&self.inputs.sort)
                                .id("sort-input")
                                .w(px(160.))
                                .aria_label("Sort field"),
                        )
                        .child(div().text_sm().child("Offset"))
                        .child(
                            Input::new(&self.inputs.offset)
                                .id("offset-input")
                                .w(px(90.))
                                .aria_label("Result offset"),
                        )
                        .child(div().text_sm().child("Limit"))
                        .child(
                            Input::new(&self.inputs.limit)
                                .id("limit-input")
                                .w(px(90.))
                                .aria_label("Result limit"),
                        )
                        .child(
                            Checkbox::new("query-exact-count")
                                .accessibility_label("Exact count")
                                .child(
                                    div()
                                        .text_color(rgb(if dark { 0xe2e8f0 } else { 0x172033 }))
                                        .child("Exact count"),
                                )
                                .checked(self.state.search.count)
                                .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                    this.state.search.count = *checked;
                                    this.search_from_inputs(false, cx);
                                })),
                        ),
                )
            })
    }
}
