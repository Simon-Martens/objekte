//! Pinned, revisioned publication boundary for a changing index.
//!
//! This module establishes immutable read views and typed identity. File
//! mutation, reconciliation and persistence are added in later lifecycle
//! phases; a wrapped static repository does not imply those operations exist.

use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

use crate::build::{
    FilePreparationBinding, IndexedCollectionSegment, PreparedCollectionBuild, PreparedFileBuild,
    SegmentAdmission, SourceFileInput, prepare_file_build,
};
use crate::discovery::selects_file;
use crate::inventory::{FileEntry, FileStatus, FileVersions};
use crate::model::{CollectionId, FileId};
pub(crate) use crate::native_segment::PublishedSegment;
use crate::native_segment::SegmentId;
use crate::repository::{Repository, SearchResult};

pub use crate::inventory::FileVersion;
pub use crate::lifecycle_initial::{
    InitialBuildPhase, InitialBuildSnapshot, InitialBuildStatus, InitialBuildTask,
};

pub(crate) const MAX_SEGMENTS_PER_COLLECTION: usize = 64;
const MAX_SPARSE_SEGMENT_BYTES: usize = 32 * 1024 * 1024;
const SIZE_MAINTENANCE_REVISIONS: u64 = 16;
const MIN_RETIREMENT_SLOTS: usize = 256;
const MAX_RECONCILIATION_ISSUES: usize = 128;

/// Monotonic publication sequence within one index instance.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct IndexRevision(pub u64);

/// Exclusive phases of one progressive segment publication. The work before
/// installation is complete before a new query view becomes visible.
pub(crate) struct InitialPublicationPhases {
    pub(crate) preparation_ns: u64,
    pub(crate) install_ns: u64,
}

impl IndexRevision {
    pub const INITIAL: Self = Self(1);

    pub fn next(self) -> Result<Self, IndexError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(IndexError::RevisionOverflow)
    }
}

/// A physical file incarnation in one index instance.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FileHandle {
    pub instance_id: u64,
    pub file_id: FileId,
    pub version: FileVersion,
}

/// Errors at the lifecycle boundary. Query failures retain the existing
/// repository result type until the lifecycle query-completeness phase.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IndexError {
    RevisionConflict {
        expected: IndexRevision,
        actual: IndexRevision,
    },
    FileConflict {
        file: FileId,
        expected: FileVersion,
        actual: FileVersion,
    },
    ForeignInstance,
    UnknownFile,
    DestinationExists,
    DuplicateRecordId {
        collection: String,
        record_id: String,
    },
    IncompatibleConfiguration(String),
    InvalidPublication(String),
    StaleLocator,
    SourceUnavailable(String),
    ResourceLimit(String),
    IncompleteBuild,
    Cancelled,
    Io(String),
    Build(String),
    Extension(String),
    RevisionOverflow,
    LockPoisoned,
}

impl fmt::Display for IndexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RevisionConflict { expected, actual } => write!(
                formatter,
                "index revision conflict: expected {}, found {}",
                expected.0, actual.0
            ),
            Self::FileConflict {
                file,
                expected,
                actual,
            } => write!(
                formatter,
                "file {} version conflict: expected {}, found {}",
                file.0, expected.0, actual.0
            ),
            Self::ForeignInstance => {
                formatter.write_str("file or locator belongs to another index")
            }
            Self::UnknownFile => formatter.write_str("unknown indexed file"),
            Self::DestinationExists => formatter.write_str("destination is already indexed"),
            Self::DuplicateRecordId {
                collection,
                record_id,
            } => write!(
                formatter,
                "duplicate record ID {record_id} in collection {collection}"
            ),
            Self::IncompatibleConfiguration(message) => {
                write!(formatter, "incompatible configuration: {message}")
            }
            Self::InvalidPublication(message) => {
                write!(formatter, "invalid index publication: {message}")
            }
            Self::StaleLocator => {
                formatter.write_str("record locator belongs to an old index view")
            }
            Self::SourceUnavailable(message) => write!(formatter, "source unavailable: {message}"),
            Self::ResourceLimit(message) => write!(formatter, "resource limit: {message}"),
            Self::IncompleteBuild => {
                formatter.write_str("initial index construction is incomplete")
            }
            Self::Cancelled => formatter.write_str("index operation cancelled"),
            Self::Io(message) => write!(formatter, "index I/O: {message}"),
            Self::Build(message) => write!(formatter, "index build: {message}"),
            Self::Extension(message) => write!(formatter, "index extension: {message}"),
            Self::RevisionOverflow => formatter.write_str("index revision overflow"),
            Self::LockPoisoned => formatter.write_str("index publication lock poisoned"),
        }
    }
}

impl std::error::Error for IndexError {}

/// Initial status vocabulary; background task state is supplied by the
/// reconciliation phase, separately from publication revisions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReconciliationState {
    NotStarted,
    Pending,
    Running,
    Complete,
    Partial,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexStatus {
    pub instance_id: u64,
    pub revision: IndexRevision,
    pub reconciliation: ReconciliationState,
    pub indexed_files: usize,
    pub live_records: u64,
    /// Physical files whose every configured collection membership has been
    /// processed. Equal to `indexed_files` on an ordinary completed index.
    pub processed_files: usize,
    pub initial_construction_complete: bool,
    /// Construction parse attempts captured by the resident repository.
    pub build_parses: u64,
    pub build_source_reads: u64,
    pub build_source_bytes_read: u64,
    /// Active fallback records across the base and published segments.
    pub fallback_records: u64,
}

/// Discovery-only facts for one physical file. Record identity, validity,
/// byte ranges and record counts require construction and are omitted here.
#[derive(Clone, Copy, Debug)]
pub struct FileCatalogueRow<'a> {
    pub id: FileId,
    pub relative_path: &'a Path,
    pub collections: &'a [CollectionId],
    pub size: u64,
    pub modified_ns: i128,
    pub created_ns: Option<i128>,
}

/// Narrow native index-family display for benchmarks and diagnostics. It
/// covers gram/posting/sort payloads only; segment accounting is separate and
/// includes metadata, maps and relation families.
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct IndexFamilyEstimate {
    pub raw_bytes: usize,
    pub fallback_bytes: usize,
    pub custom_text_bytes: usize,
    pub postings_bytes: usize,
    pub sorting_bytes: usize,
}

impl IndexFamilyEstimate {
    pub fn total(self) -> usize {
        self.raw_bytes
            + self.fallback_bytes
            + self.custom_text_bytes
            + self.postings_bytes
            + self.sorting_bytes
    }
}

