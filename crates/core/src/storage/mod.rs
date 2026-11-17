//! Explicit catalog-led on-disk snapshots for immutable publications.

use std::num::{NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::lifecycle::{Index, IndexError, IndexRevision};
use crate::repository::Repository;

mod format;
mod model;
mod reader;
pub(crate) mod serde_support;
mod writer;

pub use format::{CacheTrimReport, StorageError};
pub use reader::{
    AuditOptions, AuditReport, PruneReport, audit, prune,
};

const DEFAULT_MANIFEST_BODY_LIMIT: u64 = 16 * 1024 * 1024;
const DEFAULT_CATALOG_BODY_LIMIT: u64 = 256 * 1024 * 1024;
const DEFAULT_TOTAL_EAGER_BYTES_LIMIT: u64 = 2 * 1024 * 1024 * 1024;
const DEFAULT_DICTIONARY_BODY_LIMIT: u64 = 256 * 1024 * 1024;
const DEFAULT_ROARING_LEAF_LIMIT: u64 = 64 * 1024 * 1024;
const DEFAULT_DECODED_LEAF_LIMIT: u64 = 128 * 1024 * 1024;
const DEFAULT_COALESCED_READ_SPAN: u64 = 1024 * 1024;
const DEFAULT_COALESCED_READ_GAP: u64 = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CachePolicy {
    KeepLoaded,
    Budget { resident_bytes: NonZeroU64 },
}

impl Default for CachePolicy {
    fn default() -> Self {
        Self::KeepLoaded
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StorageReadLimits {
    pub manifest_body_bytes: u64,
    pub eager_catalog_body_bytes: u64,
    pub total_eager_catalog_bytes: u64,
    pub lazy_dictionary_body_bytes: u64,
    pub serialized_leaf_bytes: u64,
    pub decoded_leaf_bytes: u64,
    pub coalesced_read_span_bytes: u64,
    pub coalesced_read_gap_bytes: u64,
}

impl Default for StorageReadLimits {
    fn default() -> Self {
        Self {
            manifest_body_bytes: DEFAULT_MANIFEST_BODY_LIMIT,
            eager_catalog_body_bytes: DEFAULT_CATALOG_BODY_LIMIT,
            total_eager_catalog_bytes: DEFAULT_TOTAL_EAGER_BYTES_LIMIT,
            lazy_dictionary_body_bytes: DEFAULT_DICTIONARY_BODY_LIMIT,
            serialized_leaf_bytes: DEFAULT_ROARING_LEAF_LIMIT,
            decoded_leaf_bytes: DEFAULT_DECODED_LEAF_LIMIT,
            coalesced_read_span_bytes: DEFAULT_COALESCED_READ_SPAN,
            coalesced_read_gap_bytes: DEFAULT_COALESCED_READ_GAP,
        }
    }
}

impl StorageReadLimits {
    fn validate(self) -> Result<Self, StorageError> {
        if [
            self.manifest_body_bytes,
            self.eager_catalog_body_bytes,
            self.total_eager_catalog_bytes,
            self.lazy_dictionary_body_bytes,
            self.serialized_leaf_bytes,
            self.decoded_leaf_bytes,
            self.coalesced_read_span_bytes,
            self.coalesced_read_gap_bytes,
        ]
        .contains(&0)
        {
            return Err(StorageError::limit(
                "storage read limits must all be nonzero",
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StorageWriteOptions {
    pub archive_workers: NonZeroUsize,
}

impl Default for StorageWriteOptions {
    fn default() -> Self {
        Self {
            archive_workers: NonZeroUsize::new(1).expect("one is nonzero"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageOpenOptions {
    pub cache_policy: CachePolicy,
    pub max_concurrent_reads: NonZeroUsize,
    pub limits: StorageReadLimits,
}

impl Default for StorageOpenOptions {
    fn default() -> Self {
        Self {
            cache_policy: CachePolicy::KeepLoaded,
            max_concurrent_reads: NonZeroUsize::new(4).expect("four is nonzero"),
            limits: StorageReadLimits::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum StorageEncoding {
    Native,
    Archived,
    Stored,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StorageWriteReport {
    pub generation: u64,
    pub logical_revision: IndexRevision,
    pub unchanged: bool,
    pub archive_report: crate::lifecycle::ArchiveReport,
    pub objects_new: usize,
    pub objects_reused: usize,
    pub bytes_serialized: u64,
    pub bytes_written: u64,
    pub sync_calls: u64,
    pub archive_ns: u64,
    pub encode_ns: u64,
    pub write_ns: u64,
    pub sync_ns: u64,
    pub total_ns: u64,
    pub manifest_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StorageStatus {
    pub directory: PathBuf,
    pub generation: u64,
    pub logical_revision: IndexRevision,
    pub store_identity: [u8; 16],
    pub lineage_identity: [u8; 16],
    pub eager_file_rows: usize,
    pub eager_record_rows: usize,
    pub eager_index_descriptors: usize,
    pub eager_leaf_rows: usize,
    pub stored_objects: usize,
    pub referenced_bytes: u64,
    pub cache: crate::storage::format::CacheStatus,
    pub source_reads: u64,
    pub source_parses: u64,
    pub index_reads: u64,
    pub index_bytes: u64,
}

impl Index {
    pub fn archive_to(
        &self,
        directory: impl AsRef<Path>,
        options: StorageWriteOptions,
    ) -> Result<StorageWriteReport, IndexError> {
        writer::archive_index_to(self, directory.as_ref(), options)
    }

    pub fn open_storage(
        source_root: impl Into<PathBuf>,
        definitions: Vec<crate::repository::CollectionDefinition>,
        repository_options: crate::repository::RepositoryOptions,
        directory: impl AsRef<Path>,
        options: StorageOpenOptions,
    ) -> Result<Self, IndexError> {
        Self::open_storage_with_sources(
            source_root,
            definitions,
            repository_options,
            crate::source::SourceRegistry::builtin(),
            directory,
            options,
        )
    }

    pub fn open_storage_with_sources(
        source_root: impl Into<PathBuf>,
        definitions: Vec<crate::repository::CollectionDefinition>,
        repository_options: crate::repository::RepositoryOptions,
        sources: crate::source::SourceRegistry,
        directory: impl AsRef<Path>,
        options: StorageOpenOptions,
    ) -> Result<Self, IndexError> {
        reader::open_index(
            source_root.into(),
            definitions,
            repository_options,
            sources,
            directory.as_ref(),
            options,
        )
    }

    pub fn storage_status(&self) -> Result<StorageStatus, IndexError> {
        reader::index_status(self)
    }

    pub fn trim_storage_cache(&self) -> Result<CacheTrimReport, IndexError> {
        reader::index_trim_cache(self)
    }
}

impl Repository {
    pub fn archive_to(
        &mut self,
        directory: impl AsRef<Path>,
        options: StorageWriteOptions,
    ) -> Result<StorageWriteReport, IndexError> {
        writer::archive_repository_to(self, directory.as_ref(), options)
    }

    pub fn open_storage(
        source_root: impl Into<PathBuf>,
        definitions: Vec<crate::repository::CollectionDefinition>,
        repository_options: crate::repository::RepositoryOptions,
        directory: impl AsRef<Path>,
        options: StorageOpenOptions,
    ) -> Result<Self, IndexError> {
        Self::open_storage_with_sources(
            source_root,
            definitions,
            repository_options,
            crate::source::SourceRegistry::builtin(),
            directory,
            options,
        )
    }

    pub fn open_storage_with_sources(
        source_root: impl Into<PathBuf>,
        definitions: Vec<crate::repository::CollectionDefinition>,
        repository_options: crate::repository::RepositoryOptions,
        sources: crate::source::SourceRegistry,
        directory: impl AsRef<Path>,
        options: StorageOpenOptions,
    ) -> Result<Self, IndexError> {
        reader::open_repository(
            source_root.into(),
            definitions,
            repository_options,
            sources,
            directory.as_ref(),
            options,
        )
    }

    pub fn storage_status(&self) -> Result<StorageStatus, IndexError> {
        reader::repository_status(self)
    }

    pub fn trim_storage_cache(&self) -> Result<CacheTrimReport, IndexError> {
        reader::repository_trim_cache(self)
    }
}

pub(crate) fn storage_to_index_error(error: StorageError) -> IndexError {
    match error {
        StorageError::IncompleteBuild => IndexError::IncompleteBuild,
        StorageError::IncompatibleConfiguration(message) => {
            IndexError::IncompatibleConfiguration(message)
        }
        StorageError::Corrupt { message, .. }
        | StorageError::InvalidFormat(message)
        | StorageError::UnsupportedComponent(message) => {
            IndexError::InvalidPublication(message)
        }
        other => IndexError::Io(other.to_string()),
    }
}

pub(crate) struct StorageSession {
    directory: PathBuf,
    generation: u64,
    cache: format::StorageCache,
    _readers_lock: std::fs::File,
    store_identity: [u8; 16],
    lineage_identity: [u8; 16],
    manifest_digest: [u8; 32],
    manifest_length: u64,
    limits: StorageReadLimits,
    index_reads: std::sync::atomic::AtomicU64,
    index_bytes: std::sync::atomic::AtomicU64,
}

#[derive(Clone, Debug)]
pub(crate) struct StorageBinding {
    pub directory: PathBuf,
    pub generation: u64,
    pub manifest_digest: [u8; 32],
    pub manifest_length: u64,
    pub store_identity: [u8; 16],
    pub lineage_identity: [u8; 16],
    pub revision: IndexRevision,
    pub representation_generation: u64,
    pub configuration_identity: [u8; 32],
}

pub(crate) type SharedStorageBinding = Arc<Mutex<Option<StorageBinding>>>;

impl StorageSession {
    pub(crate) fn directory(&self) -> &Path {
        &self.directory
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn cache(&self) -> &format::StorageCache {
        &self.cache
    }

    pub(crate) fn identities(&self) -> ([u8; 16], [u8; 16]) {
        (self.store_identity, self.lineage_identity)
    }

    pub(crate) fn manifest_token(&self) -> ([u8; 32], u64) {
        (self.manifest_digest, self.manifest_length)
    }

    pub(crate) fn limits(&self) -> StorageReadLimits {
        self.limits
    }
}

pub(crate) type SharedStorageSession = Arc<StorageSession>;

