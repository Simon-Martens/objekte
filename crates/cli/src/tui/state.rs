//! Search state, keyboard editing, and page navigation for the terminal UI.

use super::navigation::DetailLinks;
use super::warm::WarmMode;
use objekte_core::{
    CollectionInventory, Error, InferenceReport, JsonFilter, JsonPathStep, JsonQuery,
    JsonQueryBackend, Record, Repository, Result, Sort, TextHit, TextMode, TextQuery,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::VecDeque;

pub(super) const PAGE_SIZE: usize = 100;


/// Whether the input is a field filter or a standalone full-text search.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SearchKind {
    Filter,
    Text,
    Combined,
}


/// Which full-text representation a query inspects.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Representation {
    Normalized,
    Full,
}


/// A displayed row, retaining the match metadata returned by its search mode.
#[derive(Clone)]
pub(super) enum Hit {
    Filter(Record),
    Text(TextHit),
}


impl Hit {
    pub(super) fn record(&self) -> &Record {
        match self {
            Self::Filter(record) => record,
            Self::Text(hit) => &hit.record,
        }
    }

    pub(super) fn matched_paths(&self) -> &[String] {
        match self {
            Self::Filter(_) => &[],
            Self::Text(hit) => &hit.matched_paths,
        }
    }
}


/// Request used for the visible page and for complete export.
#[derive(Clone)]
pub(super) enum Request {
    Filter(JsonQuery),
    Text(TextQuery),
}


impl Request {
    pub(super) fn whole_result(&self) -> Self {
        match self {
            Self::Filter(request) => Self::Filter(JsonQuery {
                offset: 0,
                limit: i64::MAX as usize,
                count: true,
                ..request.clone()
            }),
            Self::Text(request) => Self::Text(TextQuery {
                offset: 0,
                limit: i64::MAX as usize,
                count: true,
                ..request.clone()
            }),
        }
    }
}


fn queried_fields(request: &Request) -> Vec<String> {
    let (source, parameters) = match request {
        Request::Filter(query) => (&query.filter, &query.parameters),
        Request::Text(query) => match &query.filter {
            Some(filter) => (filter, &query.parameters),
            None => return Vec::new(),
        },
    };
    let filter = objekte_core::split_sort_clause(source)
        .map(|(filter, _)| filter)
        .unwrap_or_else(|_| source.clone());
    JsonFilter::compile(&filter, parameters)
        .map(|filter| {
            let mut fields = Vec::new();
            for path in filter.paths_in_query_order() {
                let top = match path.first() {
                    Some(JsonPathStep::Key(name)) => name.clone(),
                    Some(JsonPathStep::Index(_) | JsonPathStep::Each) => "$value".into(),
                    None => continue,
                };
                if !fields.contains(&top) {
                    fields.push(top);
                }
            }
            fields
        })
        .unwrap_or_default()
}

fn ordered_fields(request: &Request) -> Vec<String> {
    let sort: Vec<Sort> = match request {
        Request::Filter(query) if !query.sort.is_empty() => query.sort.clone(),
        Request::Filter(query) => objekte_core::split_sort_clause(&query.filter)
            .map(|(_, sort)| sort)
            .unwrap_or_default(),
        Request::Text(query) => query.sort.clone(),
    };
    let mut fields = Vec::new();
    for term in sort {
        if term.field == "@id" {
            continue;
        }
        let Ok(path) = JsonFilter::parse_path(&term.field) else {
            continue;
        };
        let top = match path.first() {
            Some(JsonPathStep::Key(name)) => name.clone(),
            Some(JsonPathStep::Index(_) | JsonPathStep::Each) => "$value".into(),
            None => continue,
        };
        if !fields.contains(&top) {
            fields.push(top);
        }
    }
    fields
}


/// Cursor-aware single-line text input shared by query and export prompts.
#[derive(Default)]
pub(super) struct Input {
    pub(super) text: String,
    pub(super) cursor: usize,
}


impl Input {
    fn byte_at(&self, character: usize) -> usize {
        self.text
            .char_indices()
            .nth(character)
            .map_or(self.text.len(), |(index, _)| index)
    }

