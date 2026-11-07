//! Search and selection state shared by the event loop and renderer.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use objekte_search_core::model::{FieldPath, LogicalType, PathRoot, Value};
use objekte_search_core::query::{FtsQuery, Query, QueryOptions, parse_filter};
use objekte_search_core::repository::{Repository, SearchHit, SearchResult};
use objekte_search_core::workload::{ReturnMode, SortField};
use ratatui::layout::Rect;

use super::export;
use super::input::Input;
use super::record::RecordState;
use super::relations::{self, LinkAction, RelationRow};
use super::settings::{self, Settings};

pub(super) const PAGE_SIZE: usize = 100;
pub(super) const DEBOUNCE: Duration = Duration::from_millis(180);


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
    Collections,
    Sort,
    History,
    Saved,
    SaveQuery,
    Columns,
    Export { all: bool },
    Help,
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
    repo: Arc<Repository>,
    pub(super) search: Input,
    pub(super) filter: Input,
    pub(super) focus: Focus,
    pub(super) view: View,
    pub(super) overlay: Option<Overlay>,
    pub(super) scope: usize,
    pub(super) picker: usize,
    pub(super) sort_picker: usize,
    pub(super) list_picker: usize,
    pub(super) column_picker: usize,
    pub(super) save_input: Input,
    pub(super) export_input: Input,
    pub(super) settings: Settings,
    settings_path: PathBuf,
    pub(super) sort: Option<SortField>,
    pub(super) show_preview: bool,
    pub(super) hits: Vec<SearchHit>,
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
    pub(super) error: Option<String>,
    pub(super) searching: bool,
    pub(super) last_search_duration: Option<Duration>,
    pub(super) done: bool,
    generation: u64,
    pending: Option<Instant>,
    requested_offset: Option<usize>,
    sender: mpsc::Sender<Update>,
    receiver: mpsc::Receiver<Update>,
}


impl App {
    pub(super) fn new(repository: Repository) -> Self {
        let (sender, receiver) = mpsc::channel();
        let settings_path = Settings::path(&repository.files().root);
        let (settings, status) = match Settings::load(&settings_path) {
            Ok(settings) => (settings, None),
            Err(error) => (Settings::default(), Some(format!("Settings: {error}"))),
        };
        let mut app = Self {
            repo: Arc::new(repository),
            search: Input::default(),
            filter: Input::default(),
            focus: Focus::Search,
            view: View::Results,
            overlay: None,
            scope: 0,
            picker: 0,
            sort_picker: 0,
            list_picker: 0,
            column_picker: 0,
            save_input: Input::default(),
            export_input: Input::default(),
            settings,
            settings_path,
            sort: None,
            show_preview: true,
            hits: Vec::new(),
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
            error: None,
            searching: false,
            last_search_duration: None,
            done: false,
            generation: 0,
            pending: None,
            requested_offset: None,
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
            self.detail_hit = Some(hit);
            self.detail_history.clear();
            self.record.reset();
            self.detail_scroll = 0;
            self.refresh_relations();
        }
    }

    pub(super) fn refresh_relations(&mut self) {
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
    ) -> Option<&objekte_search_core::inventory::SourceDiagnostic> {
        self.repo.files().diagnostic(
            objekte_search_core::model::FileId(hit.file_id),
            hit.diagnostic_ref,
        )
    }

    pub(super) fn busy(&self) -> bool {
        self.searching || self.pending.is_some()
    }

