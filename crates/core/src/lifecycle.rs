//! Pinned, revisioned publication boundary for a changing index.
//!
//! This module establishes immutable read views and typed identity. File
//! mutation, reconciliation and persistence are added in later lifecycle
//! phases; a wrapped static repository does not imply those operations exist.

use std::fmt;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use crate::inventory::{ActiveRecords, FileVersions};
use crate::model::{CollectionId, FileId, RecordSlot};
use crate::query::Query;
use crate::repository::{Repository, SearchHit, SearchResult};
use crate::streaming::{
    CancellationToken, ExecutionContext, ExecutionError, ResultScope, StreamControl, StreamError,
    StreamEvent, StreamOptions, StreamOutcome,
};
use crate::workload::ReturnMode;

pub use crate::inventory::FileVersion;


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


impl IndexStatus {
    pub fn dirty(&self) -> bool {
        self.saved_revision != Some(self.revision)
    }
}


/// One immutable publication. Later phases add bounded per-collection deltas;
/// the foundation contains one complete, static repository generation.
pub(crate) struct PublishedIndex {
    pub(crate) instance_id: u64,
    pub(crate) revision: IndexRevision,
    pub(crate) repository: Arc<Repository>,
    pub(crate) active: Vec<ActiveRecords>,
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
        let active = repository
            .collections()
            .iter()
            .map(|collection| ActiveRecords::all(collection.generation.records.len()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(IndexError::ResourceLimit)?;
        let file_versions =
            FileVersions::filled(repository.files().files.len(), FileVersion(revision.0));
        Ok(Self {
            instance_id,
            revision,
            repository,
            active,
            file_versions,
            saved_revision: None,
            reconciliation: ReconciliationState::NotStarted,
        })
    }

    fn status(&self) -> IndexStatus {
        IndexStatus {
            instance_id: self.instance_id,
            revision: self.revision,
            saved_revision: self.saved_revision,
            reconciliation: self.reconciliation,
            indexed_files: self.file_versions.len(),
            live_records: self.active.iter().map(ActiveRecords::len).sum(),
        }
    }
}


struct IndexInner {
    instance_id: u64,
    published: RwLock<Arc<PublishedIndex>>,
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
        Ok(self.view()?.0.status())
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
    pub fn instance_id(&self) -> u64 {
        self.0.instance_id
    }

    pub fn revision(&self) -> IndexRevision {
        self.0.revision
    }

    pub fn file_handle(&self, file: FileId) -> Option<FileHandle> {
        self.0.file_versions.get(file).map(|version| FileHandle {
            instance_id: self.0.instance_id,
            file_id: file,
            version,
        })
    }

    pub fn file_version(&self, file: FileId) -> Option<FileVersion> {
        self.0.file_versions.get(file)
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

    pub(crate) fn repository(&self) -> &Repository {
        &self.0.repository
    }

    pub fn search(&self, query: &Query) -> Result<SearchResult, String> {
        let resolved = query.resolve_current_clock()?;
        crate::run::search_active(self.repository(), resolved.as_ref(), &self.0.active)
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
        crate::run::search_with_context_active(
            self.repository(),
            &resolved,
            context,
            &self.0.active,
        )
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
        self.repository().get_record_with_active(
            collection,
            record_id,
            return_mode,
            Some(&self.0.active),
        )
    }

    pub fn search_reference(&self, query: &Query) -> Result<SearchResult, String> {
        self.repository()
            .search_reference_with_active(query, Some(&self.0.active))
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
