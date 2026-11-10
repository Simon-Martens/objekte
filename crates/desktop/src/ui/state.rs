//! Local view state. Every update arrives only after the integrator has
//! checked the repository and query generations that produced it.

use gpui_kit::SharedString;
use std::collections::{BTreeMap, VecDeque};

use super::completion::{CompletionContext, CompletionTarget};
use super::filter_builder::FieldChoice;
use super::model::{
    BatchRow, BatchSource, DetailView, ErrorField, ExportFormat, FilterValidationState,
    IndexUiStatus, MAX_BATCH_ROWS, OpenDraft, PageView, QueryEntry, RecordTabSummary, ResultRow,
    SearchDraft, TabViewState, UiUpdate,
};
use super::open_dialog::{DetectionState, OpenDialogState};
use super::workspace::Workspace;

/// A small, read-only preview for a query visible beside the live query.
/// Query execution and result locators remain controller-owned.
#[derive(Clone, Debug, Default)]
pub struct QuerySnapshot {
    pub columns: Vec<String>,
    pub rows: Vec<ResultRow>,
    pub exact_total: Option<usize>,
}

/// Lightweight content retained for open record tabs shown beside the live inspector.
#[derive(Clone, Debug, Default)]
pub struct RecordPreview {
    pub title: String,
    pub fields: Vec<(String, String)>,
    pub clipped: bool,
}

