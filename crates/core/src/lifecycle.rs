//! Pinned, revisioned publication boundary for a changing index.
//!
//! This module establishes immutable read views and typed identity. File
//! mutation, reconciliation and persistence are added in later lifecycle
//! phases; a wrapped static repository does not imply those operations exist.

use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, RwLock, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};

use crate::build::{
    FilePreparationBinding, IndexedCollectionSegment, PreparedCollectionBuild, PreparedFileBuild,
    SegmentAdmission, SourceFileInput, prepare_file_build,
};
use crate::discovery::selects_file;
use crate::inventory::{ActiveRecords, FileEntry, FileStatus, FileVersions};
use crate::lifecycle_catalog::{FileCatalog, OwnershipRoute};
pub use crate::lifecycle_compaction::{
    CompactionMode, CompactionOptions, CompactionReport,
};
use crate::model::{CollectionId, FileId, Value};
pub(crate) use crate::native_segment::PublishedSegment;
use crate::native_segment::SegmentId;
use crate::query::Query;
use crate::repository::{Repository, SearchHit, SearchResult};
use crate::streaming::{
    CancellationToken, ExecutionContext, ExecutionError, ResultScope, StreamControl, StreamError,
    StreamEvent, StreamOptions, StreamOutcome,
};
use crate::workload::ReturnMode;

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
    pub(crate) segment_positions: Arc<Vec<HashMap<SegmentId, usize>>>,
    pub(crate) next_segment_ordinal: u64,
    /// Hysteresis for byte-triggered coalescing; count and retirement remain
    /// independent triggers and may run sooner.
    pub(crate) last_size_maintenance_revision: u64,
    pub(crate) catalog: FileCatalog,
    pub(crate) file_versions: FileVersions,
    pub(crate) reconciliation: ReconciliationState,
    pub(crate) processed_files: usize,
    /// Only populated during an initial build. One bitmap per collection
    /// prevents a shared physical file from being counted too early.
    pub(crate) processed_memberships: Vec<Arc<RoaringBitmap>>,
    /// True when the prepared mutation reused and rebuilt its smallest
    /// eligible destination segment.
    pub(crate) destination_rebuilt: bool,
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
                let mut total = posting.values.archive_bytes();
                let _ = posting.values.for_each(|_, bitmap| {
                    total = total.saturating_add(bitmap.serialized_size() as usize);
                    Ok(())
                });
                let _ = posting.lengths.for_each(|_, bitmap| {
                    total = total.saturating_add(bitmap.serialized_size() as usize);
                    Ok(())
                });
                total
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

fn cheapest_consolidation_pair(
    group: &[Arc<PublishedSegment>],
    target_records: usize,
) -> Option<(usize, usize)> {
    let mut sized = group
        .iter()
        .enumerate()
        .map(|(index, segment)| (segment.index.record_count(), index))
        .collect::<Vec<_>>();
    sized.sort_unstable();
    for (position, &(left_size, left)) in sized.iter().enumerate() {
        for &(right_size, right) in &sized[position + 1..] {
            if left_size + right_size > target_records {
                break;
            }
            let left_index = &group[left].index;
            let right_index = &group[right].index;
            let mut overlaps = false;
            for file_number in 0..left_index.file_count() {
                let file_id = left_index.files.get(file_number)?.entry.id;
                if right_index.file_index.get(file_id).is_some() {
                    overlaps = true;
                    break;
                }
            }
            if !overlaps {
                for &slot in &left_index.identity_order {
                    let id = left_index.record(slot)?.id.0.as_ref();
                    if right_index.id_lookup.get(id).is_some() {
                        overlaps = true;
                        break;
                    }
                }
            }
            if overlaps {
                continue;
            }
            return Some((left.min(right), left.max(right)));
        }
    }
    None
}

pub(crate) fn initial_ownership_routes(
    repository: &Repository,
    active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    file_count: usize,
) -> Vec<Vec<OwnershipRoute>> {
    let mut routes = vec![Vec::new(); file_count];
    for (number, collection) in repository.collections().iter().enumerate() {
        for (ordinal, file) in collection.generation.files.iter().enumerate() {
            if active
                .get(number)
                .is_none_or(|active| active.slot_count() == 0)
                || collection
                    .generation
                    .file_records
                    .records_for(*file)
                    .is_none_or(|slots| {
                        !slots
                            .iter()
                            .any(|slot| active[number].contains(*slot))
                    })
            {
                continue;
            }
            if let Some(file_routes) = routes.get_mut(file.0 as usize) {
                file_routes.push(OwnershipRoute {
                    collection: CollectionId(number as u32),
                    segment: SegmentId::new(0),
                    file_row_ordinal: ordinal as u32,
                });
            }
        }
    }
    for group in segments {
        for segment in group.iter() {
            for ordinal in 0..segment.index.file_count() {
                let Some(file) = segment.index.files.file_id(ordinal) else {
                    continue;
                };
                if let Some(file_routes) = routes.get_mut(file.0 as usize) {
                    file_routes.push(OwnershipRoute {
                        collection: segment.index.collection,
                        segment: segment.id,
                        file_row_ordinal: ordinal as u32,
                    });
                }
            }
        }
    }
    routes
}

fn append_segment_ownership(
    segment: &PublishedSegment,
    routes: &mut Vec<(FileId, OwnershipRoute)>,
) {
    routes.extend((0..segment.index.file_count()).filter_map(|ordinal| {
        segment.index.files.file_id(ordinal).map(|file| {
            (
                file,
                OwnershipRoute {
                    collection: segment.index.collection,
                    segment: segment.id,
                    file_row_ordinal: ordinal as u32,
                },
            )
        })
    }));
}

fn removed_segment_ownership(
    segment: &PublishedSegment,
    versions: &FileVersions,
) -> Vec<(FileId, FileVersion, OwnershipRoute)> {
    (0..segment.index.file_count())
        .filter_map(|ordinal| {
            let file = segment.index.files.file_id(ordinal)?;
            let version = versions.get(file)?;
            Some((
                file,
                version,
                OwnershipRoute {
                    collection: segment.index.collection,
                    segment: segment.id,
                    file_row_ordinal: ordinal as u32,
                },
            ))
        })
        .collect()
}

impl PublishedIndex {
    fn segment_position_directory(
        segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    ) -> Arc<Vec<HashMap<SegmentId, usize>>> {
        Arc::new(
            segments
                .iter()
                .map(|group| {
                    group
                        .iter()
                        .enumerate()
                        .map(|(position, segment)| (segment.id, position))
                        .collect()
                })
                .collect(),
        )
    }

    pub(crate) fn rebuild_segment_position(&mut self, collection: usize) {
        let directory = Arc::make_mut(&mut self.segment_positions);
        directory[collection] = self.segments[collection]
            .iter()
            .enumerate()
            .map(|(position, segment)| (segment.id, position))
            .collect();
    }

    pub(crate) fn rebuild_all_segment_positions(&mut self) {
        self.segment_positions = Self::segment_position_directory(&self.segments);
    }

