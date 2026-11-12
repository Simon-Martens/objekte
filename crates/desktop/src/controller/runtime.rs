//! Generation-checked operations around one native window and retained repository.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use gpui_kit::{App, PathPromptOptions};
use objekte_cli::execution::Exit;
use objekte_search_core::lifecycle::{
    ExtendedPhase, IndexRevision, IndexStatus, IndexView, MaintenanceOperation, MaintenanceTask,
    ReconciliationState, ReconciliationTask, StagedBuildPhase, StagedBuildStatus,
    StagedBuildTask, StagedFile,
};
use objekte_search_core::posting::ScalarKey;
use objekte_search_core::posting_values::PostingValuesRequest;
use objekte_search_core::repository::SearchHit;
use objekte_search_core::run::DirectLookupResult;
use objekte_search_core::streaming::CancellationToken;

use crate::backend::{self, OpenSpec, OpenedSession};
use crate::ui::model::{
    BatchRow, BatchSource as UiBatchSource, CatalogueFile, CompletionOrigin, DetailView, DirectoryCatalog, FileCatalogue,
    ErrorField, ExportFormat, FilterValidationState, IndexTaskKind, IndexTaskState, IndexUiStatus,
    OpenDraft, PageView,
    RecordTabSummary, RelationRow, SearchDraft, SnapshotMode, TabFreshness, TabViewState, UiAction,
    UiUpdate,
};
use crate::ui::preferences::DesktopPreferences;

use super::filter_validation::{field_choices, validate_request};
use super::open_detection::{DetectedSource, detect_source};
use super::projection::detail_view_with_query;
use super::relations::{self, LinkAction};
use super::{initial_columns, open_spec, page_view_with_prior, query_spec};

/// One lightweight locator retained for every delivered result.
#[derive(Clone, Eq, PartialEq)]
struct RecordKey {
    collection: String,
    id: String,
}

#[derive(Clone)]
struct TabContext {
    publication: Publication,
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
    pinned: bool,
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
    freshness: TabFreshness,
    navigation_outdated: bool,
    cached_detail: Option<DetailView>,
}

/// Source publications are comparable only within one live index instance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Publication {
    instance_id: u64,
    revision: IndexRevision,
}

