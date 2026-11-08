//! Generation-checked operations around one native window and retained repository.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use gpui_kit::{App, ClipboardItem, PathPromptOptions};
use objekte_cli::execution::Exit;
use objekte_search_core::repository::SearchHit;
use objekte_search_core::streaming::CancellationToken;
use objekte_tui::relations::{self, LinkAction};

use crate::backend::{self, OpenSpec, OpenedSession, QuerySpec};
use crate::ui::model::{
    BatchRow, BatchSource as UiBatchSource, DetailView, ErrorField, ExportFormat, OpenDraft,
    OpenMode, PageView, RelationRow, SearchDraft, SnapshotMode, UiAction, UiUpdate,
};
use crate::ui::preferences::DesktopPreferences;

use super::{detail_view, open_spec, page_view_with_available, query_spec};


/// One record locator retained for the visible page, without source payloads.
#[derive(Clone)]
struct RecordKey {
    collection: String,
    id: String,
}


type QueryCompletion = Result<
    (
        PageView,
        Vec<RecordKey>,
        Option<usize>,
        u128,
        Vec<String>,
        Vec<String>,
        String,
    ),
    String,
>;
type OpenCompletion = Result<(OpenedSession, Result<DesktopPreferences, String>), String>;
type DetailCompletion = Result<(SearchHit, DetailView, Vec<Option<LinkAction>>), String>;


/// Completion messages from off-thread filesystem and search operations.
enum WorkerEvent {
    Opened {
        generation: u64,
        result: Box<OpenCompletion>,
    },
    Queried {
        open_generation: u64,
        query_generation: u64,
        result: Box<QueryCompletion>,
    },
    Detailed {
        open_generation: u64,
        query_generation: u64,
        detail_generation: u64,
        result: Box<DetailCompletion>,
    },
    Expanded {
        open_generation: u64,
        query_generation: u64,
        detail_generation: u64,
        view: DetailView,
    },
    MoreRelations {
        open_generation: u64,
        detail_generation: u64,
        result: Vec<relations::RelationRow>,
    },
    Picked(Result<Option<Vec<PathBuf>>, String>, OpenDraft),
    Exported {
        open_generation: u64,
        output_generation: u64,
        result: Result<String, String>,
    },
    Batched {
        open_generation: u64,
        output_generation: u64,
        result: Result<(backend::BatchReport, Vec<BatchRow>), String>,
    },
    PreferenceError(String),
    CopiedValue {
        open_generation: u64,
        detail_generation: u64,
        result: Result<(String, String), String>,
    },
}


/// UI-thread coordinator; expensive work is owned by short-lived workers.
pub struct DesktopController {
    actions: Receiver<UiAction>,
    worker_tx: Sender<WorkerEvent>,
    workers: Receiver<WorkerEvent>,
    preference_tx: Sender<DesktopPreferences>,
    session: Option<OpenedSession>,
    preferences: Option<DesktopPreferences>,
    open_diagnostics: String,
    open_draft: Option<OpenDraft>,
    open_generation: u64,
    query_generation: u64,
    detail_generation: u64,
    output_generation: u64,
    cancellation: CancellationToken,
    output_cancellation: CancellationToken,
    current_draft: SearchDraft,
    current_spec: Option<QuerySpec>,
    page_start: usize,
    next_page_start: Option<usize>,
    page: PageView,
    available_columns: Vec<String>,
    keys: Vec<RecordKey>,
    selected_hit: Option<Arc<SearchHit>>,
    detail: Option<DetailView>,
    relation_actions: Vec<Option<LinkAction>>,
    relation_back: Vec<(Arc<SearchHit>, DetailView, Vec<Option<LinkAction>>)>,
    expanded: HashSet<String>,
    pending_edit: Option<(Instant, SearchDraft)>,
}


impl DesktopController {
    /// Make the action sender consumed by `DesktopView` and its controller.
    pub fn new() -> (Self, Sender<UiAction>) {
        let (action_tx, actions) = mpsc::channel();
        let (worker_tx, workers) = mpsc::channel();
        let (preference_tx, preferences_to_save) = mpsc::channel::<DesktopPreferences>();
        let preference_results = worker_tx.clone();
        thread::spawn(move || {
            for preferences in preferences_to_save {
                if let Err(message) = preferences.persist() {
                    let _ = preference_results.send(WorkerEvent::PreferenceError(message));
                }
            }
        });
        (
            Self {
                actions,
                worker_tx,
                workers,
                preference_tx,
                session: None,
                preferences: None,
                open_diagnostics: String::new(),
                open_draft: None,
                open_generation: 0,
                query_generation: 0,
                detail_generation: 0,
                output_generation: 0,
                cancellation: CancellationToken::new(),
                output_cancellation: CancellationToken::new(),
                current_draft: SearchDraft::default(),
                current_spec: None,
                page_start: 0,
                next_page_start: None,
                page: PageView::default(),
                available_columns: Vec::new(),
                keys: Vec::new(),
                selected_hit: None,
                detail: None,
                relation_actions: Vec::new(),
                relation_back: Vec::new(),
                expanded: HashSet::new(),
                pending_edit: None,
            },
            action_tx,
        )
    }