impl RecordPreview {
    fn from_detail(detail: &DetailView) -> Self {
        const PREVIEW_ROWS: usize = 128;
        const PREVIEW_BYTES: usize = 64 * 1024;
        const PREVIEW_CELL_BYTES: usize = 1024;

        fn bounded_text(text: &str, budget: &mut usize, cell_limit: usize) -> (String, bool) {
            let limit = (*budget).min(cell_limit);
            let end = text
                .char_indices()
                .map(|(offset, character)| offset + character.len_utf8())
                .take_while(|end| *end <= limit)
                .last()
                .unwrap_or(0);
            *budget -= end;
            (text[..end].to_owned(), end < text.len())
        }

        let mut budget = PREVIEW_BYTES;
        let (title, title_clipped) = bounded_text(&detail.title, &mut budget, PREVIEW_CELL_BYTES);
        let mut preview = Self {
            title,
            fields: Vec::with_capacity(detail.structured.len().min(PREVIEW_ROWS)),
            clipped: detail.clipped || title_clipped || detail.structured.len() > PREVIEW_ROWS,
        };
        for row in detail.structured.iter().take(PREVIEW_ROWS) {
            if budget == 0 {
                preview.clipped = true;
                break;
            }
            let (label, label_clipped) = bounded_text(&row.label, &mut budget, PREVIEW_CELL_BYTES);
            let (value, value_clipped) = bounded_text(&row.value, &mut budget, PREVIEW_CELL_BYTES);
            preview.clipped |= label_clipped || value_clipped;
            preview.fields.push((label, value));
        }
        preview.clipped |= preview.fields.len() < detail.structured.len();
        preview
    }
}

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
    pub workspace: Workspace,
    pub query_snapshots: BTreeMap<u64, QuerySnapshot>,
    pub record_detail_snapshots: BTreeMap<u64, RecordPreview>,
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
    pub closing_index: Option<Option<String>>,
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
    pub index_status: IndexUiStatus,
    pub index_load_draft: String,
    pub index_save_draft: String,
    pub index_overwrite_pending: Option<String>,
    pub index_interval_draft: String,
    pub index_interval_error: Option<String>,
    pub index_picker_generation: u64,
    pub result_outdated: bool,
    pub show_raw: bool,
    pub show_links: bool,
    pub sidebar_visible: bool,
    pub history_expanded: bool,
    pub inspector_width: f32,
    pub inspector_resize_origin: Option<(f32, f32)>,
    pub secondary_query_width: f32,
    pub secondary_resize_origin: Option<(f32, f32)>,
    pub show_detail_metadata: bool,
    pub filter_validation_generation: u64,
    pub filter_validation: Option<FilterValidationState>,
    pub completion_fields: Vec<FieldChoice>,
    pub completion_revision: u64,
    pub completion_context: Option<CompletionContext>,
    pub completion_focused: bool,
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
                self.query_snapshots.clear();
                self.record_detail_snapshots.clear();
                self.tab_views.clear();
                self.detail_cache = DetailCache::default();
                self.detail_source = None;
                self.inspector_visible = false;
                self.diagnostics.clear();
                self.index_status = IndexUiStatus::default();
                self.index_load_draft.clear();
                self.index_save_draft.clear();
                self.index_overwrite_pending = None;
                self.index_interval_draft.clear();
                self.index_interval_error = None;
                self.result_outdated = false;
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
                self.workspace.opened(self.search.collection.clone());
                self.workspace.update_current_search(self.search.clone());
            }
            UiUpdate::QueryStarted(status) => {
                self.busy = Busy::Querying;
                self.status = status;
                self.errors.remove(&ErrorField::FullText);
                self.errors.remove(&ErrorField::Structured);
                self.errors.remove(&ErrorField::Sort);
                self.errors.remove(&ErrorField::Range);
            }
            UiUpdate::IndexReplaced(status) => {
                self.install_index_status(status, true);
                self.result_outdated = true;
                self.query_snapshots.clear();
            }
            UiUpdate::IndexStatus(status) => self.install_index_status(status, false),
            UiUpdate::ClosingIndex { error } => self.closing_index = Some(error),
            UiUpdate::CloseCancelled | UiUpdate::CloseReady => self.closing_index = None,
            UiUpdate::SaveIndexOverwriteRequired(path) => self.index_overwrite_pending = Some(path),
            UiUpdate::ResultsOutdated(outdated) => {
                self.result_outdated = outdated;
                if outdated {
                    self.query_snapshots.clear();
                }
            }
            UiUpdate::ChosenIndexLoadPath { generation, path } => {
                if generation == self.index_picker_generation {
                    self.index_load_draft = path;
                }
            }
            UiUpdate::ChosenIndexSavePath { generation, path } => {
                if generation == self.index_picker_generation {
                    self.index_save_draft = path;
                }
            }
            UiUpdate::Page(mut page) => {
                if page.incomplete {
                    page.exact_total = None;
                }
                for (index, column) in page.columns.iter().enumerate() {
                    self.column_widths.entry(column.clone()).or_insert_with(|| {
                        if column == "@status" { return 68.; }
                        let widest = page
                            .rows
                            .iter()
                            .take(24)
                            .filter_map(|row| row.cells.get(index))
                            .map(|cell| cell.chars().take(56).count())
                            .max()
                            .unwrap_or(0)
                            .max(column.chars().count());
                        (widest as f32 * 7.3 + 30.)
                            .clamp(if column == "@match" { 260. } else { 110. }, 420.)
                    });
                }
                page = page.bounded();
                self.delivered_count = page.rows.len();
                self.loaded_count = page.offset + self.delivered_count;
                self.projected_rows.clear();
                self.cached_page_starts.clear();
                self.has_more = page.has_next;
                if !self.has_more && !page.incomplete && page.exact_total.is_none() && (page.offset == 0 || self.delivered_count > 0) {
                    page.exact_total = Some(self.loaded_count);
                }
                self.loading_more = false;
                self.continuation_error = None;
                self.insert_projection(&mut page);
                self.page = page;
                self.showing_last_valid_results = false;
                self.result_outdated = false;
                self.busy = Busy::None;
                self.status = if self.page.incomplete {
                    "Some results could not be checked".into()
                } else if self.delivered_count == 0 {
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
                        if column == "@status" { return 68.; }
                        let widest = page
                            .rows
                            .iter()
                            .take(24)
                            .filter_map(|row| row.cells.get(index))
                            .map(|cell| cell.chars().take(56).count())
                            .max()
                            .unwrap_or(0)
                            .max(column.chars().count());
                        (widest as f32 * 7.3 + 30.)
                            .clamp(if column == "@match" { 260. } else { 110. }, 420.)
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
                self.page.incomplete |= page.incomplete;
                if self.page.incomplete {
                    self.page.exact_total = None;
                } else if let Some(total) = page.exact_total {
                    self.page.exact_total = Some(total);
                } else if !self.has_more {
                    self.page.exact_total = Some(self.loaded_count);
                }
                self.status = if self.page.incomplete {
                    "Some results could not be checked".into()
                } else {
                    format!("{} records loaded", self.delivered_count)
                };
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
                if !self.page.incomplete {
                    self.page.exact_total = Some(total);
                }
                self.page.count_pending = false;
            }
            UiUpdate::CountFailed(message) => {
                self.page.count_pending = false;
                self.status = format!("Could not count matches: {message}");
            }
            UiUpdate::RowSelected(index) => {
                self.page.selected = Some(index);
            }
            UiUpdate::RevealRow(index) => self.page.selected = Some(index),
            UiUpdate::Tabs { tabs, active } => {
                let changed = self.active_tab != active;
                if changed && let (Some(id), Some(detail)) = (self.active_tab, self.detail.as_ref()) {
                    self.record_detail_snapshots.insert(id, RecordPreview::from_detail(detail));
                }
                self.tabs = tabs;
                self.active_tab = active;
                self.workspace.sync_records(&self.tabs, active);
                self.prune_record_snapshots();
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
                if let (Some(id), Some(detail)) = (self.active_tab, self.detail.as_ref()) {
                    self.record_detail_snapshots.insert(id, RecordPreview::from_detail(detail));
                }
                self.prune_record_snapshots();
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
                self.workspace.open_query(draft.clone());
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
            UiUpdate::CompletionValues { origin, revision, field, prefix, values, partial, .. } => {
                if origin == super::model::CompletionOrigin::Filter
                    && revision == self.completion_revision
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

    /// Keep bounded previews for every open record tab so moving an active tab
    /// does not leave the other pane blank. The controller owns full details.
    pub fn prune_record_snapshots(&mut self) {
        let open = self
            .workspace
            .primary
            .tabs
            .iter()
            .chain(&self.workspace.secondary.tabs)
            .filter_map(|tab| match tab.id {
                super::workspace::TabId::Record(id) => Some(id),
                super::workspace::TabId::Search(_) => None,
            })
            .collect::<Vec<_>>();
        self.record_detail_snapshots
            .retain(|id, _| open.contains(id));
    }

    /// Preserve a bounded preview while another query uses the live editor.
    pub fn snapshot_current_query(&mut self) {
        let Some(super::workspace::TabId::Search(id)) = self.workspace.current_search else {
            return;
        };
        if self.showing_last_valid_results || self.result_outdated {
            self.query_snapshots.remove(&id);
            return;
        }
        let rows = self.projected_rows.values().take(40).cloned().collect();
        self.query_snapshots.insert(id, QuerySnapshot {
            columns: self.page.columns.clone(),
            rows,
            exact_total: self.page.exact_total,
        });
        self.prune_query_snapshots();
    }

    pub fn prune_query_snapshots(&mut self) {
        let visible = [self.workspace.primary.active, self.workspace.secondary.active];
        self.query_snapshots.retain(|id, _| {
            self.workspace.tab(super::workspace::TabId::Search(*id)).is_some()
        });
        while self.query_snapshots.len() > 3 {
            let oldest_hidden = self.query_snapshots.keys().find(|id| {
                !visible.contains(&Some(super::workspace::TabId::Search(**id)))
            }).copied();
            let oldest = oldest_hidden.or_else(|| self.query_snapshots.keys().next().copied());
            if let Some(id) = oldest {
                self.query_snapshots.remove(&id);
            }
        }
    }

    fn install_index_status(&mut self, status: IndexUiStatus, replacement: bool) {
        if replacement || self.index_status.index_instance != status.index_instance {
            self.index_load_draft = status.loaded_from.clone().unwrap_or_default();
            self.index_save_draft = status.save_destination.clone().unwrap_or_default();
            self.index_interval_draft = status.check_interval_seconds.to_string();
            self.index_interval_error = None;
        }
        if matches!(status.save, super::model::IndexTaskState::Succeeded(_)) {
            self.index_overwrite_pending = None;
        }
        self.index_status = status;
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
