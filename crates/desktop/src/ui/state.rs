//! Local view state. Every update arrives only after the integrator has
//! checked the repository and query generations that produced it.

use gpui_kit::SharedString;
use std::collections::BTreeMap;

use super::model::{
    BatchRow, BatchSource, DetailView, ErrorField, FilterValidationState, MAX_BATCH_ROWS,
    OpenDraft, PageView, QueryEntry, SearchDraft, UiUpdate,
};
use super::open_dialog::{DetectionState, OpenDialogState};


/// Which operation currently owns the visible progress indicator.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Busy {
    #[default]
    None,
    Opening,
    Querying,
    Batch,
}


/// Shared cell text built once when a bounded result page arrives.
#[derive(Clone, Debug, Default)]
pub struct GridCache {
    pub rows: Vec<Vec<SharedString>>,
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
    pub grid_cache: GridCache,
    pub detail: Option<DetailView>,
    pub detail_cache: DetailCache,
    pub detail_source: Option<SharedString>,
    pub history: Vec<QueryEntry>,
    pub saved: Vec<QueryEntry>,
    pub available_columns: Vec<String>,
    pub hidden_columns: Vec<String>,
    /// Widths are measured once from the first bounded page for each column.
    pub column_widths: BTreeMap<String, f32>,
    pub batch: Vec<BatchRow>,
    pub batch_source: BatchSource,
    pub batch_outcome: Option<String>,
    pub busy: Busy,
    pub status: String,
    pub diagnostics: String,
    pub errors: BTreeMap<ErrorField, String>,
    pub dark: bool,
    pub show_preview: bool,
    pub inspector_visible: bool,
    pub show_batch: bool,
    pub show_history: bool,
    pub show_saved: bool,
    pub show_columns: bool,
    pub show_help: bool,
    pub show_actions: bool,
    pub show_diagnostics: bool,
    pub show_raw: bool,
    pub sidebar_visible: bool,
    pub inspector_width: f32,
    pub inspector_resize_origin: Option<(f32, f32)>,
    pub show_detail_metadata: bool,
    pub filter_validation_generation: u64,
    pub filter_validation: Option<FilterValidationState>,
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
                self.hidden_columns.clear();
                self.column_widths.clear();
                self.history.clear();
                self.saved.clear();
                self.page = PageView::default();
                self.grid_cache = GridCache::default();
                self.detail = None;
                self.detail_cache = DetailCache::default();
                self.detail_source = None;
                self.inspector_visible = false;
                self.diagnostics.clear();
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
                self.detail = None;
                self.errors.remove(&ErrorField::FullText);
                self.errors.remove(&ErrorField::Structured);
                self.errors.remove(&ErrorField::Sort);
                self.errors.remove(&ErrorField::Range);
            }
            UiUpdate::Page(page) => {
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
                let previous_token = self
                    .page
                    .selected
                    .and_then(|index| self.page.rows.get(index))
                    .map(|row| row.key.token);
                let page = page.bounded();
                let next_token = page
                    .selected
                    .and_then(|index| page.rows.get(index))
                    .map(|row| row.key.token);
                if next_token.is_none() || previous_token != next_token {
                    self.detail = None;
                    self.detail_cache = DetailCache::default();
                    self.detail_source = None;
                    self.show_raw = false;
                }
                if page.selected.is_none() {
                    self.inspector_visible = false;
                }
                self.grid_cache.rows = page
                    .rows
                    .iter()
                    .map(|row| row.cells.iter().cloned().map(SharedString::from).collect())
                    .collect();
                self.page = page;
                self.showing_last_valid_results = false;
                self.busy = Busy::None;
                self.status = if self.page.rows.is_empty() {
                    "No matching records".into()
                } else {
                    format!(
                        "Showing {} records from offset {}",
                        self.page.rows.len(),
                        self.page.offset
                    )
                };
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
                                .map(|row| SharedString::from(row.label.clone()))
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
                available.truncate(super::model::MAX_COLUMNS);
                hidden.truncate(super::model::MAX_COLUMNS);
                self.available_columns = available;
                self.hidden_columns = hidden;
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
                self.search.filter_enabled |= !self.search.structured.trim().is_empty();
                self.page = PageView::default();
                self.grid_cache = GridCache::default();
                self.detail = None;
                self.detail_cache = DetailCache::default();
                self.detail_source = None;
            }
            UiUpdate::FilterValidation { generation, state } => {
                if generation >= self.filter_validation_generation {
                    self.filter_validation_generation = generation;
                    self.showing_last_valid_results = self.root.is_some()
                        && !matches!(state, FilterValidationState::Valid)
                        && (!self.page.rows.is_empty() || !self.page.columns.is_empty());
                    self.filter_validation = Some(state);
                    if self.busy == Busy::Querying {
                        self.busy = Busy::None;
                    }
                }
            }
            UiUpdate::FieldChoices(_) => {}
            UiUpdate::Theme { dark } => self.dark = dark,
        }
    }
}


#[cfg(test)]
#[path = "state.test.rs"]
mod tests;
