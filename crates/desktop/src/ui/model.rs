//! Bounded, renderer-owned state and typed actions for the desktop window.
//!
//! Repository handles, query execution, and I/O live outside this module. The
//! controller checks operation generations before it sends updates here.

/// Maximum number of result rows retained by one window page.
pub const MAX_PAGE_ROWS: usize = 100;
/// Maximum UTF-8 bytes retained by one complete result page.
pub const MAX_PAGE_BYTES: usize = 4 * 1024 * 1024;
/// Maximum number of display columns retained in the window.
pub const MAX_COLUMNS: usize = 64;
/// Maximum number of detail rows held by the window at once.
pub const MAX_DETAIL_ROWS: usize = 512;
/// Maximum UTF-8 bytes retained by one complete detail panel.
pub const MAX_DETAIL_BYTES: usize = 2 * 1024 * 1024;
/// Maximum UTF-8 bytes of one visible cell, preview, or detail value.
pub const MAX_CELL_BYTES: usize = 8 * 1024;
/// Maximum UTF-8 bytes of visible raw source text.
pub const MAX_SOURCE_BYTES: usize = 256 * 1024;
/// Maximum number of batch statuses retained in the visible list.
pub const MAX_BATCH_ROWS: usize = 200;

/// Repository opening modes supported by the existing CLI.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OpenMode {
    #[default]
    Folder,
    MultipleFolders,
    ConfiguredRepository,
}

/// Snapshot operation chosen explicitly by the user.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SnapshotMode {
    #[default]
    None,
    Write,
    Load,
    LoadAndWrite,
}

/// Editable opening options; strings stay intact until backend validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenDraft {
    pub mode: OpenMode,
    /// One path per physical line for multiple-folder mode.
    pub paths: String,
    pub schema: String,
    pub recursive: bool,
    pub auto_index: bool,
    pub unicode_index: bool,
    pub snapshot: SnapshotMode,
    pub snapshot_directory: String,
    /// Configured repositories use their declared schemas/cache by default.
    /// A selected manual snapshot policy takes precedence when this is false.
    pub configured_use_repository_settings: bool,
    pub diagnostic: bool,
}

impl Default for OpenDraft {
    fn default() -> Self {
        Self {
            mode: OpenMode::Folder,
            paths: String::new(),
            schema: String::new(),
            recursive: true,
            auto_index: false,
            unicode_index: false,
            snapshot: SnapshotMode::None,
            snapshot_directory: String::new(),
            configured_use_repository_settings: true,
            diagnostic: false,
        }
    }
}

/// The two independently editable query inputs and execution options.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchDraft {
    pub full_text: String,
    pub structured: String,
    /// The filter line can retain an inactive draft without executing it.
    pub filter_enabled: bool,
    /// None searches all collections.
    pub collection: Option<String>,
    /// Repository-relative directory selected in the file tree.
    pub directory: Option<String>,
    pub case_sensitive: bool,
    pub regex: bool,
}

impl Default for SearchDraft {
    fn default() -> Self {
        Self {
            full_text: String::new(),
            structured: String::new(),
            filter_enabled: false,
            collection: None,
            directory: None,
            case_sensitive: true,
            regex: false,
        }
    }
}

/// Validation of the current search and filter text, separate from the last result page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FilterValidationState {
    Pending,
    Valid,
    Invalid {
        message: String,
        byte_offset: Option<usize>,
    },
}

/// A stable controller token plus labels for one visible result.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct RowKey {
    /// Opaque controller-owned identity; labels may be clipped for display.
    pub token: u64,
    pub collection: String,
    pub record_id: String,
    pub file: String,
    pub byte_start: u64,
}

/// One bounded, already projected result row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResultRow {
    pub key: RowKey,
    pub cells: Vec<String>,
    pub matched_fields: Vec<String>,
    /// Byte ranges in the Match cell: `true` marks an exact FTS span;
    /// `false` marks a verified field label.
    pub match_highlights: Vec<(std::ops::Range<usize>, bool)>,
    pub preview: Option<String>,
}