    #[cfg(test)]
    pub(crate) fn validate_ownership(&self) -> Result<(), String> {
        let mut routed = std::collections::HashSet::new();
        for file in self.catalog.active_ids() {
            let version = self
                .file_versions
                .get(file)
                .ok_or("active ownership file has no version")?;
            let routes = self.catalog.ownership_routes(file, version)?;
            if routes.is_empty() {
                return Err("active file has no ownership route".into());
            }
            for route in routes {
                if route.segment.get() == 0 {
                    let collection = self
                        .repository
                        .collections()
                        .get(route.collection.0 as usize)
                        .ok_or("base route collection missing")?;
                    let row_file = collection
                        .generation
                        .files
                        .get(route.file_row_ordinal as usize)
                        .ok_or("base route file row missing")?;
                    if *row_file != file {
                        return Err("base route resolves to the wrong file".into());
                    }
                    let slots = collection
                        .generation
                        .file_records
                        .records_for(file)
                        .ok_or("base route slot list missing")?;
                    for slot in slots {
                        if self.active[route.collection.0 as usize].contains(*slot)
                            && !routed.insert((
                                route.collection,
                                SegmentId::new(0),
                                slot.0,
                            ))
                        {
                            return Err("record has duplicate base ownership".into());
                        }
                    }
                    continue;
                }
                let segment = self.segments[route.collection.0 as usize]
                    .iter()
                    .find(|segment| segment.id == route.segment)
                    .ok_or("segment route target missing")?;
                if segment.index.collection != route.collection {
                    return Err("segment route collection mismatch".into());
                }
                if segment.index.files.file_id(route.file_row_ordinal as usize) != Some(file) {
                    return Err("segment route resolves to the wrong file row".into());
                }
                let slots = segment
                    .index
                    .files
                    .archived_record_slots(route.file_row_ordinal as usize)
                    .ok_or("segment route slot list missing")?;
                for slot in slots {
                    if segment.active.contains(slot)
                        && !routed.insert((route.collection, segment.id, slot.0))
                    {
                        return Err("record has duplicate segment ownership".into());
                    }
                }
            }
        }
        for (number, active) in self.active.iter().enumerate() {
            for slot in 0..active.slot_count() {
                if active.contains(crate::model::RecordSlot(slot))
                    && !routed.contains(&(
                        CollectionId(number as u32),
                        SegmentId::new(0),
                        slot,
                    ))
                {
                    return Err("active base record is not routed".into());
                }
            }
        }
        for (number, segments) in self.segments.iter().enumerate() {
            for segment in segments.iter() {
                for slot in 0..segment.active.slot_count() {
                    if segment.active.contains(crate::model::RecordSlot(slot))
                        && !routed.contains(&(
                            CollectionId(number as u32),
                            segment.id,
                            slot,
                        ))
                    {
                        return Err("active segment record is not routed".into());
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) fn archive_bytes(&self) -> usize {
        let mut owners = std::collections::HashSet::new();
        self.segments
            .iter()
            .flat_map(|group| group.iter())
            .filter(|segment| owners.insert(Arc::as_ptr(&segment.index) as usize))
            .map(|segment| segment.index.archive_bytes())
            .sum::<usize>()
            .saturating_add(self.catalog.archive_bytes())
    }

    pub(crate) fn from_repository_owned(
        instance_id: u64,
        revision: IndexRevision,
        mut repository: Repository,
    ) -> Result<Self, IndexError> {
        let active = std::mem::take(&mut repository.active);
        let segments = std::mem::take(&mut repository.segments);
        Self::from_repository_parts(
            instance_id,
            revision,
            Arc::new(repository),
            active,
            segments,
        )
    }

    pub(crate) fn from_repository(
        instance_id: u64,
        revision: IndexRevision,
        repository: Arc<Repository>,
    ) -> Result<Self, IndexError> {
        Self::from_repository_parts(
            instance_id,
            revision,
            Arc::clone(&repository),
            repository.active.clone(),
            repository.segments.clone(),
        )
    }

    fn from_repository_parts(
        instance_id: u64,
        revision: IndexRevision,
        repository: Arc<Repository>,
        active: Vec<ActiveRecords>,
        segments: Vec<Arc<Vec<Arc<PublishedSegment>>>>,
    ) -> Result<Self, IndexError> {
        let definitions = repository
            .collections()
            .iter()
            .map(|collection| collection.definition.clone())
            .collect::<Vec<_>>();
        let configuration_identity = crate::configuration::configuration_hash(
            &definitions,
            repository.options(),
            &repository.sources,
        )
        .map_err(IndexError::IncompatibleConfiguration)?;
        let file_versions =
            FileVersions::filled(repository.files().files.len(), FileVersion(revision.0));
        let mut catalog = FileCatalog::from_parts(
            Arc::clone(&repository.files),
            repository.base_fingerprints.clone(),
            configuration_identity,
        );
        catalog
            .replace_all_ownership(
                &file_versions,
                &initial_ownership_routes(
                    &repository,
                    &active,
                    &segments,
                    file_versions.len(),
                ),
            )
            .map_err(IndexError::InvalidPublication)?;
        let processed_files = catalog.active_files() as usize;
        let next_segment_ordinal = segments
            .iter()
            .flat_map(|group| group.iter())
            .map(|segment| segment.id.get())
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| IndexError::ResourceLimit("segment ordinal overflow".into()))?;
        Ok(Self {
            instance_id,
            revision,
            repository,
            configuration_identity,
            active,
            segment_positions: Self::segment_position_directory(&segments),
            segments,
            next_segment_ordinal,
            last_size_maintenance_revision: 0,
            catalog,
            file_versions,
            reconciliation: ReconciliationState::NotStarted,
            processed_files,
            processed_memberships: Vec::new(),
            destination_rebuilt: false,
        })
    }

    pub(crate) fn begin_initial_construction(&mut self) {
        self.catalog.set_construction_complete(false);
        self.processed_files = 0;
        self.processed_memberships = (0..self.repository.collections().len())
            .map(|_| Arc::new(RoaringBitmap::new()))
            .collect();
    }

    fn status(&self) -> IndexStatus {
        debug_assert_eq!(self.catalog.allocated_files(), self.file_versions.len());
        IndexStatus {
            instance_id: self.instance_id,
            revision: self.revision,
            reconciliation: self.reconciliation,
            indexed_files: self.catalog.active_files() as usize,
            processed_files: self.processed_files,
            initial_construction_complete: self.catalog.construction_complete(),
            build_parses: self.repository.build_parses(),
            build_source_reads: self.repository.startup().build_source_reads,
            build_source_bytes_read: self.repository.startup().build_source_bytes_read,
            fallback_records: self
                .repository
                .collections()
                .iter()
                .enumerate()
                .map(|(number, collection)| {
                    let base = collection
                        .generation
                        .records
                        .iter()
                        .filter(|record| {
                            record.class == crate::inventory::RecordClass::RawFallback
                                && self.active[number].contains(record.slot)
                        })
                        .count() as u64;
                    let segments = self.segments[number]
                        .iter()
                        .map(|segment| {
                            (&segment.active.bitmap() - &segment.index.logical_records).len()
                        })
                        .sum::<u64>();
                    base + segments
                })
                .sum(),
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
        self,
        index: IndexedCollectionSegment,
    ) -> Result<Self, IndexError> {
        self.with_segment_mode(index, true, true)
    }

    fn with_segment_uncoalesced(
        self,
        index: IndexedCollectionSegment,
    ) -> Result<Self, IndexError> {
        self.with_segment_mode(index, false, false)
    }

    fn with_segment_mode(
        mut self,
        mut index: IndexedCollectionSegment,
        coalesce: bool,
        publish_ownership: bool,
    ) -> Result<Self, IndexError> {
        let number = index.collection.0 as usize;
        if number >= self.segments.len() {
            return Err(IndexError::InvalidPublication(
                "segment collection missing".into(),
            ));
        }
        if coalesce {
            self.make_room_for_segment(number)?;
        }
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
            if self.catalog.entry_owned(file.entry.id).is_none_or(|current| {
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
        let mut duplicate = None;
        index.id_lookup.for_each_id(|id| {
            if duplicate.is_some() {
                return;
            }
            let existing_duplicate = self.segments[number].iter().any(|segment| {
                segment
                    .index
                    .id_lookup
                    .get(id)
                    .is_some_and(|slot| segment.active.contains(slot))
            });
            let collection_duplicate = collection
                .id_lookup
                .get(&crate::model::RecordId(id.into()))
                .is_some_and(|slot| self.active[number].contains(*slot));
            if collection_duplicate || existing_duplicate {
                duplicate = Some(id.to_owned());
            }
        });
        if let Some(record_id) = duplicate {
            return Err(IndexError::DuplicateRecordId {
                collection: collection.definition.name.clone(),
                record_id,
            });
        }
        if !self.catalog.construction_complete() {
            for file in &index.files {
                let members = self.processed_memberships.get_mut(number).ok_or_else(|| {
                    IndexError::InvalidPublication("processed membership missing".into())
                })?;
                if Arc::make_mut(members).insert(file.entry.id.0) {
                    let catalogue_entry = self.catalog.entry_owned(file.entry.id).ok_or_else(|| {
                        IndexError::InvalidPublication("processed file missing".into())
                    })?;
                    if catalogue_entry.collections.iter().all(|collection| {
                        self.processed_memberships
                            .get(collection.0 as usize)
                            .is_some_and(|covered| covered.contains(file.entry.id.0))
                    }) {
                        self.processed_files += 1;
                    }
                }
            }
        }
        let active = ActiveRecords::all(index.records.len()).map_err(IndexError::ResourceLimit)?;
        let mut segment_id = index.id;
        if segment_id.get() == 1
            || self.segments[number]
                .iter()
                .any(|segment| segment.id == segment_id)
        {
            segment_id = SegmentId::new(self.next_segment_ordinal);
        }
        self.next_segment_ordinal = self.next_segment_ordinal.max(
            segment_id
                .get()
                .checked_add(1)
                .ok_or_else(|| IndexError::ResourceLimit("segment identity overflow".into()))?,
        );
        index.id = segment_id;
        let added_routes = (0..index.file_count())
            .filter_map(|ordinal| {
                index.files.file_id(ordinal).map(|file| {
                    (
                        file,
                        OwnershipRoute {
                            collection: index.collection,
                            segment: segment_id,
                            file_row_ordinal: ordinal as u32,
                        },
                    )
                })
            })
            .collect::<Vec<_>>();
        if publish_ownership {
            self.catalog
                .update_ownership(&self.file_versions, &[], &added_routes)
                .map_err(IndexError::InvalidPublication)?;
        }
        let published = PublishedSegment::checked(segment_id, Arc::new(index), active)
            .map_err(IndexError::InvalidPublication)?;
        Arc::make_mut(&mut self.segments[number]).push(Arc::new(published));
        self.rebuild_segment_position(number);
        if self.next_segment_ordinal == 0 {
            self.next_segment_ordinal = 1;
        }
        Ok(self)
    }

    /// Place newly prepared file contributions. For each affected collection
    /// at most one existing destination is rebuilt: the live-lightest segment
    /// that can hold the first incoming shard under the configured policy.
    pub(crate) fn place_mutation_segments(
        mut self,
        indexed: Vec<IndexedCollectionSegment>,
    ) -> Result<Self, IndexError> {
        let mut attempted = std::collections::HashSet::new();
        let mut pending = std::collections::VecDeque::from(indexed);
        let mut ownership_removed = Vec::new();
        let mut ownership_added = Vec::new();
        while let Some(incoming) = pending.pop_front() {
            let number = incoming.collection.0 as usize;
            let policy = self.repository.options().segment_policy;
            if incoming.record_count() > policy.target_records.get() {
                let active = ActiveRecords::all(incoming.record_count())
                    .map_err(IndexError::ResourceLimit)?;
                let split = crate::lifecycle_partition::compact_source_parts_with_policy(
                    &incoming,
                    &active,
                    &self.catalog,
                    policy,
                    || false,
                )?;
                if split.len() > 1 {
                    pending.extend(split);
                    continue;
                }
            }
            self = self.with_segment_uncoalesced(incoming)?;
            if !attempted.insert(number) {
                if let Some(incoming) = self.segments[number].last() {
                    append_segment_ownership(incoming, &mut ownership_added);
                }
                continue;
            }
            let group = &self.segments[number];
            let Some(incoming) = group.last() else {
                continue;
            };
            let candidate = group[..group.len().saturating_sub(1)]
                .iter()
                .enumerate()
                .filter_map(|(position, segment)| {
                    let live_records = segment.active.len() as usize;
                    if live_records == 0 {
                        return None;
                    }
                    let live_files = segment
                        .index
                        .files
                        .iter()
                        .filter(|file| {
                            file.records
                                .iter()
                                .any(|slot| segment.active.contains(*slot))
                        })
                        .count();
                    let existing_files = segment
                        .index
                        .files
                        .iter()
                        .map(|file| file.entry.id)
                        .collect::<std::collections::HashSet<_>>();
                    let new_files = incoming
                        .index
                        .files
                        .iter()
                        .filter(|file| !existing_files.contains(&file.entry.id))
                        .count();
                    policy
                        .can_grow(
                            live_records,
                            live_files,
                            incoming.index.record_count(),
                            new_files,
                        )
                        .then_some((live_records, segment.id, position))
                })
                .min();
            let Some((_, _, candidate_position)) = candidate else {
                if let Some(incoming) = group.last() {
                    append_segment_ownership(incoming, &mut ownership_added);
                }
                continue;
            };
            let candidate = &group[candidate_position];
            let mut parts = crate::lifecycle_partition::compact_source_parts_with_policy(
                &candidate.index,
                &candidate.active,
                &self.catalog,
                policy,
                || false,
            )?;
            let incoming_file_ids = incoming
                .index
                .files
                .iter()
                .map(|file| file.entry.id)
                .collect::<std::collections::HashSet<_>>();
            let projected_overlap = parts
                .first()
                .is_some_and(|part| {
                    part.files
                        .iter()
                        .any(|file| incoming_file_ids.contains(&file.entry.id))
                });
            if projected_overlap {
                let part = parts
                    .first_mut()
                    .ok_or_else(|| IndexError::Build("empty projection".into()))?;
                if part
                    .records
                    .iter()
                    .any(|record| incoming_file_ids.contains(&record.file))
                {
                    if let Some(incoming) = group.last() {
                        append_segment_ownership(incoming, &mut ownership_added);
                    }
                    continue;
                }
                let files = part
                    .files
                    .iter()
                    .filter(|file| !incoming_file_ids.contains(&file.entry.id))
                    .cloned()
                    .collect::<Vec<_>>();
                let (files, records) =
                    crate::segment_archive::SegmentRows::freeze(files, part.records.to_vec())
                        .map_err(IndexError::Build)?
                        .views();
                part.files = files;
                part.records = records;
            }
            let [projected] = parts.as_slice() else {
                if let Some(incoming) = group.last() {
                    append_segment_ownership(incoming, &mut ownership_added);
                }
                continue;
            };
            let consolidated = IndexedCollectionSegment::consolidate(
                &[projected, &incoming.index],
                policy.target_records.get(),
            )
            .map_err(IndexError::Build)?;
            if consolidated.index.record_count() > policy.target_records.get() {
                if let Some(incoming) = group.last() {
                    append_segment_ownership(incoming, &mut ownership_added);
                }
                continue;
            }
            let mut consolidated = consolidated.index;
            let segment_id = incoming.id;
            consolidated.id = segment_id;
            let active =
                ActiveRecords::all(consolidated.record_count()).map_err(IndexError::ResourceLimit)?;
            let replacement = Arc::new(
                PublishedSegment::checked(segment_id, Arc::new(consolidated), active)
                    .map_err(IndexError::InvalidPublication)?,
            );
            let mut removed = Vec::new();
            for source in [candidate, incoming] {
                for ordinal in 0..source.index.file_count() {
                    let Some(file) = source.index.files.file_id(ordinal) else {
                        continue;
                    };
                    let Some(version) = self.file_versions.get(file) else {
                        continue;
                    };
                    removed.push((
                        file,
                        version,
                        OwnershipRoute {
                            collection: source.index.collection,
                            segment: source.id,
                            file_row_ordinal: ordinal as u32,
                        },
                    ));
                }
            }
            let added = (0..replacement.index.file_count())
                .filter_map(|ordinal| {
                    replacement.index.files.file_id(ordinal).map(|file| {
                        (
                            file,
                            OwnershipRoute {
                                collection: replacement.index.collection,
                                segment: replacement.id,
                                file_row_ordinal: ordinal as u32,
                            },
                        )
                    })
                })
                .collect::<Vec<_>>();
            ownership_removed.extend(removed);
            ownership_added.extend(added);
            let group = Arc::make_mut(&mut self.segments[number]);
            group.remove(candidate_position);
            group.pop();
            group.insert(candidate_position, replacement);
            self.destination_rebuilt = true;
        }
        self.catalog
            .update_ownership(
                &self.file_versions,
                &ownership_removed,
                &ownership_added,
            )
            .map_err(IndexError::InvalidPublication)?;
        for number in attempted {
            self.rebuild_segment_position(number);
        }
        Ok(self)
    }

    /// Coalesce small delta shards without enlarging a partition beyond the
    /// local verified-maintenance budget. Full base partitions do not consume
    /// the sparse-delta allowance, even when a collection has more than 64.
    pub(crate) fn make_room_for_segment(&mut self, number: usize) -> Result<(), IndexError> {
        self.make_room_for_segment_with_limits(
            number,
            MAX_SPARSE_SEGMENT_BYTES,
            SIZE_MAINTENANCE_REVISIONS,
        )
    }

    fn make_room_for_segment_with_limits(
        &mut self,
        number: usize,
        sparse_byte_limit: usize,
        size_cooldown: u64,
    ) -> Result<(), IndexError> {
        let policy = self.repository.options().segment_policy;
        let target_records = policy.target_records.get();
        let full_partitions = self.segments[number]
            .iter()
            .filter(|segment| segment.index.record_count() >= target_records)
            .count();
        let admission_limit = MAX_SEGMENTS_PER_COLLECTION.saturating_add(full_partitions);
        let count_triggered = self.segments[number].len() >= admission_limit;
        let mut ownership_removed = Vec::new();
        let mut ownership_added = Vec::new();
        let group = Arc::make_mut(&mut self.segments[number]);
        if count_triggered {
            // A current zero-record file still owns its catalogue contribution.
            let mut kept = Vec::with_capacity(group.len());
            for segment in group.drain(..) {
                let mut current_file = false;
                for file_number in 0..segment.index.file_count() {
                    let file = segment
                        .index
                        .files
                        .get(file_number)
                        .map(|file| &file.entry)
                        .ok_or_else(|| {
                            IndexError::InvalidPublication("segment file row missing".into())
                        })?;
                    if self.catalog.matches_revision(
                        file.id,
                        file.revision,
                        file.created_ns,
                        segment.index.collection,
                    ) {
                        current_file = true;
                        break;
                    }
                }
                if segment.active.len() > 0 || current_file {
                    kept.push(segment);
                } else {
                    ownership_removed.extend(removed_segment_ownership(
                        &segment,
                        &self.file_versions,
                    ));
                }
            }
            *group = kept;
        }
        let count_triggered = group.len() >= admission_limit;
        if !count_triggered {
            let retired = group
                .iter()
                .enumerate()
                .filter_map(|(index, segment)| {
                    let total = segment.index.record_count();
                    let removed = total.saturating_sub(segment.active.len() as usize);
                    (total >= MIN_RETIREMENT_SLOTS
                        && total <= target_records
                        && removed.saturating_mul(4) >= total)
                        .then_some((removed, index))
                })
                .max();
            if let Some((_, index)) = retired {
                let old = Arc::clone(&group[index]);
                let replacements = crate::lifecycle_partition::compact_source_parts_with_policy(
                    &old.index,
                    &old.active,
                    &self.catalog,
                    policy,
                    || false,
                )?;
                ownership_removed.extend(removed_segment_ownership(
                    &old,
                    &self.file_versions,
                ));
                group.remove(index);
                for (offset, part) in replacements.into_iter().enumerate() {
                    let ordinal = self.next_segment_ordinal;
                    self.next_segment_ordinal = ordinal.checked_add(1).ok_or_else(|| {
                        IndexError::ResourceLimit("segment ordinal overflow".into())
                    })?;
                    let active = ActiveRecords::all(part.records.len())
                        .map_err(IndexError::ResourceLimit)?;
                    let id = SegmentId::new(ordinal);
                    let mut part = part;
                    part.id = id;
                    let published = PublishedSegment::checked(id, Arc::new(part), active)
                        .map_err(IndexError::InvalidPublication)?;
                    append_segment_ownership(&published, &mut ownership_added);
                    group.insert(
                        index + offset,
                        Arc::new(published),
                    );
                }
                self.catalog
                    .update_ownership(
                        &self.file_versions,
                        &ownership_removed,
                        &ownership_added,
                    )
                    .map_err(IndexError::InvalidPublication)?;
                self.rebuild_segment_position(number);
                return Ok(());
            }
        }
        // Native retained-memory estimate, not an encoded length or a strict
        // allocator count; it only orders when size-triggered maintenance runs.
        let sparse_bytes = group
            .iter()
            .filter(|segment| segment.index.record_count() < target_records)
            .map(|segment| segment.index.accounting.retained_bytes_estimated)
            .fold(0usize, usize::saturating_add);
        let size_triggered = sparse_bytes > sparse_byte_limit
            && (self.last_size_maintenance_revision == 0
                || self
                    .revision
                    .0
                    .saturating_sub(self.last_size_maintenance_revision)
                    >= size_cooldown);
        if !count_triggered && !size_triggered {
            self.catalog
                .update_ownership(&self.file_versions, &ownership_removed, &[])
                .map_err(IndexError::InvalidPublication)?;
            return Ok(());
        }
        let Some((left, right)) = cheapest_consolidation_pair(group, target_records) else {
            self.catalog
                .update_ownership(&self.file_versions, &ownership_removed, &[])
                .map_err(IndexError::InvalidPublication)?;
            self.rebuild_segment_position(number);
            return Ok(());
        };
        let merged = IndexedCollectionSegment::consolidate(
            &[&group[left].index, &group[right].index],
            target_records,
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
        let merged_index = merged.index;
        ownership_removed.extend(removed_segment_ownership(
            &group[left],
            &self.file_versions,
        ));
        ownership_removed.extend(removed_segment_ownership(
            &group[right],
            &self.file_versions,
        ));
        group.remove(right);
        group.remove(left);
        let id = SegmentId::new(ordinal);
        let published = Arc::new(
            PublishedSegment::checked(id, Arc::new(merged_index), active)
                .map_err(IndexError::InvalidPublication)?,
        );
        append_segment_ownership(&published, &mut ownership_added);
        group.push(published);
        self.catalog
            .update_ownership(
                &self.file_versions,
                &ownership_removed,
                &ownership_added,
            )
            .map_err(IndexError::InvalidPublication)?;
        self.rebuild_segment_position(number);
        if size_triggered {
            self.last_size_maintenance_revision = self.revision.0;
        }
        Ok(())
    }

    /// Retire slots owned by one incarnation without touching the catalog.
    /// The caller either replaces or removes its catalog entry atomically.
    fn retire_file_records(
        &mut self,
        file: &FileEntry,
        version: FileVersion,
    ) -> Result<u64, IndexError> {
        let mut retired_records = 0u64;
        let routes = self
            .catalog
            .ownership_routes(file.id, version)
            .map_err(IndexError::InvalidPublication)?;
        for route in routes {
            if route.segment.get() == 0 {
                let collection = self
                    .repository
                    .collections()
                    .get(route.collection.0 as usize)
                    .ok_or_else(|| {
                        IndexError::InvalidPublication(
                            "base ownership collection missing".into(),
                        )
                    })?;
                let routed_file = collection
                    .generation
                    .files
                    .get(route.file_row_ordinal as usize)
                    .ok_or_else(|| {
                        IndexError::InvalidPublication(
                            "base ownership row ordinal missing".into(),
                        )
                    })?;
                if *routed_file != file.id {
                    return Err(IndexError::InvalidPublication(
                        "base ownership route file mismatch".into(),
                    ));
                }
                let slots = collection
                    .generation
                    .file_records
                    .records_for(file.id)
                    .ok_or_else(|| {
                        IndexError::InvalidPublication(
                            "base file record route missing".into(),
                        )
                    })?;
                let active = self.active.get_mut(route.collection.0 as usize).ok_or_else(
                    || IndexError::InvalidPublication("base active domain missing".into()),
                )?;
                retired_records +=
                    slots.iter().filter(|slot| active.contains(**slot)).count() as u64;
                *active = active
                    .retired(slots)
                    .map_err(IndexError::InvalidPublication)?;
                continue;
            }
            let segments = self
                .segments
                .get_mut(route.collection.0 as usize)
                .ok_or_else(|| {
                    IndexError::InvalidPublication("routed collection missing".into())
                })?;
            let segments = Arc::make_mut(segments);
            crate::storage_usage::StorageWorkCounters::update(|counters| {
                counters.segment_descriptors_examined = counters
                    .segment_descriptors_examined
                    .saturating_add(1);
            });
            let position = self
                .segment_positions
                .get(route.collection.0 as usize)
                .and_then(|positions| positions.get(&route.segment))
                .copied()
                .ok_or_else(|| {
                    IndexError::InvalidPublication("routed segment missing".into())
                })?;
            if segments.get(position).map(|segment| segment.id) != Some(route.segment) {
                return Err(IndexError::InvalidPublication(
                    "segment position directory is stale".into(),
                ));
            }
            let segment = Arc::make_mut(&mut segments[position]);
            let routed_file = segment
                .index
                .files
                .file_id(route.file_row_ordinal as usize)
                .ok_or_else(|| {
                    IndexError::InvalidPublication("ownership row ordinal missing".into())
                })?;
            if routed_file != file.id {
                return Err(IndexError::InvalidPublication(
                    "segment ownership route file mismatch".into(),
                ));
            }
            let slots = segment
                .index
                .files
                .archived_record_slots(route.file_row_ordinal as usize)
                .ok_or_else(|| {
                    IndexError::InvalidPublication("ownership slot list missing".into())
                })?
                .collect::<Vec<_>>();
            retired_records += slots
                .iter()
                .filter(|slot| segment.active.contains(**slot))
                .count() as u64;
            segment.active = segment
                .active
                .retired(&slots)
                .map_err(IndexError::InvalidPublication)?;
        }
        self.catalog
            .retire_ownership(file.id, version)
            .map_err(IndexError::InvalidPublication)?;
        Ok(retired_records)
    }

    fn remove_obsolete_segments(&mut self) -> Result<(), IndexError> {
        let mut changed = Vec::new();
        for (number, segments) in self.segments.iter_mut().enumerate() {
            let group = Arc::make_mut(segments);
            let mut retained = Vec::with_capacity(group.len());
            for segment in group.drain(..) {
                crate::storage_usage::StorageWorkCounters::update(|counters| {
                    counters.segment_descriptors_examined =
                        counters.segment_descriptors_examined.saturating_add(1);
                });
                let current_zero_file = (0..segment.index.file_count()).any(|ordinal| {
                    let Some(file) = segment.index.files.file_id(ordinal) else {
                        return false;
                    };
                    let Some(version) = self.file_versions.get(file) else {
                        return false;
                    };
                    self.catalog
                        .ownership_routes(file, version)
                        .is_ok_and(|routes| {
                            routes.iter().any(|route| {
                                route.collection.0 as usize == number
                                    && route.segment == segment.id
                                    && route.file_row_ordinal == ordinal as u32
                            })
                        })
                });
                if segment.active.len() != 0 || current_zero_file {
                    retained.push(segment);
                }
            }
            *group = retained;
            changed.push(number);
        }
        for number in changed {
            self.rebuild_segment_position(number);
        }
        Ok(())
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

fn prepare_initial_segment(
    next: &mut PublishedIndex,
    segment: IndexedCollectionSegment,
) -> Result<(), IndexError> {
    for file in &segment.files {
        if file.entry.status == FileStatus::Valid
            && file.entry.diagnostics.is_empty()
            && file.entry.omitted_diagnostics == 0
        {
            continue;
        }
        let mut merged = next
            .catalog
            .entry_owned(file.entry.id)
            .ok_or(IndexError::UnknownFile)?
            ;
        merged.status = merged.status.max(file.entry.status);
        for diagnostic in &file.entry.diagnostics {
            merged
                .record_diagnostic(diagnostic.clone())
                .map_err(IndexError::InvalidPublication)?;
        }
        merged.omitted_diagnostics = merged
            .omitted_diagnostics
            .saturating_add(file.entry.omitted_diagnostics);
        next.catalog = next
            .catalog
            .replaced(merged, None, None)
            .map_err(IndexError::InvalidPublication)?;
    }
    *next = next.clone().with_segment(segment)?;
    Ok(())
}

impl Index {
    pub fn rkyv_archive_bytes(&self) -> Result<usize, IndexError> {
        Ok(self.view()?.0.archive_bytes())
    }

    /// Publish one complete initial work unit. A concurrent lifecycle
    /// mutation changes the revision and rejects this stale work atomically.
    pub(crate) fn publish_initial_segment(
        &self,
        expected: IndexRevision,
        segment: IndexedCollectionSegment,
        profile_phases: bool,
    ) -> Result<(IndexRevision, InitialPublicationPhases), IndexError> {
        let prepare_started = crate::detailed_timer::start_if_enabled(profile_phases);
        let view = self.view()?;
        if view.revision() != expected {
            return Err(IndexError::RevisionConflict {
                expected,
                actual: view.revision(),
            });
        }
        if view.initial_construction_complete() {
            return Err(IndexError::InvalidPublication(
                "initial construction already completed".into(),
            ));
        }
        let mut next = (*view.0).clone();
        prepare_initial_segment(&mut next, segment)?;
        next.revision = expected.next()?;
        let preparation_ns = prepare_started
            .map(|started| started.elapsed().as_nanos() as u64)
            .unwrap_or(0);
        let install_started = crate::detailed_timer::start_if_enabled(profile_phases);
        let revision = self.publish_prepared_inner(expected, next)?;
        let install_ns = install_started
            .map(|started| started.elapsed().as_nanos() as u64)
            .unwrap_or(0);
        Ok((
            revision,
            InitialPublicationPhases {
                preparation_ns,
                install_ns,
            },
        ))
    }

    /// Install a validated physical-file group as one publication revision.
    pub(crate) fn publish_initial_segments(
        &self,
        expected: IndexRevision,
        segments: Vec<IndexedCollectionSegment>,
        profile_phases: bool,
    ) -> Result<(IndexRevision, InitialPublicationPhases), IndexError> {
        if segments.is_empty() {
            return Err(IndexError::InvalidPublication(
                "initial publication group is empty".into(),
            ));
        }
        let prepare_started = crate::detailed_timer::start_if_enabled(profile_phases);
        let view = self.view()?;
        if view.revision() != expected {
            return Err(IndexError::RevisionConflict {
                expected,
                actual: view.revision(),
            });
        }
        if view.initial_construction_complete() {
            return Err(IndexError::InvalidPublication(
                "initial construction already completed".into(),
            ));
        }
        let mut next = (*view.0).clone();
        for segment in segments {
            prepare_initial_segment(&mut next, segment)?;
        }
        next.revision = expected.next()?;
        let preparation_ns = prepare_started
            .map(|started| started.elapsed().as_nanos() as u64)
            .unwrap_or(0);
        let install_started = crate::detailed_timer::start_if_enabled(profile_phases);
        let revision = self.publish_prepared_inner(expected, next)?;
        let install_ns = install_started
            .map(|started| started.elapsed().as_nanos() as u64)
            .unwrap_or(0);
        Ok((
            revision,
            InitialPublicationPhases {
                preparation_ns,
                install_ns,
            },
        ))
    }

    /// Finalize metadata without copying the completed segment payloads.
    pub(crate) fn finish_initial_construction(
        &self,
        expected: IndexRevision,
        repository: Arc<Repository>,
    ) -> Result<IndexRevision, IndexError> {
        let view = self.view()?;
        if view.revision() != expected {
            return Err(IndexError::RevisionConflict {
                expected,
                actual: view.revision(),
            });
        }
        if view.initial_construction_complete() {
            return Err(IndexError::InvalidPublication(
                "initial construction already completed".into(),
            ));
        }
        let mut next = (*view.0).clone();
        if next.processed_files != next.catalog.active_files() as usize {
            return Err(IndexError::InvalidPublication(
                "initial construction has unprocessed files".into(),
            ));
        }
        for group in &mut next.segments {
            for published in Arc::make_mut(group).iter_mut() {
                let needs_status_refresh = published.index.files.iter().any(|file| {
                    repository
                        .files
                        .files
                        .get(file.entry.id.0 as usize)
                        .is_some_and(|current| current.status != file.entry.status)
                });
                let decision = repository
                    .collections
                    .get(published.index.collection.0 as usize)
                    .and_then(|collection| collection.global_inference.as_ref());
                let needs_inference_prune = decision.is_some_and(|decision| {
                    published.index.inference.as_ref().is_some_and(|state| {
                        state.indexes.iter().any(|(path, index)| {
                            let field = decision
                                .report
                                .fields
                                .iter()
                                .find(|field| field.path == *path);
                            if field.is_none_or(|field| {
                                field.index_status != crate::inference::InferenceIndexStatus::Built
                            }) {
                                return true;
                            }
                            let diagnostic = decision
                                .diagnostics
                                .iter()
                                .find(|diagnostic| diagnostic.path == *path);
                            index.value_complete
                                && !diagnostic
                                    .is_some_and(|diagnostic| diagnostic.value_dictionary_complete)
                                || index.length_complete
                                    && !diagnostic
                                        .is_some_and(|diagnostic| diagnostic.array_length_complete)
                                || index.empty_string_complete
                                    && !diagnostic
                                        .is_some_and(|diagnostic| diagnostic.empty_string_exact)
                        })
                    })
                });
                if !needs_status_refresh && !needs_inference_prune {
                    continue;
                }
                let mut index = (*published.index).clone();
                let mut refreshed_files = index.files.to_vec();
                for file in &mut refreshed_files {
                    if let Some(current) = repository.files.files.get(file.entry.id.0 as usize) {
                        file.entry.status = current.status;
                        file.entry.diagnostics = current.diagnostics.clone();
                        file.entry.omitted_diagnostics = current.omitted_diagnostics;
                    }
                }
                let (files, records) = crate::segment_archive::SegmentRows::freeze(
                    refreshed_files,
                    index.records.to_vec(),
                )
                .map_err(IndexError::InvalidPublication)?
                .views();
                index.files = files;
                index.records = records;
                index
                    .sorting
                    .refresh_file_status_for_records(&index.records, |file| {
                        repository
                            .files
                            .files
                            .get(file.0 as usize)
                            .map_or(crate::inventory::FileStatus::Valid.code(), |entry| {
                                entry.status.code()
                            })
                    });
                if let Some(decision) = decision {
                    if let Some(inference) = index.inference.as_mut() {
                        for field in &decision.report.fields {
                            if matches!(
                                field.index_status,
                                crate::inference::InferenceIndexStatus::Redundant
                                    | crate::inference::InferenceIndexStatus::ExplicitOptOut
                            ) {
                                inference.indexes.remove(&field.path);
                                inference.empty_selections.remove(&field.path);
                                inference.short_string_coverage.remove(&field.path);
                                continue;
                            }
                            let diagnostics = decision
                                .diagnostics
                                .iter()
                                .find(|diagnostic| diagnostic.path == field.path);
                            let general_values = diagnostics
                                .is_some_and(|diagnostic| diagnostic.value_dictionary_complete);
                            let short_values = diagnostics
                                .is_some_and(|diagnostic| diagnostic.short_string_candidates);
                            let empty_string_exact =
                                diagnostics.is_some_and(|diagnostic| diagnostic.empty_string_exact);
                            if let Some(posting) = inference.indexes.get_mut(&field.path) {
                                if !general_values && !short_values {
                                    posting
                                        .thaw_storage()
                                        .map_err(IndexError::InvalidPublication)?;
                                    let empty = empty_string_exact.then(|| {
                                        posting.values.remove_public(
                                            &crate::posting::ScalarKey::String(String::new()),
                                        )
                                    });
                                    posting.values.clear();
                                    if let Some(Some(slots)) = empty {
                                        posting.values.insert_public(
                                            &crate::posting::ScalarKey::String(String::new()),
                                            slots,
                                        );
                                    }
                                    posting.values_indexed = empty_string_exact;
                                }
                                posting.value_complete = general_values;
                                posting.length_complete = diagnostics
                                    .is_some_and(|diagnostic| diagnostic.array_length_complete);
                                posting.empty_string_complete = empty_string_exact;
                            }
                            if !short_values {
                                inference.short_string_coverage.remove(&field.path);
                            }
                        }
                    }
                }
                let index = index
                    .seal_completed_storage()
                    .map_err(IndexError::InvalidPublication)?;
                let replacement = PublishedSegment::checked(
                    published.id,
                    Arc::new(index),
                    published.active.clone(),
                )
                .map_err(IndexError::InvalidPublication)?;
                *published = Arc::new(replacement);
            }
        }
        next.catalog
            .rebase(Arc::clone(&repository))
            .map_err(IndexError::InvalidPublication)?;
        next.catalog.set_construction_complete(true);
        next.processed_memberships.clear();
        next.repository = repository;
        next.revision = expected.next()?;
        self.publish_prepared(expected, next)
    }

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
        next.destination_rebuilt = false;
        next.catalog = next
            .catalog
            .appended(entry, fingerprint, Some(prepared.configuration_identity))
            .map_err(IndexError::InvalidPublication)?;
        next.file_versions = next
            .file_versions
            .appended()
            .map_err(IndexError::ResourceLimit)?;
        next = next.place_mutation_segments(indexed)?;
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
        self.stage_prepared_replace_mode(prepared, true)
    }

    pub(crate) fn stage_prepared_replace_append_only(
        &self,
        prepared: PreparedReplacement,
    ) -> Result<PublishedIndex, IndexError> {
        self.stage_prepared_replace_mode(prepared, false)
    }

    fn stage_prepared_replace_mode(
        &self,
        prepared: PreparedReplacement,
        rebuild_destination: bool,
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
        next.destination_rebuilt = false;
        next.retire_file_records(previous, prepared.expected_handle.version)?;
        next.catalog = next
            .catalog
            .replaced(entry, fingerprint, Some(prepared.configuration_identity))
            .map_err(IndexError::InvalidPublication)?;
        next.file_versions = next
            .file_versions
            .advanced(file_id)
            .map_err(IndexError::ResourceLimit)?
            .0;
        next = if rebuild_destination {
            next.place_mutation_segments(indexed)?
        } else {
            let mut next = next;
            let mut added = Vec::new();
            for segment in indexed {
                added.extend((0..segment.file_count()).filter_map(|ordinal| {
                    segment.files.file_id(ordinal).map(|file| {
                        (
                            file,
                            OwnershipRoute {
                                collection: segment.collection,
                                segment: segment.id,
                                file_row_ordinal: ordinal as u32,
                            },
                        )
                    })
                }));
                next = next.with_segment_uncoalesced(segment)?;
            }
            next.catalog
                .update_ownership(&next.file_versions, &[], &added)
                .map_err(IndexError::InvalidPublication)?;
            next
        };
        next.remove_obsolete_segments()?;
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
        let retired_records = next.retire_file_records(file, handle.version)?;
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
        next.remove_obsolete_segments()?;
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
        next: PublishedIndex,
    ) -> Result<IndexRevision, IndexError> {
        self.publish_prepared_inner(expected, next)
    }

    fn publish_prepared_inner(
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
        #[cfg(test)]
        let notify_cache = next.catalog.construction_complete();
        let next = Arc::new(next);
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
            std::mem::replace(&mut *guard, next)
        };
        drop(retired);
        #[cfg(test)]
        let _ = notify_cache;
        Ok(next_revision)
    }
}

/// Pinned immutable read context for a single publication revision.
#[derive(Clone)]
pub struct IndexView(pub(crate) Arc<PublishedIndex>, Weak<IndexInner>);

impl IndexView {
    /// Startup observations belonging to this immutable publication.
    pub fn startup_stats(&self) -> &crate::repository::StartupStats {
        self.repository().startup()
    }

    pub fn initial_construction_complete(&self) -> bool {
        self.0.catalog.construction_complete()
    }

    /// Configured collections captured by this publication.
    pub fn collection_definitions(
        &self,
    ) -> impl Iterator<Item = &crate::repository::CollectionDefinition> + '_ {
        self.repository()
            .collections()
            .iter()
            .map(|state| &state.definition)
    }

    /// Inference observations captured by this publication, including the
    /// completed global summary of an incremental construction.
    pub fn inference_report(&self, collection: &str) -> Option<&crate::inference::InferenceReport> {
        self.repository().inference_report(collection)
    }

    /// Borrow a row from this pinned publication without copying the corpus
    /// path catalogue or implying that the file has been parsed.
    pub fn catalogue_row(&self, id: FileId) -> Option<FileCatalogueRow<'_>> {
        if !self.0.catalog.is_active(id) {
            return None;
        }
        let file = self.0.catalog.entry(id)?;
        Some(FileCatalogueRow {
            id,
            relative_path: file.relative_path.as_path(),
            collections: &file.collections,
            size: file.revision.size,
            modified_ns: file.revision.modified_ns,
            created_ns: file.created_ns,
        })
    }

    pub fn catalogue_rows(&self) -> impl Iterator<Item = FileCatalogueRow<'_>> + '_ {
        (0..self.0.catalog.allocated_files())
            .filter_map(|number| self.catalogue_row(FileId(number as u32)))
    }

    /// Narrow index-family estimate for a direct repository, summed across the
    /// base and live lifecycle shards. See `SegmentAccounting` for the cached
    /// maintenance estimate.
    pub fn estimated_index_bytes(&self) -> usize {
        self.repository()
            .collections()
            .iter()
            .map(|collection| collection.index_family_bytes)
            .sum::<usize>()
            .saturating_add(
                self.0
                    .segments
                    .iter()
                    .flat_map(|group| group.iter())
                    .map(|segment| segment.index.accounting.index_family_bytes)
                    .sum::<usize>(),
            )
    }

    /// Per-collection base (ordinal 0) and published segment estimates. The
    /// benchmark uses this to expose shard overhead without counting it as a
    /// second resident catalogue.
    pub fn index_family_breakdown(
        &self,
    ) -> std::collections::BTreeMap<String, Vec<(u64, IndexFamilyEstimate)>> {
        self.repository()
            .collections()
            .iter()
            .enumerate()
            .map(|(number, collection)| {
                let mut parts = vec![(
                    0,
                    index_family_estimate(
                        &collection.raw,
                        &collection.fallback_text,
                        &collection.custom_text,
                        &collection.postings,
                        &collection.sorting,
                    ),
                )];
                parts.extend(self.0.segments[number].iter().map(|segment| {
                    (
                        segment.id.get(),
                        index_family_estimate(
                            &segment.index.raw,
                            &segment.index.fallback_text,
                            &segment.index.custom_text,
                            &segment.index.postings,
                            &segment.index.sorting,
                        ),
                    )
                }));
                (collection.definition.name.clone(), parts)
            })
            .collect()
    }

    pub fn estimated_inventory_bytes(&self) -> usize {
        self.repository().files().estimated_heap_bytes().0
    }

    fn has_segments(&self) -> bool {
        self.0.segments.iter().any(|group| !group.is_empty())
    }

    fn ensure_initial_complete(&self) -> Result<(), String> {
        if self.initial_construction_complete() {
            Ok(())
        } else {
            Err(
                "initial index construction is incomplete; use the explicit available-record APIs"
                    .into(),
            )
        }
    }

    fn ensure_local_partial_query(&self, query: &Query) -> Result<(), String> {
        if self.initial_construction_complete() {
            return Ok(());
        }
        self.ensure_record_local_query(query)
    }

    fn ensure_record_local_query(&self, query: &Query) -> Result<(), String> {
        if query.options.offset != 0 {
            return Err("pagination waits for initial index construction".into());
        }
        let prepared = crate::prepared::prepare(self.repository(), query)?;
        if prepared.collections.iter().any(|(_, filter, fts)| {
            filter
                .as_ref()
                .is_some_and(crate::run::bound_expr_has_relations)
                || fts
                    .as_ref()
                    .is_some_and(|fts| fts.fields.iter().any(|field| field.related.is_some()))
        }) {
            return Err("relation queries wait for initial index construction".into());
        }
        Ok(())
    }

    fn stamp_result(&self, mut result: SearchResult) -> SearchResult {
        for hit in &mut result.hits {
            hit.generation = self.revision().0;
            self.remap_hit_diagnostic(hit);
        }
        result
    }

    fn remap_hit_diagnostic(&self, hit: &mut SearchHit) {
        let Some(local) = hit.diagnostic_ref.stored_index() else {
            return;
        };
        let Some(collection) = self
            .repository()
            .collections()
            .iter()
            .position(|collection| collection.definition.name == hit.collection)
        else {
            return;
        };
        let file = FileId(hit.file_id);
        for segment in self.0.segments[collection].iter() {
            let Some(slot) = segment.index.id_lookup.get(hit.record_id.as_str()) else {
                continue;
            };
            if !segment.active.contains(slot)
                || segment
                    .index
                    .record(slot)
                    .is_none_or(|record| record.file != file)
            {
                continue;
            }
            let Some(source) = segment.index.file(file) else {
                return;
            };
            let Some(diagnostic) = source.entry.diagnostics.get(local) else {
                return;
            };
            let Some(position) = self.0.catalog.entry(file).and_then(|entry| {
                entry
                    .diagnostics
                    .iter()
                    .position(|candidate| candidate == diagnostic)
            }) else {
                return;
            };
            let reference = crate::inventory::DiagnosticRef((position + 1) as u32);
            hit.diagnostic_ref = reference;
            if let Some(crate::repository::Payload::LogicalUnavailable { diagnostic_ref }) =
                &mut hit.payload
            {
                *diagnostic_ref = reference;
            }
            return;
        }
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

    pub(crate) fn file_entry_owned(&self, file: FileId) -> Option<FileEntry> {
        self.0.catalog.entry_owned(file)
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
        if !self.initial_construction_complete()
            && !entry.collections.iter().all(|collection| {
                self.0
                    .processed_memberships
                    .get(collection.0 as usize)
                    .is_some_and(|covered| covered.contains(file.0))
            })
        {
            return None;
        }
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
        if !self.initial_construction_complete() {
            return Err("initial index construction is incomplete; use search_available".into());
        }
        self.search_published(query)
    }

    fn search_published(&self, query: &Query) -> Result<SearchResult, String> {
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
        result.map(|result| self.stamp_result(result))
    }

    /// Opt in to the validated records in this publication. An incomplete
    /// view cannot return a complete count or a final continuation page.
    pub fn search_available(&self, query: &Query) -> Result<AvailableSearchResult, String> {
        self.ensure_local_partial_query(query)?;
        let mut result = self.search_published(query)?;
        let complete = self.initial_construction_complete();
        if !complete {
            result.count = None;
            result.counters.match_count_complete = false;
        }
        Ok(AvailableSearchResult {
            result,
            complete,
            revision: self.revision(),
            discovered_files: self.0.catalog.active_files() as usize,
            processed_files: self.0.processed_files,
        })
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
        if !self.initial_construction_complete() {
            return Err(context.error(
                crate::streaming::ExecutionErrorKind::Execution,
                "initial index construction is incomplete; use search_available_with_context",
            ));
        }
        self.search_published_with_context(query, context)
    }

    fn search_published_with_context(
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
        result.map(|result| self.stamp_result(result))
    }

    pub fn search_available_with_context(
        &self,
        query: &Query,
        context: &ExecutionContext,
    ) -> Result<AvailableSearchResult, ExecutionError> {
        self.ensure_local_partial_query(query).map_err(|message| {
            context.error(crate::streaming::ExecutionErrorKind::InvalidQuery, message)
        })?;
        let mut result = self.search_published_with_context(query, context)?;
        let complete = self.initial_construction_complete();
        if !complete {
            result.count = None;
            result.counters.match_count_complete = false;
        }
        Ok(AvailableSearchResult {
            result,
            complete,
            revision: self.revision(),
            discovered_files: self.0.catalog.active_files() as usize,
            processed_files: self.0.processed_files,
        })
    }

    /// Use a fresh request context while keeping query cancellation separate
    /// from the independent construction worker.
    pub fn search_available_with_cancellation(
        &self,
        query: &Query,
        cancellation: &CancellationToken,
    ) -> Result<AvailableSearchResult, ExecutionError> {
        let context = ExecutionContext::new(
            crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
            crate::streaming::ExecutionLimits::default(),
            Some(cancellation.clone()),
        );
        self.search_available_with_context(query, &context)
    }

    /// Evaluate only segments published after `previous`. A caller can append
    /// these record-local matches to its previous unsorted result, including
    /// when several notifications were coalesced. A changed lineage or query
    /// with global dependencies returns `None` for a full replacement search.
    pub fn search_added_since(
        &self,
        previous: &IndexView,
        query: &Query,
    ) -> Result<Option<AvailableSearchResult>, String> {
        let context = ExecutionContext::new(
            crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
            crate::streaming::ExecutionLimits::default(),
            None,
        );
        self.search_added_since_with_context(previous, query, &context)
            .map_err(|error| error.to_string())
    }

    pub fn search_added_since_with_cancellation(
        &self,
        previous: &IndexView,
        query: &Query,
        cancellation: &CancellationToken,
    ) -> Result<Option<AvailableSearchResult>, ExecutionError> {
        let context = ExecutionContext::new(
            crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
            crate::streaming::ExecutionLimits::default(),
            Some(cancellation.clone()),
        );
        self.search_added_since_with_context(previous, query, &context)
    }

    /// Limit newly matching rows before selected payloads and evidence are
    /// materialized. The ordinary delta methods retain their all-match contract.
    pub fn search_added_since_bounded_with_cancellation(
        &self,
        previous: &IndexView,
        query: &Query,
        limit: NonZeroUsize,
        cancellation: &CancellationToken,
    ) -> Result<Option<AvailableSearchResult>, ExecutionError> {
        let context = ExecutionContext::new(
            crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
            crate::streaming::ExecutionLimits::default(),
            Some(cancellation.clone()),
        );
        self.search_added_since_with_context_limit(previous, query, &context, Some(limit))
    }

    /// Check whether a full appendable page can advance its publication
    /// cursor without executing a delta. Global and changed-lineage queries
    /// still require the caller's replacement path.
    pub fn can_append_added_since_with_cancellation(
        &self,
        previous: &IndexView,
        query: &Query,
        cancellation: &CancellationToken,
    ) -> Result<bool, ExecutionError> {
        let context = ExecutionContext::new(
            crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
            crate::streaming::ExecutionLimits::default(),
            Some(cancellation.clone()),
        );
        self.delta_query_if_appendable(previous, query, &context)
            .map(|query| query.is_some())
    }

    pub fn search_added_since_with_context(
        &self,
        previous: &IndexView,
        query: &Query,
        context: &ExecutionContext,
    ) -> Result<Option<AvailableSearchResult>, ExecutionError> {
        self.search_added_since_with_context_limit(previous, query, context, None)
    }

    fn delta_query_if_appendable(
        &self,
        previous: &IndexView,
        query: &Query,
        context: &ExecutionContext,
    ) -> Result<Option<Query>, ExecutionError> {
        context.enter()?;
        if self.instance_id() != previous.instance_id()
            || self.revision() <= previous.revision()
            || (self.initial_construction_complete() && !previous.initial_construction_complete())
            || !query.options.sort.is_empty()
            || query.options.offset != 0
            || query.options.count
            || self.0.segments.len() != previous.0.segments.len()
            || self
                .0
                .active
                .iter()
                .zip(&previous.0.active)
                .any(|(new, old)| new.bitmap() != old.bitmap())
        {
            return Ok(None);
        }
        let resolved = query.with_clock(context.clock.now).map_err(|message| {
            context.error(crate::streaming::ExecutionErrorKind::InvalidQuery, message)
        })?;
        self.ensure_record_local_query(&resolved)
            .map_err(|message| {
                context.error(crate::streaming::ExecutionErrorKind::InvalidQuery, message)
            })?;
        for (current, old) in self.0.segments.iter().zip(&previous.0.segments) {
            if old.len() > current.len()
                || old
                    .iter()
                    .zip(current.iter())
                    .any(|(left, right)| !Arc::ptr_eq(left, right))
            {
                return Ok(None);
            }
        }
        Ok(Some(resolved))
    }

    fn search_added_since_with_context_limit(
        &self,
        previous: &IndexView,
        query: &Query,
        context: &ExecutionContext,
        limit: Option<NonZeroUsize>,
    ) -> Result<Option<AvailableSearchResult>, ExecutionError> {
        let Some(resolved) = self.delta_query_if_appendable(previous, query, context)? else {
            return Ok(None);
        };
        let mut added = Vec::with_capacity(self.0.segments.len());
        let mut added_records = 0usize;
        for (current, old) in self.0.segments.iter().zip(&previous.0.segments) {
            let next = current[old.len()..].to_vec();
            added_records = added_records.saturating_add(
                next.iter()
                    .map(|segment| segment.active.len() as usize)
                    .sum::<usize>(),
            );
            added.push(Arc::new(next));
        }
        let mut request = resolved;
        request.options.limit = limit.map_or(added_records, |limit| added_records.min(limit.get()));
        request.options.count = false;
        let empty_base = self
            .0
            .active
            .iter()
            .map(|_| {
                ActiveRecords::from_bitmap(0, RoaringBitmap::new())
                    .expect("empty active bitmap is valid")
            })
            .collect::<Vec<_>>();
        let mut result = crate::run::search_segmented_with_context_lifecycle(
            self.repository(),
            &request,
            &empty_base,
            &added,
            &self.0.catalog,
            self.revision(),
            context,
            None,
        )?;
        result = self.stamp_result(result);
        result.count = None;
        result.counters.match_count_complete = false;
        Ok(Some(AvailableSearchResult {
            result,
            complete: false,
            revision: self.revision(),
            discovered_files: self.0.catalog.active_files() as usize,
            processed_files: self.0.processed_files,
        }))
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
        self.ensure_initial_complete()?;
        let mut result = crate::run::direct_lookup_segmented(
            self.repository(),
            collection,
            record_id,
            return_mode,
            &self.0.active,
            &self.0.segments,
            &self.0.catalog,
            self.revision(),
        )?;
        if let crate::run::DirectLookupResult::Found(hit) = &mut result {
            self.remap_hit_diagnostic(hit);
        }
        Ok(result)
    }

    pub fn search_reference(&self, query: &Query) -> Result<SearchResult, String> {
        self.ensure_initial_complete()?;
        if self.0.segments.iter().any(|segments| !segments.is_empty()) {
            return crate::run::search_segmented_reference(
                self.repository(),
                query,
                &self.0.active,
                &self.0.segments,
                &self.0.catalog,
                self.revision(),
            )
            .map(|result| self.stamp_result(result));
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
        self.ensure_initial_complete()?;
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
        self.ensure_initial_complete()?;
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
        self.ensure_initial_complete()?;
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
        self.ensure_initial_complete()?;
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
        self.ensure_initial_complete()?;
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
        self.ensure_initial_complete()?;
        let resolved = query.resolve_current_clock()?;
        if self.has_segments() {
            return crate::run::search_analyze_segmented(
                self.repository(),
                resolved.as_ref(),
                &self.0.active,
                &self.0.segments,
                &self.0.catalog,
                self.revision(),
            )
            .map(|(result, trace)| (self.stamp_result(result), trace));
        }
        crate::run::search_analyze_with_active(
            self.repository(),
            resolved.as_ref(),
            Some(&self.0.active),
        )
        .map(|(result, trace)| (self.stamp_result(result), trace))
    }

    pub fn explain(&self, query: &Query) -> Result<Vec<crate::run::QueryPlanTrace>, String> {
        self.ensure_initial_complete()?;
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
        if !self.initial_construction_complete() {
            return Err(StreamError::Execution {
                kind: crate::streaming::ExecutionErrorKind::Execution,
                message: "initial index construction is incomplete; use search_available".into(),
                progress: crate::streaming::StreamProgress::default(),
            });
        }
        let owner = self.1.upgrade();
        let cache = owner.as_ref().map(|owner| crate::run::LifecycleCacheScope {
            store: &owner.verified_cache,
            instance_id: self.instance_id(),
        });
        let mut remap_rows = |event: StreamEvent<'_>| -> Result<StreamControl, String> {
            match event {
                StreamEvent::OrderedRows {
                    sequence,
                    start,
                    rows,
                    progress,
                } => {
                    if rows
                        .iter()
                        .any(|hit| hit.diagnostic_ref.stored_index().is_some())
                    {
                        let mut remapped = rows.to_vec();
                        for hit in &mut remapped {
                            self.remap_hit_diagnostic(hit);
                        }
                        sink(StreamEvent::OrderedRows {
                            sequence,
                            start,
                            rows: &remapped,
                            progress,
                        })
                    } else {
                        sink(StreamEvent::OrderedRows {
                            sequence,
                            start,
                            rows,
                            progress,
                        })
                    }
                }
                StreamEvent::PageReplacement {
                    sequence,
                    rows,
                    progress,
                } => {
                    if rows
                        .iter()
                        .any(|hit| hit.diagnostic_ref.stored_index().is_some())
                    {
                        let mut remapped = rows.to_vec();
                        for hit in &mut remapped {
                            self.remap_hit_diagnostic(hit);
                        }
                        sink(StreamEvent::PageReplacement {
                            sequence,
                            rows: &remapped,
                            progress,
                        })
                    } else {
                        sink(StreamEvent::PageReplacement {
                            sequence,
                            rows,
                            progress,
                        })
                    }
                }
                StreamEvent::Progress { sequence, progress } => {
                    sink(StreamEvent::Progress { sequence, progress })
                }
            }
        };
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
            &mut remap_rows,
            cache,
        );
        result
    }
}

#[cfg(test)]
#[path = "lifecycle.test.rs"]
mod tests;
