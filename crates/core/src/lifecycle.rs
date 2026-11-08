//! Pinned, revisioned publication boundary for a changing index.
//!
//! This module establishes immutable read views and typed identity. File
//! mutation, reconciliation and persistence are added in later lifecycle
//! phases; a wrapped static repository does not imply those operations exist.

use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::build::{
    FilePreparationBinding, IndexedCollectionSegment, PreparedCollectionBuild, PreparedFileBuild,
    SegmentAdmission, SourceFileInput, prepare_file_build,
};
use crate::discovery::selects_file;
use crate::inventory::{ActiveRecords, FileEntry, FileStatus, FileVersions};
use crate::lifecycle_catalog::FileCatalog;
use crate::model::{CollectionId, FileId, RecordSlot};
use crate::query::Query;
use crate::repository::{Repository, SearchHit, SearchResult};
use crate::streaming::{
    CancellationToken, ExecutionContext, ExecutionError, ResultScope, StreamControl, StreamError,
    StreamEvent, StreamOptions, StreamOutcome,
};
use crate::workload::ReturnMode;

pub use crate::inventory::FileVersion;

const MAX_SEGMENTS_PER_COLLECTION: usize = 64;

/// Monotonic publication sequence within one index instance.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct IndexRevision(pub u64);

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
    InvalidCache(String),
    StaleLocator,
    SourceUnavailable(String),
    ResourceLimit(String),
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
            Self::InvalidCache(message) => write!(formatter, "invalid index cache: {message}"),
            Self::StaleLocator => {
                formatter.write_str("record locator belongs to an old index view")
            }
            Self::SourceUnavailable(message) => write!(formatter, "source unavailable: {message}"),
            Self::ResourceLimit(message) => write!(formatter, "resource limit: {message}"),
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
    pub saved_revision: Option<IndexRevision>,
    pub reconciliation: ReconciliationState,
    pub indexed_files: usize,
    pub live_records: u64,
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
    pub unchanged: usize,
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub skipped: usize,
    pub cancelled: bool,
    pub issues: Vec<String>,
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

impl IndexStatus {
    pub fn dirty(&self) -> bool {
        self.saved_revision != Some(self.revision)
    }
}

/// One indexed delta shard and its independently retired local slots.
#[derive(Clone)]
pub(crate) struct PublishedSegment {
    pub(crate) ordinal: u64,
    pub(crate) index: Arc<IndexedCollectionSegment>,
    pub(crate) active: ActiveRecords,
}

/// One immutable root: shared base, bounded indexed segments, liveness, file
/// catalog and configuration all belong to the same publication revision.
#[derive(Clone)]
pub(crate) struct PublishedIndex {
    pub(crate) instance_id: u64,
    pub(crate) revision: IndexRevision,
    pub(crate) repository: Arc<Repository>,
    pub(crate) configuration_identity: [u8; 32],
    pub(crate) active: Vec<ActiveRecords>,
    pub(crate) segments: Vec<Arc<Vec<Arc<PublishedSegment>>>>,
    pub(crate) next_segment_ordinal: u64,
    pub(crate) catalog: FileCatalog,
    pub(crate) file_versions: FileVersions,
    pub(crate) saved_revision: Option<IndexRevision>,
    pub(crate) reconciliation: ReconciliationState,
}