/// A bounded page and exact-count state, separate from total-result range.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PageView {
    pub columns: Vec<String>,
    pub rows: Vec<ResultRow>,
    pub offset: usize,
    pub requested_limit: Option<usize>,
    pub exact_total: Option<usize>,
    /// Missing source data prevents inferring an exact total from exhaustion.
    pub incomplete: bool,
    pub count_pending: bool,
    pub has_previous: bool,
    pub has_next: bool,
    pub selected: Option<usize>,
    /// True when the backend omitted a preview or cells due to size bounds.
    pub clipped: bool,
}

impl PageView {
    /// Enforce renderer memory bounds without changing backend query semantics.
    pub fn bounded(mut self) -> Self {
        if self.columns.len() > MAX_COLUMNS {
            self.columns.truncate(MAX_COLUMNS);
            self.clipped = true;
        }
        if self.rows.len() > MAX_PAGE_ROWS {
            self.rows.truncate(MAX_PAGE_ROWS);
            self.has_next = true;
            self.clipped = true;
        }
        let mut budget = MAX_PAGE_BYTES;
        for column in &mut self.columns {
            self.clipped |= fit_budget(column, &mut budget, MAX_CELL_BYTES);
        }
        for row in &mut self.rows {
            if row.cells.len() > MAX_COLUMNS {
                row.cells.truncate(MAX_COLUMNS);
                self.clipped = true;
            }
            self.clipped |= fit_budget(&mut row.key.collection, &mut budget, MAX_CELL_BYTES);
            self.clipped |= fit_budget(&mut row.key.record_id, &mut budget, MAX_CELL_BYTES);
            self.clipped |= fit_budget(&mut row.key.file, &mut budget, MAX_CELL_BYTES);
            for cell in &mut row.cells {
                self.clipped |= fit_budget(cell, &mut budget, MAX_CELL_BYTES);
            }
            if row.matched_fields.len() > MAX_COLUMNS {
                row.matched_fields.truncate(MAX_COLUMNS);
                self.clipped = true;
            }
            for field in &mut row.matched_fields {
                self.clipped |= fit_budget(field, &mut budget, MAX_CELL_BYTES);
            }
            if let Some(preview) = &mut row.preview {
                self.clipped |= fit_budget(preview, &mut budget, MAX_CELL_BYTES);
            }
        }
        if self
            .selected
            .is_some_and(|selected| selected >= self.rows.len())
        {
            self.selected = None;
        }
        self
    }
}

/// Where one detail row came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DetailOrigin {
    Structured,
    Metadata,
    Diagnostic,
}

/// Distinguish a present JSON null from unavailable logical data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DetailValueKind {
    Present,
    JsonNull,
    LogicalUnavailable,
}

/// One structured record field, metadata entry, or section heading.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DetailRow {
    pub label: String,
    pub value: String,
    pub depth: usize,
    pub matched: bool,
    pub filter_participant: bool,
    pub fts_matched: bool,
    pub value_highlights: Vec<std::ops::Range<usize>>,
    pub expandable: bool,
    pub expanded: bool,
    pub origin: DetailOrigin,
    pub value_kind: DetailValueKind,
}

/// One outgoing relation, backlink, or continuation choice.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelationRow {
    pub label: String,
    pub value: String,
    pub available: bool,
    pub continuation: bool,
    pub target: bool,
}

/// The selected record's bounded inspection content.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DetailView {
    pub title: String,
    pub validity: Option<bool>,
    pub structured: Vec<DetailRow>,
    pub raw_source: String,
    pub source_label: String,
    pub diagnostic: Option<String>,
    pub relations: Vec<RelationRow>,
    pub selected_row: usize,
    pub clipped: bool,
}

impl DetailView {
    /// Prefer a present top-level name for the heading shown in the inspector
    /// and retained record tabs. The canonical identity remains in `title`.
    pub fn display_title(&self) -> &str {
        for field in ["$.name", "$.title"] {
            if let Some(row) = self.structured.iter().find(|row| {
                row.origin == DetailOrigin::Structured
                    && row.label == field
                    && row.value_kind == DetailValueKind::Present
                    && !row.expandable
                    && !row.value.trim().is_empty()
            }) {
                return &row.value;
            }
        }
        &self.title
    }