#[cfg(test)]
thread_local! {
    static INDEX_FAMILY_ESTIMATE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn index_family_estimate_calls() -> usize {
    INDEX_FAMILY_ESTIMATE_CALLS.with(std::cell::Cell::get)
}

#[cfg(test)]
pub(crate) fn reset_index_family_estimate_calls() {
    INDEX_FAMILY_ESTIMATE_CALLS.with(|calls| calls.set(0));
}

/// Cached narrow index-family total for one native contribution. It counts
/// gram/posting bitmap payloads and sort keys only, not segment metadata.
pub(crate) fn index_family_total(
    raw: &crate::bigram::BigramIndex,
    fallback: &crate::bigram::BigramIndex,
    custom: &std::collections::HashMap<(Vec<String>, String), crate::bigram::BigramIndex>,
    postings: &crate::posting::PostingCollection,
    sorting: &crate::sorting::SortCollection,
) -> usize {
    index_family_estimate(raw, fallback, custom, postings, sorting).total()
}

/// Result of an explicit query against one pinned, possibly incomplete view.
/// `result.count` is absent while construction is incomplete, even if the
/// request asked for a count over the eventual repository.
pub struct AvailableSearchResult {
    pub result: SearchResult,
    pub complete: bool,
    pub revision: IndexRevision,
    pub discovered_files: usize,
    pub processed_files: usize,
}

/// Outcome of explicit full reconstruction from the configured source files.
#[derive(Clone, Debug)]
pub struct RebuildReport {
    pub revision: IndexRevision,
    pub attempts: usize,
    pub indexed_files: usize,
    pub live_records: u64,
    pub duration: Duration,
}

/// The kind of a long-running index maintenance task.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaintenanceOperation {
    Compact,
    Rebuild,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaintenancePhase {
    Pending,
    Preparing,
    Publishing,
    Complete,
    Cancelled,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MaintenanceProgress {
    pub operation: MaintenanceOperation,
    pub phase: MaintenancePhase,
    pub attempt: usize,
    pub elapsed: Duration,
}

#[derive(Clone, Debug)]
pub enum MaintenanceReport {
    Compact(crate::lifecycle_compaction::CompactionReport),
    Rebuild(RebuildReport),
}

/// Result of removing one physical source entry from the published index.
/// The source file itself is never changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeleteStrategy {
    Tombstone,
    VerifiedPhysicalRemoval,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeleteResult {
    pub revision: IndexRevision,
    pub file: FileId,
    pub retired_records: u64,
    pub affected_collections: Vec<String>,
    pub strategy: DeleteStrategy,
}

/// One metadata scan and its applied index changes. Errors affecting only a
/// source remain visible here while other independent files can proceed.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReconciliationReport {
    pub examined: usize,
    pub planned_changes: usize,
    pub unchanged: usize,
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub skipped: usize,
    pub indexed_bytes: u64,
    pub duration: Duration,
    pub cancelled: bool,
    pub issues: Vec<String>,
    pub omitted_issues: usize,
}

impl ReconciliationReport {
    fn record_issue(&mut self, issue: String) {
        if self.issues.len() < MAX_RECONCILIATION_ISSUES {
            self.issues.push(issue);
        } else {
            self.omitted_issues = self.omitted_issues.saturating_add(1);
        }
    }
}

/// Small, copyable observation while a reconciliation pass is running.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReconciliationProgress {
    pub examined: usize,
    pub planned_changes: usize,
    pub unchanged: usize,
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub skipped: usize,
    pub indexed_bytes: u64,
    pub duration: Duration,
    pub cancelled: bool,
    pub issue_count: usize,
}

impl From<&ReconciliationReport> for ReconciliationProgress {
    fn from(report: &ReconciliationReport) -> Self {
        Self {
            examined: report.examined,
            planned_changes: report.planned_changes,
            unchanged: report.unchanged,
            added: report.added,
            updated: report.updated,
            removed: report.removed,
            skipped: report.skipped,
            indexed_bytes: report.indexed_bytes,
            duration: report.duration,
            cancelled: report.cancelled,
            issue_count: report.issues.len().saturating_add(report.omitted_issues),
        }
    }
}

/// Private work captured before the mutation writer validates and publishes
/// one file across all eligible collections.
pub(crate) struct PreparedAdd {
    pub(crate) expected_revision: IndexRevision,
    pub(crate) configuration_identity: [u8; 32],
    pub(crate) file: PreparedFileBuild,
}

/// A complete replacement captured against one file incarnation. The writer
/// checks both the view revision and file token before replacing its slots.
pub(crate) struct PreparedReplacement {
    pub(crate) expected_revision: IndexRevision,
    pub(crate) expected_handle: FileHandle,
    pub(crate) configuration_identity: [u8; 32],
    pub(crate) file: PreparedFileBuild,
}

/// Native retained-memory estimate for one published contribution. It counts
/// posting and gram bitmap payloads plus estimated sort keys. It is not an
/// allocator measurement or a strict peak bound; it orders maintenance.
pub(crate) fn index_family_estimate(
    raw: &crate::bigram::BigramIndex,
    fallback: &crate::bigram::BigramIndex,
    custom: &std::collections::HashMap<(Vec<String>, String), crate::bigram::BigramIndex>,
    postings: &crate::posting::PostingCollection,
    sorting: &crate::sorting::SortCollection,
) -> IndexFamilyEstimate {
    #[cfg(test)]
    INDEX_FAMILY_ESTIMATE_CALLS.with(|calls| calls.set(calls.get() + 1));
    IndexFamilyEstimate {
        raw_bytes: raw.estimated_heap_bytes(),
        fallback_bytes: fallback.estimated_heap_bytes(),
        custom_text_bytes: custom
            .values()
            .map(|index| index.estimated_heap_bytes())
            .sum(),
        postings_bytes: postings
            .fields
            .values()
            .map(|posting| {
                posting
                    .values
                    .values()
                    .map(|bitmap| bitmap.serialized_size() as usize)
                    .sum::<usize>()
                    + posting
                        .lengths
                        .values()
                        .map(|bitmap| bitmap.serialized_size() as usize)
                        .sum::<usize>()
                    + posting
                        .structural
                        .as_ref()
                        .map_or(0, crate::posting::StructuralFacts::estimated_bytes)
                    + if posting.empty_selections.is_empty() {
                        0
                    } else {
                        posting.empty_selections.serialized_size()
                    }
            })
            .sum(),
        sorting_bytes: sorting
            .columns
            .values()
            .map(|column| {
                column
                    .keys
                    .iter()
                    .map(|key| match key {
                        crate::sorting::SortKey::String(value) => value.len(),
                        _ => 24,
                    })
                    .sum::<usize>()
            })
            .sum(),
    }
}

