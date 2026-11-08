//! Local view state. Every update arrives only after the integrator has
//! checked the repository and query generations that produced it.

use gpui_kit::SharedString;
use std::collections::{BTreeMap, VecDeque};

use super::completion::{CompletionContext, CompletionTarget};
use super::filter_builder::FieldChoice;
use super::model::{
    BatchRow, BatchSource, DetailView, ErrorField, ExportFormat, FilterValidationState,
    MAX_BATCH_ROWS, OpenDraft, PageView, QueryEntry, RecordTabSummary, ResultRow, SearchDraft,
    SnapshotMode, TabViewState, UiUpdate,
};
use super::open_dialog::{DetectionState, OpenDialogState};


/// One contextual secondary surface at a time.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ActionPanel {
    #[default]
    Columns,
    Export,
    Index,
    Diagnostics,
}


/// Which operation currently owns the visible progress indicator.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Busy {
    #[default]
    None,
    Opening,
    Querying,
    Batch,
}


/// Shared inspector text built once from a bounded detail projection.
#[derive(Clone, Debug, Default)]
pub struct DetailCache {
    pub labels: Vec<SharedString>,
    pub values: Vec<SharedString>,
    pub relation_labels: Vec<SharedString>,
    pub relation_values: Vec<SharedString>,
}


/// Bounded state rendered in one desktop window.
#[derive(Clone, Debug, Default)]
pub struct UiState {
    pub open: OpenDraft,
    pub open_dialog: OpenDialogState,
    pub search: SearchDraft,
    pub root: Option<String>,
    pub collections: Vec<String>,
    pub single_collection: bool,
    pub page: PageView,
    /// Lightweight extent is separate from a bounded cache of projected text.
    pub loaded_count: usize,
    pub delivered_count: usize,
    pub projected_rows: BTreeMap<usize, ResultRow>,
    pub(super) cached_page_starts: VecDeque<usize>,
    pub has_more: bool,
    pub loading_more: bool,
    pub continuation_error: Option<String>,
    pub detail: Option<DetailView>,
    pub tabs: Vec<RecordTabSummary>,
    pub active_tab: Option<u64>,
    pub(super) tab_views: BTreeMap<u64, TabViewState>,
    pub detail_cache: DetailCache,
    pub detail_source: Option<SharedString>,
    pub history: Vec<QueryEntry>,
    pub saved: Vec<QueryEntry>,
    pub available_columns: Vec<String>,
    pub hidden_columns: Vec<String>,
    /// Widths are measured once from the first bounded page for each column.
    pub column_widths: BTreeMap<String, f32>,
    pub wrapped_columns: Vec<String>,
    pub table_scope: Option<String>,
    pub column_resize_origin: Option<(String, f32, f32)>,
    pub batch: Vec<BatchRow>,
    pub batch_source: BatchSource,
    pub batch_outcome: Option<String>,
    pub busy: Busy,
    pub status: String,
    pub preference_write_failed: bool,
    pub diagnostics: String,
    pub errors: BTreeMap<ErrorField, String>,
    pub dark: bool,
    pub show_preview: bool,
    pub inspector_visible: bool,
    pub show_batch: bool,
    pub show_help: bool,
    pub show_actions: bool,
    pub action_panel: ActionPanel,
    pub export_format: ExportFormat,
    pub index_mode: SnapshotMode,
    pub show_raw: bool,
    pub show_links: bool,
    pub sidebar_visible: bool,
    pub history_expanded: bool,
    pub inspector_width: f32,
    pub inspector_resize_origin: Option<(f32, f32)>,
    pub show_detail_metadata: bool,
    pub filter_validation_generation: u64,
    pub filter_validation: Option<FilterValidationState>,
    pub completion_fields: Vec<FieldChoice>,
    pub completion_revision: u64,
    pub completion_context: Option<CompletionContext>,
    pub completion_suggestions: Vec<(String, String)>,
    pub completion_selected: usize,
    pub completion_partial: bool,
    pub completion_pending: bool,
    pub showing_last_valid_results: bool,
    pub show_filter_builder: bool,
}


