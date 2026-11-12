//! Search and selection state shared by the event loop and renderer.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use objekte_search_core::lifecycle_caller::IndexedRepository;
use objekte_search_core::lifecycle::ExtendedPhase;
use objekte_search_core::model::{FieldPath, LogicalType, PathRoot, Value};
use objekte_search_core::query::{
    CaseSensitivity, FilterExpr, FtsMode, FtsQuery, Modifier, Operand, Query, QueryOptions,
    RegexOptions, parse_filter, parse_structured_filter,
};
use objekte_search_core::query_path::QueryPath;
use objekte_search_core::repository::{SearchHit, SearchResult};
use objekte_search_core::streaming::{
    CancellationToken, StreamEvent, StreamMode, StreamOptions, StreamOutcome,
};
use objekte_search_core::workload::{ReturnMode, SortField};
#[cfg(test)]
use objekte_search_core::{lifecycle::Index, repository::Repository};
use ratatui::layout::Rect;
use unicode_casefold::{Locale, UnicodeCaseFold, Variant};

use super::TuiOptions;
use super::export;
use super::input::Input;
use super::record::{self, DisplayRow, RecordState};
use super::relations::{self, LinkAction, RelationRow};
use super::settings::{self, Settings};

pub(super) const PAGE_SIZE: usize = 100;
pub(super) const DEBOUNCE: Duration = Duration::from_millis(180);

fn fold_find(text: &str) -> String {
    text.case_fold_with(Variant::Full, Locale::NonTurkic)
        .collect()
}

fn push_unique(fields: &mut Vec<String>, name: String) {
    if !name.is_empty() && !fields.contains(&name) {
        fields.push(name);
    }
}

fn collect_filter_fields(filter: &FilterExpr, fields: &mut Vec<String>) {
    match filter {
        FilterExpr::Term(term) => {
            push_unique(fields, field_label(&term.path.display, &term.modifiers));
            if let Operand::Field(field) = &term.operand {
                push_unique(fields, field_label(&field.path.display, &field.modifiers));
            }
        }
        FilterExpr::And(parts) | FilterExpr::Or(parts) => {
            for part in parts {
                collect_filter_fields(part, fields);
            }
        }
        FilterExpr::Not(inner) => collect_filter_fields(inner, fields),
    }
}

fn field_label(path: &str, modifiers: &[Modifier]) -> String {
    if path == "@record.id" {
        return String::new();
    }
    let mut label = if path == "id" {
        "@root.id".to_owned()
    } else if path.starts_with('@') {
        path.to_owned()
    } else {
        path.to_owned()
    };
    for modifier in modifiers {
        let name = match modifier {
            Modifier::Lower => "lower",
            Modifier::Length => "length",
            Modifier::Each => "each",
            Modifier::Null => "null",
            Modifier::Exists => "exists",
            Modifier::NonEmpty => "?",
            Modifier::StringType => "string",
            Modifier::NumberType => "number",
            Modifier::ArrayType => "array",
            Modifier::ObjectType => "object",
            Modifier::Custom(name) => name,
        };
        label.push(':');
        label.push_str(name);
    }
    label
}


#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Focus {
    Search,
    Filter,
    Results,
}


#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum View {
    Results,
    Detail,
}


#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Overlay {
    Actions,
    Collections,
    Sort,
    History,
    Saved,
    SaveQuery,
    Columns,
    Export { all: bool },
    Help,
}


/// Commands exposed by the action picker and their keyboard shortcuts.
#[derive(Clone, Copy, Debug)]
pub(super) enum Action {
    Collections,
    Sort,
    Columns,
    History,
    Saved,
    Save,
    ExportRecord,
    ExportAll,
    Case,
    Regex,
    Preview,
    Theme,
    CancelIndexing,
    ResumeIndexing,
    Help,
}


impl Action {
    pub(super) const ALL: [Self; 15] = [
        Self::Collections,
        Self::Sort,
        Self::Columns,
        Self::History,
        Self::Saved,
        Self::Save,
        Self::ExportRecord,
        Self::ExportAll,
        Self::Case,
        Self::Regex,
        Self::Preview,
        Self::Theme,
        Self::CancelIndexing,
        Self::ResumeIndexing,
        Self::Help,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Collections => "Choose collection                Ctrl+O",
            Self::Sort => "Sort results                     Alt+S",
            Self::Columns => "Show or hide columns             Alt+L",
            Self::History => "Recent queries                   Ctrl+R",
            Self::Saved => "Saved queries                    Ctrl+B",
            Self::Save => "Save current query               Ctrl+S",
            Self::ExportRecord => "Export selected record           Alt+E",
            Self::ExportAll => "Export all matches               Alt+A",
            Self::Case => "Toggle case sensitivity          Alt+C",
            Self::Regex => "Toggle regex search              Alt+R",
            Self::Preview => "Toggle preview                   Ctrl+P",
            Self::Theme => "Switch light/dark theme          Alt+T",
            Self::CancelIndexing => "Cancel field indexing",
            Self::ResumeIndexing => "Resume field indexing",
            Self::Help => "Keyboard help                    F1",
        }
    }
}


