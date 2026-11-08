//! Generation-checked operations around one native window and retained repository.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use gpui_kit::{App, PathPromptOptions};
use objekte_cli::execution::Exit;
use objekte_search_core::posting::ScalarKey;
use objekte_search_core::posting_values::PostingValuesRequest;
use objekte_search_core::repository::SearchHit;
use objekte_search_core::streaming::CancellationToken;
use objekte_tui::relations::{self, LinkAction};

use crate::backend::{self, OpenSpec, OpenedSession};
use crate::ui::model::{
    BatchRow, BatchSource as UiBatchSource, DetailView, ErrorField, ExportFormat,
    FilterValidationState, OpenDraft, PageView, RecordTabSummary, RelationRow, SearchDraft,
    SnapshotMode, TabViewState, UiAction, UiUpdate,
};
use crate::ui::preferences::DesktopPreferences;

use super::filter_validation::{field_choices, validate_request};
use super::open_detection::{DetectedSource, detect_source};
use super::{detail_view, initial_columns, open_spec, page_view_with_prior, query_spec};


/// One lightweight locator retained for every delivered result.
#[derive(Clone, Eq, PartialEq)]
struct RecordKey {
    collection: String,
    id: String,
}


#[derive(Clone)]
struct TabContext {
    query_generation: u64,
    spec: backend::QuerySpec,
    draft: SearchDraft,
}


#[derive(Clone)]
struct TabFrame {
    key: RecordKey,
    anchor: usize,
    view: TabViewState,
    in_results: bool,
}


struct RecordTab {
    id: u64,
    context: Arc<TabContext>,
    key: RecordKey,
    anchor: usize,
    back: Vec<TabFrame>,
    expanded: HashSet<String>,
    view: TabViewState,
    in_results: bool,
    operation_generation: u64,
    cancellation: CancellationToken,
    loading: bool,
    links_pending: bool,
    links_loaded: bool,
    end_confirmed: bool,
    title: String,
}


#[derive(Clone)]
enum TabLoadKind {
    Open,
    Adjacent { anchor: usize },
    Relation { key: RecordKey },
    Back { frame: TabFrame },
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
type TabCompletion = Result<Option<(SearchHit, DetailView, Vec<Option<LinkAction>>)>, String>;


#[derive(Clone)]
struct CompletionRequest {
    revision: u64,
    collection: Option<String>,
    field: String,
    prefix: String,
}


type CompletionCacheKey = (u64, Option<String>, String, String);
type CompletionCacheValue = (Vec<(String, String)>, bool, u64);


#[derive(Clone, Copy)]
enum GridRequestKind {
    First,
    Continuation(usize),
    Projection(usize),
}


fn spawn_grid_page(
    session: OpenedSession,
    spec: backend::QuerySpec,
    page_start: usize,
    hidden: Vec<String>,
    prior_columns: Vec<String>,
    token: CancellationToken,
    sender: Sender<WorkerEvent>,
    open_generation: u64,
    query_generation: u64,
    kind: GridRequestKind,
) {
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
                let (view, available) = page_view_with_prior(&page, &spec, &hidden, &prior_columns);
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
            kind,
            result: Box::new(result),
        });
    });
}


/// Completion messages from off-thread filesystem and search operations.
enum WorkerEvent {
    Validated {
        open_generation: u64,
        validation_generation: u64,
        draft: SearchDraft,
        state: FilterValidationState,
    },
    Opened {
        generation: u64,
        result: Box<OpenCompletion>,
    },
    Queried {
        open_generation: u64,
        query_generation: u64,
        kind: GridRequestKind,
        result: Box<QueryCompletion>,
    },
    Counted {
        open_generation: u64,
        query_generation: u64,
        result: Result<usize, String>,
    },
    CompletionValues {
        open_generation: u64,
        completion_generation: u64,
        request: CompletionRequest,
        values: CompletionCacheValue,
    },
    TabLoaded {
        open_generation: u64,
        tab_id: u64,
        operation_generation: u64,
        kind: TabLoadKind,
        result: Box<TabCompletion>,
    },
    LinksLoaded {
        open_generation: u64,
        tab_id: u64,
        operation_generation: u64,
        rows: Vec<relations::RelationRow>,
    },
    Expanded {
        open_generation: u64,
        detail_generation: u64,
        view: DetailView,
    },
    MoreRelations {
        open_generation: u64,
        detail_generation: u64,
        result: Vec<relations::RelationRow>,
    },
    Detected {
        generation: u64,
        result: Result<DetectedSource, String>,
    },
    PickedFolder {
        generation: u64,
        slot: usize,
        result: Result<Option<Vec<PathBuf>>, String>,
    },
    PickedSchema {
        generation: u64,
        result: Result<Option<Vec<PathBuf>>, String>,
    },
    Exported {
        open_generation: u64,
        output_generation: u64,
        result: Result<String, String>,
    },
    Indexed {
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
    PreferenceSaved,
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
    opening_draft: Option<OpenDraft>,
    detection_generation: u64,
    picker_generation: u64,
    open_generation: u64,
    query_generation: u64,
    validation_generation: u64,
    detail_generation: u64,
    output_generation: u64,
    cancellation: CancellationToken,
    output_cancellation: CancellationToken,
    current_draft: SearchDraft,
    executed_spec: Option<backend::QuerySpec>,
    executed_draft: Option<SearchDraft>,
    next_page_start: Option<usize>,
    pending_continuation: Option<usize>,
    continuation_failed: bool,
    pending_projections: HashSet<usize>,
    projected_pages: VecDeque<usize>,
    selected_index: Option<usize>,
    available_columns: Vec<String>,
    keys: Vec<Option<RecordKey>>,
    pending_reveal: Option<usize>,
    tabs: Vec<RecordTab>,
    active_tab: Option<u64>,
    next_tab_id: u64,
    selected_hit: Option<Arc<SearchHit>>,
    detail: Option<DetailView>,
    relation_actions: Vec<Option<LinkAction>>,
    expanded: HashSet<String>,
    pending_edit: Option<(Instant, SearchDraft)>,
    pending_completion: Option<(Instant, CompletionRequest)>,
    completion_generation: u64,
    completion_cache: HashMap<CompletionCacheKey, CompletionCacheValue>,
    completion_cache_order: VecDeque<CompletionCacheKey>,
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
                match preferences.persist() {
                    Ok(()) => {
                        let _ = preference_results.send(WorkerEvent::PreferenceSaved);
                    }
                    Err(message) => {
                        let _ = preference_results.send(WorkerEvent::PreferenceError(message));
                    }
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
                opening_draft: None,
                detection_generation: 0,
                picker_generation: 0,
                open_generation: 0,
                query_generation: 0,
                validation_generation: 0,
                detail_generation: 0,
                output_generation: 0,
                cancellation: CancellationToken::new(),
                output_cancellation: CancellationToken::new(),
                current_draft: SearchDraft::default(),
                executed_spec: None,
                executed_draft: None,
                next_page_start: None,
                pending_continuation: None,
                continuation_failed: false,
                pending_projections: HashSet::new(),
                projected_pages: VecDeque::new(),
                selected_index: None,
                available_columns: Vec::new(),
                keys: Vec::new(),
                pending_reveal: None,
                tabs: Vec::new(),
                active_tab: None,
                next_tab_id: 1,
                selected_hit: None,
                detail: None,
                relation_actions: Vec::new(),
                expanded: HashSet::new(),
                pending_edit: None,
                pending_completion: None,
                completion_generation: 0,
                completion_cache: HashMap::new(),
                completion_cache_order: VecDeque::new(),
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
            self.start_validation(draft, &mut updates);
        }
        if self
            .pending_completion
            .as_ref()
            .is_some_and(|(at, _)| at.elapsed() >= Duration::from_millis(120))
        {
            let (_, request) = self.pending_completion.take().expect("checked completion");
            self.start_completion(request, &mut updates);
        }
        updates
    }