impl PublishedIndex {
    pub(crate) fn from_repository(
        instance_id: u64,
        revision: IndexRevision,
        repository: Arc<Repository>,
    ) -> Result<Self, IndexError> {
        let definitions = repository
            .collections()
            .iter()
            .map(|collection| collection.definition.clone())
            .collect::<Vec<_>>();
        let configuration_identity = crate::storage::configuration_hash(
            &definitions,
            repository.options(),
            &repository.sources,
        )
        .map_err(IndexError::IncompatibleConfiguration)?;
        let active = repository
            .collections()
            .iter()
            .map(|collection| ActiveRecords::all(collection.generation.records.len()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(IndexError::ResourceLimit)?;
        let file_versions =
            FileVersions::filled(repository.files().files.len(), FileVersion(revision.0));
        let catalog = FileCatalog::from_repository(Arc::clone(&repository))
            .map_err(IndexError::ResourceLimit)?;
        let segments = vec![Arc::new(Vec::new()); active.len()];
        Ok(Self {
            instance_id,
            revision,
            repository,
            configuration_identity,
            active,
            segments,
            next_segment_ordinal: 1,
            catalog,
            file_versions,
            saved_revision: None,
            reconciliation: ReconciliationState::NotStarted,
        })
    }

    fn status(&self) -> IndexStatus {
        debug_assert_eq!(self.catalog.allocated_files(), self.file_versions.len());
        IndexStatus {
            instance_id: self.instance_id,
            revision: self.revision,
            saved_revision: self.saved_revision,
            reconciliation: self.reconciliation,
            indexed_files: self.catalog.active_files() as usize,
            live_records: self.active.iter().map(ActiveRecords::len).sum::<u64>()
                + self
                    .segments
                    .iter()
                    .flat_map(|segments| segments.iter())
                    .map(|segment| segment.active.len())
                    .sum::<u64>(),
        }
    }

    /// Stage one fully indexed local segment in a private next root. The
    /// caller may stage several affected collections before one publication.
    pub(crate) fn with_segment(
        mut self,
        index: IndexedCollectionSegment,
    ) -> Result<Self, IndexError> {
        let number = index.collection.0 as usize;
        let collection =
            self.repository.collections().get(number).ok_or_else(|| {
                IndexError::InvalidPublication("segment collection missing".into())
            })?;
        if index.source_id != collection.source.id()
            || index.source_version != collection.source.version()
        {
            return Err(IndexError::IncompatibleConfiguration(
                "segment source registration changed".into(),
            ));
        }
        if self.segments[number].len() >= MAX_SEGMENTS_PER_COLLECTION {
            return Err(IndexError::ResourceLimit(
                "collection segment budget reached; compaction is required".into(),
            ));
        }
        for file in &index.files {
            if self.catalog.entry(file.entry.id).is_none_or(|current| {
                !current.collections.contains(&index.collection)
                    || current.relative_path != file.entry.relative_path
                    || current.revision != file.entry.revision
                    || current.created_ns != file.entry.created_ns
            }) {
                return Err(IndexError::InvalidPublication(
                    "segment file is not active in its collection".into(),
                ));
            }
        }
        for (id, _) in &index.id_lookup {
            if collection
                .id_lookup
                .get(id)
                .is_some_and(|slot| self.active[number].contains(*slot))
                || self.segments[number].iter().any(|segment| {
                    segment
                        .index
                        .id_lookup
                        .get(id)
                        .is_some_and(|slot| segment.active.contains(*slot))
                })
            {
                return Err(IndexError::DuplicateRecordId {
                    collection: collection.definition.name.clone(),
                    record_id: id.0.clone(),
                });
            }
        }
        let active = ActiveRecords::all(index.records.len()).map_err(IndexError::ResourceLimit)?;
        let ordinal = self.next_segment_ordinal;
        self.next_segment_ordinal = self
            .next_segment_ordinal
            .checked_add(1)
            .ok_or_else(|| IndexError::ResourceLimit("segment ordinal overflow".into()))?;
        Arc::make_mut(&mut self.segments[number]).push(Arc::new(PublishedSegment {
            ordinal,
            index: Arc::new(index),
            active,
        }));
        Ok(self)
    }

    /// Retire slots owned by one incarnation without touching the catalog.
    /// The caller either replaces or removes its catalog entry atomically.
    fn retire_file_records(&mut self, file: &FileEntry) -> Result<u64, IndexError> {
        let mut retired_records = 0u64;
        for collection_id in &file.collections {
            let collection = self
                .repository
                .collections()
                .get(collection_id.0 as usize)
                .ok_or_else(|| IndexError::InvalidPublication("file collection missing".into()))?;
            let active = self
                .active
                .get_mut(collection_id.0 as usize)
                .ok_or_else(|| IndexError::InvalidPublication("active domain missing".into()))?;
            if let Some(slots) = collection.generation.file_records.records_for(file.id) {
                retired_records +=
                    slots.iter().filter(|slot| active.contains(**slot)).count() as u64;
                *active = active
                    .retired(slots)
                    .map_err(IndexError::InvalidPublication)?;
            }
            let segments = self
                .segments
                .get_mut(collection_id.0 as usize)
                .ok_or_else(|| {
                    IndexError::InvalidPublication("segment collection missing".into())
                })?;
            for segment in Arc::make_mut(segments).iter_mut() {
                if let Some(slots) = segment.index.file_records(file.id).map(<[_]>::to_vec) {
                    let segment = Arc::make_mut(segment);
                    retired_records += slots
                        .iter()
                        .filter(|slot| segment.active.contains(**slot))
                        .count() as u64;
                    segment.active = segment
                        .active
                        .retired(&slots)
                        .map_err(IndexError::InvalidPublication)?;
                }
            }
        }
        Ok(retired_records)
    }
}

fn index_prepared_file(
    repository: &Repository,
    source: &SourceFileInput,
    collections: Vec<PreparedCollectionBuild>,
) -> Result<(FileEntry, Vec<IndexedCollectionSegment>), IndexError> {
    let options = repository.options();
    let mut indexed = Vec::with_capacity(collections.len());
    for collection in collections {
        let state = repository
            .collections()
            .get(collection.collection.0 as usize)
            .ok_or_else(|| IndexError::InvalidPublication("collection missing".into()))?;
        let mut admission = SegmentAdmission::new(collection.collection, &state.source);
        admission
            .admit(source, collection)
            .map_err(IndexError::Build)?;
        indexed.push(
            admission
                .seal()
                .into_indexed(
                    &state.definition,
                    options.bigram_alphabet,
                    options.inference,
                    repository.collections().len(),
                    &repository.workers,
                )
                .map_err(IndexError::Build)?,
        );
    }
    let memberships = indexed.iter().map(|segment| segment.collection).collect();
    let mut entry = FileEntry {
        id: source.id,
        relative_path: source.relative_path.clone(),
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
        relative_path: relative.to_owned(),
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
    published: RwLock<Arc<PublishedIndex>>,
    writer: Mutex<()>,
    maintenance: Mutex<MaintenanceState>,
}

struct MaintenanceState {
    reconciliation: ReconciliationState,
    current: Option<Arc<ReconciliationJob>>,
}

struct ReconciliationJob {
    cancelled: AtomicBool,
    result: Mutex<Option<ReconciliationReport>>,
    complete: Condvar,
    worker: Mutex<Option<JoinHandle<()>>>,
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
        let instance_id = crate::repository::next_repository_instance();
        let published = Arc::new(PublishedIndex::from_repository(
            instance_id,
            IndexRevision::INITIAL,
            Arc::new(repository),
        )?);
        Ok(Self(Arc::new(IndexInner {
            instance_id,
            published: RwLock::new(published),
            writer: Mutex::new(()),
            maintenance: Mutex::new(MaintenanceState {
                reconciliation: ReconciliationState::NotStarted,
                current: None,
            }),
        })))
    }

    pub fn instance_id(&self) -> u64 {
        self.0.instance_id
    }

    /// Acquire one immutable view; no publication lock is held by its queries.
    pub fn view(&self) -> Result<IndexView, IndexError> {
        let published = self
            .0
            .published
            .read()
            .map_err(|_| IndexError::LockPoisoned)?;
        Ok(IndexView(Arc::clone(&published)))
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

    /// Coalesce concurrent scan requests into one worker. The scan runs
    /// without holding a query or publication lock; waiters receive its
    /// report and can retry a later pass after partial failure.
    pub(crate) fn start_reconcile(&self) -> Result<ReconciliationTask, IndexError> {
        let mut maintenance = self
            .0
            .maintenance
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?;
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
                    owner.reconcile_pass(|| worker_job.cancelled.load(Ordering::Acquire))
                }))
                .unwrap_or_else(|_| ReconciliationReport {
                    issues: vec!["reconciliation worker panicked".into()],
                    ..ReconciliationReport::default()
                });
                let state = if report.cancelled {
                    ReconciliationState::Cancelled
                } else if report.issues.is_empty() {
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

    /// Cancel and join the current scan worker before releasing this handle.
    pub fn shutdown(&self) -> Result<Option<ReconciliationReport>, IndexError> {
        let current = self
            .0
            .maintenance
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?
            .current
            .clone();
        current
            .map(|job| {
                let task = ReconciliationTask(job);
                task.cancel();
                task.wait()
            })
            .transpose()
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
        let (entry, indexed) =
            index_prepared_file(view.repository(), &source, prepared.file.collections)?;
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
        let (entry, indexed) =
            index_prepared_file(view.repository(), &source, prepared.file.collections)?;
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
        next.revision = view.revision().next()?;
        Ok(next)
    }

    /// Apply one complete metadata observation through the same file
    /// preparation/publication paths as explicit mutations. Callers schedule
    /// this outside query threads; an unchanged pass does not read content or
    /// publish a new data revision.
    pub(crate) fn reconcile_pass(&self, cancelled: impl Fn() -> bool) -> ReconciliationReport {
        use crate::lifecycle_scan::{ScanChange, plan_scan, scan_sources};

        let mut report = ReconciliationReport::default();
        let snapshot = match self.view() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                report.issues.push(error.to_string());
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
        let prior = snapshot.0.catalog.scan_stamps();
        let scan = scan_sources(root, &definitions, &repository.sources, &cancelled);
        report.examined = scan.examined;
        report.cancelled = scan.cancelled;
        report.issues.extend(
            scan.issues
                .iter()
                .map(|issue| format!("collection {}: {}", issue.collection.0, issue.message)),
        );
        let plan = plan_scan(&prior, &scan);
        report.unchanged = plan.unchanged;
        report.skipped = plan.skipped;
        for change in plan.changes {
            if cancelled() {
                report.cancelled = true;
                break;
            }
            let path = match &change {
                ScanChange::Add(observed) | ScanChange::Remove(observed) => &observed.path,
                ScanChange::Update { observed, .. } => &observed.path,
            }
            .clone();
            let outcome = match change {
                ScanChange::Add(observed) => {
                    self.prepare_add_file(&observed.path).and_then(|prepared| {
                        if prepared.file.source.revision != observed.revision {
                            return Err(IndexError::SourceUnavailable(
                                "source changed after metadata scan".into(),
                            ));
                        }
                        let expected = prepared.expected_revision;
                        let next = self.stage_prepared_add(prepared)?;
                        self.publish_prepared(expected, next).map(|_| ())
                    })
                }
                ScanChange::Update { previous, observed } => snapshot
                    .file_handle_for_path(&previous.path)
                    .ok_or(IndexError::UnknownFile)
                    .and_then(|handle| {
                        let prepared = self.prepare_replace_file(handle, &observed.path)?;
                        if prepared.file.source.revision != observed.revision {
                            return Err(IndexError::SourceUnavailable(
                                "source changed after metadata scan".into(),
                            ));
                        }
                        let expected = prepared.expected_revision;
                        let next = self.stage_prepared_replace(prepared)?;
                        self.publish_prepared(expected, next).map(|_| ())
                    }),
                ScanChange::Remove(observed) => snapshot
                    .file_handle_for_path(&observed.path)
                    .ok_or(IndexError::UnknownFile)
                    .and_then(
                        |handle| match std::fs::metadata(root.join(&observed.path)) {
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                                self.delete_file(handle).map(|_| ())
                            }
                            Ok(_) => Err(IndexError::SourceUnavailable(
                                "source reappeared after metadata scan".into(),
                            )),
                            Err(error) => Err(IndexError::Io(error.to_string())),
                        },
                    ),
            };
            match outcome {
                Ok(()) => match scan.files.get(&path) {
                    Some(_) if prior.contains_key(&path) => report.updated += 1,
                    Some(_) => report.added += 1,
                    None => report.removed += 1,
                },
                Err(error) => {
                    report.skipped += 1;
                    report.issues.push(format!("{}: {error}", path.display()));
                }
            }
        }
        report
    }

    /// Retire all records owned by one file across its collections in one
    /// publication. This changes index state only, and requires no source
    /// content or source-file I/O. A later scan can add a still-present file.
    pub fn delete_file(&self, handle: FileHandle) -> Result<DeleteResult, IndexError> {
        if handle.instance_id != self.instance_id() {
            return Err(IndexError::ForeignInstance);
        }
        let _writer = self.0.writer.lock().map_err(|_| IndexError::LockPoisoned)?;
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
        next.revision = current.revision().next()?;
        let revision = self.publish_prepared(current.revision(), next)?;
        Ok(DeleteResult {
            revision,
            file: handle.file_id,
            retired_records,
            affected_collections,
            strategy: DeleteStrategy::Tombstone,
        })
    }

    /// Install a fully prepared root only if its captured predecessor is
    /// current. The retired Arc is dropped after releasing the write lock.
    /// No filesystem work or index construction belongs inside this method.
    pub(crate) fn publish_prepared(
        &self,
        expected: IndexRevision,
        next: PublishedIndex,
    ) -> Result<IndexRevision, IndexError> {
        if next.instance_id != self.0.instance_id {
            return Err(IndexError::ForeignInstance);
        }
        let next_revision = expected.next()?;
        if next.revision != next_revision {
            return Err(IndexError::InvalidPublication(
                "prepared root must carry the next revision".into(),
            ));
        }
        let next = Arc::new(next);
        let retired = {
            let mut guard = self
                .0
                .published
                .write()
                .map_err(|_| IndexError::LockPoisoned)?;
            if guard.revision != expected {
                return Err(IndexError::RevisionConflict {
                    expected,
                    actual: guard.revision,
                });
            }
            std::mem::replace(&mut *guard, next)
        };
        drop(retired);
        Ok(next_revision)
    }
}

/// Pinned immutable read context for a single publication revision.
#[derive(Clone)]
pub struct IndexView(Arc<PublishedIndex>);

impl IndexView {
    fn has_segments(&self) -> bool {
        self.0.segments.iter().any(|group| !group.is_empty())
    }