/// Search response tagged with the input generation and requested page.
struct Update {
    generation: u64,
    offset: usize,
    elapsed: Duration,
    result: Result<SearchResult, String>,
}


/// Query inputs, page state, and terminal navigation for one repository.
pub(super) struct App {
    repo: Arc<IndexedRepository>,
    pub(super) single_collection: bool,
    base_offset: usize,
    result_limit: Option<usize>,
    active_columns: Vec<String>,
    cli_hidden: BTreeSet<String>,
    pub(super) search: Input,
    pub(super) filter: Input,
    pub(super) focus: Focus,
    pub(super) view: View,
    pub(super) overlay: Option<Overlay>,
    pub(super) scope: usize,
    pub(super) picker: usize,
    pub(super) sort_picker: usize,
    pub(super) sort_input: Input,
    pub(super) action_picker: usize,
    pub(super) list_picker: usize,
    pub(super) column_picker: usize,
    pub(super) column_offset: usize,
    pub(super) save_input: Input,
    pub(super) export_input: Input,
    pub(super) settings: Settings,
    settings_path: PathBuf,
    pub(super) sort: Option<SortField>,
    pub(super) case_sensitive: bool,
    pub(super) regex: bool,
    pub(super) show_preview: bool,
    pub(super) hits: Vec<SearchHit>,
    pub(super) display_rows: Vec<DisplayRow>,
    window_start: usize,
    next_offset: usize,
    pub(super) total: usize,
    pub(super) selected: usize,
    pub(super) scroll: usize,
    pub(super) detail_scroll: u16,
    pub(super) record: RecordState,
    detail_hit: Option<SearchHit>,
    detail_history: Vec<SearchHit>,
    pub(super) relation_rows: Vec<RelationRow>,
    pub(super) detail_area: Rect,
    pub(super) results_area: Rect,
    pub(super) viewport_rows: usize,
    pub(super) status: Option<String>,
    pub(super) index_status: Option<String>,
    pub(super) clipboard: Option<String>,
    pub(super) error: Option<String>,
    pub(super) searching: bool,
    pub(super) last_search_duration: Option<Duration>,
    pub(super) last_projection_duration: Option<Duration>,
    pub(super) done: bool,
    generation: u64,
    pending: Option<Instant>,
    requested_offset: Option<usize>,
    inflight: usize,
    queued_offset: Option<usize>,
    current_token: Option<CancellationToken>,
    interruption: Option<CancellationToken>,
    sender: mpsc::Sender<Update>,
    receiver: mpsc::Receiver<Update>,
}


impl App {
    #[cfg(test)]
    pub(super) fn new(repository: Repository) -> Self {
        let index = Index::from_repository(repository).unwrap();
        Self::with_options(
            IndexedRepository::new(index).unwrap(),
            TuiOptions::default(),
        )
    }

    pub(super) fn with_options(repository: IndexedRepository, options: TuiOptions) -> Self {
        let (sender, receiver) = mpsc::channel();
        let settings_path = if options.single_collection {
            repository.files().root.join(".objekte/.cache/obj-tui.json")
        } else {
            Settings::path(&repository.files().root)
        };
        let (settings, status) = match Settings::load(&settings_path) {
            Ok(settings) => (settings, None),
            Err(error) => (Settings::default(), Some(format!("Settings: {error}"))),
        };
        let mut app = Self {
            repo: Arc::new(repository),
            single_collection: options.single_collection,
            base_offset: options.offset,
            result_limit: options.limit,
            active_columns: Vec::new(),
            cli_hidden: BTreeSet::new(),
            search: Input::with_text(&options.initial_search),
            filter: Input::with_text(&options.initial_filter),
            focus: Focus::Search,
            view: View::Results,
            overlay: None,
            scope: usize::from(options.single_collection),
            picker: 0,
            sort_picker: 0,
            sort_input: Input::default(),
            action_picker: 0,
            list_picker: 0,
            column_picker: 0,
            column_offset: 0,
            save_input: Input::default(),
            export_input: Input::default(),
            settings,
            settings_path,
            sort: None,
            case_sensitive: true,
            regex: false,
            show_preview: true,
            hits: Vec::new(),
            display_rows: Vec::new(),
            window_start: 0,
            next_offset: 0,
            total: 0,
            selected: 0,
            scroll: 0,
            detail_scroll: 0,
            record: RecordState::default(),
            detail_hit: None,
            detail_history: Vec::new(),
            relation_rows: Vec::new(),
            detail_area: Rect::default(),
            results_area: Rect::default(),
            viewport_rows: 1,
            status,
            index_status: None,
            clipboard: None,
            error: None,
            searching: false,
            last_search_duration: None,
            last_projection_duration: None,
            done: false,
            generation: 0,
            pending: None,
            requested_offset: None,
            inflight: 0,
            queued_offset: None,
            current_token: None,
            interruption: options.interruption,
            sender,
            receiver,
        };
        let startup_status = app.status.take();
        app.request_search();
        app.status = startup_status;
        app
    }