impl UiState {
    /// Apply one controller update without retaining unbounded result content.
    pub fn apply(&mut self, update: UiUpdate) {
        match update {
            UiUpdate::Opening(status) => {
                self.busy = Busy::Opening;
                self.status = status;
                self.errors.clear();
            }
            UiUpdate::Opened {
                root,
                collections,
                single_collection,
            } => {
                self.root = Some(root);
                self.collections = collections;
                self.single_collection = single_collection;
                self.available_columns.clear();
                self.completion_fields.clear();
                self.completion_context = None;
                self.completion_suggestions.clear();
                self.hidden_columns.clear();
                self.column_widths.clear();
                self.wrapped_columns.clear();
                self.table_scope = None;
                self.column_resize_origin = None;
                self.history.clear();
                self.saved.clear();
                self.page = PageView::default();
                self.loaded_count = 0;
                self.delivered_count = 0;
                self.projected_rows.clear();
                self.cached_page_starts.clear();
                self.has_more = false;
                self.loading_more = false;
                self.continuation_error = None;
                self.detail = None;
                self.tabs.clear();
                self.active_tab = None;
                self.tab_views.clear();
                self.detail_cache = DetailCache::default();
                self.detail_source = None;
                self.inspector_visible = false;
                self.diagnostics.clear();
                self.preference_write_failed = false;
                self.busy = Busy::None;
                self.status = "Repository ready".into();
                self.open_dialog.close();
                self.errors.remove(&ErrorField::Open);
                if single_collection {
                    self.search.collection = self.collections.first().cloned();
                } else {
                    self.search.collection = None;
                }
            }
            UiUpdate::QueryStarted(status) => {
                self.busy = Busy::Querying;
                self.status = status;
                self.errors.remove(&ErrorField::FullText);
                self.errors.remove(&ErrorField::Structured);
                self.errors.remove(&ErrorField::Sort);
                self.errors.remove(&ErrorField::Range);
            }
            UiUpdate::Page(mut page) => {
                for (index, column) in page.columns.iter().enumerate() {
                    self.column_widths.entry(column.clone()).or_insert_with(|| {
                        let widest = page
                            .rows
                            .iter()
                            .take(24)
                            .filter_map(|row| row.cells.get(index))
                            .map(|cell| cell.chars().take(56).count())
                            .max()
                            .unwrap_or(0)
                            .max(column.chars().count());
                        (widest as f32 * 7.3 + 30.).clamp(110., 420.)
                    });
                }
                page = page.bounded();
                self.delivered_count = page.rows.len();
                self.loaded_count = page.offset + self.delivered_count;
                self.projected_rows.clear();
                self.cached_page_starts.clear();
                self.has_more = page.has_next;
                if !self.has_more && page.exact_total.is_none() && (page.offset == 0 || self.delivered_count > 0) {
                    page.exact_total = Some(self.loaded_count);
                }
                self.loading_more = false;
                self.continuation_error = None;
                self.insert_projection(&mut page);
                self.page = page;
                self.showing_last_valid_results = false;
                self.busy = Busy::None;
                self.status = if self.delivered_count == 0 {
                    "No matching records".into()
                } else {
                    format!("{} records loaded", self.delivered_count)
                };
            }
            UiUpdate::AppendPage(mut page) => {
                page = page.bounded();
                if page.offset != self.loaded_count {
                    return;
                }
                for (index, column) in page.columns.iter().enumerate() {
                    self.column_widths.entry(column.clone()).or_insert_with(|| {
                        let widest = page
                            .rows
                            .iter()
                            .take(24)
                            .filter_map(|row| row.cells.get(index))
                            .map(|cell| cell.chars().take(56).count())
                            .max()
                            .unwrap_or(0)
                            .max(column.chars().count());
                        (widest as f32 * 7.3 + 30.).clamp(110., 420.)
                    });
                }
                self.page.columns = page.columns.clone();
                let delivered = page.rows.len();
                self.insert_projection(&mut page);
                self.loaded_count += delivered;
                self.delivered_count += delivered;
                self.has_more = page.has_next;
                self.loading_more = false;
                self.continuation_error = None;
                if let Some(total) = page.exact_total {
                    self.page.exact_total = Some(total);
                } else if !self.has_more {
                    self.page.exact_total = Some(self.loaded_count);
                }
                self.status = format!("{} records loaded", self.delivered_count);
            }
            UiUpdate::Projection(mut page) => {
                page = page.bounded();
                self.insert_projection(&mut page);
            }
            UiUpdate::ContinuationStarted => {
                self.loading_more = true;
                self.continuation_error = None;
            }
            UiUpdate::ContinuationFailed(message) => {
                self.loading_more = false;
                self.continuation_error = Some(message.clone());
                self.status = format!("Next batch failed: {message}");
            }
            UiUpdate::CountStarted => self.page.count_pending = true,
            UiUpdate::ExactCount(total) => {
                self.page.exact_total = Some(total);
                self.page.count_pending = false;
            }
            UiUpdate::RowSelected(index) => {
                self.page.selected = Some(index);
            }
            UiUpdate::RevealRow(index) => self.page.selected = Some(index),
            UiUpdate::Tabs { tabs, active } => {
                let changed = self.active_tab != active;
                self.tabs = tabs;
                self.active_tab = active;
                self.tab_views
                    .retain(|id, _| self.tabs.iter().any(|tab| tab.id == *id));
                if changed {
                    let view = active.and_then(|id| self.tab_views.get(&id).copied()).unwrap_or_default();
                    (self.show_raw, self.show_links, self.show_detail_metadata) = (view.raw, view.links, view.metadata);
                    self.detail = None;
                    self.detail_cache = DetailCache::default();
                    self.detail_source = None;
                }
                self.inspector_visible = active.is_some();
            }
            UiUpdate::Detail(detail) => {
                self.detail = detail.map(DetailView::bounded);
                self.detail_cache =
                    self.detail
                        .as_ref()
                        .map_or_else(DetailCache::default, |detail| DetailCache {
                            labels: detail
                                .structured
                                .iter()
                                .map(|row| {
                                    SharedString::from(
                                        if row.origin == super::model::DetailOrigin::Structured {
                                            row.label
                                                .strip_prefix("$.")
                                                .unwrap_or(&row.label)
                                                .to_owned()
                                        } else {
                                            row.label.clone()
                                        },
                                    )
                                })
                                .collect(),
                            values: detail
                                .structured
                                .iter()
                                .map(|row| SharedString::from(row.value.clone()))
                                .collect(),
                            relation_labels: detail
                                .relations
                                .iter()
                                .map(|row| SharedString::from(row.label.clone()))
                                .collect(),
                            relation_values: detail
                                .relations
                                .iter()
                                .map(|row| SharedString::from(row.value.clone()))
                                .collect(),
                        });
                self.detail_source = self
                    .detail
                    .as_ref()
                    .map(|detail| SharedString::from(detail.raw_source.clone()));
                if self.detail.is_none() {
                    self.show_raw = false;
                }
            }
            UiUpdate::RestoreTabView(view) => {
                (self.show_raw, self.show_links, self.show_detail_metadata) = (view.raw, view.links, view.metadata);
                if let Some(id) = self.active_tab { self.tab_views.insert(id, view); }
            }
            UiUpdate::History {
                mut entries,
                mut saved,
            } => {
                if entries.len() > 200 {
                    entries.drain(..entries.len() - 200);
                }
                if saved.len() > 200 {
                    saved.drain(..saved.len() - 200);
                }
                self.history = entries;
                self.saved = saved;
            }
            UiUpdate::Columns {
                mut available,
                mut hidden,
            } => {
                available.truncate(256);
                hidden.truncate(256);
                self.available_columns = available;
                self.hidden_columns = hidden;
            }
            UiUpdate::TablePresentation {
                scope,
                widths,
                wrapped,
            } => {
                if self.table_scope != scope {
                    self.column_widths.clear();
                    self.table_scope = scope;
                }
                self.column_widths
                    .extend(widths.into_iter().map(|(name, width)| (name, width as f32)));
                self.wrapped_columns = wrapped;
            }
            UiUpdate::Batch {
                mut rows,
                running,
                outcome,
            } => {
                if rows.len() > MAX_BATCH_ROWS {
                    rows.drain(..rows.len() - MAX_BATCH_ROWS);
                }
                self.batch = rows;
                self.batch_outcome = outcome;
                self.busy = if running { Busy::Batch } else { Busy::None };
            }
            UiUpdate::Diagnostics(diagnostics) => {
                self.diagnostics = diagnostics.chars().take(16 * 1024).collect();
            }
            UiUpdate::Clipboard(_) => {}
            UiUpdate::Status(status) => {
                self.status = status;
                self.busy = Busy::None;
            }
            UiUpdate::PreferenceSaved => {
                self.preference_write_failed = false;
            }
            UiUpdate::PreferenceFailed(message) => {
                self.preference_write_failed = true;
                self.status = format!("Preferences not saved: {message}");
            }
            UiUpdate::Error { field, message } => {
                if field == ErrorField::Open {
                    self.open_dialog.error = Some(message.clone());
                }
                self.errors.insert(field, message.clone());
                self.status = message;
                self.busy = Busy::None;
            }
            UiUpdate::ClearError(field) => {
                self.errors.remove(&field);
            }
            UiUpdate::OpenDetection { generation, result } => {
                if let DetectionState::Pending { path } = &self.open_dialog.detection {
                    let path = path.clone();
                    self.open_dialog.accept_detection(generation, &path, result);
                }
            }
            UiUpdate::ChosenFolder { .. } | UiUpdate::ChosenSchema { .. } => {}
            UiUpdate::RestoreQuery(draft) => {
                self.search = draft;
                self.completion_context = None;
                self.completion_suggestions.clear();
                self.page = PageView::default();
                self.loaded_count = 0;
                self.delivered_count = 0;
                self.projected_rows.clear();
                self.cached_page_starts.clear();
                self.has_more = false;
                self.loading_more = false;
                self.continuation_error = None;
            }
            UiUpdate::FilterValidation { generation, state } => {
                if generation >= self.filter_validation_generation {
                    self.filter_validation_generation = generation;
                    if let FilterValidationState::Invalid { message, .. } = &state {
                        self.status = format!("Filter error: {message}");
                    }
                    self.showing_last_valid_results = self.root.is_some()
                        && !matches!(state, FilterValidationState::Valid)
                        && self.loaded_count > 0;
                    self.filter_validation = Some(state);
                    if self.busy == Busy::Querying {
                        self.busy = Busy::None;
                    }
                }
            }
            UiUpdate::FieldChoices(fields) => {
                self.completion_fields = fields;
            }
            UiUpdate::CompletionValues { revision, field, prefix, values, partial, .. } => {
                if revision == self.completion_revision
                    && self.completion_context.as_ref().is_some_and(|context| {
                        context.prefix == prefix && matches!(&context.target, CompletionTarget::Value { field: active } if active == &field)
                    }) {
                    self.completion_suggestions = values;
                    self.completion_selected = 0;
                    self.completion_partial = partial;
                    self.completion_pending = false;
                }
            }
            UiUpdate::Theme { dark } => self.dark = dark,
        }
    }

    /// At most three 4 MiB page projections are resident. Result locators live
    /// in the controller and are unaffected by this display-cache eviction.
    fn insert_projection(&mut self, page: &mut PageView) {
        if page.rows.is_empty() {
            return;
        }
        let start = page.offset;
        self.cached_page_starts.retain(|old| *old != start);
        self.cached_page_starts.push_back(start);
        for (index, row) in std::mem::take(&mut page.rows).into_iter().enumerate() {
            self.projected_rows.insert(start + index, row);
        }
        while self.cached_page_starts.len() > 3 {
            if let Some(evicted) = self.cached_page_starts.pop_front() {
                self.projected_rows
                    .retain(|index, _| *index < evicted || *index >= evicted + 100);
            }
        }
    }
}


#[cfg(test)]
#[path = "state.test.rs"]
mod tests;
