//! Optional RocksDB storage prototype.
//!
//! One pessimistic `TransactionDB` owns all logical namespaces in one default
//! column family. Views hold a native RocksDB snapshot and borrow the database
//! owner; no self-reference or leaked database is used.

use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use rocksdb::{
    BlockBasedOptions, Cache, DBCompressionType, DBRawIteratorWithThreadMode, Env, Options,
    ReadOptions, SingleThreaded, SnapshotWithThreadMode, Transaction,
    TransactionDB, TransactionDBOptions, TransactionOptions, WriteOptions,
};

pub mod bitmap_block;
pub mod contribution_codec;
pub mod key_codec;
pub mod namespace;
pub mod value_codec;

use namespace::Namespace;
use value_codec::StoreHeader;

static NEXT_INSTANCE: AtomicU64 = AtomicU64::new(1);
const HEADER_LOGICAL_KEY: &[u8] = b"header";
const CACHE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreError {
    Database(String),
    Io(String),
    CorruptKey(String),
    CorruptValue(String),
    CorruptIndex(String),
    Incompatible(String),
    State(String),
    StaleHandle(String),
    NotFinalized(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(message) => write!(formatter, "RocksDB storage error: {message}"),
            Self::Io(message) => write!(formatter, "RocksDB prototype I/O error: {message}"),
            Self::CorruptKey(message) => write!(formatter, "corrupt prototype key: {message}"),
            Self::CorruptValue(message) => write!(formatter, "corrupt prototype value: {message}"),
            Self::CorruptIndex(message) => write!(formatter, "corrupt prototype index: {message}"),
            Self::Incompatible(message) => write!(formatter, "incompatible prototype: {message}"),
            Self::State(message) => write!(formatter, "invalid prototype state: {message}"),
            Self::StaleHandle(message) => write!(formatter, "stale prototype handle: {message}"),
            Self::NotFinalized(message) => {
                write!(formatter, "prototype index is not finalized: {message}")
            }
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rocksdb::Error> for StoreError {
    fn from(error: rocksdb::Error) -> Self {
        Self::Database(error.into_string())
    }
}

impl From<std::io::Error> for StoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreLocation {
    Memory,
    File(PathBuf),
}

#[derive(Clone, Debug)]
pub struct StoreConfig {
    pub location: StoreLocation,
    pub configuration_digest: [u8; 32],
    pub alphabet: String,
    pub source_root: Vec<u8>,
    pub build_identity: [u8; 32],
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            location: StoreLocation::Memory,
            configuration_digest: [0; 32],
            alphabet: "ascii".into(),
            source_root: Vec::new(),
            build_identity: [0; 32],
        }
    }
}

pub struct RocksStore {
    resources: Arc<StoreResources>,
}

impl fmt::Debug for RocksStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RocksStore")
            .field("instance_id", &self.instance_id())
            .field("location", &self.resources.config.location)
            .finish_non_exhaustive()
    }
}

struct StoreResources {
    db: TransactionDB<SingleThreaded>,
    _cache: Cache,
    _env: Env,
    config: StoreConfig,
    instance_id: u64,
    memory_directory: Option<MemoryDirectory>,
}

struct MemoryDirectory(PathBuf);

impl Drop for MemoryDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.0);
    }
}