    pub(super) fn collection_names(&self) -> impl Iterator<Item = &str> {
        self.repo
            .collections()
            .iter()
            .map(|c| c.definition.name.as_str())
    }

    pub(super) fn scope_label(&self) -> &str {
        if self.scope == 0 {
            "All collections"
        } else {
            &self.repo.collections()[self.scope - 1].definition.name
        }
    }

    pub(super) fn selected_hit(&self) -> Option<&SearchHit> {
        self.hits.get(self.selected)
    }

    pub(super) fn current_detail_hit(&self) -> Option<&SearchHit> {
        if self.view == View::Detail {
            self.detail_hit.as_ref().or_else(|| self.selected_hit())
        } else {
            self.selected_hit()
        }
    }

    pub(super) fn open_detail(&mut self) {
        if let Some(hit) = self.selected_hit().cloned() {
            self.view = View::Detail;
            self.detail_history.clear();
            self.record.reset();
            if self.single_collection {
                self.record.load(&hit);
            }
            self.detail_hit = Some(hit);
            self.detail_scroll = 0;
            self.refresh_relations();
        }
    }

    pub(super) fn refresh_relations(&mut self) {
        if self.single_collection {
            self.relation_rows.clear();
            return;
        }
        self.relation_rows = self
            .current_detail_hit()
            .map_or_else(Vec::new, |hit| relations::rows(&self.repo, hit));
    }

    pub(super) fn back_detail(&mut self) {
        if let Some(hit) = self.detail_history.pop() {
            self.detail_hit = Some(hit);
            self.record.reset();
            self.detail_scroll = 0;
            self.refresh_relations();
        } else {
            self.detail_hit = None;
            self.relation_rows.clear();
            self.view = View::Results;
        }
    }

    fn fetch_link(&self, collection: &str, id: &str) -> Result<SearchHit, String> {
        let query = Query {
            collections: vec![collection.into()],
            fts: None,
            filter: Some(parse_filter("@record.id = $id")?),
            options: QueryOptions {
                sort: Vec::new(),
                limit: 1,
                offset: 0,
                count: false,
                return_mode: ReturnMode::Bytes,
                evidence: false,
            },
        };
        self.repo
            .search_with_parameters(
                &query,
                &HashMap::from([("id".into(), Value::Id(id.into()))]),
            )?
            .hits
            .into_iter()
            .next()
            .ok_or_else(|| format!("linked record {collection}/{id} is unavailable"))
    }

    pub(super) fn move_detail(&mut self, delta: isize) {
        let Some(hit) = self.current_detail_hit() else {
            return;
        };
        if self.record.raw {
            self.detail_scroll = self.detail_scroll.saturating_add_signed(delta as i16);
            return;
        }
        let diagnostic = self
            .diagnostic(hit)
            .map(|issue| format!("{}: {}", issue.code, issue.message));
        let count = self.record.rows(hit, diagnostic.as_deref()).len() + self.relation_rows.len();
        self.record.cursor = self
            .record
            .cursor
            .saturating_add_signed(delta)
            .min(count.saturating_sub(1));
    }

    pub(super) fn adjacent_result(&mut self, next: bool) {
        if self.hits.is_empty() || self.view != View::Detail {
            return;
        }
        let previous = self.selected;
        self.move_selection(if next { 1 } else { -1 });
        if self.selected != previous {
            self.detail_hit = self.selected_hit().cloned();
            self.detail_history.clear();
            self.detail_scroll = 0;
            self.refresh_relations();
        }
    }

    pub(super) fn jump_detail_section(&mut self, section: u8) {
        if section == 1 {
            self.record.cursor = 0;
            return;
        }
        let Some(hit) = self.current_detail_hit() else {
            return;
        };
        let diagnostic = self
            .diagnostic(hit)
            .map(|issue| format!("{}: {}", issue.code, issue.message));
        let base = self.record.rows(hit, diagnostic.as_deref()).len();
        let label = if section == 2 {
            "Outgoing"
        } else {
            "Backlinks"
        };
        if let Some(index) = self.relation_rows.iter().position(|row| row.label == label) {
            self.record.cursor = base + index;
            self.status = None;
        } else {
            self.status = Some(format!("No {label} section for this record"));
        }
    }

    pub(super) fn cycle_detail_section(&mut self, forward: bool) {
        let Some(hit) = self.current_detail_hit() else {
            return;
        };
        let diagnostic = self
            .diagnostic(hit)
            .map(|issue| format!("{}: {}", issue.code, issue.message));
        let base = self.record.rows(hit, diagnostic.as_deref()).len();
        let mut sections = vec![0];
        for label in ["Outgoing", "Backlinks"] {
            if let Some(index) = self.relation_rows.iter().position(|row| row.label == label) {
                sections.push(base + index);
            }
        }
        sections.sort_unstable();
        let current = sections
            .iter()
            .rposition(|&position| position <= self.record.cursor)
            .unwrap_or(0);
        let next = if forward {
            (current + 1) % sections.len()
        } else {
            (current + sections.len() - 1) % sections.len()
        };
        self.record.cursor = sections[next];
        self.record.raw = false;
        self.status = None;
    }

