//! Pinned, revisioned publication boundary for a changing index.
//!
//! This module establishes immutable read views and typed identity. File
//! mutation, reconciliation and persistence are added in later lifecycle
//! phases; a wrapped static repository does not imply those operations exist.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Condvar, Mutex, RwLock, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

use crate::build::{
    FilePreparationBinding, IndexedCollectionSegment, PreparedCollectionBuild, PreparedFileBuild,
    SegmentAdmission, SourceFileInput, prepare_file_build,
};
use crate::discovery::selects_file;
use crate::inventory::{ActiveRecords, FileEntry, FileStatus, FileVersions};
use crate::lifecycle_catalog::FileCatalog;
#[cfg(test)]
use crate::model::RecordSlot;
use crate::model::{CollectionId, FileId, Value};
use crate::query::Query;
use crate::repository::{Repository, SearchHit, SearchResult};
use crate::streaming::{
    CancellationToken, ExecutionContext, ExecutionError, ResultScope, StreamControl, StreamError,
    StreamEvent, StreamOptions, StreamOutcome,
};
use crate::workload::ReturnMode;

pub use crate::inventory::FileVersion;

pub(crate) const MAX_SEGMENTS_PER_COLLECTION: usize = 64;
const MAX_RECONCILIATION_ISSUES: usize = 128;


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
    /// Changes only when record contents, membership, or binding semantics change.
    pub revision: IndexRevision,
    /// Changes when the physical capability set or its exact coverage changes.
    pub capability_revision: IndexRevision,
    pub saved_revision: Option<IndexRevision>,
    pub saved_capability_revision: Option<IndexRevision>,
    pub reconciliation: ReconciliationState,
    pub indexed_files: usize,
    pub live_records: u64,
}

