//! Application state: query lines, the live search lifecycle, the result
//! list with a stable selection and viewport, and the detail view.

use super::complete::{self, Completion};
use super::input::Input;
use super::navigation::DetailLinks;
use super::warm::WarmMode;
use objekte_core::{
    CollectionInventory, InferenceReport, JsonFilter, JsonPathStep, JsonQuery, JsonQueryBackend,
    Record, RecordStatus, Repository, Sort, TextHit, TextMode, TextQuery,
};
use std::{
    cell::Cell,
    collections::VecDeque,
    time::{Duration, Instant},
};
use unicode_width::UnicodeWidthStr;

pub(super) const PAGE_SIZE: usize = 100;
/// Pause after the last keystroke before a live search starts.
pub(super) const DEBOUNCE: Duration = Duration::from_millis(180);
/// Load the next page when the selection is this close to the end.
const LOAD_AHEAD: usize = 30;
/// How long a notice stays in the footer.
pub(super) const NOTICE_TIME: Duration = Duration::from_secs(6);


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

    fn identity(&self) -> (String, String) {
        let record = self.record();
        (record.collection.clone(), record.id.clone())
    }
}


/// Request used for a result page and for complete export.
#[derive(Clone)]
pub(super) enum Request {
    Filter(JsonQuery),
    Text(TextQuery),
}


impl Request {
    pub(super) fn whole_result(&self) -> Self {
        self.page(0, i64::MAX as usize)
    }

    fn page(&self, offset: usize, limit: usize) -> Self {
        match self {
            Self::Filter(request) => Self::Filter(JsonQuery {
                offset,
                limit,
                count: true,
                ..request.clone()
            }),
            Self::Text(request) => Self::Text(TextQuery {
                offset,
                limit,
                count: true,
                ..request.clone()
            }),
        }
    }

    pub(super) fn sort(&self) -> &[Sort] {
        match self {
            Self::Filter(query) => &query.sort,
            Self::Text(query) => &query.sort,
        }
    }

    fn is_text(&self) -> bool {
        matches!(self, Self::Text(_))
    }
}


fn top_field(path: &[JsonPathStep]) -> Option<String> {
    match path.first() {
        Some(JsonPathStep::Key(name)) => Some(name.clone()),
        Some(JsonPathStep::Index(_) | JsonPathStep::Each) => Some("$value".into()),
        None => None,
    }
}

/// Top-level fields a filter reads, in query order.
fn queried_fields(request: &Request) -> Vec<String> {
    let (source, parameters) = match request {
        Request::Filter(query) => (&query.filter, &query.parameters),
        Request::Text(query) => match &query.filter {
            Some(filter) => (filter, &query.parameters),
            None => return Vec::new(),
        },
    };
    let mut fields = Vec::new();
    if let Ok(filter) = JsonFilter::compile(source, parameters) {
        for path in filter.paths_in_query_order() {
            if let Some(top) = top_field(&path)
                && !fields.contains(&top)
            {
                fields.push(top);
            }
        }
    }
    fields
}

/// Top-level fields of the sort terms, skipping system fields.
fn sorted_fields(request: &Request) -> Vec<String> {
    let mut fields = Vec::new();
    for term in request.sort() {
        if term.field.starts_with('@') {
            continue;
        }
        let field = term.field.strip_suffix(":fts").unwrap_or(&term.field);
        if let Ok(path) = JsonFilter::parse_path(field)
            && let Some(top) = top_field(&path)
            && !fields.contains(&top)
        {
            fields.push(top);
        }
    }
    fields
}


/// Which query line receives typing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Line {
    Search,
    Filter,
}


/// The main screen below the header.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum View {
    Results,
    Detail,
}


/// Which full-text representation a query inspects.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Representation {
    Normalized,
    Full,
}


/// Commands offered by the action palette.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Action {
    ChooseCollection,
    ExportAll,
    ExportRecord,
    TogglePreview,
    ToggleRegex,
    ToggleCase,
    ToggleText,
    InferFields,
    SaveIndexes,
    RebuildIndexes,
    Help,
    Quit,
}