    pub(super) fn find_next(&mut self, forward: bool) {
        if self.record.raw {
            self.status = Some("Find is available in structured view".into());
            return;
        }
        let Some(hit) = self.current_detail_hit() else {
            return;
        };
        let needle = fold_find(&self.record.find);
        if needle.is_empty() {
            return;
        }
        let diagnostic = self
            .diagnostic(hit)
            .map(|issue| format!("{}: {}", issue.code, issue.message));
        let rows = self.record.rows(hit, diagnostic.as_deref());
        let total = rows.len() + self.relation_rows.len();
        for step in 1..=total {
            let at = if forward {
                (self.record.cursor + step) % total
            } else {
                (self.record.cursor + total - step % total) % total
            };
            let row = if at < rows.len() {
                &rows[at]
            } else {
                let relation = &self.relation_rows[at - rows.len()];
                if fold_find(&relation.label).contains(&needle)
                    || fold_find(&relation.value).contains(&needle)
                {
                    self.record.cursor = at;
                    self.status = None;
                    return;
                }
                continue;
            };
            if fold_find(&row.label).contains(&needle) || fold_find(&row.value).contains(&needle) {
                self.record.cursor = at;
                self.status = None;
                return;
            }
        }
        self.status = Some(format!("No field contains {}", self.record.find));
    }

    pub(super) fn confirm_find(&mut self) {
        if let Some(input) = self.record.find_input.take() {
            self.record.find = input.text;
            self.find_next(true);
        }
    }

    pub(super) fn copy_detail(&mut self, whole: bool) {
        let Some(hit) = self.current_detail_hit() else {
            return;
        };
        let text = if whole || self.record.raw {
            record::raw_text(hit)
        } else {
            let diagnostic = self
                .diagnostic(hit)
                .map(|issue| format!("{}: {}", issue.code, issue.message));
            let rows = self.record.rows(hit, diagnostic.as_deref());
            if let Some(row) = rows.get(self.record.cursor) {
                row.value.clone()
            } else {
                self.relation_rows
                    .get(self.record.cursor.saturating_sub(rows.len()))
                    .map_or_else(String::new, |row| row.value.clone())
            }
        };
        self.clipboard = Some(text);
        self.status = Some(
            if whole {
                "Copied record"
            } else {
                "Copied value"
            }
            .into(),
        );
    }

    pub(super) fn toggle_detail_branch(&mut self) {
        let Some(hit) = self.current_detail_hit() else {
            return;
        };
        let diagnostic = self
            .diagnostic(hit)
            .map(|issue| format!("{}: {}", issue.code, issue.message));
        let rows = self.record.rows(hit, diagnostic.as_deref());
        if let Some((path, array)) = rows
            .get(self.record.cursor)
            .and_then(|row| row.branch.clone())
        {
            self.record.toggle(&path, array);
        } else if let Some(index) = self.record.cursor.checked_sub(rows.len())
            && let Some(action) = self
                .relation_rows
                .get(index)
                .and_then(|row| row.action.clone())
        {
            match action {
                LinkAction::Target { collection, id } => match self.fetch_link(&collection, &id) {
                    Ok(next) => {
                        if let Some(current) = self.detail_hit.replace(next) {
                            self.detail_history.push(current);
                        }
                        self.record.reset();
                        self.detail_scroll = 0;
                        self.refresh_relations();
                        self.status = None;
                    }
                    Err(error) => self.status = Some(error),
                },
                more => {
                    let page = relations::continuation(&self.repo, &more);
                    self.relation_rows.remove(index);
                    self.relation_rows.splice(index..index, page);
                }
            }
        }
    }

    pub(super) fn diagnostic(
        &self,
        hit: &SearchHit,
    ) -> Option<objekte_search_core::inventory::SourceDiagnostic> {
        self.repo.diagnostic(
            objekte_search_core::model::FileId(hit.file_id),
            hit.diagnostic_ref,
        )
    }

    pub(super) fn busy(&self) -> bool {
        self.searching || self.pending.is_some()
    }

    pub(super) fn interrupted(&self) -> bool {
        self.interruption
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
    }

    pub(super) fn current_query(&self) -> settings::Query {
        settings::Query {
            search: self.search.text.clone(),
            filter: self.filter.text.clone(),
            collection: (self.scope != 0).then(|| self.scope_label().to_owned()),
            case_insensitive: !self.case_sensitive,
            regex: self.regex,
        }
    }

    fn persist_settings(&mut self) {
        if let Err(error) = self.settings.save(&self.settings_path) {
            self.status = Some(format!("Settings: {error}"));
        }
    }

    pub(super) fn confirm_query(&mut self) {
        let query = self.current_query();
        self.settings.remember(query);
        self.request_search();
        self.persist_settings();
    }