/// Construction milestones are independent of ordinary query execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StagedBuildPhase {
    Discovering,
    BuildingBase,
    SearchReady,
    BuildingExtended,
    Complete,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagedFile {
    pub id: FileId,
    pub relative_path: PathBuf,
    pub collections: Vec<CollectionId>,
    pub size: u64,
    pub created_ns: Option<i128>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagedBuildStatus {
    pub phase: StagedBuildPhase,
    pub discovered_files: usize,
    pub located_records: u64,
    pub data_revision: Option<IndexRevision>,
    pub capability_revision: Option<IndexRevision>,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtendedPhase {
    Unnecessary,
    Pending,
    Building,
    Complete,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtendedStatus {
    pub phase: ExtendedPhase,
    pub completed_files: usize,
    pub total_files: usize,
    pub error: Option<String>,
}

struct ExtendedJob {
    cancelled: AtomicBool,
    status: Mutex<ExtendedStatus>,
    result: Mutex<Option<Result<(), IndexError>>>,
    complete: Condvar,
}

struct StagedOpening {
    cancelled: AtomicBool,
    status: Mutex<StagedBuildStatus>,
    catalogue: Mutex<Vec<StagedFile>>,
    index: Mutex<Option<Index>>,
    ready: Condvar,
    error: Mutex<Option<IndexError>>,
    extended: Arc<ExtendedJob>,
}

pub(crate) struct StagedReadyConfiguration {
    pub(crate) cache: PathBuf,
    pub(crate) protected_files: Vec<PathBuf>,
    pub(crate) sync_cache: bool,
    pub(crate) replace_invalid_cache: bool,
}

/// A fresh construction starts without holding the calling thread. The
/// returned `Index` retains its own extended worker after SearchReady.
#[derive(Clone)]
pub struct StagedBuildTask(Arc<StagedOpening>);


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


impl IndexStatus {
    pub fn dirty(&self) -> bool {
        self.saved_revision != Some(self.revision)
            || self.saved_capability_revision != Some(self.capability_revision)
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
    pub(crate) capability_revision: IndexRevision,
    pub(crate) repository: Arc<Repository>,
    pub(crate) configuration_identity: [u8; 32],
    pub(crate) active: Vec<ActiveRecords>,
    pub(crate) segments: Vec<Arc<Vec<Arc<PublishedSegment>>>>,
    pub(crate) next_segment_ordinal: u64,
    pub(crate) catalog: FileCatalog,
    pub(crate) file_versions: FileVersions,
    pub(crate) saved_revision: Option<IndexRevision>,
    pub(crate) saved_capability_revision: Option<IndexRevision>,
    pub(crate) reconciliation: ReconciliationState,
}


fn cheapest_consolidation_pair(group: &[Arc<PublishedSegment>]) -> Option<(usize, usize)> {
    let mut sized = group
        .iter()
        .enumerate()
        .map(|(index, segment)| (segment.index.records.len(), index))
        .collect::<Vec<_>>();
    sized.sort_unstable();
    for (position, &(left_size, left)) in sized.iter().enumerate() {
        for &(right_size, right) in &sized[position + 1..] {
            if left_size + right_size > crate::lifecycle_partition::BASE_PARTITION_RECORDS {
                break;
            }
            let left_index = &group[left].index;
            let right_index = &group[right].index;
            if left_index
                .files
                .iter()
                .any(|file| right_index.file(file.entry.id).is_some())
                || left_index
                    .id_lookup
                    .keys()
                    .any(|id| right_index.id_lookup.contains_key(id))
            {
                continue;
            }
            return Some((left.min(right), left.max(right)));
        }
    }
    None
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
            capability_revision: IndexRevision::INITIAL,
            repository,
            configuration_identity,
            active,
            segments,
            next_segment_ordinal: 1,
            catalog,
            file_versions,
            saved_revision: None,
            saved_capability_revision: None,
            reconciliation: ReconciliationState::NotStarted,
        })
    }

    fn status(&self) -> IndexStatus {
        debug_assert_eq!(self.catalog.allocated_files(), self.file_versions.len());
        IndexStatus {
            instance_id: self.instance_id,
            revision: self.revision,
            capability_revision: self.capability_revision,
            saved_revision: self.saved_revision,
            saved_capability_revision: self.saved_capability_revision,
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
        if number >= self.segments.len() {
            return Err(IndexError::InvalidPublication(
                "segment collection missing".into(),
            ));
        }
        self.make_room_for_segment(number)?;
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

    /// Coalesce small delta shards without enlarging a partition beyond the
    /// local verified-maintenance budget. Full base partitions do not consume
    /// the sparse-delta allowance, even when a collection has more than 64.
    pub(crate) fn make_room_for_segment(&mut self, number: usize) -> Result<(), IndexError> {
        let full_partitions = self.segments[number]
            .iter()
            .filter(|segment| {
                segment.index.records.len() >= crate::lifecycle_partition::BASE_PARTITION_RECORDS
            })
            .count();
        let admission_limit = MAX_SEGMENTS_PER_COLLECTION.saturating_add(full_partitions);
        if self.segments[number].len() < admission_limit {
            return Ok(());
        }
        let group = Arc::make_mut(&mut self.segments[number]);
        group.retain(|segment| segment.active.len() > 0);
        if group.len() < admission_limit {
            return Ok(());
        }
        let Some((left, right)) = cheapest_consolidation_pair(group) else {
            return Ok(());
        };
        let merged = IndexedCollectionSegment::consolidate(
            &[&group[left].index, &group[right].index],
            crate::lifecycle_partition::BASE_PARTITION_RECORDS,
        )
        .map_err(IndexError::Build)?;
        let active = merged
            .translate_active(&[&group[left].active, &group[right].active])
            .map_err(IndexError::Build)?;
        let ordinal = self.next_segment_ordinal;
        self.next_segment_ordinal = self
            .next_segment_ordinal
            .checked_add(1)
            .ok_or_else(|| IndexError::ResourceLimit("segment ordinal overflow".into()))?;
        group.remove(right);
        group.remove(left);
        group.push(Arc::new(PublishedSegment {
            ordinal,
            index: Arc::new(merged.index),
            active,
        }));
        Ok(())
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
    restored_from_cache: AtomicBool,
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
    persistence: Mutex<PersistenceState>,
    sync_cache: Mutex<Option<SyncCacheWorker>>,
    sync_cache_error: Mutex<Option<String>>,
    extended: Mutex<Option<Arc<ExtendedJob>>>,
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


struct SyncCacheWorker {
    signal: SyncSender<()>,
    worker: JoinHandle<()>,
}


struct PersistenceState {
    selected_cache: Option<PathBuf>,
    saved: HashMap<PathBuf, (IndexRevision, IndexRevision)>,
    protected_files: Vec<PathBuf>,
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

impl ExtendedJob {
    fn new(phase: ExtendedPhase, total_files: usize) -> Self {
        let terminal = matches!(phase, ExtendedPhase::Unnecessary | ExtendedPhase::Complete);
        Self {
            cancelled: AtomicBool::new(false),
            status: Mutex::new(ExtendedStatus {
                phase,
                completed_files: if terminal { total_files } else { 0 },
                total_files,
                error: None,
            }),
            result: Mutex::new(terminal.then_some(Ok(()))),
            complete: Condvar::new(),
        }
    }

    fn set_phase(&self, phase: ExtendedPhase) {
        if let Ok(mut status) = self.status.lock() {
            status.phase = phase;
        }
    }

    fn finish(&self, result: Result<(), IndexError>) {
        if let Ok(mut status) = self.status.lock() {
            status.phase = match &result {
                Ok(()) if status.phase == ExtendedPhase::Unnecessary => ExtendedPhase::Unnecessary,
                Ok(()) => ExtendedPhase::Complete,
                Err(IndexError::Cancelled) => ExtendedPhase::Cancelled,
                Err(_) => ExtendedPhase::Failed,
            };
            if result.is_ok() {
                status.completed_files = status.total_files;
            } else {
                status.error = result.as_ref().err().map(ToString::to_string);
            }
        }
        if let Ok(mut outcome) = self.result.lock() {
            *outcome = Some(result);
            self.complete.notify_all();
        }
    }

    fn wait(&self) -> Result<(), IndexError> {
        let mut result = self.result.lock().map_err(|_| IndexError::LockPoisoned)?;
        while result.is_none() {
            result = self.complete.wait(result).map_err(|_| IndexError::LockPoisoned)?;
        }
        result.as_ref().cloned().ok_or(IndexError::LockPoisoned)?
    }
}

impl StagedBuildTask {
    fn new() -> Self {
        Self(Arc::new(StagedOpening {
            cancelled: AtomicBool::new(false),
            status: Mutex::new(StagedBuildStatus {
                phase: StagedBuildPhase::Discovering,
                discovered_files: 0,
                located_records: 0,
                data_revision: None,
                capability_revision: None,
                error: None,
            }),
            catalogue: Mutex::new(Vec::new()),
            index: Mutex::new(None),
            ready: Condvar::new(),
            error: Mutex::new(None),
            extended: Arc::new(ExtendedJob::new(ExtendedPhase::Pending, 0)),
        }))
    }

    pub(crate) fn from_ready(index: Index) -> Result<Self, IndexError> {
        let task = Self::new();
        let view = index.view()?;
        let status = index.status()?;
        let catalogue = view
            .0
            .catalog
            .snapshot_slots()
            .into_iter()
            .flatten()
            .map(|(entry, _, _)| StagedFile {
                id: entry.id,
                relative_path: entry.relative_path,
                collections: entry.collections,
                size: entry.revision.size,
                created_ns: entry.created_ns,
            })
            .collect::<Vec<_>>();
        *task.0.catalogue.lock().map_err(|_| IndexError::LockPoisoned)? = catalogue;
        *task.0.status.lock().map_err(|_| IndexError::LockPoisoned)? = StagedBuildStatus {
            phase: StagedBuildPhase::Complete,
            discovered_files: status.indexed_files,
            located_records: status.live_records,
            data_revision: Some(status.revision),
            capability_revision: Some(status.capability_revision),
            error: None,
        };
        task.0.extended.finish(Ok(()));
        *task.0.index.lock().map_err(|_| IndexError::LockPoisoned)? = Some(index);
        task.0.ready.notify_all();
        Ok(task)
    }

    pub fn catalogue(&self) -> Result<Vec<StagedFile>, IndexError> {
        Ok(self.0.catalogue.lock().map_err(|_| IndexError::LockPoisoned)?.clone())
    }

    pub fn status(&self) -> Result<StagedBuildStatus, IndexError> {
        let mut status = self.0.status.lock().map_err(|_| IndexError::LockPoisoned)?.clone();
        if let Some(index) = self.0.index.lock().map_err(|_| IndexError::LockPoisoned)?.as_ref() {
            let current = index.status()?;
            status.data_revision = Some(current.revision);
            status.capability_revision = Some(current.capability_revision);
            status.located_records = current.live_records;
        }
        Ok(status)
    }

    pub fn wait_search_ready(&self) -> Result<Index, IndexError> {
        let mut index = self.0.index.lock().map_err(|_| IndexError::LockPoisoned)?;
        loop {
            if let Some(index) = index.as_ref() {
                return Ok(index.clone());
            }
            if let Some(error) = self.0.error.lock().map_err(|_| IndexError::LockPoisoned)?.as_ref() {
                return Err(error.clone());
            }
            index = self.0.ready.wait(index).map_err(|_| IndexError::LockPoisoned)?;
        }
    }

    pub fn wait_complete(&self) -> Result<Index, IndexError> {
        let index = self.wait_search_ready()?;
        index.wait_extended()?;
        Ok(index)
    }

    pub fn cancel_extended(&self) {
        self.0.extended.cancelled.store(true, Ordering::Release);
    }

    /// Cancel base construction before SearchReady, or optional work after it.
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
        self.cancel_extended();
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
    /// Begin discovery and base search construction on a worker. The returned
    /// task exposes file identities before SearchReady and yields a live Index
    /// as soon as the base is published.
    pub fn start_staged_build(
        root: impl Into<PathBuf>,
        definitions: Vec<crate::repository::CollectionDefinition>,
        options: crate::repository::RepositoryOptions,
    ) -> Result<StagedBuildTask, IndexError> {
        Self::start_staged_build_with_sources(
            root, definitions, options, crate::source::SourceRegistry::builtin(),
        )
    }

    pub fn start_staged_build_with_sources(
        root: impl Into<PathBuf>,
        definitions: Vec<crate::repository::CollectionDefinition>,
        options: crate::repository::RepositoryOptions,
        sources: crate::source::SourceRegistry,
    ) -> Result<StagedBuildTask, IndexError> {
        Self::start_staged_build_with_sources_and_cache(
            root, definitions, options, sources, None,
        )
    }

    pub(crate) fn start_staged_build_with_sources_and_cache(
        root: impl Into<PathBuf>,
        definitions: Vec<crate::repository::CollectionDefinition>,
        mut options: crate::repository::RepositoryOptions,
        sources: crate::source::SourceRegistry,
        ready_configuration: Option<StagedReadyConfiguration>,
    ) -> Result<StagedBuildTask, IndexError> {
        options.snapshot_mode = crate::repository::SnapshotMode::Disabled;
        options.inference.validate().map_err(IndexError::Build)?;
        crate::relation::validate_definitions(&definitions).map_err(IndexError::Build)?;
        let root = root.into();
        let task = StagedBuildTask::new();
        let opening = Arc::clone(&task.0);
        std::thread::Builder::new()
            .name("objekte-staged-build".into())
            .spawn(move || {
                let on_catalogue = |files: &crate::inventory::FileInventory| {
                    let catalogue = files
                        .files
                        .iter()
                        .map(|entry| StagedFile {
                            id: entry.id,
                            relative_path: entry.relative_path.clone(),
                            collections: entry.collections.clone(),
                            size: entry.revision.size,
                            created_ns: entry.created_ns,
                        })
                        .collect::<Vec<_>>();
                    if let Ok(mut status) = opening.status.lock() {
                        status.phase = StagedBuildPhase::BuildingBase;
                        status.discovered_files = catalogue.len();
                    }
                    if let Ok(mut files) = opening.catalogue.lock() {
                        *files = catalogue;
                    }
                    opening.ready.notify_all();
                };
                let outcome = (|| -> Result<(Index, bool), IndexError> {
                    let base = Repository::build_search_base(
                        root,
                        definitions,
                        options,
                        sources,
                        &on_catalogue,
                        &|| opening.cancelled.load(Ordering::Acquire),
                    )
                    .map_err(|error| {
                        if opening.cancelled.load(Ordering::Acquire) {
                            IndexError::Cancelled
                        } else {
                            IndexError::Build(error)
                        }
                    })?;
                    let requested = base.requests_extended();
                    let base = Arc::new(base);
                    let instance_id = crate::repository::next_repository_instance();
                    let published = PublishedIndex::from_repository(
                        instance_id,
                        IndexRevision::INITIAL,
                        Arc::clone(&base),
                    )?;
                    let index = Index::from_published(published);
                    *index.0.extended.lock().map_err(|_| IndexError::LockPoisoned)? =
                        Some(Arc::clone(&opening.extended));
                    if let Some(configuration) = &ready_configuration {
                        index.select_cache_destination(
                            &configuration.cache,
                            None,
                            configuration.protected_files.clone(),
                        )?;
                        if configuration.replace_invalid_cache {
                            index.discard_invalid_cache(&configuration.cache)?;
                        }
                        if configuration.sync_cache {
                            index.enable_sync_cache()?;
                        }
                    }
                    if let Ok(mut progress) = opening.extended.status.lock() {
                        progress.total_files = base.files().files.len();
                        progress.phase = if requested {
                            ExtendedPhase::Pending
                        } else {
                            ExtendedPhase::Unnecessary
                        };
                    }
                    if let Ok(mut status) = opening.status.lock() {
                        status.phase = StagedBuildPhase::SearchReady;
                        status.located_records = base
                            .collections()
                            .iter()
                            .map(|collection| collection.generation.records.len() as u64)
                            .sum();
                        status.data_revision = Some(IndexRevision::INITIAL);
                        status.capability_revision = Some(IndexRevision::INITIAL);
                    }
                    *opening.index.lock().map_err(|_| IndexError::LockPoisoned)? =
                        Some(index.clone());
                    opening.ready.notify_all();
                    if let Some(configuration) = &ready_configuration
                        && configuration.replace_invalid_cache
                    {
                        // Replacement persistence is outside the SearchReady
                        // critical path and uses the existing destination guard.
                        if let Err(error) = index.flush_cache(&configuration.cache) {
                            if let Ok(mut status) = opening.status.lock() {
                                status.error = Some(error.to_string());
                            }
                        }
                    }
                    if !requested {
                        opening.extended.finish(Ok(()));
                        if let Ok(mut status) = opening.status.lock() {
                            status.phase = StagedBuildPhase::Complete;
                        }
                        return Ok((index, false));
                    }
                    if opening.extended.cancelled.load(Ordering::Acquire) {
                        opening.extended.finish(Err(IndexError::Cancelled));
                        if let Ok(mut status) = opening.status.lock() {
                            status.phase = StagedBuildPhase::Cancelled;
                        }
                        return Ok((index, false));
                    }
                    opening.extended.set_phase(ExtendedPhase::Building);
                    if let Ok(mut status) = opening.status.lock() {
                        status.phase = StagedBuildPhase::BuildingExtended;
                    }
                    let expected = index.view()?.revision();
                    let extended = Repository::build_extended_from_base(
                        &base,
                        &|files, _records| {
                            if let Ok(mut status) = opening.extended.status.lock() {
                                status.completed_files = status.completed_files.saturating_add(files);
                            }
                        },
                        &|| opening.extended.cancelled.load(Ordering::Acquire),
                    )
                    .map_err(|error| {
                        if opening.extended.cancelled.load(Ordering::Acquire) {
                            IndexError::Cancelled
                        } else {
                            IndexError::Build(error)
                        }
                    });
                    let result = match extended {
                        Ok(repository) if opening.extended.cancelled.load(Ordering::Acquire) => {
                            drop(repository);
                            Err(IndexError::Cancelled)
                        }
                        Ok(repository) => index
                            .publish_extended_repository(expected, repository)
                            .map(|_| ()),
                        Err(error) => Err(error),
                    };
                    if let Ok(mut status) = opening.status.lock() {
                        status.phase = match &result {
                            Ok(()) => StagedBuildPhase::Complete,
                            Err(IndexError::Cancelled) => StagedBuildPhase::Cancelled,
                            Err(_) => StagedBuildPhase::Failed,
                        };
                        status.error = result.as_ref().err().map(ToString::to_string);
                    }
                    opening.extended.finish(result);
                    Ok((index, true))
                })();
                if let Err(error) = outcome {
                    if let Ok(mut state) = opening.error.lock() {
                        *state = Some(error.clone());
                    }
                    if let Ok(mut status) = opening.status.lock() {
                        status.phase = StagedBuildPhase::Failed;
                        status.error = Some(error.to_string());
                    }
                    opening.extended.finish(Err(error));
                    opening.ready.notify_all();
                }
            })
            .map_err(|error| IndexError::Build(error.to_string()))?;
        Ok(task)
    }

    /// Adopt a completed static repository as the first published view.
    /// This does not trigger discovery, reconciliation or cache persistence.
    pub fn from_repository(repository: Repository) -> Result<Self, IndexError> {
        let instance_id = crate::repository::next_repository_instance();
        let published = PublishedIndex::from_repository(
            instance_id,
            IndexRevision::INITIAL,
            Arc::new(repository),
        )?;
        let published = crate::lifecycle_partition::partition_base(published)?;
        Ok(Self::from_published(published))
    }

    pub(crate) fn from_published(published: PublishedIndex) -> Self {
        let instance_id = published.instance_id;
        let verified_cache_bytes = published.repository.options().verified_cache_bytes;
        Self(Arc::new(IndexInner {
            instance_id,
            restored_from_cache: AtomicBool::new(false),
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
            persistence: Mutex::new(PersistenceState {
                selected_cache: None,
                saved: HashMap::new(),
                protected_files: Vec::new(),
            }),
            sync_cache: Mutex::new(None),
            sync_cache_error: Mutex::new(None),
            extended: Mutex::new(None),
        }))
    }

    pub fn instance_id(&self) -> u64 {
        self.0.instance_id
    }

    pub fn extended_status(&self) -> Result<ExtendedStatus, IndexError> {
        if let Some(job) = self.0.extended.lock().map_err(|_| IndexError::LockPoisoned)?.as_ref() {
            return Ok(job.status.lock().map_err(|_| IndexError::LockPoisoned)?.clone());
        }
        let view = self.view()?;
        let phase = if view.repository().requests_extended() {
            ExtendedPhase::Complete
        } else {
            ExtendedPhase::Unnecessary
        };
        Ok(ExtendedStatus {
            phase,
            completed_files: view.0.catalog.active_files() as usize,
            total_files: view.0.catalog.active_files() as usize,
            error: None,
        })
    }

    pub fn wait_extended(&self) -> Result<(), IndexError> {
        let job = self.0.extended.lock().map_err(|_| IndexError::LockPoisoned)?.clone();
        job.map_or(Ok(()), |job| job.wait())
    }

    pub fn cancel_extended(&self) -> Result<(), IndexError> {
        if let Some(job) = self.0.extended.lock().map_err(|_| IndexError::LockPoisoned)?.as_ref() {
            job.cancelled.store(true, Ordering::Release);
        }
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

    pub fn restored_from_cache(&self) -> bool {
        self.0.restored_from_cache.load(Ordering::Acquire)
    }

    pub(crate) fn mark_restored_from_cache(&self) {
        self.0.restored_from_cache.store(true, Ordering::Release);
    }

    /// Acquire one immutable view; no publication lock is held by its queries.
    pub fn view(&self) -> Result<IndexView, IndexError> {
        let published = self
            .0
            .published
            .read()
            .map_err(|_| IndexError::LockPoisoned)?;
        Ok(IndexView(Arc::clone(&published), Arc::downgrade(&self.0)))
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
            let fresh = Self::build_with_sources(
                repository.files().root.clone(),
                definitions,
                repository.options(),
                repository.sources.clone(),
            )?;
            let next_revision = view.revision().next()?;
            let mut next = (*fresh.view()?.0).clone();
            next.instance_id = self.instance_id();
            next.revision = next_revision;
            next.file_versions =
                FileVersions::filled(next.catalog.allocated_files(), FileVersion(next_revision.0));
            next.saved_revision = view.0.saved_revision;
            next.saved_capability_revision = view.0.saved_capability_revision;
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

    /// Persist one pinned revision to an explicit destination. Snapshot
    /// writers are serialized by destination state, while queries and file
    /// publication remain free to continue on newer roots.
    pub fn flush_cache(&self, path: &Path) -> Result<IndexRevision, IndexError> {
        self.flush_cache_with_stats(path)
            .map(|(revision, _)| revision)
    }

    pub fn flush_cache_with_stats(
        &self,
        path: &Path,
    ) -> Result<(IndexRevision, crate::storage::SnapshotSaveStats), IndexError> {
        self.flush_cache_with_stats_select(path, true)
    }

    /// Persist an additional cache copy without selecting it as the target of
    /// later automatic writes when no target has been selected yet.
    pub fn flush_cache_copy_with_stats(
        &self,
        path: &Path,
    ) -> Result<(IndexRevision, crate::storage::SnapshotSaveStats), IndexError> {
        self.flush_cache_with_stats_select(path, false)
    }

    fn flush_cache_with_stats_select(
        &self,
        path: &Path,
        select_if_none: bool,
    ) -> Result<(IndexRevision, crate::storage::SnapshotSaveStats), IndexError> {
        let view = self.view()?;
        let destination = crate::lifecycle_cache_guard::canonical_cache_destination(path)
            .map_err(IndexError::InvalidCache)?;
        let mut persistence = self
            .0
            .persistence
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?;
        if let Some((saved, saved_capability)) = persistence.saved.get(&destination).copied()
            && saved >= view.revision()
            && saved_capability >= view.capability_revision()
        {
            if select_if_none && persistence.selected_cache.is_none() {
                persistence.selected_cache = Some(destination);
                drop(persistence);
                let mut published = self
                    .0
                    .published
                    .write()
                    .map_err(|_| IndexError::LockPoisoned)?;
                let mut next = (**published).clone();
                next.saved_revision = Some(saved);
                next.saved_capability_revision = Some(saved_capability);
                *published = Arc::new(next);
            }
            return Ok((saved, crate::storage::SnapshotSaveStats::default()));
        }
        let guard = crate::lifecycle_cache_guard::CacheDestinationGuard::new(
            &view.0,
            &persistence.protected_files,
        );
        guard
            .ensure_safe(&destination)
            .map_err(IndexError::InvalidCache)?;
        let stats = crate::storage::save_lifecycle_snapshot_with_stats(
            &view.0,
            &destination,
            crate::lifecycle_segment_codec::encode_segment,
            || guard.ensure_safe(&destination),
        )
        .map_err(IndexError::Io)?;
        persistence
            .saved
            .insert(destination.clone(), (view.revision(), view.capability_revision()));
        let selected = match &persistence.selected_cache {
            Some(selected) => selected == &destination,
            None if select_if_none => {
                persistence.selected_cache = Some(destination);
                true
            }
            None => false,
        };
        drop(persistence);
        if selected {
            let mut published = self
                .0
                .published
                .write()
                .map_err(|_| IndexError::LockPoisoned)?;
            if published.saved_revision.is_none_or(|saved| saved < view.revision())
                || published
                    .saved_capability_revision
                    .is_none_or(|saved| saved < view.capability_revision())
            {
                let mut next = (**published).clone();
                next.saved_revision = Some(view.revision());
                next.saved_capability_revision = Some(view.capability_revision());
                *published = Arc::new(next);
            }
        }
        Ok((view.revision(), stats))
    }

    pub(crate) fn select_cache_destination(
        &self,
        path: &Path,
        saved: Option<IndexRevision>,
        protected_files: Vec<PathBuf>,
    ) -> Result<(), IndexError> {
        let destination = crate::lifecycle_cache_guard::canonical_cache_destination(path)
            .map_err(IndexError::InvalidCache)?;
        let mut persistence = self
            .0
            .persistence
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?;
        persistence.selected_cache = Some(destination.clone());
        persistence.protected_files = protected_files;
        if let Some(revision) = saved {
            persistence.saved.insert(destination, (revision, IndexRevision::INITIAL));
        }
        Ok(())
    }

    /// Protect explicitly selected schema and configuration paths from cache
    /// writes. Register them before the first flush; this operation resolves
    /// relative paths against the current working directory once.
    pub fn protect_configuration_files(&self, paths: &[PathBuf]) -> Result<(), IndexError> {
        let mut resolved = Vec::with_capacity(paths.len());
        for path in paths {
            resolved.push(
                std::path::absolute(path)
                    .map_err(|error| IndexError::InvalidCache(error.to_string()))?,
            );
        }
        let mut persistence = self
            .0
            .persistence
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?;
        persistence.protected_files.extend(resolved);
        persistence.protected_files.sort();
        persistence.protected_files.dedup();
        Ok(())
    }

    /// Remove a rejected cache only after the rebuilt index has proved that
    /// the destination is neither indexed source data nor configuration.
    pub(crate) fn discard_invalid_cache(&self, path: &Path) -> Result<(), IndexError> {
        let view = self.view()?;
        let persistence = self
            .0
            .persistence
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?;
        let guard = crate::lifecycle_cache_guard::CacheDestinationGuard::new(
            &view.0,
            &persistence.protected_files,
        );
        guard.ensure_safe(path).map_err(IndexError::InvalidCache)?;
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|error| IndexError::Io(format!("{}: {error}", path.display())))?;
        if !metadata.file_type().is_file() {
            return Err(IndexError::InvalidCache(format!(
                "refusing to delete non-file cache: {}",
                path.display()
            )));
        }
        std::fs::remove_file(path)
            .map_err(|error| IndexError::Io(format!("{}: {error}", path.display())))
    }

    /// Enable one bounded, coalescing background cache writer for the selected
    /// repository destination. Mutation commits only send a nonblocking signal.
    /// Configure the repository's selected cache writer at runtime. Enabling
    /// requires a cache destination, as selected by repository opening or a
    /// previous explicit flush. Disabling stops future writes and joins the
    /// worker; callers must run this off interactive threads.
    pub fn set_sync_cache(&self, enabled: bool) -> Result<(), IndexError> {
        if enabled {
            self.enable_sync_cache()
        } else {
            self.stop_sync_cache()
        }
    }

    pub(crate) fn enable_sync_cache(&self) -> Result<(), IndexError> {
        if self.0.quiescing.load(Ordering::Acquire) {
            return Err(IndexError::Cancelled);
        }
        if self
            .0
            .persistence
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?
            .selected_cache
            .is_none()
        {
            return Err(IndexError::InvalidCache(
                "no cache destination selected".into(),
            ));
        }
        let mut sync = self
            .0
            .sync_cache
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?;
        if self.0.quiescing.load(Ordering::Acquire) {
            return Err(IndexError::Cancelled);
        }
        if sync.is_some() {
            return Ok(());
        }
        let (signal, receiver) = mpsc::sync_channel(1);
        let owner = Arc::downgrade(&self.0);
        let worker = std::thread::Builder::new()
            .name("objekte-sync-cache".into())
            .spawn(move || {
                while receiver.recv().is_ok() {
                    // A quiet window folds bursts into one snapshot, while
                    // a one-second ceiling prevents continuous writes from
                    // postponing persistence indefinitely.
                    let first_signal = Instant::now();
                    while first_signal.elapsed() < Duration::from_secs(1)
                        && receiver.recv_timeout(Duration::from_millis(100)).is_ok()
                    {
                    }
                    let Some(inner) = owner.upgrade() else { break };
                    // Closing owns the final persistence decision. A queued
                    // automatic write must not become an implicit save after
                    // the publication barrier has been established.
                    if inner.quiescing.load(Ordering::Acquire) {
                        break;
                    }
                    let index = Index(inner);
                    let destination = index
                        .0
                        .persistence
                        .lock()
                        .ok()
                        .and_then(|state| state.selected_cache.clone());
                    let result = destination
                        .map(|path| index.flush_cache(&path).map(|_| ()))
                        .unwrap_or_else(|| {
                            Err(IndexError::InvalidCache(
                                "no cache destination selected".into(),
                            ))
                        });
                    if let Ok(mut error) = index.0.sync_cache_error.lock() {
                        *error = result.err().map(|error| error.to_string());
                    }
                }
            })
            .map_err(|error| IndexError::Io(error.to_string()))?;
        *sync = Some(SyncCacheWorker { signal, worker });
        drop(sync);
        self.notify_sync_cache();
        Ok(())
    }

    fn notify_sync_cache(&self) {
        if let Ok(sync) = self.0.sync_cache.lock() {
            if let Some(sync) = sync.as_ref() {
                let _ = sync.signal.try_send(());
            }
        }
    }

    pub fn sync_cache_error(&self) -> Result<Option<String>, IndexError> {
        Ok(self
            .0
            .sync_cache_error
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?
            .clone())
    }

    pub fn stop_sync_cache(&self) -> Result<(), IndexError> {
        let sync = self
            .0
            .sync_cache
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?
            .take();
        if let Some(sync) = sync {
            drop(sync.signal);
            sync.worker
                .join()
                .map_err(|_| IndexError::InvalidPublication("cache writer panicked".into()))?;
        }
        Ok(())
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
    /// final cache save. This call may be retried after a save failure.
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
        self.stop_sync_cache()?;
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
        let cache_paths = {
            let persistence = self.0.persistence.lock();
            match persistence {
                Ok(persistence) => persistence
                    .saved
                    .keys()
                    .filter_map(|destination| {
                        destination
                            .strip_prefix(root)
                            .ok()
                            .map(|relative| relative.to_owned())
                    })
                    .collect::<Vec<_>>(),
                Err(_) => {
                    report.record_issue("cache destination state is unavailable".into());
                    report.duration = started.elapsed();
                    on_progress(&report);
                    return report;
                }
            }
        };
        let excluded = |path: &Path| {
            !cache_paths.iter().any(|cache| {
                path == cache
                    || (path.parent() == cache.parent()
                        && path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| {
                                name.starts_with(".objekte-snapshot-") && name.ends_with(".tmp")
                            }))
            })
        };
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
            |path| !excluded(path),
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
                if !excluded(&stamp.path) {
                    continue;
                }
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
        next.revision = current.revision().next()?;
        Ok((
            current.revision(),
            next,
            retired_records,
            affected_collections,
        ))
    }

    /// Install a fully prepared root only if its captured predecessor is
    /// current. The retired Arc is dropped after releasing the write lock.
    /// No filesystem work or index construction belongs inside this method.
    pub(crate) fn publish_prepared(
        &self,
        expected: IndexRevision,
        mut next: PublishedIndex,
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
        let retired = {
            let mut guard = self
                .0
                .published
                .write()
                .map_err(|_| IndexError::LockPoisoned)?;
            if self.0.quiescing.load(Ordering::Acquire) {
                return Err(IndexError::Cancelled);
            }
            if guard.revision != expected {
                return Err(IndexError::RevisionConflict {
                    expected,
                    actual: guard.revision,
                });
            }
            next.capability_revision = guard.capability_revision.next()?;
            std::mem::replace(&mut *guard, Arc::new(next))
        };
        drop(retired);
        self.notify_sync_cache();
        Ok(next_revision)
    }

    /// Publish optional capabilities for an unchanged data generation. Query
    /// membership, continuations, and verified-result cache keys retain their
    /// semantic revision; new views see the new physical capability revision.
    pub(crate) fn publish_extended_repository(
        &self,
        expected_data: IndexRevision,
        repository: Repository,
    ) -> Result<IndexRevision, IndexError> {
        let repository = Arc::new(repository);
        let current = self.view()?;
        if current.revision() != expected_data {
            return Err(IndexError::RevisionConflict {
                expected: expected_data,
                actual: current.revision(),
            });
        }
        let catalog = FileCatalog::restored(
            Arc::clone(&repository),
            current.0.catalog.snapshot_slots(),
        )
        .map_err(IndexError::InvalidPublication)?;
        let retired = {
            let mut guard = self.0.published.write().map_err(|_| IndexError::LockPoisoned)?;
            if self.0.quiescing.load(Ordering::Acquire) {
                return Err(IndexError::Cancelled);
            }
            if guard.revision != expected_data {
                return Err(IndexError::RevisionConflict {
                    expected: expected_data,
                    actual: guard.revision,
                });
            }
            if guard.capability_revision != current.capability_revision() {
                return Err(IndexError::RevisionConflict {
                    expected: current.capability_revision(),
                    actual: guard.capability_revision,
                });
            }
            let mut next = (**guard).clone();
            if next.configuration_identity
                != crate::storage::configuration_hash(
                    &repository
                        .collections()
                        .iter()
                        .map(|collection| collection.definition.clone())
                        .collect::<Vec<_>>(),
                    repository.options(),
                    &repository.sources,
                )
                .map_err(IndexError::IncompatibleConfiguration)?
            {
                return Err(IndexError::IncompatibleConfiguration(
                    "extended configuration changed".into(),
                ));
            }
            next.repository = repository;
            next.catalog = catalog;
            next.capability_revision = next.capability_revision.next()?;
            std::mem::replace(&mut *guard, Arc::new(next))
        };
        drop(retired);
        self.notify_sync_cache();
        Ok(self.view()?.capability_revision())
    }
}


/// Pinned immutable read context for a single publication revision.
#[derive(Clone)]
pub struct IndexView(pub(crate) Arc<PublishedIndex>, Weak<IndexInner>);


impl IndexView {
    fn has_segments(&self) -> bool {
        self.0.segments.iter().any(|group| !group.is_empty())
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

    pub fn capability_revision(&self) -> IndexRevision {
        self.0.capability_revision
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

    /// Paths of files active in this immutable publication.
    pub fn file_paths(&self) -> impl Iterator<Item = &Path> + '_ {
        (0..self.0.catalog.allocated_files()).filter_map(|number| {
            self.0
                .catalog
                .entry(FileId(number as u32))
                .map(|entry| entry.relative_path.as_path())
        })
    }

    /// Paths of active files indexed for one collection in this publication.
    pub fn file_paths_for_collection(&self, name: &str) -> impl Iterator<Item = &Path> + '_ {
        let collection_id = self
            .0
            .repository
            .collections()
            .iter()
            .find(|collection| collection.definition.name == name)
            .map(|collection| collection.generation.collection);
        (0..self.0.catalog.allocated_files()).filter_map(move |number| {
            let entry = self.0.catalog.entry(FileId(number as u32))?;
            collection_id
                .filter(|id| entry.collections.contains(id))
                .map(|_| entry.relative_path.as_path())
        })
    }

    pub fn file_version(&self, file: FileId) -> Option<FileVersion> {
        self.0
            .catalog
            .is_active(file)
            .then(|| self.0.file_versions.get(file))?
    }

    pub(crate) fn file_version_including_inactive(&self, file: FileId) -> Option<FileVersion> {
        self.0.file_versions.get(file)
    }

    pub(crate) fn file_entry(&self, file: FileId) -> Option<&FileEntry> {
        self.0.catalog.entry(file)
    }

    pub fn diagnostic(
        &self,
        file: FileId,
        reference: crate::inventory::DiagnosticRef,
    ) -> Option<&crate::inventory::SourceDiagnostic> {
        self.0
            .catalog
            .entry(file)?
            .diagnostics
            .get(reference.stored_index()?)
    }

    pub(crate) fn file_fingerprint(&self, file: FileId) -> Option<[u8; 32]> {
        self.0.catalog.fingerprint(file)
    }

    pub(crate) fn file_configuration(&self, file: FileId) -> Option<[u8; 32]> {
        self.0.catalog.configuration(file)
    }

    pub(crate) fn file_record_count(&self, file: FileId) -> Option<u64> {
        let entry = self.file_entry(file)?;
        let mut count = 0;
        for collection_id in &entry.collections {
            let number = collection_id.0 as usize;
            let collection = self.repository().collections().get(number)?;
            if let Some(slots) = collection.generation.file_records.records_for(file) {
                count += slots
                    .iter()
                    .filter(|slot| self.0.active[number].contains(**slot))
                    .count() as u64;
            }
            for segment in self.0.segments.get(number)?.iter() {
                if let Some(slots) = segment.index.file_records(file) {
                    count += slots
                        .iter()
                        .filter(|slot| segment.active.contains(**slot))
                        .count() as u64;
                }
            }
        }
        Some(count)
    }

    #[cfg(test)]
    pub(crate) fn is_active(&self, collection: CollectionId, slot: RecordSlot) -> bool {
        self.0
            .active
            .get(collection.0 as usize)
            .is_some_and(|active| active.contains(slot))
    }

    #[cfg(test)]
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
        let owner = self.1.upgrade();
        let cache = owner.as_ref().map(|owner| crate::run::LifecycleCacheScope {
            store: &owner.verified_cache,
            instance_id: self.instance_id(),
        });
        let result = if !self.has_segments() {
            crate::run::search_active_available_lifecycle(
                self.repository(),
                resolved.as_ref(),
                &self.0.active,
                &self.0.catalog,
                self.revision(),
                cache,
            )
            .map(|result| self.stamp_result(result))
        } else {
            crate::run::search_segmented_lifecycle(
                self.repository(),
                resolved.as_ref(),
                &self.0.active,
                &self.0.segments,
                &self.0.catalog,
                self.revision(),
                cache,
            )
        };
        result
    }

    /// Bind typed parameters before querying this pinned publication.
    pub fn search_with_parameters(
        &self,
        query: &Query,
        parameters: &HashMap<String, Value>,
    ) -> Result<SearchResult, String> {
        self.search(&query.with_parameters(parameters)?)
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
        let owner = self.1.upgrade();
        let cache = owner.as_ref().map(|owner| crate::run::LifecycleCacheScope {
            store: &owner.verified_cache,
            instance_id: self.instance_id(),
        });
        let result = if !self.has_segments() {
            crate::run::search_active_available_with_context_lifecycle(
                self.repository(),
                &resolved,
                &self.0.active,
                &self.0.catalog,
                self.revision(),
                context,
                cache,
            )
            .map(|result| self.stamp_result(result))
        } else {
            crate::run::search_segmented_with_context_lifecycle(
                self.repository(),
                &resolved,
                &self.0.active,
                &self.0.segments,
                &self.0.catalog,
                self.revision(),
                context,
                cache,
            )
        };
        result
    }

    /// Search only when every required source was available to this execution.
    pub fn search_strict(&self, query: &Query) -> Result<SearchResult, String> {
        let result = self.search(query)?;
        if !result.counters.match_count_complete {
            return Err("search source data is incomplete".into());
        }
        Ok(result)
    }

    /// Context-aware strict search with a typed source-change failure.
    pub fn search_strict_with_context(
        &self,
        query: &Query,
        context: &ExecutionContext,
    ) -> Result<SearchResult, ExecutionError> {
        let result = self.search_with_context(query, context)?;
        if !result.counters.match_count_complete {
            return Err(context.error(
                crate::streaming::ExecutionErrorKind::SourceChanged,
                "search source data is incomplete",
            ));
        }
        Ok(result)
    }

    pub fn count_exact(&self, query: &Query) -> Result<usize, String> {
        let mut request = query.clone();
        request.options.count = true;
        request.options.limit = 0;
        request.options.offset = 0;
        self.search_strict(&request)?
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
        self.search_strict_with_context(&request, context)?
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
        match self.get_record_available(collection, record_id, return_mode)? {
            crate::run::DirectLookupResult::Found(hit) => Ok(Some(hit)),
            crate::run::DirectLookupResult::NotFound => Ok(None),
            crate::run::DirectLookupResult::SourceUnavailable(diagnostic) => {
                Err(diagnostic.message)
            }
        }
    }

    /// Direct lookup with a distinct unavailable-source outcome. Explicit
    /// lifecycle operations own source reconciliation.
    pub fn get_record_available(
        &self,
        collection: &str,
        record_id: &str,
        return_mode: ReturnMode,
    ) -> Result<crate::run::DirectLookupResult, String> {
        let result = crate::run::direct_lookup_segmented(
            self.repository(),
            collection,
            record_id,
            return_mode,
            &self.0.active,
            &self.0.segments,
            &self.0.catalog,
            self.revision(),
        )?;
        Ok(result)
    }

    pub fn search_reference(&self, query: &Query) -> Result<SearchResult, String> {
        if self.0.segments.iter().any(|segments| !segments.is_empty()) {
            return crate::run::search_segmented_reference(
                self.repository(),
                query,
                &self.0.active,
                &self.0.segments,
                &self.0.catalog,
                self.revision(),
            );
        }
        self.repository()
            .search_reference_with_active(query, Some(&self.0.active))
            .map(|result| self.stamp_result(result))
    }

    pub fn select_exact(
        &self,
        collection: &str,
        query: &Query,
    ) -> Result<crate::posting_values::ExactSelection, String> {
        if self.has_segments() {
            return crate::posting_values::select_exact_segmented(
                self.repository(),
                collection,
                query,
                self.revision(),
                &self.0.active,
                &self.0.segments,
                &self.0.catalog,
            );
        }
        self.repository()
            .select_exact_for_view(collection, query, self.revision(), &self.0.active)
    }

    pub fn posting_values(
        &self,
        request: &crate::posting_values::PostingValuesRequest,
    ) -> Result<crate::posting_values::PostingValuesPage, String> {
        if self.has_segments() {
            return crate::posting_values::posting_values_segmented(
                self.repository(),
                request,
                self.revision(),
                &self.0.active,
                &self.0.segments,
                &self.0.catalog,
            );
        }
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
        if self.has_segments() {
            return crate::relation_inspection::relation_outgoing_page_segmented(
                self.repository(),
                source_collection,
                relation_path,
                source_id,
                offset,
                limit,
                self.revision(),
                &self.0.active,
                &self.0.segments,
                &self.0.catalog,
            );
        }
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
        if self.has_segments() {
            return crate::relation_inspection::relation_backlink_page_segmented(
                self.repository(),
                target_collection,
                reverse_alias,
                target_id,
                offset,
                limit,
                self.revision(),
                &self.0.active,
                &self.0.segments,
                &self.0.catalog,
            );
        }
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
        if self.has_segments() {
            return crate::relation_inspection::collection_graph_segmented(
                self.repository(),
                self.revision(),
                &self.0.active,
                &self.0.segments,
                &self.0.catalog,
            );
        }
        self.repository().collection_graph_for_view(&self.0.active)
    }

    pub fn search_analyze(
        &self,
        query: &Query,
    ) -> Result<(SearchResult, Vec<crate::run::QueryPlanTrace>), String> {
        let resolved = query.resolve_current_clock()?;
        if self.has_segments() {
            return crate::run::search_analyze_segmented(
                self.repository(),
                resolved.as_ref(),
                &self.0.active,
                &self.0.segments,
                &self.0.catalog,
                self.revision(),
            );
        }
        crate::run::search_analyze_with_active(
            self.repository(),
            resolved.as_ref(),
            Some(&self.0.active),
        )
        .map(|(result, trace)| (self.stamp_result(result), trace))
    }

    pub fn explain(&self, query: &Query) -> Result<Vec<crate::run::QueryPlanTrace>, String> {
        let resolved = query.resolve_current_clock()?;
        if self.has_segments() {
            return crate::run::explain_segmented(
                self.repository(),
                resolved.as_ref(),
                &self.0.active,
                &self.0.segments,
                self.revision(),
            );
        }
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
        let owner = self.1.upgrade();
        let cache = owner.as_ref().map(|owner| crate::run::LifecycleCacheScope {
            store: &owner.verified_cache,
            instance_id: self.instance_id(),
        });
        let result = crate::run::search_stream_segmented_with_context_lifecycle(
            self.repository(),
            query,
            scope,
            options,
            context,
            &self.0.active,
            &self.0.segments,
            &self.0.catalog,
            self.revision(),
            sink,
            cache,
        );
        result
    }
}


#[cfg(test)]
#[path = "lifecycle.test.rs"]
mod tests;