impl Action {
    pub(super) const ALL: [Self; 12] = [
        Self::ChooseCollection,
        Self::ExportAll,
        Self::ExportRecord,
        Self::TogglePreview,
        Self::ToggleRegex,
        Self::ToggleCase,
        Self::ToggleText,
        Self::InferFields,
        Self::SaveIndexes,
        Self::RebuildIndexes,
        Self::Help,
        Self::Quit,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::ChooseCollection => "Choose collection…",
            Self::ExportAll => "Export all matches…",
            Self::ExportRecord => "Export selected record…",
            Self::TogglePreview => "Toggle preview pane",
            Self::ToggleRegex => "Toggle regex search",
            Self::ToggleCase => "Toggle case-sensitive search",
            Self::ToggleText => "Toggle normalized / full text",
            Self::InferFields => "Infer fields and index rules…",
            Self::SaveIndexes => "Save indexes to disk",
            Self::RebuildIndexes => "Rebuild all indexes and save",
            Self::Help => "Keyboard shortcuts",
            Self::Quit => "Quit",
        }
    }

    pub(super) fn shortcut(self) -> &'static str {
        match self {
            Self::ChooseCollection => "Ctrl+O",
            Self::TogglePreview => "Ctrl+P",
            Self::ToggleRegex => "Alt+R",
            Self::ToggleCase => "Alt+C",
            Self::ToggleText => "Alt+T",
            Self::Help => "F1",
            Self::Quit => "Ctrl+Q",
            _ => "",
        }
    }
}


/// A list dialog with a filter line: the collection picker or the palette.
#[derive(Default)]
pub(super) struct Chooser {
    pub(super) input: Input,
    pub(super) selected: usize,
}


/// A dialog drawn over the screen; it receives all keys.
pub(super) enum Overlay {
    Collections(Chooser),
    Palette(Chooser),
    Help,
    Export { all: bool, input: Input },
    Infer,
    InferReady,
}


/// A short message in the footer.
pub(super) struct Notice {
    pub(super) text: String,
    pub(super) error: bool,
    pub(super) at: Instant,
}


/// A result column: its source key and fixed display width.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) struct Column {
    pub(super) key: String,
    pub(super) width: u16,
}


/// Scope and schema choice for an explicit background inference scan.
pub(super) struct InferRequest {
    pub(super) collection: Option<String>,
    pub(super) auto_index: bool,
}