    pub(super) fn restore_query(&mut self, query: settings::Query) {
        self.search = Input::with_text(&query.search);
        self.filter = Input::with_text(&query.filter);
        self.case_sensitive = !query.case_insensitive;
        self.regex = query.regex;
        self.scope = query
            .collection
            .as_deref()
            .and_then(|name| {
                self.repo
                    .collections()
                    .iter()
                    .position(|collection| collection.definition.name == name)
            })
            .map_or(usize::from(self.single_collection), |index| index + 1);
        self.column_offset = 0;
        self.sort = None;
        self.view = View::Results;
        self.overlay = None;
        self.confirm_query();
    }

    pub(super) fn save_query(&mut self) {
        let name = self.save_input.text.trim();
        if name.is_empty() {
            self.status = Some("Enter a name for the saved query".into());
            return;
        }
        let query = self.current_query();
        if query.search.trim().is_empty() && query.filter.trim().is_empty() {
            self.status = Some("Enter a search or filter before saving".into());
            return;
        }
        self.settings.saved.retain(|saved| saved.name != name);
        self.settings.saved.push(settings::SavedQuery {
            name: name.to_owned(),
            query,
        });
        self.overlay = None;
        self.status = Some(format!("Saved query {name}"));
        self.persist_settings();
    }

    pub(super) fn set_theme(&mut self, light: bool) {
        self.settings.theme = Some(if light { "light" } else { "dark" }.into());
        self.persist_settings();
    }

    pub(super) fn toggle_case(&mut self) {
        self.case_sensitive = !self.case_sensitive;
        self.request_search();
    }

    pub(super) fn toggle_regex(&mut self) {
        self.regex = !self.regex;
        self.request_search();
    }

    pub(super) fn choose_action(&mut self) -> Option<Action> {
        self.overlay = None;
        Action::ALL.get(self.action_picker).copied()
    }

    pub(super) fn sort_choices(&self) -> Vec<Option<SortField>> {
        let mut choices = vec![None];
        if self.scope == 0 {
            return choices;
        }
        if self.single_collection {
            for field in self.column_keys() {
                for descending in [false, true] {
                    choices.push(Some(SortField::simple(field.clone(), descending)));
                }
            }
            return choices;
        }
        let collection = &self.repo.collections()[self.scope - 1];
        for path in &collection.definition.sort_fields {
            if path.root == objekte_search_core::model::PathRoot::Backlinks {
                continue;
            }
            let field = if path.segments.is_empty() {
                match path.root {
                    objekte_search_core::model::PathRoot::Metadata(metadata) => {
                        metadata.as_str().to_owned()
                    }
                    objekte_search_core::model::PathRoot::Data => continue,
                    objekte_search_core::model::PathRoot::Backlinks => continue,
                }
            } else {
                path.segments.join(".")
            };
            for descending in [false, true] {
                choices.push(Some(SortField {
                    field: field.clone(),
                    descending,
                    expression: None,
                }));
            }
        }
        choices
    }

    pub(super) fn scope_name(&self) -> Option<&str> {
        (self.scope != 0).then(|| self.scope_label())
    }

    pub(super) fn column_keys(&self) -> Vec<String> {
        if self.single_collection {
            return self.active_columns.clone();
        }
        let mut keys = BTreeSet::new();
        for collection in self.repo.collections() {
            if self.scope == 0 || collection.definition.name == self.scope_label() {
                for (path, field) in collection.definition.schema.fields() {
                    if matches!(field.kind, LogicalType::Object) {
                        continue;
                    }
                    keys.insert(
                        FieldPath {
                            root: PathRoot::Data,
                            segments: path.clone(),
                        }
                        .display(),
                    );
                }
            }
        }
        let mut keys = keys.into_iter().collect::<Vec<_>>();
        keys.sort_by_key(|key| {
            let leaf = key.rsplit('.').next().unwrap_or(key);
            let priority = match leaf {
                "preferred_title" | "title" | "name" | "label" => 0,
                "year" | "date" | "type" => 1,
                _ => 2,
            };
            (priority, key.matches('.').count(), key.clone())
        });
        keys
    }

    pub(super) fn visible_columns(&self) -> Vec<String> {
        if self.single_collection {
            return self
                .active_columns
                .iter()
                .filter(|field| !self.cli_hidden.contains(*field))
                .cloned()
                .collect();
        }
        let hidden = self.settings.hidden(self.scope_name());
        let mut columns = self
            .column_keys()
            .into_iter()
            .filter(|key| !hidden.contains(key))
            .collect::<Vec<_>>();
        let mut searched = BTreeSet::new();
        if !self.search.text.trim().is_empty() {
            for hit in &self.hits {
                for path in &hit.matched_fields {
                    if let Ok(path) = QueryPath::parse(path) {
                        searched.insert(path.field.display());
                    }
                }
            }
        }
        if let Ok(filter) = parse_filter(self.filter.text.trim()) {
            let mut fields = Vec::new();
            collect_filter_fields(&filter, &mut fields);
            searched.extend(fields.into_iter().filter_map(|field| {
                QueryPath::parse(field.split(':').next().unwrap_or(&field))
                    .ok()
                    .map(|path| path.field.display())
            }));
        }
        columns.sort_by_key(|key| {
            if searched.contains(key) {
                0
            } else if self.sort.as_ref().is_some_and(|sort| sort.field == *key) {
                1
            } else {
                2
            }
        });
        columns
    }