    /// Keep nested records and source text from growing the render cache.
    pub fn bounded(mut self) -> Self {
        if self.structured.len() > MAX_DETAIL_ROWS {
            self.structured.truncate(MAX_DETAIL_ROWS);
            self.clipped = true;
        }
        if self.relations.len() > MAX_DETAIL_ROWS {
            self.relations.truncate(MAX_DETAIL_ROWS);
            self.clipped = true;
        }
        let mut budget = MAX_DETAIL_BYTES;
        self.clipped |= fit_budget(&mut self.title, &mut budget, MAX_CELL_BYTES);
        self.clipped |= fit_budget(&mut self.source_label, &mut budget, MAX_CELL_BYTES);
        for row in &mut self.structured {
            self.clipped |= fit_budget(&mut row.label, &mut budget, MAX_CELL_BYTES);
            self.clipped |= fit_budget(&mut row.value, &mut budget, MAX_CELL_BYTES);
            row.value_highlights.retain(|span| {
                span.start < span.end
                    && span.end <= row.value.len()
                    && row.value.is_char_boundary(span.start)
                    && row.value.is_char_boundary(span.end)
            });
        }
        for row in &mut self.relations {
            self.clipped |= fit_budget(&mut row.label, &mut budget, MAX_CELL_BYTES);
            self.clipped |= fit_budget(&mut row.value, &mut budget, MAX_CELL_BYTES);
        }
        if let Some(diagnostic) = &mut self.diagnostic {
            self.clipped |= fit_budget(diagnostic, &mut budget, MAX_CELL_BYTES);
        }
        self.clipped |= fit_budget(&mut self.raw_source, &mut budget, MAX_SOURCE_BYTES);
        self
    }
}

/// A query stored in history or under a user-provided name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryEntry {
    /// Stable bookmark identity; history entries have no bookmark ID.
    pub id: Option<u64>,
    pub name: Option<String>,
    pub draft: SearchDraft,
}

/// Lightweight identity and navigation controls for a record view.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordTabSummary {
    pub id: u64,
    /// A selected row uses the temporary split preview; only pinned records
    /// belong in the workspace tab strip.
    pub pinned: bool,
    pub title: String,
    pub identity: String,
    pub position: usize,
    pub can_back: bool,
    pub can_previous: bool,
    pub can_next: bool,
    pub loading: bool,
    pub links_loading: bool,
    pub freshness: TabFreshness,
}

/// Freshness of a tab's displayed record and originating navigation context.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TabFreshness {
    #[default]
    Current,
    Refreshing,
    Outdated,
    NavigationUnavailable,
    Removed,
    Failed,
}

/// Independent index task categories; query Stop never targets these tasks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndexTaskKind {
    Check,
    Save,
    Load,
    Rebuild,
    Compact,
    Extended,
}

/// Terminal outcomes and bounded progress for one index operation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum IndexTaskState {
    #[default]
    Idle,
    Running {
        operation_id: u64,
        processed: Option<u64>,
        total: Option<u64>,
    },
    Succeeded(String),
    Failed(String),
    Cancelled,
}

/// Cheap status snapshot supplied by the active lifecycle handle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexUiStatus {
    pub source_location: String,
    pub opening_mode: String,
    pub loaded_from: Option<String>,
    pub save_destination: Option<String>,
    pub schema_paths: Vec<String>,
    pub indexed_files: Option<usize>,
    pub live_records: Option<usize>,
    pub index_instance: u64,
    pub revision: u64,
    pub capability_revision: u64,
    pub saved_revision: Option<u64>,
    pub saved_capability_revision: Option<u64>,
    pub dirty: bool,
    pub last_check: Option<String>,
    pub check_complete: Option<bool>,
    pub automatic_check: bool,
    pub check_interval_seconds: u64,
    pub automatic_save: bool,
    pub automatic_save_available: bool,
    pub automatic_save_reason: Option<String>,
    pub check: IndexTaskState,
    pub save: IndexTaskState,
    pub load: IndexTaskState,
    pub maintenance: IndexTaskState,
    pub maintenance_kind: Option<IndexTaskKind>,
    pub extended: IndexTaskState,
    pub extended_phase: String,
}