/// Long-lived repository handle and the interactive search.
pub(super) struct App<'a> {
    pub(super) repo: &'a mut Repository,
    pub(super) collections: Vec<String>,
    /// 0 means every collection; otherwise `collections[scope - 1]`.
    pub(super) scope: usize,
    pub(super) regex: bool,
    pub(super) case_sensitive: bool,
    pub(super) representation: Representation,
    pub(super) search: Input,
    pub(super) filter: Input,
    pub(super) line: Line,
    pub(super) completion: Option<Completion>,
    pub(super) completion_selected: usize,
    pub(super) view: View,
    pub(super) overlay: Option<Overlay>,
    pub(super) show_preview: bool,
    pub(super) notice: Option<Notice>,
    /// The line and message of the last query that could not be parsed.
    pub(super) query_error: Option<(Line, String)>,
    /// The error came from Enter rather than from pausing while typing, so
    /// it is shown as an error instead of a quiet hint.
    pub(super) error_confirmed: bool,

    pub(super) hits: Vec<Hit>,
    pub(super) columns: Vec<Column>,
    pub(super) column_offset: usize,
    pub(super) selected: usize,
    /// First visible result row.
    pub(super) scroll: usize,
    /// Result rows the last frame could show; the renderer updates it.
    pub(super) visible_rows: Cell<usize>,
    pub(super) total: Option<usize>,
    pub(super) truncated: bool,
    pub(super) elapsed_ms: Option<f64>,
    /// Base request at offset 0 for the shown results.
    pub(super) last_request: Option<Request>,
    pub(super) searching: bool,
    pub(super) generation: u64,
    pub(super) pending_search: Option<Request>,
    /// The next update replaces the shown results.
    fresh: bool,
    /// Rows from this index on belong to the page being loaded.
    append_base: Option<usize>,
    /// Restore this record and its screen row when results are replaced.
    restore: Option<((String, String), usize)>,
    /// Advance the detail view once the next page arrives.
    pending_next: bool,
    /// When the query lines last changed without a search.
    pub(super) dirty: Option<Instant>,
    pub(super) queued_search: bool,
    /// Synchronous search for tests; the TUI runs a background worker.
    pub(super) async_search: bool,

    pub(super) detail_scroll: usize,
    pub(super) links: DetailLinks,

    pub(super) index_status: Option<String>,
    pub(super) reconciling: bool,
    pub(super) inventory_complete: bool,
    pub(super) pending_inventories: VecDeque<CollectionInventory>,
    pub(super) reconciled_files: usize,
    pub(super) pending_cache_action: Option<WarmMode>,
    pub(super) infer_all: bool,
    pub(super) infer_auto_index: bool,
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
            scope: 0,
            regex: false,
            case_sensitive: false,
            representation: Representation::Normalized,
            search: Input::default(),
            filter: Input::default(),
            line: Line::Search,
            completion: None,
            completion_selected: 0,
            view: View::Results,
            overlay: None,
            show_preview: true,
            notice: None,
            query_error: None,
            error_confirmed: false,
            hits: Vec::new(),
            columns: Vec::new(),
            column_offset: 0,
            selected: 0,
            scroll: 0,
            visible_rows: Cell::new(20),
            total: None,
            truncated: false,
            elapsed_ms: None,
            last_request: None,
            searching: false,
            generation: 0,
            pending_search: None,
            fresh: false,
            append_base: None,
            restore: None,
            pending_next: false,
            dirty: None,
            queued_search: false,
            async_search: false,
            detail_scroll: 0,
            links: DetailLinks::default(),
            index_status: None,
            reconciling: false,
            inventory_complete: false,
            pending_inventories: VecDeque::new(),
            reconciled_files: 0,
            pending_cache_action: None,
            infer_all: false,
            infer_auto_index: true,
            pending_inference: None,
            last_inference: None,
            pending_schema_write: false,
            inference_saved: false,
            done: false,
        }
    }

    pub(super) fn scope_name(&self) -> Option<&str> {
        (self.scope > 0).then(|| self.collections[self.scope - 1].as_str())
    }

    pub(super) fn scope_label(&self) -> &str {
        self.scope_name().unwrap_or("all collections")
    }

    pub(super) fn notice(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice {
            text: text.into(),
            error: false,
            at: Instant::now(),
        });
    }

    pub(super) fn error(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice {
            text: text.into(),
            error: true,
            at: Instant::now(),
        });
    }

    pub(super) fn active_input(&mut self) -> &mut Input {
        match self.line {
            Line::Search => &mut self.search,
            Line::Filter => &mut self.filter,
        }
    }

    pub(super) fn indexes_busy(&self) -> bool {
        self.index_status.is_some() && !self.reconciling
    }

    // ----- query lines ----------------------------------------------------

    /// The query lines changed: search again shortly.
    pub(super) fn edited(&mut self) {
        self.dirty = Some(Instant::now());
        if self.completion.is_some() {
            self.refresh_completion(false);
        }
    }

    /// Run a debounced search once typing pauses.
    pub(super) fn tick(&mut self, now: Instant) {
        if self
            .dirty
            .is_some_and(|at| now.duration_since(at) >= DEBOUNCE)
        {
            self.search_as_typed(false);
        }
        if self
            .notice
            .as_ref()
            .is_some_and(|notice| now.duration_since(notice.at) >= NOTICE_TIME)
        {
            self.notice = None;
        }
    }

    /// Open or update completion for the filter line. With `insert`, a
    /// single candidate or a shared prefix is inserted immediately.
    pub(super) fn refresh_completion(&mut self, insert: bool) {
        let schema = complete::Schema::new(&self.repo.model().collections, self.scope_name());
        let found = complete::complete(&self.filter.text, self.filter.cursor, &schema);
        let Some(found) = found else {
            self.completion = None;
            return;
        };
        if insert {
            let typed = self
                .filter
                .text
                .chars()
                .skip(found.start)
                .take(self.filter.cursor.saturating_sub(found.start))
                .collect::<String>();
            // Substring matches only show in the list; Tab inserts when the
            // typed text leaves a single candidate that starts with it.
            let leading = found
                .items
                .iter()
                .filter(|item| starts_with_ignore_case(&item.insert, &typed))
                .collect::<Vec<_>>();
            if found.items.len() == 1 || (!typed.is_empty() && leading.len() == 1) {
                let item = leading.first().copied().unwrap_or(&found.items[0]);
                let insert = item.insert.clone();
                self.filter.replace(found.start, found.end, &insert);
                self.completion = None;
                self.edited();
                if insert.ends_with(['.', '(']) {
                    self.refresh_completion(false);
                }
                return;
            }
            // Like a shell, extend to the longest shared prefix first.
            let prefix = shared_prefix(leading.iter().map(|item| item.insert.as_str()));
            if leading.len() > 1 && prefix.chars().count() > typed.chars().count() {
                self.filter
                    .replace(found.start, self.filter.cursor, &prefix);
                self.dirty = Some(Instant::now());
                return self.refresh_completion(false);
            }
        }
        if self
            .completion
            .as_ref()
            .is_none_or(|open| open.items != found.items)
        {
            self.completion_selected = 0;
        }
        self.completion = Some(found);
    }

    pub(super) fn accept_completion(&mut self) {
        let Some(found) = self.completion.take() else {
            return;
        };
        let Some(item) = found.items.get(self.completion_selected) else {
            return;
        };
        self.filter
            .replace(found.start, found.end, &item.insert.clone());
        self.edited();
        // Continue into a nested field or a value without another keypress.
        if item.insert.ends_with(['.', '(']) {
            self.refresh_completion(false);
        }
    }

    // ----- requests -------------------------------------------------------

    fn request(&self) -> std::result::Result<Option<Request>, (Line, String)> {
        let (pattern, text_sort) = objekte_core::split_sort_clause(&self.search.text)
            .map_err(|error| (Line::Search, error.to_string()))?;
        let pattern = if !text_sort.is_empty() && pattern.starts_with('"') && pattern.ends_with('"')
        {
            serde_json::from_str::<String>(&pattern).map_err(|error| {
                (
                    Line::Search,
                    format!("invalid quoted search pattern: {error}"),
                )
            })?
        } else {
            pattern
        };
        let (filter, filter_sort) = objekte_core::split_sort_clause(&self.filter.text)
            .map_err(|error| (Line::Filter, error.to_string()))?;
        if !filter.trim().is_empty()
            && let Err(error) = JsonFilter::compile(&filter, &Default::default())
        {
            return Err((Line::Filter, error.to_string()));
        }
        let sort = if text_sort.is_empty() {
            filter_sort
        } else {
            text_sort
        };
        if pattern.trim().is_empty() && filter.trim().is_empty() {
            return Ok(None);
        }
        let collections = self.scope_name().map(|name| vec![name.to_owned()]);
        Ok(Some(if pattern.trim().is_empty() {
            Request::Filter(JsonQuery {
                filter,
                sort,
                collections,
                limit: PAGE_SIZE,
                count: true,
                backend: JsonQueryBackend::Auto,
                ..Default::default()
            })
        } else {
            Request::Text(TextQuery {
                pattern,
                filter: (!filter.trim().is_empty()).then_some(filter),
                sort,
                mode: match (self.regex, self.representation) {
                    (false, Representation::Normalized) => TextMode::Word,
                    (false, Representation::Full) => TextMode::WordRaw,
                    (true, Representation::Normalized) => TextMode::RegexClean,
                    (true, Representation::Full) => TextMode::RegexRaw,
                },
                // Regex patterns carry their own (?i) flag.
                case_sensitive: self.regex || self.case_sensitive,
                collections,
                limit: PAGE_SIZE,
                count: true,
                ..Default::default()
            })
        }))
    }

    // ----- search lifecycle -----------------------------------------------

    /// Start a new search for the current lines from the first row.
    pub(super) fn search(&mut self) {
        self.search_as_typed(true);
    }

    /// Search for the current lines. Unless `confirmed` (Enter), a query
    /// that does not compile yet is most likely still being typed.
    fn search_as_typed(&mut self, confirmed: bool) {
        self.dirty = None;
        let request = match self.request() {
            Ok(request) => request,
            Err(error) => {
                // Keep the previous results; the summary shows the error.
                self.query_error = Some(error);
                self.error_confirmed = confirmed;
                return;
            }
        };
        self.query_error = None;
        let Some(request) = request else {
            self.clear_results();
            return;
        };
        if self.indexes_busy() {
            self.queued_search = true;
            return;
        }
        self.queued_search = false;
        self.restore = None;
        self.begin(request, true);
    }

    /// Run the shown search again, keeping the selected record in place.
    pub(super) fn rerun(&mut self) {
        let Some(request) = self.last_request.clone() else {
            return;
        };
        self.restore = self
            .hits
            .get(self.selected)
            .map(|hit| (hit.identity(), self.selected.saturating_sub(self.scroll)));
        self.begin(request, false);
    }

    fn begin(&mut self, request: Request, reset_view: bool) {
        self.generation = self.generation.wrapping_add(1);
        self.append_base = None;
        self.pending_next = false;
        if reset_view {
            self.column_offset = 0;
        }
        self.last_request = Some(request.clone());
        self.links.generation = self.links.generation.wrapping_add(1);
        // Informational notices are stale once the user searches.
        if self.notice.as_ref().is_some_and(|notice| !notice.error) {
            self.notice = None;
        }
        if !self.async_search {
            self.fresh = true;
            self.run_now(request);
            return;
        }
        self.fresh = true;
        self.searching = true;
        self.pending_search = Some(request);
    }

    fn clear_results(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.pending_search = None;
        self.searching = false;
        self.last_request = None;
        self.hits.clear();
        self.columns.clear();
        self.total = None;
        self.truncated = false;
        self.elapsed_ms = None;
        self.selected = 0;
        self.scroll = 0;
        self.column_offset = 0;
        self.append_base = None;
        self.queued_search = false;
        if self.view == View::Detail {
            self.close_detail();
        }
    }

    fn run_now(&mut self, request: Request) {
        let result = match &request {
            Request::Filter(query) => self.repo.query_json(query).map(|result| {
                (
                    result
                        .records
                        .into_iter()
                        .map(Hit::Filter)
                        .collect::<Vec<_>>(),
                    result.total,
                    result.truncated,
                    result.elapsed_ms,
                )
            }),
            Request::Text(query) => self.repo.search_text(query).map(|result| {
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
                self.receive_page(hits);
                self.finish_search(Ok((total, truncated, elapsed_ms)));
            }
            Err(error) => self.finish_search(Err(error.to_string())),
        }
    }

    /// Load the next page when the selection nears the end of the rows.
    pub(super) fn load_more(&mut self) {
        let Some(request) = &self.last_request else {
            return;
        };
        let more = self.total.is_some_and(|total| self.hits.len() < total);
        if self.searching || !more || self.selected + LOAD_AHEAD < self.hits.len() {
            return;
        }
        let page = request.page(self.hits.len(), PAGE_SIZE);
        self.generation = self.generation.wrapping_add(1);
        self.append_base = Some(self.hits.len());
        self.fresh = false;
        if !self.async_search {
            self.run_now(page);
            return;
        }
        self.searching = true;
        self.pending_search = Some(page);
    }

    fn take_fresh(&mut self) {
        if !self.fresh {
            return;
        }
        self.fresh = false;
        self.hits.clear();
        self.columns.clear();
        self.selected = 0;
        self.scroll = 0;
    }

    /// One streamed row in result order.
    pub(super) fn receive_hit(&mut self, hit: Hit) {
        self.take_fresh();
        self.hits.push(hit);
        self.update_columns();
        self.apply_restore();
    }

    /// A provisional sorted page that replaces the rows of its request.
    pub(super) fn receive_page(&mut self, page: Vec<Hit>) {
        let fresh = self.fresh;
        self.take_fresh();
        let anchor = (!fresh)
            .then(|| self.hits.get(self.selected).map(Hit::identity))
            .flatten()
            .map(|identity| (identity, self.selected.saturating_sub(self.scroll)));
        let base = self.append_base.unwrap_or(0);
        self.hits.truncate(base);
        self.hits.extend(page);
        self.update_columns();
        if let Some(anchor) = anchor {
            self.restore = Some(anchor);
        }
        self.apply_restore();
        self.selected = self.selected.min(self.hits.len().saturating_sub(1));
        self.keep_visible();
    }

    /// Put a remembered record back at its screen row if it is present.
    fn apply_restore(&mut self) {
        let Some((identity, row)) = &self.restore else {
            return;
        };
        if let Some(index) = self.hits.iter().position(|hit| &hit.identity() == identity) {
            self.selected = index;
            self.scroll = index.saturating_sub(*row);
            self.keep_visible();
        }
    }

    pub(super) fn finish_search(
        &mut self,
        result: std::result::Result<(Option<usize>, bool, f64), String>,
    ) {
        self.searching = false;
        let appending = self.append_base.take().is_some();
        match result {
            Ok((total, truncated, elapsed_ms)) => {
                if self.fresh {
                    // The search finished without any rows.
                    self.take_fresh();
                }
                self.restore = None;
                self.total = total;
                self.truncated = truncated;
                if !appending {
                    self.elapsed_ms = Some(elapsed_ms);
                }
                self.selected = self.selected.min(self.hits.len().saturating_sub(1));
                self.keep_visible();
                if std::mem::take(&mut self.pending_next) {
                    self.detail_step(true);
                }
            }
            Err(error) => {
                self.restore = None;
                self.pending_next = false;
                if !appending {
                    // Streamed rows are provisional until the query succeeds.
                    self.fresh = false;
                    self.hits.clear();
                    self.columns.clear();
                    self.total = None;
                    self.elapsed_ms = None;
                    self.last_request = None;
                    if self.view == View::Detail {
                        self.close_detail();
                    }
                }
                self.error(error);
            }
        }
    }

    // ----- result columns -------------------------------------------------

    /// Add columns for new keys. Existing columns never move or resize.
    fn update_columns(&mut self) {
        let Some(request) = &self.last_request else {
            return;
        };
        let mut wanted = Vec::new();
        if self.scope == 0 {
            wanted.push("@collection".to_string());
        }
        wanted.push("@id".into());
        wanted.push("@status".into());
        if request.is_text() {
            wanted.push("@match".into());
        }
        wanted.extend(queried_fields(request));
        wanted.extend(sorted_fields(request));
        for hit in self.hits.iter().take(PAGE_SIZE) {
            let record = hit.record();
            match &record.data {
                serde_json::Value::Object(map) => wanted.extend(map.keys().cloned()),
                _ if record.raw.is_none() => wanted.push("$value".into()),
                _ => {}
            }
        }
        for key in wanted {
            if self.columns.iter().any(|column| column.key == key) {
                continue;
            }
            let width = column_width(&key, &self.hits);
            self.columns.push(Column { key, width });
        }
    }

    // ----- selection and viewport -----------------------------------------

    /// Keep the selection inside the viewport, scrolling as little as possible.
    pub(super) fn keep_visible(&mut self) {
        let rows = self.visible_rows.get().max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + rows {
            self.scroll = self.selected + 1 - rows;
        }
        let max_scroll = self.hits.len().saturating_sub(rows);
        self.scroll = self.scroll.min(max_scroll);
    }

    pub(super) fn move_selection(&mut self, delta: isize) {
        if self.hits.is_empty() {
            return;
        }
        let last = self.hits.len() - 1;
        self.selected = self.selected.saturating_add_signed(delta).min(last);
        self.keep_visible();
        self.load_more();
    }

    pub(super) fn select_first(&mut self) {
        self.selected = 0;
        self.keep_visible();
    }

    pub(super) fn select_last(&mut self) {
        self.selected = self.hits.len().saturating_sub(1);
        self.keep_visible();
        self.load_more();
    }

    pub(super) fn scroll_columns(&mut self, forward: bool) {
        let fixed = self.fixed_columns();
        let movable = self.columns.len().saturating_sub(fixed);
        self.column_offset = if forward {
            (self.column_offset + 1).min(movable.saturating_sub(1))
        } else {
            self.column_offset.saturating_sub(1)
        };
    }

    /// Identity columns stay put while the others scroll sideways.
    pub(super) fn fixed_columns(&self) -> usize {
        self.columns
            .iter()
            .take_while(|column| matches!(column.key.as_str(), "@collection" | "@id" | "@status"))
            .count()
    }

    pub(super) fn selected_hit(&self) -> Option<&Hit> {
        self.hits.get(self.selected)
    }

    // ----- options --------------------------------------------------------

    fn options_changed(&mut self, text: &str) {
        self.notice(text);
        if self.last_request.is_some() || self.dirty.is_some() || self.queued_search {
            self.search();
        }
    }

    pub(super) fn set_scope(&mut self, scope: usize) {
        if scope == self.scope {
            return;
        }
        self.scope = scope;
        let text = format!("Searching {}", self.scope_label());
        self.options_changed(&text);
    }

    pub(super) fn toggle_regex(&mut self) {
        self.regex = !self.regex;
        self.options_changed(if self.regex {
            "Regex on · use (?i) for case-insensitive patterns"
        } else {
            "Regex off · literal substring search"
        });
    }

    pub(super) fn toggle_case(&mut self) {
        if self.regex {
            self.notice("Regex patterns set case with (?i)");
            return;
        }
        self.case_sensitive = !self.case_sensitive;
        self.options_changed(if self.case_sensitive {
            "Case-sensitive search"
        } else {
            "Case-insensitive search"
        });
    }

    pub(super) fn toggle_representation(&mut self) {
        self.representation = match self.representation {
            Representation::Normalized => Representation::Full,
            Representation::Full => Representation::Normalized,
        };
        self.options_changed(match self.representation {
            Representation::Normalized => "Normalized text · markup removed",
            Representation::Full => "Full text · markup kept",
        });
    }

    // ----- detail view ----------------------------------------------------

    pub(super) fn open_detail(&mut self) {
        if self.hits.is_empty() {
            return;
        }
        self.links.reset();
        self.view = View::Detail;
        self.detail_scroll = 0;
        self.completion = None;
        self.prepare_detail_links();
    }

    pub(super) fn close_detail(&mut self) {
        self.links.reset();
        self.view = View::Results;
        self.pending_next = false;
    }

    /// Show the previous or next result in the detail view.
    pub(super) fn detail_step(&mut self, forward: bool) {
        if self.links.override_hit.is_some() {
            self.notice("Backspace returns to the previous record");
            return;
        }
        if forward {
            if self.selected + 1 < self.hits.len() {
                self.selected += 1;
            } else if self.searching || self.total.is_some_and(|total| self.hits.len() < total) {
                self.pending_next = true;
                self.load_more();
                return;
            } else {
                return;
            }
        } else if self.selected > 0 {
            self.selected -= 1;
        } else {
            return;
        }
        self.keep_visible();
        self.load_more();
        self.detail_scroll = 0;
        self.links.reset();
        self.prepare_detail_links();
    }

    pub(super) fn scroll_detail(&mut self, delta: isize) {
        self.detail_scroll = self.detail_scroll.saturating_add_signed(delta);
    }

    // ----- dialogs --------------------------------------------------------

    pub(super) fn open_export(&mut self, all: bool) {
        if self.searching && all {
            self.notice("Wait for the search to finish before exporting");
            return;
        }
        let has_record = self.view == View::Detail || self.selected_hit().is_some();
        if self.last_request.is_none() && self.links.override_hit.is_none() || !all && !has_record {
            self.notice("Search and select a record before exporting");
            return;
        }
        let name = if all {
            "objekte-results.json"
        } else {
            "objekte-record.json"
        };
        self.overlay = Some(Overlay::Export {
            all,
            input: Input::with_text(name),
        });
    }

    pub(super) fn open_inference(&mut self) {
        self.overlay = Some(Overlay::Infer);
    }

    pub(super) fn queue_inference(&mut self) {
        self.overlay = None;
        if self.index_status.is_some() {
            self.notice("Index work is running; inference starts afterwards");
        }
        self.pending_inference = Some(InferRequest {
            collection: (!self.infer_all)
                .then(|| self.scope_name().map(str::to_owned))
                .flatten(),
            auto_index: self.infer_auto_index,
        });
    }

    pub(super) fn queue_cache_action(&mut self, mode: WarmMode) {
        if self.index_status.is_some() {
            self.notice("Index work is already running");
            return;
        }
        self.queued_search = self.last_request.is_some() || self.dirty.is_some();
        self.pending_cache_action = Some(mode);
    }

    /// Run an action chosen from the palette.
    pub(super) fn run_action(&mut self, action: Action) {
        self.overlay = None;
        match action {
            Action::ChooseCollection => {
                self.overlay = Some(Overlay::Collections(Chooser {
                    selected: self.scope,
                    ..Default::default()
                }))
            }
            Action::ExportAll => self.open_export(true),
            Action::ExportRecord => self.open_export(false),
            Action::TogglePreview => self.show_preview = !self.show_preview,
            Action::ToggleRegex => self.toggle_regex(),
            Action::ToggleCase => self.toggle_case(),
            Action::ToggleText => self.toggle_representation(),
            Action::InferFields => self.open_inference(),
            Action::SaveIndexes => self.queue_cache_action(WarmMode::Save),
            Action::RebuildIndexes => self.queue_cache_action(WarmMode::Reindex),
            Action::Help => self.overlay = Some(Overlay::Help),
            Action::Quit => self.done = true,
        }
    }

    /// Palette actions matching the typed filter.
    pub(super) fn palette_actions(&self, filter: &str) -> Vec<Action> {
        let filter = filter.to_lowercase();
        Action::ALL
            .into_iter()
            .filter(|action| action.label().to_lowercase().contains(&filter))
            .collect()
    }

    /// Collection choices matching the typed filter, as scope indices.
    pub(super) fn collection_choices(&self, filter: &str) -> Vec<usize> {
        let filter = filter.to_lowercase();
        (0..=self.collections.len())
            .filter(|&scope| {
                let name = if scope == 0 {
                    "all collections"
                } else {
                    &self.collections[scope - 1]
                };
                name.to_lowercase().contains(&filter)
            })
            .collect()
    }
}