    fn require_base_only(&self, route: &str) -> Result<(), String> {
        if self.has_segments() {
            Err(format!(
                "{route} across indexed segments is not available yet"
            ))
        } else {
            Ok(())
        }
    }

    fn stamp_result(&self, mut result: SearchResult) -> SearchResult {
        for hit in &mut result.hits {
            hit.generation = self.revision().0;
        }
        result
    }

    pub fn instance_id(&self) -> u64 {
        self.0.instance_id
    }

    pub fn revision(&self) -> IndexRevision {
        self.0.revision
    }

    /// Fingerprint of the schema, source registrations and index settings
    /// captured by this publication.
    pub fn configuration_identity(&self) -> [u8; 32] {
        self.0.configuration_identity
    }

    pub fn file_handle(&self, file: FileId) -> Option<FileHandle> {
        self.0.catalog.is_active(file).then(|| {
            self.0.file_versions.get(file).map(|version| FileHandle {
                instance_id: self.0.instance_id,
                file_id: file,
                version,
            })
        })?
    }

    /// Resolve an active file by a repository-relative path, or by an
    /// absolute path beneath this view's source root.
    pub fn file_handle_for_path(&self, path: &Path) -> Option<FileHandle> {
        let relative = if path.is_absolute() {
            path.strip_prefix(&self.0.repository.files().root).ok()?
        } else {
            path
        };
        self.0
            .catalog
            .id_for(relative)
            .and_then(|id| self.file_handle(id))
    }