impl Default for IndexUiStatus {
    fn default() -> Self {
        Self {
            source_location: String::new(),
            opening_mode: String::new(),
            loaded_from: None,
            save_destination: None,
            schema_paths: Vec::new(),
            indexed_files: None,
            live_records: None,
            index_instance: 0,
            revision: 0,
            capability_revision: 0,
            saved_revision: None,
            saved_capability_revision: None,
            dirty: false,
            last_check: None,
            check_complete: None,
            automatic_check: false,
            check_interval_seconds: 30,
            automatic_save: false,
            automatic_save_available: false,
            automatic_save_reason: None,
            check: IndexTaskState::Idle,
            save: IndexTaskState::Idle,
            load: IndexTaskState::Idle,
            maintenance: IndexTaskState::Idle,
            maintenance_kind: None,
            extended: IndexTaskState::Idle,
            extended_phase: String::new(),
        }
    }
}

/// One physical batch line and its current execution result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchRow {
    pub line: usize,
    pub query: String,
    pub status: String,
    pub error: Option<String>,
}

/// Explicit source choice when both a pasted batch and file path are present.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BatchSource {
    #[default]
    PastedText,
    QueryFile,
}

/// Which export representation is requested.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ExportFormat {
    SelectedJson,
    #[default]
    ResultsJson,
    Ndjson,
}

/// Which input field should show a nearby validation message.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ErrorField {
    Open,
    FullText,
    Structured,
    Sort,
    Range,
    Batch,
    Export,
    General,
}

/// Local inspector choices and scroll offsets saved with each navigation frame.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TabViewState {
    pub raw: bool,
    pub links: bool,
    pub metadata: bool,
    pub fields_y: i32,
    pub links_y: i32,
    pub source_y: i32,
}

/// Which editor owns a bounded posting-value completion request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionOrigin {
    Filter,
    Builder,
}

/// User intent emitted from GPUI controls to the integrator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiAction {
    DetectOpen {
        path: String,
        generation: u64,
    },
    ChooseFolder {
        draft: OpenDraft,
        slot: usize,
        generation: u64,
    },
    ChooseSchema {
        generation: u64,
    },
    CancelOpen,
    Open(OpenDraft),
    SearchEdited(SearchDraft),
    SearchNow(SearchDraft),
    CompleteValues {
        origin: CompletionOrigin,
        revision: u64,
        collection: Option<String>,
        field: String,
        prefix: String,
    },
    Cancel,
    CancelBatch,
    VisibleRows {
        start: usize,
        end: usize,
    },
    RetryContinuation,
    CountExact,
    SelectRow(usize),
    OpenRecord(usize),
    OpenRecordNewTab(usize),
    PinPreview,
    ActivateTab(u64),
    SetTabView {
        id: u64,
        view: TabViewState,
    },
    CloseTab(u64),
    OpenTabQuery,
    LoadLinks,
    OpenDetail,
    AdjacentResult(bool),
    DetailRowSelected(usize),
    ToggleDetailExpansion(usize),
    DetailSectionRelative(bool),
    ToggleRawSource,
    FollowRelation(usize),
    OpenRelationTab(usize),
    CopyRelation(usize),
    BackRelation,
    CopyRecord,
    SaveQuery(SearchDraft),
    RenameSaved {
        id: u64,
        title: String,
    },
    RemoveSaved(u64),
    RetryPreferences,
    RestoreHistory(usize),
    RestoreSaved(u64),
    ToggleColumn(String),
    ResizeColumn {
        column: String,
        width: u16,
    },
    ToggleWrapColumn(String),
    TogglePreview,
    Export {
        format: ExportFormat,
        path: String,
    },
    RunBatch {
        source: BatchSource,
        text: String,
        path: String,
        output_path: String,
    },
    WriteIndex,
    Index {
        mode: SnapshotMode,
        directory: String,
    },
    CheckForChanges,
    SaveIndex,
    SaveIndexAs(String),
    ConfirmSaveIndexAs(String),
    LoadIndex(String),
    RebuildIndex,
    CompactIndex,
    ResumeExtendedIndex,
    SetAutomaticChecking {
        enabled: bool,
        interval_seconds: u64,
    },
    SetAutomaticSaving(bool),
    CancelIndexTask(IndexTaskKind),
    RequestClose,
    RetryClose,
    CloseWithoutSaving,
    CancelClose,
    RefreshResults,
    RefreshRecord(u64),
    ChooseIndexLoadPath {
        generation: u64,
        load_after_selection: bool,
    },
    ChooseIndexSavePath {
        generation: u64,
        save_after_selection: bool,
    },
    SetTheme {
        dark: bool,
    },
    ShowHelp,
}

