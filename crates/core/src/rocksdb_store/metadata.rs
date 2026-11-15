//! Bounded put/delete batching shared by discovery and finalization.

use std::collections::BTreeMap;
use std::time::Instant;

use serde::Serialize;

use super::{RocksStore, StoreError, StoreResult};

pub(crate) const MAX_METADATA_BATCH_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_METADATA_BATCH_OPS: usize = 32_768;
const WRITE_RECORD_OVERHEAD_BYTES: usize = 32;

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct MetadataWriteStats {
    pub batches: u64,
    pub puts: u64,
    pub deletes: u64,
    pub bytes: u64,
    pub max_batch_bytes: usize,
    pub max_batch_ops: usize,
    pub commit_ns: u64,
}

pub(crate) struct MetadataWriter<'a> {
    store: &'a RocksStore,
    ops: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
    bytes: usize,
    max_bytes: usize,
    max_ops: usize,
    stats: MetadataWriteStats,
}

impl<'a> MetadataWriter<'a> {
    pub(crate) fn new(store: &'a RocksStore) -> Self {
        Self {
            store,
            ops: BTreeMap::new(),
            bytes: 0,
            max_bytes: MAX_METADATA_BATCH_BYTES,
            max_ops: MAX_METADATA_BATCH_OPS,
            stats: MetadataWriteStats::default(),
        }
    }

    #[cfg(test)]
    fn with_limits(store: &'a RocksStore, max_bytes: usize, max_ops: usize) -> Self {
        Self {
            store,
            ops: BTreeMap::new(),
            bytes: 0,
            max_bytes,
            max_ops,
            stats: MetadataWriteStats::default(),
        }
    }

    pub(crate) fn put(&mut self, key: Vec<u8>, value: Vec<u8>) -> StoreResult<()> {
        self.operation(key, Some(value))
    }

    pub(crate) fn delete(&mut self, key: Vec<u8>) -> StoreResult<()> {
        self.operation(key, None)
    }

    pub(crate) fn put_group(
        &mut self,
        operations: impl IntoIterator<Item = (Vec<u8>, Option<Vec<u8>>)>,
    ) -> StoreResult<()> {
        let operations = operations.into_iter().collect::<Vec<_>>();
        let bytes = operations.iter().fold(0usize, |bytes, op| {
            bytes.saturating_add(Self::operation_bytes(&op.0, op.1.as_deref()))
        });
        if !operations.is_empty()
            && self.ops.len().saturating_add(operations.len()) > self.max_ops
            && !self.ops.is_empty()
        {
            self.flush()?;
        }
        if !operations.is_empty()
            && self.bytes.saturating_add(bytes) > self.max_bytes
            && !self.ops.is_empty()
        {
            self.flush()?;
        }
        for (key, value) in operations {
            self.bytes = self
                .bytes
                .saturating_add(Self::operation_bytes(&key, value.as_deref()));
            self.ops.insert(key, value);
        }
        if self.ops.len() >= self.max_ops || self.bytes >= self.max_bytes {
            self.flush()?;
        }
        Ok(())
    }

    pub(crate) fn flush(&mut self) -> StoreResult<()> {
        if self.ops.is_empty() {
            return Ok(());
        }
        let batch_bytes = self.bytes;
        let batch_ops = self.ops.len();
        let started = Instant::now();
        let transaction = self.store.transaction();
        let mut puts = 0u64;
        let mut deletes = 0u64;
        for (key, value) in &self.ops {
            if let Some(value) = value {
                transaction.put(key, value)?;
                puts += 1;
            } else {
                transaction.delete(key)?;
                deletes += 1;
            }
        }
        transaction.commit()?;
        self.stats.commit_ns = self
            .stats
            .commit_ns
            .saturating_add(started.elapsed().as_nanos() as u64);
        self.stats.batches += 1;
        self.stats.puts += puts;
        self.stats.deletes += deletes;
        self.stats.bytes = self.stats.bytes.saturating_add(batch_bytes as u64);
        self.stats.max_batch_bytes = self.stats.max_batch_bytes.max(batch_bytes);
        self.stats.max_batch_ops = self.stats.max_batch_ops.max(batch_ops);
        self.ops.clear();
        self.bytes = 0;
        Ok(())
    }

    pub(crate) fn stats(&self) -> MetadataWriteStats {
        self.stats
    }

    fn operation(&mut self, key: Vec<u8>, value: Option<Vec<u8>>) -> StoreResult<()> {
        let bytes = Self::operation_bytes(&key, value.as_deref());
        if bytes > self.max_bytes {
            return Err(StoreError::State(format!(
                "single metadata operation requires {bytes} bytes"
            )));
        }
        if !self.ops.is_empty()
            && (self.ops.len() >= self.max_ops || self.bytes.saturating_add(bytes) > self.max_bytes)
        {
            self.flush()?;
        }
        self.bytes = self.bytes.saturating_add(bytes);
        self.ops.insert(key, value);
        if self.ops.len() >= self.max_ops || self.bytes >= self.max_bytes {
            self.flush()?;
        }
        Ok(())
    }

    fn operation_bytes(key: &[u8], value: Option<&[u8]>) -> usize {
        key.len()
            .saturating_add(value.map_or(0, <[u8]>::len))
            .saturating_add(WRITE_RECORD_OVERHEAD_BYTES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rocksdb_store::{StoreConfig, StoreLocation};

    fn store() -> RocksStore {
        RocksStore::create(StoreConfig {
            location: StoreLocation::Memory,
            ..StoreConfig::default()
        })
        .unwrap()
    }

    #[test]
    fn tiny_limits_bound_mixed_puts_and_deletes() {
        let store = store();
        let mut writer = MetadataWriter::with_limits(&store, 160, 3);
        for index in 0u32..12 {
            writer
                .put(vec![0x7f, index as u8], vec![index as u8; 24])
                .unwrap();
        }
        for index in 0u32..4 {
            writer.delete(vec![0x7f, index as u8]).unwrap();
        }
        writer.flush().unwrap();
        let stats = writer.stats();
        assert!(stats.batches >= 4);
        assert!(stats.max_batch_bytes <= 160);
        assert!(stats.max_batch_ops <= 3);
        assert_eq!(stats.puts, 12);
        assert_eq!(stats.deletes, 4);
    }

    #[test]
    fn atomic_groups_are_isolated_when_oversized() {
        let store = store();
        let mut writer = MetadataWriter::with_limits(&store, 80, 2);
        writer.put(vec![1], vec![1; 20]).unwrap();
        writer
            .put_group([(vec![2], Some(vec![2; 80])), (vec![3], Some(vec![3; 80]))])
            .unwrap();
        writer.flush().unwrap();
        assert_eq!(writer.stats().batches, 2);
        assert!(writer.stats().max_batch_ops <= 2);
    }
}