    pub fn file_version(&self, file: FileId) -> Option<FileVersion> {
        self.0
            .catalog
            .is_active(file)
            .then(|| self.0.file_versions.get(file))?
    }

    pub(crate) fn is_active(&self, collection: CollectionId, slot: RecordSlot) -> bool {
        self.0
            .active
            .get(collection.0 as usize)
            .is_some_and(|active| active.contains(slot))
    }

    pub(crate) fn active_slots(&self, collection: CollectionId) -> Option<roaring::RoaringBitmap> {
        self.0
            .active
            .get(collection.0 as usize)
            .map(ActiveRecords::bitmap)
    }

    pub(crate) fn segments(&self, collection: CollectionId) -> Option<&[Arc<PublishedSegment>]> {
        self.0
            .segments
            .get(collection.0 as usize)
            .map(|segments| segments.as_slice())
    }

    pub(crate) fn repository(&self) -> &Repository {
        &self.0.repository
    }

    pub fn search(&self, query: &Query) -> Result<SearchResult, String> {
        let resolved = query.resolve_current_clock()?;
        if !self.has_segments() {
            crate::run::search_active(self.repository(), resolved.as_ref(), &self.0.active)
                .map(|result| self.stamp_result(result))
        } else {
            crate::run::search_segmented(
                self.repository(),
                resolved.as_ref(),
                &self.0.active,
                &self.0.segments,
                &self.0.catalog,
                self.revision(),
            )
        }
    }