/// Directory paths in the current index publication, including collection scopes.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DirectoryCatalog {
    pub all: Vec<String>,
    pub by_collection: std::collections::BTreeMap<String, Vec<String>>,
}

/// Stable physical file identity exposed by staged discovery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogueFile {
    pub id: u32,
    pub relative_path: String,
    pub collections: Vec<u32>,
    pub size: u64,
    pub created_ns: Option<i128>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FileCatalogue {
    pub files: std::sync::Arc<Vec<CatalogueFile>>,
    pub by_collection: std::collections::BTreeMap<String, std::sync::Arc<Vec<usize>>>,
}

/// Backend-owned state that the UI may display after generation validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiUpdate {
    OpenDetection {
        generation: u64,
        result: Result<crate::controller::open_detection::DetectedSource, String>,
    },
    ChosenFolder {
        generation: u64,
        slot: usize,
        path: String,
    },
    ChosenSchema {
        generation: u64,
        path: String,
    },
    Opening(String),
    Opened {
        root: String,
        collections: Vec<String>,
        single_collection: bool,
        directories: DirectoryCatalog,
    },
    Directories(DirectoryCatalog),
    FileCatalogue(FileCatalogue),
    /// A validated replacement for the same source configuration preserves UI drafts/tabs.
    IndexReplaced(IndexUiStatus),
    IndexStatus(IndexUiStatus),
    ClosingIndex {
        error: Option<String>,
    },
    CloseCancelled,
    CloseReady,
    SaveIndexOverwriteRequired(String),
    ResultsOutdated(bool),
    ChosenIndexLoadPath {
        generation: u64,
        path: String,
    },
    ChosenIndexSavePath {
        generation: u64,
        path: String,
    },
    QueryStarted {
        status: String,
        retain_preview: bool,
    },
    Page(PageView),
    AppendPage(PageView),
    Projection(PageView),
    ContinuationStarted,
    ContinuationFailed(String),
    CountStarted,
    ExactCount(usize),
    CountFailed(String),
    RowSelection(Option<usize>),
    RevealRow(usize),
    Tabs {
        tabs: Vec<RecordTabSummary>,
        active: Option<u64>,
    },
    Detail(Option<DetailView>),
    RestoreTabView(TabViewState),
    History {
        entries: Vec<QueryEntry>,
        saved: Vec<QueryEntry>,
    },
    Columns {
        available: Vec<String>,
        hidden: Vec<String>,
    },
    TablePresentation {
        scope: Option<String>,
        widths: std::collections::BTreeMap<String, u16>,
        wrapped: Vec<String>,
    },
    Batch {
        rows: Vec<BatchRow>,
        running: bool,
        outcome: Option<String>,
    },
    Diagnostics(String),
    /// Complete clipboard text; consumed by the window without entering bounded display state.
    Clipboard(String),
    Status(String),
    PreferenceSaved,
    PreferenceFailed(String),
    Error {
        field: ErrorField,
        message: String,
    },
    ClearError(ErrorField),
    RestoreQuery(SearchDraft),
    FilterValidation {
        generation: u64,
        state: FilterValidationState,
    },
    FieldChoices(Vec<super::filter_builder::FieldChoice>),
    CompletionValues {
        origin: CompletionOrigin,
        revision: u64,
        field: String,
        prefix: String,
        values: Vec<(String, String)>,
        partial: bool,
        visited_keys: u64,
    },
    Theme {
        dark: bool,
    },
}

/// Truncate at a UTF-8 boundary and mark that content is incomplete.
fn truncate_utf8(text: &mut String, limit: usize) -> bool {
    if text.len() <= limit {
        return false;
    }
    if limit < '…'.len_utf8() {
        text.clear();
        return true;
    }
    let mut end = limit - '…'.len_utf8();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push('…');
    true
}

/// Consume a shared byte budget after enforcing one field's size limit.
fn fit_budget(text: &mut String, budget: &mut usize, field_limit: usize) -> bool {
    let clipped = truncate_utf8(text, field_limit.min(*budget));
    *budget = budget.saturating_sub(text.len());
    clipped
}

#[cfg(test)]
#[path = "model.test.rs"]
mod tests;
