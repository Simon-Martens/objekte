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
            diagnostic: false,
        }
    }
}


/// The two independently editable query inputs and execution options.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchDraft {
    pub full_text: String,
    pub structured: String,
    /// None searches all collections.
    pub collection: Option<String>,
    pub case_sensitive: bool,
    pub regex: bool,
    /// Explicit sort control; inline SORT BY remains part of `structured`.
    pub sort: String,
    /// Numeric input is kept as text so a validation error does not erase it.
    pub offset: String,
    /// Empty means unlimited; zero means count-only when explicitly entered.
    pub limit: String,
    pub count: bool,
}


impl Default for SearchDraft {
    fn default() -> Self {
        Self {
            full_text: String::new(),
            structured: String::new(),
            collection: None,
            case_sensitive: true,
            regex: false,
            sort: String::new(),
            offset: "0".into(),
            limit: String::new(),
            count: false,
        }
    }
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
}


/// The selected record's bounded inspection content.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DetailView {
    pub title: String,
    pub structured: Vec<DetailRow>,
    pub raw_source: String,
    pub source_label: String,
    pub diagnostic: Option<String>,
    pub relations: Vec<RelationRow>,
    pub selected_row: usize,
    pub clipped: bool,
}


impl DetailView {
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
    pub name: Option<String>,
    pub draft: SearchDraft,
}


/// One physical batch line and its current execution result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchRow {
    pub line: usize,
    pub query: String,
    pub status: String,
    pub error: Option<String>,
}


/// Which export representation is requested.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExportFormat {
    SelectedJson,
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


/// User intent emitted from GPUI controls to the integrator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiAction {
    ChooseFolder(OpenDraft),
    Open(OpenDraft),
    SearchEdited(SearchDraft),
    SearchNow(SearchDraft),
    Cancel,
    PageRelative(isize),
    SelectRow(usize),
    OpenDetail,
    CloseDetail,
    AdjacentResult(bool),
    DetailRowSelected(usize),
    ToggleDetailExpansion(usize),
    ToggleRawSource,
    FindInRecord { text: String, forward: bool },
    FollowRelation(usize),
    BackRelation,
    CopySelectedValue,
    CopyRecord,
    SaveQuery { name: String, draft: SearchDraft },
    RestoreHistory(usize),
    RestoreSaved(String),
    ToggleColumn(String),
    TogglePreview,
    Export { format: ExportFormat, path: String },
    RunBatch { text: String, path: String },
    WriteIndex,
    SetTheme { dark: bool },
    ShowHelp,
}


/// Backend-owned state that the UI may display after generation validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiUpdate {
    Opening(String),
    Opened {
        root: String,
        collections: Vec<String>,
        single_collection: bool,
    },
    QueryStarted(String),
    Page(PageView),
    Detail(Option<DetailView>),
    History {
        entries: Vec<QueryEntry>,
        saved: Vec<QueryEntry>,
    },
    Batch {
        rows: Vec<BatchRow>,
        running: bool,
        outcome: Option<String>,
    },
    Status(String),
    Error {
        field: ErrorField,
        message: String,
    },
    ClearError(ErrorField),
    ChosenPaths(String),
    RestoreQuery(SearchDraft),
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