    pub fn search_with_context(
        &self,
        query: &Query,
        context: &ExecutionContext,
    ) -> Result<SearchResult, ExecutionError> {
        context.enter()?;
        let resolved = query.with_clock(context.clock.now).map_err(|message| {
            context.error(crate::streaming::ExecutionErrorKind::InvalidQuery, message)
        })?;
        if !self.has_segments() {
            crate::run::search_with_context_active(
                self.repository(),
                &resolved,
                context,
                &self.0.active,
            )
            .map(|result| self.stamp_result(result))
        } else {
            crate::run::search_segmented(
                self.repository(),
                &resolved,
                &self.0.active,
                &self.0.segments,
                &self.0.catalog,
                self.revision(),
            )
            .map_err(|message| {
                context.error(crate::streaming::ExecutionErrorKind::Execution, message)
            })
        }
    }

    pub fn count_exact(&self, query: &Query) -> Result<usize, String> {
        let mut request = query.clone();
        request.options.count = true;
        request.options.limit = 0;
        request.options.offset = 0;
        self.search(&request)?
            .count
            .ok_or("count result missing".into())
    }

    pub fn count_exact_with_context(
        &self,
        query: &Query,
        context: &ExecutionContext,
    ) -> Result<usize, ExecutionError> {
        let mut request = query.clone();
        request.options.count = true;
        request.options.limit = 0;
        request.options.offset = 0;
        self.search_with_context(&request, context)?
            .count
            .ok_or_else(|| ExecutionError {
                kind: crate::streaming::ExecutionErrorKind::Execution,
                message: "count result missing".into(),
            })
    }