    pub(super) fn key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char(ch)
                if !ch.is_control()
                    && (!key.modifiers.contains(KeyModifiers::CONTROL)
                        || key.modifiers.contains(KeyModifiers::ALT)) =>
            {
                // Some CSI-u terminals omit the shifted codepoint. Recover
                // ASCII letters without assuming a layout for punctuation.
                // Ctrl+Alt may be AltGr on international keyboards.
                let ch = if key.modifiers.contains(KeyModifiers::SHIFT) {
                    ch.to_ascii_uppercase()
                } else {
                    ch
                };
                self.text.insert(self.byte_at(self.cursor), ch);
                self.cursor += 1;
                true
            }
            KeyCode::Backspace if self.cursor > 0 => {
                let start = self.byte_at(self.cursor - 1);
                let end = self.byte_at(self.cursor);
                self.text.replace_range(start..end, "");
                self.cursor -= 1;
                true
            }
            KeyCode::Delete if self.cursor < self.text.chars().count() => {
                let start = self.byte_at(self.cursor);
                let end = self.byte_at(self.cursor + 1);
                self.text.replace_range(start..end, "");
                true
            }
            KeyCode::Left => {
                self.cursor = self.cursor.saturating_sub(1);
                true
            }
            KeyCode::Right => {
                self.cursor = (self.cursor + 1).min(self.text.chars().count());
                true
            }
            KeyCode::Home => {
                self.cursor = 0;
                true
            }
            KeyCode::End => {
                self.cursor = self.text.chars().count();
                true
            }
            _ => false,
        }
    }
}


/// Which panel currently consumes ordinary character and arrow keys.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Focus {
    Query,
    FilterInput,
    Results,
    Detail,
    ExportAll,
    ExportOne,
    Infer,
    InferReady,
}


/// Scope and schema choice for an explicit background inference scan.
pub(super) struct InferRequest {
    pub(super) collection: Option<String>,
    pub(super) auto_index: bool,
}


/// Result to open after an adjacent detail page finishes streaming.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DetailEdge {
    First,
    Last,
}


/// Long-lived repository handle and the current interactive query page.
pub(super) struct App<'a> {
    pub(super) repo: &'a mut Repository,
    pub(super) collections: Vec<String>,
    pub(super) collection: usize,
    pub(super) kind: SearchKind,
    pub(super) representation: Representation,
    pub(super) regex: bool,
    pub(super) case_sensitive: bool,
    pub(super) input: Input,
    pub(super) filter_input: Input,
    pub(super) export_input: Input,
    pub(super) export_return_focus: Focus,
    pub(super) focus: Focus,
    pub(super) hits: Vec<Hit>,
    pub(super) query_fields: Vec<String>,
    pub(super) sort_fields: Vec<String>,
    pub(super) async_search: bool,
    pub(super) searching: bool,
    pub(super) search_generation: u64,
    pub(super) pending_search: Option<Request>,
    pub(super) pending_detail: Option<DetailEdge>,
    pub(super) last_request: Option<Request>,
    pub(super) total: Option<usize>,
    pub(super) total_truncated: bool,
    pub(super) search_elapsed_ms: Option<f64>,
    pub(super) offset: usize,
    pub(super) selected: usize,
    pub(super) column_offset: usize,
    pub(super) detail_scroll: u16,
    pub(super) links: DetailLinks,
    pub(super) message: String,
    pub(super) index_status: Option<String>,
    pub(super) reconciling: bool,
    pub(super) inventory_complete: bool,
    pub(super) pending_inventories: VecDeque<CollectionInventory>,
    pub(super) reconciled_files: usize,
    pub(super) pending_cache_action: Option<WarmMode>,
    pub(super) queued_search: bool,
    pub(super) infer_all: bool,
    pub(super) infer_auto_index: bool,
    pub(super) infer_return_focus: Focus,
    pub(super) pending_inference: Option<InferRequest>,
    pub(super) last_inference: Option<InferenceReport>,
    pub(super) pending_schema_write: bool,
    pub(super) inference_saved: bool,
    pub(super) done: bool,
}