/// Width for a new column: its header and the values seen so far.
fn column_width(key: &str, hits: &[Hit]) -> u16 {
    let (min, max) = match key {
        "@status" => return 1,
        "@match" => (24, 56),
        "@id" | "@collection" => (6, 24),
        _ => (6, 26),
    };
    let header = key.trim_start_matches('@').width();
    let content = hits
        .iter()
        .take(PAGE_SIZE)
        .map(|hit| super::view::results::cell_text(hit, key).width())
        .max()
        .unwrap_or(0);
    header.max(content).clamp(min, max) as u16
}

fn shared_prefix<'s>(mut items: impl Iterator<Item = &'s str>) -> String {
    let Some(first) = items.next() else {
        return String::new();
    };
    let mut prefix = first.to_string();
    for item in items {
        let common = prefix
            .chars()
            .zip(item.chars())
            .take_while(|(a, b)| a == b)
            .count();
        prefix = prefix.chars().take(common).collect();
    }
    prefix
}

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.to_lowercase().starts_with(&prefix.to_lowercase())
}

pub(super) fn status_name(status: RecordStatus) -> &'static str {
    match status {
        RecordStatus::Valid => "valid",
        RecordStatus::Invalid => "invalid",
        RecordStatus::IncorrectJson => "malformed JSON",
        RecordStatus::NoSchema => "no schema",
        RecordStatus::Pending => "not validated",
    }
}


#[cfg(test)]
#[path = "app.test.rs"]
mod tests;