fn index_prepared_file(
    repository: &Repository,
    source: &SourceFileInput,
    collections: Vec<PreparedCollectionBuild>,
    first_segment_id: u64,
    policy: crate::native_segment::SegmentPolicy,
) -> Result<(FileEntry, Vec<IndexedCollectionSegment>), IndexError> {
    let options = repository.options();
    let mut indexed = Vec::with_capacity(collections.len());
    let mut next_segment_id = first_segment_id;
    for collection in collections {
        let state = repository
            .collections()
            .get(collection.collection.0 as usize)
            .ok_or_else(|| IndexError::InvalidPublication("collection missing".into()))?;
        if collection.collection != state.generation.collection
            || collection.source_id != state.source.id()
            || collection.source_version != state.source.version()
        {
            return Err(IndexError::Build(
                "prepared collection binding changed".into(),
            ));
        }
        let mut admission = SegmentAdmission::with_limits_and_id(
            collection.collection,
            &state.source,
            policy,
            SegmentId::new(next_segment_id),
        );
        next_segment_id = next_segment_id
            .checked_add(1)
            .ok_or_else(|| IndexError::ResourceLimit("segment identity overflow".into()))?;
        let mut finish = |admission: SegmentAdmission| -> Result<(), IndexError> {
            indexed.push(
                admission
                    .finish()
                    .into_indexed(
                        &state.definition,
                        options.bigram_alphabet,
                        options.inference,
                        repository.collections().len(),
                        &repository.workers,
                    )
                    .map_err(IndexError::Build)?,
            );
            Ok(())
        };
        for batch in collection.contributions {
            if batch.record_count() != 0
                && admission.file_count() != 0
                && !admission.can_admit_batch(&batch)
            {
                finish(std::mem::replace(
                    &mut admission,
                    SegmentAdmission::with_limits_and_id(
                        collection.collection,
                        &state.source,
                        policy,
                        SegmentId::new(next_segment_id),
                    ),
                ))?;
                next_segment_id = next_segment_id
                    .checked_add(1)
                    .ok_or_else(|| IndexError::ResourceLimit("segment identity overflow".into()))?;
            }
            admission.admit_batch(batch).map_err(IndexError::Build)?;
            if admission.at_capacity() {
                finish(std::mem::replace(
                    &mut admission,
                    SegmentAdmission::with_limits_and_id(
                        collection.collection,
                        &state.source,
                        policy,
                        SegmentId::new(next_segment_id),
                    ),
                ))?;
                next_segment_id = next_segment_id
                    .checked_add(1)
                    .ok_or_else(|| IndexError::ResourceLimit("segment identity overflow".into()))?;
            }
        }
        if admission.file_count() != 0 {
            finish(admission)?;
        }
    }
    let memberships = indexed.iter().map(|segment| segment.collection).collect();
    let mut entry = FileEntry {
        id: source.id,
        relative_path: source.relative_path.clone().into(),
        revision: source.revision,
        created_ns: source.created_ns,
        collections: memberships,
        status: FileStatus::Valid,
        diagnostics: Vec::new(),
        omitted_diagnostics: 0,
    };
    for segment in &indexed {
        let file = segment
            .file(source.id)
            .ok_or_else(|| IndexError::InvalidPublication("segment file entry missing".into()))?;
        entry.status = entry.status.max(file.entry.status);
        entry.omitted_diagnostics = entry
            .omitted_diagnostics
            .saturating_add(file.entry.omitted_diagnostics);
        for diagnostic in &file.entry.diagnostics {
            entry
                .record_diagnostic(diagnostic.clone())
                .map_err(IndexError::InvalidPublication)?;
        }
    }
    Ok((entry, indexed))
}

fn prepare_file_at(
    view: &IndexView,
    path: &Path,
    id: FileId,
) -> Result<PreparedFileBuild, IndexError> {
    let repository = view.repository();
    let root = &repository.files().root;
    let relative = if path.is_absolute() {
        path.strip_prefix(root)
            .map_err(|_| IndexError::SourceUnavailable("file is outside root".into()))?
    } else {
        path
    };
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err(IndexError::SourceUnavailable(
            "file path must be repository-relative and contained".into(),
        ));
    }
    if view
        .0
        .catalog
        .id_for(relative)
        .is_some_and(|other| other != id)
    {
        return Err(IndexError::DestinationExists);
    }
    let mut bindings = Vec::new();
    for collection in repository.collections() {
        if selects_file(
            root,
            &collection.definition.source_path,
            &collection.definition.discovery,
            &collection.source,
            relative,
        )
        .map_err(IndexError::Build)?
        {
            bindings.push(FilePreparationBinding {
                collection: collection.generation.collection,
                definition: &collection.definition,
                source: &collection.source,
            });
        }
    }
    if bindings.is_empty() {
        return Err(IndexError::IncompatibleConfiguration(
            "no configured collection selects this file".into(),
        ));
    }
    let metadata = std::fs::metadata(root.join(relative))
        .map_err(|error| IndexError::Io(error.to_string()))?;
    let revision =
        crate::inventory::SourceRevision::from_metadata(&metadata).map_err(IndexError::Io)?;
    let created_ns = metadata
        .created()
        .ok()
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos() as i128);
    let source = SourceFileInput {
        id,
        relative_path: relative.to_owned().into(),
        revision,
        created_ns,
    };
    let options = repository.options();
    prepare_file_build(
        root,
        source,
        &bindings,
        options.bigram_alphabet,
        options.inference,
        1,
    )
    .map_err(IndexError::Build)
}

struct IndexInner {
    instance_id: u64,
    timings: Mutex<IndexTimingsMs>,
    /// Set under the publication write lock before closing waits for workers.
    /// Every data publication checks it while holding that same lock.
    quiescing: AtomicBool,
    /// One aggregate budget for all pinned publications of this index.
    verified_cache: Mutex<crate::verified::VerifiedCache>,
    published: RwLock<Arc<PublishedIndex>>,
    writer: Mutex<()>,
    maintenance: Mutex<MaintenanceState>,
    maintenance_operation: Mutex<Option<Arc<MaintenanceJob>>>,
    periodic_transition: Mutex<()>,
    periodic: Mutex<Option<PeriodicReconciliation>>,
}

/// Coarse startup milestones available in ordinary builds. Values are elapsed
/// milliseconds from the beginning of the corresponding build/load operation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct IndexTimingsMs {
    pub metadata: Option<u64>,
    pub search_available: Option<u64>,
    pub full_available: Option<u64>,
    pub inference_full_available: Option<u64>,
}

struct PeriodicReconciliation {
    signal: Sender<PeriodicSignal>,
    worker: JoinHandle<()>,
}

enum PeriodicSignal {
    #[cfg(test)]
    Tick,
    Stop,
}

struct MaintenanceState {
    reconciliation: ReconciliationState,
    current: Option<Arc<ReconciliationJob>>,
}

struct ReconciliationJob {
    cancelled: AtomicBool,
    progress: Mutex<ReconciliationProgress>,
    result: Mutex<Option<ReconciliationReport>>,
    complete: Condvar,
    worker: Mutex<Option<JoinHandle<()>>>,
}

struct MaintenanceJob {
    operation: MaintenanceOperation,
    started: Instant,
    cancelled: AtomicBool,
    progress: Mutex<MaintenanceProgress>,
    result: Mutex<Option<Result<MaintenanceReport, IndexError>>>,
    complete: Condvar,
    worker: Mutex<Option<JoinHandle<()>>>,
}

/// One observable compaction or rebuild. Cancellation prevents publication
/// and leaves the current view available.
#[derive(Clone)]
pub struct MaintenanceTask(Arc<MaintenanceJob>);