impl<'a> App<'a> {
    pub(super) fn new(repo: &'a mut Repository) -> Self {
        Self {
            collections: repo.model().collections.keys().cloned().collect(),
            repo,
            collection: 0,
            kind: SearchKind::Text,
            representation: Representation::Normalized,
            regex: false,
            case_sensitive: false,
            input: Input::default(),
            filter_input: Input::default(),
            export_input: Input::default(),
            export_return_focus: Focus::Results,
            focus: Focus::Query,
            hits: Vec::new(),
            query_fields: Vec::new(),
            sort_fields: Vec::new(),
            async_search: false,
            searching: false,
            search_generation: 0,
            pending_search: None,
            pending_detail: None,
            last_request: None,
            total: None,
            total_truncated: false,
            search_elapsed_ms: None,
            offset: 0,
            selected: 0,
            column_offset: 0,
            detail_scroll: 0,
            links: DetailLinks::default(),
            message: "Search all collections. Alt+S saves indexes; Alt+R reindexes all.".into(),
            index_status: None,
            reconciling: false,
            inventory_complete: false,
            pending_inventories: VecDeque::new(),
            reconciled_files: 0,
            pending_cache_action: None,
            queued_search: false,
            infer_all: false,
            infer_auto_index: true,
            infer_return_focus: Focus::Query,
            pending_inference: None,
            last_inference: None,
            pending_schema_write: false,
            inference_saved: false,
            done: false,
        }
    }

    pub(super) fn collection_label(&self) -> &str {
        if self.collection == 0 {
            "all collections"
        } else {
            &self.collections[self.collection - 1]
        }
    }

    pub(super) fn open_inference(&mut self) {
        self.infer_return_focus = self.focus;
        self.focus = Focus::Infer;
    }

    pub(super) fn close_inference(&mut self) {
        self.focus = self.infer_return_focus;
        if self.queued_search {
            self.search();
        }
    }

    fn scope(&self) -> Option<Vec<String>> {
        (self.collection > 0).then(|| vec![self.collections[self.collection - 1].clone()])
    }

    fn request(&self) -> Result<Request> {
        Ok(match self.kind {
            SearchKind::Filter => Request::Filter(JsonQuery {
                filter: self.input.text.clone(),
                collections: self.scope(),
                limit: PAGE_SIZE,
                offset: self.offset,
                count: true,
                backend: JsonQueryBackend::Auto,
                ..Default::default()
            }),
            SearchKind::Text | SearchKind::Combined => {
                let (pattern, filter, sort) = if self.kind == SearchKind::Combined {
                    let (filter, sort) = objekte_core::split_sort_clause(&self.filter_input.text)?;
                    (self.input.text.clone(), Some(filter), sort)
                } else {
                    let (pattern, sort) = objekte_core::split_sort_clause(&self.input.text)?;
                    let pattern =
                        if !sort.is_empty() && pattern.starts_with('"') && pattern.ends_with('"') {
                            serde_json::from_str::<String>(&pattern).map_err(|error| {
                                Error::Request(format!("invalid quoted search pattern: {error}"))
                            })?
                        } else {
                            pattern
                        };
                    (pattern, None, sort)
                };
                Request::Text(TextQuery {
                    pattern,
                    filter,
                    sort,
                    mode: match (self.regex, self.representation) {
                        (false, Representation::Normalized) => TextMode::Word,
                        (false, Representation::Full) => TextMode::WordRaw,
                        (true, Representation::Normalized) => TextMode::RegexClean,
                        (true, Representation::Full) => TextMode::RegexRaw,
                    },
                    // Regex patterns carry their own (?i) flag. The case toggle
                    // applies only to literal word searches.
                    case_sensitive: self.regex || self.case_sensitive,
                    collections: self.scope(),
                    limit: PAGE_SIZE,
                    offset: self.offset,
                    count: true,
                    ..Default::default()
                })
            }
        })
    }