    pub(super) fn toggle_column(&mut self) {
        if let Some(key) = self.column_keys().get(self.column_picker) {
            if self.single_collection {
                if !self.cli_hidden.insert(key.clone()) {
                    self.cli_hidden.remove(key);
                }
                self.column_offset = self
                    .column_offset
                    .min(self.visible_columns().len().saturating_sub(1));
                return;
            }
            let scope = self.scope_name().map(str::to_owned);
            self.settings.toggle_column(scope.as_deref(), key);
            self.column_offset = self
                .column_offset
                .min(self.visible_columns().len().saturating_sub(1));
            self.persist_settings();
        }
    }

    pub(super) fn scroll_columns(&mut self, right: bool) {
        if right {
            self.column_offset =
                (self.column_offset + 1).min(self.visible_columns().len().saturating_sub(1));
        } else {
            self.column_offset = self.column_offset.saturating_sub(1);
        }
    }

    pub(super) fn changed(&mut self, now: Instant) {
        if let Some(token) = self.current_token.take() {
            token.cancel();
        }
        self.queued_offset = None;
        self.generation = self.generation.wrapping_add(1);
        self.pending = Some(now + DEBOUNCE);
        self.searching = true;
        self.error = None;
        self.status = None;
        self.last_search_duration = None;
        self.last_projection_duration = None;
    }

    pub(super) fn request_search(&mut self) {
        if let Some(token) = self.current_token.take() {
            token.cancel();
        }
        self.queued_offset = None;
        self.generation = self.generation.wrapping_add(1);
        self.pending = None;
        self.status = None;
        self.error = None;
        self.hits.clear();
        self.display_rows.clear();
        self.window_start = 0;
        self.next_offset = 0;
        self.column_offset = 0;
        self.active_columns.clear();
        self.total = 0;
        self.selected = 0;
        self.scroll = 0;
        self.record.reset();
        self.detail_hit = None;
        self.detail_history.clear();
        self.relation_rows.clear();
        self.requested_offset = None;
        self.last_search_duration = None;
        self.last_projection_duration = None;
        self.launch(0);
    }

    pub(super) fn tick(&mut self, now: Instant) {
        self.refresh_index_status();
        while let Ok(update) = self.receiver.try_recv() {
            self.inflight = self.inflight.saturating_sub(1);
            if update.generation != self.generation {
                continue;
            }
            self.searching = false;
            self.requested_offset = None;
            self.last_search_duration = Some(update.elapsed);
            match update.result {
                Ok(result) => {
                    let Some(total) = result.count else {
                        self.error = Some("search omitted the requested count".into());
                        continue;
                    };
                    self.total = self
                        .result_limit
                        .map_or(total.saturating_sub(self.base_offset), |limit| {
                            limit.min(total.saturating_sub(self.base_offset))
                        });
                    if self.single_collection
                        && update.offset + result.hits.len() == self.window_start
                        && !self.hits.is_empty()
                    {
                        let added = result.hits.len();
                        self.hits.splice(0..0, result.hits);
                        self.window_start = update.offset;
                        self.selected = added.saturating_sub(1);
                        self.scroll = self
                            .selected
                            .saturating_sub(self.viewport_rows.saturating_sub(1));
                        self.hits.truncate(3 * PAGE_SIZE);
                        self.next_offset = self.window_start + self.hits.len();
                    } else if update.offset == 0 {
                        self.hits = result.hits;
                        self.next_offset = self.hits.len();
                    } else if update.offset == self.next_offset {
                        self.next_offset += result.hits.len();
                        self.hits.extend(result.hits);
                        if self.single_collection && self.hits.len() > 3 * PAGE_SIZE {
                            let drop = self.hits.len() - 3 * PAGE_SIZE;
                            self.hits.drain(..drop);
                            self.window_start += drop;
                            self.selected = self.selected.saturating_sub(drop);
                            self.scroll = self.scroll.saturating_sub(drop);
                        }
                    }
                    if self.single_collection {
                        let projection_started = Instant::now();
                        self.display_rows = self
                            .hits
                            .iter()
                            .map(|hit| {
                                record::display_row(
                                    hit,
                                    &self.active_columns,
                                    self.search.text.trim(),
                                    self.case_sensitive,
                                    self.regex,
                                )
                            })
                            .collect();
                        self.last_projection_duration = Some(projection_started.elapsed());
                        if let Some(hit) = self.selected_hit().cloned() {
                            self.record.load(&hit);
                        }
                    }
                    self.error = None;
                }
                Err(error) => {
                    self.active_columns.clear();
                    self.error = Some(error);
                }
            }
        }
        if self.inflight < 2 {
            if let Some(offset) = self.queued_offset.take() {
                self.launch(offset);
            }
        }
        if self.pending.is_some_and(|at| at <= now) {
            self.pending = None;
            self.hits.clear();
            self.display_rows.clear();
            self.window_start = 0;
            self.next_offset = 0;
            self.column_offset = 0;
            self.total = 0;
            self.selected = 0;
            self.scroll = 0;
            self.detail_hit = None;
            self.detail_history.clear();
            self.relation_rows.clear();
            self.launch(0);
        }
        if self.pending.is_none()
            && self.requested_offset.is_none()
            && !self.searching
            && self.error.is_none()
            && self.total > self.next_offset
            && self.selected.saturating_add(30) >= self.hits.len()
        {
            self.launch(self.next_offset);
        }
    }