impl RocksStore {
    pub fn create(config: StoreConfig) -> StoreResult<Self> {
        let instance_id = NEXT_INSTANCE.fetch_add(1, Ordering::Relaxed);
        let (path, memory_directory) = match &config.location {
            StoreLocation::Memory => {
                let path = std::env::temp_dir().join(format!(
                    "objekte-rocksdb-memory-{}-{instance_id}",
                    std::process::id()
                ));
                if path.exists() {
                    std::fs::remove_dir_all(&path)?;
                }
                std::fs::create_dir_all(&path)?;
                (path.clone(), Some(MemoryDirectory(path)))
            }
            StoreLocation::File(path) => {
                if path.exists() {
                    if !path.is_dir() {
                        return Err(StoreError::State(format!(
                            "build destination is not a directory: {}",
                            path.display()
                        )));
                    }
                    if std::fs::read_dir(path)?.next().is_some() {
                        return Err(StoreError::State(format!(
                            "refusing nonempty build destination: {}",
                            path.display()
                        )));
                    }
                }
                (path.clone(), None)
            }
        };

        let mut env = match config.location {
            StoreLocation::Memory => Env::mem_env()?,
            StoreLocation::File(_) => Env::new()?,
        };
        let background_jobs = std::thread::available_parallelism()
            .map(|count| count.get().min(2).max(1) as i32)
            .unwrap_or(1);
        env.set_background_threads(background_jobs);

        let cache = Cache::new_lru_cache(CACHE_BYTES);
        let mut block = BlockBasedOptions::default();
        block.set_block_size(4 * 1024);
        block.set_block_cache(&cache);
        block.set_bloom_filter(10.0, true);
        block.set_cache_index_and_filter_blocks(true);

        let mut options = Options::default();
        options.create_if_missing(true);
        options.set_env(&env);
        options.set_compression_type(DBCompressionType::Lz4);
        options.set_block_based_table_factory(&block);
        options.set_allow_concurrent_memtable_write(true);
        options.set_unordered_write(false);
        options.set_write_buffer_size(32 * 1024 * 1024);
        options.set_max_write_buffer_number(3);
        options.set_min_write_buffer_number_to_merge(1);
        options.set_level_zero_file_num_compaction_trigger(4);
        options.set_level_zero_slowdown_writes_trigger(20);
        options.set_level_zero_stop_writes_trigger(36);
        options.set_max_background_jobs(background_jobs);
        options.set_max_open_files(256);

        let mut transaction_db_options = TransactionDBOptions::default();
        transaction_db_options.set_num_stripes(64);
        transaction_db_options.set_default_lock_timeout(1_000);
        transaction_db_options.set_txn_lock_timeout(1_000);

        let db = TransactionDB::<SingleThreaded>::open(
            &options,
            &transaction_db_options,
            &path,
        )?;
        let header = StoreHeader::new(
            config.configuration_digest,
            config.alphabet.clone(),
            config.source_root.clone(),
            config.build_identity,
            if matches!(config.location, StoreLocation::Memory) {
                "memory"
            } else {
                "file"
            },
        );
        let header_key = Namespace::StoreMetadata.physical(HEADER_LOGICAL_KEY);
        if db.get(&header_key)?.is_some() {
            return Err(StoreError::State(
                "fresh store already contains prototype metadata".into(),
            ));
        }
        let write_options = write_options(matches!(config.location, StoreLocation::Memory));
        let transaction_options = transaction_options();
        let transaction = db.transaction_opt(&write_options, &transaction_options);
        transaction.put(&header_key, value_codec::encode(&header)?)?;
        transaction.commit()?;

        Ok(Self {
            resources: Arc::new(StoreResources {
                db,
                _cache: cache,
                _env: env,
                config,
                instance_id,
                memory_directory,
            }),
        })
    }

    pub fn reopen_file(path: impl Into<PathBuf>, config: StoreConfig) -> StoreResult<Self> {
        let path = path.into();
        if !path.is_dir() {
            return Err(StoreError::State(format!(
                "reopen path is not a directory: {}",
                path.display()
            )));
        }
        let config = StoreConfig {
            location: StoreLocation::File(path.clone()),
            ..config
        };
        let instance_id = NEXT_INSTANCE.fetch_add(1, Ordering::Relaxed);
        let mut env = Env::new()?;
        let background_jobs = std::thread::available_parallelism()
            .map(|count| count.get().min(2).max(1) as i32)
            .unwrap_or(1);
        env.set_background_threads(background_jobs);
        let cache = Cache::new_lru_cache(CACHE_BYTES);
        let mut block = BlockBasedOptions::default();
        block.set_block_size(4 * 1024);
        block.set_block_cache(&cache);
        block.set_bloom_filter(10.0, true);
        block.set_cache_index_and_filter_blocks(true);
        let mut options = Options::default();
        options.create_if_missing(false);
        options.set_env(&env);
        options.set_compression_type(DBCompressionType::Lz4);
        options.set_block_based_table_factory(&block);
        options.set_allow_concurrent_memtable_write(true);
        options.set_unordered_write(false);
        options.set_write_buffer_size(32 * 1024 * 1024);
        options.set_max_write_buffer_number(3);
        options.set_min_write_buffer_number_to_merge(1);
        options.set_level_zero_file_num_compaction_trigger(4);
        options.set_level_zero_slowdown_writes_trigger(20);
        options.set_level_zero_stop_writes_trigger(36);
        options.set_max_background_jobs(background_jobs);
        options.set_max_open_files(256);
        let mut transaction_db_options = TransactionDBOptions::default();
        transaction_db_options.set_num_stripes(64);
        transaction_db_options.set_default_lock_timeout(1_000);
        transaction_db_options.set_txn_lock_timeout(1_000);
        let db = TransactionDB::<SingleThreaded>::open(
            &options,
            &transaction_db_options,
            &path,
        )?;
        let header_key = Namespace::StoreMetadata.physical(HEADER_LOGICAL_KEY);
        let bytes = db.get(&header_key)?.ok_or_else(|| {
            StoreError::Incompatible("missing RocksDB prototype metadata".into())
        })?;
        let header: StoreHeader = value_codec::decode(&bytes, "store header")?;
        header.validate()?;
        if header.storage_mode != "file" {
            return Err(StoreError::Incompatible(
                "database header is not in file mode".into(),
            ));
        }
        Ok(Self {
            resources: Arc::new(StoreResources {
                db,
                _cache: cache,
                _env: env,
                config,
                instance_id,
                memory_directory: None,
            }),
        })
    }

