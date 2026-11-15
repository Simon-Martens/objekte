//! Authoritative in-memory index storage.
//!
//! One [`IndexStore`] owns a single redb database created with
//! [`redb::InMemoryBackend`]. Every index family (catalog metadata, record
//! identity, structured memberships, configured sort orders, relations and
//! FTS blocks) lives in that database. Queries pin one [`StoreView`] read
//! transaction and keep the owner alive through an `Arc`.
//!
//! No file-backed database, checkpoint, export or disk spill exists in this
//! module. Source files are read by the repository layer; the store never
//! writes them.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use redb::backends::InMemoryBackend;
use redb::{Database, ReadTransaction, ReadableDatabase, WriteTransaction};

pub mod construction;
pub mod contributions;
pub mod fts;
#[cfg(feature = "store-feasibility")]
pub mod feasibility;
pub mod key_codec;
pub(crate) mod keys;
pub mod mutations;
pub mod read;
pub mod regeneration;
pub mod relations;
pub mod slot_allocator;
pub mod sorting;
pub mod tables;
pub mod validation;
pub mod value_codec;
pub mod write;

use tables::{ALL_TABLES, STORE_METADATA};
use value_codec::{StoreHeader, StoreMetadata};

static NEXT_STORE_INSTANCE: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_instance_id() -> u64 {
    NEXT_STORE_INSTANCE.fetch_add(1, Ordering::Relaxed)
}

/// Storage failures. Every variant propagates to the caller; none is converted
/// into an empty result or an absent field.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreError {
    /// A redb transaction, table or backend operation failed.
    Database(String),
    /// A stored record failed structural decoding or an invariant.
    CorruptIndex(String),
    /// A caller used a handle that is not valid for the target snapshot.
    StaleHandle(String),
    /// The store was explicitly faulted after an uncertain commit outcome.
    Faulted(String),
    /// An operation is not meaningful in the current construction state.
    State(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(message) => write!(formatter, "index store database error: {message}"),
            Self::CorruptIndex(message) => write!(formatter, "corrupt index: {message}"),
            Self::StaleHandle(message) => write!(formatter, "stale index handle: {message}"),
            Self::Faulted(message) => write!(formatter, "faulted index store: {message}"),
            Self::State(message) => write!(formatter, "index store state: {message}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl StoreError {
    pub(crate) fn database(error: impl fmt::Display) -> Self {
        Self::Database(error.to_string())
    }
}

pub type StoreResult<T> = Result<T, StoreError>;

/// Explicit configuration for one store. The cache limit is a redb page-cache
/// setting, not a total process-memory bound.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoreConfig {
    pub cache_bytes: usize,
    pub configuration_digest: [u8; 32],
    pub source_root: Vec<u8>,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            cache_bytes: tables::PAGE_CACHE_BYTES,
            configuration_digest: [0; 32],
            source_root: Vec::new(),
        }
    }
}

/// Owning handle for the one authoritative database of an index instance.
pub struct IndexStore {
    database: Arc<Database>,
    instance_id: u64,
    config: StoreConfig,
    /// Serializes coordinator writes. redb itself allows only one write
    /// transaction at a time; this lock keeps coordinator bookkeeping and
    /// failure handling in one place.
    writer: Mutex<()>,
}

impl fmt::Debug for IndexStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IndexStore")
            .field("instance_id", &self.instance_id)
            .field("cache_bytes", &self.config.cache_bytes)
            .finish_non_exhaustive()
    }
}

impl IndexStore {
    /// Create an empty in-memory store and initialize every registered table.
    pub fn new(config: StoreConfig) -> StoreResult<Self> {
        let mut builder = redb::Builder::new();
        builder.set_cache_size(config.cache_bytes);
        let database = builder
            .create_with_backend(InMemoryBackend::new())
            .map_err(StoreError::database)?;
        let instance_id = next_instance_id();
        let store = Self {
            database: Arc::new(database),
            instance_id,
            config,
            writer: Mutex::new(()),
        };
        store.initialize()?;
        Ok(store)
    }

    /// Create a store in a shared database for tests of reset and ownership.
    pub fn from_database(database: Arc<Database>, config: StoreConfig) -> StoreResult<Self> {
        let store = Self {
            database,
            instance_id: next_instance_id(),
            config,
            writer: Mutex::new(()),
        };
        store.initialize()?;
        Ok(store)
    }