    pub(super) fn current_query(&self) -> settings::Query {
        settings::Query {
            search: self.search.text.clone(),
            filter: self.filter.text.clone(),
            collection: (self.scope != 0).then(|| self.scope_label().to_owned()),
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
        self.scope = query
            .collection
            .as_deref()
            .and_then(|name| {
                self.repo
                    .collections()
                    .iter()
                    .position(|collection| collection.definition.name == name)
            })
            .map_or(0, |index| index + 1);
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

    pub(super) fn sort_choices(&self) -> Vec<Option<SortField>> {
        let mut choices = vec![None];
        if self.scope == 0 {
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
                }));
            }
        }
        choices
    }

    pub(super) fn scope_name(&self) -> Option<&str> {
        (self.scope != 0).then(|| self.scope_label())
    }

    pub(super) fn column_keys(&self) -> Vec<String> {
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
        let hidden = self.settings.hidden(self.scope_name());
        self.column_keys()
            .into_iter()
            .filter(|key| !hidden.contains(key))
            .collect()
    }

    pub(super) fn toggle_column(&mut self) {
        if let Some(key) = self.column_keys().get(self.column_picker) {
            let scope = self.scope_name().map(str::to_owned);
            self.settings.toggle_column(scope.as_deref(), key);
            self.persist_settings();
        }
    }

    pub(super) fn changed(&mut self, now: Instant) {
        self.generation = self.generation.wrapping_add(1);
        self.pending = Some(now + DEBOUNCE);
        self.searching = true;
        self.error = None;
        self.status = None;
        self.last_search_duration = None;
    }

    pub(super) fn request_search(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.pending = None;
        self.status = None;
        self.error = None;
        self.hits.clear();
        self.total = 0;
        self.selected = 0;
        self.scroll = 0;
        self.record.reset();
        self.detail_hit = None;
        self.detail_history.clear();
        self.relation_rows.clear();
        self.requested_offset = None;
        self.last_search_duration = None;
        self.launch(0);
    }

    pub(super) fn tick(&mut self, now: Instant) {
        while let Ok(update) = self.receiver.try_recv() {
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
                    self.total = total;
                    if update.offset == 0 {
                        self.hits = result.hits;
                    } else if update.offset == self.hits.len() {
                        self.hits.extend(result.hits);
                    }
                    self.error = None;
                }
                Err(error) => self.error = Some(error),
            }
        }
        if self.pending.is_some_and(|at| at <= now) {
            self.pending = None;
            self.hits.clear();
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
            && self.total > self.hits.len()
            && self.selected.saturating_add(30) >= self.hits.len()
        {
            self.launch(self.hits.len());
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
        let repository = Arc::clone(&self.repo);
        let sender = self.sender.clone();
        let generation = self.generation;
        self.searching = true;
        self.requested_offset = Some(offset);
        std::thread::spawn(move || {
            let started = Instant::now();
            let result = repository.search(&query);
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
        Ok(Query {
            collections: if self.scope == 0 {
                Vec::new()
            } else {
                vec![self.scope_label().to_owned()]
            },
            fts: (!search.is_empty()).then(|| FtsQuery {
                pattern_parameter: None,
                mode: objekte_search_core::query::FtsMode::Literal,
                case_sensitivity: objekte_search_core::query::CaseSensitivity::Sensitive,
                literal: search.to_owned(),
                fields: Vec::new(),
            }),
            filter: (!filter.is_empty())
                .then(|| parse_filter(filter))
                .transpose()?,
            options: QueryOptions {
                sort: self.sort.iter().cloned().collect(),
                limit: PAGE_SIZE,
                offset,
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
        let previous = self.selected;
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(self.hits.len() - 1);
        if self.selected != previous {
            self.detail_scroll = 0;
            self.record.reset();
        }
        let rows = self.viewport_rows.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + rows {
            self.scroll = self.selected + 1 - rows;
        }
    }

    pub(super) fn choose_scope(&mut self) {
        self.scope = self.picker.min(self.repo.collections().len());
        self.sort = None;
        self.overlay = None;
        self.request_search();
    }

    pub(super) fn choose_sort(&mut self) {
        self.sort = self.sort_choices().get(self.sort_picker).cloned().flatten();
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
            self.query(0)
                .and_then(|query| export::all(&self.repo, query, &self.current_query(), &path))
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


#[cfg(test)]
#[path = "app.test.rs"]
mod tests;