    pub fn get_record(
        &self,
        collection: &str,
        record_id: &str,
        return_mode: ReturnMode,
    ) -> Result<Option<SearchHit>, String> {
        let mut base = self.repository().get_record_with_active(
            collection,
            record_id,
            return_mode.clone(),
            Some(&self.0.active),
        )?;
        if let Some(hit) = &mut base {
            hit.generation = self.revision().0;
        }
        if base.is_some() {
            return Ok(base);
        }
        let number = self
            .repository()
            .collections()
            .iter()
            .position(|state| state.definition.name == collection)
            .ok_or_else(|| format!("unknown collection {collection}"))?;
        for segment in self.0.segments[number].iter() {
            if let Some(hit) = crate::run::get_segment_record_by_id(
                self.repository(),
                number,
                &segment.index,
                &segment.active,
                &self.0.catalog,
                self.revision(),
                record_id,
                return_mode.clone(),
            )? {
                return Ok(Some(hit));
            }
        }
        Ok(None)
    }

    pub fn search_reference(&self, query: &Query) -> Result<SearchResult, String> {
        self.require_base_only("reference search")?;
        self.repository()
            .search_reference_with_active(query, Some(&self.0.active))
            .map(|result| self.stamp_result(result))
    }

    pub fn select_exact(
        &self,
        collection: &str,
        query: &Query,
    ) -> Result<crate::posting_values::ExactSelection, String> {
        self.require_base_only("exact selection")?;
        self.repository()
            .select_exact_for_view(collection, query, self.revision(), &self.0.active)
    }