    fn handle(&mut self, action: UiAction, cx: &mut App, updates: &mut Vec<UiUpdate>) {
        match action {
            UiAction::DetectOpen { path, generation } => self.start_detection(path, generation),
            UiAction::ChooseFolder {
                draft: _,
                slot,
                generation,
            } => self.choose_folder(slot, generation, cx),
            UiAction::ChooseSchema { generation } => self.choose_schema(generation, cx),
            UiAction::CancelOpen => self.cancel_open(updates),
            UiAction::Open(draft) => self.start_open(draft, updates),
            UiAction::SearchEdited(draft) => {
                self.current_draft = draft.clone();
                self.pending_edit = Some((Instant::now(), draft));
                self.validation_generation += 1;
                updates.push(UiUpdate::FilterValidation {
                    generation: self.validation_generation,
                    state: FilterValidationState::Pending,
                });
                self.push_field_choices(updates);
            }
            UiAction::SearchNow(draft) => {
                self.pending_edit = None;
                self.current_draft = draft.clone();
                self.start_validation(draft, updates);
            }
            UiAction::CompleteValues {
                revision,
                collection,
                field,
                prefix,
            } => {
                self.completion_generation += 1;
                self.pending_completion = Some((
                    Instant::now(),
                    CompletionRequest {
                        revision,
                        collection,
                        field,
                        prefix,
                    },
                ));
            }
            UiAction::Cancel => {
                self.cancellation.cancel();
                self.output_cancellation.cancel();
                self.query_generation += 1;
                self.output_generation += 1;
                self.pending_edit = None;
                self.validation_generation += 1;
                if let Some(session) = &self.session {
                    updates.push(UiUpdate::FilterValidation {
                        generation: self.validation_generation,
                        state: validate_request(session, &self.current_draft),
                    });
                }
                updates.push(UiUpdate::Status("Operation cancelled".into()));
            }
            UiAction::VisibleRows { start, end } => {
                self.ensure_projection(start, end);
                if !self.continuation_failed && end.saturating_add(20) >= self.keys.len() {
                    self.start_continuation(updates);
                }
            }
            UiAction::RetryContinuation => {
                self.continuation_failed = false;
                self.start_continuation(updates);
            }
            UiAction::CountExact => self.start_count(updates),
            UiAction::SelectRow(index) => self.select_row(index, updates),
            UiAction::OpenRecord(index) => self.open_record(index, updates),
            UiAction::ActivateTab(id) => self.activate_tab(id, updates),
            UiAction::SetTabView { id, view } => {
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
                    tab.view = view;
                }
            }
            UiAction::CloseTab(id) => self.close_tab(id, updates),
            UiAction::OpenTabQuery => self.open_tab_query(updates),
            UiAction::LoadLinks => self.load_links(updates),
            UiAction::OpenDetail => {
                if let Some(index) = self.selected_index {
                    self.open_record(index, updates);
                }
            }
            UiAction::CloseDetail => {
                if let Some(id) = self.active_tab {
                    self.close_tab(id, updates);
                }
            }
            UiAction::AdjacentResult(forward) => self.navigate_tab(forward, updates),
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
                    if let Some(tab) = self
                        .active_tab
                        .and_then(|id| self.tabs.iter_mut().find(|tab| tab.id == id))
                    {
                        tab.expanded = self.expanded.clone();
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
                    let detail_generation = self.detail_generation;
                    let sender = self.worker_tx.clone();
                    thread::spawn(move || {
                        let mut view = detail_view(&hit, &expanded);
                        view.relations = relations;
                        view.selected_row = selected.min(view.structured.len().saturating_sub(1));
                        let _ = sender.send(WorkerEvent::Expanded {
                            open_generation,
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
            UiAction::FollowRelation(index) => self.follow_relation(index, updates),
            UiAction::OpenRelationTab(index) => self.open_relation_tab(index, updates),
            UiAction::CopyRelation(index) => {
                if let Some(relation) = self
                    .detail
                    .as_ref()
                    .and_then(|detail| detail.relations.get(index))
                {
                    updates.push(UiUpdate::Clipboard(relation.value.clone()));
                    updates.push(UiUpdate::Status("Relation value copied".into()));
                }
            }
            UiAction::BackRelation => {
                if let Some(tab) = self
                    .active_tab
                    .and_then(|id| self.tabs.iter().find(|tab| tab.id == id))
                {
                    if let Some(frame) = tab.back.last().cloned() {
                        self.start_tab_load(tab.id, TabLoadKind::Back { frame }, updates);
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
            UiAction::WriteIndex => self.start_index(SnapshotMode::Write, String::new(), updates),
            UiAction::Index { mode, directory } => self.start_index(mode, directory, updates),
            UiAction::SaveQuery(draft) => {
                if let Some(session) = &self.session
                    && let FilterValidationState::Invalid { message, .. } =
                        validate_request(session, &draft)
                {
                    updates.push(UiUpdate::Error {
                        field: ErrorField::Structured,
                        message,
                    });
                    return;
                }
                if let Some(preferences) = &mut self.preferences {
                    preferences.save_query_local(&draft);
                    let _ = self.preference_tx.send(preferences.clone());
                    updates.push(UiUpdate::History {
                        entries: preferences.history(),
                        saved: preferences.saved(),
                    });
                }
            }
            UiAction::RenameSaved { id, title } => {
                if let Some(preferences) = &mut self.preferences {
                    match preferences.rename_local(id, &title) {
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
            UiAction::RemoveSaved(id) => {
                if let Some(preferences) = &mut self.preferences
                    && preferences.remove_local(id)
                {
                    let _ = self.preference_tx.send(preferences.clone());
                    updates.push(UiUpdate::History {
                        entries: preferences.history(),
                        saved: preferences.saved(),
                    });
                }
            }
            UiAction::RetryPreferences => {
                if let Some(preferences) = &self.preferences {
                    let _ = self.preference_tx.send(preferences.clone());
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
                    self.start_query(self.current_draft.clone(), 0, updates);
                }
            }
            UiAction::ResizeColumn { column, width } => {
                if let Some(preferences) = &mut self.preferences {
                    preferences.set_column_width_local(
                        self.current_draft.collection.as_deref(),
                        &column,
                        width as f32,
                    );
                    let _ = self.preference_tx.send(preferences.clone());
                }
            }
            UiAction::ToggleWrapColumn(column) => {
                if let Some(preferences) = &mut self.preferences {
                    preferences
                        .toggle_wrap_local(self.current_draft.collection.as_deref(), &column);
                    let _ = self.preference_tx.send(preferences.clone());
                    let (widths, wrapped) =
                        preferences.table_presentation(self.current_draft.collection.as_deref());
                    updates.push(UiUpdate::TablePresentation {
                        scope: self.current_draft.collection.clone(),
                        widths,
                        wrapped,
                    });
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
            UiAction::RestoreSaved(id) => {
                if let Some(draft) = self
                    .preferences
                    .as_ref()
                    .and_then(|prefs| prefs.restore_saved(id))
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

    fn start_detection(&mut self, path: String, generation: u64) {
        self.detection_generation = generation;
        let sender = self.worker_tx.clone();
        thread::spawn(move || {
            let result = detect_source(Path::new(&path));
            let _ = sender.send(WorkerEvent::Detected { generation, result });
        });
    }

    fn choose_folder(&mut self, slot: usize, generation: u64, cx: &mut App) {
        self.picker_generation = generation;
        let sender = self.worker_tx.clone();
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose Objekte folder".into()),
        });
        cx.spawn(async move |_| {
            let result = receiver
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result.map_err(|error| error.to_string()));
            let _ = sender.send(WorkerEvent::PickedFolder {
                generation,
                slot,
                result,
            });
        })
        .detach();
    }

    fn choose_schema(&mut self, generation: u64, cx: &mut App) {
        self.picker_generation = generation;
        let sender = self.worker_tx.clone();
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose JSON Schema".into()),
        });
        cx.spawn(async move |_| {
            let result = receiver
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result.map_err(|error| error.to_string()));
            let _ = sender.send(WorkerEvent::PickedSchema { generation, result });
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
        self.pending_completion = None;
        self.completion_generation += 1;
        self.completion_cache.clear();
        self.completion_cache_order.clear();
        self.detection_generation = self.detection_generation.wrapping_add(1);
        self.picker_generation = self.picker_generation.wrapping_add(1);
        self.query_generation += 1;
        self.validation_generation += 1;
        self.output_generation += 1;
        self.opening_draft = Some(draft);
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

    fn cancel_open(&mut self, updates: &mut Vec<UiUpdate>) {
        if self.opening_draft.take().is_some() {
            self.cancellation.cancel();
            self.open_generation = self.open_generation.wrapping_add(1);
            updates.push(UiUpdate::Status("Open cancelled".into()));
        }
    }

    fn start_validation(&mut self, draft: SearchDraft, updates: &mut Vec<UiUpdate>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        self.validation_generation += 1;
        let validation_generation = self.validation_generation;
        let open_generation = self.open_generation;
        updates.push(UiUpdate::FilterValidation {
            generation: validation_generation,
            state: FilterValidationState::Pending,
        });
        let sender = self.worker_tx.clone();
        thread::spawn(move || {
            let state = validate_request(&session, &draft);
            let _ = sender.send(WorkerEvent::Validated {
                open_generation,
                validation_generation,
                draft,
                state,
            });
        });
    }

    fn push_field_choices(&self, updates: &mut Vec<UiUpdate>) {
        if let Some(session) = &self.session {
            updates.push(UiUpdate::FieldChoices(field_choices(
                session,
                self.current_draft.collection.as_deref(),
                &self.available_columns,
            )));
        }
    }

    fn start_query(&mut self, draft: SearchDraft, page_start: usize, updates: &mut Vec<UiUpdate>) {
        let Some(session) = self.session.clone() else {
            updates.push(UiUpdate::Error {
                field: ErrorField::General,
                message: "Open a repository first".into(),
            });
            return;
        };
        self.pending_edit = None;
        self.validation_generation += 1;
        let validation = validate_request(&session, &draft);
        updates.push(UiUpdate::FilterValidation {
            generation: self.validation_generation,
            state: validation.clone(),
        });
        if !matches!(validation, FilterValidationState::Valid) {
            self.current_draft = draft;
            return;
        }
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
        self.current_draft = draft.clone();
        self.executed_draft = Some(draft.clone());
        self.executed_spec = Some(spec.clone());
        self.selected_index = None;
        self.keys.clear();
        self.pending_reveal = None;
        self.next_page_start = None;
        self.pending_continuation = None;
        self.continuation_failed = false;
        self.pending_projections.clear();
        self.projected_pages.clear();
        let hidden = self.preferences.as_ref().map_or_else(Vec::new, |prefs| {
            prefs.hidden(draft.collection.as_deref()).to_vec()
        });
        if let Some(preferences) = &self.preferences {
            let (widths, wrapped) = preferences.table_presentation(draft.collection.as_deref());
            updates.push(UiUpdate::TablePresentation {
                scope: draft.collection.clone(),
                widths,
                wrapped,
            });
        }
        updates.push(UiUpdate::QueryStarted("Searching…".into()));
        let columns = initial_columns(&session, &draft);
        spawn_grid_page(
            session,
            spec,
            page_start,
            hidden,
            columns,
            self.cancellation.clone(),
            self.worker_tx.clone(),
            self.open_generation,
            self.query_generation,
            GridRequestKind::First,
        );
    }

    fn start_continuation(&mut self, updates: &mut Vec<UiUpdate>) {
        let (Some(session), Some(spec), Some(start)) = (
            self.session.clone(),
            self.executed_spec.clone(),
            self.next_page_start,
        ) else {
            return;
        };
        if self.pending_continuation.is_some() {
            return;
        }
        self.pending_continuation = Some(start);
        let hidden = self.preferences.as_ref().map_or_else(Vec::new, |prefs| {
            prefs
                .hidden(
                    self.executed_draft
                        .as_ref()
                        .and_then(|draft| draft.collection.as_deref()),
                )
                .to_vec()
        });
        updates.push(UiUpdate::ContinuationStarted);
        spawn_grid_page(
            session,
            spec,
            start,
            hidden,
            self.available_columns.clone(),
            self.cancellation.clone(),
            self.worker_tx.clone(),
            self.open_generation,
            self.query_generation,
            GridRequestKind::Continuation(start),
        );
    }

    fn ensure_projection(&mut self, first: usize, end: usize) {
        let (Some(session), Some(spec)) = (self.session.clone(), self.executed_spec.clone()) else {
            return;
        };
        let last = end.min(self.keys.len());
        if first >= last {
            return;
        }
        for start in ((first / 100) * 100..last).step_by(100) {
            if self.projected_pages.contains(&start) || !self.pending_projections.insert(start) {
                continue;
            }
            if self.pending_projections.len() > 2 {
                self.pending_projections.remove(&start);
                break;
            }
            let hidden = self.preferences.as_ref().map_or_else(Vec::new, |prefs| {
                prefs
                    .hidden(
                        self.executed_draft
                            .as_ref()
                            .and_then(|draft| draft.collection.as_deref()),
                    )
                    .to_vec()
            });
            spawn_grid_page(
                session.clone(),
                spec.clone(),
                start,
                hidden,
                self.available_columns.clone(),
                self.cancellation.clone(),
                self.worker_tx.clone(),
                self.open_generation,
                self.query_generation,
                GridRequestKind::Projection(start),
            );
        }
    }

    fn remember_projection(&mut self, start: usize) {
        self.projected_pages.retain(|old| *old != start);
        self.projected_pages.push_back(start);
        while self.projected_pages.len() > 3 {
            self.projected_pages.pop_front();
        }
    }

    fn start_count(&mut self, updates: &mut Vec<UiUpdate>) {
        let (Some(session), Some(spec)) = (self.session.clone(), self.executed_spec.clone()) else {
            return;
        };
        let sender = self.worker_tx.clone();
        let token = self.cancellation.clone();
        let open_generation = self.open_generation;
        let query_generation = self.query_generation;
        updates.push(UiUpdate::CountStarted);
        thread::spawn(move || {
            let result =
                backend::count_exact(&session, &spec, &token).map_err(|error| error.to_string());
            let _ = sender.send(WorkerEvent::Counted {
                open_generation,
                query_generation,
                result,
            });
        });
    }

    fn start_completion(&mut self, request: CompletionRequest, updates: &mut Vec<UiUpdate>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let key = (
            self.open_generation,
            request.collection.clone(),
            request.field.clone(),
            request.prefix.clone(),
        );
        if let Some((values, partial, visited_keys)) = self.completion_cache.get(&key) {
            updates.push(UiUpdate::CompletionValues {
                revision: request.revision,
                field: request.field,
                prefix: request.prefix,
                values: values.clone(),
                partial: *partial,
                visited_keys: *visited_keys,
            });
            return;
        }
        let sender = self.worker_tx.clone();
        let open_generation = self.open_generation;
        let completion_generation = self.completion_generation;
        thread::spawn(move || {
            let values = posting_completions(&session, &request);
            let _ = sender.send(WorkerEvent::CompletionValues {
                open_generation,
                completion_generation,
                request,
                values,
            });
        });
    }

    fn select_row(&mut self, index: usize, updates: &mut Vec<UiUpdate>) {
        if self.keys.get(index).and_then(Option::as_ref).is_none() {
            return;
        }
        self.selected_index = Some(index);
        updates.push(UiUpdate::RowSelected(index));
    }

    fn emit_tabs(&self, updates: &mut Vec<UiUpdate>) {
        updates.push(UiUpdate::Tabs {
            tabs: self
                .tabs
                .iter()
                .map(|tab| RecordTabSummary {
                    id: tab.id,
                    title: tab.title.clone(),
                    identity: format!("{} / {}", tab.key.collection, tab.key.id),
                    position: tab.anchor + 1,
                    can_back: !tab.back.is_empty(),
                    can_previous: tab.anchor > 0,
                    can_next: !tab.end_confirmed,
                    loading: tab.loading,
                    links_loading: tab.links_pending,
                })
                .collect(),
            active: self.active_tab,
        });
    }

    fn open_record(&mut self, index: usize, updates: &mut Vec<UiUpdate>) {
        let (Some(key), Some(spec), Some(draft)) = (
            self.keys.get(index).cloned().flatten(),
            self.executed_spec.clone(),
            self.executed_draft.clone(),
        ) else {
            return;
        };
        self.select_row(index, updates);
        if let Some(id) = self
            .tabs
            .iter()
            .find(|tab| tab.key == key && tab.context.query_generation == self.query_generation)
            .map(|tab| tab.id)
        {
            self.activate_tab(id, updates);
            return;
        }
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs.push(RecordTab {
            id,
            context: Arc::new(TabContext {
                query_generation: self.query_generation,
                spec,
                draft,
            }),
            key: key.clone(),
            anchor: index,
            back: Vec::new(),
            expanded: HashSet::new(),
            view: TabViewState::default(),
            in_results: true,
            operation_generation: 0,
            cancellation: CancellationToken::new(),
            loading: false,
            links_pending: false,
            links_loaded: false,
            end_confirmed: false,
            title: key.id.clone(),
        });
        self.activate_tab(id, updates);
    }

    fn activate_tab(&mut self, id: u64, updates: &mut Vec<UiUpdate>) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        if self.active_tab == Some(id) && self.detail.is_some() {
            self.emit_tabs(updates);
            return;
        }
        if let Some(old) = self
            .active_tab
            .and_then(|old| self.tabs.iter_mut().find(|tab| tab.id == old))
        {
            old.expanded = self.expanded.clone();
        }
        self.active_tab = Some(id);
        self.detail_generation += 1;
        self.selected_hit = None;
        self.detail = None;
        self.relation_actions.clear();
        self.expanded = self.tabs[index].expanded.clone();
        self.tabs[index].links_loaded = false;
        self.tabs[index].links_pending = false;
        self.emit_tabs(updates);
        self.start_tab_load(id, TabLoadKind::Open, updates);
    }

    fn close_tab(&mut self, id: u64, updates: &mut Vec<UiUpdate>) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        self.tabs[index].cancellation.cancel();
        self.tabs.remove(index);
        if self.active_tab == Some(id) {
            self.active_tab = None;
            self.detail_generation += 1;
            self.selected_hit = None;
            self.detail = None;
            self.relation_actions.clear();
            self.expanded.clear();
            if let Some(next) = self
                .tabs
                .get(index.min(self.tabs.len().saturating_sub(1)))
                .map(|tab| tab.id)
            {
                self.activate_tab(next, updates);
                return;
            }
            updates.push(UiUpdate::Detail(None));
        }
        self.emit_tabs(updates);
    }

    fn navigate_tab(&mut self, forward: bool, updates: &mut Vec<UiUpdate>) {
        let Some(tab) = self
            .active_tab
            .and_then(|id| self.tabs.iter().find(|tab| tab.id == id))
        else {
            return;
        };
        if !forward && tab.anchor == 0 {
            return;
        }
        if forward && tab.end_confirmed {
            return;
        }
        let anchor = if forward {
            tab.anchor + 1
        } else {
            tab.anchor - 1
        };
        self.start_tab_load(tab.id, TabLoadKind::Adjacent { anchor }, updates);
    }

    fn open_tab_query(&mut self, updates: &mut Vec<UiUpdate>) {
        let Some(tab) = self
            .active_tab
            .and_then(|id| self.tabs.iter().find(|tab| tab.id == id))
        else {
            return;
        };
        let draft = tab.context.draft.clone();
        let anchor = tab.anchor;
        updates.push(UiUpdate::RestoreQuery(draft.clone()));
        self.start_query(draft, (anchor / 100) * 100, updates);
        self.pending_reveal = Some(anchor);
    }

    fn start_tab_load(&mut self, id: u64, kind: TabLoadKind, updates: &mut Vec<UiUpdate>) {
        let (Some(session), Some(tab)) = (
            self.session.clone(),
            self.tabs.iter_mut().find(|tab| tab.id == id),
        ) else {
            return;
        };
        tab.cancellation.cancel();
        tab.cancellation = CancellationToken::new();
        tab.operation_generation += 1;
        tab.loading = true;
        tab.links_loaded = false;
        tab.links_pending = false;
        let operation_generation = tab.operation_generation;
        let token = tab.cancellation.clone();
        let spec = tab.context.spec.clone();
        let key = tab.key.clone();
        let tab_anchor = tab.anchor;
        let in_results = tab.in_results;
        let open_generation = self.open_generation;
        let sender = self.worker_tx.clone();
        self.emit_tabs(updates);
        thread::spawn(move || {
            let result = match &kind {
                TabLoadKind::Adjacent { anchor } => {
                    backend::query_page(&session, &spec, *anchor, 1, &token)
                        .map(|mut page| page.hits.pop())
                }
                TabLoadKind::Relation { key } => {
                    backend::record_by_id(&session, &key.collection, &key.id, &token)
                }
                TabLoadKind::Back { frame } => {
                    if frame.in_results {
                        tab_result_hit(&session, &spec, frame.anchor, &frame.key, &token)
                    } else {
                        backend::record_by_id(
                            &session,
                            &frame.key.collection,
                            &frame.key.id,
                            &token,
                        )
                    }
                }
                TabLoadKind::Open => {
                    if in_results {
                        tab_result_hit(&session, &spec, tab_anchor, &key, &token)
                    } else {
                        backend::record_by_id(&session, &key.collection, &key.id, &token)
                    }
                }
            }
            .map(|hit| hit.map(|hit| record_presentation(&session, hit)))
            .map_err(|error| error.to_string());
            let _ = sender.send(WorkerEvent::TabLoaded {
                open_generation,
                tab_id: id,
                operation_generation,
                kind,
                result: Box::new(result),
            });
        });
    }

    fn load_links(&mut self, updates: &mut Vec<UiUpdate>) {
        let (Some(session), Some(hit), Some(id)) = (
            self.session.clone(),
            self.selected_hit.clone(),
            self.active_tab,
        ) else {
            return;
        };
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) else {
            return;
        };
        if tab.links_loaded || tab.links_pending {
            return;
        }
        tab.links_pending = true;
        let operation_generation = tab.operation_generation;
        let token = tab.cancellation.clone();
        let open_generation = self.open_generation;
        let sender = self.worker_tx.clone();
        self.emit_tabs(updates);
        thread::spawn(move || {
            if token.is_cancelled() {
                return;
            }
            let rows = relations::rows(&session.repository, &hit);
            if token.is_cancelled() {
                return;
            }
            let _ = sender.send(WorkerEvent::LinksLoaded {
                open_generation,
                tab_id: id,
                operation_generation,
                rows,
            });
        });
    }

    fn complete(&mut self, event: WorkerEvent, _cx: &mut App, updates: &mut Vec<UiUpdate>) {
        match event {
            WorkerEvent::Validated {
                open_generation,
                validation_generation,
                draft,
                state,
            } if open_generation == self.open_generation
                && validation_generation == self.validation_generation =>
            {
                updates.push(UiUpdate::FilterValidation {
                    generation: validation_generation,
                    state: state.clone(),
                });
                if matches!(state, FilterValidationState::Valid) {
                    self.start_query(draft, 0, updates);
                }
            }
            WorkerEvent::Opened { generation, result } if generation == self.open_generation => {
                match *result {
                    Ok((session, preferences)) => {
                        for tab in &self.tabs {
                            tab.cancellation.cancel();
                        }
                        self.tabs.clear();
                        self.active_tab = None;
                        self.emit_tabs(updates);
                        self.open_draft = self.opening_draft.take();
                        self.executed_spec = None;
                        self.executed_draft = None;
                        self.next_page_start = None;
                        self.pending_continuation = None;
                        self.continuation_failed = false;
                        self.pending_projections.clear();
                        self.projected_pages.clear();
                        self.keys.clear();
                        self.selected_hit = None;
                        self.detail = None;
                        self.relation_actions.clear();
                        self.selected_index = None;
                        self.available_columns.clear();
                        self.preferences = None;
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
                        self.push_field_choices(updates);
                        updates.push(UiUpdate::Opened {
                            root,
                            collections: collections.clone(),
                            single_collection,
                        });
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
                        updates.push(UiUpdate::Diagnostics(self.open_diagnostics.clone()));
                        let mut draft = self.current_draft.clone();
                        draft.collection = if single_collection {
                            collections.first().cloned()
                        } else {
                            None
                        };
                        self.start_query(draft, 0, updates);
                    }
                    Err(message) => {
                        self.opening_draft = None;
                        updates.push(UiUpdate::Error {
                            field: ErrorField::Open,
                            message,
                        });
                    }
                }
            }
            WorkerEvent::Queried {
                open_generation,
                query_generation,
                kind,
                result,
            } if open_generation == self.open_generation
                && query_generation == self.query_generation =>
            {
                match *result {
                    Ok((view, keys, next, elapsed_ns, available, hidden, diagnostics)) => {
                        match kind {
                            GridRequestKind::First => {
                                self.available_columns = available.clone();
                                self.keys = vec![None; view.offset];
                                self.keys.extend(keys.into_iter().map(Some));
                                self.next_page_start = next;
                                self.remember_projection(view.offset);
                                updates.push(UiUpdate::Page(view));
                                if let Some(index) = self.pending_reveal.take() {
                                    self.selected_index = Some(index);
                                    updates.push(UiUpdate::RevealRow(index));
                                }
                                updates.push(UiUpdate::Status(format!(
                                    "Query complete in {:.1} ms",
                                    elapsed_ns as f64 / 1_000_000.0
                                )));
                                if let (Some(preferences), Some(draft)) =
                                    (&mut self.preferences, &self.executed_draft)
                                {
                                    preferences.remember_local(draft);
                                    let _ = self.preference_tx.send(preferences.clone());
                                    updates.push(UiUpdate::History {
                                        entries: preferences.history(),
                                        saved: preferences.saved(),
                                    });
                                }
                            }
                            GridRequestKind::Continuation(start) => {
                                if self.pending_continuation != Some(start)
                                    || start != self.keys.len()
                                {
                                    return;
                                }
                                self.pending_continuation = None;
                                self.continuation_failed = false;
                                self.keys.extend(keys.into_iter().map(Some));
                                self.next_page_start = next;
                                self.remember_projection(start);
                                updates.push(UiUpdate::AppendPage(view));
                            }
                            GridRequestKind::Projection(start) => {
                                if !self.pending_projections.remove(&start) {
                                    return;
                                }
                                for (index, key) in keys.into_iter().enumerate() {
                                    if let Some(slot) = self.keys.get_mut(start + index) {
                                        *slot = Some(key);
                                    }
                                }
                                self.remember_projection(start);
                                updates.push(UiUpdate::Projection(view));
                            }
                        }
                        if self.available_columns != available {
                            self.available_columns = available.clone();
                            self.push_field_choices(updates);
                        }
                        updates.push(UiUpdate::Columns { available, hidden });
                        updates.push(UiUpdate::Diagnostics(format!(
                            "{}\n\n{diagnostics}",
                            self.open_diagnostics
                        )));
                    }
                    Err(message) => match kind {
                        GridRequestKind::First => updates.push(UiUpdate::Error {
                            field: query_error_field(
                                &message,
                                self.executed_draft.as_ref().unwrap_or(&self.current_draft),
                            ),
                            message,
                        }),
                        GridRequestKind::Continuation(start) => {
                            if self.pending_continuation == Some(start) {
                                self.pending_continuation = None;
                                self.continuation_failed = true;
                                updates.push(UiUpdate::ContinuationFailed(message));
                            }
                        }
                        GridRequestKind::Projection(start) => {
                            self.pending_projections.remove(&start);
                            updates.push(UiUpdate::Status(format!(
                                "Could not refresh visible rows: {message}"
                            )));
                        }
                    },
                }
            }
            WorkerEvent::Counted {
                open_generation,
                query_generation,
                result,
            } if open_generation == self.open_generation
                && query_generation == self.query_generation =>
            {
                match result {
                    Ok(total) => updates.push(UiUpdate::ExactCount(total)),
                    Err(message) => updates.push(UiUpdate::Error {
                        field: ErrorField::General,
                        message,
                    }),
                }
            }
            WorkerEvent::CompletionValues {
                open_generation,
                completion_generation,
                request,
                values,
            } if open_generation == self.open_generation
                && completion_generation == self.completion_generation =>
            {
                let key = (
                    open_generation,
                    request.collection.clone(),
                    request.field.clone(),
                    request.prefix.clone(),
                );
                self.completion_cache.insert(key.clone(), values.clone());
                self.completion_cache_order.push_back(key);
                while self.completion_cache_order.len() > 64 {
                    if let Some(old) = self.completion_cache_order.pop_front() {
                        self.completion_cache.remove(&old);
                    }
                }
                updates.push(UiUpdate::CompletionValues {
                    revision: request.revision,
                    field: request.field,
                    prefix: request.prefix,
                    values: values.0,
                    partial: values.1,
                    visited_keys: values.2,
                });
            }
            WorkerEvent::TabLoaded {
                open_generation,
                tab_id,
                operation_generation,
                kind,
                result,
            } if open_generation == self.open_generation => {
                let Some(index) = self.tabs.iter().position(|tab| {
                    tab.id == tab_id && tab.operation_generation == operation_generation
                }) else {
                    return;
                };
                self.tabs[index].loading = false;
                match *result {
                    Ok(Some((hit, view, actions))) => {
                        let tab = &mut self.tabs[index];
                        match &kind {
                            TabLoadKind::Adjacent { anchor } => {
                                if tab.back.len() >= 32 {
                                    tab.back.remove(0);
                                }
                                tab.back.push(TabFrame {
                                    key: tab.key.clone(),
                                    anchor: tab.anchor,
                                    view: tab.view,
                                    in_results: tab.in_results,
                                });
                                tab.anchor = *anchor;
                                tab.in_results = true;
                                tab.end_confirmed = false;
                                tab.expanded.clear();
                                tab.links_loaded = false;
                            }
                            TabLoadKind::Relation { .. } => {
                                if tab.back.len() >= 32 {
                                    tab.back.remove(0);
                                }
                                tab.back.push(TabFrame {
                                    key: tab.key.clone(),
                                    anchor: tab.anchor,
                                    view: tab.view,
                                    in_results: tab.in_results,
                                });
                                tab.expanded.clear();
                                tab.in_results = false;
                                tab.links_loaded = false;
                            }
                            TabLoadKind::Back { frame } => {
                                if tab.back.last().is_some_and(|last| {
                                    last.key == frame.key && last.anchor == frame.anchor
                                }) {
                                    tab.back.pop();
                                }
                                tab.anchor = frame.anchor;
                                tab.view = frame.view;
                                tab.in_results = frame.in_results;
                                tab.expanded.clear();
                                tab.links_loaded = false;
                            }
                            TabLoadKind::Open => {}
                        }
                        tab.key = RecordKey {
                            collection: hit.collection.clone(),
                            id: hit.record_id.clone(),
                        };
                        tab.title = view.title.clone();
                        if self.active_tab == Some(tab_id) {
                            self.selected_hit = Some(Arc::new(hit));
                            self.detail = Some(view.clone());
                            self.relation_actions = actions;
                            self.expanded = tab.expanded.clone();
                            updates.push(UiUpdate::Detail(Some(view)));
                            if matches!(kind, TabLoadKind::Back { .. }) {
                                updates.push(UiUpdate::RestoreTabView(tab.view));
                            }
                        }
                    }
                    Ok(None) => {
                        if matches!(kind, TabLoadKind::Adjacent { .. }) {
                            self.tabs[index].end_confirmed = true;
                            updates.push(UiUpdate::Status("End of results".into()));
                        } else {
                            updates.push(UiUpdate::Error {
                                field: ErrorField::General,
                                message: "Record is unavailable".into(),
                            });
                        }
                    }
                    Err(message) => updates.push(UiUpdate::Error {
                        field: ErrorField::General,
                        message,
                    }),
                }
                self.emit_tabs(updates);
            }
            WorkerEvent::LinksLoaded {
                open_generation,
                tab_id,
                operation_generation,
                rows,
            } if open_generation == self.open_generation => {
                let Some(tab) = self.tabs.iter_mut().find(|tab| {
                    tab.id == tab_id && tab.operation_generation == operation_generation
                }) else {
                    return;
                };
                tab.links_pending = false;
                tab.links_loaded = true;
                if self.active_tab == Some(tab_id) {
                    let (relations, actions) = relation_presentation(rows);
                    if let Some(detail) = &mut self.detail {
                        detail.relations = relations;
                        self.relation_actions = actions;
                        updates.push(UiUpdate::Detail(Some(detail.clone())));
                    }
                }
                self.emit_tabs(updates);
            }
            WorkerEvent::Detected { generation, result }
                if generation == self.detection_generation =>
            {
                updates.push(UiUpdate::OpenDetection { generation, result });
            }
            WorkerEvent::PickedFolder {
                generation,
                slot,
                result,
            } if generation == self.picker_generation => match result {
                Ok(Some(paths)) => {
                    if let Some(path) = paths.into_iter().next() {
                        updates.push(UiUpdate::ChosenFolder {
                            generation,
                            slot,
                            path: path.display().to_string(),
                        });
                    }
                }
                Ok(None) => {}
                Err(message) => updates.push(UiUpdate::Error {
                    field: ErrorField::Open,
                    message: format!("Folder chooser failed: {message}. Enter a path instead."),
                }),
            },
            WorkerEvent::PickedSchema { generation, result }
                if generation == self.picker_generation =>
            {
                match result {
                    Ok(Some(paths)) => {
                        if let Some(path) = paths.into_iter().next() {
                            updates.push(UiUpdate::ChosenSchema {
                                generation,
                                path: path.display().to_string(),
                            });
                        }
                    }
                    Ok(None) => {}
                    Err(message) => updates.push(UiUpdate::Error {
                        field: ErrorField::Open,
                        message: format!("Schema chooser failed: {message}. Enter a path instead."),
                    }),
                }
            }
            WorkerEvent::Expanded {
                open_generation,
                detail_generation,
                view,
            } if open_generation == self.open_generation
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
            WorkerEvent::Indexed {
                open_generation,
                output_generation,
                result,
            } if open_generation == self.open_generation
                && output_generation == self.output_generation =>
            {
                match result {
                    Ok(message) => updates.push(UiUpdate::Status(message)),
                    Err(message) => updates.push(UiUpdate::Error {
                        field: ErrorField::General,
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
            WorkerEvent::PreferenceError(message) => {
                updates.push(UiUpdate::PreferenceFailed(message))
            }
            WorkerEvent::PreferenceSaved => updates.push(UiUpdate::PreferenceSaved),
            WorkerEvent::CopiedValue {
                open_generation,
                detail_generation,
                result,
            } if open_generation == self.open_generation
                && detail_generation == self.detail_generation =>
            {
                match result {
                    Ok((value, message)) => {
                        updates.push(UiUpdate::Clipboard(value));
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
        match action {
            LinkAction::Target { collection, id } => {
                let Some(tab_id) = self.active_tab else {
                    return;
                };
                updates.push(UiUpdate::Status(format!(
                    "Following relation to {collection} / {id}…"
                )));
                self.start_tab_load(
                    tab_id,
                    TabLoadKind::Relation {
                        key: RecordKey { collection, id },
                    },
                    updates,
                );
            }
            continuation => {
                let Some(session) = self.session.clone() else {
                    return;
                };
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

    fn open_relation_tab(&mut self, index: usize, updates: &mut Vec<UiUpdate>) {
        let Some(LinkAction::Target { collection, id }) =
            self.relation_actions.get(index).cloned().flatten()
        else {
            return;
        };
        let Some(origin) = self
            .active_tab
            .and_then(|active| self.tabs.iter().find(|tab| tab.id == active))
        else {
            return;
        };
        let context = origin.context.clone();
        let anchor = origin.anchor;
        let key = RecordKey { collection, id };
        if let Some(existing) = self
            .tabs
            .iter()
            .find(|tab| tab.key == key && Arc::ptr_eq(&tab.context, &context))
            .map(|tab| tab.id)
        {
            self.activate_tab(existing, updates);
            return;
        }
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs.push(RecordTab {
            id,
            context,
            key: key.clone(),
            anchor,
            back: Vec::new(),
            expanded: HashSet::new(),
            view: TabViewState::default(),
            in_results: false,
            operation_generation: 0,
            cancellation: CancellationToken::new(),
            loading: false,
            links_pending: false,
            links_loaded: false,
            end_confirmed: false,
            title: key.id.clone(),
        });
        self.activate_tab(id, updates);
    }

    fn start_export(&mut self, format: ExportFormat, path: String, updates: &mut Vec<UiUpdate>) {
        let Some(session) = self.session.clone() else {
            updates.push(UiUpdate::Error {
                field: ErrorField::Export,
                message: "Open a repository first".into(),
            });
            return;
        };
        if format != ExportFormat::SelectedJson
            && !matches!(
                validate_request(&session, &self.current_draft),
                FilterValidationState::Valid
            )
        {
            updates.push(UiUpdate::Error {
                field: ErrorField::Export,
                message: "Finish or clear the current filter before exporting".into(),
            });
            return;
        }
        let path = PathBuf::from(path.trim());
        if path.as_os_str().is_empty() {
            updates.push(UiUpdate::Error {
                field: ErrorField::Export,
                message: "Enter a new output path".into(),
            });
            return;
        }
        let spec = if format == ExportFormat::SelectedJson {
            None
        } else {
            match query_spec(&self.current_draft) {
                Ok(spec) => Some(spec),
                Err(message) => {
                    updates.push(UiUpdate::Error {
                        field: ErrorField::Export,
                        message,
                    });
                    return;
                }
            }
        };
        let hit = self.selected_hit.clone();
        let open_generation = self.open_generation;
        self.output_cancellation.cancel();
        self.output_cancellation = CancellationToken::new();
        self.output_generation += 1;
        let output_generation = self.output_generation;
        let token = self.output_cancellation.clone();
        let sender = self.worker_tx.clone();
        updates.push(UiUpdate::ClearError(ErrorField::Export));
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

    fn start_index(&mut self, mode: SnapshotMode, directory: String, updates: &mut Vec<UiUpdate>) {
        let Some(mut draft) = self.open_draft.clone() else {
            updates.push(UiUpdate::Error {
                field: ErrorField::General,
                message: "Open a repository before running an index operation".into(),
            });
            return;
        };
        draft.snapshot = mode;
        draft.snapshot_directory = directory;
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let spec = match open_spec(&draft, &cwd) {
            Ok(spec) => spec,
            Err(message) => {
                updates.push(UiUpdate::Error {
                    field: ErrorField::General,
                    message,
                });
                return;
            }
        };
        self.output_cancellation.cancel();
        self.output_cancellation = CancellationToken::new();
        self.output_generation += 1;
        let output_generation = self.output_generation;
        let open_generation = self.open_generation;
        let token = self.output_cancellation.clone();
        let sender = self.worker_tx.clone();
        updates.push(UiUpdate::Status("Running index operation…".into()));
        thread::spawn(move || {
            let result = backend::open(&spec, &token)
                .map(|session| {
                    format!(
                        "Index operation complete for {} ({:.1} ms)",
                        session.paths.root.display(),
                        session.diagnostics.open_ns as f64 / 1_000_000.0
                    )
                })
                .map_err(|error| error.to_string());
            let _ = sender.send(WorkerEvent::Indexed {
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
        updates.push(UiUpdate::ClearError(ErrorField::Batch));
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
    _session: &OpenedSession,
    hit: SearchHit,
) -> (SearchHit, DetailView, Vec<Option<LinkAction>>) {
    let view = detail_view(&hit, &HashSet::new());
    (hit, view, Vec::new())
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
            let target = matches!(row.action.as_ref(), Some(LinkAction::Target { .. }));
            (
                RelationRow {
                    label: row.label,
                    value: row.value,
                    available,
                    continuation,
                    target,
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

/// Query already-built posting dictionaries only. Missing or partial indexes
/// yield fewer suggestions; they never trigger a source-document scan.
fn tab_result_hit(
    session: &OpenedSession,
    spec: &backend::QuerySpec,
    anchor: usize,
    key: &RecordKey,
    token: &CancellationToken,
) -> Result<Option<SearchHit>, backend::BackendError> {
    let mut page = backend::query_page(session, spec, anchor, 1, token)?;
    if let Some(hit) = page.hits.pop()
        && hit.collection == key.collection
        && hit.record_id == key.id
    {
        return Ok(Some(hit));
    }
    backend::record_by_id(session, &key.collection, &key.id, token)
}

fn posting_completions(
    session: &OpenedSession,
    request: &CompletionRequest,
) -> CompletionCacheValue {
    let mut values = Vec::new();
    let mut partial = false;
    let mut visited_keys = 0;
    let path = match objekte_search_core::model::FieldPath::parse(&request.field) {
        Ok(path) => path,
        Err(_) => return (values, true, visited_keys),
    };
    for collection in session.repository.collections() {
        if request
            .collection
            .as_deref()
            .is_some_and(|scope| scope != collection.definition.name)
        {
            continue;
        }
        // A transformed posting key need not be a valid source literal. The
        // public posting API intentionally exposes stored keys, not inverse
        // transforms, so withhold custom-bound fields.
        if collection
            .definition
            .schema
            .custom_bindings()
            .keys()
            .any(|(segments, _)| segments == &path.segments)
        {
            partial = true;
            continue;
        }
        let mut query = PostingValuesRequest::new(&collection.definition.name, &request.field, 20);
        if !request.prefix.is_empty() {
            query.prefix = Some(request.prefix.clone());
        }
        let page = match session.repository.posting_values(&query) {
            Ok(page) => page,
            Err(_) => {
                // Numeric and boolean dictionaries do not support native
                // prefixes. A bounded first page is still useful.
                query.prefix = None;
                match session.repository.posting_values(&query) {
                    Ok(page) => page,
                    Err(_) => {
                        partial = true;
                        continue;
                    }
                }
            }
        };
        visited_keys += page.visited_keys;
        partial |= !page.coverage.value_counts_complete || page.continuation.is_some();
        for value in page.values {
            let Some(literal) = scalar_literal(&value.value) else {
                continue;
            };
            if !request.prefix.is_empty()
                && !literal
                    .to_lowercase()
                    .contains(&request.prefix.to_lowercase())
            {
                continue;
            }
            let label = if request.collection.is_some() {
                literal.clone()
            } else {
                format!("{literal} · {}", collection.definition.name)
            };
            if !values.iter().any(|(existing, _)| existing == &literal) {
                values.push((literal, label));
            }
            if values.len() >= 50 {
                partial = true;
                break;
            }
        }
        if values.len() >= 50 {
            break;
        }
    }
    (values, partial, visited_keys)
}

fn scalar_literal(value: &ScalarKey) -> Option<String> {
    use objekte_search_core::model::Number;
    Some(match value {
        ScalarKey::String(value) | ScalarKey::Id(value) => serde_json::to_string(value).ok()?,
        ScalarKey::Number(Number::Integer(value)) => value.to_string(),
        ScalarKey::Number(Number::Float(value)) => value.to_string(),
        ScalarKey::Number(Number::ExactDecimal(value)) => value.to_string(),
        ScalarKey::Bool(value) => value.to_string(),
        ScalarKey::Date(value) => serde_json::to_string(&value.to_string()).ok()?,
        ScalarKey::Datetime(value) => serde_json::to_string(&value.to_string()).ok()?,
        ScalarKey::Unixtime(value) => value.to_string(),
    })
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