impl MaintenanceTask {
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
    }

    pub fn progress(&self) -> Result<MaintenanceProgress, IndexError> {
        let mut progress = *self
            .0
            .progress
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?;
        if !matches!(
            progress.phase,
            MaintenancePhase::Complete | MaintenancePhase::Cancelled | MaintenancePhase::Failed
        ) {
            progress.elapsed = self.0.started.elapsed();
        }
        Ok(progress)
    }

    pub fn is_complete(&self) -> Result<bool, IndexError> {
        Ok(self
            .0
            .result
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?
            .is_some())
    }

    pub fn wait(&self) -> Result<MaintenanceReport, IndexError> {
        let mut result = self.0.result.lock().map_err(|_| IndexError::LockPoisoned)?;
        while result.is_none() {
            result = self
                .0
                .complete
                .wait(result)
                .map_err(|_| IndexError::LockPoisoned)?;
        }
        let outcome = result.as_ref().cloned().ok_or(IndexError::LockPoisoned)?;
        drop(result);
        if let Some(worker) = self
            .0
            .worker
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?
            .take()
        {
            worker.join().map_err(|_| {
                IndexError::InvalidPublication("maintenance worker panicked".into())
            })?;
        }
        outcome
    }
}

impl MaintenanceJob {
    fn set_progress(&self, phase: MaintenancePhase, attempt: usize, elapsed: Duration) {
        if let Ok(mut progress) = self.progress.lock() {
            progress.phase = phase;
            progress.attempt = attempt;
            progress.elapsed = elapsed;
        }
    }
}

/// Observable, cancellable handle for one metadata reconciliation pass.
#[derive(Clone)]
pub struct ReconciliationTask(Arc<ReconciliationJob>);

impl ReconciliationTask {
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
    }

    pub fn is_complete(&self) -> Result<bool, IndexError> {
        Ok(self
            .0
            .result
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?
            .is_some())
    }

    pub fn progress(&self) -> Result<ReconciliationProgress, IndexError> {
        Ok(*self
            .0
            .progress
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?)
    }

    pub fn wait(&self) -> Result<ReconciliationReport, IndexError> {
        let mut guard = self.0.result.lock().map_err(|_| IndexError::LockPoisoned)?;
        while guard.is_none() {
            guard = self
                .0
                .complete
                .wait(guard)
                .map_err(|_| IndexError::LockPoisoned)?;
        }
        let result = guard.as_ref().cloned().ok_or(IndexError::LockPoisoned)?;
        drop(guard);
        if let Some(worker) = self
            .0
            .worker
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?
            .take()
        {
            worker.join().map_err(|_| {
                IndexError::InvalidPublication("reconciliation worker panicked".into())
            })?;
        }
        Ok(result)
    }
}

/// Shared lifecycle owner. Cloning this handle does not copy indexed data.
#[derive(Clone)]
pub struct Index(Arc<IndexInner>);

impl Index {
    /// Adopt a completed static repository as the first published view.
    /// This does not trigger discovery, reconciliation or cache persistence.
    pub fn from_repository(repository: Repository) -> Result<Self, IndexError> {
        let build_ns = repository.startup().build_ns;
        let metadata_ns = repository.startup().build_discovery_ns;
        let inference_enabled =
            repository.options().inference.mode != crate::inference::InferenceMode::Disabled;
        let instance_id = crate::repository::next_repository_instance();
        let published =
            PublishedIndex::from_repository_owned(instance_id, IndexRevision::INITIAL, repository)?;
        let index = Self::from_published(published);
        if build_ns > 0 {
            let full_ms = build_ns / 1_000_000;
            index.update_timings_ms(|timings| {
                timings.metadata = Some(metadata_ns / 1_000_000);
                timings.search_available = Some(full_ms);
                timings.full_available = Some(full_ms);
                if inference_enabled {
                    timings.inference_full_available = Some(full_ms);
                }
            })?;
        }
        Ok(index)
    }

    /// Consume a completed blocking build and retain its adopted native
    /// segments as the Repository's primary payload.
    #[cfg(test)]
    pub(crate) fn into_repository(self) -> Result<Repository, IndexError> {
        let inner = Arc::try_unwrap(self.0)
            .map_err(|_| IndexError::InvalidPublication("index has other owners".into()))?;
        let published = inner.published.into_inner().map_err(|_| {
            IndexError::InvalidPublication("index publication lock poisoned".into())
        })?;
        let published = Arc::try_unwrap(published).map_err(|_| {
            IndexError::InvalidPublication("published index has other owners".into())
        })?;
        let mut repository = Arc::try_unwrap(published.repository).map_err(|_| {
            IndexError::InvalidPublication("repository snapshot has other owners".into())
        })?;
        repository.active = published.active;
        repository.segments = published.segments;
        repository.catalog = published.catalog;
        repository.revision = published.revision;
        Ok(repository)
    }

    pub(crate) fn from_published(published: PublishedIndex) -> Self {
        let instance_id = published.instance_id;
        let verified_cache_bytes = published.repository.options().verified_cache_bytes;
        Self(Arc::new(IndexInner {
            instance_id,
            timings: Mutex::new(IndexTimingsMs::default()),
            quiescing: AtomicBool::new(false),
            verified_cache: Mutex::new(crate::verified::VerifiedCache::new(verified_cache_bytes)),
            published: RwLock::new(Arc::new(published)),
            writer: Mutex::new(()),
            maintenance: Mutex::new(MaintenanceState {
                reconciliation: ReconciliationState::NotStarted,
                current: None,
            }),
            maintenance_operation: Mutex::new(None),
            periodic_transition: Mutex::new(()),
            periodic: Mutex::new(None),
        }))
    }

    pub fn instance_id(&self) -> u64 {
        self.0.instance_id
    }

    pub fn timings_ms(&self) -> Result<IndexTimingsMs, IndexError> {
        Ok(*self
            .0
            .timings
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?)
    }

    pub(crate) fn update_timings_ms(
        &self,
        update: impl FnOnce(&mut IndexTimingsMs),
    ) -> Result<(), IndexError> {
        let mut timings = self
            .0
            .timings
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?;
        update(&mut timings);
        Ok(())
    }

    /// Set the aggregate in-memory verified-result cache budget for this
    /// lifecycle instance. It is shared across all pinned revisions.
    pub fn configure_verified_cache(&self, bytes: usize) -> Result<(), IndexError> {
        let displaced = self
            .0
            .verified_cache
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?
            .set_budget(bytes);
        drop(displaced);
        Ok(())
    }

    pub fn verified_cache_budget(&self) -> Result<usize, IndexError> {
        Ok(self
            .0
            .verified_cache
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?
            .budget())
    }

    pub fn status(&self) -> Result<IndexStatus, IndexError> {
        let mut status = self.view()?.0.status();
        status.reconciliation = self
            .0
            .maintenance
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?
            .reconciliation;
        Ok(status)
    }

    /// Run one source-free compaction on a worker while pinned queries use
    /// their current view. Concurrent requests for the same operation share
    /// its handle; a different operation waits for the active one to finish.
    pub fn start_compaction(&self) -> Result<MaintenanceTask, IndexError> {
        self.start_maintenance(MaintenanceOperation::Compact)
    }

    /// Reconstruct the index from the current explicit configuration on a
    /// worker. Cancellation leaves the published view unchanged.
    pub fn start_rebuild(&self) -> Result<MaintenanceTask, IndexError> {
        self.start_maintenance(MaintenanceOperation::Rebuild)
    }

