//! redb-backed checkpointing and loading for a published index.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use redb::{Database, Durability, ReadableDatabase, ReadableTable, WriteTransaction};
use serde::{Deserialize, Serialize};

use crate::index_store_keys as keys;
use crate::index_store_shard::{read_shard, write_shard};
use crate::inventory::{FileEntry, FileVersions};
use crate::lifecycle::{Index, IndexError, IndexRevision, PublishedIndex};
use crate::repository::{CollectionDefinition, GlobalInferenceSummary, Repository, RepositoryOptions};
use crate::source::SourceRegistry;

pub(crate) type CatalogSlot = (FileEntry, Option<[u8; 32]>, Option<[u8; 32]>);

pub(crate) struct IndexStore {
    database: Database,
    pub(crate) path: PathBuf,
    durable: bool,
    revision: u64,
    next_segment_ordinal: u64,
    next_store_id: u64,
    #[cfg(test)]
    fail_next_commit: bool,
}

pub(crate) struct StoreContents {
    pub revision: u64,
    pub next_segment_ordinal: u64,
    pub next_store_id: u64,
    pub shards: Vec<(u64, crate::build::IndexedCollectionSegment, crate::inventory::ActiveRecords)>,
    pub catalog: Vec<Option<CatalogSlot>>,
    pub global_inference: Vec<Option<GlobalInferenceSummary>>,
}

pub(crate) enum StoreOpenError {
    Locked(String),
    Damaged(String),
}

fn store_error(error: impl std::fmt::Display) -> String {
    format!("index store: {error}")
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    bincode::serialize(value).map_err(store_error)
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    bincode::deserialize(bytes).map_err(store_error)
}

fn le_u64(bytes: &[u8]) -> Option<u64> {
    bytes.get(..8).map(|bytes| u64::from_le_bytes(bytes.try_into().unwrap()))
}

fn meta_get(database: &Database, key: &str) -> Result<Option<Vec<u8>>, String> {
    let read = database.begin_read().map_err(store_error)?;
    let table = match read.open_table(keys::META) {
        Ok(table) => table,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
        Err(error) => return Err(store_error(error)),
    };
    Ok(table
        .get(key)
        .map_err(store_error)?
        .map(|value| value.value().to_vec()))
}

fn meta_bytes(database: &Database, key: &str) -> Result<Option<Vec<u8>>, StoreOpenError> {
    meta_get(database, key).map_err(StoreOpenError::Damaged)
}