    fn refresh_index_status(&mut self) {
        let Ok(status) = self.repo.index().extended_status() else {
            return;
        };
        self.index_status = match status.phase {
            ExtendedPhase::Unnecessary => None,
            ExtendedPhase::Pending | ExtendedPhase::Building if status.total_files > 0 => {
                Some(format!(
                    "Search ready · Indexing files {}/{}",
                    status.completed_files, status.total_files
                ))
            }
            ExtendedPhase::Pending | ExtendedPhase::Building => {
                Some("Search ready · Building field indexes".into())
            }
            ExtendedPhase::Complete => Some("Indexing complete".into()),
            ExtendedPhase::Paused => Some("Search ready · Field indexing paused".into()),
            ExtendedPhase::Cancelled => Some("Search ready · Field indexing stopped".into()),
            ExtendedPhase::Failed => Some("Search ready · Field indexing failed".into()),
        };
    }

    pub(super) fn cancel_indexing(&mut self) {
        match self.repo.index().extended_status() {
            Ok(status) if matches!(status.phase, ExtendedPhase::Unnecessary | ExtendedPhase::Complete) => {
                self.status = Some("Field indexing is already complete".into());
            }
            Ok(status) if status.phase == ExtendedPhase::Cancelled => {
                self.status = Some("Field indexing is already stopped".into());
            }
            Ok(_) => match self.repo.index().cancel_extended() {
                Ok(()) => {
                    self.refresh_index_status();
                    self.status = Some("Field indexing stopped; search remains available".into());
                }
                Err(error) => self.status = Some(format!("Could not stop field indexing: {error}")),
            },
            Err(error) => self.status = Some(format!("Could not read field indexing: {error}")),
        }
    }

    pub(super) fn resume_indexing(&mut self) {
        match self.repo.index().extended_status() {
            Ok(status) if matches!(status.phase, ExtendedPhase::Unnecessary | ExtendedPhase::Complete) => {
                self.status = Some("Field indexing is already complete".into());
                return;
            }
            Ok(status) if matches!(status.phase, ExtendedPhase::Pending | ExtendedPhase::Building) => {
                self.status = Some("Field indexing is already running".into());
                return;
            }
            Err(error) => {
                self.status = Some(format!("Could not read field indexing: {error}"));
                return;
            }
            Ok(_) => {}
        }
        match self.repo.index().resume_extended() {
            Ok(()) => {
                self.refresh_index_status();
                self.status = Some("Field indexing resumed".into());
            }
            Err(error) => self.status = Some(format!("Could not resume field indexing: {error}")),
        }
    }

    fn launch(&mut self, offset: usize) {
        let query = match self.query(offset) {
            Ok(query) => query,
            Err(error) => {
                self.error = Some(error);
                self.searching = false;
                return;
            }
        };
        if self.single_collection {
            let mut fields = Vec::new();
            if let Some(filter) = &query.filter {
                collect_filter_fields(filter, &mut fields);
            }
            for sort in &query.options.sort {
                let label = sort_label(sort);
                push_unique(&mut fields, label);
            }
            self.active_columns = fields;
            self.cli_hidden
                .retain(|field| self.active_columns.contains(field));
        }
        let repository = Arc::clone(&self.repo);
        let sender = self.sender.clone();
        let generation = self.generation;
        self.searching = true;
        self.requested_offset = Some(offset);
        if self.inflight >= 2 {
            self.queued_offset = Some(offset);
            return;
        }
        self.inflight += 1;
        let token = CancellationToken::new();
        self.current_token = Some(token.clone());
        let single_collection = self.single_collection;
        std::thread::spawn(move || {
            let started = Instant::now();
            let result = if single_collection {
                let mut page = Vec::new();
                let mut options = StreamOptions::default();
                options.mode = StreamMode::PageReplacements;
                let outcome = repository.search_stream(&query, options, &token, &mut |event| {
                    if let StreamEvent::PageReplacement { rows, .. } = event {
                        page = rows.to_vec();
                    }
                    Ok(objekte_search_core::streaming::StreamControl::Continue)
                });
                match outcome {
                    Ok(StreamOutcome::Complete(summary)) => Ok(SearchResult {
                        count: summary.count,
                        hits: page,
                        counters: summary.counters,
                    }),
                    Ok(StreamOutcome::Cancelled(_)) => Err("search cancelled".into()),
                    Err(error) => Err(error.to_string()),
                }
            } else {
                repository.search(&query)
            };
            let _ = sender.send(Update {
                generation,
                offset,
                elapsed: started.elapsed(),
                result,
            });
        });
    }