    fn start_maintenance(
        &self,
        operation: MaintenanceOperation,
    ) -> Result<MaintenanceTask, IndexError> {
        let mut current = self
            .0
            .maintenance_operation
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?;
        if self.0.quiescing.load(Ordering::Acquire) {
            return Err(IndexError::Cancelled);
        }
        if let Some(job) = current.as_ref() {
            if job
                .result
                .lock()
                .map_err(|_| IndexError::LockPoisoned)?
                .is_none()
            {
                if job.operation == operation {
                    return Ok(MaintenanceTask(Arc::clone(job)));
                }
                return Err(IndexError::ResourceLimit(
                    "another index maintenance task is running".into(),
                ));
            }
        }
        let job = Arc::new(MaintenanceJob {
            operation,
            started: Instant::now(),
            cancelled: AtomicBool::new(false),
            progress: Mutex::new(MaintenanceProgress {
                operation,
                phase: MaintenancePhase::Pending,
                attempt: 0,
                elapsed: Duration::ZERO,
            }),
            result: Mutex::new(None),
            complete: Condvar::new(),
            worker: Mutex::new(None),
        });
        let owner = self.clone();
        let worker_job = Arc::clone(&job);
        let worker = std::thread::Builder::new()
            .name(
                match operation {
                    MaintenanceOperation::Compact => "objekte-compact",
                    MaintenanceOperation::Rebuild => "objekte-rebuild",
                }
                .into(),
            )
            .spawn(move || {
                let outcome =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match operation {
                        MaintenanceOperation::Compact => owner
                            .compact_with_control(
                                || worker_job.cancelled.load(Ordering::Acquire),
                                |phase, attempt| {
                                    worker_job.set_progress(
                                        phase,
                                        attempt,
                                        worker_job.started.elapsed(),
                                    )
                                },
                            )
                            .map(MaintenanceReport::Compact),
                        MaintenanceOperation::Rebuild => owner
                            .rebuild_with_control(
                                || worker_job.cancelled.load(Ordering::Acquire),
                                |phase, attempt| {
                                    worker_job.set_progress(
                                        phase,
                                        attempt,
                                        worker_job.started.elapsed(),
                                    )
                                },
                                |_| {},
                            )
                            .map(MaintenanceReport::Rebuild),
                    }))
                    .unwrap_or_else(|_| {
                        Err(IndexError::InvalidPublication(
                            "maintenance worker panicked".into(),
                        ))
                    });
                let phase = match &outcome {
                    Ok(_) => MaintenancePhase::Complete,
                    Err(IndexError::Cancelled) => MaintenancePhase::Cancelled,
                    Err(_) => MaintenancePhase::Failed,
                };
                let attempt = worker_job
                    .progress
                    .lock()
                    .ok()
                    .map(|progress| progress.attempt)
                    .unwrap_or(0);
                worker_job.set_progress(phase, attempt, worker_job.started.elapsed());
                if let Ok(mut result) = worker_job.result.lock() {
                    *result = Some(outcome);
                    worker_job.complete.notify_all();
                }
            })
            .map_err(|error| IndexError::Io(error.to_string()))?;
        *job.worker.lock().map_err(|_| IndexError::LockPoisoned)? = Some(worker);
        *current = Some(Arc::clone(&job));
        Ok(MaintenanceTask(job))
    }

    /// Rebuild from the current explicit configuration. Queries keep using
    /// their pinned prior view while source extraction runs. A concurrent
    /// commit makes the candidate stale and causes a fresh bounded retry.
    pub fn rebuild(&self) -> Result<RebuildReport, IndexError> {
        self.rebuild_with_before_publish(|_| {})
    }

    fn rebuild_with_before_publish(
        &self,
        mut before_publish: impl FnMut(usize),
    ) -> Result<RebuildReport, IndexError> {
        self.rebuild_with_control(|| false, |_, _| {}, |attempt| before_publish(attempt))
    }

    fn rebuild_with_control(
        &self,
        cancelled: impl Fn() -> bool,
        mut progress: impl FnMut(MaintenancePhase, usize),
        mut before_publish: impl FnMut(usize),
    ) -> Result<RebuildReport, IndexError> {
        let started = Instant::now();
        let mut conflict = None;
        for attempt in 1..=3 {
            if cancelled() {
                return Err(IndexError::Cancelled);
            }
            progress(MaintenancePhase::Preparing, attempt);
            let view = self.view()?;
            let repository = view.repository();
            let definitions = repository
                .collections()
                .iter()
                .map(|collection| collection.definition.clone())
                .collect::<Vec<_>>();
            let task = Self::start_build(
                repository.files().root.clone(),
                definitions,
                repository.options(),
                repository.sources.clone(),
            )?;
            let fresh = loop {
                if cancelled() {
                    task.cancel();
                    let _ = task.wait_complete();
                    return Err(IndexError::Cancelled);
                }
                match task.status()?.phase {
                    InitialBuildPhase::Complete => break task.wait_complete()?,
                    InitialBuildPhase::Cancelled => return Err(IndexError::Cancelled),
                    InitialBuildPhase::Failed => {
                        return match task.wait_complete() {
                            Err(error) => Err(error),
                            Ok(_) => unreachable!("failed build task cannot complete successfully"),
                        };
                    }
                    InitialBuildPhase::Discovering | InitialBuildPhase::Building => {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                }
            };
            let next_revision = view.revision().next()?;
            let mut next = (*fresh.view()?.0).clone();
            next.instance_id = self.instance_id();
            next.revision = next_revision;
            next.file_versions =
                FileVersions::filled(next.catalog.allocated_files(), FileVersion(next_revision.0));
            if cancelled() {
                return Err(IndexError::Cancelled);
            }
            progress(MaintenancePhase::Publishing, attempt);
            before_publish(attempt);
            if cancelled() {
                return Err(IndexError::Cancelled);
            }
            match self.publish_prepared(view.revision(), next) {
                Ok(revision) => {
                    let status = self.status()?;
                    return Ok(RebuildReport {
                        revision,
                        attempts: attempt,
                        indexed_files: status.indexed_files,
                        live_records: status.live_records,
                        duration: started.elapsed(),
                    });
                }
                Err(error @ IndexError::RevisionConflict { .. }) => conflict = Some(error),
                Err(error) => return Err(error),
            }
        }
        Err(conflict.expect("rebuild retry always captures a revision conflict"))
    }

    /// Coalesce concurrent scan requests into one worker. The scan runs
    /// without holding a query or publication lock; waiters receive its
    /// report and can retry a later pass after partial failure.
    pub fn start_reconcile(&self) -> Result<ReconciliationTask, IndexError> {
        let mut maintenance = self
            .0
            .maintenance
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?;
        if self.0.quiescing.load(Ordering::Acquire) {
            return Err(IndexError::Cancelled);
        }
        if let Some(job) = &maintenance.current {
            if job
                .result
                .lock()
                .map_err(|_| IndexError::LockPoisoned)?
                .is_none()
            {
                return Ok(ReconciliationTask(Arc::clone(job)));
            }
        }
        let job = Arc::new(ReconciliationJob {
            cancelled: AtomicBool::new(false),
            progress: Mutex::new(ReconciliationProgress::default()),
            result: Mutex::new(None),
            complete: Condvar::new(),
            worker: Mutex::new(None),
        });
        let owner = self.clone();
        let worker_job = Arc::clone(&job);
        let worker = std::thread::Builder::new()
            .name("objekte-reconcile".into())
            .spawn(move || {
                if let Ok(mut maintenance) = owner.0.maintenance.lock() {
                    maintenance.reconciliation = ReconciliationState::Running;
                }
                let report = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    owner.reconcile_pass_with_progress(
                        || worker_job.cancelled.load(Ordering::Acquire),
                        |progress| {
                            if let Ok(mut observed) = worker_job.progress.lock() {
                                *observed = ReconciliationProgress::from(progress);
                            }
                        },
                    )
                }))
                .unwrap_or_else(|_| ReconciliationReport {
                    issues: vec!["reconciliation worker panicked".into()],
                    ..ReconciliationReport::default()
                });
                if let Ok(mut observed) = worker_job.progress.lock() {
                    *observed = ReconciliationProgress::from(&report);
                }
                let state = if report.cancelled {
                    ReconciliationState::Cancelled
                } else if report.issues.is_empty() && report.omitted_issues == 0 {
                    ReconciliationState::Complete
                } else {
                    ReconciliationState::Partial
                };
                if let Ok(mut maintenance) = owner.0.maintenance.lock() {
                    maintenance.reconciliation = state;
                }
                if let Ok(mut result) = worker_job.result.lock() {
                    *result = Some(report);
                    worker_job.complete.notify_all();
                }
            })
            .map_err(|error| IndexError::Io(error.to_string()))?;
        *job.worker.lock().map_err(|_| IndexError::LockPoisoned)? = Some(worker);
        maintenance.reconciliation = ReconciliationState::Pending;
        maintenance.current = Some(Arc::clone(&job));
        Ok(ReconciliationTask(job))
    }

    /// Observe the most recently started reconciliation without scheduling a
    /// new pass. The returned handle remains valid after the pass completes;
    /// callers can inspect progress and retrieve its bounded outcome.
    pub fn current_reconciliation_task(&self) -> Result<Option<ReconciliationTask>, IndexError> {
        Ok(self
            .0
            .maintenance
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?
            .current
            .as_ref()
            .map(|job| ReconciliationTask(Arc::clone(job))))
    }

    /// Schedule one coalesced metadata reconciliation at each interval.
    /// Calling this again replaces the previous schedule. Stopping the
    /// schedule does not cancel a pass that has already started.
    pub fn start_periodic_reconciliation(&self, interval: Duration) -> Result<(), IndexError> {
        if interval.is_zero() {
            return Err(IndexError::ResourceLimit(
                "reconciliation interval must be nonzero".into(),
            ));
        }
        let _transition = self
            .0
            .periodic_transition
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?;
        if self.0.quiescing.load(Ordering::Acquire) {
            return Err(IndexError::Cancelled);
        }
        self.stop_periodic_reconciliation_unlocked()?;
        let (signal, receiver) = mpsc::channel();
        let owner = Arc::downgrade(&self.0);
        let worker = std::thread::Builder::new()
            .name("objekte-reconcile-periodic".into())
            .spawn(move || {
                loop {
                    match receiver.recv_timeout(interval) {
                        Ok(PeriodicSignal::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                        #[cfg(test)]
                        Ok(PeriodicSignal::Tick) => {}
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                    let Some(inner) = owner.upgrade() else { break };
                    let index = Index(inner);
                    let task = index.start_reconcile();
                    drop(index);
                    match task {
                        Ok(task) => drop(task),
                        Err(_) => break,
                    }
                }
            })
            .map_err(|error| IndexError::Io(error.to_string()))?;
        let mut periodic = self
            .0
            .periodic
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?;
        if self.0.quiescing.load(Ordering::Acquire) {
            let _ = signal.send(PeriodicSignal::Stop);
            drop(periodic);
            worker.join().map_err(|_| {
                IndexError::InvalidPublication("periodic reconciliation worker panicked".into())
            })?;
            return Err(IndexError::Cancelled);
        }
        *periodic = Some(PeriodicReconciliation { signal, worker });
        Ok(())
    }

    pub fn stop_periodic_reconciliation(&self) -> Result<(), IndexError> {
        let _transition = self
            .0
            .periodic_transition
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?;
        self.stop_periodic_reconciliation_unlocked()
    }

    fn stop_periodic_reconciliation_unlocked(&self) -> Result<(), IndexError> {
        let schedule = self
            .0
            .periodic
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?
            .take();
        if let Some(schedule) = schedule {
            let _ = schedule.signal.send(PeriodicSignal::Stop);
            schedule.worker.join().map_err(|_| {
                IndexError::InvalidPublication("periodic reconciliation worker panicked".into())
            })?;
        }
        Ok(())
    }

    /// Whether a live periodic scheduler currently belongs to this index.
    pub fn periodic_reconciliation_enabled(&self) -> Result<bool, IndexError> {
        Ok(self
            .0
            .periodic
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?
            .as_ref()
            .is_some_and(|schedule| !schedule.worker.is_finished()))
    }

    pub fn wait_for_reconciliation(&self) -> Result<Option<ReconciliationReport>, IndexError> {
        let current = self
            .0
            .maintenance
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?
            .current
            .clone();
        current
            .map(|job| ReconciliationTask(job).wait())
            .transpose()
    }

    /// Prevent new data publications, stop recurring work, and wait for every
    /// currently owned mutation worker. The resulting view is stable for a
    /// caller that is closing the index.
    pub fn quiesce(&self) -> Result<Option<ReconciliationReport>, IndexError> {
        {
            let _published = self
                .0
                .published
                .write()
                .map_err(|_| IndexError::LockPoisoned)?;
            self.0.quiescing.store(true, Ordering::Release);
        }
        self.stop_periodic_reconciliation()?;
        let operation = self
            .0
            .maintenance_operation
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?
            .take();
        let operation_error = if let Some(job) = operation {
            let task = MaintenanceTask(job);
            task.cancel();
            match task.wait() {
                Ok(_) | Err(IndexError::Cancelled) => None,
                Err(error) => Some(error),
            }
        } else {
            None
        };
        let current = self
            .0
            .maintenance
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?
            .current
            .clone();
        let reconciliation = current
            .map(|job| {
                let task = ReconciliationTask(job);
                task.cancel();
                task.wait()
            })
            .transpose()?;
        if let Some(error) = operation_error {
            return Err(error);
        }
        Ok(reconciliation)
    }

    /// Reopen publication after a cancelled close. Callers restore only the
    /// explicit recurring policies they held before quiescing.
    pub fn resume_after_quiesce(&self) -> Result<(), IndexError> {
        let _published = self
            .0
            .published
            .write()
            .map_err(|_| IndexError::LockPoisoned)?;
        self.0.quiescing.store(false, Ordering::Release);
        Ok(())
    }

    /// Stop the lifecycle service without writing a cache.
    pub fn shutdown(&self) -> Result<Option<ReconciliationReport>, IndexError> {
        self.quiesce()
    }

    /// Prepare a selected source without holding the publication or writer
    /// lock. The eventual commit revalidates the captured root and file ID.
    pub(crate) fn prepare_add_file(&self, path: &Path) -> Result<PreparedAdd, IndexError> {
        let view = self.view()?;
        let id = FileId(
            u32::try_from(view.0.catalog.allocated_files())
                .map_err(|_| IndexError::ResourceLimit("too many files".into()))?,
        );
        let file = prepare_file_at(&view, path, id)?;
        Ok(PreparedAdd {
            expected_revision: view.revision(),
            configuration_identity: view.configuration_identity(),
            file,
        })
    }

    /// Prepare an update or move after the application has changed the
    /// source file. The existing file ID survives; only its version advances.
    pub(crate) fn prepare_replace_file(
        &self,
        handle: FileHandle,
        path: &Path,
    ) -> Result<PreparedReplacement, IndexError> {
        if handle.instance_id != self.instance_id() {
            return Err(IndexError::ForeignInstance);
        }
        let view = self.view()?;
        let actual = view
            .0
            .file_versions
            .get(handle.file_id)
            .ok_or(IndexError::UnknownFile)?;
        if actual != handle.version {
            return Err(IndexError::FileConflict {
                file: handle.file_id,
                expected: handle.version,
                actual,
            });
        }
        if !view.0.catalog.is_active(handle.file_id) {
            return Err(IndexError::UnknownFile);
        }
        let file = prepare_file_at(&view, path, handle.file_id)?;
        Ok(PreparedReplacement {
            expected_revision: view.revision(),
            expected_handle: handle,
            configuration_identity: view.configuration_identity(),
            file,
        })
    }

    /// Build all affected collection segments and metadata privately. This
    /// returns an unpublished root; its captured revision must still be
    /// checked by the mutation writer before the eventual root swap.
    pub(crate) fn stage_prepared_add(
        &self,
        prepared: PreparedAdd,
    ) -> Result<PublishedIndex, IndexError> {
        let view = self.view()?;
        if view.revision() != prepared.expected_revision {
            return Err(IndexError::RevisionConflict {
                expected: prepared.expected_revision,
                actual: view.revision(),
            });
        }
        if view.configuration_identity() != prepared.configuration_identity {
            return Err(IndexError::IncompatibleConfiguration(
                "configuration changed during file preparation".into(),
            ));
        }
        if view.0.catalog.allocated_files() != prepared.file.source.id.0 as usize {
            return Err(IndexError::InvalidPublication(
                "prepared file ID is no longer next".into(),
            ));
        }
        if view
            .0
            .catalog
            .id_for(&prepared.file.source.relative_path)
            .is_some()
        {
            return Err(IndexError::DestinationExists);
        }
        let source = prepared.file.source;
        let fingerprint = prepared.file.fingerprint;
        let (entry, indexed) = index_prepared_file(
            view.repository(),
            &source,
            prepared.file.collections,
            view.0.next_segment_ordinal,
            view.repository().options().segment_policy,
        )?;
        let mut next = (*view.0).clone();
        next.catalog = next
            .catalog
            .appended(entry, fingerprint, Some(prepared.configuration_identity))
            .map_err(IndexError::InvalidPublication)?;
        next.file_versions = next
            .file_versions
            .appended()
            .map_err(IndexError::ResourceLimit)?;
        for segment in indexed {
            next = next.with_segment(segment)?;
        }
        next.revision = view.revision().next()?;
        Ok(next)
    }

    /// Stage an update or move with the old incarnation retired in every
    /// collection. The returned root is not visible until the writer checks
    /// the captured revision and publishes it.
    pub(crate) fn stage_prepared_replace(
        &self,
        prepared: PreparedReplacement,
    ) -> Result<PublishedIndex, IndexError> {
        let view = self.view()?;
        if prepared.expected_handle.instance_id != self.instance_id() {
            return Err(IndexError::ForeignInstance);
        }
        if view.revision() != prepared.expected_revision {
            return Err(IndexError::RevisionConflict {
                expected: prepared.expected_revision,
                actual: view.revision(),
            });
        }
        if view.configuration_identity() != prepared.configuration_identity {
            return Err(IndexError::IncompatibleConfiguration(
                "configuration changed during file preparation".into(),
            ));
        }
        let file_id = prepared.expected_handle.file_id;
        if prepared.file.source.id != file_id {
            return Err(IndexError::InvalidPublication(
                "prepared replacement file ID changed".into(),
            ));
        }
        let actual = view
            .0
            .file_versions
            .get(file_id)
            .ok_or(IndexError::UnknownFile)?;
        if actual != prepared.expected_handle.version {
            return Err(IndexError::FileConflict {
                file: file_id,
                expected: prepared.expected_handle.version,
                actual,
            });
        }
        let previous = view
            .0
            .catalog
            .entry(file_id)
            .ok_or(IndexError::UnknownFile)?;
        if view
            .0
            .catalog
            .id_for(&prepared.file.source.relative_path)
            .is_some_and(|other| other != file_id)
        {
            return Err(IndexError::DestinationExists);
        }
        let source = prepared.file.source;
        let fingerprint = prepared.file.fingerprint;
        let (entry, indexed) = index_prepared_file(
            view.repository(),
            &source,
            prepared.file.collections,
            view.0.next_segment_ordinal,
            view.repository().options().segment_policy,
        )?;
        let mut next = (*view.0).clone();
        next.retire_file_records(previous)?;
        next.catalog = next
            .catalog
            .replaced(entry, fingerprint, Some(prepared.configuration_identity))
            .map_err(IndexError::InvalidPublication)?;
        next.file_versions = next
            .file_versions
            .advanced(file_id)
            .map_err(IndexError::ResourceLimit)?
            .0;
        for segment in indexed {
            next = next.with_segment(segment)?;
        }
        // A replacement may remove a collection contribution without adding
        // a new shard there. Recheck its retired ratio after the catalog swap.
        for collection in &previous.collections {
            next.make_room_for_segment(collection.0 as usize)?;
        }
        next.revision = view.revision().next()?;
        Ok(next)
    }

    /// Apply one complete metadata observation through the same file
    /// preparation/publication paths as explicit mutations. Callers schedule
    /// this outside query threads; an unchanged pass does not read content or
    /// publish a new data revision.
    #[cfg(test)]
    pub(crate) fn reconcile_pass(&self, cancelled: impl Fn() -> bool) -> ReconciliationReport {
        self.reconcile_pass_with_progress(cancelled, |_| {})
    }

    fn reconcile_pass_with_progress(
        &self,
        cancelled: impl Fn() -> bool,
        mut on_progress: impl FnMut(&ReconciliationReport),
    ) -> ReconciliationReport {
        use crate::lifecycle_scan::{ScanChange, scan_sources_batched};

        let started = Instant::now();
        let mut report = ReconciliationReport::default();
        let snapshot = match self.view() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                report.record_issue(error.to_string());
                report.duration = started.elapsed();
                on_progress(&report);
                return report;
            }
        };
        let repository = snapshot.repository();
        let root = &repository.files().root;
        let definitions = repository
            .collections()
            .iter()
            .map(|collection| collection.definition.clone())
            .collect::<Vec<_>>();
        let mut apply = |change: ScanChange| {
            if cancelled() {
                report.cancelled = true;
                return;
            }
            report.planned_changes = report.planned_changes.saturating_add(1);
            let bytes = match &change {
                ScanChange::Add(observed) | ScanChange::Update { observed, .. } => {
                    observed.revision.size
                }
                ScanChange::Remove(_) => 0,
            };
            let path = match &change {
                ScanChange::Add(observed) | ScanChange::Remove(observed) => &observed.path,
                ScanChange::Update { observed, .. } => &observed.path,
            }
            .clone();
            let kind = match &change {
                ScanChange::Add(_) => 0,
                ScanChange::Update { .. } => 1,
                ScanChange::Remove(_) => 2,
            };
            let outcome = self.apply_reconciliation_change(&snapshot, root, change);
            match outcome {
                Ok(()) => {
                    report.indexed_bytes = report.indexed_bytes.saturating_add(bytes);
                    match kind {
                        0 => report.added += 1,
                        1 => report.updated += 1,
                        _ => report.removed += 1,
                    }
                }
                Err(error) => {
                    report.skipped += 1;
                    report.record_issue(format!("{}: {error}", path.display()));
                }
            }
            report.duration = started.elapsed();
            on_progress(&report);
        };
        let scan = scan_sources_batched(
            root,
            &definitions,
            &repository.sources,
            |_| false,
            |path| {
                snapshot
                    .0
                    .catalog
                    .id_for(path)
                    .and_then(|id| snapshot.0.catalog.scan_stamp(id).map(|stamp| (id, stamp)))
            },
            &cancelled,
            256,
            |batch| {
                for change in batch {
                    apply(change);
                }
            },
        );
        if !scan.cancelled {
            for id in snapshot.0.catalog.active_ids() {
                if cancelled() {
                    break;
                }
                if scan.seen_existing.contains(id.0) {
                    continue;
                }
                let Some(stamp) = snapshot.0.catalog.scan_stamp(id) else {
                    continue;
                };
                if stamp.collections.is_subset(&scan.complete_collections) {
                    apply(ScanChange::Remove(stamp));
                }
            }
        }
        drop(apply);
        report.examined = scan.examined;
        report.unchanged = scan.unchanged;
        report.skipped = report.skipped.saturating_add(scan.skipped);
        report.cancelled |= scan.cancelled || cancelled();
        for issue in &scan.issues {
            report.record_issue(format!(
                "collection {}: {}",
                issue.collection.0, issue.message
            ));
        }
        report.duration = started.elapsed();
        on_progress(&report);
        report
    }

    fn apply_reconciliation_change(
        &self,
        snapshot: &IndexView,
        root: &Path,
        change: crate::lifecycle_scan::ScanChange,
    ) -> Result<(), IndexError> {
        use crate::lifecycle_scan::ScanChange;
        match change {
            ScanChange::Add(observed) => {
                let prepared = self.prepare_add_file(&observed.path)?;
                if prepared.file.source.revision != observed.revision {
                    return Err(IndexError::SourceUnavailable(
                        "source changed after metadata scan".into(),
                    ));
                }
                let expected = prepared.expected_revision;
                let next = self.stage_prepared_add(prepared)?;
                self.publish_prepared(expected, next).map(|_| ())
            }
            ScanChange::Update { previous, observed } => {
                let handle = snapshot
                    .file_handle_for_path(&previous.path)
                    .ok_or(IndexError::UnknownFile)?;
                let prepared = self.prepare_replace_file(handle, &observed.path)?;
                if prepared.file.source.revision != observed.revision {
                    return Err(IndexError::SourceUnavailable(
                        "source changed after metadata scan".into(),
                    ));
                }
                let expected = prepared.expected_revision;
                let next = self.stage_prepared_replace(prepared)?;
                self.publish_prepared(expected, next).map(|_| ())
            }
            ScanChange::Remove(observed) => {
                let handle = snapshot
                    .file_handle_for_path(&observed.path)
                    .ok_or(IndexError::UnknownFile)?;
                match std::fs::metadata(root.join(&observed.path)) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        self.delete_file(handle).map(|_| ())
                    }
                    Ok(_) => Err(IndexError::SourceUnavailable(
                        "source reappeared after metadata scan".into(),
                    )),
                    Err(error) => Err(IndexError::Io(error.to_string())),
                }
            }
        }
    }

    /// Retire all records owned by one file across its collections in one
    /// publication. This changes index state only, and requires no source
    /// content or source-file I/O. A later scan can add a still-present file.
    pub fn delete_file(&self, handle: FileHandle) -> Result<DeleteResult, IndexError> {
        let _writer = self.0.writer.lock().map_err(|_| IndexError::LockPoisoned)?;
        let (expected, next, retired_records, affected_collections) =
            self.stage_prepared_delete(handle)?;
        let revision = self.publish_prepared(expected, next)?;
        Ok(DeleteResult {
            revision,
            file: handle.file_id,
            retired_records,
            affected_collections,
            strategy: DeleteStrategy::Tombstone,
        })
    }

    pub(crate) fn stage_prepared_delete(
        &self,
        handle: FileHandle,
    ) -> Result<(IndexRevision, PublishedIndex, u64, Vec<String>), IndexError> {
        if handle.instance_id != self.instance_id() {
            return Err(IndexError::ForeignInstance);
        }
        let current = self.view()?;
        let actual = current
            .0
            .file_versions
            .get(handle.file_id)
            .ok_or(IndexError::UnknownFile)?;
        if actual != handle.version {
            return Err(IndexError::FileConflict {
                file: handle.file_id,
                expected: handle.version,
                actual,
            });
        }
        if !current.0.catalog.is_active(handle.file_id) {
            return Err(IndexError::UnknownFile);
        }
        let file = current
            .0
            .catalog
            .entry(handle.file_id)
            .ok_or(IndexError::UnknownFile)?;
        let mut next = (*current.0).clone();
        let retired_records = next.retire_file_records(file)?;
        let affected_collections = file
            .collections
            .iter()
            .map(|id| {
                next.repository.collections()[id.0 as usize]
                    .definition
                    .name
                    .clone()
            })
            .collect();
        next.catalog = next
            .catalog
            .retired(handle.file_id)
            .map_err(IndexError::InvalidPublication)?;
        next.file_versions = next
            .file_versions
            .advanced(handle.file_id)
            .map_err(IndexError::ResourceLimit)?
            .0;
        // Pure deletions have no shard admission to trigger retirement work.
        for collection in &file.collections {
            next.make_room_for_segment(collection.0 as usize)?;
        }
        next.revision = current.revision().next()?;
        Ok((
            current.revision(),
            next,
            retired_records,
            affected_collections,
        ))
    }
}

mod publication;

pub(crate) use publication::PublishedIndex;

mod view;

pub use view::{
    CollectionConstructionObservation, ConstructionObservation, IndexView, RecordLookupObservation,
};

#[cfg(test)]
mod tests;