    pub fn posting_values(
        &self,
        request: &crate::posting_values::PostingValuesRequest,
    ) -> Result<crate::posting_values::PostingValuesPage, String> {
        self.require_base_only("posting values")?;
        self.repository()
            .posting_values_for_view(request, self.revision(), &self.0.active)
    }

    pub fn relation_outgoing_page(
        &self,
        source_collection: &str,
        relation_path: &[String],
        source_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<crate::relation_inspection::OutgoingReferencePage, String> {
        self.require_base_only("outgoing relation inspection")?;
        self.repository().relation_outgoing_page_for_view(
            source_collection,
            relation_path,
            source_id,
            offset,
            limit,
            &self.0.active,
        )
    }

    pub fn relation_backlink_page(
        &self,
        target_collection: &str,
        reverse_alias: &str,
        target_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<crate::relation_inspection::BacklinkPage, String> {
        self.require_base_only("backlink relation inspection")?;
        self.repository().relation_backlink_page_for_view(
            target_collection,
            reverse_alias,
            target_id,
            offset,
            limit,
            &self.0.active,
        )
    }

    pub fn collection_graph(&self) -> Result<crate::relation_inspection::CollectionGraph, String> {
        self.require_base_only("collection graph")?;
        self.repository().collection_graph_for_view(&self.0.active)
    }

    pub fn search_analyze(
        &self,
        query: &Query,
    ) -> Result<(SearchResult, Vec<crate::run::QueryPlanTrace>), String> {
        self.require_base_only("analyzed search")?;
        let resolved = query.resolve_current_clock()?;
        crate::run::search_analyze_with_active(
            self.repository(),
            resolved.as_ref(),
            Some(&self.0.active),
        )
        .map(|(result, trace)| (self.stamp_result(result), trace))
    }

    pub fn explain(&self, query: &Query) -> Result<Vec<crate::run::QueryPlanTrace>, String> {
        self.require_base_only("query explanation")?;
        let resolved = query.resolve_current_clock()?;
        crate::run::explain_with_active(self.repository(), resolved.as_ref(), Some(&self.0.active))
    }

    pub fn search_stream<F>(
        &self,
        query: &Query,
        options: StreamOptions,
        cancellation: &CancellationToken,
        sink: &mut F,
    ) -> Result<StreamOutcome, StreamError>
    where
        F: for<'event> FnMut(StreamEvent<'event>) -> Result<StreamControl, String>,
    {
        self.search_stream_scoped(
            query,
            ResultScope::Page {
                offset: query.options.offset,
                limit: query.options.limit,
            },
            options,
            cancellation,
            sink,
        )
    }

    pub fn search_stream_scoped<F>(
        &self,
        query: &Query,
        scope: ResultScope,
        options: StreamOptions,
        cancellation: &CancellationToken,
        sink: &mut F,
    ) -> Result<StreamOutcome, StreamError>
    where
        F: for<'event> FnMut(StreamEvent<'event>) -> Result<StreamControl, String>,
    {
        let context = ExecutionContext::new(
            crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
            crate::streaming::ExecutionLimits::default(),
            Some(cancellation.clone()),
        );
        self.search_stream_scoped_with_context(query, scope, options, &context, sink)
    }

    pub fn search_stream_scoped_with_context<F>(
        &self,
        query: &Query,
        scope: ResultScope,
        options: StreamOptions,
        context: &ExecutionContext,
        sink: &mut F,
    ) -> Result<StreamOutcome, StreamError>
    where
        F: for<'event> FnMut(StreamEvent<'event>) -> Result<StreamControl, String>,
    {
        self.require_base_only("streamed search")
            .map_err(|message| StreamError::Execution {
                kind: crate::streaming::ExecutionErrorKind::Execution,
                message,
                progress: Default::default(),
            })?;
        let scoped_context = context.with_active(&self.0.active);
        self.repository().search_stream_scoped_with_context_active(
            query,
            scope,
            options,
            &scoped_context,
            sink,
            Some(&self.0.active),
        )
    }
}

#[cfg(test)]
#[path = "lifecycle.test.rs"]
mod tests;