    pub fn instance_id(&self) -> u64 {
        self.resources.instance_id
    }

    pub fn config(&self) -> &StoreConfig {
        &self.resources.config
    }

    pub fn view(&self) -> StoreResult<StoreView<'_>> {
        let snapshot = self.resources.db.snapshot();
        let header_key = Namespace::StoreMetadata.physical(HEADER_LOGICAL_KEY);
        let bytes = snapshot.get(&header_key)?.ok_or_else(|| {
            StoreError::Incompatible("missing RocksDB prototype metadata".into())
        })?;
        let header: StoreHeader = value_codec::decode(&bytes, "store header")?;
        header.validate()?;
        Ok(StoreView {
            resources: &self.resources,
            sequence_number: snapshot.sequence_number(),
            snapshot,
            header,
        })
    }

    pub fn flush_wal(&self, sync: bool) -> StoreResult<()> {
        self.resources.db.flush_wal(sync)?;
        Ok(())
    }

    pub fn flush_memtable(&self) -> StoreResult<()> {
        let mut options = rocksdb::FlushOptions::default();
        options.set_wait(true);
        self.resources.db.flush_opt(&options)?;
        Ok(())
    }

    pub fn transaction(&self) -> Transaction<'_, TransactionDB<SingleThreaded>> {
        let write = write_options(matches!(
            self.resources.config.location,
            StoreLocation::Memory
        ));
        let transaction = transaction_options();
        // Both option objects only initialize the native transaction.
        self.resources
            .db
            .transaction_opt(&write, &transaction)
    }

    pub fn memory_host_directory(&self) -> Option<&std::path::Path> {
        self.resources
            .memory_directory
            .as_ref()
            .map(|directory| directory.0.as_path())
    }
}

fn write_options(memory: bool) -> WriteOptions {
    let mut options = WriteOptions::default();
    options.disable_wal(memory);
    options.set_sync(false);
    options
}

fn transaction_options() -> TransactionOptions {
    let mut options = TransactionOptions::default();
    options.set_snapshot(false);
    options.set_deadlock_detect(true);
    options.set_lock_timeout(1_000);
    options
}

pub struct StoreView<'db> {
    resources: &'db StoreResources,
    snapshot: SnapshotWithThreadMode<'db, TransactionDB<SingleThreaded>>,
    sequence_number: u64,
    header: StoreHeader,
}

impl<'db> StoreView<'db> {
    pub fn sequence_number(&self) -> u64 {
        self.sequence_number
    }

    pub fn header(&self) -> &StoreHeader {
        &self.header
    }

    pub fn instance_id(&self) -> u64 {
        self.resources.instance_id
    }

    pub fn get(&self, namespace: Namespace, logical_key: &[u8]) -> StoreResult<Option<Vec<u8>>> {
        let key = namespace.physical(logical_key);
        self.snapshot.get(key).map_err(StoreError::from)
    }

    pub fn get_pinned(
        &'db self,
        namespace: Namespace,
        logical_key: &[u8],
    ) -> StoreResult<Option<rocksdb::DBPinnableSlice<'db>>> {
        let key = namespace.physical(logical_key);
        self.snapshot
            .get_pinned(key)
            .map_err(StoreError::from)
    }

    pub fn iterator(
        &'db self,
        namespace: Namespace,
        lower: Option<&[u8]>,
        upper: Option<&[u8]>,
    ) -> StoreResult<DBRawIteratorWithThreadMode<'db, TransactionDB<SingleThreaded>>> {
        let mut options = ReadOptions::default();
        if let Some(lower) = lower {
            options.set_iterate_lower_bound(namespace.physical(lower));
        }
        if let Some(upper) = upper {
            options.set_iterate_upper_bound(namespace.physical(upper));
        }
        Ok(self.snapshot.raw_iterator_opt(options))
    }

    pub fn namespace_prefix(
        &'db self,
        namespace: Namespace,
        logical_prefix: &[u8],
    ) -> StoreResult<DBRawIteratorWithThreadMode<'db, TransactionDB<SingleThreaded>>> {
        let lower = namespace.physical(logical_prefix);
        let upper = key_codec::prefix_successor(logical_prefix)
            .map(|upper| namespace.physical(&upper));
        let mut options = ReadOptions::default();
        options.set_iterate_lower_bound(lower);
        if let Some(upper) = upper {
            options.set_iterate_upper_bound(upper);
        } else {
            options.set_iterate_upper_bound([namespace.tag().saturating_add(1)]);
        }
        Ok(self.snapshot.raw_iterator_opt(options))
    }
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