    pub(super) fn search(&mut self) {
        if self.input.text.trim().is_empty() {
            self.message = "Enter a query first.".into();
            return;
        }
        if self.kind == SearchKind::Combined && self.filter_input.text.trim().is_empty() {
            self.message = "Enter a JSON filter first.".into();
            self.focus = Focus::FilterInput;
            return;
        }
        let request = match self.request() {
            Ok(request) => request,
            Err(error) => {
                self.message = error.to_string();
                self.focus = if self.kind == SearchKind::Combined {
                    Focus::FilterInput
                } else {
                    Focus::Query
                };
                return;
            }
        };
        if let Request::Text(text) = &request
            && let Some(filter) = &text.filter
            && let Err(error) = JsonFilter::compile(filter, &text.parameters)
        {
            self.message = error.to_string();
            self.focus = Focus::FilterInput;
            return;
        }
        if self.index_status.is_some() && !self.reconciling {
            self.queued_search = true;
            self.message = "Search queued; indexes are still building.".into();
            return;
        }
        self.queued_search = false;
        self.links.reset();
        self.search_generation = self.search_generation.wrapping_add(1);
        self.pending_search = None;
        self.pending_detail = None;
        self.searching = false;
        self.hits.clear();
        self.total = None;
        self.total_truncated = false;
        self.search_elapsed_ms = None;
        self.selected = 0;
        self.column_offset = 0;
        if self.async_search {
            self.query_fields = queried_fields(&request);
            self.sort_fields = ordered_fields(&request);
            self.last_request = Some(request.clone());
            self.pending_search = Some(request);
            self.searching = true;
            self.focus = Focus::Results;
            self.message.clear();
            return;
        }
        let result = match &request {
            Request::Filter(request) => self.repo.query_json(request).map(|result| {
                (
                    result.records.into_iter().map(Hit::Filter).collect(),
                    result.total,
                    result.truncated,
                    result.elapsed_ms,
                )
            }),
            Request::Text(request) => self.repo.search_text(request).map(|result| {
                (
                    result.hits.into_iter().map(Hit::Text).collect(),
                    result.total,
                    result.truncated,
                    result.elapsed_ms,
                )
            }),
        };
        match result {
            Ok((hits, total, truncated, elapsed_ms)) => {
                self.hits = hits;
                self.total = total;
                self.total_truncated = truncated;
                self.search_elapsed_ms = Some(elapsed_ms);
                self.query_fields = queried_fields(&request);
                self.sort_fields = ordered_fields(&request);
                self.last_request = Some(request);
                self.focus = Focus::Results;
                self.message.clear();
            }
            Err(error) => {
                self.last_request = None;
                self.message = error.to_string();
                self.focus = Focus::Query;
            }
        }
    }

    pub(super) fn can_next_page(&self) -> bool {
        self.last_request.is_some()
            && !self.hits.is_empty()
            && self.offset + self.hits.len() < self.total.unwrap_or(0)
    }

    pub(super) fn can_previous_page(&self) -> bool {
        self.offset > 0 && self.last_request.is_some()
    }

    pub(super) fn next_page(&mut self) {
        if self.searching {
            self.message = "Wait for the current search to finish before paging.".into();
            return;
        }
        if self.can_next_page() {
            self.offset += PAGE_SIZE;
            self.search();
        } else {
            self.message = if self.hits.is_empty() {
                "No result pages.".into()
            } else {
                "Already on the last page.".into()
            };
        }
    }

    pub(super) fn previous_page(&mut self) {
        if self.searching {
            self.message = "Wait for the current search to finish before paging.".into();
            return;
        }
        if self.can_previous_page() {
            self.offset = self.offset.saturating_sub(PAGE_SIZE);
            self.search();
        } else {
            self.message = if self.hits.is_empty() {
                "No result pages.".into()
            } else {
                "Already on the first page.".into()
            };
        }
    }

    pub(super) fn detail_position(&mut self) {
        // The detail header owns record position and identity. Reserve the
        // status line for errors and actions that need attention.
        self.message.clear();
    }

    pub(super) fn detail_neighbor(&mut self, forward: bool) {
        if self.links.override_hit.is_some() {
            self.message = "Backspace returns to the previous item.".into();
            return;
        }
        if self.searching {
            self.message = "Wait for the current page to finish loading.".into();
            return;
        }
        if self.hits.is_empty() {
            return;
        }
        if forward {
            if self.selected + 1 < self.hits.len() {
                self.selected += 1;
            } else if self.offset + self.hits.len() < self.total.unwrap_or(0) {
                self.next_page();
                if self.async_search {
                    self.pending_detail = Some(DetailEdge::First);
                    self.focus = Focus::Detail;
                    self.message = "Loading next result page…".into();
                    return;
                }
            } else {
                return;
            }
        } else if self.selected > 0 {
            self.selected -= 1;
        } else if self.offset > 0 {
            self.previous_page();
            if self.async_search {
                self.pending_detail = Some(DetailEdge::Last);
                self.focus = Focus::Detail;
                self.message = "Loading previous result page…".into();
                return;
            }
            self.selected = self.hits.len().saturating_sub(1);
        } else {
            return;
        }
        if !self.hits.is_empty() && matches!(self.focus, Focus::Results | Focus::Detail) {
            self.focus = Focus::Detail;
            self.detail_scroll = 0;
            self.detail_position();
            self.prepare_detail_links();
        }
    }

