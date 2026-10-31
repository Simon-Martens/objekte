//! Search state, keyboard editing, and page navigation for the terminal UI.

use objekte_core::{JsonQuery, JsonQueryBackend, Record, Repository, TextHit, TextMode, TextQuery};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const PAGE_SIZE: usize = 100;


/// Whether the input is a field filter or a standalone full-text search.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SearchKind {
    Filter,
    Text,
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
    Results,
    Detail,
    ExportAll,
    ExportOne,
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
    pub(super) export_input: Input,
    pub(super) export_return_focus: Focus,
    pub(super) focus: Focus,
    pub(super) hits: Vec<Hit>,
    pub(super) last_request: Option<Request>,
    pub(super) total: Option<usize>,
    pub(super) offset: usize,
    pub(super) selected: usize,
    pub(super) column_offset: usize,
    pub(super) detail_scroll: u16,
    pub(super) message: String,
    pub(super) done: bool,
}


impl<'a> App<'a> {
    pub(super) fn new(repo: &'a mut Repository) -> Self {
        Self {
            collections: repo.model().collections.keys().cloned().collect(),
            repo,
            collection: 0,
            kind: SearchKind::Filter,
            representation: Representation::Normalized,
            regex: false,
            case_sensitive: false,
            input: Input::default(),
            export_input: Input::default(),
            export_return_focus: Focus::Results,
            focus: Focus::Query,
            hits: Vec::new(),
            last_request: None,
            total: None,
            offset: 0,
            selected: 0,
            column_offset: 0,
            detail_scroll: 0,
            message: "Enter a filter and press Enter. Tab switches search mode.".into(),
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

    fn scope(&self) -> Option<Vec<String>> {
        (self.collection > 0).then(|| vec![self.collections[self.collection - 1].clone()])
    }

    fn request(&self) -> Request {
        match self.kind {
            SearchKind::Filter => Request::Filter(JsonQuery {
                filter: self.input.text.clone(),
                collections: self.scope(),
                limit: PAGE_SIZE,
                offset: self.offset,
                count: true,
                backend: JsonQueryBackend::Auto,
                ..Default::default()
            }),
            SearchKind::Text => Request::Text(TextQuery {
                pattern: self.input.text.clone(),
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
            }),
        }
    }

    pub(super) fn search(&mut self) {
        if self.input.text.trim().is_empty() {
            self.message = "Enter a query first.".into();
            return;
        }
        self.hits.clear();
        self.total = None;
        self.selected = 0;
        self.column_offset = 0;
        let request = self.request();
        let result = match &request {
            Request::Filter(request) => self.repo.query_json(request).map(|result| {
                (
                    result.records.into_iter().map(Hit::Filter).collect(),
                    result.total,
                )
            }),
            Request::Text(request) => self.repo.search_text(request).map(|result| {
                (
                    result.hits.into_iter().map(Hit::Text).collect(),
                    result.total,
                )
            }),
        };
        match result {
            Ok((hits, total)) => {
                self.hits = hits;
                self.total = total;
                self.last_request = Some(request);
                self.focus = Focus::Results;
                self.message = format!(
                    "{} matches; showing {}–{}.",
                    total.unwrap_or(self.hits.len()),
                    if self.hits.is_empty() {
                        0
                    } else {
                        self.offset + 1
                    },
                    self.offset + self.hits.len()
                );
            }
            Err(error) => {
                self.last_request = None;
                self.message = error.to_string();
                self.focus = Focus::Query;
            }
        }
    }

    pub(super) fn next_page(&mut self) {
        if self.offset + self.hits.len() < self.total.unwrap_or(0) {
            self.offset += PAGE_SIZE;
            self.search();
        }
    }

    pub(super) fn previous_page(&mut self) {
        if self.offset > 0 {
            self.offset = self.offset.saturating_sub(PAGE_SIZE);
            self.search();
        }
    }

    pub(super) fn detail_position(&mut self) {
        self.message = format!(
            "Result {} of {}. Left/Right or p/n flips results.",
            self.offset + self.selected + 1,
            self.total.unwrap_or(self.offset + self.hits.len())
        );
    }

    pub(super) fn detail_neighbor(&mut self, forward: bool) {
        if self.hits.is_empty() {
            return;
        }
        if forward {
            if self.selected + 1 < self.hits.len() {
                self.selected += 1;
            } else if self.offset + self.hits.len() < self.total.unwrap_or(0) {
                self.next_page();
            } else {
                return;
            }
        } else if self.selected > 0 {
            self.selected -= 1;
        } else if self.offset > 0 {
            self.previous_page();
            self.selected = self.hits.len().saturating_sub(1);
        } else {
            return;
        }
        if !self.hits.is_empty() && matches!(self.focus, Focus::Results | Focus::Detail) {
            self.focus = Focus::Detail;
            self.detail_scroll = 0;
            self.detail_position();
        }
    }

    pub(super) fn invalidate_results(&mut self) {
        self.hits.clear();
        self.last_request = None;
        self.total = None;
        self.offset = 0;
        self.selected = 0;
        self.column_offset = 0;
    }

    pub(super) fn cycle_collection(&mut self, backwards: bool) {
        let count = self.collections.len() + 1;
        self.collection = if backwards {
            (self.collection + count - 1) % count
        } else {
            (self.collection + 1) % count
        };
        self.invalidate_results();
        self.message = format!(
            "Collection: {}. Ctrl+Tab cycles; [ and ] work in results.",
            self.collection_label()
        );
    }

    pub(super) fn toggle_mode(&mut self) {
        self.kind = if self.kind == SearchKind::Filter {
            SearchKind::Text
        } else {
            SearchKind::Filter
        };
        self.invalidate_results();
        self.focus = Focus::Query;
        self.message = if self.kind == SearchKind::Filter {
            "Filter mode: compare JSON fields.".into()
        } else {
            "Full-text mode: search included values.".into()
        };
    }

    pub(super) fn toggle_regex(&mut self) {
        if self.kind != SearchKind::Text {
            return;
        }
        self.regex = !self.regex;
        self.invalidate_results();
        self.focus = Focus::Query;
        self.message = if self.regex {
            "Regex on; use inline flags such as (?i) for case-insensitive matching.".into()
        } else {
            "Word search on; Ctrl+E changes case sensitivity.".into()
        };
    }

    pub(super) fn toggle_representation(&mut self) {
        if self.kind != SearchKind::Text {
            return;
        }
        self.representation = if self.representation == Representation::Normalized {
            Representation::Full
        } else {
            Representation::Normalized
        };
        self.invalidate_results();
        self.focus = Focus::Query;
        self.message = if self.representation == Representation::Normalized {
            "Normalized text: markup removed and whitespace collapsed.".into()
        } else {
            "Full text: decoded values with markup intact.".into()
        };
    }

    pub(super) fn toggle_case(&mut self) {
        if self.kind != SearchKind::Text || self.regex {
            return;
        }
        self.case_sensitive = !self.case_sensitive;
        self.invalidate_results();
        self.focus = Focus::Query;
        self.message = if self.case_sensitive {
            "Word search is case-sensitive.".into()
        } else {
            "Word search is case-insensitive.".into()
        };
    }

    pub(super) fn start_export(&mut self, all: bool) {
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