    fn query(&self, offset: usize) -> Result<Query, String> {
        let filter = self.filter.text.trim();
        let search = self.search.text.trim();
        let parsed = (!filter.is_empty())
            .then(|| parse_structured_filter(filter))
            .transpose()?;
        if parsed.as_ref().is_some_and(|parsed| parsed.sort.is_some()) && self.sort.is_some() {
            return Err("inline SORT BY conflicts with independent sort options".into());
        }
        Ok(Query {
            collections: if self.scope == 0 {
                Vec::new()
            } else {
                vec![self.scope_label().to_owned()]
            },
            fts: (!search.is_empty()).then(|| FtsQuery {
                pattern_parameter: None,
                mode: if self.regex {
                    FtsMode::Regex(RegexOptions::default())
                } else {
                    FtsMode::Literal
                },
                case_sensitivity: if self.case_sensitive {
                    CaseSensitivity::Sensitive
                } else {
                    CaseSensitivity::Insensitive
                },
                literal: search.to_owned(),
                fields: Vec::new(),
            }),
            filter: parsed.as_ref().and_then(|parsed| parsed.filter.clone()),
            options: QueryOptions {
                sort: parsed
                    .and_then(|parsed| parsed.sort)
                    .unwrap_or_else(|| self.sort.iter().cloned().collect()),
                limit: self.result_limit.map_or(PAGE_SIZE, |limit| {
                    PAGE_SIZE.min(limit.saturating_sub(offset))
                }),
                offset: self.base_offset.saturating_add(offset),
                count: true,
                return_mode: ReturnMode::Bytes,
                evidence: true,
            },
        })
    }

    pub(super) fn move_selection(&mut self, delta: isize) {
        if self.hits.is_empty() {
            return;
        }
        if self.single_collection
            && delta < 0
            && self.selected == 0
            && self.window_start > 0
            && self.requested_offset.is_none()
        {
            self.launch(self.window_start.saturating_sub(PAGE_SIZE));
            return;
        }
        let previous = self.selected;
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(self.hits.len() - 1);
        if self.selected != previous {
            self.detail_scroll = 0;
            self.record.reset();
            if self.single_collection {
                if let Some(hit) = self.selected_hit().cloned() {
                    self.record.load(&hit);
                }
            }
        }
        let rows = self.viewport_rows.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + rows {
            self.scroll = self.selected + 1 - rows;
        }
    }

    pub(super) fn choose_scope(&mut self) {
        self.scope = if self.single_collection {
            1
        } else {
            self.picker.min(self.repo.collections().len())
        };
        self.sort = None;
        self.column_offset = 0;
        self.view = View::Results;
        self.overlay = None;
        self.request_search();
    }

    pub(super) fn choose_sort(&mut self) {
        if self.single_collection && !self.sort_input.text.trim().is_empty() {
            match parse_structured_filter(&format!("SORT BY {}", self.sort_input.text.trim())) {
                Ok(parsed) if parsed.sort.as_ref().is_some_and(|sort| sort.len() == 1) => {
                    self.sort = parsed.sort.and_then(|mut sort| sort.pop());
                    self.overlay = None;
                    self.request_search();
                }
                Ok(_) => self.status = Some("Enter one sort expression".into()),
                Err(error) => self.status = Some(error),
            }
            return;
        }
        self.sort = self.sort_choices().get(self.sort_picker).cloned().flatten();
        self.view = View::Results;
        self.overlay = None;
        self.request_search();
    }

    pub(super) fn finish_export(&mut self, all: bool) {
        let path = PathBuf::from(self.export_input.text.trim());
        if path.as_os_str().is_empty() {
            self.status = Some("Enter a JSON file path".into());
            return;
        }
        let result = if all {
            self.query(0).and_then(|query| {
                export::all(
                    &self.repo,
                    query,
                    &self.current_query(),
                    &path,
                    self.result_limit,
                )
            })
        } else {
            self.selected_hit()
                .ok_or("No record selected".to_owned())
                .and_then(|hit| export::one(hit, &path).map(|()| 1))
        };
        match result {
            Ok(count) => {
                self.overlay = None;
                self.status = Some(format!("Exported {count} record(s) to {}", path.display()));
            }
            Err(error) => self.status = Some(format!("Export failed: {error}")),
        }
    }
}


fn sort_label(sort: &SortField) -> String {
    let Some(expression) = &sort.expression else {
        return field_label(&sort.field, &[]);
    };
    let mut path = field_label(&expression.path, &[]);
    if let Some(binding) = &expression.binding {
        path = format!("{path}:{binding}");
    }
    if let Some(reducer) = expression.reducer {
        format!("{}({path})", format!("{reducer:?}").to_uppercase())
    } else {
        path
    }
}

#[cfg(test)]
#[path = "app.test.rs"]
mod tests;