    /// Drain bounded action and completion queues without waiting in a render frame.
    pub fn tick(&mut self, cx: &mut App) -> Vec<UiUpdate> {
        let mut updates = Vec::new();
        for _ in 0..128 {
            match self.actions.try_recv() {
                Ok(action) => self.handle(action, cx, &mut updates),
                Err(_) => break,
            }
        }
        for _ in 0..128 {
            match self.workers.try_recv() {
                Ok(event) => self.complete(event, cx, &mut updates),
                Err(_) => break,
            }
        }
        if self
            .pending_edit
            .as_ref()
            .is_some_and(|(at, _)| at.elapsed() >= Duration::from_millis(180))
        {
            let (_, draft) = self.pending_edit.take().expect("checked pending query");
            self.start_query(draft, 0, &mut updates);
        }
        updates
    }

    fn handle(&mut self, action: UiAction, cx: &mut App, updates: &mut Vec<UiUpdate>) {
        match action {
            UiAction::ChooseFolder(draft) => self.choose_folder(draft, cx),
            UiAction::Open(draft) => self.start_open(draft, updates),
            UiAction::SearchEdited(draft) => {
                self.cancellation.cancel();
                self.query_generation += 1;
                self.detail_generation += 1;
                self.current_draft = draft.clone();
                self.pending_edit = Some((Instant::now(), draft));
            }
            UiAction::SearchNow(draft) => {
                self.pending_edit = None;
                self.start_query(draft, 0, updates);
            }
            UiAction::Cancel => {
                self.cancellation.cancel();
                self.output_cancellation.cancel();
                self.query_generation += 1;
                self.output_generation += 1;
                self.pending_edit = None;
                updates.push(UiUpdate::Status("Operation cancelled".into()));
            }
            UiAction::PageRelative(direction) => {
                let next = if direction > 0 {
                    self.next_page_start
                } else if direction < 0 {
                    (self.page_start > 0).then(|| self.page_start.saturating_sub(100))
                } else {
                    None
                };
                if let Some(start) = next {
                    self.start_query(self.current_draft.clone(), start, updates);
                }
            }
            UiAction::SelectRow(index) => self.select_row(index, updates),
            UiAction::OpenDetail => {
                if let Some(index) = self.page.selected {
                    self.select_row(index, updates);
                }
            }
            UiAction::CloseDetail => {
                self.detail = None;
                updates.push(UiUpdate::Detail(None));
            }
            UiAction::AdjacentResult(forward) => {
                let selected = self.page.selected.unwrap_or(0);
                let next = if forward {
                    selected.saturating_add(1)
                } else {
                    selected.saturating_sub(1)
                };
                self.select_row(next, updates);
            }
            UiAction::ToggleDetailExpansion(index) => {
                if let Some(hit) = &self.selected_hit
                    && let Some(row) = self
                        .detail
                        .as_ref()
                        .and_then(|detail| detail.structured.get(index))
                {
                    let path = row.label.clone();
                    if !self.expanded.insert(path.clone()) {
                        self.expanded.remove(&path);
                    }
                    let hit = hit.clone();
                    let expanded = self.expanded.clone();
                    let relations = self
                        .detail
                        .as_ref()
                        .map_or_else(Vec::new, |detail| detail.relations.clone());
                    let selected = index;
                    self.detail_generation += 1;
                    let open_generation = self.open_generation;
                    let query_generation = self.query_generation;
                    let detail_generation = self.detail_generation;
                    let sender = self.worker_tx.clone();
                    thread::spawn(move || {
                        let mut view = detail_view(&hit, &expanded);
                        view.relations = relations;
                        view.selected_row = selected.min(view.structured.len().saturating_sub(1));
                        let _ = sender.send(WorkerEvent::Expanded {
                            open_generation,
                            query_generation,
                            detail_generation,
                            view: view.bounded(),
                        });
                    });
                }
            }
            UiAction::DetailRowSelected(index) => {
                if let Some(detail) = &mut self.detail {
                    detail.selected_row = index.min(
                        detail
                            .structured
                            .len()
                            .saturating_add(detail.relations.len())
                            .saturating_sub(1),
                    );
                    updates.push(UiUpdate::Detail(Some(detail.clone())));
                }
            }
            UiAction::DetailSectionRelative(forward) => self.move_detail_section(forward, updates),
            UiAction::FindInRecord { text, forward } => {
                self.find_in_record(&text, forward, updates)
            }
            UiAction::FollowRelation(index) => self.follow_relation(index, updates),
            UiAction::CopyRelation(index) => {
                if let Some(relation) = self
                    .detail
                    .as_ref()
                    .and_then(|detail| detail.relations.get(index))
                {
                    cx.write_to_clipboard(ClipboardItem::new_string(relation.value.clone()));
                    updates.push(UiUpdate::Status("Relation value copied".into()));
                }
            }
            UiAction::BackRelation => {
                if let Some((hit, view, actions)) = self.relation_back.pop() {
                    self.selected_hit = Some(hit);
                    self.detail = Some(view.clone());
                    self.relation_actions = actions;
                    updates.push(UiUpdate::Detail(Some(view)));
                }
            }
            UiAction::CopySelectedValue => {
                if let Some(detail) = &self.detail {
                    if let Some(row) = detail.structured.get(detail.selected_row) {
                        if row.value.ends_with('…')
                            && row.origin == crate::ui::model::DetailOrigin::Structured
                        {
                            if let Some(hit) = &self.selected_hit {
                                let hit = hit.clone();
                                let path = row.label.clone();
                                let sender = self.worker_tx.clone();
                                let open_generation = self.open_generation;
                                let detail_generation = self.detail_generation;
                                thread::spawn(move || {
                                    let result = full_field_value(&hit, &path)
                                        .map(|value| (value, "Complete field value copied".into()));
                                    let _ = sender.send(WorkerEvent::CopiedValue {
                                        open_generation,
                                        detail_generation,
                                        result,
                                    });
                                });
                            }
                        } else {
                            cx.write_to_clipboard(ClipboardItem::new_string(row.value.clone()));
                            updates.push(UiUpdate::Status("Selected value copied".into()));
                        }
                    } else if let Some(row) = detail
                        .relations
                        .get(detail.selected_row.saturating_sub(detail.structured.len()))
                    {
                        cx.write_to_clipboard(ClipboardItem::new_string(row.value.clone()));
                        updates.push(UiUpdate::Status("Relation value copied".into()));
                    }
                }
            }
            UiAction::CopyRecord => {
                if let Some(hit) = &self.selected_hit {
                    let hit = hit.clone();
                    let sender = self.worker_tx.clone();
                    let open_generation = self.open_generation;
                    let detail_generation = self.detail_generation;
                    thread::spawn(move || {
                        let result = Ok((
                            objekte_tui::record_raw_text(&hit),
                            "Complete record copied".into(),
                        ));
                        let _ = sender.send(WorkerEvent::CopiedValue {
                            open_generation,
                            detail_generation,
                            result,
                        });
                    });
                }
            }
            UiAction::Export { format, path } => self.start_export(format, path, updates),
            UiAction::RunBatch {
                source,
                text,
                path,
                output_path,
            } => {
                self.start_batch(source, text, path, output_path, updates);
            }
            UiAction::WriteIndex => {
                if let Some(mut draft) = self.open_draft.clone() {
                    draft.snapshot = SnapshotMode::Write;
                    self.start_open(draft, updates);
                } else {
                    updates.push(UiUpdate::Error {
                        field: ErrorField::Open,
                        message: "Open a repository before writing an index".into(),
                    });
                }
            }
            UiAction::SaveQuery { name, draft } => {
                if let Some(preferences) = &mut self.preferences {
                    match preferences.save_query_local(&name, &draft) {
                        Ok(()) => {
                            let _ = self.preference_tx.send(preferences.clone());
                            updates.push(UiUpdate::History {
                                entries: preferences.history(),
                                saved: preferences.saved(),
                            });
                        }
                        Err(message) => updates.push(UiUpdate::Error {
                            field: ErrorField::General,
                            message,
                        }),
                    }
                }
            }
            UiAction::ToggleColumn(column) => {
                if !self.available_columns.contains(&column) {
                    return;
                }
                if let Some(preferences) = &mut self.preferences {
                    preferences
                        .toggle_column_local(self.current_draft.collection.as_deref(), &column);
                    let _ = self.preference_tx.send(preferences.clone());
                    self.start_query(self.current_draft.clone(), self.page_start, updates);
                }
            }
            UiAction::RestoreHistory(index) => {
                if let Some(draft) = self
                    .preferences
                    .as_ref()
                    .and_then(|prefs| prefs.restore_history(index))
                {
                    updates.push(UiUpdate::RestoreQuery(draft.clone()));
                    self.start_query(draft, 0, updates);
                }
            }
            UiAction::RestoreSaved(name) => {
                if let Some(draft) = self
                    .preferences
                    .as_ref()
                    .and_then(|prefs| prefs.restore_saved(&name))
                {
                    updates.push(UiUpdate::RestoreQuery(draft.clone()));
                    self.start_query(draft, 0, updates);
                }
            }
            UiAction::SetTheme { dark } => {
                if let Some(preferences) = &mut self.preferences {
                    preferences.set_dark_local(dark);
                    let _ = self.preference_tx.send(preferences.clone());
                }
            }
            UiAction::ShowHelp | UiAction::ToggleRawSource | UiAction::TogglePreview => {}
        }
    }