impl From<&IndexStatus> for Publication {
    fn from(status: &IndexStatus) -> Self {
        Self {
            instance_id: status.instance_id,
            revision: status.revision,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CheckOrigin {
    Initial,
    Explicit,
    Periodic,
}

#[derive(Clone)]
enum TabLoadKind {
    Open,
    Refresh,
    Replace {
        key: RecordKey,
        context: Arc<TabContext>,
        anchor: usize,
    },
    Adjacent {
        anchor: usize,
    },
    Relation {
        key: RecordKey,
    },
    Back {
        frame: TabFrame,
    },
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
type OpenCompletion = Result<
    (
        OpenedSession,
        Result<DesktopPreferences, String>,
        Publication,
        DirectoryCatalog,
    ),
    String,
>;
type TabCompletion = Result<Option<(SearchHit, DetailView, Vec<Option<LinkAction>>)>, String>;

enum IndexSaveCompletion {
    Saved(backend::index_lifecycle::SaveOutcome),
    OverwriteRequired(PathBuf),
    Failed(String),
}

#[derive(Clone)]
struct CompletionRequest {
    origin: CompletionOrigin,
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
    view: IndexView,
    spec: backend::QuerySpec,
    page_start: usize,
    hidden: Vec<String>,
    shown_nested: Vec<String>,
    prior_columns: Vec<String>,
    token: CancellationToken,
    sender: Sender<WorkerEvent>,
    open_generation: u64,
    query_generation: u64,
    kind: GridRequestKind,
) {
    thread::spawn(move || {
        let publication = Publication {
            instance_id: view.instance_id(),
            revision: view.revision(),
        };
        let result = backend::query_page_on_view(&session, &view, &spec, page_start, 100, &token)
            .map(|page| {
                let diagnostics = format!(
                    "Query: {:.1} ms; page start {}; rows {}; exact total {:?}\nCounters: {:#?}",
                    page.elapsed_ns as f64 / 1_000_000.0,
                    page.page_start,
                    page.hits.len(),
                    page.count,
                    page.counters,
                );
                let (view, available, hidden) =
                    page_view_with_prior(&page, &spec, &hidden, &shown_nested, &prior_columns);
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
            publication,
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
    StagedStarted {
        generation: u64,
        task: StagedBuildTask,
    },
    StagedProgress {
        generation: u64,
        status: StagedBuildStatus,
    },
    CatalogueReady {
        generation: u64,
        root: PathBuf,
        collections: Vec<String>,
        directories: DirectoryCatalog,
        file_catalogue: FileCatalogue,
    },
    Directories {
        open_generation: u64,
        publication: Publication,
        directories: DirectoryCatalog,
    },
    Queried {
        open_generation: u64,
        query_generation: u64,
        publication: Publication,
        kind: GridRequestKind,
        result: Box<QueryCompletion>,
    },
    Counted {
        open_generation: u64,
        query_generation: u64,
        publication: Publication,
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
        publication: Publication,
        tab_id: u64,
        operation_generation: u64,
        kind: TabLoadKind,
        result: Box<TabCompletion>,
    },
    LinksLoaded {
        open_generation: u64,
        publication: Publication,
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
    PickedIndexPath {
        generation: u64,
        load: bool,
        run_after_selection: bool,
        result: Result<Option<PathBuf>, String>,
    },
    Exported {
        open_generation: u64,
        export_generation: u64,
        result: Result<String, String>,
    },
    IndexSaved {
        open_generation: u64,
        operation_id: u64,
        make_default: bool,
        result: IndexSaveCompletion,
    },
    IndexLoaded {
        open_generation: u64,
        operation_id: u64,
        result: Result<OpenedSession, String>,
    },
    IndexShutdown {
        open_generation: u64,
        operation_id: u64,
        result: Result<(), String>,
    },
    IndexCloseCancelled {
        open_generation: u64,
        operation_id: u64,
        result: Result<(), String>,
    },
    AutomaticCheckChanged {
        open_generation: u64,
        operation_id: u64,
        enabled: bool,
        interval_seconds: u64,
        result: Result<(), String>,
    },
    AutomaticSaveChanged {
        open_generation: u64,
        operation_id: u64,
        enabled: bool,
        result: Result<(), String>,
    },
    Batched {
        open_generation: u64,
        batch_generation: u64,
        result: Result<(backend::BatchReport, Vec<BatchRow>), String>,
    },
    PreferenceError(String),
    PreferenceSaved,
    CopiedValue {
        open_generation: u64,
        publication: Publication,
        tab_id: u64,
        operation_generation: u64,
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
    observed_publication: Option<Publication>,
    pending_directories: Option<Publication>,
    executed_view: Option<IndexView>,
    results_outdated: bool,
    refreshing_results: bool,
    refresh_anchor: Option<RecordKey>,
    pending_outdated_open: Option<(RecordKey, bool)>,
    check_task: Option<ReconciliationTask>,
    check_origin: Option<CheckOrigin>,
    maintenance_task: Option<MaintenanceTask>,
    last_index_poll: Instant,
    index_ui: IndexUiStatus,
    last_index_status_sent: Option<IndexUiStatus>,
    index_operation_id: u64,
    save_operation_id: u64,
    load_operation_id: u64,
    close_operation_id: u64,
    closing: bool,
    automatic_check_operation_id: u64,
    automatic_check_epoch: Arc<AtomicU64>,
    automatic_check_gate: Arc<Mutex<()>>,
    requested_check: (bool, u64),
    automatic_save_operation_id: u64,
    automatic_save_epoch: Arc<AtomicU64>,
    automatic_save_gate: Arc<Mutex<()>>,
    load_cancellation: CancellationToken,
    pending_index_overwrite: Option<PathBuf>,
    pending_save_select_default: bool,
    preferences: Option<DesktopPreferences>,
    open_diagnostics: String,
    open_draft: Option<OpenDraft>,
    opening_draft: Option<OpenDraft>,
    opening_task: Option<StagedBuildTask>,
    catalogue_opened: bool,
    detection_generation: u64,
    picker_generation: u64,
    index_picker_generation: u64,
    pending_open_generation: u64,
    open_generation: u64,
    query_generation: u64,
    validation_generation: u64,
    detail_generation: u64,
    export_generation: u64,
    batch_generation: u64,
    cancellation: CancellationToken,
    open_cancellation: CancellationToken,
    export_cancellation: CancellationToken,
    batch_cancellation: CancellationToken,
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
    completion_token: CancellationToken,
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
                observed_publication: None,
                pending_directories: None,
                executed_view: None,
                results_outdated: false,
                refreshing_results: false,
                refresh_anchor: None,
                pending_outdated_open: None,
                check_task: None,
                check_origin: None,
                maintenance_task: None,
                last_index_poll: Instant::now(),
                index_ui: IndexUiStatus::default(),
                last_index_status_sent: None,
                index_operation_id: 0,
                save_operation_id: 0,
                load_operation_id: 0,
                close_operation_id: 0,
                closing: false,
                automatic_check_operation_id: 0,
                automatic_check_epoch: Arc::new(AtomicU64::new(0)),
                automatic_check_gate: Arc::new(Mutex::new(())),
                requested_check: (false, 30),
                automatic_save_operation_id: 0,
                automatic_save_epoch: Arc::new(AtomicU64::new(0)),
                automatic_save_gate: Arc::new(Mutex::new(())),
                load_cancellation: CancellationToken::new(),
                pending_index_overwrite: None,
                pending_save_select_default: false,
                preferences: None,
                open_diagnostics: String::new(),
                open_draft: None,
                opening_draft: None,
                opening_task: None,
                catalogue_opened: false,
                detection_generation: 0,
                picker_generation: 0,
                index_picker_generation: 0,
                pending_open_generation: 0,
                open_generation: 0,
                query_generation: 0,
                validation_generation: 0,
                detail_generation: 0,
                export_generation: 0,
                batch_generation: 0,
                cancellation: CancellationToken::new(),
                open_cancellation: CancellationToken::new(),
                export_cancellation: CancellationToken::new(),
                batch_cancellation: CancellationToken::new(),
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
                completion_token: CancellationToken::new(),
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
        if self.last_index_poll.elapsed() >= Duration::from_millis(100) {
            self.last_index_poll = Instant::now();
            self.poll_publication(&mut updates);
            self.poll_index_tasks(&mut updates);
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
        if let Some(result) = self
            .session
            .as_ref()
            .and_then(OpenedSession::take_background_snapshot_save)
        {
            match result {
                Ok(Some(stats)) => {
                    self.open_diagnostics
                        .push_str(&format!("\nBackground index save: {stats:#?}"));
                    updates.push(UiUpdate::Diagnostics(self.open_diagnostics.clone()));
                }
                Ok(None) => {
                    self.open_diagnostics
                        .push_str("\nBackground index save: already current");
                    updates.push(UiUpdate::Diagnostics(self.open_diagnostics.clone()));
                }
                Err(message) => updates.push(UiUpdate::Error {
                    field: ErrorField::Open,
                    message: format!("Cache write after reconciliation failed: {message}"),
                }),
            }
        }
        updates
    }

    fn poll_publication(&mut self, updates: &mut Vec<UiUpdate>) {
        let Some(session) = &self.session else {
            return;
        };
        let Ok(status) = session.repository.index().status() else {
            return;
        };
        let current = Publication::from(&status);
        match self.observed_publication {
            None => self.observed_publication = Some(current),
            Some(previous) if previous != current => {
                self.observed_publication = Some(current);
                self.invalidate_published_contexts(updates);
                self.request_directories();
            }
            Some(_) => {}
        }
    }

    fn request_directories(&mut self) {
        if self.pending_directories.is_some() {
            return;
        }
        let Some(session) = &self.session else {
            return;
        };
        let Ok(view) = session.repository.index().view() else {
            return;
        };
        let publication = Publication {
            instance_id: view.instance_id(),
            revision: view.revision(),
        };
        self.pending_directories = Some(publication);
        let sender = self.worker_tx.clone();
        let open_generation = self.open_generation;
        let collections = session.collections.clone();
        thread::spawn(move || {
            let directories = indexed_directories(&view, &collections);
            let _ = sender.send(WorkerEvent::Directories {
                open_generation,
                publication,
                directories,
            });
        });
    }

    fn publication_is_current(&self, publication: Publication) -> bool {
        self.executed_view.as_ref().is_some_and(|view| {
            view.instance_id() == publication.instance_id && view.revision() == publication.revision
        }) && self.session.as_ref().is_some_and(|session| {
            session
                .repository
                .index()
                .status()
                .is_ok_and(|status| Publication::from(&status) == publication)
        })
    }

    fn session_publication_is_current(&self, publication: Publication) -> bool {
        self.session.as_ref().is_some_and(|session| {
            session
                .repository
                .index()
                .status()
                .is_ok_and(|status| Publication::from(&status) == publication)
        })
    }

    fn update_index_ui(&mut self, status: &IndexStatus, updates: &mut Vec<UiUpdate>) {
        let Some(session) = &self.session else {
            return;
        };
        self.index_ui.automatic_check = session
            .repository
            .index()
            .periodic_reconciliation_enabled()
            .unwrap_or(false);
        self.index_ui.source_location = session.paths.root.display().to_string();
        self.index_ui.opening_mode = if session.configuration.configured_repository {
            "Configured repository".into()
        } else if session.args.multiple_collections() {
            "Multiple folders".into()
        } else {
            "Plain folder".into()
        };
        self.index_ui.loaded_from = session
            .diagnostics
            .startup
            .restored
            .then(|| session.configuration.cache_load.as_ref())
            .flatten()
            .map(|path| path.display().to_string());
        self.index_ui.save_destination = session
            .configuration
            .cache_save
            .as_ref()
            .map(|path| path.display().to_string());
        self.index_ui.schema_paths = session
            .diagnostics
            .schema_files
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        self.index_ui.indexed_files = Some(status.indexed_files);
        self.index_ui.live_records = usize::try_from(status.live_records).ok();
        self.index_ui.index_instance = status.instance_id;
        self.index_ui.revision = status.revision.0;
        self.index_ui.capability_revision = status.capability_revision.0;
        self.index_ui.saved_revision = status.saved_revision.map(|revision| revision.0);
        self.index_ui.saved_capability_revision = status.saved_capability_revision.map(|revision| revision.0);
        self.index_ui.dirty = status.dirty();
        if let Ok(extended) = session.repository.index().extended_status() {
            self.index_ui.extended_phase = match extended.phase {
                ExtendedPhase::Unnecessary => "Not requested",
                ExtendedPhase::Pending => "Pending",
                ExtendedPhase::Building => "Building",
                ExtendedPhase::Paused => "Paused",
                ExtendedPhase::Complete => "Complete",
                ExtendedPhase::Cancelled => "Cancelled",
                ExtendedPhase::Failed => "Failed",
            }.into();
            self.index_ui.extended = match extended.phase {
                ExtendedPhase::Pending | ExtendedPhase::Building => IndexTaskState::Running {
                    operation_id: status.instance_id,
                    processed: Some(extended.completed_files as u64),
                    total: Some(extended.total_files as u64),
                },
                ExtendedPhase::Complete => IndexTaskState::Succeeded("Field indexes complete".into()),
                ExtendedPhase::Paused => IndexTaskState::Cancelled,
                ExtendedPhase::Cancelled => IndexTaskState::Cancelled,
                ExtendedPhase::Failed => IndexTaskState::Failed(
                    extended.error.unwrap_or_else(|| "Field indexing failed".into()),
                ),
                ExtendedPhase::Unnecessary => IndexTaskState::Idle,
            };
        }
        self.index_ui.check_complete = match status.reconciliation {
            ReconciliationState::Complete => Some(true),
            ReconciliationState::Partial | ReconciliationState::Cancelled => Some(false),
            _ => None,
        };
        self.index_ui.automatic_save_available = session.configuration.configured_repository
            && session.configuration.cache_save.is_some();
        self.index_ui.automatic_save_reason = (!self.index_ui.automatic_save_available).then(|| {
            "Automatic saving is available for configured repositories with a selected index destination".into()
        });
        if let Ok(Some(error)) = backend::index_lifecycle::automatic_save_error(session)
            && !matches!(self.index_ui.save, IndexTaskState::Running { .. })
        {
            self.index_ui.save = IndexTaskState::Failed(error);
        }
        if self.last_index_status_sent.as_ref() != Some(&self.index_ui) {
            self.last_index_status_sent = Some(self.index_ui.clone());
            updates.push(UiUpdate::IndexStatus(self.index_ui.clone()));
        }
    }

    fn invalidate_published_contexts(&mut self, updates: &mut Vec<UiUpdate>) {
        if self.executed_spec.is_some() {
            self.results_outdated = true;
            self.cancellation.cancel();
            self.query_generation = self.query_generation.wrapping_add(1);
            self.next_page_start = None;
            self.pending_continuation = None;
            self.pending_projections.clear();
            self.projected_pages.clear();
            self.executed_view = None;
            updates.push(UiUpdate::ResultsOutdated(true));
        }
        self.detail_generation = self.detail_generation.wrapping_add(1);
        self.selected_hit = None;
        self.relation_actions.clear();
        for tab in &mut self.tabs {
            tab.cancellation.cancel();
            tab.operation_generation = tab.operation_generation.wrapping_add(1);
            tab.freshness = TabFreshness::Outdated;
            tab.navigation_outdated = true;
            tab.loading = false;
            tab.links_pending = false;
            tab.links_loaded = false;
        }
        if !self.tabs.is_empty() {
            self.emit_tabs(updates);
        }
    }

    fn poll_index_tasks(&mut self, updates: &mut Vec<UiUpdate>) {
        let Some(session) = &self.session else {
            return;
        };
        let index = session.repository.index().clone();
        if self.check_task.is_none()
            && matches!(
                index.status().map(|status| status.reconciliation),
                Ok(ReconciliationState::Pending | ReconciliationState::Running)
            )
        {
            self.check_task = index.current_reconciliation_task().ok().flatten();
            self.check_origin = self.check_task.as_ref().map(|_| CheckOrigin::Periodic);
        }
        if let Some(task) = &self.check_task
            && let Ok(progress) = task.progress()
            && !task.is_complete().unwrap_or(false)
        {
            self.index_ui.check = IndexTaskState::Running {
                operation_id: self.index_operation_id,
                processed: Some(progress.examined as u64),
                total: None,
            };
        }
        let completed = self
            .check_task
            .as_ref()
            .and_then(|task| task.is_complete().ok())
            .unwrap_or(false);
        if completed {
            let origin = self.check_origin.take();
            let task = self.check_task.take().expect("completed check task");
            match task.wait() {
                Ok(report) => {
                    if report.cancelled {
                        self.index_ui.check = IndexTaskState::Cancelled;
                        updates.push(UiUpdate::Status("File check stopped".into()));
                    } else if !report.issues.is_empty() || report.omitted_issues > 0 {
                        self.index_ui.check = IndexTaskState::Failed(format!(
                            "Some files could not be checked ({} issues)",
                            report.issues.len() + report.omitted_issues
                        ));
                        self.index_ui.last_check = Some("Partial check this session".into());
                        updates.push(UiUpdate::Status("Some files could not be checked".into()));
                    } else {
                        self.index_ui.check = IndexTaskState::Succeeded(format!(
                            "Files checked: {} examined, {} changed",
                            report.examined,
                            report.added + report.updated + report.removed
                        ));
                        self.index_ui.last_check = Some("Checked this session".into());
                    }
                    if !report.cancelled
                        && matches!(origin, Some(CheckOrigin::Initial | CheckOrigin::Explicit))
                    {
                        self.refresh_results_if_ready(updates);
                    }
                }
                Err(error) => {
                    self.index_ui.check = IndexTaskState::Failed(error.to_string());
                    updates.push(UiUpdate::Error {
                        field: ErrorField::General,
                        message: format!("File check failed: {error}"),
                    });
                }
            }
        }
        if let Some(task) = &self.maintenance_task
            && let Ok(progress) = task.progress()
            && !task.is_complete().unwrap_or(false)
        {
            self.index_ui.maintenance = IndexTaskState::Running {
                operation_id: self.index_operation_id,
                processed: Some(progress.attempt as u64),
                total: None,
            };
        }
        let maintenance_done = self
            .maintenance_task
            .as_ref()
            .and_then(|task| task.is_complete().ok())
            .unwrap_or(false);
        if maintenance_done {
            let task = self.maintenance_task.take().expect("completed maintenance");
            match task.wait() {
                Ok(report) => {
                    self.index_ui.maintenance = IndexTaskState::Succeeded(match report {
                        objekte_search_core::lifecycle::MaintenanceReport::Compact(ref compact) => {
                            format!(
                                "Index compacted: {} obsolete slots removed",
                                compact.retired_slots_removed
                            )
                        }
                        objekte_search_core::lifecycle::MaintenanceReport::Rebuild(ref rebuild) => {
                            format!("Index rebuilt: {} files", rebuild.indexed_files)
                        }
                    });
                    if let Some(session) = &self.session {
                        match session.refresh_base_metadata() {
                            Ok(refreshed) => self.session = Some(refreshed),
                            Err(error) => {
                                self.index_ui.maintenance =
                                    IndexTaskState::Failed(error.to_string())
                            }
                        }
                    }
                    self.refresh_results_if_ready(updates);
                }
                Err(error) => {
                    self.index_ui.maintenance = if matches!(
                        &error,
                        objekte_search_core::lifecycle::IndexError::Cancelled
                    ) {
                        IndexTaskState::Cancelled
                    } else {
                        IndexTaskState::Failed(error.to_string())
                    };
                }
            }
        }
        if let Ok(status) = index.status() {
            self.update_index_ui(&status, updates);
        }
    }

    fn refresh_results_if_ready(&mut self, updates: &mut Vec<UiUpdate>) {
        if !self.results_outdated || self.refreshing_results {
            return;
        }
        let Some(draft) = self.executed_draft.clone() else {
            return;
        };
        if self.current_draft != draft {
            return;
        }
        let Some(session) = &self.session else {
            return;
        };
        if !matches!(
            validate_request(session, &draft),
            FilterValidationState::Valid
        ) {
            return;
        }
        self.start_query_impl(draft, 0, true, false, updates);
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
                origin,
                revision,
                collection,
                field,
                prefix,
            } => {
                self.completion_token.cancel();
                self.completion_token = CancellationToken::new();
                self.completion_generation += 1;
                self.pending_completion = Some((
                    Instant::now(),
                    CompletionRequest {
                        origin,
                        revision,
                        collection,
                        field,
                        prefix,
                    },
                ));
            }
            UiAction::Cancel => {
                self.cancellation.cancel();
                self.query_generation += 1;
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
            UiAction::CancelBatch => {
                self.batch_cancellation.cancel();
                self.batch_generation = self.batch_generation.wrapping_add(1);
                updates.push(UiUpdate::Batch {
                    rows: Vec::new(),
                    running: false,
                    outcome: Some("Batch stopped".into()),
                });
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
            UiAction::OpenRecord(index) => self.open_record(index, false, updates),
            UiAction::OpenRecordNewTab(index) => self.open_record(index, true, updates),
            UiAction::PinPreview => self.pin_preview(updates),
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
                    self.open_record(index, false, updates);
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
                    let spec = self
                        .active_tab
                        .and_then(|id| self.tabs.iter().find(|tab| tab.id == id))
                        .filter(|tab| tab.in_results)
                        .map(|tab| tab.context.spec.clone());
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
                        let mut view = detail_view_with_query(&hit, &expanded, spec.as_ref());
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
                let Some(tab) = self
                    .active_tab
                    .and_then(|id| self.tabs.iter().find(|tab| tab.id == id))
                else {
                    return;
                };
                if !matches!(
                    tab.freshness,
                    TabFreshness::Current | TabFreshness::NavigationUnavailable
                ) {
                    updates.push(UiUpdate::Status(
                        "Refresh this record before copying it".into(),
                    ));
                    return;
                }
                let Some(session) = &self.session else {
                    return;
                };
                let Ok(view) = session.repository.index().view() else {
                    return;
                };
                let publication = Publication {
                    instance_id: view.instance_id(),
                    revision: view.revision(),
                };
                let key = tab.key.clone();
                let tab_id = tab.id;
                let operation_generation = tab.operation_generation;
                let token = tab.cancellation.clone();
                let sender = self.worker_tx.clone();
                let open_generation = self.open_generation;
                thread::spawn(move || {
                    let result = canonical_hit_on_view(&view, &key, &token)
                        .map_err(|error| error.to_string())
                        .and_then(|hit| hit.ok_or_else(|| "Record is no longer available".into()))
                        .map(|hit| {
                            (
                                crate::backend::record_json::raw_text(&hit),
                                "Complete record copied".into(),
                            )
                        });
                    let _ = sender.send(WorkerEvent::CopiedValue {
                        open_generation,
                        publication,
                        tab_id,
                        operation_generation,
                        result,
                    });
                });
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
            UiAction::WriteIndex | UiAction::SaveIndex => {
                self.save_index(None, false, true, cx, updates)
            }
            UiAction::Index { mode, directory } => match mode {
                SnapshotMode::None => self.start_check(updates),
                SnapshotMode::Write => self.save_index(Some(directory), false, true, cx, updates),
                SnapshotMode::Load => self.load_index(directory, cx, updates),
                SnapshotMode::LoadAndWrite => self.load_index(directory, cx, updates),
            },
            UiAction::CheckForChanges => self.start_check(updates),
            UiAction::SaveIndexAs(path) => self.save_index(Some(path), false, false, cx, updates),
            UiAction::ConfirmSaveIndexAs(path) => {
                self.save_index(Some(path), true, false, cx, updates)
            }
            UiAction::LoadIndex(path) => self.load_index(path, cx, updates),
            UiAction::RebuildIndex => {
                self.start_maintenance(MaintenanceOperation::Rebuild, updates)
            }
            UiAction::CompactIndex => {
                self.start_maintenance(MaintenanceOperation::Compact, updates)
            }
            UiAction::ResumeExtendedIndex => {
                if let Some(session) = &self.session {
                    match session.repository.index().resume_extended() {
                        Ok(()) => self.poll_index_tasks(updates),
                        Err(error) => updates.push(UiUpdate::Error {
                            field: ErrorField::General,
                            message: error.to_string(),
                        }),
                    }
                }
            }
            UiAction::SetAutomaticChecking {
                enabled,
                interval_seconds,
            } => self.set_automatic_check(enabled, interval_seconds, updates),
            UiAction::SetAutomaticSaving(enabled) => self.set_automatic_save(enabled, updates),
            UiAction::CancelIndexTask(kind) => self.cancel_index_task(kind, updates),
            UiAction::RequestClose | UiAction::RetryClose => self.request_close(updates),
            UiAction::CloseWithoutSaving => self.close_without_saving(updates),
            UiAction::CancelClose => self.cancel_close(updates),
            UiAction::RefreshResults => self.refresh_results_if_ready(updates),
            UiAction::RefreshRecord(id) => self.refresh_record(id, updates),
            UiAction::ChooseIndexLoadPath {
                generation,
                load_after_selection,
            } => self.choose_index_path(generation, true, load_after_selection, cx),
            UiAction::ChooseIndexSavePath {
                generation,
                save_after_selection,
            } => {
                self.pending_save_select_default = false;
                self.choose_index_path(generation, false, save_after_selection, cx);
            }
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
                    self.start_query_impl(self.current_draft.clone(), 0, false, true, updates);
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

    fn choose_index_path(
        &mut self,
        generation: u64,
        load: bool,
        run_after_selection: bool,
        cx: &mut App,
    ) {
        let Some(session) = &self.session else {
            return;
        };
        self.index_picker_generation = generation;
        let sender = self.worker_tx.clone();
        if load {
            let receiver = cx.prompt_for_paths(PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("Load Objekte index".into()),
            });
            cx.spawn(async move |_| {
                let result = receiver
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|result| result.map_err(|error| error.to_string()))
                    .map(|paths| paths.and_then(|paths| paths.into_iter().next()));
                let _ = sender.send(WorkerEvent::PickedIndexPath {
                    generation,
                    load,
                    run_after_selection,
                    result,
                });
            })
            .detach();
        } else {
            let suggested = session.configuration.cache_save.clone().unwrap_or_else(|| {
                objekte_search_core::storage::snapshot_path_in(
                    &session.paths.root.join(".objekte"),
                    session.configuration.options.bigram_alphabet,
                )
            });
            let directory = suggested
                .parent()
                .unwrap_or(&session.paths.root)
                .to_path_buf();
            let name = suggested
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned);
            let receiver = cx.prompt_for_new_path(&directory, name.as_deref());
            cx.spawn(async move |_| {
                let result = receiver
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|result| result.map_err(|error| error.to_string()));
                let _ = sender.send(WorkerEvent::PickedIndexPath {
                    generation,
                    load,
                    run_after_selection,
                    result,
                });
            })
            .detach();
        }
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
        self.open_cancellation.cancel();
        self.open_cancellation = CancellationToken::new();
        self.pending_open_generation += 1;
        self.detection_generation = self.detection_generation.wrapping_add(1);
        self.picker_generation = self.picker_generation.wrapping_add(1);
        self.opening_draft = Some(draft);
        if let Some(task) = self.opening_task.take() {
            task.cancel();
        }
        self.catalogue_opened = false;
        let generation = self.pending_open_generation;
        let token = self.open_cancellation.clone();
        let sender = self.worker_tx.clone();
        updates.push(UiUpdate::Opening(
            "Opening repository…".into(),
        ));
        thread::spawn(move || {
            let progress_sender = sender.clone();
            let progress_token = token.clone();
            let result = open_with_preferences_staged(&spec, &token, move |task, root, collections| {
                let _ = progress_sender.send(WorkerEvent::StagedStarted {
                    generation,
                    task: task.clone(),
                });
                let watcher_sender = progress_sender.clone();
                thread::spawn(move || {
                    let mut last_status = None;
                    let mut catalogue_sent = false;
                    loop {
                        if progress_token.is_cancelled() {
                            return;
                        }
                        let Ok(status) = task.status() else { return };
                        if last_status.as_ref() != Some(&status) {
                            let _ = watcher_sender.send(WorkerEvent::StagedProgress {
                                generation,
                                status: status.clone(),
                            });
                            last_status = Some(status.clone());
                        }
                        if !catalogue_sent && matches!(
                            status.phase,
                            StagedBuildPhase::BuildingBase
                                | StagedBuildPhase::SearchReady
                                | StagedBuildPhase::BuildingExtended
                                | StagedBuildPhase::Complete
                        ) {
                            if let Ok(files) = task.catalogue() {
                                let (directories, file_catalogue) = prepare_staged_catalogue(files, &collections);
                                let _ = watcher_sender.send(WorkerEvent::CatalogueReady {
                                    generation,
                                    root: root.clone(),
                                    collections: collections.clone(),
                                    directories,
                                    file_catalogue,
                                });
                                catalogue_sent = true;
                            }
                        }
                        if matches!(status.phase, StagedBuildPhase::SearchReady
                            | StagedBuildPhase::BuildingExtended
                            | StagedBuildPhase::Complete
                            | StagedBuildPhase::Cancelled
                            | StagedBuildPhase::Failed)
                        {
                            return;
                        }
                        thread::sleep(Duration::from_millis(50));
                    }
                });
            });
            let _ = sender.send(WorkerEvent::Opened {
                generation,
                result: Box::new(result),
            });
        });
    }

    fn cancel_open(&mut self, updates: &mut Vec<UiUpdate>) {
        if self.opening_draft.take().is_some() {
            self.open_cancellation.cancel();
            if let Some(task) = self.opening_task.take() {
                task.cancel();
            }
            self.pending_open_generation = self.pending_open_generation.wrapping_add(1);
            if self.catalogue_opened {
                self.index_ui.load = IndexTaskState::Cancelled;
                updates.push(UiUpdate::IndexStatus(self.index_ui.clone()));
            }
            updates.push(UiUpdate::Status("Open cancelled".into()));
        }
    }

    fn start_validation(&mut self, draft: SearchDraft, updates: &mut Vec<UiUpdate>) {
        if self.opening_draft.is_some() {
            self.current_draft = draft;
            return;
        }
        let Some(session) = self.session.clone() else { return };
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
        self.start_query_impl(draft, page_start, false, false, updates);
    }

    fn start_query_impl(
        &mut self,
        draft: SearchDraft,
        page_start: usize,
        refresh: bool,
        retain_preview: bool,
        updates: &mut Vec<UiUpdate>,
    ) {
        if self.opening_draft.is_some() {
            self.current_draft = draft;
            updates.push(UiUpdate::Status("Query queued until search is ready".into()));
            return;
        }
        self.poll_publication(updates);
        let retain_preview = retain_preview
            && !self.results_outdated
            && self.executed_draft.as_ref() == Some(&draft);
        let Some(session) = self.session.clone() else {
            if self.opening_draft.is_some() {
                self.current_draft = draft;
                updates.push(UiUpdate::Status("Query queued while source files are discovered".into()));
                return;
            }
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
        let view = match session.repository.index().view() {
            Ok(view) => view,
            Err(error) => {
                updates.push(UiUpdate::Error {
                    field: ErrorField::General,
                    message: error.to_string(),
                });
                return;
            }
        };
        self.cancellation.cancel();
        self.cancellation = CancellationToken::new();
        self.query_generation += 1;
        self.executed_view = Some(view);
        self.refreshing_results = refresh;
        self.refresh_anchor = refresh
            .then(|| {
                self.selected_index
                    .and_then(|index| self.keys.get(index).cloned().flatten())
            })
            .flatten();
        if !refresh {
            self.pending_outdated_open = None;
        }
        self.current_draft = draft.clone();
        self.executed_draft = Some(draft.clone());
        self.executed_spec = Some(spec.clone());
        if !refresh && !retain_preview {
            self.selected_index = None;
        }
        if !refresh {
            self.keys.clear();
        }
        self.pending_reveal = None;
        self.next_page_start = None;
        self.pending_continuation = None;
        self.continuation_failed = false;
        self.pending_projections.clear();
        self.projected_pages.clear();
        let hidden = self.preferences.as_ref().map_or_else(Vec::new, |prefs| {
            prefs.hidden(draft.collection.as_deref()).to_vec()
        });
        let shown_nested = self.preferences.as_ref().map_or_else(Vec::new, |prefs| {
            prefs.shown_nested(draft.collection.as_deref()).to_vec()
        });
        if let Some(preferences) = &self.preferences {
            let (widths, wrapped) = preferences.table_presentation(draft.collection.as_deref());
            updates.push(UiUpdate::TablePresentation {
                scope: draft.collection.clone(),
                widths,
                wrapped,
            });
        }
        updates.push(UiUpdate::QueryStarted {
            status: if refresh || retain_preview {
                "Updating results…".into()
            } else {
                "Searching…".into()
            },
            retain_preview,
        });
        let columns = initial_columns(&session, &draft);
        spawn_grid_page(
            session,
            self.executed_view
                .as_ref()
                .expect("query view pinned")
                .clone(),
            spec,
            page_start,
            hidden,
            shown_nested,
            columns,
            self.cancellation.clone(),
            self.worker_tx.clone(),
            self.open_generation,
            self.query_generation,
            GridRequestKind::First,
        );
    }

    fn start_continuation(&mut self, updates: &mut Vec<UiUpdate>) {
        if self.results_outdated || self.refreshing_results {
            return;
        }
        let (Some(session), Some(view), Some(spec), Some(start)) = (
            self.session.clone(),
            self.executed_view.clone(),
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
        let collection = self
            .executed_draft
            .as_ref()
            .and_then(|draft| draft.collection.as_deref());
        let shown_nested = self
            .preferences
            .as_ref()
            .map_or_else(Vec::new, |prefs| prefs.shown_nested(collection).to_vec());
        updates.push(UiUpdate::ContinuationStarted);
        spawn_grid_page(
            session,
            view,
            spec,
            start,
            hidden,
            shown_nested,
            self.available_columns.clone(),
            self.cancellation.clone(),
            self.worker_tx.clone(),
            self.open_generation,
            self.query_generation,
            GridRequestKind::Continuation(start),
        );
    }

    fn ensure_projection(&mut self, first: usize, end: usize) {
        if self.results_outdated || self.refreshing_results {
            return;
        }
        let (Some(session), Some(view), Some(spec)) = (
            self.session.clone(),
            self.executed_view.clone(),
            self.executed_spec.clone(),
        ) else {
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
            let collection = self
                .executed_draft
                .as_ref()
                .and_then(|draft| draft.collection.as_deref());
            let shown_nested = self
                .preferences
                .as_ref()
                .map_or_else(Vec::new, |prefs| prefs.shown_nested(collection).to_vec());
            spawn_grid_page(
                session.clone(),
                view.clone(),
                spec.clone(),
                start,
                hidden,
                shown_nested,
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
        if self.results_outdated || self.refreshing_results {
            return;
        }
        let (Some(view), Some(spec)) = (self.executed_view.clone(), self.executed_spec.clone())
        else {
            return;
        };
        let sender = self.worker_tx.clone();
        let token = self.cancellation.clone();
        let open_generation = self.open_generation;
        let query_generation = self.query_generation;
        let publication = Publication {
            instance_id: view.instance_id(),
            revision: view.revision(),
        };
        updates.push(UiUpdate::CountStarted);
        thread::spawn(move || {
            let result = backend::count_exact_on_view(&view, &spec, &token)
                .map_err(|error| error.to_string());
            let _ = sender.send(WorkerEvent::Counted {
                open_generation,
                query_generation,
                publication,
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
                origin: request.origin,
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
        let token = self.completion_token.clone();
        thread::spawn(move || {
            let values = posting_completions(&session, &request, &token);
            if token.is_cancelled() {
                return;
            }
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
        updates.push(UiUpdate::RowSelection(Some(index)));
    }

    fn sync_tab_row_selection(&mut self, tab_index: usize, updates: &mut Vec<UiUpdate>) {
        let tab = &self.tabs[tab_index];
        let selected = self
            .keys
            .get(tab.anchor)
            .and_then(Option::as_ref)
            .filter(|key| *key == &tab.key)
            .map(|_| tab.anchor)
            .or_else(|| {
                self.keys
                    .iter()
                    .position(|key| key.as_ref() == Some(&tab.key))
            });
        self.selected_index = selected;
        updates.push(UiUpdate::RowSelection(selected));
    }

    fn sync_active_tab_row_selection_in(&mut self, start: usize, updates: &mut Vec<UiUpdate>) {
        let Some(index) = self
            .active_tab
            .and_then(|id| self.tabs.iter().position(|tab| tab.id == id))
        else {
            return;
        };
        let key = &self.tabs[index].key;
        if let Some(selected) = self.selected_index {
            if self.keys.get(selected).and_then(Option::as_ref) == Some(key) {
                return;
            }
            self.sync_tab_row_selection(index, updates);
            return;
        }
        if let Some(relative) = self
            .keys
            .get(start..)
            .and_then(|keys| keys.iter().position(|candidate| candidate.as_ref() == Some(key)))
        {
            let selected = start + relative;
            self.selected_index = Some(selected);
            updates.push(UiUpdate::RowSelection(Some(selected)));
        }
    }

    fn emit_tabs(&self, updates: &mut Vec<UiUpdate>) {
        updates.push(UiUpdate::Tabs {
            tabs: self
                .tabs
                .iter()
                .map(|tab| RecordTabSummary {
                    id: tab.id,
                    pinned: tab.pinned,
                    title: tab.title.clone(),
                    identity: format!("{} / {}", tab.key.collection, tab.key.id),
                    position: tab.anchor + 1,
                    can_back: !tab.back.is_empty()
                        && tab.freshness == TabFreshness::Current
                        && !tab.navigation_outdated,
                    can_previous: tab.anchor > 0
                        && tab.freshness == TabFreshness::Current
                        && !tab.navigation_outdated,
                    can_next: !tab.end_confirmed
                        && tab.freshness == TabFreshness::Current
                        && !tab.navigation_outdated,
                    loading: tab.loading,
                    links_loading: tab.links_pending,
                    freshness: tab.freshness,
                })
                .collect(),
            active: self.active_tab,
        });
    }

    fn open_record(&mut self, index: usize, new_tab: bool, updates: &mut Vec<UiUpdate>) {
        let (Some(key), Some(spec), Some(draft)) = (
            self.keys.get(index).cloned().flatten(),
            self.executed_spec.clone(),
            self.executed_draft.clone(),
        ) else {
            return;
        };
        if self.results_outdated {
            if self.current_draft == self.executed_draft.clone().unwrap_or_default() {
                self.pending_outdated_open = Some((key, new_tab));
                updates.push(UiUpdate::Status(
                    "Updating results before opening this record".into(),
                ));
                self.refresh_results_if_ready(updates);
            } else {
                updates.push(UiUpdate::Status(
                    "Finish the current query before opening an outdated result".into(),
                ));
            }
            return;
        }
        let Some(view) = self.executed_view.clone().or_else(|| {
            self.session
                .as_ref()
                .and_then(|session| session.repository.index().view().ok())
        }) else {
            return;
        };
        self.select_row(index, updates);
        let context = Arc::new(TabContext {
            publication: Publication {
                instance_id: view.instance_id(),
                revision: view.revision(),
            },
            spec,
            draft,
        });
        if !new_tab {
            if let Some(id) = self.tabs.iter().find(|tab| !tab.pinned).map(|tab| tab.id) {
                self.activate_for_replacement(id, updates);
                self.start_tab_load(
                    id,
                    TabLoadKind::Replace {
                        key,
                        context,
                        anchor: index,
                    },
                    updates,
                );
                return;
            }
        }
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs.push(RecordTab {
            id,
            pinned: new_tab,
            context,
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
            freshness: TabFreshness::Current,
            navigation_outdated: false,
            cached_detail: None,
        });
        self.activate_tab(id, updates);
    }

    fn activate_for_replacement(&mut self, id: u64, updates: &mut Vec<UiUpdate>) {
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
        self.expanded.clear();
        updates.push(UiUpdate::Detail(None));
        self.emit_tabs(updates);
    }

    fn pin_preview(&mut self, updates: &mut Vec<UiUpdate>) {
        if let Some(tab) = self
            .active_tab
            .and_then(|id| self.tabs.iter_mut().find(|tab| tab.id == id && !tab.pinned))
        {
            tab.pinned = true;
            self.emit_tabs(updates);
        }
    }

    /// A stale row retains its canonical identity, but its old position is not
    /// a locator in the new index. Re-resolve the identity without scanning a
    /// deep result prefix when it is absent from the refreshed first page.
    fn open_canonical_outside_results(
        &mut self,
        key: RecordKey,
        new_tab: bool,
        updates: &mut Vec<UiUpdate>,
    ) {
        let (Some(spec), Some(draft), Some(view)) = (
            self.executed_spec.clone(),
            self.executed_draft.clone(),
            self.executed_view.clone(),
        ) else {
            return;
        };
        let context = Arc::new(TabContext {
            publication: Publication {
                instance_id: view.instance_id(),
                revision: view.revision(),
            },
            spec,
            draft,
        });
        let id = if !new_tab {
            if let Some(id) = self.tabs.iter().find(|tab| !tab.pinned).map(|tab| tab.id) {
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
                    tab.cancellation.cancel();
                    tab.context = context.clone();
                    tab.key = key.clone();
                    tab.anchor = 0;
                    tab.back.clear();
                    tab.expanded.clear();
                    tab.view = TabViewState::default();
                    tab.in_results = false;
                    tab.navigation_outdated = true;
                    tab.cached_detail = None;
                    tab.freshness = TabFreshness::Outdated;
                    tab.title = key.id.clone();
                    id
                } else {
                    return;
                }
            } else {
                self.next_tab_id
            }
        } else {
            self.next_tab_id
        };
        if id == self.next_tab_id {
            self.next_tab_id = self.next_tab_id.wrapping_add(1);
            self.tabs.push(RecordTab {
                id,
                pinned: new_tab,
                context,
                key: key.clone(),
                anchor: 0,
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
                freshness: TabFreshness::Outdated,
                navigation_outdated: true,
                cached_detail: None,
            });
        }
        self.active_tab = Some(id);
        self.selected_index = None;
        updates.push(UiUpdate::RowSelection(None));
        self.selected_hit = None;
        self.detail = None;
        self.relation_actions.clear();
        updates.push(UiUpdate::Detail(None));
        self.emit_tabs(updates);
        self.start_tab_load(id, TabLoadKind::Refresh, updates);
    }

    fn activate_tab(&mut self, id: u64, updates: &mut Vec<UiUpdate>) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        self.sync_tab_row_selection(index, updates);
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
        if self.tabs[index].freshness == TabFreshness::Current {
            self.start_tab_load(id, TabLoadKind::Open, updates);
        } else {
            let detail = self.tabs[index].cached_detail.clone();
            self.detail = detail.clone();
            updates.push(UiUpdate::Detail(detail));
        }
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
        let Ok(index_view) = session.repository.index().view() else {
            return;
        };
        let publication = Publication {
            instance_id: index_view.instance_id(),
            revision: index_view.revision(),
        };
        let expected = match &kind {
            TabLoadKind::Replace { context, .. } => context.publication,
            _ => tab.context.publication,
        };
        if !matches!(&kind, TabLoadKind::Refresh)
            && (expected != publication
                || (!matches!(&kind, TabLoadKind::Replace { .. })
                    && tab.freshness != TabFreshness::Current))
        {
            tab.freshness = TabFreshness::Outdated;
            tab.navigation_outdated = true;
            self.emit_tabs(updates);
            return;
        }
        tab.cancellation.cancel();
        tab.cancellation = CancellationToken::new();
        tab.operation_generation += 1;
        tab.loading = true;
        if matches!(&kind, TabLoadKind::Refresh) {
            tab.freshness = TabFreshness::Refreshing;
        }
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
            let query_result = match &kind {
                TabLoadKind::Refresh => true,
                TabLoadKind::Replace { .. } => true,
                TabLoadKind::Adjacent { .. } => true,
                TabLoadKind::Relation { .. } => false,
                TabLoadKind::Back { frame } => frame.in_results,
                TabLoadKind::Open => in_results,
            };
            let result = match &kind {
                TabLoadKind::Refresh => canonical_hit_on_view(&index_view, &key, &token),
                TabLoadKind::Replace {
                    key,
                    context,
                    anchor,
                } => tab_result_hit_on_view(
                    &session,
                    &index_view,
                    &context.spec,
                    *anchor,
                    key,
                    &token,
                ),
                TabLoadKind::Adjacent { anchor } => {
                    backend::query_page_on_view(&session, &index_view, &spec, *anchor, 1, &token)
                        .map(|mut page| page.hits.pop())
                }
                TabLoadKind::Relation { key } => canonical_hit_on_view(&index_view, key, &token),
                TabLoadKind::Back { frame } => {
                    if frame.in_results {
                        tab_result_hit_on_view(
                            &session,
                            &index_view,
                            &spec,
                            frame.anchor,
                            &frame.key,
                            &token,
                        )
                    } else {
                        canonical_hit_on_view(&index_view, &frame.key, &token)
                    }
                }
                TabLoadKind::Open => {
                    if in_results {
                        tab_result_hit_on_view(
                            &session,
                            &index_view,
                            &spec,
                            tab_anchor,
                            &key,
                            &token,
                        )
                    } else {
                        canonical_hit_on_view(&index_view, &key, &token)
                    }
                }
            }
            .map(|hit| {
                let presentation_spec = match &kind {
                    TabLoadKind::Replace { context, .. } => &context.spec,
                    _ => &spec,
                };
                hit.map(|hit| {
                    record_presentation(&session, query_result.then_some(presentation_spec), hit)
                })
            })
            .map_err(|error| error.to_string());
            let _ = sender.send(WorkerEvent::TabLoaded {
                open_generation,
                publication,
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
        if tab.freshness != TabFreshness::Current || tab.navigation_outdated {
            return;
        }
        if tab.links_loaded || tab.links_pending {
            return;
        }
        let Ok(view) = session.repository.index().view() else {
            return;
        };
        let publication = Publication {
            instance_id: view.instance_id(),
            revision: view.revision(),
        };
        if tab.context.publication != publication {
            tab.freshness = TabFreshness::Outdated;
            tab.navigation_outdated = true;
            self.emit_tabs(updates);
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
            let Ok(pinned) = session.repository.with_pinned_view(view) else {
                return;
            };
            let rows = relations::rows(&pinned, &hit);
            if token.is_cancelled() {
                return;
            }
            let _ = sender.send(WorkerEvent::LinksLoaded {
                open_generation,
                publication,
                tab_id: id,
                operation_generation,
                rows,
            });
        });
    }

    fn complete(&mut self, event: WorkerEvent, cx: &mut App, updates: &mut Vec<UiUpdate>) {
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
            WorkerEvent::Directories {
                open_generation,
                publication,
                directories,
            } if open_generation == self.open_generation => {
                self.pending_directories = None;
                if self.session_publication_is_current(publication) {
                    updates.push(UiUpdate::Directories(directories));
                } else {
                    self.request_directories();
                }
            }
            WorkerEvent::StagedStarted { generation, task }
                if generation == self.pending_open_generation && self.opening_draft.is_some() =>
            {
                self.opening_task = Some(task);
            }
            WorkerEvent::StagedProgress { generation, status }
                if generation == self.pending_open_generation && self.opening_draft.is_some() =>
            {
                let message = match status.phase {
                    StagedBuildPhase::Discovering => "Discovering source files…".to_owned(),
                    StagedBuildPhase::BuildingBase => format!(
                        "Building search base from {} files…", status.discovered_files,
                    ),
                    StagedBuildPhase::SearchReady | StagedBuildPhase::BuildingExtended =>
                        "Search ready; building extended indexes…".to_owned(),
                    StagedBuildPhase::Complete => "Index complete".to_owned(),
                    StagedBuildPhase::Cancelled => "Opening cancelled".to_owned(),
                    StagedBuildPhase::Failed => status.error.unwrap_or_else(|| "Opening failed".into()),
                };
                updates.push(UiUpdate::Status(message));
            }
            WorkerEvent::CatalogueReady { generation, root, collections, directories, file_catalogue }
                if generation == self.pending_open_generation
                    && (self.opening_draft.is_some() || self.session.is_some()) =>
            {
                if self.catalogue_opened {
                    return;
                }
                if self.opening_draft.is_none() {
                    updates.push(UiUpdate::FileCatalogue(file_catalogue));
                    return;
                }
                self.catalogue_opened = true;
                self.cancellation.cancel();
                self.export_cancellation.cancel();
                self.batch_cancellation.cancel();
                self.completion_token.cancel();
                self.completion_token = CancellationToken::new();
                self.cancellation = CancellationToken::new();
                self.open_generation = self.open_generation.wrapping_add(1);
                self.query_generation = self.query_generation.wrapping_add(1);
                self.validation_generation = self.validation_generation.wrapping_add(1);
                self.export_generation = self.export_generation.wrapping_add(1);
                self.batch_generation = self.batch_generation.wrapping_add(1);
                self.completion_generation = self.completion_generation.wrapping_add(1);
                self.pending_edit = None;
                self.pending_completion = None;
                self.completion_cache.clear();
                self.completion_cache_order.clear();
                self.pending_directories = None;
                self.check_task = None;
                self.maintenance_task = None;
                self.index_ui = IndexUiStatus::default();
                self.index_operation_id = self.index_operation_id.wrapping_add(1);
                self.index_ui.load = IndexTaskState::Running {
                    operation_id: self.index_operation_id,
                    processed: None,
                    total: None,
                };
                self.last_index_status_sent = None;
                self.results_outdated = false;
                self.refreshing_results = false;
                self.preferences = None;
                self.executed_view = None;
                self.executed_spec = None;
                self.executed_draft = None;
                self.next_page_start = None;
                self.pending_continuation = None;
                self.keys.clear();
                self.selected_hit = None;
                self.detail = None;
                self.available_columns.clear();
                self.observed_publication = None;
                for tab in &self.tabs {
                    tab.cancellation.cancel();
                }
                self.tabs.clear();
                self.active_tab = None;
                self.emit_tabs(updates);
                if let Some(previous) = self.session.take() {
                    thread::spawn(move || {
                        let _ = backend::index_lifecycle::stop_automation(&previous);
                    });
                }
                let root = root.display().to_string();
                let single_collection = collections.len() == 1;
                self.open_diagnostics = format!(
                    "Root: {root}\nCollections: {}\nDiscovered files: {}\nBuilding search base",
                    collections.join(", "),
                    file_catalogue.files.len(),
                );
                updates.push(UiUpdate::Opened {
                    root,
                    collections: collections.clone(),
                    single_collection,
                    directories,
                });
                updates.push(UiUpdate::FileCatalogue(file_catalogue));
                updates.push(UiUpdate::IndexStatus(self.index_ui.clone()));
                updates.push(UiUpdate::Status(
                    "File catalogue ready; building search base".into(),
                ));
                let mut draft = self.current_draft.clone();
                draft.collection = if single_collection { collections.first().cloned() } else { None };
                self.current_draft = draft;
            }
            WorkerEvent::Opened { generation, result }
                if generation == self.pending_open_generation && self.opening_draft.is_some() =>
            {
                match *result {
                    Ok((session, preferences, directory_publication, directories)) => {
                        let had_catalogue = std::mem::take(&mut self.catalogue_opened);
                        self.opening_task = None;
                                self.cancellation.cancel();
                        self.export_cancellation.cancel();
                        self.batch_cancellation.cancel();
                        self.open_generation = self.open_generation.wrapping_add(1);
                        self.pending_directories = None;
                        self.query_generation = self.query_generation.wrapping_add(1);
                        self.validation_generation = self.validation_generation.wrapping_add(1);
                        self.export_generation = self.export_generation.wrapping_add(1);
                        self.batch_generation = self.batch_generation.wrapping_add(1);
                        self.pending_edit = None;
                        self.pending_completion = None;
                        self.completion_token.cancel();
                        self.completion_token = CancellationToken::new();
                        self.completion_generation = self.completion_generation.wrapping_add(1);
                        self.completion_cache.clear();
                        self.completion_cache_order.clear();
                        self.observed_publication = session
                            .repository
                            .index()
                            .status()
                            .ok()
                            .as_ref()
                            .map(Publication::from);
                        self.executed_view = None;
                        self.results_outdated = false;
                        self.refreshing_results = false;
                        self.refresh_anchor = None;
                        self.check_task = session
                            .repository
                            .index()
                            .current_reconciliation_task()
                            .ok()
                            .flatten();
                        self.check_origin = self.check_task.as_ref().map(|_| CheckOrigin::Initial);
                        self.maintenance_task = None;
                        updates.push(UiUpdate::ResultsOutdated(false));
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
                            "Open: {:.1} ms\nRoot: {}\nCollections: {}\nSchema fields: {}; files: {:?}\nSnapshot save: {:#?}\nStartup: {:#?}{}",
                            session.diagnostics.open_ns as f64 / 1_000_000.0,
                            root,
                            collections.join(", "),
                            session.diagnostics.schema_fields,
                            session.diagnostics.schema_files,
                            session.diagnostics.snapshot_save,
                            session.diagnostics.startup,
                            if session.args.write_index.is_some() {
                                "\nIndex write: pending"
                            } else {
                                ""
                            },
                        );
                        self.automatic_check_operation_id =
                            self.automatic_check_operation_id.wrapping_add(1);
                        self.automatic_check_epoch
                            .store(self.automatic_check_operation_id, Ordering::Release);
                        self.invalidate_automatic_save_policy();
                        self.requested_check = (false, 30);
                        let previous_session = self.session.replace(session);
                        if self.observed_publication != Some(directory_publication) {
                            self.request_directories();
                        }
                        if let Some(previous_session) = previous_session {
                            let gate = Arc::clone(&self.automatic_check_gate);
                            let save_gate = Arc::clone(&self.automatic_save_gate);
                            thread::spawn(move || {
                                let _gate = gate.lock();
                                let _save_gate = save_gate.lock();
                                let _ =
                                    backend::index_lifecycle::stop_automation(&previous_session);
                            });
                        }
                        self.index_ui = IndexUiStatus::default();
                        self.push_field_choices(updates);
                        if had_catalogue {
                            updates.push(UiUpdate::Directories(directories));
                        } else {
                            updates.push(UiUpdate::Opened {
                                root,
                                collections: collections.clone(),
                                single_collection,
                                directories,
                            });
                        }
                        if let Some(session) = &self.session
                            && let Ok(status) = backend::index_lifecycle::status(session)
                        {
                            self.update_index_ui(&status, updates);
                        }
                        match preferences {
                            Ok(preferences) => {
                                updates.push(UiUpdate::History {
                                    entries: preferences.history(),
                                    saved: preferences.saved(),
                                });
                                if let Some(dark) = preferences.dark() {
                                    updates.push(UiUpdate::Theme { dark });
                                }
                                if let Some(session) = &self.session {
                                    let source_key = lifecycle_source_key(session);
                                    let destination = session
                                        .configuration
                                        .cache_save
                                        .as_ref()
                                        .map(|path| path.display().to_string());
                                    let policy = preferences
                                        .lifecycle_policy(&source_key, destination.as_deref());
                                    self.index_ui.check_interval_seconds =
                                        policy.check_interval_seconds;
                                    self.requested_check =
                                        (policy.automatic_check, policy.check_interval_seconds);
                                    self.preferences = Some(preferences);
                                    if policy.automatic_check {
                                        self.set_automatic_check(
                                            true,
                                            policy.check_interval_seconds,
                                            updates,
                                        );
                                    }
                                    if policy.automatic_save {
                                        self.set_automatic_save(true, updates);
                                    }
                                    updates.push(UiUpdate::IndexStatus(self.index_ui.clone()));
                                } else {
                                    self.preferences = Some(preferences);
                                }
                            }
                            Err(message) => updates.push(UiUpdate::Error {
                                field: ErrorField::General,
                                message,
                            }),
                        }
                        updates.push(UiUpdate::Diagnostics(self.open_diagnostics.clone()));
                        let mut draft = self.current_draft.clone();
                        if !had_catalogue {
                            draft.collection = if single_collection {
                                collections.first().cloned()
                            } else {
                                None
                            };
                        }
                        self.start_query(draft, 0, updates);
                    }
                    Err(message) => {
                        self.opening_draft = None;
                        self.opening_task = None;
                        self.catalogue_opened = true;
                        if self.catalogue_opened {
                            self.index_ui.load = IndexTaskState::Failed(message.clone());
                            updates.push(UiUpdate::IndexStatus(self.index_ui.clone()));
                        }
                        updates.push(UiUpdate::Error {
                            field: ErrorField::Open,
                            message,
                        });
                        updates.push(UiUpdate::Status("Index unavailable".into()));
                    }
                }
            }
            WorkerEvent::Queried {
                open_generation,
                query_generation,
                publication,
                kind,
                result,
            } if open_generation == self.open_generation
                && query_generation == self.query_generation =>
            {
                if !self.publication_is_current(publication) {
                    self.poll_publication(updates);
                    return;
                }
                match *result {
                    Ok((mut view, keys, next, elapsed_ns, available, hidden, diagnostics)) => {
                        match kind {
                            GridRequestKind::First => {
                                let was_refresh = self.refreshing_results;
                                self.refreshing_results = false;
                                self.results_outdated = false;
                                updates.push(UiUpdate::ResultsOutdated(false));
                                let needs_count = view.exact_total.is_none() && view.has_next;
                                if self.selected_index.is_some() {
                                    view.selected = self.selected_index;
                                }
                                self.keys = vec![None; view.offset];
                                self.keys.extend(keys.into_iter().map(Some));
                                let refreshed_anchor = self.refresh_anchor.take();
                                self.next_page_start = next;
                                self.remember_projection(view.offset);
                                updates.push(UiUpdate::Page(view));
                                if !was_refresh {
                                    self.sync_active_tab_row_selection_in(0, updates);
                                }
                                if let Some(anchor) = refreshed_anchor {
                                    if let Some(position) = self
                                        .keys
                                        .iter()
                                        .position(|key| key.as_ref() == Some(&anchor))
                                    {
                                        self.selected_index = Some(position);
                                        updates.push(UiUpdate::RevealRow(position));
                                    } else {
                                        self.selected_index = None;
                                        updates.push(UiUpdate::Status("Results updated; reading position reset to the beginning".into()));
                                    }
                                }
                                if was_refresh
                                    && let Some((key, new_tab)) = self.pending_outdated_open.take()
                                {
                                    if let Some(position) = self
                                        .keys
                                        .iter()
                                        .position(|candidate| candidate.as_ref() == Some(&key))
                                    {
                                        self.open_record(position, new_tab, updates);
                                    } else {
                                        self.open_canonical_outside_results(key, new_tab, updates);
                                    }
                                }
                                if needs_count {
                                    self.start_count(updates);
                                }
                                if let Some(index) = self.pending_reveal.take() {
                                    self.selected_index = Some(index);
                                    updates.push(UiUpdate::RevealRow(index));
                                }
                                updates.push(UiUpdate::Status(format!(
                                    "Query complete in {:.1} ms",
                                    elapsed_ns as f64 / 1_000_000.0
                                )));
                                if !was_refresh
                                    && let (Some(preferences), Some(draft)) =
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
                                self.sync_active_tab_row_selection_in(start, updates);
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
                        GridRequestKind::First => {
                            let was_refresh = std::mem::take(&mut self.refreshing_results);
                            updates.push(UiUpdate::Error {
                                field: if was_refresh {
                                    ErrorField::General
                                } else {
                                    query_error_field(
                                        &message,
                                        self.executed_draft.as_ref().unwrap_or(&self.current_draft),
                                    )
                                },
                                message,
                            });
                        }
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
                publication,
                result,
            } if open_generation == self.open_generation
                && query_generation == self.query_generation =>
            {
                if !self.publication_is_current(publication) {
                    self.poll_publication(updates);
                    return;
                }
                match result {
                    Ok(total) => updates.push(UiUpdate::ExactCount(total)),
                    Err(message) => updates.push(UiUpdate::CountFailed(message)),
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
                    origin: request.origin,
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
                publication,
                tab_id,
                operation_generation,
                kind,
                result,
            } if open_generation == self.open_generation => {
                if !self.session_publication_is_current(publication) {
                    self.poll_publication(updates);
                    return;
                }
                let Some(index) = self.tabs.iter().position(|tab| {
                    tab.id == tab_id && tab.operation_generation == operation_generation
                }) else {
                    return;
                };
                self.tabs[index].loading = false;
                let mut reveal_selection = false;
                match *result {
                    Ok(Some((hit, view, actions))) => {
                        reveal_selection = matches!(
                            kind,
                            TabLoadKind::Adjacent { .. } | TabLoadKind::Back { .. }
                        );
                        let tab = &mut self.tabs[index];
                        match &kind {
                            TabLoadKind::Refresh => {
                                tab.context = Arc::new(TabContext {
                                    publication,
                                    ..(*tab.context).clone()
                                });
                                tab.in_results = false;
                                tab.navigation_outdated = true;
                                tab.freshness = TabFreshness::NavigationUnavailable;
                                tab.links_loaded = false;
                                tab.expanded.clear();
                            }
                            TabLoadKind::Replace {
                                context, anchor, ..
                            } => {
                                tab.context = context.clone();
                                tab.anchor = *anchor;
                                tab.back.clear();
                                tab.view = TabViewState::default();
                                tab.in_results = true;
                                tab.end_confirmed = false;
                                tab.expanded.clear();
                                tab.links_loaded = false;
                                tab.navigation_outdated = false;
                            }
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
                        if !matches!(&kind, TabLoadKind::Refresh) {
                            tab.freshness = TabFreshness::Current;
                        }
                        tab.key = RecordKey {
                            collection: hit.collection.clone(),
                            id: hit.record_id.clone(),
                        };
                        tab.title = view.display_title().to_string();
                        tab.cached_detail = Some(view.clone());
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
                        if matches!(kind, TabLoadKind::Refresh) {
                            self.tabs[index].freshness = TabFreshness::Removed;
                            self.tabs[index].navigation_outdated = true;
                            if self.active_tab == Some(tab_id) {
                                self.selected_hit = None;
                            }
                            updates.push(UiUpdate::Status("Record is no longer available".into()));
                        } else if matches!(kind, TabLoadKind::Adjacent { .. }) {
                            self.tabs[index].end_confirmed = true;
                            updates.push(UiUpdate::Status("End of results".into()));
                        } else {
                            updates.push(UiUpdate::Error {
                                field: ErrorField::General,
                                message: "Record is unavailable".into(),
                            });
                        }
                    }
                    Err(message) => {
                        if matches!(kind, TabLoadKind::Refresh) {
                            self.tabs[index].freshness = TabFreshness::Failed;
                            self.tabs[index].navigation_outdated = true;
                        }
                        updates.push(UiUpdate::Error {
                            field: ErrorField::General,
                            message,
                        });
                    }
                }
                if self.active_tab == Some(tab_id) {
                    self.sync_tab_row_selection(index, updates);
                    if reveal_selection && let Some(selected) = self.selected_index {
                        updates.push(UiUpdate::RevealRow(selected));
                    }
                }
                self.emit_tabs(updates);
            }
            WorkerEvent::LinksLoaded {
                open_generation,
                publication,
                tab_id,
                operation_generation,
                rows,
            } if open_generation == self.open_generation => {
                if !self.session_publication_is_current(publication) {
                    self.poll_publication(updates);
                    return;
                }
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
            WorkerEvent::PickedIndexPath {
                generation,
                load,
                run_after_selection,
                result,
            } if generation == self.index_picker_generation => match result {
                Ok(Some(path)) => {
                    let path = path.display().to_string();
                    if load {
                        updates.push(UiUpdate::ChosenIndexLoadPath {
                            generation,
                            path: path.clone(),
                        });
                        if run_after_selection {
                            self.load_index(path, cx, updates);
                        }
                    } else {
                        updates.push(UiUpdate::ChosenIndexSavePath {
                            generation,
                            path: path.clone(),
                        });
                        if run_after_selection {
                            self.save_index(
                                Some(path),
                                false,
                                self.pending_save_select_default,
                                cx,
                                updates,
                            );
                        }
                    }
                }
                Ok(None) => {}
                Err(message) => updates.push(UiUpdate::Error {
                    field: ErrorField::General,
                    message: format!("Index picker failed: {message}. Enter a path instead."),
                }),
            },
            WorkerEvent::IndexSaved {
                open_generation,
                operation_id,
                make_default,
                result,
            } if open_generation == self.open_generation
                && operation_id == self.save_operation_id =>
            {
                match result {
                    IndexSaveCompletion::Saved(saved) => {
                        if make_default && let Some(session) = &self.session {
                            self.session =
                                Some(session.with_first_save_target(saved.destination.clone()));
                        }
                        self.index_ui.save = IndexTaskState::Succeeded(
                            if saved.current_status.revision == saved.saved_revision {
                                "Index saved".into()
                            } else {
                                "Newer index changes are not saved yet".into()
                            },
                        );
                        self.update_index_ui(&saved.current_status, updates);
                    }
                    IndexSaveCompletion::OverwriteRequired(path) => {
                        self.pending_index_overwrite = Some(path.clone());
                        self.index_ui.save = IndexTaskState::Failed(
                            "This file already exists; confirm replacement".into(),
                        );
                        updates.push(UiUpdate::SaveIndexOverwriteRequired(
                            path.display().to_string(),
                        ));
                        updates.push(UiUpdate::IndexStatus(self.index_ui.clone()));
                    }
                    IndexSaveCompletion::Failed(message) => {
                        self.index_ui.save = IndexTaskState::Failed(message);
                        updates.push(UiUpdate::IndexStatus(self.index_ui.clone()));
                    }
                }
            }
            WorkerEvent::IndexLoaded {
                open_generation,
                operation_id,
                result,
            } if open_generation == self.open_generation
                && operation_id == self.load_operation_id =>
            {
                match result {
                    Ok(session) => {
                        self.invalidate_automatic_save_policy();
                        self.cancellation.cancel();
                        self.export_cancellation.cancel();
                        self.batch_cancellation.cancel();
                        self.open_generation = self.open_generation.wrapping_add(1);
                        self.pending_directories = None;
                        self.query_generation = self.query_generation.wrapping_add(1);
                        self.export_generation = self.export_generation.wrapping_add(1);
                        self.batch_generation = self.batch_generation.wrapping_add(1);
                        self.check_task = session
                            .repository
                            .index()
                            .current_reconciliation_task()
                            .ok()
                            .flatten();
                        self.check_origin = self.check_task.as_ref().map(|_| CheckOrigin::Initial);
                        self.observed_publication = session
                            .repository
                            .index()
                            .status()
                            .ok()
                            .as_ref()
                            .map(Publication::from);
                        let replacement = session.clone();
                        let old = self.session.replace(session);
                        self.request_directories();
                        // The replacement has no periodic scheduler yet. Its
                        // control reflects the actual state until transfer is
                        // confirmed by the policy worker.
                        self.index_ui.automatic_check = false;
                        self.transfer_check_policy(replacement, old);
                        self.invalidate_published_contexts(updates);
                        self.index_ui.load = IndexTaskState::Succeeded("Index loaded".into());
                        if let Some(session) = &self.session
                            && let Ok(status) = backend::index_lifecycle::status(session)
                        {
                            self.update_index_ui(&status, updates);
                        }
                        updates.push(UiUpdate::IndexReplaced(self.index_ui.clone()));
                        if self.check_task.is_none() {
                            self.refresh_results_if_ready(updates);
                        }
                    }
                    Err(message) => {
                        self.index_ui.load = IndexTaskState::Failed(message);
                        updates.push(UiUpdate::IndexStatus(self.index_ui.clone()));
                    }
                }
            }
            WorkerEvent::IndexLoaded {
                result: Ok(discarded),
                ..
            } => {
                // A superseded load owns an initial reconciliation worker;
                // ignoring the reply alone would leave that worker detached.
                thread::spawn(move || {
                    let _ = backend::index_lifecycle::shutdown(&discarded);
                });
            }
            WorkerEvent::IndexShutdown {
                open_generation,
                operation_id,
                result,
            } if open_generation == self.open_generation
                && operation_id == self.close_operation_id =>
            {
                self.closing = false;
                match result {
                    Ok(()) => updates.push(UiUpdate::CloseReady),
                    Err(error) => updates.push(UiUpdate::ClosingIndex { error: Some(error) }),
                }
            }
            WorkerEvent::IndexCloseCancelled {
                open_generation,
                operation_id,
                result,
            } if open_generation == self.open_generation
                && operation_id == self.close_operation_id =>
            {
                self.closing = false;
                updates.push(UiUpdate::CloseCancelled);
                if let Err(message) = result {
                    self.index_ui.automatic_check = false;
                    self.index_ui.automatic_save = false;
                    updates.push(UiUpdate::IndexStatus(self.index_ui.clone()));
                    updates.push(UiUpdate::Error {
                        field: ErrorField::General,
                        message,
                    });
                }
            }
            WorkerEvent::AutomaticCheckChanged {
                open_generation,
                operation_id,
                enabled,
                interval_seconds,
                result,
            } if open_generation == self.open_generation
                && operation_id == self.automatic_check_operation_id =>
            {
                match result {
                    Ok(()) => {
                        self.index_ui.automatic_check = enabled;
                        self.index_ui.check_interval_seconds = interval_seconds;
                        if let (Some(session), Some(preferences)) =
                            (&self.session, &mut self.preferences)
                        {
                            let key = lifecycle_source_key(session);
                            if preferences
                                .set_automatic_check_local(&key, enabled, interval_seconds)
                                .is_ok()
                            {
                                let _ = self.preference_tx.send(preferences.clone());
                            }
                        }
                        updates.push(UiUpdate::IndexStatus(self.index_ui.clone()));
                    }
                    Err(message) => {
                        self.index_ui.automatic_check = false;
                        self.requested_check.0 = false;
                        self.index_ui.check = IndexTaskState::Failed(format!(
                            "Automatic checking is unavailable: {message}"
                        ));
                        updates.push(UiUpdate::IndexStatus(self.index_ui.clone()));
                        updates.push(UiUpdate::Error {
                            field: ErrorField::General,
                            message,
                        });
                    }
                }
            }
            WorkerEvent::AutomaticSaveChanged {
                open_generation,
                operation_id,
                enabled,
                result,
            } if open_generation == self.open_generation
                && operation_id == self.automatic_save_operation_id =>
            {
                match result {
                    Ok(()) => {
                        self.index_ui.automatic_save = enabled;
                        if let (Some(session), Some(preferences)) =
                            (&self.session, &mut self.preferences)
                        {
                            let key = lifecycle_source_key(session);
                            if let Some(destination) = session.configuration.cache_save.as_ref() {
                                if preferences
                                    .set_automatic_save_local(
                                        &key,
                                        &destination.display().to_string(),
                                        enabled,
                                    )
                                    .is_ok()
                                {
                                    let _ = self.preference_tx.send(preferences.clone());
                                }
                            }
                        }
                        updates.push(UiUpdate::IndexStatus(self.index_ui.clone()));
                    }
                    Err(message) => updates.push(UiUpdate::Error {
                        field: ErrorField::General,
                        message,
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
                export_generation,
                result,
            } if open_generation == self.open_generation
                && export_generation == self.export_generation =>
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
                batch_generation,
                result,
            } if open_generation == self.open_generation
                && batch_generation == self.batch_generation =>
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
                publication,
                tab_id,
                operation_generation,
                result,
            } if open_generation == self.open_generation
                && self.session_publication_is_current(publication)
                && self.active_tab == Some(tab_id)
                && self.tabs.iter().any(|tab| {
                    tab.id == tab_id
                        && tab.operation_generation == operation_generation
                        && matches!(
                            tab.freshness,
                            TabFreshness::Current | TabFreshness::NavigationUnavailable
                        )
                }) =>
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
            pinned: true,
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
            freshness: TabFreshness::Current,
            navigation_outdated: false,
            cached_detail: None,
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
        let selected_key = if format == ExportFormat::SelectedJson {
            let Some(tab) = self
                .active_tab
                .and_then(|id| self.tabs.iter().find(|tab| tab.id == id))
            else {
                updates.push(UiUpdate::Error {
                    field: ErrorField::Export,
                    message: "Select a record before exporting it".into(),
                });
                return;
            };
            if !matches!(
                tab.freshness,
                TabFreshness::Current | TabFreshness::NavigationUnavailable
            ) {
                updates.push(UiUpdate::Error {
                    field: ErrorField::Export,
                    message: "Refresh this record before exporting it".into(),
                });
                return;
            }
            Some(tab.key.clone())
        } else {
            None
        };
        let view = match session.repository.index().view() {
            Ok(view) => view,
            Err(error) => {
                updates.push(UiUpdate::Error {
                    field: ErrorField::Export,
                    message: error.to_string(),
                });
                return;
            }
        };
        let open_generation = self.open_generation;
        self.export_cancellation.cancel();
        self.export_cancellation = CancellationToken::new();
        self.export_generation += 1;
        let export_generation = self.export_generation;
        let token = self.export_cancellation.clone();
        let sender = self.worker_tx.clone();
        updates.push(UiUpdate::ClearError(ErrorField::Export));
        updates.push(UiUpdate::Status("Exporting…".into()));
        thread::spawn(move || {
            let result = match format {
                ExportFormat::SelectedJson => match selected_key {
                    Some(key) => backend::export_selected_json_on_view(
                        &session,
                        &view,
                        &key.collection,
                        &key.id,
                        &path,
                        &token,
                    )
                    .map(|()| format!("Selected record exported to {}", path.display()))
                    .map_err(|error| error.to_string()),
                    None => Err("Select a record before exporting it".into()),
                },
                ExportFormat::ResultsJson => match spec {
                    Some(spec) => {
                        backend::export_all_json_on_view(&session, &view, &spec, &path, &token)
                            .map(|count| format!("Exported {count} results to {}", path.display()))
                            .map_err(|error| error.to_string())
                    }
                    None => Err("Run a query before exporting results".into()),
                },
                ExportFormat::Ndjson => match spec {
                    Some(spec) => {
                        backend::export_query_ndjson_on_view(&session, &view, &spec, &path, &token)
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
                            .map_err(|error| error.to_string())
                    }
                    None => Err("Run a query before exporting NDJSON".into()),
                },
            };
            let _ = sender.send(WorkerEvent::Exported {
                open_generation,
                export_generation,
                result,
            });
        });
    }

    fn request_close(&mut self, updates: &mut Vec<UiUpdate>) {
        if self.closing {
            return;
        }
        let Some(session) = self.session.clone() else {
            updates.push(UiUpdate::CloseReady);
            return;
        };
        if !self.index_ui.automatic_save {
            self.close_without_saving(updates);
            return;
        }
        if matches!(self.index_ui.save, IndexTaskState::Running { .. }) {
            updates.push(UiUpdate::ClosingIndex {
                error: Some("Wait for the current index save to finish, then Retry".into()),
            });
            return;
        }
        self.invalidate_policy_for_close();
        self.closing = true;
        self.close_operation_id = self.close_operation_id.wrapping_add(1);
        let operation_id = self.close_operation_id;
        let open_generation = self.open_generation;
        let sender = self.worker_tx.clone();
        let check_gate = Arc::clone(&self.automatic_check_gate);
        let save_gate = Arc::clone(&self.automatic_save_gate);
        updates.push(UiUpdate::ClosingIndex { error: None });
        thread::spawn(move || {
            let result = match check_gate.lock() {
                Ok(_check_gate) => match save_gate.lock() {
                    Ok(_save_gate) => backend::index_lifecycle::shutdown_with_final_save(&session)
                        .map_err(|error| error.to_string()),
                    Err(_) => Err("automatic saving coordinator is unavailable".into()),
                },
                Err(_) => Err("automatic checking coordinator is unavailable".into()),
            };
            let _ = sender.send(WorkerEvent::IndexShutdown {
                open_generation,
                operation_id,
                result,
            });
        });
    }

    fn invalidate_policy_for_close(&mut self) {
        self.load_cancellation.cancel();
        self.load_operation_id = self.load_operation_id.wrapping_add(1);
        self.automatic_check_operation_id = self.automatic_check_operation_id.wrapping_add(1);
        self.automatic_check_epoch
            .store(self.automatic_check_operation_id, Ordering::Release);
        self.invalidate_automatic_save_policy();
    }

    fn invalidate_automatic_save_policy(&mut self) {
        self.automatic_save_operation_id = self.automatic_save_operation_id.wrapping_add(1);
        self.automatic_save_epoch
            .store(self.automatic_save_operation_id, Ordering::Release);
    }

    fn close_without_saving(&mut self, updates: &mut Vec<UiUpdate>) {
        if self.closing {
            return;
        }
        let Some(session) = self.session.clone() else {
            updates.push(UiUpdate::CloseReady);
            return;
        };
        self.invalidate_policy_for_close();
        self.closing = true;
        self.close_operation_id = self.close_operation_id.wrapping_add(1);
        let operation_id = self.close_operation_id;
        let open_generation = self.open_generation;
        let sender = self.worker_tx.clone();
        let check_gate = Arc::clone(&self.automatic_check_gate);
        let save_gate = Arc::clone(&self.automatic_save_gate);
        updates.push(UiUpdate::ClosingIndex { error: None });
        thread::spawn(move || {
            let result = match check_gate.lock() {
                Ok(_check_gate) => match save_gate.lock() {
                    Ok(_save_gate) => backend::index_lifecycle::shutdown(&session)
                        .map_err(|error| error.to_string()),
                    Err(_) => Err("automatic saving coordinator is unavailable".into()),
                },
                Err(_) => Err("automatic checking coordinator is unavailable".into()),
            };
            let _ = sender.send(WorkerEvent::IndexShutdown {
                open_generation,
                operation_id,
                result,
            });
        });
    }

    fn cancel_close(&mut self, updates: &mut Vec<UiUpdate>) {
        if self.closing {
            return;
        }
        let Some(session) = self.session.clone() else {
            updates.push(UiUpdate::CloseCancelled);
            return;
        };
        self.closing = true;
        self.close_operation_id = self.close_operation_id.wrapping_add(1);
        let operation_id = self.close_operation_id;
        let open_generation = self.open_generation;
        let (check_enabled, interval_seconds) = self.requested_check;
        let automatic_save = self.index_ui.automatic_save;
        let sender = self.worker_tx.clone();
        let gate = Arc::clone(&self.automatic_check_gate);
        let save_gate = Arc::clone(&self.automatic_save_gate);
        thread::spawn(move || {
            let result = match gate.lock() {
                Ok(_guard) => match save_gate.lock() {
                    Ok(_save_guard) => backend::index_lifecycle::resume_after_cancelled_close(
                        &session,
                        check_enabled.then(|| Duration::from_secs(interval_seconds)),
                        automatic_save,
                    )
                    .map_err(|error| error.to_string()),
                    Err(_) => Err("automatic saving coordinator is unavailable".into()),
                },
                Err(_) => Err("automatic checking coordinator is unavailable".into()),
            };
            let _ = sender.send(WorkerEvent::IndexCloseCancelled {
                open_generation,
                operation_id,
                result,
            });
        });
    }

    fn start_check(&mut self, updates: &mut Vec<UiUpdate>) {
        let Some(session) = &self.session else {
            return;
        };
        match backend::index_lifecycle::check_for_changes(session) {
            Ok(task) => {
                self.index_operation_id = self.index_operation_id.wrapping_add(1);
                self.check_task = Some(task);
                self.check_origin = Some(CheckOrigin::Explicit);
                self.index_ui.check = IndexTaskState::Running {
                    operation_id: self.index_operation_id,
                    processed: Some(0),
                    total: None,
                };
                updates.push(UiUpdate::IndexStatus(self.index_ui.clone()));
            }
            Err(error) => {
                self.index_ui.check = IndexTaskState::Failed(error.to_string());
                updates.push(UiUpdate::IndexStatus(self.index_ui.clone()));
            }
        }
    }

    fn start_maintenance(&mut self, operation: MaintenanceOperation, updates: &mut Vec<UiUpdate>) {
        let Some(session) = &self.session else {
            return;
        };
        if self.maintenance_task.is_some() {
            self.index_ui.maintenance =
                IndexTaskState::Failed("Another index maintenance task is running".into());
            updates.push(UiUpdate::IndexStatus(self.index_ui.clone()));
            return;
        }
        match backend::index_lifecycle::start_maintenance(session, operation) {
            Ok(task) => {
                self.index_operation_id = self.index_operation_id.wrapping_add(1);
                self.maintenance_task = Some(task);
                self.index_ui.maintenance_kind = Some(match operation {
                    MaintenanceOperation::Rebuild => IndexTaskKind::Rebuild,
                    MaintenanceOperation::Compact => IndexTaskKind::Compact,
                });
                self.index_ui.maintenance = IndexTaskState::Running {
                    operation_id: self.index_operation_id,
                    processed: None,
                    total: None,
                };
                updates.push(UiUpdate::IndexStatus(self.index_ui.clone()));
            }
            Err(error) => {
                self.index_ui.maintenance = IndexTaskState::Failed(error.to_string());
                updates.push(UiUpdate::IndexStatus(self.index_ui.clone()));
            }
        }
    }

    fn save_index(
        &mut self,
        requested: Option<String>,
        allow_replace: bool,
        make_default: bool,
        cx: &mut App,
        updates: &mut Vec<UiUpdate>,
    ) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let destination = requested
            .filter(|path| !path.trim().is_empty())
            .map(PathBuf::from)
            .or_else(|| session.configuration.cache_save.clone());
        let Some(destination) = destination else {
            self.pending_save_select_default = make_default;
            self.index_picker_generation = self.index_picker_generation.wrapping_add(1);
            self.choose_index_path(self.index_picker_generation, false, true, cx);
            return;
        };
        let destination = if destination.is_absolute() {
            destination
        } else {
            session.paths.root.join(destination)
        };
        if allow_replace && self.pending_index_overwrite.as_ref() != Some(&destination) {
            self.index_ui.save =
                IndexTaskState::Failed("The index destination changed; choose it again".into());
            updates.push(UiUpdate::IndexStatus(self.index_ui.clone()));
            return;
        }
        if matches!(self.index_ui.save, IndexTaskState::Running { .. }) {
            return;
        }
        self.pending_index_overwrite = None;
        self.save_operation_id = self.save_operation_id.wrapping_add(1);
        let operation_id = self.save_operation_id;
        let open_generation = self.open_generation;
        let sender = self.worker_tx.clone();
        self.index_ui.save = IndexTaskState::Running {
            operation_id,
            processed: None,
            total: None,
        };
        updates.push(UiUpdate::IndexStatus(self.index_ui.clone()));
        thread::spawn(move || {
            let selected = session.configuration.cache_save.as_deref();
            let same_selected = selected.is_some_and(|selected| {
                selected == destination
                    || matches!(
                        (selected.canonicalize(), destination.canonicalize()),
                        (Ok(selected), Ok(destination)) if selected == destination
                    )
            });
            let result = if !allow_replace
                && !same_selected
                && std::fs::symlink_metadata(&destination).is_ok()
            {
                IndexSaveCompletion::OverwriteRequired(destination)
            } else {
                let save = if make_default {
                    backend::index_lifecycle::save_index(&session, &destination, allow_replace)
                } else {
                    backend::index_lifecycle::save_index_copy(&session, &destination, allow_replace)
                };
                match save {
                    Ok(saved) => IndexSaveCompletion::Saved(saved),
                    Err(error) => IndexSaveCompletion::Failed(error.to_string()),
                }
            };
            let _ = sender.send(WorkerEvent::IndexSaved {
                open_generation,
                operation_id,
                make_default,
                result,
            });
        });
    }

    fn load_index(&mut self, path: String, cx: &mut App, updates: &mut Vec<UiUpdate>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        if path.trim().is_empty() {
            self.index_picker_generation = self.index_picker_generation.wrapping_add(1);
            self.choose_index_path(self.index_picker_generation, true, true, cx);
            return;
        }
        if self.index_ui.automatic_save
            || matches!(self.index_ui.save, IndexTaskState::Running { .. })
        {
            self.index_ui.load = IndexTaskState::Failed(
                "Finish saving and turn off automatic saving before loading another index".into(),
            );
            updates.push(UiUpdate::IndexStatus(self.index_ui.clone()));
            return;
        }
        let path = PathBuf::from(path.trim());
        let path = if path.is_absolute() {
            path
        } else {
            session.paths.root.join(path)
        };
        self.load_cancellation.cancel();
        self.load_cancellation = CancellationToken::new();
        self.load_operation_id = self.load_operation_id.wrapping_add(1);
        let operation_id = self.load_operation_id;
        let token = self.load_cancellation.clone();
        let open_generation = self.open_generation;
        let sender = self.worker_tx.clone();
        self.index_ui.load = IndexTaskState::Running {
            operation_id,
            processed: None,
            total: None,
        };
        updates.push(UiUpdate::IndexStatus(self.index_ui.clone()));
        thread::spawn(move || {
            let result = backend::index_lifecycle::load_index(&session, &path, &token)
                .map_err(|error| error.to_string());
            let _ = sender.send(WorkerEvent::IndexLoaded {
                open_generation,
                operation_id,
                result,
            });
        });
    }

    /// Apply the latest requested periodic policy to a successfully loaded
    /// replacement before retiring the previous scheduler. The gate and
    /// epoch also prevent late toggle workers from resurrecting an old index.
    fn transfer_check_policy(&mut self, replacement: OpenedSession, old: Option<OpenedSession>) {
        self.automatic_check_operation_id = self.automatic_check_operation_id.wrapping_add(1);
        let operation_id = self.automatic_check_operation_id;
        self.automatic_check_epoch
            .store(operation_id, Ordering::Release);
        let epoch = Arc::clone(&self.automatic_check_epoch);
        let gate = Arc::clone(&self.automatic_check_gate);
        let save_gate = Arc::clone(&self.automatic_save_gate);
        let (enabled, interval_seconds) = self.requested_check;
        let open_generation = self.open_generation;
        let sender = self.worker_tx.clone();
        thread::spawn(move || {
            let result = match gate.lock() {
                Ok(_gate) => {
                    let current = epoch.load(Ordering::Acquire) == operation_id;
                    let applied = if current {
                        backend::index_lifecycle::set_periodic_check(
                            &replacement,
                            enabled.then(|| Duration::from_secs(interval_seconds)),
                        )
                        .map_err(|error| error.to_string())
                    } else {
                        Ok(())
                    };
                    let retired = match save_gate.lock() {
                        Ok(_save_gate) => old
                            .as_ref()
                            .map(backend::index_lifecycle::stop_automation)
                            .transpose()
                            .map(|_| ())
                            .map_err(|error| error.to_string()),
                        Err(_) => Err("automatic saving coordinator is unavailable".into()),
                    };
                    if !current {
                        return;
                    }
                    applied.and(retired)
                }
                Err(_) => Err("automatic checking coordinator is unavailable".into()),
            };
            let _ = sender.send(WorkerEvent::AutomaticCheckChanged {
                open_generation,
                operation_id,
                enabled,
                interval_seconds,
                result,
            });
        });
    }

    fn cancel_index_task(&mut self, kind: IndexTaskKind, updates: &mut Vec<UiUpdate>) {
        match kind {
            IndexTaskKind::Check => {
                if let Some(task) = &self.check_task {
                    task.cancel();
                }
            }
            IndexTaskKind::Rebuild | IndexTaskKind::Compact => {
                if let Some(task) = &self.maintenance_task {
                    task.cancel();
                }
            }
            IndexTaskKind::Load => {
                self.load_cancellation.cancel();
                self.load_operation_id = self.load_operation_id.wrapping_add(1);
                self.index_ui.load = IndexTaskState::Cancelled;
            }
            IndexTaskKind::Save => {
                updates.push(UiUpdate::Status(
                    "An index save already in progress finishes atomically".into(),
                ));
            }
            IndexTaskKind::Extended => {
                if let Some(session) = &self.session {
                    if let Err(error) = session.repository.index().cancel_extended() {
                        updates.push(UiUpdate::Error {
                            field: ErrorField::General,
                            message: error.to_string(),
                        });
                    }
                }
            }
        }
        updates.push(UiUpdate::IndexStatus(self.index_ui.clone()));
    }

    fn set_automatic_check(
        &mut self,
        enabled: bool,
        interval_seconds: u64,
        updates: &mut Vec<UiUpdate>,
    ) {
        let Some(session) = self.session.clone() else {
            return;
        };
        if !(1..=86_400).contains(&interval_seconds) {
            updates.push(UiUpdate::Error {
                field: ErrorField::General,
                message: "Check interval must be between 1 and 86,400 seconds".into(),
            });
            return;
        }
        self.requested_check = (enabled, interval_seconds);
        self.automatic_check_operation_id = self.automatic_check_operation_id.wrapping_add(1);
        let operation_id = self.automatic_check_operation_id;
        self.automatic_check_epoch
            .store(operation_id, Ordering::Release);
        let epoch = Arc::clone(&self.automatic_check_epoch);
        let gate = Arc::clone(&self.automatic_check_gate);
        let open_generation = self.open_generation;
        let sender = self.worker_tx.clone();
        thread::spawn(move || {
            let result = match gate.lock() {
                Ok(_gate) if epoch.load(Ordering::Acquire) == operation_id => {
                    backend::index_lifecycle::set_periodic_check(
                        &session,
                        enabled.then(|| Duration::from_secs(interval_seconds)),
                    )
                    .map_err(|error| error.to_string())
                }
                Ok(_) => return,
                Err(_) => Err("automatic checking coordinator is unavailable".into()),
            };
            let _ = sender.send(WorkerEvent::AutomaticCheckChanged {
                open_generation,
                operation_id,
                enabled,
                interval_seconds,
                result,
            });
        });
    }

    fn set_automatic_save(&mut self, enabled: bool, updates: &mut Vec<UiUpdate>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        if enabled && !self.index_ui.automatic_save_available {
            updates.push(UiUpdate::Error {
                field: ErrorField::General,
                message: self
                    .index_ui
                    .automatic_save_reason
                    .clone()
                    .unwrap_or_else(|| {
                        "Choose a configured repository index destination first".into()
                    }),
            });
            return;
        }
        self.automatic_save_operation_id = self.automatic_save_operation_id.wrapping_add(1);
        let operation_id = self.automatic_save_operation_id;
        self.automatic_save_epoch
            .store(operation_id, Ordering::Release);
        let epoch = Arc::clone(&self.automatic_save_epoch);
        let gate = Arc::clone(&self.automatic_save_gate);
        let open_generation = self.open_generation;
        let sender = self.worker_tx.clone();
        thread::spawn(move || {
            let result = match gate.lock() {
                Ok(_gate) if epoch.load(Ordering::Acquire) == operation_id => {
                    backend::index_lifecycle::set_automatic_save(&session, enabled)
                        .map_err(|error| error.to_string())
                }
                Ok(_) => return,
                Err(_) => Err("automatic saving coordinator is unavailable".into()),
            };
            let _ = sender.send(WorkerEvent::AutomaticSaveChanged {
                open_generation,
                operation_id,
                enabled,
                result,
            });
        });
    }

    fn refresh_record(&mut self, id: u64, updates: &mut Vec<UiUpdate>) {
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) else {
            return;
        };
        tab.freshness = TabFreshness::Refreshing;
        self.emit_tabs(updates);
        self.start_tab_load(id, TabLoadKind::Refresh, updates);
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
        let view = match session.repository.index().view() {
            Ok(view) => view,
            Err(error) => {
                updates.push(UiUpdate::Error {
                    field: ErrorField::Batch,
                    message: error.to_string(),
                });
                return;
            }
        };
        let path = PathBuf::from(output_path.trim());
        self.batch_cancellation.cancel();
        self.batch_cancellation = CancellationToken::new();
        self.batch_generation += 1;
        let batch_generation = self.batch_generation;
        let open_generation = self.open_generation;
        let token = self.batch_cancellation.clone();
        let sender = self.worker_tx.clone();
        updates.push(UiUpdate::ClearError(ErrorField::Batch));
        updates.push(UiUpdate::Batch {
            rows: Vec::new(),
            running: true,
            outcome: None,
        });
        thread::spawn(move || {
            let result = backend::run_batch_on_view(&session, &view, &spec, &path, &token)
                .map(|report| {
                    let rows = batch_rows(&report.path).unwrap_or_default();
                    (report, rows)
                })
                .map_err(|error| error.to_string());
            let _ = sender.send(WorkerEvent::Batched {
                open_generation,
                batch_generation,
                result,
            });
        });
    }
}

fn open_with_preferences_staged(
    spec: &OpenSpec,
    token: &CancellationToken,
    on_task: impl FnOnce(StagedBuildTask, PathBuf, Vec<String>),
) -> OpenCompletion {
    let session = backend::open_staged(spec, token, on_task)
        .map_err(|error| error.to_string())?;
    finish_open_session(session)
}

fn finish_open_session(session: OpenedSession) -> OpenCompletion {
    let preferences = DesktopPreferences::load(&session.paths.root);
    let view = session.repository.index().view().map_err(|error| error.to_string())?;
    let publication = Publication {
        instance_id: view.instance_id(),
        revision: view.revision(),
    };
    let directories = indexed_directories(&view, &session.collections);
    Ok((session, preferences, publication, directories))
}

fn indexed_directories(view: &IndexView, collections: &[String]) -> DirectoryCatalog {
    let all = parent_directories(view.file_paths());
    let by_collection = collections
        .iter()
        .map(|name| {
            (
                name.clone(),
                parent_directories(view.file_paths_for_collection(name)),
            )
        })
        .collect();
    DirectoryCatalog { all, by_collection }
}

/// Build the virtual-list rows and collection scopes on the discovery worker.
/// The UI receives only cheap Arc/map moves even for a very large catalogue.
fn prepare_staged_catalogue(
    files: Vec<StagedFile>,
    collections: &[String],
) -> (DirectoryCatalog, FileCatalogue) {
    let mut all_directories = BTreeSet::new();
    let mut scoped_directories = vec![BTreeSet::new(); collections.len()];
    let mut scoped_indices = vec![Vec::new(); collections.len()];
    let mut rows = Vec::with_capacity(files.len());
    for (index, file) in files.into_iter().enumerate() {
        let collection_ids = file.collections.iter().map(|id| id.0).collect::<Vec<_>>();
        for id in &collection_ids {
            if let Some(indices) = scoped_indices.get_mut(*id as usize) {
                indices.push(index);
            }
        }
        let mut parent = file.relative_path.parent();
        while let Some(directory) = parent.filter(|directory| !directory.as_os_str().is_empty()) {
            let path = objekte_search_core::path_repr::query_path(directory);
            all_directories.insert(path.clone());
            for id in &collection_ids {
                if let Some(directories) = scoped_directories.get_mut(*id as usize) {
                    directories.insert(path.clone());
                }
            }
            parent = directory.parent();
        }
        rows.push(CatalogueFile {
            id: file.id.0,
            relative_path: objekte_search_core::path_repr::query_path(&file.relative_path),
            collections: collection_ids,
            size: file.size,
            created_ns: file.created_ns,
        });
    }
    let directories = DirectoryCatalog {
        all: all_directories.into_iter().collect(),
        by_collection: collections.iter().cloned()
            .zip(scoped_directories.into_iter().map(|set| set.into_iter().collect()))
            .collect(),
    };
    let file_catalogue = FileCatalogue {
        files: Arc::new(rows),
        by_collection: collections.iter().cloned()
            .zip(scoped_indices.into_iter().map(Arc::new))
            .collect(),
    };
    (directories, file_catalogue)
}

fn parent_directories<'a>(paths: impl Iterator<Item = &'a Path>) -> Vec<String> {
    let mut directories = BTreeSet::new();
    for path in paths {
        let mut parent = path.parent();
        while let Some(directory) = parent.filter(|directory| !directory.as_os_str().is_empty()) {
            directories.insert(objekte_search_core::path_repr::query_path(directory));
            parent = directory.parent();
        }
    }
    directories.into_iter().collect()
}

fn lifecycle_source_key(session: &OpenedSession) -> String {
    let mut key = session.paths.root.display().to_string();
    key.push('|');
    for byte in session.configuration.configuration_identity {
        use std::fmt::Write as _;
        let _ = write!(&mut key, "{byte:02x}");
    }
    key
}

fn record_presentation(
    _session: &OpenedSession,
    spec: Option<&backend::QuerySpec>,
    hit: SearchHit,
) -> (SearchHit, DetailView, Vec<Option<LinkAction>>) {
    let view = detail_view_with_query(&hit, &HashSet::new(), spec);
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
fn tab_result_hit_on_view(
    session: &OpenedSession,
    view: &IndexView,
    spec: &backend::QuerySpec,
    anchor: usize,
    key: &RecordKey,
    token: &CancellationToken,
) -> Result<Option<SearchHit>, backend::BackendError> {
    let mut page = backend::query_page_on_view(session, view, spec, anchor, 1, token)?;
    if let Some(hit) = page.hits.pop()
        && hit.collection == key.collection
        && hit.record_id == key.id
    {
        return Ok(Some(hit));
    }
    Ok(None)
}

fn canonical_hit_on_view(
    view: &IndexView,
    key: &RecordKey,
    token: &CancellationToken,
) -> Result<Option<SearchHit>, backend::BackendError> {
    match backend::record_by_id_on_view(view, &key.collection, &key.id, token)? {
        DirectLookupResult::Found(hit) => Ok(Some(hit)),
        DirectLookupResult::NotFound => Ok(None),
        DirectLookupResult::SourceUnavailable(diagnostic) => Err(backend::BackendError::new(
            backend::BackendErrorKind::Query,
            diagnostic.message,
        )),
    }
}

fn posting_completions(
    session: &OpenedSession,
    request: &CompletionRequest,
    token: &CancellationToken,
) -> CompletionCacheValue {
    let mut values = Vec::new();
    let mut partial = false;
    let mut visited_keys = 0;
    let path = match objekte_search_core::model::FieldPath::parse(&request.field) {
        Ok(path) => path,
        Err(_) => return (values, true, visited_keys),
    };
    let mut visited_collections = 0;
    for collection in session.repository.collections() {
        if token.is_cancelled() {
            break;
        }
        if request
            .collection
            .as_deref()
            .is_some_and(|scope| scope != collection.definition.name)
        {
            continue;
        }
        // One worker visits a bounded set of collection dictionaries. A
        // larger all-collection scope is explicitly partial in the popup.
        visited_collections += 1;
        if visited_collections > 8 {
            partial = true;
            break;
        }
        if let Some(declared) = session
            .declared_values
            .get(&(collection.definition.name.clone(), request.field.clone()))
        {
            for literal in declared
                .iter()
                .filter(|literal| completion_literal_matches(literal, &request.prefix))
                .take(20)
            {
                if values.len() >= 50 {
                    partial = true;
                    break;
                }
                add_completion_value(
                    &mut values,
                    literal.clone(),
                    &collection.definition.name,
                    request.collection.is_some(),
                );
            }
            partial |= declared.len() > 20;
        }
        if values.len() >= 50 {
            partial = true;
            break;
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
            if !completion_literal_matches(&literal, &request.prefix) {
                continue;
            }
            add_completion_value(
                &mut values,
                literal,
                &collection.definition.name,
                request.collection.is_some(),
            );
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

fn completion_literal_matches(literal: &str, prefix: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    let value = serde_json::from_str::<String>(literal).unwrap_or_else(|_| literal.to_owned());
    value.to_lowercase().contains(&prefix.to_lowercase())
}

fn add_completion_value(
    values: &mut Vec<(String, String)>,
    literal: String,
    collection: &str,
    scoped: bool,
) {
    if let Some((_, label)) = values.iter_mut().find(|(existing, _)| existing == &literal) {
        if !scoped && !label.ends_with(collection) {
            label.push_str(", ");
            label.push_str(collection);
        }
    } else {
        let label = if scoped {
            literal.clone()
        } else {
            format!("{literal} · {collection}")
        };
        values.push((literal, label));
    }
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