    pub(super) fn invalidate_results(&mut self) {
        self.links.reset();
        self.search_generation = self.search_generation.wrapping_add(1);
        self.hits.clear();
        self.query_fields.clear();
        self.sort_fields.clear();
        self.pending_search = None;
        self.pending_detail = None;
        self.searching = false;
        self.last_request = None;
        self.queued_search = false;
        self.total = None;
        self.total_truncated = false;
        self.search_elapsed_ms = None;
        self.offset = 0;
        self.selected = 0;
        self.column_offset = 0;
    }

    fn refresh_after_selection(&mut self, message: String) {
        let active = self.last_request.is_some() || self.queued_search;
        self.invalidate_results();
        if active {
            // Selection changes start a fresh page of the same input.
            self.search();
        } else {
            self.focus = Focus::Query;
            self.message = message;
        }
    }

    pub(super) fn cycle_collection(&mut self, backwards: bool) {
        let count = self.collections.len() + 1;
        self.collection = if backwards {
            (self.collection + count - 1) % count
        } else {
            (self.collection + 1) % count
        };
        self.refresh_after_selection(format!(
            "Collection: {}. Ctrl+Tab cycles; [ and ] work in results.",
            self.collection_label()
        ));
    }

    pub(super) fn toggle_mode(&mut self, backwards: bool) {
        self.kind = match (self.kind, backwards) {
            (SearchKind::Text, false) | (SearchKind::Combined, true) => {
                std::mem::swap(&mut self.input, &mut self.filter_input);
                SearchKind::Filter
            }
            (SearchKind::Filter, false) => {
                std::mem::swap(&mut self.input, &mut self.filter_input);
                SearchKind::Combined
            }
            (SearchKind::Filter, true) => {
                std::mem::swap(&mut self.input, &mut self.filter_input);
                SearchKind::Text
            }
            (SearchKind::Text, true) => SearchKind::Combined,
            (SearchKind::Combined, false) => SearchKind::Text,
        };
        self.invalidate_results();
        self.focus = Focus::Query;
        self.message = match self.kind {
            SearchKind::Filter => "Filter mode: compare JSON fields.".into(),
            SearchKind::Text => "Full-text mode: search included values.".into(),
            SearchKind::Combined => {
                "Combined mode: JSON filter first, then full-text. Ctrl+G switches inputs.".into()
            }
        };
    }

    pub(super) fn toggle_regex(&mut self) {
        if self.kind == SearchKind::Filter {
            return;
        }
        self.regex = !self.regex;
        let message = if self.regex {
            "Regex on; use inline flags such as (?i) for case-insensitive matching."
        } else {
            "Literal substring search on; Ctrl+E changes case sensitivity."
        };
        self.refresh_after_selection(message.into());
    }

    pub(super) fn toggle_representation(&mut self) {
        if self.kind == SearchKind::Filter {
            return;
        }
        self.representation = if self.representation == Representation::Normalized {
            Representation::Full
        } else {
            Representation::Normalized
        };
        let message = if self.representation == Representation::Normalized {
            "Normalized text: markup removed and whitespace collapsed."
        } else {
            "Full text: decoded values with markup intact."
        };
        self.refresh_after_selection(message.into());
    }

    pub(super) fn toggle_case(&mut self) {
        if self.kind == SearchKind::Filter || self.regex {
            return;
        }
        self.case_sensitive = !self.case_sensitive;
        let message = if self.case_sensitive {
            "Literal search is case-sensitive."
        } else {
            "Literal search is case-insensitive."
        };
        self.refresh_after_selection(message.into());
    }

    pub(super) fn start_export(&mut self, all: bool) {
        if self.searching {
            self.message = "Wait for the search to finish before exporting.".into();
            return;
        }
        if self.last_request.is_none() || (!all && self.hits.is_empty()) {
            self.message = "Run a query and select a hit before exporting.".into();
            return;
        }
        self.export_input = Input {
            text: if all {
                "objekte-results.json".into()
            } else {
                "objekte-hit.json".into()
            },
            cursor: 0,
        };
        self.export_input.cursor = self.export_input.text.chars().count();
        self.export_return_focus = self.focus;
        self.focus = if all {
            Focus::ExportAll
        } else {
            Focus::ExportOne
        };
    }
}


#[cfg(test)]
#[path = "state.test.rs"]
mod tests;