    fn choose_folder(&self, draft: OpenDraft, cx: &mut App) {
        let sender = self.worker_tx.clone();
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: draft.mode == OpenMode::MultipleFolders,
            prompt: Some("Choose Objekte folder".into()),
        });
        cx.spawn(async move |_| {
            let result = receiver
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result.map_err(|error| error.to_string()));
            let _ = sender.send(WorkerEvent::Picked(result, draft));
        })
        .detach();
    }

    fn start_open(&mut self, draft: OpenDraft, updates: &mut Vec<UiUpdate>) {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let spec = match open_spec(&draft, &cwd) {
            Ok(spec) => spec,
            Err(message) => {
                updates.push(UiUpdate::Error {
                    field: ErrorField::Open,
                    message,
                });
                return;
            }
        };
        self.cancellation.cancel();
        self.output_cancellation.cancel();
        self.cancellation = CancellationToken::new();
        self.open_generation += 1;
        self.query_generation += 1;
        self.output_generation += 1;
        self.session = None;
        self.preferences = None;
        self.open_diagnostics.clear();
        self.open_draft = Some(draft);
        self.keys.clear();
        self.selected_hit = None;
        self.detail = None;
        self.relation_actions.clear();
        self.relation_back.clear();
        self.page = PageView::default();
        self.available_columns.clear();
        self.pending_edit = None;
        let generation = self.open_generation;
        let token = self.cancellation.clone();
        let sender = self.worker_tx.clone();
        updates.push(UiUpdate::Opening(
            "Opening repository and building indexes…".into(),
        ));
        thread::spawn(move || {
            let result = open_with_preferences(&spec, &token);
            let _ = sender.send(WorkerEvent::Opened {
                generation,
                result: Box::new(result),
            });
        });
    }

    fn start_query(&mut self, draft: SearchDraft, page_start: usize, updates: &mut Vec<UiUpdate>) {
        let Some(session) = self.session.clone() else {
            updates.push(UiUpdate::Error {
                field: ErrorField::General,
                message: "Open a repository first".into(),
            });
            return;
        };
        let spec = match query_spec(&draft) {
            Ok(spec) => spec,
            Err(message) => {
                updates.push(UiUpdate::Error {
                    field: query_error_field(&message, &draft),
                    message,
                });
                return;
            }
        };
        self.cancellation.cancel();
        self.cancellation = CancellationToken::new();
        self.query_generation += 1;
        self.detail_generation += 1;
        self.current_draft = draft.clone();
        self.current_spec = Some(spec.clone());
        self.page_start = page_start;
        self.selected_hit = None;
        self.detail = None;
        self.relation_actions.clear();
        self.relation_back.clear();
        let open_generation = self.open_generation;
        let query_generation = self.query_generation;
        let token = self.cancellation.clone();
        let sender = self.worker_tx.clone();
        let hidden = self.preferences.as_ref().map_or_else(Vec::new, |prefs| {
            prefs.hidden(draft.collection.as_deref()).to_vec()
        });
        updates.push(UiUpdate::QueryStarted("Searching…".into()));
        thread::spawn(move || {
            let result = backend::query_page(&session, &spec, page_start, 100, &token)
                .map(|page| {
                    let diagnostics = format!(
                        "Query: {:.1} ms; page start {}; rows {}; exact total {:?}\nCounters: {:#?}",
                        page.elapsed_ns as f64 / 1_000_000.0,
                        page.page_start,
                        page.hits.len(),
                        page.count,
                        page.counters,
                    );
                    let (view, available) = page_view_with_available(&page, &spec, &hidden);
                    let keys = page
                        .hits
                        .into_iter()
                        .map(|hit| RecordKey {
                            collection: hit.collection,
                            id: hit.record_id,
                        })
                        .collect();
                    (
                        view,
                        keys,
                        page.next_page_start,
                        page.elapsed_ns,
                        available,
                        hidden,
                        diagnostics,
                    )
                })
                .map_err(|error| error.to_string());
            let _ = sender.send(WorkerEvent::Queried {
                open_generation,
                query_generation,
                result: Box::new(result),
            });
        });
    }

    fn select_row(&mut self, index: usize, updates: &mut Vec<UiUpdate>) {
        let (Some(session), Some(key)) = (self.session.clone(), self.keys.get(index).cloned())
        else {
            return;
        };
        self.page.selected = Some(index);
        self.detail = None;
        self.relation_actions.clear();
        updates.push(UiUpdate::Page(self.page.clone()));
        self.detail_generation += 1;
        let detail_generation = self.detail_generation;
        let open_generation = self.open_generation;
        let query_generation = self.query_generation;
        let token = self.cancellation.clone();
        let sender = self.worker_tx.clone();
        thread::spawn(move || {
            let result = backend::record_by_id(&session, &key.collection, &key.id, &token)
                .and_then(|hit| {
                    hit.ok_or_else(|| {
                        backend::BackendError::new(
                            backend::BackendErrorKind::Query,
                            "Record is no longer available",
                        )
                    })
                })
                .map(|hit| record_presentation(&session, hit))
                .map_err(|error| error.to_string());
            let _ = sender.send(WorkerEvent::Detailed {
                open_generation,
                query_generation,
                detail_generation,
                result: Box::new(result),
            });
        });
    }

    fn complete(&mut self, event: WorkerEvent, cx: &mut App, updates: &mut Vec<UiUpdate>) {
        match event {
            WorkerEvent::Opened { generation, result } if generation == self.open_generation => {
                match *result {
                    Ok((session, preferences)) => {
                        let root = session.paths.root.display().to_string();
                        let collections = session.collections.clone();
                        let single_collection = session.single_collection;
                        self.open_diagnostics = format!(
                            "Open: {:.1} ms\nRoot: {}\nCollections: {}\nSchema fields: {}; files: {:?}\nSnapshot save: {:#?}\nStartup: {:#?}",
                            session.diagnostics.open_ns as f64 / 1_000_000.0,
                            root,
                            collections.join(", "),
                            session.diagnostics.schema_fields,
                            session.diagnostics.schema_files,
                            session.diagnostics.snapshot_save,
                            session.diagnostics.startup,
                        );
                        self.session = Some(session);
                        match preferences {
                            Ok(preferences) => {
                                updates.push(UiUpdate::History {
                                    entries: preferences.history(),
                                    saved: preferences.saved(),
                                });
                                if let Some(dark) = preferences.dark() {
                                    updates.push(UiUpdate::Theme { dark });
                                }
                                self.preferences = Some(preferences);
                            }
                            Err(message) => updates.push(UiUpdate::Error {
                                field: ErrorField::General,
                                message,
                            }),
                        }
                        updates.push(UiUpdate::Opened {
                            root,
                            collections: collections.clone(),
                            single_collection,
                        });
                        updates.push(UiUpdate::Diagnostics(self.open_diagnostics.clone()));
                        let mut draft = self.current_draft.clone();
                        draft.collection = if single_collection {
                            collections.first().cloned()
                        } else {
                            None
                        };
                        self.start_query(draft, 0, updates);
                    }
                    Err(message) => updates.push(UiUpdate::Error {
                        field: ErrorField::Open,
                        message,
                    }),
                }
            }
            WorkerEvent::Queried {
                open_generation,
                query_generation,
                result,
            } if open_generation == self.open_generation
                && query_generation == self.query_generation =>
            {
                match *result {
                    Ok((view, keys, next, elapsed_ns, available, hidden, diagnostics)) => {
                        self.page = view.clone();
                        self.available_columns = available.clone();
                        self.keys = keys;
                        self.next_page_start = next;
                        updates.push(UiUpdate::Columns { available, hidden });
                        updates.push(UiUpdate::Page(view));
                        updates.push(UiUpdate::Diagnostics(format!(
                            "{}\n\n{diagnostics}",
                            self.open_diagnostics
                        )));
                        updates.push(UiUpdate::Status(format!(
                            "Query complete in {:.1} ms",
                            elapsed_ns as f64 / 1_000_000.0
                        )));
                        if let Some(preferences) = &mut self.preferences {
                            preferences.remember_local(&self.current_draft);
                            let _ = self.preference_tx.send(preferences.clone());
                            updates.push(UiUpdate::History {
                                entries: preferences.history(),
                                saved: preferences.saved(),
                            });
                        }
                    }
                    Err(message) => updates.push(UiUpdate::Error {
                        field: query_error_field(&message, &self.current_draft),
                        message,
                    }),
                }
            }
            WorkerEvent::Detailed {
                open_generation,
                query_generation,
                detail_generation,
                result,
            } if open_generation == self.open_generation
                && query_generation == self.query_generation
                && detail_generation == self.detail_generation =>
            {
                match *result {
                    Ok((hit, view, actions)) => {
                        self.selected_hit = Some(Arc::new(hit));
                        self.expanded.clear();
                        self.detail = Some(view.clone());
                        self.relation_actions = actions;
                        updates.push(UiUpdate::Detail(Some(view)));
                    }
                    Err(message) => updates.push(UiUpdate::Error {
                        field: ErrorField::General,
                        message,
                    }),
                }
            }
            WorkerEvent::Picked(result, draft) => match result {
                Ok(Some(paths)) if !paths.is_empty() => {
                    let selected = paths
                        .into_iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join("\n");
                    let text = if draft.mode == OpenMode::MultipleFolders
                        && !draft.paths.trim().is_empty()
                    {
                        format!("{}\n{selected}", draft.paths.trim())
                    } else {
                        selected
                    };
                    updates.push(UiUpdate::ChosenPaths(text));
                }
                Ok(_) => {}
                Err(message) => updates.push(UiUpdate::Error {
                    field: ErrorField::Open,
                    message: format!("Folder chooser failed: {message}. Enter a path instead."),
                }),
            },
            WorkerEvent::Expanded {
                open_generation,
                query_generation,
                detail_generation,
                view,
            } if open_generation == self.open_generation
                && query_generation == self.query_generation
                && detail_generation == self.detail_generation =>
            {
                self.detail = Some(view.clone());
                updates.push(UiUpdate::Detail(Some(view)));
            }
            WorkerEvent::Exported {
                open_generation,
                output_generation,
                result,
            } if open_generation == self.open_generation
                && output_generation == self.output_generation =>
            {
                match result {
                    Ok(message) => updates.push(UiUpdate::Status(message)),
                    Err(message) => updates.push(UiUpdate::Error {
                        field: ErrorField::Export,
                        message,
                    }),
                }
            }
            WorkerEvent::Batched {
                open_generation,
                output_generation,
                result,
            } if open_generation == self.open_generation
                && output_generation == self.output_generation =>
            {
                match result {
                    Ok((report, rows)) => {
                        let outcome = format!(
                            "Batch {:?}: {} bytes written to {}",
                            report.exit,
                            report.bytes_written,
                            report.path.display()
                        );
                        updates.push(UiUpdate::Batch {
                            rows,
                            running: false,
                            outcome: Some(outcome.clone()),
                        });
                        if report.exit == Exit::Success {
                            updates.push(UiUpdate::Status(outcome));
                        } else {
                            updates.push(UiUpdate::Error {
                                field: ErrorField::Batch,
                                message: outcome,
                            });
                        }
                    }
                    Err(message) => updates.push(UiUpdate::Error {
                        field: ErrorField::Batch,
                        message,
                    }),
                }
            }
            WorkerEvent::MoreRelations {
                open_generation,
                detail_generation,
                result,
            } if open_generation == self.open_generation
                && detail_generation == self.detail_generation =>
            {
                if let Some(detail) = &mut self.detail {
                    let (rows, actions) = relation_presentation(result);
                    detail.relations.extend(rows);
                    self.relation_actions.extend(actions);
                    *detail = detail.clone().bounded();
                    self.relation_actions.truncate(detail.relations.len());
                    updates.push(UiUpdate::Detail(Some(detail.clone())));
                }
            }
            WorkerEvent::PreferenceError(message) => updates.push(UiUpdate::Error {
                field: ErrorField::General,
                message,
            }),
            WorkerEvent::CopiedValue {
                open_generation,
                detail_generation,
                result,
            } if open_generation == self.open_generation
                && detail_generation == self.detail_generation =>
            {
                match result {
                    Ok((value, message)) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(value));
                        updates.push(UiUpdate::Status(message));
                    }
                    Err(message) => updates.push(UiUpdate::Error {
                        field: ErrorField::General,
                        message,
                    }),
                }
            }
            _ => {}
        }
    }

    fn find_in_record(&mut self, text: &str, forward: bool, updates: &mut Vec<UiUpdate>) {
        let Some(detail) = &mut self.detail else {
            return;
        };
        if text.is_empty() || detail.structured.is_empty() {
            return;
        }
        let len = detail.structured.len();
        let start = detail.selected_row;
        for step in 1..=len {
            let index = if forward {
                (start + step) % len
            } else {
                (start + len - step % len) % len
            };
            let row = &detail.structured[index];
            if row.label.contains(text) || row.value.contains(text) {
                detail.selected_row = index;
                updates.push(UiUpdate::Detail(Some(detail.clone())));
                return;
            }
        }
        updates.push(UiUpdate::Status(format!("No field contains {text}")));
    }

    fn move_detail_section(&mut self, forward: bool, updates: &mut Vec<UiUpdate>) {
        let Some(detail) = &mut self.detail else {
            return;
        };
        let mut starts: Vec<usize> = Vec::new();
        for (index, row) in detail.structured.iter().enumerate() {
            if starts.is_empty() || detail.structured[starts[starts.len() - 1]].origin != row.origin
            {
                starts.push(index);
            }
        }
        if !detail.relations.is_empty() {
            starts.push(detail.structured.len());
        }
        if starts.is_empty() {
            return;
        }
        let current = starts
            .iter()
            .rposition(|start| *start <= detail.selected_row)
            .unwrap_or(0);
        let next = if forward {
            (current + 1) % starts.len()
        } else {
            (current + starts.len() - 1) % starts.len()
        };
        detail.selected_row = starts[next];
        updates.push(UiUpdate::Detail(Some(detail.clone())));
    }

    fn follow_relation(&mut self, index: usize, updates: &mut Vec<UiUpdate>) {
        let Some(action) = self.relation_actions.get(index).cloned().flatten() else {
            return;
        };
        let Some(session) = self.session.clone() else {
            return;
        };
        match action {
            LinkAction::Target { collection, id } => {
                if let (Some(hit), Some(view)) = (&self.selected_hit, &self.detail) {
                    if self.relation_back.len() >= 32 {
                        self.relation_back.remove(0);
                    }
                    self.relation_back.push((
                        hit.clone(),
                        view.clone(),
                        self.relation_actions.clone(),
                    ));
                }
                self.detail_generation += 1;
                let detail_generation = self.detail_generation;
                let open_generation = self.open_generation;
                let query_generation = self.query_generation;
                let token = self.cancellation.clone();
                let sender = self.worker_tx.clone();
                updates.push(UiUpdate::Status(format!(
                    "Following relation to {collection} / {id}…"
                )));
                thread::spawn(move || {
                    let result = backend::record_by_id(&session, &collection, &id, &token)
                        .and_then(|hit| {
                            hit.ok_or_else(|| {
                                backend::BackendError::new(
                                    backend::BackendErrorKind::Query,
                                    "Related record is unavailable",
                                )
                            })
                        })
                        .map(|hit| record_presentation(&session, hit))
                        .map_err(|error| error.to_string());
                    let _ = sender.send(WorkerEvent::Detailed {
                        open_generation,
                        query_generation,
                        detail_generation,
                        result: Box::new(result),
                    });
                });
            }
            continuation => {
                let open_generation = self.open_generation;
                let detail_generation = self.detail_generation;
                let sender = self.worker_tx.clone();
                thread::spawn(move || {
                    let result = relations::continuation(&session.repository, &continuation);
                    let _ = sender.send(WorkerEvent::MoreRelations {
                        open_generation,
                        detail_generation,
                        result,
                    });
                });
            }
        }
    }

    fn start_export(&mut self, format: ExportFormat, path: String, updates: &mut Vec<UiUpdate>) {
        let Some(session) = self.session.clone() else {
            updates.push(UiUpdate::Error {
                field: ErrorField::Export,
                message: "Open a repository first".into(),
            });
            return;
        };
        let path = PathBuf::from(path.trim());
        if path.as_os_str().is_empty() {
            updates.push(UiUpdate::Error {
                field: ErrorField::Export,
                message: "Enter a new output path".into(),
            });
            return;
        }
        let spec = self.current_spec.clone();
        let hit = self.selected_hit.clone();
        let open_generation = self.open_generation;
        self.output_cancellation.cancel();
        self.output_cancellation = CancellationToken::new();
        self.output_generation += 1;
        let output_generation = self.output_generation;
        let token = self.output_cancellation.clone();
        let sender = self.worker_tx.clone();
        updates.push(UiUpdate::Status("Exporting…".into()));
        thread::spawn(move || {
            let result = match format {
                ExportFormat::SelectedJson => match hit {
                    Some(hit) => backend::export_selected_json(&session, &hit, &path, &token)
                        .map(|()| format!("Selected record exported to {}", path.display()))
                        .map_err(|error| error.to_string()),
                    None => Err("Select a record before exporting it".into()),
                },
                ExportFormat::ResultsJson => match spec {
                    Some(spec) => backend::export_all_json(&session, &spec, &path, &token)
                        .map(|count| format!("Exported {count} results to {}", path.display()))
                        .map_err(|error| error.to_string()),
                    None => Err("Run a query before exporting results".into()),
                },
                ExportFormat::Ndjson => match spec {
                    Some(spec) => backend::export_query_ndjson(&session, &spec, &path, &token)
                        .and_then(|report| {
                            if report.exit == Exit::Success {
                                Ok(report)
                            } else {
                                Err(backend::BackendError::new(
                                    backend::BackendErrorKind::Query,
                                    format!(
                                        "NDJSON ended {:?}; inspect {} for events",
                                        report.exit,
                                        report.path.display()
                                    ),
                                ))
                            }
                        })
                        .map(|report| {
                            format!(
                                "NDJSON complete: {} bytes written to {}",
                                report.bytes_written,
                                report.path.display()
                            )
                        })
                        .map_err(|error| error.to_string()),
                    None => Err("Run a query before exporting NDJSON".into()),
                },
            };
            let _ = sender.send(WorkerEvent::Exported {
                open_generation,
                output_generation,
                result,
            });
        });
    }

    fn start_batch(
        &mut self,
        source: UiBatchSource,
        text: String,
        path: String,
        output_path: String,
        updates: &mut Vec<UiUpdate>,
    ) {
        let Some(session) = self.session.clone() else {
            updates.push(UiUpdate::Error {
                field: ErrorField::Batch,
                message: "Open a repository first".into(),
            });
            return;
        };
        let source = match source {
            UiBatchSource::PastedText if !text.trim().is_empty() => {
                backend::BatchSource::Text(text)
            }
            UiBatchSource::QueryFile if !path.trim().is_empty() => {
                backend::BatchSource::File(PathBuf::from(path.trim()))
            }
            UiBatchSource::PastedText => {
                updates.push(UiUpdate::Error {
                    field: ErrorField::Batch,
                    message: "Paste one or more query lines".into(),
                });
                return;
            }
            UiBatchSource::QueryFile => {
                updates.push(UiUpdate::Error {
                    field: ErrorField::Batch,
                    message: "Enter a query file path".into(),
                });
                return;
            }
        };
        if output_path.trim().is_empty() {
            updates.push(UiUpdate::Error {
                field: ErrorField::Batch,
                message: "Enter a new NDJSON output path".into(),
            });
            return;
        }
        let query = match query_spec(&self.current_draft) {
            Ok(spec) => spec,
            Err(message) => {
                updates.push(UiUpdate::Error {
                    field: query_error_field(&message, &self.current_draft),
                    message,
                });
                return;
            }
        };
        let spec = backend::BatchSpec {
            source,
            offset: query.offset,
            limit: query.limit,
            count: query.count,
        };
        let path = PathBuf::from(output_path.trim());
        self.output_cancellation.cancel();
        self.output_cancellation = CancellationToken::new();
        self.output_generation += 1;
        let output_generation = self.output_generation;
        let open_generation = self.open_generation;
        let token = self.output_cancellation.clone();
        let sender = self.worker_tx.clone();
        updates.push(UiUpdate::Batch {
            rows: Vec::new(),
            running: true,
            outcome: None,
        });
        thread::spawn(move || {
            let result = backend::run_batch(&session, &spec, &path, &token)
                .map(|report| {
                    let rows = batch_rows(&report.path).unwrap_or_default();
                    (report, rows)
                })
                .map_err(|error| error.to_string());
            let _ = sender.send(WorkerEvent::Batched {
                open_generation,
                output_generation,
                result,
            });
        });
    }
}


