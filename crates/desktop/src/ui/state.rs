//! Local view state. Every update arrives only after the integrator has
//! checked the repository and query generations that produced it.

use std::collections::BTreeMap;

use super::model::{
    BatchRow, BatchSource, DetailView, ErrorField, FilterValidationState, MAX_BATCH_ROWS,
    OpenDraft, PageView, QueryEntry, SearchDraft, UiUpdate,
};


/// Which operation currently owns the visible progress indicator.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Busy {
    #[default]
    None,
    Opening,
    Querying,
    Batch,
}


/// Bounded state rendered in one desktop window.
#[derive(Clone, Debug, Default)]
pub struct UiState {
    pub open: OpenDraft,
    pub search: SearchDraft,
    pub root: Option<String>,
    pub collections: Vec<String>,
    pub single_collection: bool,
    pub page: PageView,
    pub detail: Option<DetailView>,
    pub history: Vec<QueryEntry>,
    pub saved: Vec<QueryEntry>,
    pub available_columns: Vec<String>,
    pub hidden_columns: Vec<String>,
    pub batch: Vec<BatchRow>,
    pub batch_source: BatchSource,
    pub batch_outcome: Option<String>,
    pub busy: Busy,
    pub status: String,
    pub diagnostics: String,
    pub errors: BTreeMap<ErrorField, String>,
    pub dark: bool,
    pub show_preview: bool,
    pub show_open_options: bool,
    pub show_batch: bool,
    pub show_history: bool,
    pub show_saved: bool,
    pub show_columns: bool,
    pub show_help: bool,
    pub show_actions: bool,
    pub show_diagnostics: bool,
    pub show_raw: bool,
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
                self.root = None;
                self.collections.clear();
                self.available_columns.clear();
                self.hidden_columns.clear();
                self.history.clear();
                self.saved.clear();
                self.page = PageView::default();
                self.detail = None;
                self.diagnostics.clear();
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
                self.busy = Busy::None;
                self.status = "Repository ready".into();
                self.show_open_options = false;
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
                    self.show_raw = false;
                }
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
                self.errors.insert(field, message.clone());
                self.status = message;
                self.busy = Busy::None;
            }
            UiUpdate::ClearError(field) => {
                self.errors.remove(&field);
            }
            UiUpdate::ChosenPaths(paths) => {
                self.open.paths = paths;
            }
            UiUpdate::RestoreQuery(draft) => {
                self.search = draft;
                self.search.filter_enabled |= !self.search.structured.trim().is_empty();
                self.page = PageView::default();
                self.detail = None;
            }
            UiUpdate::FilterValidation { generation, state } => {
                if generation == self.filter_validation_generation {
                    self.filter_validation = Some(state);
                }
            }
            UiUpdate::Theme { dark } => self.dark = dark,
        }
    }
}


#[cfg(test)]
#[path = "state.test.rs"]
mod tests;
