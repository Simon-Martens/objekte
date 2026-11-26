//! Index lifecycle coordination over pinned, revisioned publications.
//!
//! `Index` owns construction, mutations, reconciliation, maintenance, periodic
//! scheduling and shutdown. Each operation has a focused child module while
//! `IndexInner` remains the sole owner of the published root and lifecycle locks.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};
use std::{borrow::Cow, fmt};

use serde::{Deserialize, Serialize};

use crate::inventory::{FileEntry, FileVersion};
use crate::model::{CollectionId, FileId};
use crate::query_results::SearchResult;

pub(crate) mod builder;
#[cfg(test)]
pub(crate) use crate::index::memory::file_catalog as catalog;
pub mod initial;
pub mod maintenance;
pub mod mutation;
mod open;
pub mod reconciliation;
mod service;

pub use builder::{FileCatalogRegistration, IndexBuilder, RecordRegistration};
use maintenance::MaintenanceJob;
use reconciliation::{MaintenanceState, PeriodicReconciliation, ReconciliationState};

pub(crate) const MAX_SEGMENTS_PER_COLLECTION: usize = 64;
const MAX_SPARSE_SEGMENT_BYTES: usize = 32 * 1024 * 1024;
const SIZE_MAINTENANCE_REVISIONS: u64 = 16;
const MIN_RETIREMENT_SLOTS: usize = 256;

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
    Storage(String),
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

impl From<crate::index::storage::transfer::StorageError> for IndexError {
    fn from(error: crate::index::storage::transfer::StorageError) -> Self {
        use crate::index::storage::transfer::StorageError;
        match error {
            StorageError::Cancelled => Self::Cancelled,
            StorageError::ResourceLimit(message) => Self::ResourceLimit(message),
            StorageError::OversizedItem { .. } => Self::ResourceLimit(error.to_string()),
            StorageError::InvalidTransfer { .. } => Self::Storage(error.to_string()),
        }
    }
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
            Self::Storage(message) => write!(formatter, "index storage: {message}"),
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
#[derive(Clone, Debug)]
pub struct FileCatalogueRow<'a> {
    pub id: FileId,
    pub relative_path: Cow<'a, Path>,
    pub collections: Cow<'a, [CollectionId]>,
    pub size: u64,
    pub modified_ns: i128,
    pub created_ns: Option<i128>,
}

impl<'a> FileCatalogueRow<'a> {
    pub(crate) fn from_entry(entry: Cow<'a, FileEntry>) -> Self {
        let id = entry.id;
        let size = entry.revision.size;
        let modified_ns = entry.revision.modified_ns;
        let created_ns = entry.created_ns;
        let (relative_path, collections) = match entry {
            Cow::Borrowed(entry) => (
                Cow::Borrowed(entry.relative_path.as_path()),
                Cow::Borrowed(entry.collections.as_slice()),
            ),
            Cow::Owned(entry) => (
                Cow::Owned(entry.relative_path.to_path_buf()),
                Cow::Owned(entry.collections),
            ),
        };
        Self {
            id,
            relative_path,
            collections,
            size,
            modified_ns,
            created_ns,
        }
    }
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
pub(crate) use crate::index::memory::segment::statistics::{
    index_family_estimate_calls, reset_index_family_estimate_calls,
};

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

struct IndexInner {
    instance_id: u64,
    timings: Mutex<IndexTimingsMs>,
    /// Set under the publication write lock before closing waits for workers.
    /// Every data publication checks it while holding that same lock.
    quiescing: AtomicBool,
    /// One aggregate budget for all pinned publications of this index.
    verified_cache: Mutex<crate::query_results::verified::VerifiedCache>,
    published: RwLock<Arc<PublishedIndex>>,
    writer: Mutex<()>,
    ingestion: Mutex<std::sync::Weak<crate::ingestion::queue::SessionCore>>,
    maintenance: Mutex<MaintenanceState>,
    maintenance_operation: Mutex<Option<Arc<MaintenanceJob>>>,
    periodic_transition: Mutex<()>,
    periodic: Mutex<Option<PeriodicReconciliation>>,
    #[cfg(test)]
    maintenance_worker_hook: Mutex<Option<Arc<maintenance::MaintenanceWorkerEntryHook>>>,
    #[cfg(test)]
    quiesce_slot_observation: Mutex<Option<std::sync::mpsc::Sender<Option<usize>>>>,
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

/// Shared lifecycle owner. Cloning this handle does not copy indexed data.
#[derive(Clone)]
pub struct Index(Arc<IndexInner>);

impl Index {
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
}

pub(crate) mod publication;
pub mod session;

pub(crate) use publication::PublishedIndex;

mod view;

pub use view::{CollectionIndexObservation, IndexObservation, IndexView};

#[cfg(test)]
mod tests;