    fn initialize(&self) -> StoreResult<()> {
        let write = self
            .database
            .begin_write()
            .map_err(StoreError::database)?;
        for (_, definition) in ALL_TABLES {
            write
                .open_table(*definition)
                .map_err(StoreError::database)?;
        }
        let metadata = StoreMetadata::empty(
            self.instance_id,
            self.config.configuration_digest,
            self.config.source_root.clone(),
        );
        let encoded = value_codec::encode(&metadata)?;
        let mut table = write
            .open_table(STORE_METADATA)
            .map_err(StoreError::database)?;
        table
            .insert(tables::meta::FORMAT, encoded.as_slice())
            .map_err(StoreError::database)?;
        drop(table);
        write.commit().map_err(StoreError::database)?;
        Ok(())
    }

    pub fn instance_id(&self) -> u64 {
        self.instance_id
    }

    pub fn config(&self) -> &StoreConfig {
        &self.config
    }

    /// Begin one read transaction and return a view that keeps the database
    /// owner alive. The revision is read from the same transaction.
    pub fn view(self: &Arc<Self>) -> StoreResult<StoreView> {
        let read = self
            .database
            .begin_read()
            .map_err(StoreError::database)?;
        let metadata = read_metadata(&read)?;
        let generation = read::collection_generation_sum(&read)?;
        Ok(StoreView {
            owner: Arc::clone(self),
            read,
            revision: metadata.revision,
            build_epoch: metadata.build_epoch,
            generation,
            metadata,
        })
    }

    pub fn begin_write(&self) -> StoreResult<WriteTransaction> {
        let _guard = self
            .writer
            .lock()
            .map_err(|_| StoreError::State("index store writer lock poisoned".into()))?;
        self.database
            .begin_write()
            .map_err(StoreError::database)
    }

    /// Read the current metadata without holding a long-lived view.
    pub fn metadata(&self) -> StoreResult<StoreMetadata> {
        let read = self
            .database
            .begin_read()
            .map_err(StoreError::database)?;
        read_metadata(&read)
    }

    pub(crate) fn database(&self) -> &Arc<Database> {
        &self.database
    }
}

/// One snapshot-bound read view.
pub struct StoreView {
    owner: Arc<IndexStore>,
    pub(crate) read: ReadTransaction,
    pub(crate) revision: u64,
    pub(crate) build_epoch: u64,
    pub(crate) generation: u64,
    pub(crate) metadata: StoreMetadata,
}

impl fmt::Debug for StoreView {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoreView")
            .field("revision", &self.revision)
            .field("build_epoch", &self.build_epoch)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl StoreView {
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn build_epoch(&self) -> u64 {
        self.build_epoch
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn instance_id(&self) -> u64 {
        self.owner.instance_id()
    }

    pub fn metadata(&self) -> &StoreMetadata {
        &self.metadata
    }

    pub fn owner(&self) -> &Arc<IndexStore> {
        &self.owner
    }

    pub fn read(&self) -> &ReadTransaction {
        &self.read
    }

}

pub(crate) fn read_metadata(read: &ReadTransaction) -> StoreResult<StoreMetadata> {
    let table = read
        .open_table(STORE_METADATA)
        .map_err(StoreError::database)?;
    let value = table
        .get(tables::meta::FORMAT)
        .map_err(StoreError::database)?
        .ok_or_else(|| StoreError::CorruptIndex("missing store metadata row".into()))?;
    value_codec::decode::<StoreMetadata>(value.value(), "store metadata")
}

/// The header is stored inside the metadata record; this helper exists for
/// compatibility checks that only need the format identity.
pub fn header_matches(read: &ReadTransaction) -> StoreResult<bool> {
    let metadata = read_metadata(read)?;
    let expected = StoreHeader {
        format_version: tables::STORE_FORMAT_VERSION,
        scalar_codec_version: tables::SCALAR_CODEC_VERSION,
        fts_codec_version: tables::FTS_CODEC_VERSION,
        collation_version: tables::COLLATION_VERSION.into(),
        instance_id: metadata.header.instance_id,
    };
    Ok(metadata.header == expected)
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