fn open_with_preferences(
    spec: &OpenSpec,
    token: &CancellationToken,
) -> Result<(OpenedSession, Result<DesktopPreferences, String>), String> {
    let session = backend::open(spec, token).map_err(|error| error.to_string())?;
    let preferences = DesktopPreferences::load(&session.paths.root);
    Ok((session, preferences))
}

fn record_presentation(
    session: &OpenedSession,
    hit: SearchHit,
) -> (SearchHit, DetailView, Vec<Option<LinkAction>>) {
    let mut view = detail_view(&hit, &HashSet::new());
    let (rows, actions) = relation_presentation(relations::rows(&session.repository, &hit));
    view.relations = rows;
    view = view.bounded();
    let mut actions = actions;
    actions.truncate(view.relations.len());
    (hit, view, actions)
}

fn relation_presentation(
    rows: Vec<relations::RelationRow>,
) -> (Vec<RelationRow>, Vec<Option<LinkAction>>) {
    rows.into_iter()
        .take(512)
        .map(|row| {
            let continuation = matches!(
                row.action.as_ref(),
                Some(LinkAction::MoreOutgoing { .. } | LinkAction::MoreBacklinks { .. })
            );
            let available = row.action.is_some();
            (
                RelationRow {
                    label: row.label,
                    value: row.value,
                    available,
                    continuation,
                },
                row.action,
            )
        })
        .unzip()
}