impl IndexStore {
    pub(crate) fn open(path: &Path, durable: bool) -> Result<Self, StoreOpenError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| StoreOpenError::Damaged(store_error(error)))?;
        }
        let database = Database::create(path).map_err(|error| {
            let text = error.to_string();
            if text.to_ascii_lowercase().contains("already open") {
                StoreOpenError::Locked(store_error(error))
            } else {
                StoreOpenError::Damaged(store_error(error))
            }
        })?;
        let revision = meta_bytes(&database, "revision")?
            .as_deref()
            .and_then(le_u64)
            .unwrap_or(0);
        let next_segment_ordinal = meta_bytes(&database, "next_segment_ordinal")?
            .as_deref()
            .and_then(le_u64)
            .unwrap_or(1);
        let next_store_id = meta_bytes(&database, "next_store_id")?
            .as_deref()
            .and_then(le_u64)
            .unwrap_or(1);
        Ok(Self {
            database,
            path: path.to_path_buf(),
            durable,
            revision,
            next_segment_ordinal,
            next_store_id,
            #[cfg(test)]
            fail_next_commit: false,
        })
    }

    #[cfg(test)]
    pub(crate) fn open_in_memory() -> Self {
        let database = Database::builder()
            .create_with_backend(redb::backends::InMemoryBackend::new())
            .unwrap();
        Self {
            database,
            path: PathBuf::from(":memory:"),
            durable: false,
            revision: 0,
            next_segment_ordinal: 1,
            next_store_id: 1,
            fail_next_commit: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn fail_next_commit(&mut self) {
        self.fail_next_commit = true;
    }

    pub(crate) fn validate(
        &self,
        configuration: &[u8; 32],
        chunk_bits: u32,
    ) -> Result<(), crate::lifecycle_store::StoreRebuildReason> {
        use crate::lifecycle_store::StoreRebuildReason;
        let damage = |message: &str| StoreRebuildReason::Damaged(message.into());
        let format = meta_bytes(&self.database, "format")
            .map_err(|_| damage("meta read failed"))?
            .ok_or(StoreRebuildReason::Missing)?;
        if format.len() != 4
            || u32::from_le_bytes(format[..4].try_into().unwrap())
                != keys::STORE_FORMAT_VERSION
        {
            return Err(StoreRebuildReason::FormatVersion);
        }
        let identity = meta_bytes(&self.database, "configuration")
            .map_err(|_| damage("meta read failed"))?
            .ok_or_else(|| damage("configuration missing"))?;
        if identity.as_slice() != configuration {
            return Err(StoreRebuildReason::Configuration);
        }
        let bits = meta_bytes(&self.database, "chunk_bits")
            .map_err(|_| damage("meta read failed"))?
            .ok_or_else(|| damage("chunk bits missing"))?;
        if bits.len() != 4
            || u32::from_le_bytes(bits[..4].try_into().unwrap()) != chunk_bits
        {
            return Err(StoreRebuildReason::Configuration);
        }
        let complete = meta_bytes(&self.database, "complete")
            .map_err(|_| damage("meta read failed"))?
            .ok_or_else(|| damage("completion flag missing"))?;
        if complete.as_slice() != [1] {
            return Err(damage("incomplete checkpoint"));
        }
        Ok(())
    }

    fn clear_all_rows(&self, write: &WriteTransaction) -> Result<(), String> {
        for definition in [
            keys::COLLECTIONS,
            keys::CATALOG,
            keys::SHARDS,
            keys::ACTIVE,
            keys::FILES,
            keys::RECORDS,
            keys::POSTING_FIELDS,
            keys::POSTINGS,
            keys::BITMAPS,
            keys::SORT_COLUMNS,
            keys::SORT_KEYS,
            keys::COMPOSITES,
            keys::RELATIONS,
            keys::REFERENCES,
            keys::INFERENCE,
        ] {
            let mut table = write.open_table(definition).map_err(store_error)?;
            table
                .retain_in::<&[u8], _>(.., |_, _| false)
                .map_err(store_error)?;
        }
        Ok(())
    }

    pub(crate) fn checkpoint(
        &mut self,
        published: &PublishedIndex,
        revision: u64,
    ) -> Result<crate::lifecycle_store::CheckpointReport, String> {
        let started = std::time::Instant::now();
        let mut report = crate::lifecycle_store::CheckpointReport {
            revision,
            ..Default::default()
        };
        let result = (|| -> Result<(), String> {
            let mut write = self.database.begin_write().map_err(store_error)?;
            write
                .set_durability(if self.durable {
                    Durability::Immediate
                } else {
                    Durability::None
                })
                .map_err(store_error)?;
            {
                let mut meta = write.open_table(keys::META).map_err(store_error)?;
                meta.insert("complete", [0u8].as_slice())
                    .map_err(store_error)?;
            }
            self.clear_all_rows(&write)?;
            let mut next_store_id = self.next_store_id;
            for (number, group) in published.segments.iter().enumerate() {
                for segment in group.iter() {
                    let store_id = next_store_id;
                    next_store_id += 1;
                    write_shard(
                        &write,
                        number as u32,
                        store_id,
                        segment.ordinal,
                        segment.index.as_ref(),
                        &segment.active,
                    )?;
                    report.shards_written += 1;
                }
            }
            for (number, collection) in published.repository.collections().iter().enumerate() {
                let mut table = write.open_table(keys::COLLECTIONS).map_err(store_error)?;
                table
                    .insert(
                        (number as u32).to_be_bytes().as_slice(),
                        encode(&collection.global_inference)?.as_slice(),
                    )
                    .map_err(store_error)?;
            }
            let slots = published.catalog.snapshot_slots();
            let mut catalog = write.open_table(keys::CATALOG).map_err(store_error)?;
            for (number, slot) in slots.iter().enumerate() {
                if let Some(slot) = slot {
                    catalog
                        .insert(
                            (number as u32).to_be_bytes().as_slice(),
                            encode(slot)?.as_slice(),
                        )
                        .map_err(store_error)?;
                }
            }
            drop(catalog);
            {
                let mut meta = write.open_table(keys::META).map_err(store_error)?;
                meta.insert(
                    "format",
                    keys::STORE_FORMAT_VERSION.to_le_bytes().as_slice(),
                )
                .map_err(store_error)?;
                meta.insert("configuration", published.configuration_identity.as_slice())
                    .map_err(store_error)?;
                meta.insert(
                    "chunk_bits",
                    crate::posting_map::POSTING_CHUNK_BITS.to_le_bytes().as_slice(),
                )
                .map_err(store_error)?;
                meta.insert("revision", revision.to_le_bytes().as_slice())
                    .map_err(store_error)?;
                meta.insert(
                    "next_segment_ordinal",
                    published.next_segment_ordinal.to_le_bytes().as_slice(),
                )
                .map_err(store_error)?;
                meta.insert("next_store_id", next_store_id.to_le_bytes().as_slice())
                    .map_err(store_error)?;
                meta.insert("complete", [1u8].as_slice())
                    .map_err(store_error)?;
            }
            #[cfg(test)]
            if self.fail_next_commit {
                self.fail_next_commit = false;
                return Err("index store: injected commit failure".into());
            }
            write.commit().map_err(store_error)?;
            self.revision = revision;
            self.next_store_id = next_store_id;
            self.next_segment_ordinal = published.next_segment_ordinal;
            Ok(())
        })();
        match result {
            Ok(()) => {
                report.duration = started.elapsed();
                Ok(report)
            }
            Err(error) => Err(error),
        }
    }

    pub(crate) fn load_contents(&self) -> Result<StoreContents, String> {
        let revision = meta_get(&self.database, "revision")?
            .as_deref()
            .and_then(le_u64)
            .unwrap_or(0);
        let next_segment_ordinal = meta_get(&self.database, "next_segment_ordinal")?
            .as_deref()
            .and_then(le_u64)
            .unwrap_or(1);
        let next_store_id = meta_get(&self.database, "next_store_id")?
            .as_deref()
            .and_then(le_u64)
            .unwrap_or(1);
        let read = self.database.begin_read().map_err(store_error)?;
        let mut shards = Vec::new();
        {
            let table = read.open_table(keys::SHARDS).map_err(store_error)?;
            for row in table.iter().map_err(store_error)? {
                let (key, _) = row.map_err(store_error)?;
                let key = key.value();
                if key.len() < 12 {
                    return Err("index store: short shard key".into());
                }
                let collection = u32::from_be_bytes(key[..4].try_into().unwrap());
                let store_id = u64::from_be_bytes(key[4..12].try_into().unwrap());
                let (mut shard, active, header) = read_shard(&read, collection, store_id)?;
                if shard.role == crate::build::ShardRole::Main {
                    shard.lineage = crate::build::next_lineage();
                }
                shards.push((header.ordinal, shard, active));
            }
        }
        let mut catalog = BTreeMap::new();
        {
            let table = read.open_table(keys::CATALOG).map_err(store_error)?;
            for row in table.iter().map_err(store_error)? {
                let (key, value) = row.map_err(store_error)?;
                let key = key.value();
                if key.len() != 4 {
                    return Err("index store: invalid catalog key".into());
                }
                let id = u32::from_be_bytes(key.try_into().unwrap());
                catalog.insert(id, decode(value.value())?);
            }
        }
        let catalog_len = catalog.keys().next_back().map_or(0, |id| *id as usize + 1);
        let mut slots = vec![None; catalog_len];
        for (id, slot) in catalog {
            slots[id as usize] = Some(slot);
        }
        let mut global_inference = Vec::new();
        {
            let table = read.open_table(keys::COLLECTIONS).map_err(store_error)?;
            let mut by_collection = BTreeMap::new();
            for row in table.iter().map_err(store_error)? {
                let (key, value) = row.map_err(store_error)?;
                let key = key.value();
                if key.len() != 4 {
                    return Err("index store: invalid collection key".into());
                }
                by_collection.insert(
                    u32::from_be_bytes(key.try_into().unwrap()),
                    decode(value.value())?,
                );
            }
            let len = by_collection
                .keys()
                .next_back()
                .map_or(0, |id| *id as usize + 1);
            global_inference.resize(len, None);
            for (id, summary) in by_collection {
                global_inference[id as usize] = summary;
            }
        }
        Ok(StoreContents {
            revision,
            next_segment_ordinal,
            next_store_id,
            shards,
            catalog: slots,
            global_inference,
        })
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }
}

pub(crate) fn load_index(
    store: IndexStore,
    root: PathBuf,
    definitions: Vec<CollectionDefinition>,
    options: RepositoryOptions,
    sources: SourceRegistry,
) -> Result<Index, String> {
    crate::lifecycle_store::assemble_loaded_index(store, root, definitions, options, sources)
}