fn query_error_field(message: &str, draft: &SearchDraft) -> ErrorField {
    let lower = message.to_ascii_lowercase();
    if lower.contains("offset") || lower.contains("limit") {
        ErrorField::Range
    } else if lower.contains("sort") {
        ErrorField::Sort
    } else if draft.regex && lower.contains("regex") {
        ErrorField::FullText
    } else {
        ErrorField::Structured
    }
}

/// Resolve a visible JSON field from the retained source when its display text was clipped.
fn full_field_value(hit: &SearchHit, path: &str) -> Result<String, String> {
    let value = objekte_tui::record_json_value(hit)
        .ok_or_else(|| "Selected field is unavailable".to_string())?;
    let found = find_json_path(&value, "$", path)
        .ok_or_else(|| "Selected field is no longer available".to_string())?;
    Ok(match found {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    })
}

fn find_json_path<'a>(
    value: &'a serde_json::Value,
    current: &str,
    target: &str,
) -> Option<&'a serde_json::Value> {
    if current == target {
        return Some(value);
    }
    match value {
        serde_json::Value::Object(fields) => fields.iter().find_map(|(key, child)| {
            let next = format!("{current}.{key}");
            target
                .starts_with(&next)
                .then(|| find_json_path(child, &next, target))
                .flatten()
        }),
        serde_json::Value::Array(values) => values.iter().enumerate().find_map(|(index, child)| {
            let next = format!("{current}[{index}]");
            target
                .starts_with(&next)
                .then(|| find_json_path(child, &next, target))
                .flatten()
        }),
        _ => None,
    }
}

/// Read only short event lines; complete record events can be arbitrarily large.
fn batch_rows(path: &Path) -> Result<Vec<BatchRow>, String> {
    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut chunk = [0u8; 8192];
    let mut line = Vec::with_capacity(1024);
    let mut rows: Vec<(u64, BatchRow)> = Vec::new();
    let mut truncated = false;
    loop {
        let count = file.read(&mut chunk).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        for byte in &chunk[..count] {
            if *byte == b'\n' {
                if !truncated && let Ok(event) = serde_json::from_slice::<serde_json::Value>(&line)
                {
                    note_batch_event(&mut rows, &event);
                }
                line.clear();
                truncated = false;
            } else if line.len() < 16 * 1024 {
                line.push(*byte);
            } else {
                truncated = true;
            }
        }
    }
    Ok(rows.into_iter().map(|(_, row)| row).collect())
}

fn note_batch_event(rows: &mut Vec<(u64, BatchRow)>, event: &serde_json::Value) {
    let Some(kind) = event.get("type").and_then(serde_json::Value::as_str) else {
        return;
    };
    let Some(id) = event.get("query_id").and_then(serde_json::Value::as_u64) else {
        return;
    };
    if kind == "query_start" {
        let line = event
            .get("line")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(id) as usize;
        let query = event
            .get("input")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .chars()
            .take(512)
            .collect();
        rows.push((
            id,
            BatchRow {
                line,
                query,
                status: "Running".into(),
                error: None,
            },
        ));
        if rows.len() > 200 {
            rows.remove(0);
        }
    } else if let Some((_, row)) = rows.iter_mut().find(|(row_id, _)| *row_id == id) {
        match kind {
            "query_complete" => row.status = "Complete".into(),
            "query_cancelled" => row.status = "Cancelled".into(),
            "query_error" => {
                row.status = "Error".into();
                row.error = Some(
                    event
                        .get("error")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("Query failed")
                        .chars()
                        .take(1024)
                        .collect(),
                );
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "runtime.test.rs"]
mod tests;
